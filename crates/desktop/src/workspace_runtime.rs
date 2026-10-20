use crate::{
    Desktop, Message, drawing_interaction,
    history::HistoryState,
    indicator_manager, market_selector,
    state::Action,
    workspace::{ResetKind, SCHEMA_VERSION, SavedMarket, SavedView, Snapshot},
    workspace_controls::{Controls, Event},
    workspace_store::{SaveResult, Store},
};
use iced::{Subscription, Task, keyboard};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

#[derive(Default)]
pub struct Session {
    pub store: Option<Arc<Store>>,
    pub controls: Controls,
    pub error: Option<String>,
    pub warning: Option<String>,
    pub waiting_market: bool,
    pub restored_view: Option<SavedView>,
    pub reset_pending: bool,
    pub closing: Option<iced::window::Id>,
    enabled: bool,
    track_changes: bool,
    epoch: u64,
    revision: u64,
    saved_revision: u64,
    dirty_at: Option<Instant>,
    in_flight: Option<u64>,
    reset_kind: Option<ResetKind>,
    reset_retry: Option<(ResetKind, Arc<Snapshot>, u64)>,
}

impl Session {
    pub fn mark_dirty(&mut self) {
        if (self.enabled || self.track_changes) && !self.reset_pending && self.reset_retry.is_none()
        {
            let Some(revision) = self.revision.checked_add(1) else {
                self.dirty_at = Some(Instant::now());
                self.error =
                    Some("Workspace revision IDs exhausted; changes are not saved.".into());
                return;
            };
            self.revision = revision;
            self.dirty_at = Some(Instant::now());
        }
    }
    pub fn description(&self) -> String {
        let path = self.store.as_ref().map_or_else(
            || "Ephemeral storage".to_owned(),
            |store| store.path().display().to_string(),
        );
        let state = if !self.enabled {
            "Saving unavailable; existing files preserved"
        } else if self.waiting_market {
            "Validating restored native market"
        } else if self.reset_pending {
            "Committing reset"
        } else if self.in_flight.is_some() {
            "Saving"
        } else if self.dirty_at.is_some() {
            "Unsaved changes"
        } else {
            "Saved configuration; native data is never stored"
        };
        format!(
            "{path}\n{state}{}{}",
            self.warning
                .as_ref()
                .map_or_else(String::new, |warning| format!("\n{warning}")),
            self.error
                .as_ref()
                .map_or_else(String::new, |error| format!("\n{error}"))
        )
    }
}
pub(crate) fn durable_action(action: &Action) -> bool {
    !matches!(
        action,
        Action::Inspect(_)
            | Action::InspectPane { .. }
            | Action::InspectStep(_)
            | Action::Older
            | Action::Retry
            | Action::ManageIndicators
            | Action::ManageMarket
            | Action::ScrollPanes { .. }
            | Action::TogglePaneMaximized(_)
    )
}

impl Desktop {
    pub(crate) fn launch() -> (Self, Task<Message>) {
        let (mut desktop, startup) = Self::new();
        if desktop.stress.is_some() {
            return (desktop, startup);
        }
        // These tasks have not been polled: restore before any native feed starts.
        drop(startup);
        desktop.history = HistoryState::default();
        desktop.workspace.track_changes = true;
        match Store::open_default() {
            Ok(opened) => {
                desktop.workspace.warning = opened.warning;
                desktop.workspace.enabled = opened.writable;
                desktop.workspace.store = Some(Arc::new(opened.store));
                if let Some(snapshot) = opened.snapshot
                    && let Err(error) = desktop.install_workspace(&snapshot)
                {
                    desktop.workspace.error = Some(error);
                }
            }
            Err(error) => {
                desktop.workspace.error = Some(error);
                desktop.workspace.warning = Some("Workspace persistence is unavailable; changes cannot be saved. No existing files will be overwritten.".into());
            }
        }
        desktop.workspace.waiting_market = true;
        let market = desktop.chart.market.clone();
        let focus = desktop.selector.show_exact(&market);
        let catalog = desktop.load_catalog(market.exchange, market.product);
        (desktop, Task::batch([focus, catalog]))
    }

    fn install_workspace(&mut self, snapshot: &Snapshot) -> Result<(), String> {
        snapshot.validate()?;
        self.chart.restore_workspace(&snapshot.chart)?;
        self.drawings
            .restore_workspace(&self.chart.market, &snapshot.books)?;
        self.selector
            .restore_quick_markets(&snapshot.recent_markets, &snapshot.favorite_markets)?;
        self.reset_profile_runtime();
        self.drawing_interaction = drawing_interaction::Interaction::default();
        self.drawing_interaction.set_magnet(snapshot.magnet);
        self.workspace.epoch = snapshot.reset_epoch;
        self.workspace.restored_view = Some(snapshot.chart.view.clone());
        self.pane_caches.clear();
        for instance in &self.chart.indicators.instances {
            if !instance.spec.is_overlay() {
                self.pane_caches.entry(instance.id).or_default();
            }
        }
        self.clear_geometry();
        Ok(())
    }

    fn workspace_snapshot(&self) -> Snapshot {
        let mut chart = self.chart.workspace_snapshot();
        if let Some(view) = &self.workspace.restored_view {
            chart.view = view.clone();
        }
        Snapshot {
            schema_version: SCHEMA_VERSION,
            reset_epoch: self.workspace.epoch,
            chart,
            books: self.drawings.workspace_books(),
            recent_markets: self
                .selector
                .recent_markets()
                .iter()
                .map(SavedMarket::from)
                .collect(),
            favorite_markets: self
                .selector
                .favorite_markets()
                .iter()
                .map(SavedMarket::from)
                .collect(),
            magnet: self.drawing_interaction.magnet(),
        }
    }

    pub(crate) fn complete_workspace_market_validation(&mut self) -> Task<Message> {
        let Some(market) = self.selector.take_ready_quick() else {
            return Task::none();
        };
        if market != self.chart.market {
            return self
                .selector
                .catalog()
                .cloned()
                .map_or_else(Task::none, |catalog| self.switch_market(market, catalog));
        }
        self.workspace.waiting_market = false;
        self.selector.open = false;
        self.sync_indicator_sources();
        if self.workspace.enabled {
            // Create the first checkpoint even for a fresh, empty workspace.
            self.workspace.mark_dirty();
        }
        Task::batch([
            self.fetch(crate::history::Request::Recent),
            self.ensure_source_catalogs(),
        ])
    }

    pub(crate) fn workspace_keyboard_subscription() -> Subscription<Message> {
        iced::event::listen_with(|event, status, _| {
            if let iced::Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) = event
            {
                Some(Message::Key {
                    key,
                    modifiers,
                    captured: status == iced::event::Status::Captured,
                })
            } else {
                None
            }
        })
    }

    pub(crate) fn workspace_save_subscription(&self) -> Subscription<Message> {
        if self.workspace.dirty_at.is_some()
            && self.workspace.enabled
            && self.workspace.error.is_none()
            && !self.workspace.reset_pending
        {
            iced::time::every(Duration::from_millis(100)).map(|_| Message::WorkspacePoll)
        } else {
            Subscription::none()
        }
    }

    pub(crate) fn poll_workspace_save(&mut self, force: bool) -> Task<Message> {
        if self.workspace.reset_pending || self.workspace.in_flight.is_some() {
            return Task::none();
        }
        if force && self.workspace.reset_retry.is_some() {
            return self.workspace_event(Event::RetrySave);
        }
        let dirty = self.workspace.dirty_at.is_some();
        if !dirty {
            return self
                .workspace
                .closing
                .map_or_else(Task::none, iced::window::close);
        }
        if force && self.workspace.error.is_some() {
            self.workspace
                .controls
                .show_save_error(self.workspace.error.as_deref().unwrap());
            return Task::none();
        }
        if !self.workspace.enabled {
            if force {
                self.workspace.error = Some("Workspace changes cannot be saved while storage is unavailable or a newer schema is protected.".into());
                self.workspace
                    .controls
                    .show_save_error(self.workspace.error.as_deref().unwrap());
            }
            return Task::none();
        }
        if !force
            && (self.workspace.error.is_some()
                || self
                    .workspace
                    .dirty_at
                    .is_none_or(|time| time.elapsed() < Duration::from_millis(500)))
        {
            return Task::none();
        }
        let Some(store) = self.workspace.store.clone() else {
            return Task::none();
        };
        let snapshot = self.workspace_snapshot();
        if let Err(error) = snapshot.validate() {
            self.workspace.error = Some(error);
            if force {
                self.workspace
                    .controls
                    .show_save_error(self.workspace.error.as_deref().unwrap());
            }
            return Task::none();
        }
        let revision = self.workspace.revision;
        self.workspace.in_flight = Some(revision);
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || store.save(revision, &snapshot))
                    .await
                    .unwrap_or_else(|error| Err(format!("Workspace writer failed: {error}")))
            },
            move |result| Message::WorkspaceSaved { revision, result },
        )
    }

    pub(crate) fn workspace_saved(
        &mut self,
        revision: u64,
        result: Result<SaveResult, String>,
    ) -> Task<Message> {
        if self.workspace.in_flight == Some(revision) {
            self.workspace.in_flight = None;
        }
        if revision < self.workspace.saved_revision
            || self.workspace.reset_pending
            || self.workspace.reset_retry.is_some()
        {
            return Task::none();
        }
        match result {
            Ok(saved) if saved.reset_epoch == self.workspace.epoch => {
                self.workspace.saved_revision = saved.revision;
                self.workspace.error = None;
                if self.workspace.revision == revision {
                    self.workspace.dirty_at = None;
                }
                if self.workspace.closing.is_some() {
                    return self.poll_workspace_save(true);
                }
            }
            Ok(_) => {}
            Err(error) => {
                self.workspace.error = Some(error.clone());
                if self.workspace.closing.is_some() {
                    self.workspace.controls.show_save_error(&error);
                }
            }
        }
        Task::none()
    }

    pub(crate) fn workspace_event(&mut self, event: Event) -> Task<Message> {
        match event {
            Event::Open(kind) => {
                if self.workspace.reset_pending {
                    return Task::none();
                }
                self.workspace.reset_kind = Some(kind);
                let drawings = if kind == ResetKind::All {
                    self.drawings.total_objects()
                } else {
                    self.drawings.objects().len()
                };
                let profiles = self
                    .chart
                    .profiles
                    .iter()
                    .filter(|profile| {
                        kind == ResetKind::All
                            || profile.instrument.as_ref().is_none_or(|bound| {
                                bound.exchange == self.chart.market.exchange
                                    && bound.product == self.chart.market.product
                                    && bound.symbol == self.chart.market.symbol
                            })
                    })
                    .count();
                self.workspace.controls.show(
                    kind,
                    &format!(
                        "{} {} {} / {}",
                        self.chart.market.exchange,
                        self.chart.market.product,
                        self.chart.market.symbol,
                        self.chart.market.interval
                    ),
                    self.chart.indicators.instances.len(),
                    profiles,
                    drawings,
                );
                self.drawing_interaction.suspend_overlay();
            }
            Event::Cancel => {
                if !self.workspace.reset_pending {
                    if self
                        .workspace
                        .reset_retry
                        .as_ref()
                        .is_some_and(|(_, snapshot, _)| {
                            self.workspace.store.as_ref().is_some_and(|store| {
                                store.reset_is_authoritative(snapshot.reset_epoch)
                            })
                        })
                    {
                        return Task::none();
                    }
                    self.workspace.reset_retry = None;
                    self.workspace.controls.close();
                    self.workspace.closing = None;
                }
            }
            Event::Confirm => {
                if self.workspace.reset_pending || !self.workspace.controls.is_open() {
                    return Task::none();
                }
                if self.workspace.reset_retry.is_some() {
                    return self.workspace_event(Event::RetrySave);
                }
                let Some(kind) = self.workspace.reset_kind else {
                    return Task::none();
                };
                if kind == ResetKind::Current
                    && self.workspace.store.is_some()
                    && !self.workspace.enabled
                {
                    self.workspace.error = Some("A newer workspace cannot be cleared partially. Use a newer app, or explicitly Reset everything.".into());
                    return Task::none();
                }
                let snapshot = match self.workspace_snapshot().cleared(kind) {
                    Ok(snapshot) => snapshot,
                    Err(error) => {
                        self.workspace.error = Some(error);
                        return Task::none();
                    }
                };
                self.workspace.revision = self.workspace.revision.saturating_add(1);
                return self.commit_workspace_reset(
                    kind,
                    Arc::new(snapshot),
                    self.workspace.revision,
                );
            }
            Event::RetrySave => {
                self.workspace.error = None;
                if let Some((kind, snapshot, revision)) = self.workspace.reset_retry.clone() {
                    return self.commit_workspace_reset(kind, snapshot, revision);
                }
                if !self.workspace.enabled {
                    self.workspace.error = Some("Workspace writer is unavailable or a newer schema is protected; cannot retry ordinary saving.".into());
                    if self.workspace.closing.is_some() {
                        self.workspace
                            .controls
                            .show_save_error(self.workspace.error.as_deref().unwrap());
                    }
                    return Task::none();
                }
                self.workspace.controls.close();
                self.workspace.mark_dirty();
                return self.poll_workspace_save(true);
            }
            Event::ExitWithoutSaving => {
                if let Some(id) = self.workspace.closing {
                    return iced::window::close(id);
                }
                if self.workspace.reset_retry.is_some() {
                    return iced::exit();
                }
            }
        }
        Task::none()
    }

    fn commit_workspace_reset(
        &mut self,
        kind: ResetKind,
        snapshot: Arc<Snapshot>,
        revision: u64,
    ) -> Task<Message> {
        let Some(store) = self.workspace.store.clone() else {
            self.workspace.error = Some(
                "Workspace storage is unavailable; a persistent reset cannot be committed.".into(),
            );
            return Task::none();
        };
        self.workspace.reset_pending = true;
        self.workspace.reset_retry = Some((kind, snapshot.clone(), revision));
        let callback_snapshot = snapshot.clone();
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || match kind {
                    ResetKind::Current => store.reset(revision, &snapshot),
                    ResetKind::All => store.reset_with_kind(revision, &snapshot, kind),
                })
                .await
                .unwrap_or_else(|error| Err(format!("Workspace reset writer failed: {error}")))
            },
            move |result| Message::WorkspaceResetDone {
                revision,
                kind,
                snapshot: callback_snapshot.clone(),
                result,
            },
        )
    }

    pub(crate) fn workspace_reset_done(
        &mut self,
        revision: u64,
        kind: ResetKind,
        snapshot: Arc<Snapshot>,
        result: Result<SaveResult, String>,
    ) -> Task<Message> {
        self.workspace.reset_pending = false;
        let authoritative = result.is_ok()
            || self
                .workspace
                .store
                .as_ref()
                .is_some_and(|store| store.reset_is_authoritative(snapshot.reset_epoch));
        let mut task = Task::none();
        if authoritative {
            self.workspace.enabled = self
                .workspace
                .store
                .as_ref()
                .is_some_and(|store| store.writable());
            self.restart_connection();
            self.reset_source_discovery();
            self.source_requests.clear();
            self.source_inputs.clear();
            self.open_interest_history.reset();
            self.manager = indicator_manager::Manager::default();
            self.selector = market_selector::Selector::default();
            self.drawing_controls.close();
            self.drawing_interaction.cancel();
            self.commands = None;
            self.command_pending = false;
            self.command_focus = None;
            self.help_open = false;
            self.details = None;
            self.goto_time.open = false;
            self.goto_target = None;
            self.goto_page_oldest = None;
            self.goto_notice = None;
            self.drawing_projection.set(None);
            self.drawing_error = None;
            if let Err(error) = self.install_workspace(&snapshot) {
                self.workspace.error = Some(error);
                return Task::none();
            }
            self.workspace.revision = revision;
            self.workspace.saved_revision = revision;
            self.workspace.dirty_at = None;
            self.workspace.waiting_market = true;
            self.active_catalog = None;
            self.native_quote = "native quote".into();
            self.native_volume_unit = "native volume".into();
            let market = self.chart.market.clone();
            task = Task::batch([
                self.selector.show_exact(&market),
                self.load_catalog(market.exchange, market.product),
            ]);
        }
        match result {
            Ok(_) => {
                self.workspace.reset_retry = None;
                self.workspace.warning = None;
                self.workspace.error = None;
                self.workspace.controls.close();
                if let Some(id) = self.workspace.closing {
                    return iced::window::close(id);
                }
            }
            Err(error) => {
                if authoritative {
                    self.workspace.reset_retry = Some((kind, snapshot, revision));
                }
                self.workspace.error = Some(error.clone());
                if authoritative || self.workspace.closing.is_some() {
                    self.workspace.controls.show_save_error(&error);
                }
            }
        }
        task
    }
}

#[cfg(test)]
#[path = "workspace_runtime_tests.rs"]
mod tests;
