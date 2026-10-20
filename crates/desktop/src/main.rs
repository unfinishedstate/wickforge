mod chart;
mod chart_surface;
mod command_palette;
mod drawing_controls;
mod drawing_geometry;
mod drawing_interaction;
mod drawing_layer;
mod drawing_types;
mod drawings;
mod feeds;
mod goto_time;
mod history;
mod indicator_manager;
mod key_help;
mod market_selector;
mod palette;
mod pane_resize;
mod pointer_input;
mod presentation;
mod profile_controls;
mod profile_layer;
mod profiles_runtime;
mod source_controller;
mod source_feeds;
mod state;
mod stress;
mod ui;
mod workspace;
mod workspace_controls;
mod workspace_runtime;
mod workspace_store;

use arrayvec::ArrayVec;
use drawing_types::{Command as DrawingCommand, DrawingId, Tool as DrawingTool};
use history::{HistoryState, OpenInterestHistoryState, Request, repair_request};
use iced::widget::{
    button, canvas, column, container, mouse_area, pick_list, row, scrollable, text, tooltip,
};
use iced::{Element, Length, Subscription, Task, Theme, keyboard};
use mexc_data::DataService;
use mexc_indicators::profiles::{
    Instance as ProfileInstance, Range as ProfileRangeSpec, Spec as ProfileSpec,
};
use mexc_indicators::{IndicatorSource, IndicatorSpec, SourceData};
use mexc_model::executions::NativeInstrument;
use mexc_model::{
    Catalog, Exchange, FlowBar, HistoryEvent, HistoryPage, Market, MarketEvent, OpenInterestEvent,
    OpenInterestPage, OpenInterestSample, Product, Quote, Timeframe,
};
use state::{Action, ChartState};
use std::cell::Cell;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
enum Message {
    Workspace(workspace_controls::Event),
    WorkspacePoll,
    WorkspaceSaved {
        revision: u64,
        result: Result<workspace_store::SaveResult, String>,
    },
    WorkspaceResetDone {
        revision: u64,
        kind: workspace::ResetKind,
        snapshot: Arc<workspace::Snapshot>,
        result: Result<workspace_store::SaveResult, String>,
    },
    CloseRequested(iced::window::Id),
    PointerCancelled,
    Action(Action),
    DrawingInput {
        generation: u64,
        event: drawing_interaction::Input,
    },
    DrawingBlank {
        generation: u64,
    },
    DrawingCommand {
        generation: u64,
        command: DrawingCommand,
    },
    DrawingControl(drawing_controls::Event),
    DrawingMenu,
    DrawingProperties(DrawingId),
    DrawingCancel,
    DrawingClose,
    Manager(indicator_manager::Event),
    Selector(market_selector::Event),
    SelectInterval(Timeframe),
    IndicatorSettings(u64),
    IndicatorDetails(u64),
    StatusDetails,
    DetailsClose,
    EditorClose,
    ModalBackdrop,
    UseChartSource(u64),
    RetryIndicatorSources(u64),
    RetryContributor {
        id: u64,
        market: Market,
    },
    RetrySourceDiscovery {
        id: u64,
        exchange: Exchange,
        product: Product,
    },
    CommandOpen,
    CommandFocused(Option<iced::advanced::widget::Id>),
    CommandPalette(command_palette::Event),
    GotoTimeOpen,
    GotoTime(goto_time::Event),
    QuickMarket(Market),
    StressPulse,
    StressFrame(Instant),
    StressFinish,
    Key {
        key: keyboard::Key,
        modifiers: keyboard::Modifiers,
        captured: bool,
    },
    HelpOpen,
    HelpClose,
    HelpActivate,
    HelpAction(Action),
    HelpBackdrop,
    MarketData(feeds::Event),
    OpenInterest(feeds::OpenInterestBatch),
    OpenInterestHistory {
        market: Market,
        generation: u64,
        open_interest_generation: u64,
        before_ms: i64,
        result: Result<OpenInterestPage, String>,
    },
    History {
        request: Request,
        market: Market,
        generation: u64,
        history_id: u64,
        epoch: u64,
        result: Result<HistoryPage, String>,
    },
    HistoryFlow {
        request: Request,
        market: Market,
        generation: u64,
        history_id: u64,
        epoch: u64,
        flow_generation: u64,
        events: Result<Box<[HistoryEvent]>, String>,
    },
    Catalog {
        exchange: Exchange,
        product: Product,
        generation: u64,
        result: Result<Arc<Catalog>, String>,
    },
    SourceCatalog {
        exchange: Exchange,
        product: Product,
        generation: u64,
        result: Result<Arc<Catalog>, String>,
    },
    SourceData(source_feeds::Batch),
    Profile(profile_controls::Event),
    ProfileEntry(indicator_manager::ProfileEntry),
    ProfileEdit(profile_layer::Edit),
    ProfileData(profiles_runtime::Event),
    Tick,
}

#[derive(Debug, Clone, Copy)]
enum Details {
    Status,
    Indicator(u64),
}

struct ProfileRange {
    id: Option<u64>,
    spec: ProfileSpec,
    first: Option<i64>,
    cursor_ms: i64,
}

struct Desktop {
    chart: ChartState,
    cache: canvas::Cache,
    pane_caches: BTreeMap<u64, canvas::Cache>,
    drawings: drawings::Store,
    drawing_interaction: drawing_interaction::Interaction,
    pointer_generation: u64,
    drawing_controls: drawing_controls::Controls,
    drawing_scene: drawing_layer::Scene,
    drawing_projection: Cell<Option<drawing_geometry::Projection>>,
    drawing_error: Option<String>,
    manager: indicator_manager::Manager,
    profiles: profiles_runtime::Runtime,
    profile_controls: profile_controls::Controls,
    profile_scene: profile_layer::Scene,
    profile_range: Option<ProfileRange>,
    selected_profile: Option<u64>,
    next_profile_id: u64,
    selector: market_selector::Selector,
    active_catalog: Option<Arc<Catalog>>,
    help_open: bool,
    help_selection: usize,
    details: Option<Details>,
    commands: Option<command_palette::Commands>,
    command_focus: Option<iced::advanced::widget::Id>,
    command_pending: bool,
    goto_time: goto_time::Editor,
    goto_target: Option<i64>,
    goto_page_oldest: Option<i64>,
    goto_notice: Option<String>,
    vwap_backfill_cursor: Option<(u64, i64)>,
    stress: Option<stress::Metrics>,
    theme_file: palette::ThemeFile,
    data: DataService,
    market_generation: u64,
    flow_generation: u64,
    catalog_generation: u64,
    native_quote: String,
    native_volume_unit: String,
    upstream_connected: bool,
    status: String,
    history: HistoryState,
    open_interest_history: OpenInterestHistoryState,
    flow_status: Option<String>,
    source_catalogs: source_controller::Catalogs,
    source_catalog_pending: source_controller::PendingCatalogs,
    source_catalog_generation: u64,
    source_requests: source_controller::Requests,
    source_inputs: HashMap<Market, SourceData>,
    next_source_generation: u64,
    workspace: workspace_runtime::Session,
}

impl Desktop {
    fn new() -> (Self, Task<Message>) {
        let data = DataService::new().expect("valid native data configuration");
        let chart = ChartState::default();
        let drawings = drawings::Store::new(&chart.market);
        let mut desktop = Self {
            chart,
            cache: canvas::Cache::new(),
            pane_caches: BTreeMap::new(),
            drawings,
            drawing_interaction: Default::default(),
            pointer_generation: 0,
            drawing_controls: Default::default(),
            drawing_scene: Default::default(),
            drawing_projection: Cell::new(None),
            drawing_error: None,
            manager: indicator_manager::Manager::default(),
            profiles: Default::default(),
            profile_controls: Default::default(),
            profile_scene: Default::default(),
            profile_range: None,
            selected_profile: None,
            next_profile_id: 1,
            selector: market_selector::Selector::default(),
            active_catalog: None,
            help_open: false,
            help_selection: 0,
            details: None,
            commands: None,
            command_focus: None,
            command_pending: false,
            goto_time: Default::default(),
            goto_target: None,
            goto_page_oldest: None,
            goto_notice: None,
            vwap_backfill_cursor: None,
            stress: stress::Scenario::from_args().map(stress::Metrics::new),
            theme_file: palette::ThemeFile::new(),
            data,
            catalog_generation: 0,
            market_generation: 0,
            flow_generation: 0,
            upstream_connected: false,
            status: "Connecting to native exchange…".into(),
            history: HistoryState::default(),
            open_interest_history: OpenInterestHistoryState::default(),
            flow_status: None,
            native_quote: "native quote".into(),
            native_volume_unit: "native volume".into(),
            source_catalogs: Default::default(),
            source_catalog_pending: Default::default(),
            source_catalog_generation: 0,
            source_requests: Default::default(),
            source_inputs: Default::default(),
            next_source_generation: 0,
            workspace: Default::default(),
        };
        desktop.selector.observe_market(&desktop.chart.market);
        if let Some(metrics) = &desktop.stress {
            stress::populate(
                &mut desktop.chart,
                &mut desktop.drawings,
                &mut desktop.pane_caches,
                metrics.scenario,
            );
            desktop.native_quote = "synthetic price".into();
            desktop.native_volume_unit = "synthetic units".into();
            desktop.status = "Synthetic benchmark fixture; native feeds are disabled".into();
            return (
                desktop,
                Task::perform(
                    async {
                        tokio::time::sleep(Duration::from_secs(20)).await;
                    },
                    |_| Message::StressFinish,
                ),
            );
        }
        let startup = Task::batch([
            desktop.fetch(Request::Recent),
            desktop.load_catalog(desktop.chart.market.exchange, desktop.chart.market.product),
        ]);
        (desktop, startup)
    }

    fn profile_tick(&self) -> Option<rust_decimal::Decimal> {
        self.active_catalog
            .as_deref()?
            .instruments
            .iter()
            .find(|instrument| {
                instrument.exchange == self.chart.market.exchange
                    && instrument.product == self.chart.market.product
                    && instrument.symbol == self.chart.market.symbol
            })
            .and_then(|instrument| instrument.tick_size)
    }

    fn sync_profiles(&mut self) {
        if self.workspace.waiting_market || self.stress.is_some() || self.chart.latest().is_none() {
            return;
        }
        let before = self.profiles.revision;
        let view = self.chart.viewport;
        let tick = self.profile_tick();
        self.profiles.sync(
            &self.chart.market,
            tick,
            &self.chart.profiles,
            (
                (view.left * 1000.0) as i64,
                ((view.left + view.span) * 1000.0) as i64,
            ),
        );
        if before != self.profiles.revision {
            self.profile_statuses();
        }
    }

    fn profile_statuses(&mut self) {
        if !self.profile_controls.open {
            return;
        }
        for instance in &self.chart.profiles {
            if let Some(bound) = &instance.instrument
                && (bound.exchange != self.chart.market.exchange
                    || bound.product != self.chart.market.product
                    || bound.symbol != self.chart.market.symbol)
            {
                self.profile_controls.set_status(instance.id, Some(format!(
                    "Dormant: bound to {} · {} · {}. Switch to that native instrument to compute; no source is substituted.",
                    bound.exchange, bound.product, bound.symbol)));
                continue;
            }
            let status = self.profiles.errors.get(&instance.id).cloned().or_else(|| {
                self.profiles
                    .outputs
                    .get(&instance.id)
                    .and_then(|rows| rows.last())
                    .and_then(|profile| profile.reason.clone())
            });
            let source = self.profiles.sources.status(instance.spec.kind);
            let status = Some(format!(
                "{}History: {} · Live: {}",
                status.map_or(String::new(), |value| format!("{value} · ")),
                source.history,
                source.live
            ));
            let status = if instance.spec.kind == mexc_indicators::profiles::Kind::Volume {
                Some(format!(
                    "Native volume unit: {}{}",
                    self.native_volume_unit,
                    status
                        .as_ref()
                        .map_or(String::new(), |value| format!(" · {value}"))
                ))
            } else {
                status
            };
            self.profile_controls.set_status(instance.id, status);
        }
    }

    fn profile_inspection(&self) -> Option<Element<'static, Message>> {
        if self.chart.active_pane.is_some() {
            return None;
        }
        let (time, price) = self.chart.inspection?;
        let tick = rust_decimal::prelude::ToPrimitive::to_f64(&self.profile_tick()?)?;
        let mut values = row![].spacing(8);
        let mut count = 0;
        for instance in self
            .chart
            .profiles
            .iter()
            .filter(|instance| instance.visible)
        {
            let Some(snapshots) = self.profiles.outputs.get(&instance.id) else {
                continue;
            };
            let snapshot = match instance.spec.range {
                ProfileRangeSpec::Session(_) => snapshots.iter().find(|profile| {
                    profile.start_ms <= time.saturating_mul(1000)
                        && profile.end_ms > time.saturating_mul(1000)
                }),
                _ => snapshots.last(),
            };
            let Some(profile) = snapshot else {
                continue;
            };
            let at = profile.rows.partition_point(|row| row.low <= price);
            let Some(value) = at
                .checked_sub(1)
                .and_then(|index| profile.rows.get(index))
                .filter(|row| price < row.high + tick)
            else {
                continue;
            };
            let quantity = if instance.spec.kind == mexc_indicators::profiles::Kind::Tpo {
                format!("{} blocks", value.blocks.len())
            } else {
                format!(
                    "{} {}",
                    presentation::quantity(value.weight),
                    self.native_volume_unit
                )
            };
            values = values.push(
                text(format!(
                    "#{} {} · {}{}",
                    instance.id,
                    instance.spec.kind,
                    quantity,
                    match profile.volume_method {
                        mexc_indicators::profiles::VolumeMethod::Estimated => " · estimated",
                        mexc_indicators::profiles::VolumeMethod::Mixed => " · mixed",
                        _ if !profile.covered => " · observed",
                        _ => "",
                    }
                ))
                .size(11)
                .wrapping(iced::widget::text::Wrapping::None)
                .style(ui::muted_text),
            );
            count += 1;
        }
        (count > 0).then(|| values.into())
    }

    fn reset_profile_runtime(&mut self) {
        self.profiles.reset();
        self.profile_scene.clear();
        self.profile_controls = Default::default();
        self.profile_range = None;
        self.selected_profile = None;
        self.next_profile_id = self
            .chart
            .profiles
            .iter()
            .map(|instance| instance.id)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        self.pointer_generation = self.pointer_generation.wrapping_add(1);
    }

    fn put_profile(
        &mut self,
        id: Option<u64>,
        spec: ProfileSpec,
        placed: bool,
    ) -> Result<u64, String> {
        spec.validate()?;
        let key = NativeInstrument::from_market(&self.chart.market);
        let id = if let Some(id) = id {
            let instance = self
                .chart
                .profiles
                .iter_mut()
                .find(|instance| instance.id == id)
                .ok_or("This profile was removed")?;
            let binding = if matches!(spec.range, ProfileRangeSpec::Fixed { .. }) {
                if placed {
                    Some(key)
                } else {
                    instance.instrument.clone().or(Some(key))
                }
            } else {
                None
            };
            instance.spec = spec;
            instance.instrument = binding;
            id
        } else {
            let id = self.next_profile_id;
            let next = id
                .checked_add(1)
                .filter(|_| id != 0)
                .ok_or("Profile IDs are exhausted")?;
            self.next_profile_id = next;
            let instrument = matches!(spec.range, ProfileRangeSpec::Fixed { .. }).then_some(key);
            self.chart.profiles.push(ProfileInstance {
                id,
                spec,
                visible: true,
                instrument,
            });
            id
        };
        self.workspace.mark_dirty();
        self.profile_scene.clear();
        self.profile_controls.applied(&self.chart.profiles, id);
        Ok(id)
    }

    fn profile_entry(&mut self, entry: indicator_manager::ProfileEntry) -> Task<Message> {
        use indicator_manager::ProfileEntry;
        use mexc_indicators::profiles::Kind;
        if self.help_open || self.workspace.controls.is_open() || self.goto_time.open {
            return Task::none();
        }
        let kind = if matches!(entry, ProfileEntry::Tpo) {
            Kind::Tpo
        } else {
            Kind::Volume
        };
        let _ = self.profile_event(profile_controls::Event::New(kind));
        if self.profile_tick().is_none() {
            self.profile_controls.set_error(
                "Native tick-size metadata is unavailable; no price increment will be guessed."
                    .into(),
            );
            return self.profile_controls.focus();
        }
        if matches!(entry, ProfileEntry::SelectedVolume) {
            let _ = self.profile_event(profile_controls::Event::Mode(
                profile_controls::RangeMode::Fixed,
            ));
            return self.profile_event(profile_controls::Event::PlaceRange);
        }
        if matches!(entry, ProfileEntry::VisibleVolume) {
            let _ = self.profile_event(profile_controls::Event::Mode(
                profile_controls::RangeMode::Visible,
            ));
        }
        self.profile_event(profile_controls::Event::Apply)
    }

    fn edit_profile(&mut self, edit: profile_layer::Edit) -> Task<Message> {
        if self.help_open
            || self.workspace.controls.is_open()
            || self.goto_time.open
            || self.manager.open
            || self.selector.open
            || self.profile_controls.open
        {
            return Task::none();
        }
        let id = match edit {
            profile_layer::Edit::Select(id) | profile_layer::Edit::Range { id, .. } => id,
        };
        if self.profile_bound_elsewhere(id) {
            self.profile_controls.show(&self.chart.profiles, Some(id));
            self.profile_controls.set_error(
                "Switch to this fixed profile's bound native instrument before editing its range."
                    .into(),
            );
            return self.profile_controls.focus();
        }
        let Some(instance) = self.chart.profiles.iter().find(|instance| {
            instance.id == id && matches!(instance.spec.range, ProfileRangeSpec::Fixed { .. })
        }) else {
            return Task::none();
        };
        let mut spec = instance.spec.clone();
        if let profile_layer::Edit::Range {
            start_ms, end_ms, ..
        } = edit
        {
            spec.range = ProfileRangeSpec::Fixed { start_ms, end_ms };
            if let Err(error) = self.put_profile(Some(id), spec, false) {
                self.profile_controls.set_error(error);
                return Task::none();
            }
        }
        self.selected_profile = Some(id);
        self.drawing_controls.close();
        self.drawing_interaction.choose_tool(DrawingTool::Select);
        self.drawings.select(None);
        self.chart.dispatch(Action::ActivatePane(None));
        Task::none()
    }

    fn profile_point(&mut self, time_ms: i64) -> Task<Message> {
        let Some(placement) = &mut self.profile_range else {
            return Task::none();
        };
        let time_ms = if placement.spec.kind == mexc_indicators::profiles::Kind::Tpo {
            time_ms.div_euclid(60_000).saturating_mul(60_000)
        } else {
            time_ms
        };
        placement.cursor_ms = time_ms;
        let Some(first) = placement.first else {
            placement.first = Some(time_ms);
            return Task::none();
        };
        if first == time_ms {
            return Task::none();
        }
        let mut placement = self.profile_range.take().unwrap();
        placement.spec.range = ProfileRangeSpec::Fixed {
            start_ms: first.min(time_ms),
            end_ms: first.max(time_ms),
        };
        match self.put_profile(placement.id, placement.spec, true) {
            Ok(id) => self.selected_profile = Some(id),
            Err(error) => self.profile_controls.set_error(error),
        }
        self.pointer_generation = self.pointer_generation.wrapping_add(1);
        Task::none()
    }

    fn profile_bound_elsewhere(&self, id: u64) -> bool {
        self.chart
            .profiles
            .iter()
            .find(|profile| profile.id == id)
            .and_then(|profile| profile.instrument.as_ref())
            .is_some_and(|bound| {
                bound.exchange != self.chart.market.exchange
                    || bound.product != self.chart.market.product
                    || bound.symbol != self.chart.market.symbol
            })
    }

    fn profile_event(&mut self, event: profile_controls::Event) -> Task<Message> {
        use profile_controls::{Command, Event};
        if self.help_open || self.workspace.controls.is_open() {
            return Task::none();
        }
        if let Event::RangePoint(time_ms) = event {
            return self.profile_point(time_ms);
        }
        if let Event::RangeCursor(time_ms) = event {
            if let Some(placement) = &mut self.profile_range {
                placement.cursor_ms = if placement.spec.kind == mexc_indicators::profiles::Kind::Tpo
                {
                    time_ms.div_euclid(60_000).saturating_mul(60_000)
                } else {
                    time_ms
                };
            }
            return Task::none();
        }
        if matches!(event, Event::RangeCancel) {
            self.profile_range = None;
            self.pointer_generation = self.pointer_generation.wrapping_add(1);
            return Task::none();
        }
        if matches!(event, Event::Open(_) | Event::New(_)) {
            self.profile_range = None;
            self.drawing_interaction.cancel();
            self.drawing_controls.close();
            self.manager.open = false;
            self.selector.open = false;
            self.details = None;
            self.goto_time.open = false;
            self.pointer_generation = self.pointer_generation.wrapping_add(1);
        }
        let quick = matches!(event, Event::QuickAdd(_));
        let focus = matches!(
            event,
            Event::Open(_)
                | Event::New(_)
                | Event::Select(_)
                | Event::Kind(_)
                | Event::Mode(_)
                | Event::Period(_)
        );
        let suggest = matches!(event, Event::New(_))
            || matches!(event, Event::Open(None)) && self.chart.profiles.is_empty();
        let command = self.profile_controls.update(event, &self.chart.profiles);
        if suggest && let Some(tick) = self.profile_tick() {
            self.profile_controls
                .suggest_rows(profiles_runtime::suggested_rows(tick, self.chart.candles()));
        }
        let result: Result<(), String> = match command {
            Some(Command::Rebin(id) | Command::PlaceRange { id: Some(id), .. })
                if self.profile_bound_elsewhere(id) => {
                Err("Switch to this fixed profile's bound native instrument before rebinning or placing its range".into())
            }
            Some(Command::Add(mut spec)) => {
                if quick {
                    if let Some(tick) = self.profile_tick() {
                        spec.ticks_per_row =
                            profiles_runtime::suggested_rows(tick, self.chart.candles());
                    } else {
                        self.profile_controls
                            .update(Event::New(spec.kind), &self.chart.profiles);
                        self.profile_controls.set_error("Native tick-size metadata is unavailable; no price increment will be guessed.".into());
                        return Task::none();
                    }
                }
                self.put_profile(None, spec, false).map(|_| ())
            }
            Some(Command::Apply { id, spec }) => {
                self.put_profile(Some(id), spec, false).map(|_| ())
            }
            Some(Command::Delete(id)) => {
                if self.selected_profile == Some(id) { self.selected_profile = None; }
                let before = self.chart.profiles.len();
                self.chart.profiles.retain(|instance| instance.id != id);
                if before != self.chart.profiles.len() {
                    self.workspace.mark_dirty();
                    self.profile_scene.clear();
                }
                Ok(())
            }
            Some(Command::SetVisible { id, visible }) => {
                if let Some(instance) = self
                    .chart
                    .profiles
                    .iter_mut()
                    .find(|instance| instance.id == id)
                    && instance.visible != visible
                {
                    instance.visible = visible;
                    self.workspace.mark_dirty();
                    self.profile_scene.clear();
                }
                Ok(())
            }
            Some(Command::Rebin(id)) => {
                if let Some(tick) = self.profile_tick() {
                    let width = profiles_runtime::suggested_rows(tick, self.chart.candles());
                    let spec = self
                        .chart
                        .profiles
                        .iter()
                        .find(|instance| instance.id == id)
                        .map(|instance| {
                            let mut spec = instance.spec.clone();
                            spec.ticks_per_row = width;
                            spec
                        });
                    match spec {
                        Some(spec) => self.put_profile(Some(id), spec, false).map(|_| ()),
                        None => Err("This profile was removed".into()),
                    }
                } else {
                    Err("Authoritative tick-size metadata is unavailable".into())
                }
            }
            Some(Command::PlaceRange { id, spec }) => {
                if let Some(candle) = self.chart.inspected().or_else(|| self.chart.latest()) {
                    let cursor_ms = candle.time.saturating_mul(1000);
                    self.drawing_interaction.cancel();
                    self.drawing_controls.close();
                    self.profile_controls.open = false;
                    self.profile_scene.clear();
                    self.chart.dispatch(Action::ActivatePane(None));
                    self.profile_range = Some(ProfileRange {
                        id,
                        spec,
                        first: None,
                        cursor_ms,
                    });
                    self.pointer_generation = self.pointer_generation.wrapping_add(1);
                    Ok(())
                } else {
                    Err("Wait for native candles before placing a profile range".into())
                }
            }
            Some(Command::Retry) => {
                self.profiles.retry();
                Ok(())
            }
            Some(Command::Focus { backwards }) => {
                let task = if backwards {
                    iced::widget::focus_previous()
                } else {
                    iced::widget::focus_next()
                };
                return task.chain(profile_controls::Controls::reveal_focus());
            }
            Some(Command::Scroll(amount)) => return profile_controls::Controls::scroll(amount),
            None => Ok(()),
        };
        if let Err(error) = result {
            self.profile_controls.set_error(error);
        }
        self.profile_statuses();
        if focus && self.profile_controls.open {
            return self.profile_controls.focus_visible();
        }
        Task::none()
    }

    fn has_cvd(&self) -> bool {
        self.chart.indicators.instances.iter().any(|instance| {
            instance.source.is_chart() && matches!(instance.spec, IndicatorSpec::Cvd { .. })
        })
    }

    fn has_open_interest(&self) -> bool {
        self.chart.indicators.instances.iter().any(|instance| {
            instance.source.is_chart() && matches!(instance.spec, IndicatorSpec::OpenInterest)
        })
    }

    fn fetch_older_open_interest(&mut self) -> Task<Message> {
        if !self.has_open_interest()
            || self.open_interest_history.in_flight
            || self.open_interest_history.retry_ticks != 0
        {
            return Task::none();
        }
        let Some(before_ms) = self.open_interest_history.next_before_ms else {
            return Task::none();
        };
        let Some(target) = self.chart.oldest().map(|time| time.saturating_mul(1000)) else {
            return Task::none();
        };
        if before_ms <= target {
            return Task::none();
        }
        self.open_interest_history.in_flight = true;
        let (cancel, receiver) = tokio::sync::oneshot::channel();
        self.open_interest_history.cancel = Some(cancel);
        let market = self.chart.market.clone();
        let generation = self.market_generation;
        let open_interest_generation = self.open_interest_history.generation;
        let data = self.data.clone();
        let request_market = market.clone();
        Task::perform(
            async move {
                tokio::select! {
                    _ = receiver => Err("OI history request cancelled".into()),
                    result = data.open_interest_history(&request_market, Some(before_ms)) =>
                        result.map_err(|error| format!("{error:#}")),
                }
            },
            move |result| Message::OpenInterestHistory {
                market: market.clone(),
                generation,
                open_interest_generation,
                before_ms,
                result,
            },
        )
    }

    fn clear_open_interest_geometry(&self) {
        for instance in &self.chart.indicators.instances {
            if instance.visible
                && matches!(instance.spec, IndicatorSpec::OpenInterest)
                && let Some(cache) = self.pane_caches.get(&instance.id)
            {
                cache.clear();
            }
        }
    }

    fn flush_open_interest(&mut self, samples: &mut ArrayVec<OpenInterestSample, 64>) {
        if !samples.is_empty() && self.chart.merge_open_interest_samples(samples.as_slice()) {
            self.clear_open_interest_geometry();
        }
        samples.clear();
    }

    fn cancel_flow_history(&mut self) {
        self.history.cancel();
    }

    fn fetch(&mut self, request: Request) -> Task<Message> {
        if self.stress.is_some() {
            return Task::none();
        }
        self.history.in_flight = true;
        self.history.failed = None;
        self.history.error = None;
        self.history.page = None;
        self.history.generation = self.history.generation.wrapping_add(1);
        let history_id = self.history.generation;
        let epoch = self.chart.epoch;
        let market = self.chart.market.clone();
        let generation = self.market_generation;
        if self.has_cvd() {
            self.cancel_flow_history();
            let (cancel, receiver) = tokio::sync::oneshot::channel();
            self.history.cancel = Some(cancel);
            let flow_generation = self.flow_generation;
            Task::run(
                feeds::enriched_history(
                    self.data.clone(),
                    market.clone(),
                    request.before(),
                    receiver,
                ),
                move |events| Message::HistoryFlow {
                    request,
                    market: market.clone(),
                    generation,
                    history_id,
                    epoch,
                    flow_generation,
                    events,
                },
            )
        } else {
            self.cancel_flow_history();
            let data = self.data.clone();
            let source_market = market.clone();
            Task::perform(
                async move {
                    data.history(&source_market, request.before())
                        .await
                        .map_err(|error| format!("{error:#}"))
                },
                move |result| Message::History {
                    request,
                    market: market.clone(),
                    generation,
                    epoch,
                    history_id,
                    result,
                },
            )
        }
    }

    fn load_catalog(&mut self, exchange: Exchange, product: Product) -> Task<Message> {
        self.catalog_request(exchange, product, false)
    }

    fn retry_catalog(&mut self, exchange: Exchange, product: Product) -> Task<Message> {
        self.catalog_request(exchange, product, true)
    }

    fn catalog_request(
        &mut self,
        exchange: Exchange,
        product: Product,
        retry: bool,
    ) -> Task<Message> {
        if self.stress.is_some() {
            return Task::none();
        }
        self.catalog_generation = self.catalog_generation.wrapping_add(1);
        self.selector.begin_catalog();
        let generation = self.catalog_generation;
        let data = self.data.clone();
        Task::perform(
            async move {
                let result = if retry {
                    data.retry_catalog(exchange, product).await
                } else {
                    data.catalog(exchange, product).await
                };
                result.map_err(|error| format!("{error:#}"))
            },
            move |result| Message::Catalog {
                exchange,
                product,
                generation,
                result,
            },
        )
    }

    fn accepts(&self, market: &Market, generation: u64) -> bool {
        generation == self.market_generation && market == &self.chart.market
    }

    fn restart_connection(&mut self) {
        self.cancel_flow_history();
        self.open_interest_history.reset();
        self.chart.open_interest.connected = false;
        if self.has_open_interest() {
            self.chart.open_interest.message = "Connecting to native open interest…".into();
        }
        self.market_generation += 1;
        self.upstream_connected = false;
        if self.chart.revoke_live_flow() {
            self.clear_flow_geometry();
        }
        self.history.in_flight = false;
        self.history.pending_sync = false;
        self.history.anchor = None;
        self.history.page = None;
        self.history.flow_target = self.has_cvd().then(|| self.chart.oldest()).flatten();
        self.history.failed = None;
        self.history.retry_ticks = 0;
        self.history.error = None;
        self.history.flow_error = None;
        self.flow_status = None;
        self.history.progress = None;
    }

    fn remember_anchor(&mut self) {
        if let Some(time) = self.chart.latest().map(|candle| candle.time) {
            self.history.anchor = Some(self.history.anchor.map_or(time, |anchor| anchor.min(time)));
        }
    }

    fn sync(&mut self) -> Task<Message> {
        self.remember_anchor();
        self.chart.epoch += 1;
        if self.history.in_flight {
            self.history.pending_sync = true;
            Task::none()
        } else {
            self.fetch(Request::Recent)
        }
    }

    fn apply_history_page(&mut self, page: Arc<HistoryPage>, request: Request, epoch: u64) {
        self.history.error = None;
        let previous_oldest = self.chart.oldest();
        let oldest = page.candles.first().map(|candle| candle.time);
        if previous_oldest.is_none()
            || oldest.is_some_and(|time| previous_oldest.is_some_and(|old| time <= old))
        {
            self.chart.next_before = page.next_before;
        } else if matches!(request, Request::Older { .. }) && page.candles.is_empty() {
            self.chart.next_before = None;
        }
        self.chart.merge_page(&page, epoch);
        if !self.chart.candles().is_empty()
            && let Some(view) = self.workspace.restored_view.take()
        {
            if let Err(error) = self.chart.reapply_workspace_view(&view) {
                self.workspace.error = Some(error);
            }
            if !view.live {
                self.goto_target = view.left.map(|left| left.floor() as i64);
                if let Some(target) = self.goto_target {
                    let _ = self.goto_time.show(target);
                    self.goto_time.open = false;
                }
            }
        }
        self.update_source_history_target();
        self.clear_geometry();
        self.history.page = Some(page);
    }

    fn fail_history(&mut self, request: Request, error: String) {
        self.history.in_flight = false;
        self.history.progress = None;
        self.history.error = Some(format!(
            "History: {error} — retrying every 5s; Y / Retry now"
        ));
        self.history.failed = Some(request);
        self.history.retry_ticks = 0;
        self.history.page = None;
        self.cancel_flow_history();
    }

    fn finish_history(&mut self, request: Request) -> Task<Message> {
        self.history.in_flight = false;
        let Some(page) = self.history.page.take() else {
            self.fail_history(
                request,
                "CVD history ended before its native price page".into(),
            );
            return Task::none();
        };
        if matches!(request, Request::Older { .. })
            && self.goto_target.is_some()
            && self
                .goto_page_oldest
                .take()
                .is_some_and(|previous| self.chart.oldest().is_none_or(|oldest| oldest >= previous))
        {
            self.goto_target = None;
            return self.goto_time.reject("Native history did not advance toward the requested UTC time; no missing candles were fabricated.");
        }
        if self.history.pending_sync {
            self.history.pending_sync = false;
            return self.fetch(Request::Recent);
        }
        let target = match request {
            Request::Recent => self.history.anchor,
            Request::Repair { target, .. } | Request::FlowBackfill { target, .. } => Some(target),
            Request::Older { .. } => None,
        };
        if let Some(next) = repair_request(request, &page, target) {
            return self.fetch(next);
        }
        if matches!(request, Request::FlowBackfill { .. }) {
            self.history.flow_target = None;
        } else if let Some(target) = self.history.flow_target {
            if self.has_cvd()
                && page
                    .candles
                    .first()
                    .is_some_and(|candle| candle.time > target)
                && let Some(before) = page.next_before
            {
                return self.fetch(Request::FlowBackfill { before, target });
            }
            self.history.flow_target = None;
        }
        if !matches!(request, Request::Older { .. }) {
            self.history.anchor = None;
        }
        if self.goto_target.is_some() {
            return self.continue_goto();
        }
        if self.chart.needs_older() {
            return self.older();
        }
        Task::none()
    }

    fn finish_stale_flow_history(&mut self) -> Task<Message> {
        self.history.in_flight = false;
        self.history.page = None;
        if self.history.pending_sync {
            self.history.pending_sync = false;
            self.fetch(Request::Recent)
        } else {
            Task::none()
        }
    }

    fn vwap_history_origin(&self) -> Option<i64> {
        let latest = self.chart.latest()?.time.saturating_mul(1000);
        let visible = (self.chart.viewport.left * 1000.0) as i64;
        let mut origin = None;
        for instance in &self.chart.indicators.instances {
            if let IndicatorSpec::Vwap { reset } = instance.spec {
                for time in [visible.min(latest), latest] {
                    let start = mexc_indicators::profiles::period_bounds(reset, time)
                        .0
                        .div_euclid(1000);
                    origin = Some(origin.map_or(start, |old: i64| old.min(start)));
                }
            }
        }
        for drawing in self.drawings.objects() {
            if let drawing_types::Shape::AnchoredVwap(at) = drawing.shape {
                let start = at.utc_ms.div_euclid(1000);
                origin = Some(origin.map_or(start, |old| old.min(start)));
            }
        }
        origin
    }

    /// Load the actual origin without moving the viewport or fabricating bars.
    /// A nonadvancing native cursor is not retried on every UI/live update.
    fn backfill_vwap_origin(&mut self) -> Option<Task<Message>> {
        if self.stress.is_some()
            || self.workspace.waiting_market
            || !self.chart.initialized
            || self.goto_target.is_some()
            || self.history.in_flight
            || self.history.failed.is_some()
        {
            return None;
        }
        let origin = self.vwap_history_origin()?;
        if self.chart.oldest()? <= origin {
            return None;
        }
        let before = self.chart.next_before?;
        if self
            .vwap_backfill_cursor
            .is_some_and(|(generation, previous)| {
                generation == self.market_generation && before >= previous
            })
        {
            return None;
        }
        self.vwap_backfill_cursor = Some((self.market_generation, before));
        Some(self.older())
    }

    fn older(&mut self) -> Task<Message> {
        if self.history.in_flight || self.history.failed.is_some() {
            return Task::none();
        }
        self.chart
            .next_before
            .map(|before| self.fetch(Request::Older { before }))
            .unwrap_or_else(Task::none)
    }

    fn clear_geometry(&mut self) {
        self.cache.clear();
        for cache in self.pane_caches.values() {
            cache.clear();
        }
    }

    fn clear_flow_geometry(&self) {
        for instance in &self.chart.indicators.instances {
            if instance.visible
                && matches!(instance.spec, IndicatorSpec::Cvd { .. })
                && let Some(cache) = self.pane_caches.get(&instance.id)
            {
                cache.clear();
            }
        }
    }

    fn flush_flows(&mut self, flows: &mut ArrayVec<FlowBar, 64>, live: bool) {
        if !flows.is_empty() && self.chart.merge_flows(flows.drain(..), live) {
            self.clear_flow_geometry();
        }
    }

    fn scale_action(&self, action: Action) -> Action {
        let Some(id) = self.chart.active_pane.filter(|id| {
            self.chart.indicators.instances.iter().any(|instance| {
                instance.id == *id && instance.visible && !instance.spec.is_overlay()
            })
        }) else {
            return action;
        };
        match action {
            Action::Pan { time, price } => Action::PanePan {
                id,
                time,
                value: price,
            },
            Action::Zoom { time, price, x, y } => Action::PaneZoom {
                id,
                time,
                value: price,
                x,
                y,
            },
            Action::AdjustPaneHeight { id: None, delta } => Action::AdjustPaneHeight {
                id: Some(id),
                delta,
            },
            Action::ResetPaneHeight(None) => Action::ResetPaneHeight(Some(id)),
            other => other,
        }
    }

    fn cycle_scale_target(&mut self, direction: i32) {
        let panes = || {
            self.chart
                .indicators
                .instances
                .iter()
                .filter(|instance| instance.visible && !instance.spec.is_overlay())
        };
        let count = panes().count() + 1;
        let current = self
            .chart
            .active_pane
            .and_then(|id| panes().position(|instance| instance.id == id))
            .map_or(0, |index| index + 1);
        let next = (current as i32 + direction).rem_euclid(count as i32) as usize;
        let id = next
            .checked_sub(1)
            .and_then(|index| panes().nth(index).map(|instance| instance.id));
        if self.chart.maximized() && !self.chart.pane_should_render(id) {
            self.chart.dispatch(Action::TogglePaneMaximized(id));
        }
        self.chart.dispatch(Action::ActivatePane(id));
        self.workspace.mark_dirty();
    }

    fn drawing_blocked(&self) -> bool {
        self.commands.is_some()
            || self.command_pending
            || self.goto_time.open
            || self.help_open
            || self.manager.open
            || self.profile_controls.open
            || self.selector.open
            || self.details.is_some()
            || self.drawing_controls.is_open()
            || self.workspace.controls.is_open()
    }

    fn drawing_focus(&self) -> Task<Message> {
        self.drawing_controls
            .focus_id()
            .map_or_else(Task::none, iced::widget::text_input::focus)
    }

    fn commit_drawing(&mut self, command: DrawingCommand) -> bool {
        let previous = self.drawings.revision();
        match self.drawings.apply(command) {
            Ok(_) => {
                self.drawing_error = None;
                if self.drawings.revision() != previous {
                    self.workspace.mark_dirty();
                }
                true
            }
            Err(error) => {
                if self.drawing_controls.is_open() {
                    self.drawing_controls.set_error(error.clone());
                }
                self.drawing_error = Some(error);
                false
            }
        }
    }

    fn native_intervals(&self) -> &[Timeframe] {
        self.active_catalog
            .as_deref()
            .filter(|catalog| {
                (catalog.exchange, catalog.product)
                    == (self.chart.market.exchange, self.chart.market.product)
            })
            .map_or(&[], |catalog| catalog.intervals.as_slice())
    }

    fn switch_market(&mut self, market: Market, catalog: Arc<Catalog>) -> Task<Message> {
        if (market.exchange, market.product) != (catalog.exchange, catalog.product)
            || !catalog.intervals.contains(&market.interval)
        {
            return Task::none();
        }
        let Some(instrument) = catalog.instruments.iter().find(|item| {
            item.symbol == market.symbol
                && item.exchange == market.exchange
                && item.product == market.product
        }) else {
            return Task::none();
        };
        let was_waiting = self.workspace.waiting_market;
        self.workspace.waiting_market = false;
        self.workspace.restored_view = None;
        self.selector.observe_market(&market);
        self.workspace.mark_dirty();
        self.goto_target = None;
        self.goto_page_oldest = None;
        self.goto_time.open = false;
        self.goto_notice = None;
        let price_scale = instrument.price_scale;
        let native_quote = instrument.quote_coin.to_string();
        let native_volume_unit = instrument.volume_unit.clone();
        let base_asset = instrument.base_coin.clone();
        self.active_catalog = Some(catalog);
        self.selector.open = false;
        if market == self.chart.market {
            if was_waiting {
                self.chart.price_scale = price_scale;
                self.chart.base_asset = base_asset;
                self.native_quote = native_quote;
                self.native_volume_unit = native_volume_unit;
                self.sync_indicator_sources();
                return Task::batch([self.fetch(Request::Recent), self.ensure_source_catalogs()]);
            }
            return Task::none();
        }
        self.native_quote = native_quote;
        self.native_volume_unit = native_volume_unit;
        self.pointer_generation = self.pointer_generation.wrapping_add(1);
        self.drawing_interaction.cancel();
        self.profile_range = None;
        self.selected_profile = None;
        self.drawing_controls.close();
        self.drawings.switch_context(&market);
        self.drawing_projection.set(None);
        self.drawing_error = None;
        if NativeInstrument::from_market(&market)
            != NativeInstrument::from_market(&self.chart.market)
        {
            self.profiles.reset();
            self.profile_scene.clear();
        }
        self.chart.select_market(market, price_scale);
        self.chart.base_asset = base_asset;
        self.restart_connection();
        self.reset_source_discovery();
        self.sync_indicator_sources();
        self.status = "Loading selected native market…".into();
        self.clear_geometry();
        Task::batch([self.fetch(Request::Recent), self.ensure_source_catalogs()])
    }

    fn close_help(&mut self) -> Task<Message> {
        self.help_open = false;
        if self.goto_time.open {
            self.goto_time.focus()
        } else if self.profile_controls.open {
            self.profile_controls.focus()
        } else if self.manager.open {
            self.manager.show()
        } else if self.selector.open {
            iced::widget::text_input::focus("market-search")
        } else if self.drawing_controls.is_open() {
            self.drawing_focus()
        } else {
            Task::none()
        }
    }

    fn close_commands(&mut self) -> Task<Message> {
        self.commands = None;
        self.command_pending = false;
        self.command_focus.take().map_or_else(Task::none, |id| {
            iced::advanced::widget::operate(iced::advanced::widget::operation::focusable::focus::<
                Message,
            >(id))
        })
    }

    fn command_snapshot(&self) -> command_palette::Snapshot {
        let context = if self.goto_time.open {
            key_help::Context::Details
        } else if self.drawing_interaction.captures_pointer() {
            key_help::Context::Drawings
        } else {
            self.help_context()
        };
        let mut snapshot = command_palette::Snapshot::new(context);
        snapshot.active_pane = self.chart.active_pane;
        snapshot.selected_drawing = self.drawings.selected();
        snapshot.profiles = self
            .chart
            .profiles
            .iter()
            .map(|profile| command_palette::ProfileTarget {
                id: profile.id,
                label: format!("{} · {}", profile.spec.kind, profile.spec.range),
                visible: profile.visible,
                fixed: matches!(profile.spec.range, ProfileRangeSpec::Fixed { .. }),
            })
            .collect();
        for instance in &self.chart.indicators.instances {
            snapshot.indicators.push(command_palette::IndicatorTarget {
                id: instance.id,
                label: format!("{} · {}", instance.spec.label(), instance.source.label()),
                external_sources: !instance.source.is_chart()
                    && matches!(
                        instance.spec,
                        IndicatorSpec::Cvd { .. } | IndicatorSpec::OpenInterest
                    ),
            });
            if !instance.spec.is_overlay() {
                if instance.visible {
                    snapshot.pane_ids.push(instance.id);
                } else {
                    snapshot.hidden_pane_ids.push(instance.id);
                }
            }
        }
        if self.stress.is_none() {
            snapshot
                .recent_markets
                .extend_from_slice(self.selector.recent_markets());
            snapshot
                .favorite_markets
                .extend_from_slice(self.selector.favorite_markets());
        }
        snapshot
    }

    fn continue_goto(&mut self) -> Task<Message> {
        let Some(target) = self.goto_target else {
            return Task::none();
        };
        if self.history.in_flight {
            return Task::none();
        }
        let Some(oldest) = self.chart.oldest() else {
            self.goto_target = None;
            self.goto_page_oldest = None;
            return self
                .goto_time
                .reject("No native candles are available for this market.");
        };
        if oldest <= target {
            self.goto_target = None;
            self.goto_page_oldest = None;
            let center = (self.chart.viewport.left + self.chart.viewport.span * 0.5) as i64;
            let beyond_latest = self.chart.latest().is_some_and(|candle| {
                center
                    >= candle
                        .time
                        .saturating_add(self.chart.market.interval.seconds())
            });
            self.goto_notice = Some(if beyond_latest {
                "Beyond the latest loaded native candle; no future candles are fabricated.".into()
            } else {
                format!(
                    "Centered on {} UTC · native gaps remain gaps",
                    chart::utc(center, "%Y-%m-%d %H:%M:%S")
                )
            });
            return Task::none();
        }
        if self.chart.next_before.is_none() {
            self.goto_target = None;
            self.goto_page_oldest = None;
            return self.goto_time.reject(format!(
                "Requested time precedes available native history. Earliest loaded candle: {} UTC.",
                chart::utc(oldest, "%Y-%m-%d %H:%M:%S"),
            ));
        }
        if self.history.failed.is_some() {
            self.goto_notice = Some(
                "UTC jump awaits history retry; existing connection/history errors remain visible."
                    .into(),
            );
            return Task::none();
        }
        self.goto_notice = Some(format!(
            "Loading native history toward {} UTC · Go live cancels the jump",
            chart::utc(target, "%Y-%m-%d %H:%M:%S"),
        ));
        self.goto_page_oldest = Some(oldest);
        self.older()
    }
    fn help_context(&self) -> key_help::Context {
        if self.goto_time.open || self.details.is_some() {
            key_help::Context::Details
        } else if self.manager.open {
            key_help::Context::Indicators
        } else if self.profile_controls.open {
            key_help::Context::Details
        } else if self.selector.open {
            key_help::Context::Market
        } else if self.drawing_controls.is_open() {
            key_help::Context::Drawings
        } else {
            key_help::Context::Chart
        }
    }

    fn activate_help_selection(&mut self) -> Task<Message> {
        if !self.help_open {
            return Task::none();
        }
        if self.help_selection == 0 {
            return self.close_help();
        }
        let Some(action) = key_help::action_at(self.help_context(), self.help_selection - 1) else {
            return Task::none();
        };
        self.update(Message::HelpAction(action))
    }
    fn update(&mut self, message: Message) -> Task<Message> {
        let measured = matches!(&message, Message::Key { .. } | Message::StressPulse);
        let started = self.stress.as_ref().filter(|_| measured).map(|metrics| {
            metrics.input();
            Instant::now()
        });
        let result = self.handle_message(message);
        let dirty_from = self.chart.take_candle_dirty_from();
        self.drawings.sync_vwap_preview(
            self.chart.candles(),
            self.chart.market.interval,
            dirty_from,
            self.drawing_interaction.vwap_preview_start(),
        );
        self.sync_profiles();
        if let (Some(started), Some(metrics)) = (started, &self.stress) {
            metrics.update_sample(started.elapsed());
        }
        if let Some(history) = self.backfill_vwap_origin() {
            Task::batch([result, history])
        } else {
            result
        }
    }

    fn handle_message(&mut self, message: Message) -> Task<Message> {
        if self.profile_controls.open
            && matches!(
                &message,
                Message::DrawingInput { .. }
                    | Message::DrawingBlank { .. }
                    | Message::DrawingCommand { .. }
                    | Message::DrawingControl(_)
                    | Message::DrawingMenu
                    | Message::DrawingProperties(_)
                    | Message::Manager(_)
                    | Message::Selector(_)
                    | Message::SelectInterval(_)
                    | Message::IndicatorSettings(_)
                    | Message::IndicatorDetails(_)
                    | Message::StatusDetails
            )
        {
            return Task::none();
        }
        if self.drawing_interaction.is_dragging()
            && matches!(
                &message,
                Message::Action(
                    Action::Inspect(_)
                        | Action::InspectPane { .. }
                        | Action::InspectStep(_)
                        | Action::ActivatePane(_)
                        | Action::Pan { .. }
                        | Action::PanePan { .. }
                        | Action::Zoom { .. }
                        | Action::PaneZoom { .. }
                )
            )
        {
            return Task::none();
        }
        if self.drawing_interaction.is_dragging() && matches!(&message, Message::Action(_)) {
            self.pointer_generation = self.pointer_generation.wrapping_add(1);
            self.drawing_interaction.suspend_overlay();
        }
        if self.workspace.controls.is_open()
            && !matches!(
                &message,
                Message::Workspace(_)
                    | Message::WorkspacePoll
                    | Message::WorkspaceSaved { .. }
                    | Message::WorkspaceResetDone { .. }
                    | Message::CloseRequested(_)
                    | Message::PointerCancelled
                    | Message::MarketData(_)
                    | Message::ProfileData(_)
                    | Message::History { .. }
                    | Message::HistoryFlow { .. }
                    | Message::OpenInterest(_)
                    | Message::OpenInterestHistory { .. }
                    | Message::SourceData(_)
                    | Message::SourceCatalog { .. }
                    | Message::Catalog { .. }
                    | Message::Tick
                    | Message::Key { .. }
            )
        {
            return Task::none();
        }
        if (self.commands.is_some() || self.command_pending)
            && matches!(
                &message,
                Message::Action(_)
                    | Message::DrawingInput { .. }
                    | Message::DrawingBlank { .. }
                    | Message::DrawingCommand { .. }
                    | Message::DrawingControl(_)
                    | Message::DrawingMenu
                    | Message::DrawingProperties(_)
                    | Message::DrawingCancel
                    | Message::DrawingClose
                    | Message::Manager(_)
                    | Message::Profile(_)
                    | Message::ProfileEntry(_)
                    | Message::ProfileEdit(_)
                    | Message::Selector(_)
                    | Message::SelectInterval(_)
                    | Message::IndicatorSettings(_)
                    | Message::IndicatorDetails(_)
                    | Message::StatusDetails
                    | Message::DetailsClose
                    | Message::EditorClose
                    | Message::GotoTimeOpen
                    | Message::GotoTime(_)
                    | Message::QuickMarket(_)
                    | Message::RetryContributor { .. }
                    | Message::RetrySourceDiscovery { .. }
                    | Message::UseChartSource(_)
                    | Message::RetryIndicatorSources(_)
            )
        {
            return Task::none();
        }
        if self.goto_time.open
            && matches!(
                &message,
                Message::SelectInterval(_)
                    | Message::IndicatorSettings(_)
                    | Message::IndicatorDetails(_)
                    | Message::StatusDetails
                    | Message::DrawingControl(_)
                    | Message::DrawingMenu
                    | Message::DrawingProperties(_)
                    | Message::Manager(_)
                    | Message::Selector(_)
                    | Message::UseChartSource(_)
                    | Message::RetryIndicatorSources(_)
            )
        {
            return Task::none();
        }
        if self.drawing_controls.is_open()
            && matches!(
                &message,
                Message::IndicatorSettings(_)
                    | Message::IndicatorDetails(_)
                    | Message::StatusDetails
                    | Message::SelectInterval(_)
                    | Message::UseChartSource(_)
                    | Message::RetryIndicatorSources(_)
            )
        {
            return Task::none();
        }
        match message {
            Message::PointerCancelled => {
                // Canvas does not receive window lifecycle events. Reset every
                // pane's held input state at this explicit cancellation boundary.
                self.pointer_generation = self.pointer_generation.wrapping_add(1);
                self.drawing_interaction.suspend_overlay();
                self.profile_range = None;
                self.chart.dispatch(Action::Inspect(None));
            }
            Message::Profile(event) => return self.profile_event(event),
            Message::ProfileEntry(entry) => return self.profile_entry(entry),
            Message::ProfileEdit(edit) => return self.edit_profile(edit),
            Message::ProfileData(event) => {
                self.profiles.accept(event);
                self.profile_statuses();
            }
            Message::Workspace(event) => return self.workspace_event(event),
            Message::WorkspacePoll => return self.poll_workspace_save(false),
            Message::WorkspaceSaved { revision, result } => {
                return self.workspace_saved(revision, result);
            }
            Message::WorkspaceResetDone {
                revision,
                kind,
                snapshot,
                result,
            } => {
                return self.workspace_reset_done(revision, kind, snapshot, result);
            }
            Message::CloseRequested(id) => {
                self.workspace.closing = Some(id);
                return self.poll_workspace_save(true);
            }
            Message::CommandOpen => {
                self.profile_range = None;
                if self.commands.is_some() || self.command_pending {
                    return self.close_commands();
                }
                self.command_pending = true;
                return iced::advanced::widget::operate(FindFocused(None))
                    .map(Message::CommandFocused);
            }
            Message::CommandFocused(focus) => {
                if !self.command_pending {
                    return Task::none();
                }
                self.command_pending = false;
                self.command_focus = focus;
                self.help_open = false;
                self.drawing_interaction.suspend_overlay();
                self.commands = Some(command_palette::Commands::show(self.command_snapshot()));
                return self.commands.as_ref().unwrap().focus();
            }
            Message::CommandPalette(event) => {
                if matches!(event, command_palette::Event::Escape) {
                    return self.close_commands();
                }
                let Some(commands) = &mut self.commands else {
                    return Task::none();
                };
                if let Some(message) = commands.update(event) {
                    let focus = self.close_commands();
                    return Task::batch([focus, self.update(message)]);
                }
                return self.commands.as_ref().unwrap().follow_selection();
            }
            Message::GotoTimeOpen => {
                if self.drawing_blocked() {
                    return Task::none();
                }
                self.goto_target = None;
                self.goto_page_oldest = None;
                self.goto_notice = None;
                self.drawing_interaction.cancel();
                return self.goto_time.show(self.chart.inspection.map_or_else(
                    || (self.chart.viewport.left + self.chart.viewport.span * 0.5) as i64,
                    |(time, _)| time,
                ));
            }
            Message::GotoTime(event) => {
                if self.help_open {
                    return Task::none();
                }
                match self.goto_time.update(event) {
                    Some(goto_time::Effect::Jump(time)) => {
                        self.chart.dispatch(Action::GotoTime(time));
                        self.workspace.mark_dirty();
                        self.clear_geometry();
                        self.goto_target = Some(time);
                        self.goto_page_oldest = None;
                        return self.continue_goto();
                    }
                    Some(goto_time::Effect::Close) => {}
                    None => {}
                }
                return Task::none();
            }
            Message::QuickMarket(market) => {
                if self.help_open
                    || self.manager.open
                    || self.details.is_some()
                    || self.goto_time.open
                    || self.drawing_controls.is_open()
                {
                    return Task::none();
                }
                if self.stress.is_some() {
                    return Task::none();
                }
                self.drawing_interaction.cancel();
                let focus = self.selector.show_exact(&market);
                return Task::batch([focus, self.load_catalog(market.exchange, market.product)]);
            }
            Message::RetryContributor { id, market } => {
                if !self.help_open
                    && !self.manager.open
                    && !self.selector.open
                    && !self.goto_time.open
                {
                    return self.retry_indicator_contributor(id, &market);
                }
            }
            Message::RetrySourceDiscovery {
                id,
                exchange,
                product,
            } => {
                if !self.help_open
                    && !self.manager.open
                    && !self.selector.open
                    && !self.goto_time.open
                {
                    return self.retry_indicator_source_discovery(id, exchange, product);
                }
            }
            Message::StressPulse => {
                if !self.drawing_blocked()
                    && let Some(metrics) = &self.stress
                {
                    self.chart.dispatch(Action::Pan {
                        time: metrics.pan(),
                        price: 0.0,
                    });
                    self.clear_geometry();
                }
                return Task::none();
            }
            Message::StressFrame(at) => {
                if let Some(metrics) = &self.stress {
                    metrics.frame(at);
                }
                return Task::none();
            }
            Message::StressFinish => {
                if let Some(metrics) = &self.stress {
                    metrics.report();
                    return iced::exit();
                }
            }
            Message::DrawingCancel => {
                if !self.drawing_blocked() {
                    self.drawing_interaction.cancel();
                    self.drawings.select(None);
                    self.selected_profile = None;
                }
            }
            Message::DrawingMenu => {
                if self.drawing_blocked() {
                    return Task::none();
                }
                self.drawing_interaction.cancel();
                self.drawing_controls.open_menu(&self.drawings);
                return scrollable::scroll_to(
                    scrollable::Id::new(drawing_controls::BODY_ID),
                    scrollable::AbsoluteOffset { x: 0.0, y: 0.0 },
                );
            }
            Message::DrawingProperties(id) => {
                if self.drawing_blocked() || self.drawings.find(id).is_none() {
                    return Task::none();
                }
                self.drawing_interaction.cancel();
                self.drawings.select(Some(id));
                if self.drawing_controls.open_properties(&self.drawings, id) {
                    return Task::batch([
                        self.drawing_focus(),
                        scrollable::scroll_to(
                            scrollable::Id::new(drawing_controls::BODY_ID),
                            scrollable::AbsoluteOffset { x: 0.0, y: 0.0 },
                        ),
                    ]);
                }
            }
            Message::DrawingClose => {
                if !self.help_open {
                    if self.drawing_controls.is_new_text() {
                        self.drawing_interaction.finish_text(false);
                    }
                    self.drawing_controls.close();
                }
            }
            Message::DrawingCommand {
                generation,
                command,
            } => {
                if self.drawing_blocked() || generation != self.drawings.generation() {
                    return Task::none();
                }
                if matches!(&command, DrawingCommand::Style { .. }) {
                    self.drawing_interaction.suspend_overlay();
                } else {
                    self.drawing_interaction.cancel();
                }
                self.commit_drawing(command);
            }
            Message::DrawingBlank { generation } => {
                if !self.drawing_blocked() && generation == self.drawing_interaction.generation() {
                    self.drawing_interaction.suspend_overlay();
                    self.drawings.select(None);
                    self.chart.dispatch(Action::ActivatePane(None));
                }
            }
            Message::DrawingInput { generation, event } => {
                if self.drawing_blocked() || generation != self.drawing_interaction.generation() {
                    return Task::none();
                }
                let inspection = match &event {
                    drawing_interaction::Input::Move {
                        point, projection, ..
                    } if self.drawing_interaction.tool() == DrawingTool::Select
                        && !self.drawing_interaction.is_dragging()
                        && projection.contains(*point) =>
                    {
                        projection.anchor(*point)
                    }
                    _ => None,
                };
                if matches!(
                    &event,
                    drawing_interaction::Input::Down { .. }
                        | drawing_interaction::Input::Key { .. }
                ) {
                    self.chart.dispatch(Action::ActivatePane(None));
                    self.drawing_error = None;
                }
                if matches!(&event, drawing_interaction::Input::Down { .. }) {
                    self.chart.dispatch(Action::Inspect(None));
                }
                if matches!(&event, drawing_interaction::Input::Leave) {
                    self.chart.dispatch(Action::Inspect(None));
                }
                let effect = self.drawing_interaction.update_native(
                    event,
                    &self.drawings,
                    self.chart.candles(),
                );
                match effect {
                    drawing_interaction::Effect::None => {}
                    drawing_interaction::Effect::Select(id) => {
                        if id.is_some() {
                            self.selected_profile = None;
                        }
                        self.drawings.select(id);
                    }
                    drawing_interaction::Effect::Commit(command) => {
                        self.commit_drawing(command);
                    }
                    drawing_interaction::Effect::Text(anchor) => {
                        self.drawing_interaction.suspend_for_text();
                        self.drawing_controls.open_text(&self.drawings, anchor);
                        return Task::batch([
                            self.drawing_focus(),
                            scrollable::scroll_to(
                                scrollable::Id::new(drawing_controls::BODY_ID),
                                scrollable::AbsoluteOffset { x: 0.0, y: 0.0 },
                            ),
                        ]);
                    }
                }
                if let Some(anchor) = inspection {
                    self.chart.dispatch(Action::InspectPane {
                        id: None,
                        point: Some((anchor.utc_ms as f64 / 1000.0, anchor.price)),
                    });
                }
            }
            Message::DrawingControl(event) => {
                let quick_style = matches!(
                    &event,
                    drawing_controls::Event::QuickInk(_)
                        | drawing_controls::Event::QuickPattern(_)
                        | drawing_controls::Event::QuickWidth(_)
                );
                if self.help_open
                    || self.manager.open
                    || self.selector.open
                    || self.details.is_some()
                    || self.goto_time.open
                {
                    return Task::none();
                }
                if !self.drawing_controls.is_open()
                    && !quick_style
                    && !matches!(&event, drawing_controls::Event::Repeat(_))
                {
                    self.drawing_controls.open_menu(&self.drawings);
                }
                if self.drawing_controls.is_open()
                    && self.drawing_controls.context_generation() != self.drawings.generation()
                {
                    return Task::none();
                }
                let new_text = self.drawing_controls.is_new_text();
                let cancel_text = new_text
                    && matches!(
                        &event,
                        drawing_controls::Event::Cancel | drawing_controls::Event::Close
                    );
                let editing = self.drawing_controls.is_editing();
                let effect = self.drawing_controls.update(event, &self.drawings);
                match effect {
                    drawing_controls::Effect::None => {}
                    drawing_controls::Effect::SelectedVolumeProfile => {
                        return self.profile_entry(indicator_manager::ProfileEntry::SelectedVolume);
                    }
                    drawing_controls::Effect::ChooseTool(tool) => {
                        if tool != DrawingTool::Select && self.chart.latest().is_none() {
                            self.drawing_controls
                                .set_error("Waiting for native price data".into());
                            return Task::none();
                        }
                        self.drawing_controls.close();
                        self.drawing_interaction.choose_tool(tool);
                        self.selected_profile = None;
                        if tool != DrawingTool::Select {
                            self.drawings.select(None);
                        }
                        if self.chart.maximized() && !self.chart.pane_should_render(None) {
                            self.chart.dispatch(Action::TogglePaneMaximized(None));
                        }
                        self.chart.dispatch(Action::ActivatePane(None));
                        self.drawing_error = None;
                    }
                    drawing_controls::Effect::Select(id) => {
                        self.selected_profile = None;
                        self.drawings.select(id);
                        self.chart.dispatch(Action::ActivatePane(None));
                        if !editing && self.drawing_controls.is_editing() {
                            return Task::batch([
                                self.drawing_focus(),
                                scrollable::scroll_to(
                                    scrollable::Id::new(drawing_controls::BODY_ID),
                                    scrollable::AbsoluteOffset { x: 0.0, y: 0.0 },
                                ),
                            ]);
                        }
                    }
                    drawing_controls::Effect::Magnet(enabled) => {
                        self.drawing_interaction.set_magnet(enabled);
                        self.workspace.mark_dirty();
                    }
                    drawing_controls::Effect::ToggleMagnet => {
                        self.drawing_interaction
                            .set_magnet(!self.drawing_interaction.magnet());
                        self.workspace.mark_dirty();
                    }
                    drawing_controls::Effect::Repeat(repeat) => {
                        self.drawing_interaction.set_repeat(repeat);
                    }
                    drawing_controls::Effect::Commit(command) => {
                        if quick_style {
                            self.drawing_interaction.suspend_overlay();
                        } else if !new_text {
                            self.drawing_interaction.cancel();
                        }
                        if self.commit_drawing(command) {
                            if new_text {
                                self.drawing_interaction.finish_text(true);
                            }
                            if editing {
                                self.drawing_controls.close();
                            }
                        }
                    }
                    drawing_controls::Effect::Close => self.drawing_controls.close(),
                }
                if cancel_text {
                    self.drawing_interaction.finish_text(false);
                    self.drawing_controls.close();
                }
                if editing && self.drawing_controls.is_open() && !self.drawing_controls.is_editing()
                {
                    return scrollable::scroll_to(
                        scrollable::Id::new(drawing_controls::BODY_ID),
                        scrollable::AbsoluteOffset { x: 0.0, y: 0.0 },
                    );
                }
            }
            Message::Key {
                key,
                modifiers,
                captured,
            } => {
                use keyboard::{Key, key::Named};
                if self.workspace.controls.is_open() {
                    if let Some(event) = self.workspace.controls.key(
                        key.as_ref(),
                        modifiers,
                        self.workspace.reset_pending,
                    ) {
                        return self.workspace_event(event);
                    }
                    return Task::none();
                }
                if modifiers.control()
                    && !modifiers.alt()
                    && matches!(key.as_ref(), Key::Character("k" | "K"))
                {
                    return self.update(Message::CommandOpen);
                }
                if self.commands.is_some() {
                    if matches!(key.as_ref(), Key::Named(Named::Escape)) {
                        return self.close_commands();
                    }
                    if matches!(key.as_ref(), Key::Named(Named::F1)) {
                        let focus = self.close_commands();
                        self.help_open = true;
                        self.help_selection = 0;
                        return focus;
                    }
                    let (_, message) = self.commands.as_mut().unwrap().key(key, modifiers);
                    if let Some(message) = message {
                        let focus = self.close_commands();
                        return Task::batch([focus, self.update(message)]);
                    }
                    return self.commands.as_ref().unwrap().follow_selection();
                }
                if self.command_pending {
                    return Task::none();
                }
                if matches!(key.as_ref(), Key::Named(Named::F1))
                    || (!self.selector.open
                        && !self.manager.open
                        && !captured
                        && matches!(key.as_ref(), Key::Character("?")))
                {
                    if !self.help_open {
                        self.drawing_interaction.cancel();
                        self.profile_range = None;
                        self.help_open = true;
                        self.help_selection = 0;
                    }
                    return Task::none();
                }
                if self.goto_time.open && !self.help_open {
                    if matches!(key.as_ref(), Key::Named(Named::Tab)) {
                        return if modifiers.shift() {
                            iced::widget::focus_previous()
                        } else {
                            iced::widget::focus_next()
                        };
                    }
                    if (matches!(key.as_ref(), Key::Named(Named::Escape)) || !captured)
                        && let Some(event) = self.goto_time.key(key.as_ref(), modifiers)
                    {
                        return self.update(Message::GotoTime(event));
                    }
                    return Task::none();
                }
                if modifiers.control()
                    && modifiers.shift()
                    && !modifiers.alt()
                    && matches!(key.as_ref(), Key::Character("r" | "R"))
                    && !self.help_open
                    && !self.selector.open
                    && !self.manager.open
                    && !self.profile_controls.open
                    && self.details.is_none()
                    && !self.drawing_controls.is_editing()
                    && (self.drawing_controls.is_open()
                        || (self.chart.active_pane.is_none()
                            && (self.drawing_interaction.tool() != DrawingTool::Select
                                || self.drawings.selected().is_some())))
                {
                    return self.update(Message::DrawingControl(drawing_controls::Event::Repeat(
                        !self.drawing_interaction.repeat(),
                    )));
                }
                if self.help_open {
                    if matches!(key.as_ref(), Key::Named(Named::Escape)) {
                        return self.update(Message::HelpClose);
                    }
                    if matches!(key.as_ref(), Key::Named(Named::Tab)) {
                        let count = key_help::action_count(self.help_context()) + 1;
                        self.help_selection = if modifiers.shift() {
                            (self.help_selection + count - 1) % count
                        } else {
                            (self.help_selection + 1) % count
                        };
                        let offset = if self.help_selection == 0 {
                            0.0
                        } else {
                            key_help::action_scroll_offset(
                                self.help_context(),
                                self.help_selection - 1,
                            )
                            .unwrap_or(0.0)
                        };
                        return scrollable::scroll_to(
                            scrollable::Id::new(key_help::LIST_SCROLL_ID),
                            scrollable::AbsoluteOffset { x: 0.0, y: offset },
                        );
                    }
                    if matches!(
                        key.as_ref(),
                        Key::Named(Named::Enter | Named::Space) | Key::Character(" ")
                    ) {
                        return self.update(Message::HelpActivate);
                    }
                    let scroll = match key.as_ref() {
                        Key::Named(Named::PageUp) => Some(-450.0),
                        Key::Named(Named::PageDown) => Some(450.0),
                        Key::Named(Named::ArrowUp) => Some(-100.0),
                        Key::Named(Named::ArrowDown) => Some(100.0),
                        _ => None,
                    };
                    if let Some(amount) = scroll {
                        return scrollable::scroll_by(
                            scrollable::Id::new(key_help::LIST_SCROLL_ID),
                            scrollable::AbsoluteOffset { x: 0.0, y: amount },
                        );
                    }
                    return Task::none();
                } else if self.details.is_some() {
                    if let Some(Details::Indicator(id)) = self.details {
                        if modifiers.control() {
                            let action = match key.as_ref() {
                                Key::Character("e" | "E") => Some(Message::IndicatorSettings(id)),
                                Key::Character("r" | "R") => {
                                    Some(Message::RetryIndicatorSources(id))
                                }
                                Key::Character("c" | "C") if modifiers.shift() => {
                                    Some(Message::UseChartSource(id))
                                }
                                _ => None,
                            };
                            if let Some(action) = action {
                                return self.update(action);
                            }
                        }
                    } else if matches!(key.as_ref(), Key::Character("y" | "Y")) {
                        return self.update(Message::HelpAction(Action::Retry));
                    }
                    if matches!(key.as_ref(), Key::Named(Named::Escape)) {
                        return self.update(Message::DetailsClose);
                    }
                    if matches!(key.as_ref(), Key::Named(Named::Tab)) {
                        return if modifiers.shift() {
                            iced::widget::focus_previous()
                        } else {
                            iced::widget::focus_next()
                        };
                    }
                    let amount = match key.as_ref() {
                        Key::Named(Named::PageUp) => Some(-350.0),
                        Key::Named(Named::PageDown) => Some(350.0),
                        Key::Named(Named::ArrowUp) => Some(-80.0),
                        Key::Named(Named::ArrowDown) => Some(80.0),
                        _ => None,
                    };
                    if let Some(amount) = amount {
                        return scrollable::scroll_by(
                            scrollable::Id::new("details-body"),
                            scrollable::AbsoluteOffset { x: 0.0, y: amount },
                        );
                    }
                } else if self.selector.open {
                    if matches!(key.as_ref(), Key::Named(Named::Escape)) {
                        return self.update(Message::Action(Action::ManageMarket));
                    }
                    if matches!(key.as_ref(), Key::Named(Named::Tab)) {
                        return if modifiers.shift() {
                            iced::widget::focus_previous()
                        } else {
                            iced::widget::focus_next()
                        };
                    }
                    if let Some(event) = self.selector.key(key.as_ref(), modifiers) {
                        return self.update(Message::Selector(event));
                    }
                    if !captured && matches!(key.as_ref(), Key::Named(Named::Enter)) {
                        return self.update(Message::Selector(market_selector::Event::Submit));
                    }
                } else if self.profile_controls.open {
                    // Focused inputs/buttons own Enter; do not submit a second
                    // time through the global keyboard listener.
                    if captured && matches!(key.as_ref(), Key::Named(Named::Enter)) {
                        return Task::none();
                    }
                    if let Some(event) =
                        self.profile_controls
                            .key(key.as_ref(), modifiers, &self.chart.profiles)
                    {
                        return self.update(Message::Profile(event));
                    }
                    return Task::none();
                } else if let Some(placement) = &mut self.profile_range {
                    match key.as_ref() {
                        Key::Named(Named::Escape) => {
                            return self
                                .update(Message::Profile(profile_controls::Event::RangeCancel));
                        }
                        Key::Named(Named::Enter) => {
                            let time = placement.cursor_ms;
                            return self.update(Message::Profile(
                                profile_controls::Event::RangePoint(time),
                            ));
                        }
                        Key::Named(Named::ArrowLeft | Named::ArrowRight) => {
                            let sign = if matches!(key.as_ref(), Key::Named(Named::ArrowLeft)) {
                                -1
                            } else {
                                1
                            };
                            let step = self
                                .chart
                                .market
                                .interval
                                .seconds()
                                .saturating_mul(1000)
                                .saturating_mul(if modifiers.shift() { 10 } else { 1 });
                            placement.cursor_ms = placement
                                .cursor_ms
                                .saturating_add(step.saturating_mul(sign));
                            let time = placement.cursor_ms;
                            let price = self.chart.latest().map_or(0.0, |bar| bar.close);
                            self.chart
                                .dispatch(Action::Inspect(Some((time as f64 / 1000.0, price))));
                        }
                        _ => {}
                    }
                    return Task::none();
                } else if self.manager.open {
                    if modifiers.control() && matches!(key.as_ref(), Key::Character("j" | "J")) {
                        if let Some(id) = self.manager.selected() {
                            return self.update(Message::IndicatorDetails(id));
                        }
                        return Task::none();
                    }
                    if matches!(key.as_ref(), Key::Named(Named::Escape))
                        || (!captured && matches!(key.as_ref(), Key::Character("i" | "I")))
                    {
                        return self.update(Message::Action(Action::ManageIndicators));
                    }
                    if matches!(key.as_ref(), Key::Named(Named::Tab)) {
                        let task = if modifiers.shift() {
                            iced::widget::focus_previous()
                        } else {
                            iced::widget::focus_next()
                        };
                        return task.chain(self.manager.reveal_focus());
                    }
                    if matches!(key.as_ref(), Key::Named(Named::PageUp | Named::PageDown)) {
                        return self.manager.scroll_parameters(
                            if matches!(key.as_ref(), Key::Named(Named::PageUp)) {
                                -300.0
                            } else {
                                300.0
                            },
                        );
                    }
                    if let Some(event) =
                        self.manager
                            .key(key.as_ref(), modifiers, captured, &self.chart)
                    {
                        return self.update(Message::Manager(event));
                    }
                } else if self.drawing_controls.is_open() {
                    if matches!(key.as_ref(), Key::Named(Named::Escape)) {
                        self.drawing_interaction.cancel();
                    }
                    if matches!(key.as_ref(), Key::Named(Named::Tab)) {
                        return if modifiers.shift() {
                            iced::widget::focus_previous()
                        } else {
                            iced::widget::focus_next()
                        };
                    }
                    if matches!(key.as_ref(), Key::Named(Named::PageUp | Named::PageDown)) {
                        let y = if matches!(key.as_ref(), Key::Named(Named::PageUp)) {
                            -350.0
                        } else {
                            350.0
                        };
                        return scrollable::scroll_by(
                            scrollable::Id::new(drawing_controls::BODY_ID),
                            scrollable::AbsoluteOffset { x: 0.0, y },
                        );
                    }
                    if let Some(event) =
                        self.drawing_controls
                            .key(key.as_ref(), modifiers, &self.drawings)
                    {
                        return self.update(Message::DrawingControl(event));
                    }
                } else if !captured {
                    if let Some(id) = self.selected_profile
                        && self.chart.active_pane.is_none()
                        && self.drawings.selected().is_none()
                        && self.drawing_interaction.tool() == DrawingTool::Select
                    {
                        if matches!(key.as_ref(), Key::Named(Named::Escape)) {
                            self.selected_profile = None;
                            return Task::none();
                        }
                        if modifiers.control()
                            && !modifiers.alt()
                            && matches!(key.as_ref(), Key::Character("e" | "E"))
                        {
                            return self.profile_event(profile_controls::Event::Open(Some(id)));
                        }
                        if matches!(key.as_ref(), Key::Named(Named::Delete)) {
                            self.selected_profile = None;
                            return self.profile_event(profile_controls::Event::Remove(id));
                        }
                        if modifiers.control()
                            && !modifiers.alt()
                            && matches!(key.as_ref(), Key::Character("h" | "H"))
                        {
                            return self.profile_event(profile_controls::Event::ToggleVisible(id));
                        }
                    }
                    if modifiers.control()
                        && !modifiers.alt()
                        && matches!(key.as_ref(), Key::Character("g" | "G"))
                    {
                        return self.update(Message::GotoTimeOpen);
                    }
                    if modifiers.control() && modifiers.alt() {
                        if modifiers.shift()
                            && matches!(key.as_ref(), Key::Named(Named::ArrowUp | Named::ArrowDown))
                        {
                            return self.chart.active_pane.map_or_else(Task::none, |id| {
                                self.update(Message::Action(Action::MovePane {
                                    id,
                                    direction: if matches!(key.as_ref(), Key::Named(Named::ArrowUp))
                                    {
                                        -1
                                    } else {
                                        1
                                    },
                                }))
                            });
                        }
                        if matches!(key.as_ref(), Key::Character("m" | "M")) {
                            return self.update(Message::Action(Action::TogglePaneMaximized(
                                self.chart.active_pane,
                            )));
                        }
                        if let Some(event) =
                            self.drawing_controls
                                .key(key.as_ref(), modifiers, &self.drawings)
                        {
                            return self.update(Message::DrawingControl(event));
                        }
                    }
                    if modifiers.control()
                        && modifiers.shift()
                        && matches!(key.as_ref(), Key::Character("d" | "D"))
                    {
                        return self.update(Message::DrawingMenu);
                    }
                    if modifiers.control()
                        && !modifiers.alt()
                        && matches!(key.as_ref(), Key::Character("z" | "Z"))
                    {
                        self.drawing_interaction.cancel();
                        self.commit_drawing(if modifiers.shift() {
                            DrawingCommand::Redo
                        } else {
                            DrawingCommand::Undo
                        });
                        return Task::none();
                    }
                    if self.chart.active_pane.is_none() {
                        if matches!(key.as_ref(), Key::Named(Named::Escape))
                            && (self.drawing_interaction.cancel() || self.drawings.select(None))
                        {
                            return Task::none();
                        }
                        if let Some(id) = self.drawings.selected() {
                            if matches!(key.as_ref(), Key::Named(Named::Delete)) {
                                self.drawing_interaction.cancel();
                                self.commit_drawing(DrawingCommand::Delete(id));
                                return Task::none();
                            }
                            if modifiers.control()
                                && matches!(key.as_ref(), Key::Character("e" | "E"))
                            {
                                return self.update(Message::DrawingProperties(id));
                            }
                        }
                    }
                    if modifiers.control()
                        && modifiers.alt()
                        && matches!(key.as_ref(), Key::Character("c" | "C"))
                    {
                        return self.chart.active_pane.map_or_else(Task::none, |id| {
                            self.update(Message::Action(Action::TogglePaneCollapsed(id)))
                        });
                    }
                    if !modifiers.control()
                        && !modifiers.alt()
                        && matches!(key.as_ref(), Key::Character("s" | "S"))
                    {
                        return self.update(Message::StatusDetails);
                    }
                    if modifiers.control()
                        && matches!(
                            key.as_ref(),
                            Key::Named(Named::ArrowLeft | Named::ArrowRight)
                        )
                    {
                        let intervals = self.native_intervals();
                        if let Some(index) = intervals
                            .iter()
                            .position(|interval| *interval == self.chart.market.interval)
                        {
                            let next = if matches!(key.as_ref(), Key::Named(Named::ArrowLeft)) {
                                (index + intervals.len() - 1) % intervals.len()
                            } else {
                                (index + 1) % intervals.len()
                            };
                            return self.update(Message::SelectInterval(intervals[next]));
                        }
                        return Task::none();
                    }
                    if modifiers.control()
                        && let Some(id) = self.chart.active_pane
                    {
                        let local = match key.as_ref() {
                            Key::Character("e" | "E") => Some(Message::IndicatorSettings(id)),
                            Key::Character("j" | "J") => Some(Message::IndicatorDetails(id)),
                            Key::Character("h" | "H") => {
                                Some(Message::Action(Action::ToggleIndicator(id)))
                            }
                            Key::Character("d" | "D") => {
                                Some(Message::Action(Action::RemoveIndicator(id)))
                            }
                            _ => None,
                        };
                        if let Some(message) = local {
                            return self.update(message);
                        }
                    }
                    if modifiers.control() && matches!(key.as_ref(), Key::Named(Named::Tab)) {
                        self.cycle_scale_target(if modifiers.shift() { -1 } else { 1 });
                    } else if let Some(action) = key_action(key.as_ref(), modifiers) {
                        return self.update(Message::Action(self.scale_action(action)));
                    }
                }
            }
            Message::HelpOpen => {
                self.drawing_interaction.cancel();
                self.profile_range = None;
                self.help_open = true;
                self.help_selection = 0;
            }
            Message::HelpClose => {
                return self.close_help();
            }
            Message::HelpActivate => {
                return self.activate_help_selection();
            }
            Message::HelpBackdrop => {}
            Message::HelpAction(action) => {
                if matches!(&action, Action::ManageIndicators | Action::ManageMarket) {
                    self.help_open = false;
                    return self.update(Message::Action(action));
                }
                let focus = self.close_help();
                self.details = None;
                let task = self.update(Message::Action(self.scale_action(action)));
                return Task::batch([task, focus]);
            }
            Message::SelectInterval(interval) => {
                if self.help_open
                    || self.manager.open
                    || self.selector.open
                    || self.details.is_some()
                {
                    return Task::none();
                }
                let Some(catalog) = self.active_catalog.clone() else {
                    return Task::none();
                };
                let mut market = self.chart.market.clone();
                market.interval = interval;
                return self.switch_market(market, catalog);
            }
            Message::IndicatorSettings(id) => {
                if self.help_open
                    || !self
                        .chart
                        .indicators
                        .instances
                        .iter()
                        .any(|item| item.id == id)
                {
                    return Task::none();
                }
                self.drawing_interaction.cancel();
                self.selector.open = false;
                self.details = None;
                self.chart.indicator_error = None;
                let focus = self.manager.edit(id, &self.chart);
                return Task::batch([focus, self.ensure_source_catalogs()]);
            }
            Message::IndicatorDetails(id) => {
                if !self.help_open
                    && !self.selector.open
                    && self
                        .chart
                        .indicators
                        .instances
                        .iter()
                        .any(|item| item.id == id)
                {
                    self.drawing_interaction.cancel();
                    self.manager.open = false;
                    self.details = Some(Details::Indicator(id));
                }
            }
            Message::StatusDetails => {
                if !self.help_open && !self.manager.open && !self.selector.open {
                    self.drawing_interaction.cancel();
                    self.details = Some(Details::Status);
                }
            }
            Message::DetailsClose => {
                if !self.help_open {
                    self.details = None;
                }
            }
            Message::EditorClose => {
                if !self.help_open {
                    self.manager.open = false;
                    self.selector.open = false;
                }
            }
            Message::ModalBackdrop => {}
            Message::UseChartSource(id) => {
                if self.help_open || self.manager.open || self.selector.open {
                    return Task::none();
                }
                let Some(instance) = self
                    .chart
                    .indicators
                    .instances
                    .iter()
                    .find(|item| item.id == id)
                else {
                    return Task::none();
                };
                if instance.source.is_chart() {
                    return Task::none();
                }
                let spec = instance.spec;
                self.details = None;
                return self.update(Message::Action(Action::ApplyIndicator {
                    id: Some(id),
                    spec,
                    source: IndicatorSource::default(),
                }));
            }
            Message::RetryIndicatorSources(id) => {
                if self.help_open || self.manager.open || self.selector.open || self.goto_time.open
                {
                    return Task::none();
                }
                return self.retry_indicator_sources(id);
            }
            Message::Manager(event) => {
                let discovery = if matches!(&event, indicator_manager::Event::SourceRetry) {
                    self.retry_source_discovery_task()
                } else {
                    Task::none()
                };
                if matches!(
                    &event,
                    indicator_manager::Event::Kind(_)
                        | indicator_manager::Event::Field(..)
                        | indicator_manager::Event::Reset(_)
                        | indicator_manager::Event::Select(_)
                        | indicator_manager::Event::New
                ) {
                    self.chart.indicator_error = None;
                }
                let (action, focus) = self.manager.update(event, &self.chart);
                if let Some(action) = action {
                    return Task::batch([self.update(Message::Action(action)), focus, discovery]);
                }
                return Task::batch([focus, discovery, self.ensure_source_catalogs()]);
            }
            Message::Selector(event) => {
                if let market_selector::Event::Quick(market) = event {
                    return self.update(Message::QuickMarket(market));
                }
                if matches!(&event, market_selector::Event::Retry) {
                    let (exchange, product) = self.selector.scope();
                    return self.retry_catalog(exchange, product);
                }
                let prior_scope = self.selector.scope();
                let favorite = matches!(&event, market_selector::Event::ToggleFavorite);
                let submitting = matches!(&event, market_selector::Event::Submit);
                if let Some(action) = self.selector.update(event) {
                    return self.update(Message::Action(action));
                }
                if favorite {
                    self.workspace.mark_dirty();
                }
                let scope = self.selector.scope();
                if scope != prior_scope {
                    return Task::batch([
                        self.selector.follow_selection(),
                        self.load_catalog(scope.0, scope.1),
                    ]);
                }
                if submitting {
                    return self.selector.focus_search();
                }
                return self.selector.follow_selection();
            }
            Message::SourceCatalog {
                exchange,
                product,
                generation,
                result,
            } => {
                if generation != self.source_catalog_generation
                    || product != self.chart.market.product
                    || !self.source_catalog_pending.remove(&(exchange, product))
                {
                    return Task::none();
                }
                let result = source_controller::validated_source_catalog(exchange, product, result);
                self.source_catalogs
                    .insert((exchange, product), result.clone());
                self.manager
                    .accept_source_catalog(exchange, product, result);
                self.sync_indicator_sources();
            }
            Message::SourceData(batch) => {
                self.accept_source_batch(batch);
                return Task::none();
            }
            Message::Catalog {
                exchange,
                product,
                generation,
                result,
            } => {
                // Only authoritative current discovery can change the All exchange universe.
                if generation != self.catalog_generation
                    || (exchange, product) != self.selector.scope()
                {
                    return Task::none();
                }
                let result = source_controller::validated_source_catalog(exchange, product, result);
                self.source_catalog_pending.remove(&(exchange, product));
                self.source_catalogs
                    .insert((exchange, product), result.clone());
                self.manager
                    .accept_source_catalog(exchange, product, result.clone());
                self.selector.set_catalog(exchange, product, result);
                if (exchange, product) == (self.chart.market.exchange, self.chart.market.product)
                    && let Some(instrument) =
                        self.selector.active_instrument(&self.chart.market.symbol)
                {
                    self.active_catalog = self.selector.catalog().cloned();
                    self.chart.price_scale = instrument.price_scale;
                    self.native_quote = instrument.quote_coin.to_string();
                    self.native_volume_unit = instrument.volume_unit.clone();
                    self.chart.base_asset = instrument.base_coin.clone();
                    self.clear_geometry();
                }
                if self.workspace.waiting_market {
                    return self.complete_workspace_market_validation();
                }
                if let Some(market) = self.selector.take_ready_quick()
                    && let Some(catalog) = self.selector.catalog().cloned()
                {
                    return self.switch_market(market, catalog);
                }
                self.sync_indicator_sources();
                return Task::batch([
                    if self.selector.open {
                        self.selector.follow_selection()
                    } else {
                        Task::none()
                    },
                    self.ensure_source_catalogs(),
                ]);
            }
            Message::Tick => {
                if self.theme_file.reload() {
                    self.clear_geometry();
                }
                self.open_interest_history.retry_ticks =
                    self.open_interest_history.retry_ticks.saturating_sub(1);
                self.refresh_source_panes();
                if self.history.failed.is_some() && !self.history.in_flight {
                    self.history.retry_ticks += 1;
                    if self.history.retry_ticks >= 5 {
                        self.history.retry_ticks = 0;
                        if self.history.pending_sync {
                            self.history.pending_sync = false;
                            return self.fetch(Request::Recent);
                        }
                        return self.fetch(self.history.failed.unwrap());
                    }
                }
            }
            Message::Action(action) => {
                if self.help_open {
                    return Task::none();
                }
                // Widget input is blocked behind dialogs; also reject queued canvas actions.
                if (self.manager.open
                    || self.selector.open
                    || self.profile_controls.open
                    || self.details.is_some()
                    || self.drawing_controls.is_open()
                    || self.goto_time.open)
                    && !matches!(&action, Action::ManageIndicators | Action::ManageMarket)
                    && !(self.selector.open && matches!(&action, Action::SelectMarket(_)))
                    && !(self.manager.open
                        && matches!(
                            &action,
                            Action::AddIndicator(_)
                                | Action::ConfigureIndicator { .. }
                                | Action::RemoveIndicator(_)
                                | Action::ApplyIndicator { .. }
                                | Action::ToggleIndicator(_)
                        ))
                {
                    return Task::none();
                }
                let leaves_goto = match &action {
                    Action::Reset | Action::Live | Action::SelectMarket(_) => true,
                    Action::Pan { time, .. } | Action::PanePan { time, .. } => *time != 0.0,
                    Action::Zoom { time, .. } | Action::PaneZoom { time, .. } => *time != 1.0,
                    Action::InspectStep(_) => true,
                    _ => false,
                };
                if leaves_goto {
                    self.goto_target = None;
                    self.goto_page_oldest = None;
                    self.goto_notice = None;
                }
                if matches!(&action, Action::ManageIndicators | Action::ManageMarket) {
                    self.goto_time.open = false;
                    self.profile_controls.open = false;
                    self.profile_range = None;
                }
                if let Action::ActivatePane(id) = &action
                    && self.chart.maximized()
                    && !self.chart.pane_should_render(*id)
                {
                    self.chart.dispatch(Action::TogglePaneMaximized(*id));
                }
                if matches!(&action, Action::ManageIndicators | Action::ManageMarket) {
                    self.drawing_interaction.cancel();
                    self.drawing_controls.close();
                } else if matches!(&action, Action::ActivatePane(Some(_))) {
                    self.drawing_interaction.cancel();
                }
                match action {
                    Action::TogglePaneMaximized(target) => {
                        self.chart.dispatch(Action::TogglePaneMaximized(target));
                        if let Some(target) = self.chart.maximized_target() {
                            self.chart.dispatch(Action::ActivatePane(target));
                        }
                        self.clear_geometry();
                        return Task::none();
                    }
                    Action::ManageIndicators => {
                        self.details = None;
                        if self.manager.open {
                            self.manager.open = false;
                        } else {
                            self.selector.open = false;
                            let focus = self.manager.show();
                            return Task::batch([focus, self.ensure_source_catalogs()]);
                        }
                    }
                    Action::ManageMarket => {
                        self.details = None;
                        if self.selector.open {
                            self.selector.open = false;
                        } else {
                            self.manager.open = false;
                            let focus = self.selector.show(
                                &self.chart.market,
                                Quote::parse(&self.native_quote).unwrap_or_default(),
                            );
                            let (exchange, product) = self.selector.scope();
                            let catalog = if self.selector.catalog_error.is_some()
                                || self
                                    .selector
                                    .active_instrument(&self.chart.market.symbol)
                                    .is_none()
                            {
                                self.load_catalog(exchange, product)
                            } else {
                                Task::none()
                            };
                            return Task::batch([focus, catalog]);
                        }
                    }
                    Action::SelectMarket(market) => {
                        if self.selector.instrument(&market.symbol).is_none()
                            || (market.exchange, market.product) != self.selector.scope()
                            || !self.selector.intervals().contains(&market.interval)
                        {
                            self.selector.reject("Choose an available native market and interval from the selected scope");
                            return self.selector.focus_search();
                        }
                        let Some(catalog) = self.selector.catalog().cloned() else {
                            return Task::none();
                        };
                        return self.switch_market(market, catalog);
                    }
                    Action::ScrollPanes { amount, legends } => {
                        return scrollable::scroll_by(
                            scrollable::Id::new(if legends {
                                "indicator-legends"
                            } else {
                                "indicator-panes"
                            }),
                            scrollable::AbsoluteOffset { x: 0.0, y: amount },
                        );
                    }
                    Action::Retry => {
                        if self.workspace.waiting_market {
                            return self.retry_catalog(
                                self.chart.market.exchange,
                                self.chart.market.product,
                            );
                        }
                        self.theme_file.reload();
                        self.clear_geometry();
                        // One generation invalidates old native subscriptions and history tasks.
                        let repair_anchor = self.history.anchor;
                        self.restart_connection();
                        let now = source_controller::now_ms();
                        for source in self.source_inputs.values_mut() {
                            source.flow_connected = false;
                            source.oi_connected = false;
                            source.revoke_unfinished(now);
                            source.revision = source.revision.wrapping_add(1);
                        }
                        for request in self.source_requests.values_mut() {
                            self.next_source_generation =
                                self.next_source_generation.wrapping_add(1);
                            request.generation = self.next_source_generation;
                        }
                        self.history.anchor = repair_anchor;
                        self.status = "Reconnecting to native market data…".into();
                        return Task::batch([
                            self.sync(),
                            self.load_catalog(
                                self.chart.market.exchange,
                                self.chart.market.product,
                            ),
                        ]);
                    }
                    Action::Older => return self.older(),
                    _ => {
                        if matches!(
                            &action,
                            Action::AdjustPaneHeight { .. }
                                | Action::ResetPaneHeight(_)
                                | Action::MovePane { .. }
                                | Action::TogglePaneCollapsed(_)
                                | Action::TogglePaneMaximized(_)
                                | Action::ToggleIndicator(_)
                                | Action::AddIndicator(_)
                                | Action::RemoveIndicator(_)
                                | Action::ConfigureIndicator { .. }
                                | Action::ApplyIndicator { .. }
                                | Action::Reset
                        ) {
                            // Non-pointer layout commands end an owned resize.
                            // SetPaneHeight is intentionally exempt: its replies
                            // are the resize itself, not a new input context.
                            self.pointer_generation = self.pointer_generation.wrapping_add(1);
                        }
                        let changes_instances = matches!(
                            &action,
                            Action::AddIndicator(_)
                                | Action::ConfigureIndicator { .. }
                                | Action::RemoveIndicator(_)
                                | Action::ApplyIndicator { .. }
                        );
                        let applies_parameters = matches!(
                            &action,
                            Action::AddIndicator(_)
                                | Action::ConfigureIndicator { .. }
                                | Action::ApplyIndicator { .. }
                        );
                        let pane_only = match &action {
                            Action::PanePan { id, time, .. } if *time == 0.0 => Some(*id),
                            Action::PaneZoom { id, time, .. } if *time == 1.0 => Some(*id),
                            Action::FitPane(id) => Some(*id),
                            Action::AutoFit => self.chart.active_pane,
                            _ => None,
                        };
                        let had_cvd = self.has_cvd();
                        let had_open_interest = self.has_open_interest();
                        let removal_focus = match &action {
                            Action::RemoveIndicator(id) => {
                                self.manager.prepare_removal(*id, &self.chart)
                            }
                            _ => Task::none(),
                        };
                        let durable = workspace_runtime::durable_action(&action);
                        let geometry_changed = self.chart.dispatch(action);
                        if durable && (!applies_parameters || self.chart.indicator_error.is_none())
                        {
                            self.workspace.mark_dirty();
                        }
                        if geometry_changed {
                            if let Some(id) = pane_only {
                                if let Some(cache) = self.pane_caches.get(&id) {
                                    cache.clear();
                                }
                            } else {
                                self.clear_geometry();
                            }
                        }
                        let focus = Task::batch([
                            removal_focus,
                            if applies_parameters && self.chart.indicator_error.is_none() {
                                self.manager.applied(&self.chart)
                            } else {
                                if applies_parameters
                                    && let Some(error) = &self.chart.indicator_error
                                {
                                    self.manager.reject_source(error.clone());
                                }
                                Task::none()
                            },
                        ]);
                        if changes_instances {
                            self.pane_caches.retain(|id, _| {
                                self.chart
                                    .indicators
                                    .instances
                                    .iter()
                                    .any(|item| item.id == *id && !item.spec.is_overlay())
                            });
                            for instance in &self.chart.indicators.instances {
                                if !instance.spec.is_overlay() {
                                    self.pane_caches.entry(instance.id).or_default();
                                }
                            }
                            self.sync_indicator_sources();
                        }
                        let focus = Task::batch([
                            focus,
                            if changes_instances {
                                self.ensure_source_catalogs()
                            } else {
                                Task::none()
                            },
                        ]);
                        if had_open_interest != self.has_open_interest() {
                            self.open_interest_history.reset();
                            self.chart.open_interest.connected = false;
                            self.chart.open_interest.message =
                                "Loading native open interest…".into();
                            if !self.has_open_interest() {
                                self.chart.open_interest.samples.clear();
                                self.chart.open_interest.support = None;
                            }
                        }
                        let has_cvd = self.has_cvd();
                        if had_cvd != has_cvd {
                            self.flow_generation = self.flow_generation.wrapping_add(1);
                            self.history.flow_error = None;
                            if has_cvd {
                                self.history.progress = Some("Loading native CVD history…".into());
                                self.history.flow_target = self.chart.oldest();
                            } else {
                                self.cancel_flow_history();
                                self.flow_status = None;
                                self.history.progress = None;
                                self.history.flow_target = None;
                            }
                            return Task::batch([focus, self.sync()]);
                        }
                        if self.chart.needs_older() {
                            return Task::batch([focus, self.older()]);
                        }
                        return focus;
                    }
                }
            }
            Message::MarketData(event) => {
                if !self.accepts(&event.scope.market, event.scope.generation)
                    || event.scope.flow_generation != self.flow_generation
                    || event.scope.cvd != self.has_cvd()
                {
                    return Task::none();
                }
                let mut flows = ArrayVec::<FlowBar, 64>::new();
                let mut repair = false;
                for event in event.events.into_vec() {
                    if let MarketEvent::Flow { bar } = event {
                        flows.push(bar);
                        continue;
                    }
                    self.flush_flows(&mut flows, true);
                    match event {
                        MarketEvent::Status { connected, message } => {
                            self.upstream_connected = connected;
                            self.status = message;
                            if !connected {
                                self.remember_anchor();
                                if self.chart.revoke_live_flow() {
                                    self.clear_flow_geometry();
                                }
                            }
                        }
                        MarketEvent::FlowStatus { connected, message } => {
                            if !connected && self.chart.revoke_live_flow() {
                                self.clear_flow_geometry();
                            }
                            self.flow_status = Some(if connected {
                                message
                            } else {
                                format!("Flow unavailable: {message}")
                            });
                        }
                        MarketEvent::Resync => {
                            self.remember_anchor();
                            repair = true;
                        }
                        MarketEvent::Candle { candle } => {
                            if self.chart.latest().is_some_and(|latest| {
                                candle.time > latest.time + self.chart.market.interval.seconds()
                            }) {
                                self.remember_anchor();
                                repair = true;
                            }
                            if self.chart.merge_stream(candle) {
                                self.clear_geometry();
                            }
                        }
                        MarketEvent::Flow { .. } => unreachable!(),
                    }
                }
                self.flush_flows(&mut flows, true);
                if repair {
                    return self.sync();
                }
            }
            Message::OpenInterest(batch) => {
                if !self.accepts(&batch.scope.market, batch.scope.generation)
                    || batch.scope.open_interest_generation != self.open_interest_history.generation
                    || !self.has_open_interest()
                {
                    return Task::none();
                }
                let mut samples = ArrayVec::<OpenInterestSample, 64>::new();
                for event in batch.events.into_vec() {
                    if let OpenInterestEvent::Sample(sample) = event {
                        samples.push(sample);
                        continue;
                    }
                    self.flush_open_interest(&mut samples);
                    match event {
                        OpenInterestEvent::Support(support) => {
                            self.chart.open_interest.support = Some(support);
                        }
                        OpenInterestEvent::History(page) if page.market == self.chart.market => {
                            if self.chart.merge_open_interest_samples(&page.samples) {
                                self.clear_open_interest_geometry();
                            }
                            self.chart.open_interest.support = Some(page.support.clone());
                            if !self.open_interest_history.initialized {
                                self.open_interest_history.initialized = true;
                                self.open_interest_history.next_before_ms = page.next_before_ms;
                            } else if let (Some(cursor), Some(next)) = (
                                self.open_interest_history.next_before_ms,
                                page.next_before_ms,
                            ) {
                                self.open_interest_history.next_before_ms = Some(next.min(cursor));
                            }
                        }
                        OpenInterestEvent::History(_) => {
                            self.chart.open_interest.connected = false;
                            self.chart.open_interest.message =
                                "Native OI returned history for a different market".into();
                        }
                        OpenInterestEvent::Status { connected, message } => {
                            self.chart.open_interest.connected = connected;
                            self.chart.open_interest.message = message;
                        }
                        OpenInterestEvent::Sample(_) => unreachable!(),
                    }
                    self.clear_open_interest_geometry();
                }
                self.flush_open_interest(&mut samples);
                return self.fetch_older_open_interest();
            }
            Message::OpenInterestHistory {
                market,
                generation,
                open_interest_generation,
                before_ms,
                result,
            } => {
                if !self.accepts(&market, generation)
                    || open_interest_generation != self.open_interest_history.generation
                    || !self.has_open_interest()
                {
                    return Task::none();
                }
                self.open_interest_history.in_flight = false;
                self.open_interest_history.cancel = None;
                match result {
                    Ok(page) if page.market == self.chart.market => {
                        self.open_interest_history.next_before_ms =
                            page.next_before_ms.filter(|cursor| *cursor < before_ms);
                        self.open_interest_history.error = None;
                        if self.chart.merge_open_interest_samples(&page.samples) {
                            self.clear_open_interest_geometry();
                        }
                    }
                    Ok(_) => {
                        self.open_interest_history.error =
                            Some("Native OI returned history for a different market".into());
                        self.open_interest_history.retry_ticks = 5;
                    }
                    Err(error) => {
                        self.open_interest_history.error = Some(error);
                        self.open_interest_history.retry_ticks = 5;
                    }
                }
                return self.fetch_older_open_interest();
            }
            Message::History {
                request,
                market,
                generation,
                history_id,
                epoch,
                result,
            } => {
                if !self.accepts(&market, generation) || history_id != self.history.generation {
                    return Task::none();
                }
                match result {
                    Ok(page) if page.market == self.chart.market => {
                        self.apply_history_page(Arc::new(page), request, epoch);
                        return self.finish_history(request);
                    }
                    Ok(_) => {
                        self.fail_history(
                            request,
                            "Native source returned history for a different market".into(),
                        );
                    }
                    Err(error) => self.fail_history(request, error),
                }
            }
            Message::HistoryFlow {
                request,
                market,
                generation,
                history_id,
                epoch,
                flow_generation,
                events,
            } => {
                if !self.accepts(&market, generation)
                    || history_id != self.history.generation
                    || !self.history.in_flight
                {
                    return Task::none();
                }
                let current_flow = flow_generation == self.flow_generation && self.has_cvd();
                let events = match events {
                    Ok(events) => events,
                    Err(error) if current_flow => {
                        self.history.flow_error = Some(error.clone());
                        self.fail_history(request, format!("CVD history: {error}"));
                        return Task::none();
                    }
                    Err(_) => return self.finish_stale_flow_history(),
                };
                let mut flows = ArrayVec::<FlowBar, 64>::new();
                for event in events.into_vec() {
                    if let HistoryEvent::Flow { bar } = event {
                        if current_flow {
                            flows.push(bar);
                        }
                        continue;
                    }
                    self.flush_flows(&mut flows, false);
                    match event {
                        HistoryEvent::History { page } => {
                            if page.market != self.chart.market {
                                self.fail_history(
                                    request,
                                    "Native source returned history for a different market".into(),
                                );
                                return Task::none();
                            }
                            self.apply_history_page(page, request, epoch);
                        }
                        HistoryEvent::FlowStatus { message } if current_flow => {
                            self.history.progress = Some(message);
                        }
                        HistoryEvent::Finished if current_flow => {
                            self.history.flow_error = None;
                            self.history.progress = None;
                            return self.finish_history(request);
                        }
                        HistoryEvent::Error { message } if current_flow => {
                            self.history.flow_error = Some(message.clone());
                            self.fail_history(request, format!("CVD history: {message}"));
                            return Task::none();
                        }
                        HistoryEvent::Finished | HistoryEvent::Error { .. } => {
                            return self.finish_stale_flow_history();
                        }
                        HistoryEvent::FlowStatus { .. } => {}
                        HistoryEvent::Flow { .. } => unreachable!(),
                    }
                }
                self.flush_flows(&mut flows, false);
            }
        }
        self.fetch_older_open_interest()
    }

    fn subscription(&self) -> Subscription<Message> {
        let pointer_lifecycle = iced::event::listen_with(|event, _, _| match event {
            iced::Event::Window(
                iced::window::Event::Unfocused | iced::window::Event::Resized(_),
            ) => Some(Message::PointerCancelled),
            _ => None,
        });
        if self.stress.is_some() {
            return Subscription::batch([
                pointer_lifecycle,
                iced::time::every(Duration::from_millis(200)).map(|_| Message::StressPulse),
                iced::window::frames().map(Message::StressFrame),
            ]);
        }
        if self.workspace.waiting_market {
            return Subscription::batch([
                pointer_lifecycle,
                Self::workspace_keyboard_subscription(),
                iced::window::close_requests().map(Message::CloseRequested),
                iced::time::every(Duration::from_secs(1)).map(|_| Message::Tick),
                self.workspace_save_subscription(),
            ]);
        }
        Subscription::batch([
            pointer_lifecycle,
            iced::window::close_requests().map(Message::CloseRequested),
            self.workspace_save_subscription(),
            Subscription::run_with_id(
                (
                    self.market_generation,
                    self.flow_generation,
                    self.chart.market.clone(),
                    self.has_cvd(),
                ),
                feeds::live(
                    self.data.clone(),
                    feeds::Scope {
                        market: self.chart.market.clone(),
                        generation: self.market_generation,
                        flow_generation: self.flow_generation,
                        cvd: self.has_cvd(),
                    },
                ),
            )
            .map(Message::MarketData),
            if self.has_open_interest() {
                Subscription::run_with_id(
                    (
                        "open_interest",
                        self.market_generation,
                        self.open_interest_history.generation,
                        self.chart.market.clone(),
                    ),
                    feeds::open_interest(
                        self.data.clone(),
                        feeds::OpenInterestScope {
                            market: self.chart.market.clone(),
                            generation: self.market_generation,
                            open_interest_generation: self.open_interest_history.generation,
                        },
                    ),
                )
                .map(Message::OpenInterest)
            } else {
                Subscription::none()
            },
            self.source_subscription(),
            self.profiles
                .subscription(&self.data)
                .map(Message::ProfileData),
            iced::time::every(Duration::from_secs(1)).map(|_| Message::Tick),
            Self::workspace_keyboard_subscription(),
        ])
    }

    fn theme(&self) -> Theme {
        self.theme_file.theme.clone()
    }

    fn title(&self) -> String {
        if let Some(metrics) = &self.stress {
            return metrics.label();
        }
        format!(
            "{} · {} · {} · {} · {}",
            self.chart.market.exchange,
            self.chart.market.product,
            self.chart.market.symbol,
            self.chart.market.interval,
            self.native_quote
        )
    }

    fn status_details_view(&self) -> Element<'_, Message> {
        let mut details = column![
            text("Prices & candle history").size(17),
            text(format!(
                "{} {} · {} · {} · {} candles",
                self.chart.market.exchange,
                self.chart.market.product,
                self.chart.market.symbol,
                self.chart.market.interval,
                self.chart.len()
            ))
            .size(13),
            text(format!(
                "Connection: {} · {}",
                if self.upstream_connected {
                    "live"
                } else {
                    "offline"
                },
                self.status
            ))
            .size(13),
            text(if self.chart.viewport.live {
                "Viewport follows realtime."
            } else {
                "Viewport is historical; data continues updating."
            })
            .size(13),
            text(if self.history.in_flight {
                "Loading candle history…"
            } else if self.chart.initialized && self.chart.next_before.is_none() {
                "Start of native candle history reached."
            } else {
                "History idle; older pages load at the left edge."
            })
            .size(13),
        ]
        .spacing(8);
        if let Some(error) = &self.history.error {
            details = details.push(text(format!("History error: {error}")).size(13));
        }
        if self.has_cvd() {
            details = details.push(text("Chart CVD").size(17));
            if let Some(status) = &self.flow_status {
                details = details.push(text(format!("Flow: {status}")).size(13));
            }
            if let Some(progress) = &self.history.progress {
                details = details.push(text(format!("History: {progress}")).size(13));
            }
            if let Some(error) = &self.history.flow_error {
                details = details.push(text(format!("History error: {error}")).size(13));
            }
            details = details.push(text("Per-instance legends show measured quality and accumulation origin; price connectivity does not imply complete CVD coverage.").size(12));
        }
        if self.has_open_interest() {
            details = details.push(text("Chart open interest").size(17));
            let native = &self.chart.open_interest;
            if let Some(support) = &native.support {
                details = details.push(text(format!("Source: {}", support.description)).size(13));
            }
            if !native.message.is_empty() {
                details = details.push(text(format!("Status: {}", native.message)).size(13));
            }
            if let Some(error) = &self.open_interest_history.error {
                details = details.push(text(format!("History error: {error}")).size(13));
            }
            details = details.push(text("Freshness, actual observation times and timestamp provenance are shown on each OI instance.").size(12));
        }
        if self
            .chart
            .indicators
            .instances
            .iter()
            .any(|item| !item.source.is_chart())
        {
            details = details.push(text("Independent indicator sources").size(17));
            for instance in self
                .chart
                .indicators
                .instances
                .iter()
                .filter(|item| !item.source.is_chart())
            {
                details = details.push(
                    button(
                        text(format!(
                            "#{} {} · {} · {}",
                            instance.id,
                            instance.spec.label(),
                            instance.source.mode,
                            instance
                                .external
                                .as_ref()
                                .map_or("Awaiting native data", |output| output.status.as_str())
                        ))
                        .size(13),
                    )
                    .on_press(Message::IndicatorDetails(instance.id))
                    .width(Length::Fill),
                );
            }
        }
        details = details.push(text("Market discovery").size(17));
        if self.selector.scope() == (self.chart.market.exchange, self.chart.market.product) {
            if self.selector.loading {
                details = details.push(text("Loading native market catalog…").size(13));
            } else if let Some(error) = &self.selector.catalog_error {
                details = details
                    .push(text(format!("Catalog error: {error}")).size(13))
                    .push(
                        button("Retry catalog")
                            .on_press(Message::Selector(market_selector::Event::Retry)),
                    );
            } else {
                details = details.push(text("Native catalog available.").size(13));
            }
        } else {
            details = details.push(
                text(if self.active_catalog.is_some() {
                    "Active native catalog retained independently of the cancelled market draft."
                } else {
                    "Awaiting active native catalog."
                })
                .size(13),
            );
        }
        details = details.push(text("Theme").size(17));
        details = details.push(
            text(self.theme_file.error.as_ref().map_or_else(
                || "Palette loaded.".to_owned(),
                |error| format!("{error} — keeping last valid palette; retrying every second"),
            ))
            .size(13),
        );
        details = details.push(text("Workspace").size(17));
        details = details.push(text(self.workspace.description()).size(13));
        if self.workspace.error.is_some() {
            details = details.push(
                button("Retry workspace save")
                    .on_press(Message::Workspace(workspace_controls::Event::RetrySave)),
            );
        }
        details.into()
    }

    fn details_view(&self, details: Details) -> Element<'_, Message> {
        let (title, body) = match details {
            Details::Status => ("Status details".to_owned(), self.status_details_view()),
            Details::Indicator(id) => {
                let Some(instance) = self
                    .chart
                    .indicators
                    .instances
                    .iter()
                    .find(|item| item.id == id)
                else {
                    return text("Indicator removed").into();
                };
                (
                    format!("#{} {} details", id, instance.spec.label()),
                    chart::indicator_details_view(
                        &self.chart,
                        instance,
                        self.theme_file.palette,
                        &self.native_volume_unit,
                        self.indicator_source_health(instance),
                    ),
                )
            }
        };
        let mut footer = row![].spacing(6);
        match details {
            Details::Status => {
                footer = footer.push(
                    button(text("Retry connection").size(12))
                        .style(ui::quiet)
                        .padding([6, 10])
                        .on_press(Message::HelpAction(Action::Retry)),
                );
            }
            Details::Indicator(id) => {
                footer = footer.push(
                    button(text("Settings").size(12))
                        .style(ui::quiet)
                        .padding([6, 10])
                        .on_press(Message::IndicatorSettings(id)),
                );
            }
        }
        footer = footer
            .push(iced::widget::Space::new(Length::Fill, Length::Shrink))
            .push(
                button(text("Close").size(12))
                    .style(ui::quiet)
                    .padding([6, 10])
                    .on_press(Message::DetailsClose),
            )
            .push(ui::key("Esc"));
        column![
            row![
                text(title).size(20),
                iced::widget::Space::new(Length::Fill, Length::Shrink),
                button(ui::key("F1"))
                    .style(ui::quiet)
                    .padding(2)
                    .on_press(Message::HelpOpen)
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center),
            ui::separator(),
            scrollable(container(body).padding(iced::Padding {
                right: 16.0,
                ..iced::Padding::default()
            }))
            .id(scrollable::Id::new("details-body"))
            .height(Length::Fill),
            ui::separator(),
            footer,
        ]
        .spacing(12)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }

    fn view(&self) -> Element<'_, Message> {
        let started = self.stress.as_ref().map(|_| Instant::now());
        let view = self.view_content();
        if let (Some(started), Some(metrics)) = (started, &self.stress) {
            metrics.view_sample(started.elapsed());
        }
        view
    }

    fn view_content(&self) -> Element<'_, Message> {
        let blocked = self.drawing_blocked();
        let candle = self.chart.inspected().or_else(|| self.chart.latest());
        let readout: Element<'_, Message> = if let Some(candle) = candle {
            let value = |label: &'static str, value: String| {
                row![
                    text(label).size(10).style(ui::muted_text),
                    text(value).size(12).font(iced::Font::MONOSPACE),
                ]
                .spacing(4)
                .align_y(iced::Alignment::Center)
            };
            let volume = tooltip(
                value(
                    "VOL",
                    format!(
                        "{} {}",
                        presentation::quantity(candle.volume),
                        self.native_volume_unit
                    ),
                ),
                text(format!(
                    "Native volume: {} {}",
                    presentation::quantity_exact(candle.volume),
                    self.native_volume_unit
                ))
                .size(12),
                tooltip::Position::Top,
            );
            column![
                text(format!(
                    "{} UTC · {}",
                    chart::utc(candle.time, "%Y-%m-%d %H:%M"),
                    if self.chart.inspected().is_some() {
                        "Inspecting"
                    } else {
                        "Latest candle"
                    }
                ))
                .size(10)
                .font(iced::Font::MONOSPACE)
                .style(ui::muted_text),
                row![
                    value("O", self.chart.format_price(candle.open)),
                    value("H", self.chart.format_price(candle.high)),
                    value("L", self.chart.format_price(candle.low)),
                    value("C", self.chart.format_price(candle.close)),
                    volume,
                ]
                .spacing(10)
                .wrap(),
            ]
            .spacing(4)
            .into()
        } else {
            text(format!(
                "Waiting for native {} · {} candles",
                self.chart.market.symbol, self.chart.market.interval
            ))
            .size(12)
            .style(ui::muted_text)
            .into()
        };
        // Inspection is content, not layout: reserve its single-line slot while
        // any current-native profile is visible, including while data loads.
        let native = NativeInstrument::from_market(&self.chart.market);
        let readout: Element<'_, Message> = if self.chart.profiles.iter().any(|profile| {
            profile.visible
                && profile
                    .instrument
                    .as_ref()
                    .is_none_or(|bound| bound == &native)
        }) {
            let inspection = self
                .profile_inspection()
                .unwrap_or_else(|| iced::widget::Space::new(Length::Fill, Length::Shrink).into());
            column![
                readout,
                container(inspection)
                    .width(Length::Fill)
                    .height(16)
                    .clip(true)
            ]
            .spacing(4)
            .into()
        } else {
            readout
        };
        let drawing_toolbar: Option<Element<'_, Message>> = if self.drawing_interaction.tool()
            != DrawingTool::Select
            || self.drawings.selected().is_some()
        {
            let mut controls = row![].spacing(4).align_y(iced::Alignment::Center);
            if self.drawing_interaction.tool() != DrawingTool::Select {
                controls = controls.push(ui::badge(
                    format!(
                        "{}{}",
                        drawing_tool_name(self.drawing_interaction.tool()),
                        if self.drawing_interaction.repeat() {
                            " · repeat"
                        } else {
                            ""
                        }
                    ),
                    ui::Tone::Accent,
                ));
            }
            if let Some(drawing) = self.drawings.selected_drawing() {
                let generation = self.drawings.generation();
                controls = controls
                    .push(
                        text(format!(
                            "{} #{}{}",
                            drawing_tool_name(drawing.shape.tool()),
                            drawing.id,
                            if drawing.locked { " · locked" } else { "" }
                        ))
                        .size(11),
                    )
                    .push(
                        button(text("Edit").size(11))
                            .style(ui::quiet)
                            .padding([4, 6])
                            .on_press_maybe(
                                (!blocked).then_some(Message::DrawingProperties(drawing.id)),
                            ),
                    )
                    .push(
                        button(text(if drawing.visible { "Hide" } else { "Show" }).size(11))
                            .style(ui::quiet)
                            .padding([4, 6])
                            .on_press_maybe((!blocked).then_some(Message::DrawingCommand {
                                generation,
                                command: DrawingCommand::SetVisible {
                                    id: drawing.id,
                                    visible: !drawing.visible,
                                },
                            })),
                    )
                    .push(
                        button(text(if drawing.locked { "Unlock" } else { "Lock" }).size(11))
                            .style(ui::quiet)
                            .padding([4, 6])
                            .on_press_maybe((!blocked).then_some(Message::DrawingCommand {
                                generation,
                                command: DrawingCommand::SetLocked {
                                    id: drawing.id,
                                    locked: !drawing.locked,
                                },
                            })),
                    )
                    .push(
                        button(text("Delete").size(11))
                            .style(ui::danger)
                            .padding([4, 6])
                            .on_press_maybe((!blocked).then_some(Message::DrawingCommand {
                                generation,
                                command: DrawingCommand::Delete(drawing.id),
                            })),
                    );
                controls = controls.push(self.drawing_controls.context_style_view(&self.drawings));
            } else {
                controls = controls
                    .push(text(drawing_tool_name(self.drawing_interaction.tool())).size(11))
                    .push(
                        text("Click anchors or arrows + Enter")
                            .size(10)
                            .style(ui::muted_text),
                    );
            }
            controls = controls.push(
                iced::widget::checkbox("Repeat", self.drawing_interaction.repeat())
                    .size(14)
                    .text_size(11)
                    .on_toggle(|enabled| {
                        Message::DrawingControl(drawing_controls::Event::Repeat(enabled))
                    }),
            );
            controls = controls.push(
                button(ui::key("Esc"))
                    .style(ui::quiet)
                    .padding(2)
                    .on_press_maybe((!blocked).then_some(Message::DrawingCancel)),
            );
            Some(
                container(controls.wrap())
                    .padding(6)
                    .style(ui::panel)
                    .into(),
            )
        } else {
            None
        };
        let mut charts = column![].spacing(6);
        let has_lower = self
            .chart
            .indicators
            .instances
            .iter()
            .any(|item| item.visible && !item.spec.is_overlay())
            && !self.chart.maximized();
        let mut legends = column![].spacing(2);
        let mut overlay_height = 0.0_f32;
        if self.chart.pane_should_render(None) {
            let instrument = NativeInstrument::from_market(&self.chart.market);
            for profile in self.chart.profiles.iter().filter(|profile| {
                profile.visible
                    && profile
                        .instrument
                        .as_ref()
                        .is_none_or(|key| key == &instrument)
            }) {
                let snapshot = self
                    .profiles
                    .outputs
                    .get(&profile.id)
                    .and_then(|items| items.last());
                let status = if self.profiles.errors.contains_key(&profile.id) {
                    "Unavailable"
                } else if let Some(snapshot) = snapshot {
                    if snapshot.volume_method == mexc_indicators::profiles::VolumeMethod::Estimated
                    {
                        "Estimated"
                    } else if snapshot.volume_method
                        == mexc_indicators::profiles::VolumeMethod::Mixed
                    {
                        "Mixed"
                    } else if !snapshot.covered {
                        "Partial"
                    } else if snapshot.complete {
                        "Complete"
                    } else {
                        "Developing"
                    }
                } else {
                    "Loading"
                };
                let title = if profile.spec.kind == mexc_indicators::profiles::Kind::Volume {
                    format!("#{} Volume · {}", profile.id, self.native_volume_unit)
                } else {
                    format!("#{} TPO · {}", profile.id, profile.spec.range)
                };
                legends = legends.push(
                    row![
                        ui::badge(title, ui::Tone::Accent),
                        ui::badge(
                            status,
                            if status == "Complete" {
                                ui::Tone::Positive
                            } else {
                                ui::Tone::Neutral
                            }
                        ),
                        button(text("Settings").size(11))
                            .style(ui::quiet)
                            .on_press_maybe((!blocked).then_some(Message::Profile(
                                profile_controls::Event::Open(Some(profile.id))
                            ))),
                        button(text("Hide").size(11))
                            .style(ui::quiet)
                            .on_press_maybe((!blocked).then_some(Message::Profile(
                                profile_controls::Event::ToggleVisible(profile.id)
                            ))),
                    ]
                    .spacing(6)
                    .align_y(iced::Alignment::Center),
                );
                overlay_height += 30.0;
            }
        }
        for instance in self.chart.indicators.instances.iter().filter(|item| {
            item.visible && item.spec.is_overlay() && self.chart.pane_should_render(None)
        }) {
            overlay_height += chart::legend_height(instance) + 2.0;
            legends = legends.push(chart::legend_view(
                &self.chart,
                instance,
                self.theme_file.palette,
                &self.native_volume_unit,
                blocked,
                self.indicator_source_health(instance),
            ));
        }
        if overlay_height > 0.0 {
            charts = charts.push(
                scrollable(legends)
                    .id(scrollable::Id::new("indicator-legends"))
                    .height(overlay_height.min(120.0)),
            );
        }
        if self.chart.pane_should_render(None) {
            let height = if has_lower {
                Length::Fixed(self.chart.plot_height(None))
            } else {
                Length::Fill
            };
            let price = chart::view(
                chart::Chart {
                    data: &self.chart,
                    cache: &self.cache,
                    palette: self.theme_file.palette,
                    indicator: None,
                    quote: &self.native_quote,
                    volume_unit: &self.native_volume_unit,
                    drawings: Some(drawing_layer::View {
                        store: &self.drawings,
                        interaction: &self.drawing_interaction,
                        scene: &self.drawing_scene,
                        projection: &self.drawing_projection,
                        native_candles: self.chart.candles(),
                    }),
                    profiles: Some(profile_layer::View {
                        instances: &self.chart.profiles,
                        outputs: &self.profiles.outputs,
                        scene: &self.profile_scene,
                        revision: self.profiles.revision,
                        selected: self.selected_profile,
                        market: &self.chart.market,
                    }),
                    profile_range: self.profile_range.as_ref().map(|placement| {
                        profile_layer::RangePlacement {
                            first: placement.first,
                            cursor_ms: Some(placement.cursor_ms),
                        }
                    }),
                    metrics: self.stress.as_ref(),
                },
                height,
                blocked,
                drawing_toolbar.map(|toolbar| container(toolbar).padding(6).into()),
                self.pointer_generation,
            );
            charts = charts.push(price);
            if has_lower {
                charts = charts.push(pane_resize::view(
                    None,
                    self.chart.plot_height(None),
                    self.theme_file.palette,
                    blocked,
                    self.pointer_generation,
                ));
            }
        }
        for instance in self.chart.indicators.instances.iter().filter(|item| {
            item.visible && !item.spec.is_overlay() && self.chart.pane_should_render(Some(item.id))
        }) {
            if let Some(cache) = self.pane_caches.get(&instance.id) {
                let mut pane = column![chart::legend_view(
                    &self.chart,
                    instance,
                    self.theme_file.palette,
                    &self.native_volume_unit,
                    blocked,
                    self.indicator_source_health(instance),
                )];
                if !self.chart.pane_collapsed(instance.id) {
                    let height = self.chart.plot_height(Some(instance.id));
                    pane = pane.push(chart::view(
                        chart::Chart {
                            data: &self.chart,
                            cache,
                            palette: self.theme_file.palette,
                            indicator: Some(instance),
                            quote: &self.native_quote,
                            volume_unit: &self.native_volume_unit,
                            drawings: None,
                            profiles: None,
                            profile_range: None,
                            metrics: self.stress.as_ref(),
                        },
                        if self.chart.maximized() {
                            Length::Fill
                        } else {
                            Length::Fixed(height)
                        },
                        blocked,
                        None,
                        self.pointer_generation,
                    ));
                    if !self.chart.maximized() {
                        pane = pane.push(pane_resize::view(
                            Some(instance.id),
                            height,
                            self.theme_file.palette,
                            blocked,
                            self.pointer_generation,
                        ));
                    }
                }
                charts = charts.push(pane.spacing(2).height(if self.chart.maximized() {
                    Length::Fill
                } else {
                    Length::Shrink
                }));
            }
        }
        let charts: Element<'_, Message> = if has_lower {
            scrollable(charts.width(Length::Fill))
                .id(scrollable::Id::new("indicator-panes"))
                .height(Length::Fill)
                .into()
        } else {
            charts.width(Length::Fill).height(Length::Fill).into()
        };
        let market_control = tooltip(
            button(
                column![
                    text(&self.chart.market.symbol).size(18),
                    text(format!(
                        "{} · {} · {}",
                        self.chart.market.exchange,
                        if self.chart.market.product == Product::Spot {
                            "Spot"
                        } else {
                            "Perp"
                        },
                        self.native_quote
                    ))
                    .size(10)
                    .style(ui::muted_text),
                ]
                .spacing(2),
            )
            .style(ui::quiet)
            .padding([4, 6])
            .width(Length::Fill)
            .on_press_maybe((!blocked).then_some(Message::Action(Action::ManageMarket))),
            text("Choose native market · M").size(12),
            tooltip::Position::Bottom,
        );
        let interval_control: Element<'_, Message> = if !self.native_intervals().is_empty() {
            pick_list(
                self.native_intervals(),
                Some(self.chart.market.interval),
                Message::SelectInterval,
            )
            .text_size(12)
            .padding([6, 8])
            .width(70)
            .style(ui::pick)
            .into()
        } else {
            text(self.chart.market.interval.to_string()).size(12).into()
        };
        let header = row![
            market_control,
            interval_control,
            tooltip(
                button(text("Indicators").size(12))
                    .style(ui::quiet)
                    .padding([6, 8])
                    .on_press_maybe(
                        (!blocked).then_some(Message::Action(Action::ManageIndicators))
                    ),
                text("Indicators · I").size(12),
                tooltip::Position::Bottom
            ),
            tooltip(
                button(text("Draw").size(12))
                    .style(ui::quiet)
                    .padding([6, 8])
                    .on_press_maybe((!blocked).then_some(Message::DrawingMenu)),
                text("Drawing palette · Ctrl+Shift+D").size(12),
                tooltip::Position::Bottom
            ),
            tooltip(
                button(text("?").size(14))
                    .style(ui::quiet)
                    .padding([6, 8])
                    .on_press(Message::HelpOpen),
                text("Keyboard & mouse actions · F1").size(12),
                tooltip::Position::Bottom
            ),
        ]
        .spacing(5)
        .align_y(iced::Alignment::Center);
        let status_control = |label: String| {
            button(text(label).size(11))
                .style(ui::quiet)
                .padding([4, 5])
                .on_press(Message::StatusDetails)
        };
        let history_state = if self.history.error.is_some() {
            "error"
        } else if self.history.in_flight {
            "loading"
        } else if self.chart.initialized && self.chart.next_before.is_none() {
            "start reached"
        } else {
            "ready"
        };
        let mut status_row = row![
            status_control(format!(
                "Prices {}",
                if self.upstream_connected {
                    "live"
                } else {
                    "offline"
                }
            )),
            status_control(format!("History {history_state}")),
        ]
        .spacing(6)
        .align_y(iced::Alignment::Center);
        let cvd = self
            .chart
            .indicators
            .instances
            .iter()
            .filter(|item| matches!(item.spec, IndicatorSpec::Cvd { .. }))
            .count();
        let oi = self
            .chart
            .indicators
            .instances
            .iter()
            .filter(|item| item.spec == IndicatorSpec::OpenInterest)
            .count();
        if cvd != 0 {
            let problem = self.history.flow_error.is_some()
                || self.chart.indicators.instances.iter().any(|item| {
                    matches!(item.spec, IndicatorSpec::Cvd { .. })
                        && !item.source.is_chart()
                        && item.external.as_ref().is_none_or(|output| !output.complete)
                });
            status_row = status_row.push(status_control(format!(
                "CVD {cvd} · {}",
                if problem {
                    "needs attention"
                } else {
                    "see quality"
                }
            )));
        }
        if oi != 0 {
            let problem = self.open_interest_history.error.is_some()
                || self.chart.indicators.instances.iter().any(|item| {
                    item.spec == IndicatorSpec::OpenInterest
                        && !item.source.is_chart()
                        && item.external.as_ref().is_none_or(|output| !output.complete)
                });
            status_row = status_row.push(status_control(format!(
                "OI {oi} · {}",
                if problem {
                    "needs attention"
                } else {
                    "see freshness"
                }
            )));
        }
        let active_catalog_scope =
            self.selector.scope() == (self.chart.market.exchange, self.chart.market.product);
        if active_catalog_scope && (self.selector.loading || self.selector.catalog_error.is_some())
        {
            status_row = status_row.push(status_control(format!(
                "Catalog {}",
                if self.selector.loading {
                    "loading"
                } else {
                    "error"
                }
            )));
        }
        if self.theme_file.error.is_some() {
            status_row = status_row.push(status_control("Theme error".into()));
        }
        status_row = status_row.push(status_control("Details".into()));
        if !self.chart.viewport.live {
            status_row = status_row.push(tooltip(
                button(text("Go live").size(11))
                    .style(ui::selected)
                    .on_press_maybe((!blocked).then_some(Message::Action(Action::Live)))
                    .padding([4, 8]),
                text("Resume live following · L / End · keep manual value scales").size(12),
                tooltip::Position::Top,
            ));
        }
        if let Some(error) = &self.drawing_error {
            status_row = status_row.push(text(format!("Draw: {error}")).size(12));
        }
        if let Some(target) = self.chart.maximized_target() {
            status_row = status_row.push(
                button(text("Restore panes").size(11))
                    .style(ui::selected)
                    .on_press_maybe(
                        (!blocked).then_some(Message::Action(Action::TogglePaneMaximized(target))),
                    ),
            );
        }
        if self.workspace.error.is_some() || self.workspace.warning.is_some() {
            status_row = status_row.push(
                button(text("Workspace attention").size(11))
                    .style(ui::quiet)
                    .on_press_maybe((!blocked).then_some(Message::StatusDetails)),
            );
        }
        if let Some(notice) = &self.goto_notice {
            status_row = status_row.push(text(notice).size(11).style(ui::muted_text));
        }
        let mut content = column![header, ui::separator(), readout].spacing(8);
        if let Some(metrics) = &self.stress {
            content = content.push(ui::badge(metrics.label(), ui::Tone::Accent));
        }
        let content = content
            .push(charts)
            .push(ui::separator())
            .push(status_row.wrap());
        let base: Element<'_, Message> = container(content)
            .padding(10)
            .width(Length::Fill)
            .height(Length::Fill)
            .into();
        let mut layers = iced::widget::stack![key_help::modal_background(base, blocked)]
            .width(Length::Fill)
            .height(Length::Fill);
        let profile_navigation = |profiles: bool| {
            row![
                button("Indicators")
                    .style(if profiles { ui::quiet } else { ui::selected })
                    .on_press(Message::Action(Action::ManageIndicators)),
                button("Profiles")
                    .style(if profiles { ui::selected } else { ui::quiet })
                    .on_press(Message::Profile(profile_controls::Event::Open(None))),
            ]
            .spacing(6)
        };
        let dialog = if self.profile_controls.open {
            Some((
                column![
                    profile_navigation(true),
                    key_help::profile_shortcuts(
                        self.profile_controls
                            .view(&self.chart.profiles, &self.profiles.outputs)
                            .map(Message::Profile)
                    ),
                ]
                .spacing(8)
                .height(Length::Fill)
                .into(),
                Message::Profile(profile_controls::Event::Close),
            ))
        } else if self.manager.open {
            Some((
                column![profile_navigation(false), self.manager.view(&self.chart)]
                    .spacing(8)
                    .height(Length::Fill)
                    .into(),
                Message::EditorClose,
            ))
        } else if self.selector.open {
            Some((self.selector.view(), Message::EditorClose))
        } else if self.drawing_controls.is_open() {
            Some((
                self.drawing_controls.view(
                    &self.drawings,
                    &self.chart.profiles,
                    self.drawing_interaction.tool(),
                    self.drawing_interaction.magnet(),
                    self.chart.latest().is_some(),
                    self.theme_file.palette,
                    self.chart.price_scale,
                    &self.native_quote,
                    self.drawing_interaction.repeat(),
                ),
                Message::DrawingClose,
            ))
        } else {
            self.details
                .map(|details| (self.details_view(details), Message::DetailsClose))
        };
        if let Some((body, close)) = dialog {
            layers = layers.push(key_help::modal_background(
                dialog_layer(
                    body,
                    close,
                    if self.drawing_controls.is_open() && !self.drawing_controls.is_editing() {
                        (380.0 + self.drawings.objects().len().min(6) as f32 * 42.0).min(660.0)
                    } else if self.manager.open {
                        self.manager.dialog_height(&self.chart)
                    } else {
                        720.0
                    },
                ),
                self.help_open || self.commands.is_some(),
            ));
        }
        if self.goto_time.open {
            layers = layers.push(key_help::modal_background(
                overlay_panel(
                    self.goto_time.view(),
                    Message::GotoTime(goto_time::Event::Close),
                ),
                self.help_open || self.commands.is_some(),
            ));
        }
        if self.help_open {
            layers = layers.push(key_help::view(self.help_context(), self.help_selection));
        }
        if let Some(commands) = &self.commands {
            layers = layers.push(overlay_panel(
                commands.view(),
                Message::CommandPalette(command_palette::Event::Escape),
            ));
        }
        if self.workspace.controls.is_open() {
            layers = layers.push(overlay_panel(
                self.workspace.controls.view(
                    self.workspace.reset_pending,
                    self.workspace.error.as_deref(),
                ),
                Message::Workspace(workspace_controls::Event::Cancel),
            ));
        }
        pointer_input::view(layers.into())
    }
}

fn drawing_tool_name(tool: DrawingTool) -> &'static str {
    match tool {
        DrawingTool::Select => "Select",
        DrawingTool::Horizontal => "Horizontal",
        DrawingTool::Vertical => "Vertical",
        DrawingTool::Trend => "Trend",
        DrawingTool::Ray => "Ray",
        DrawingTool::Rectangle => "Zone",
        DrawingTool::Fibonacci => "Fibonacci",
        DrawingTool::Text => "Text",
        DrawingTool::Measure => "Measure",
        DrawingTool::AnchoredVwap => "Anchored VWAP",
    }
}

fn dialog_layer<'a>(
    body: Element<'a, Message>,
    close: Message,
    max_height: f32,
) -> Element<'a, Message> {
    let panel = container(body)
        .padding(20)
        .width(Length::Fill)
        .max_width(760)
        .height(Length::Fill)
        .max_height(max_height)
        .style(ui::panel);
    overlay_panel(panel.into(), close)
}

fn overlay_panel<'a, R: iced::advanced::Renderer + 'a>(
    body: Element<'a, Message, Theme, R>,
    close: Message,
) -> Element<'a, Message, Theme, R> {
    let backdrop = mouse_area(
        container(iced::widget::Space::new(Length::Fill, Length::Fill))
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_| container::Style {
                background: Some(iced::Color::from_rgba(0.0, 0.0, 0.0, 0.55).into()),
                ..Default::default()
            }),
    )
    .on_press(close)
    .on_move(|_| Message::ModalBackdrop);
    iced::widget::stack![
        backdrop,
        // Containers do not capture presses on padding, labels, or controls
        // that ignore a repeated click. Keep those presses off the backdrop.
        container(iced::widget::opaque(body))
            .padding(16)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(iced::Alignment::Center)
            .align_y(iced::Alignment::Center),
    ]
    .into()
}

struct FindFocused(Option<iced::advanced::widget::Id>);

impl iced::advanced::widget::Operation<Option<iced::advanced::widget::Id>> for FindFocused {
    fn focusable(
        &mut self,
        state: &mut dyn iced::advanced::widget::operation::focusable::Focusable,
        id: Option<&iced::advanced::widget::Id>,
    ) {
        if state.is_focused() {
            self.0 = id.cloned();
        }
    }

    fn container(
        &mut self,
        _id: Option<&iced::advanced::widget::Id>,
        _bounds: iced::Rectangle,
        children: &mut dyn FnMut(
            &mut dyn iced::advanced::widget::Operation<Option<iced::advanced::widget::Id>>,
        ),
    ) {
        children(self);
    }

    fn finish(
        &self,
    ) -> iced::advanced::widget::operation::Outcome<Option<iced::advanced::widget::Id>> {
        iced::advanced::widget::operation::Outcome::Some(self.0.clone())
    }
}

fn zoom(time: f64, price: f64) -> Action {
    Action::Zoom {
        time,
        price,
        x: 0.5,
        y: 0.5,
    }
}

fn key_action(key: keyboard::Key<&str>, modifiers: keyboard::Modifiers) -> Option<Action> {
    use keyboard::{Key, key::Named};
    if modifiers.control() && modifiers.alt() {
        return match key {
            Key::Named(Named::ArrowUp) => Some(Action::AdjustPaneHeight {
                id: None,
                delta: 32.0,
            }),
            Key::Named(Named::ArrowDown) => Some(Action::AdjustPaneHeight {
                id: None,
                delta: -32.0,
            }),
            Key::Character("0") => Some(Action::ResetPaneHeight(None)),
            _ => None,
        };
    }
    let pan = if modifiers.shift() { 0.5 } else { 0.12 };
    match key {
        Key::Named(Named::PageUp | Named::PageDown) => Some(Action::ScrollPanes {
            amount: if matches!(key, Key::Named(Named::PageUp)) {
                -280.0
            } else {
                280.0
            },
            legends: modifiers.shift(),
        }),
        Key::Named(Named::ArrowLeft) => Some(Action::Pan {
            time: -pan,
            price: 0.0,
        }),
        Key::Named(Named::ArrowRight) => Some(Action::Pan {
            time: pan,
            price: 0.0,
        }),
        Key::Named(Named::ArrowUp) if modifiers.control() => Some(zoom(1.0, 0.8)),
        Key::Named(Named::ArrowDown) if modifiers.control() => Some(zoom(1.0, 1.25)),
        Key::Named(Named::ArrowUp) => Some(Action::Pan {
            time: 0.0,
            price: pan,
        }),
        Key::Named(Named::ArrowDown) => Some(Action::Pan {
            time: 0.0,
            price: -pan,
        }),
        Key::Named(Named::Home) => Some(Action::Reset),
        Key::Named(Named::End) => Some(Action::Live),
        Key::Named(Named::Escape) => Some(Action::Inspect(None)),
        Key::Character("+" | "=") => Some(if modifiers.shift() {
            zoom(1.0, 0.8)
        } else {
            zoom(0.8, 1.0)
        }),
        Key::Character("-" | "_") => Some(if modifiers.shift() {
            zoom(1.0, 1.25)
        } else {
            zoom(1.25, 1.0)
        }),
        Key::Character("[" | ",") => Some(Action::InspectStep(-1)),
        Key::Character("]" | ".") => Some(Action::InspectStep(1)),
        Key::Character(value) => match value {
            "r" | "R" => Some(Action::Reset),
            "f" | "F" => Some(Action::AutoFit),
            "l" | "L" => Some(Action::Live),
            "b" | "B" => Some(Action::Older),
            "y" | "Y" => Some(Action::Retry),
            "i" | "I" => Some(Action::ManageIndicators),
            "m" | "M" => Some(Action::ManageMarket),
            _ => None,
        },
        _ => None,
    }
}

fn main() -> iced::Result {
    iced::application(Desktop::title, Desktop::update, Desktop::view)
        .theme(Desktop::theme)
        .settings(iced::Settings {
            default_text_size: iced::Pixels(14.0),
            antialiasing: true,
            ..iced::Settings::default()
        })
        .subscription(Desktop::subscription)
        .window_size((1200.0, 760.0))
        .exit_on_close_request(false)
        .run_with(Desktop::launch)
}

#[cfg(test)]
mod tests {
    fn flow_support() -> mexc_model::FlowSupport {
        mexc_model::FlowSupport {
            history: mexc_model::FlowAvailability::Unavailable,
            live: mexc_model::FlowAvailability::Unavailable,
            description: "test fixture".into(),
        }
    }
    use super::*;
    use iced::widget::column;

    fn modal_event(
        modal: &mut Element<'_, Message, Theme, ()>,
        tree: &mut iced::advanced::widget::Tree,
        node: &iced::advanced::layout::Node,
        event: iced::mouse::Event,
        position: iced::Point,
        messages: &mut Vec<Message>,
    ) -> iced::event::Status {
        modal.as_widget_mut().on_event(
            tree,
            iced::Event::Mouse(event),
            iced::advanced::Layout::new(node),
            iced::mouse::Cursor::Available(position),
            &(),
            &mut iced::advanced::clipboard::Null,
            &mut iced::advanced::Shell::new(messages),
            &iced::Rectangle::with_size(iced::Size::new(800.0, 600.0)),
        )
    }

    #[test]
    fn modal_blank_content_and_reselected_controls_do_not_dismiss_but_outside_does() {
        use iced::mouse::{Button, Event};
        let body = container(column![
            container(iced::widget::radio("Chart", true, Some(true), |_| {
                Message::Manager(indicator_manager::Event::SourceMode(
                    mexc_indicators::SourceMode::Chart,
                ))
            }))
            .height(32),
            button("Apply")
                .on_press(Message::Manager(indicator_manager::Event::Submit))
                .width(Length::Fill)
                .height(32),
            iced::widget::Space::new(Length::Fill, Length::Fill),
        ])
        .padding(10)
        .width(240)
        .height(160);
        let mut modal: Element<'_, Message, Theme, ()> =
            overlay_panel(body.into(), Message::EditorClose);
        let mut tree = iced::advanced::widget::Tree::new(modal.as_widget());
        let node = modal.as_widget().layout(
            &mut tree,
            &(),
            &iced::advanced::layout::Limits::new(iced::Size::ZERO, iced::Size::new(800.0, 600.0)),
        );
        let mut messages = Vec::new();
        // The fixed 240x160 body is centered at (280, 220). Both blank
        // content and an already-selected radio normally ignore the press.
        for position in [
            iced::Point::new(300.0, 242.0),
            iced::Point::new(300.0, 340.0),
            iced::Point::new(282.0, 222.0),
        ] {
            assert_eq!(
                modal_event(
                    &mut modal,
                    &mut tree,
                    &node,
                    Event::ButtonPressed(Button::Left),
                    position,
                    &mut messages,
                ),
                iced::event::Status::Captured,
            );
            modal_event(
                &mut modal,
                &mut tree,
                &node,
                Event::CursorMoved {
                    position: iced::Point::new(10.0, 10.0),
                },
                iced::Point::new(10.0, 10.0),
                &mut messages,
            );
            modal_event(
                &mut modal,
                &mut tree,
                &node,
                Event::ButtonReleased(Button::Left),
                iced::Point::new(10.0, 10.0),
                &mut messages,
            );
        }
        assert!(
            !messages
                .iter()
                .any(|message| matches!(message, Message::EditorClose))
        );
        let apply = iced::Point::new(300.0, 278.0);
        for event in [
            Event::ButtonPressed(Button::Left),
            Event::ButtonReleased(Button::Left),
        ] {
            modal_event(&mut modal, &mut tree, &node, event, apply, &mut messages);
        }
        assert!(
            messages.iter().any(|message| matches!(
                message,
                Message::Manager(indicator_manager::Event::Submit)
            ))
        );
        modal_event(
            &mut modal,
            &mut tree,
            &node,
            Event::ButtonPressed(Button::Left),
            iced::Point::new(10.0, 10.0),
            &mut messages,
        );
        assert_eq!(
            messages
                .iter()
                .filter(|message| matches!(message, Message::EditorClose))
                .count(),
            1,
        );
    }

    #[test]
    fn batched_pointer_events_use_the_press_position_not_the_end_of_frame_cursor() {
        use iced::mouse::{Button, Event};
        let body = container(iced::widget::Space::new(Length::Fill, Length::Fill))
            .width(240)
            .height(160);
        let mut modal: Element<'_, Message, Theme, ()> =
            pointer_input::view(overlay_panel(body.into(), Message::EditorClose));
        let mut tree = iced::advanced::widget::Tree::new(modal.as_widget());
        let node = modal.as_widget().layout(
            &mut tree,
            &(),
            &iced::advanced::layout::Limits::new(iced::Size::ZERO, iced::Size::new(800.0, 600.0)),
        );
        let inside = iced::Point::new(300.0, 340.0);
        let outside = iced::Point::new(10.0, 10.0);
        let mut messages = Vec::new();
        // iced 0.13 supplies the final cursor snapshot to every event in a batch.
        for event in [
            Event::CursorMoved { position: inside },
            Event::ButtonPressed(Button::Left),
            Event::CursorMoved { position: outside },
            Event::ButtonReleased(Button::Left),
        ] {
            modal_event(&mut modal, &mut tree, &node, event, outside, &mut messages);
        }
        assert!(
            !messages
                .iter()
                .any(|message| matches!(message, Message::EditorClose)),
            "an inside press must not become an outside press when the cursor moves later",
        );
        for event in [
            Event::CursorMoved { position: outside },
            Event::ButtonPressed(Button::Left),
            Event::CursorMoved { position: inside },
            Event::ButtonReleased(Button::Left),
        ] {
            modal_event(&mut modal, &mut tree, &node, event, inside, &mut messages);
        }
        assert_eq!(
            messages
                .iter()
                .filter(|message| matches!(message, Message::EditorClose))
                .count(),
            1,
            "an outside press must still dismiss when the cursor moves inside later",
        );
    }

    #[test]
    fn modal_dropdown_overlay_selects_beyond_panel_without_backdrop_dismissal() {
        use iced::mouse::{Button, Event};
        let body = container(column![
            pick_list(
                [
                    mexc_indicators::CvdReset::Continuous,
                    mexc_indicators::CvdReset::UtcDay
                ],
                Some(mexc_indicators::CvdReset::Continuous),
                |reset| Message::Manager(indicator_manager::Event::Reset(reset)),
            )
            .width(Length::Fill)
            .padding(6),
            iced::widget::Space::new(Length::Fill, 32),
        ])
        .width(240)
        .height(64);
        let mut modal: Element<'_, Message, Theme, ()> =
            pointer_input::view(overlay_panel(body.into(), Message::EditorClose));
        let mut tree = iced::advanced::widget::Tree::new(modal.as_widget());
        let size = iced::Size::new(800.0, 600.0);
        let node = modal.as_widget().layout(
            &mut tree,
            &(),
            &iced::advanced::layout::Limits::new(iced::Size::ZERO, size),
        );
        let mut messages = Vec::new();
        let picker = iced::Point::new(300.0, 284.0);
        for event in [
            Event::CursorMoved { position: picker },
            Event::ButtonPressed(Button::Left),
        ] {
            modal_event(&mut modal, &mut tree, &node, event, picker, &mut messages);
        }
        let mut overlay = modal
            .as_widget_mut()
            .overlay(
                &mut tree,
                iced::advanced::Layout::new(&node),
                &(),
                iced::Vector::ZERO,
            )
            .expect("the modal must expose its child's open dropdown");
        let menu_node = overlay.layout(&(), size);
        // The menu opens below the picker, past the centered panel's bottom.
        let selection = iced::Point::new(300.0, 350.0);
        assert!(overlay.is_over(iced::advanced::Layout::new(&menu_node), &(), selection));
        for event in [
            Event::CursorMoved {
                position: selection,
            },
            Event::ButtonPressed(Button::Left),
        ] {
            let status = overlay.on_event(
                iced::Event::Mouse(event),
                iced::advanced::Layout::new(&menu_node),
                iced::mouse::Cursor::Available(selection),
                &(),
                &mut iced::advanced::clipboard::Null,
                &mut iced::advanced::Shell::new(&mut messages),
            );
            if matches!(event, Event::ButtonPressed(_)) {
                assert_eq!(status, iced::event::Status::Captured);
            }
        }
        assert!(messages.iter().any(|message| matches!(
            message,
            Message::Manager(indicator_manager::Event::Reset(
                mexc_indicators::CvdReset::UtcDay
            ))
        )));
        assert!(
            !messages
                .iter()
                .any(|message| matches!(message, Message::EditorClose))
        );
        drop(overlay);
        // The overlay consumed the motion to this point. The next base click
        // must not reuse the earlier picker position and accidentally reopen it.
        for event in [
            Event::ButtonReleased(Button::Left),
            Event::ButtonPressed(Button::Left),
        ] {
            modal_event(
                &mut modal,
                &mut tree,
                &node,
                event,
                selection,
                &mut messages,
            );
        }
        assert_eq!(
            messages
                .iter()
                .filter(|message| matches!(message, Message::EditorClose))
                .count(),
            1,
        );
    }

    #[test]
    fn escape_cancels_indicator_editor_even_when_a_control_captured_the_key() {
        let (mut desktop, _) = Desktop::new();
        let _ = desktop.update(Message::Action(Action::ManageIndicators));
        assert!(desktop.manager.open);
        let before = desktop.chart.indicators.instances.len();
        let _ = desktop.update(Message::Key {
            key: keyboard::Key::Named(keyboard::key::Named::Escape),
            modifiers: keyboard::Modifiers::default(),
            captured: true,
        });
        assert!(!desktop.manager.open);
        assert_eq!(desktop.chart.indicators.instances.len(), before);
    }

    fn page(oldest: i64, next_before: Option<i64>) -> HistoryPage {
        HistoryPage {
            market: Market::default(),
            candles: vec![mexc_model::Candle {
                time: oldest,
                open: 1.0,
                high: 1.0,
                low: 1.0,
                close: 1.0,
                volume: 1.0,
                flow: None,
            }],
            next_before,
        }
    }

    #[test]
    fn reconnect_repairs_multiple_pages_until_prior_watermark() {
        let first =
            repair_request(Request::Recent, &page(900_000, Some(900_000)), Some(300)).unwrap();
        let second = repair_request(first, &page(600_000, Some(600_000)), Some(300)).unwrap();
        let third = repair_request(second, &page(300_000, Some(300_000)), Some(300)).unwrap();
        assert!(repair_request(third, &page(0, Some(0)), Some(300)).is_none());
    }

    #[test]
    fn exhausted_and_nonprogressing_repair_cursors_stop() {
        assert!(repair_request(Request::Recent, &page(900, None), Some(300)).is_none());
        assert!(
            repair_request(
                Request::Repair {
                    before: 900,
                    target: 300
                },
                &page(900, Some(900)),
                Some(300)
            )
            .is_none()
        );
    }

    #[test]
    fn vwap_origin_history_pages_without_moving_the_chart_or_restarting_at_loaded_history() {
        let (mut desktop, _) = Desktop::new();
        let origin = 2 * 86_400;
        let first = origin + 12 * 3600;
        let mut initial = page(first, Some(first));
        initial.candles.push(mexc_model::Candle {
            time: first + 3600,
            ..initial.candles[0]
        });
        desktop.apply_history_page(Arc::new(initial), Request::Recent, 0);
        desktop.history.in_flight = false;
        desktop.chart.viewport.left = (first + 1800) as f64;
        desktop.chart.viewport.span = 1800.0;
        desktop.chart.viewport.live = false;
        let view = desktop.chart.viewport;
        let _ = desktop.update(Message::Action(Action::AddIndicator(IndicatorSpec::Vwap {
            reset: mexc_indicators::profiles::Period::Day,
        })));
        assert_eq!(desktop.vwap_history_origin(), Some(origin));
        assert!(desktop.history.in_flight);
        assert_eq!(
            desktop.chart.indicators.instances[0]
                .vwap_context(1)
                .unwrap()
                .origin_ms,
            origin * 1000
        );
        assert!(
            !desktop.chart.indicators.instances[0]
                .vwap_context(1)
                .unwrap()
                .covered
        );
        for (before, oldest) in [(first, origin + 6 * 3600), (origin + 6 * 3600, origin)] {
            let _ = desktop.update(Message::History {
                request: Request::Older { before },
                market: desktop.chart.market.clone(),
                generation: desktop.market_generation,
                history_id: desktop.history.generation,
                epoch: 0,
                result: Ok(page(oldest, Some(oldest))),
            });
            assert_eq!(desktop.chart.oldest(), Some(oldest));
            assert_eq!(
                (
                    desktop.chart.viewport.left,
                    desktop.chart.viewport.span,
                    desktop.chart.viewport.live
                ),
                (view.left, view.span, view.live)
            );
        }
        assert!(!desktop.history.in_flight);
        assert_eq!(
            desktop.chart.indicators.instances[0]
                .vwap_context(0)
                .unwrap()
                .origin_ms,
            origin * 1000
        );
        assert!(desktop.backfill_vwap_origin().is_none());
    }

    #[test]
    fn flow_backfill_keeps_using_the_older_page_cursor_until_loaded_history_is_covered() {
        let first = Request::FlowBackfill {
            before: 1_200,
            target: 300,
        };
        let next = repair_request(first, &page(900, Some(800)), Some(300)).unwrap();
        assert_eq!(
            next,
            Request::FlowBackfill {
                before: 800,
                target: 300
            }
        );
        let last = repair_request(next, &page(300, Some(200)), Some(300));
        assert!(last.is_none());
    }

    #[derive(Debug, PartialEq)]
    struct PipelineSnapshot {
        market: Market,
        generation: u64,
        latest: Option<mexc_model::Candle>,
        candles: usize,
        epoch: u64,
        next_before: Option<i64>,
        inspection: Option<(i64, f64)>,
        in_flight: bool,
        pending_sync: bool,
        sync_anchor: Option<i64>,
        failed: Option<Request>,
        retry_ticks: u8,
        upstream_connected: bool,
        status: String,
        history_error: Option<String>,
        catalog_loading: bool,
        catalog_error: Option<String>,
    }

    fn pipeline_snapshot(desktop: &Desktop) -> PipelineSnapshot {
        PipelineSnapshot {
            market: desktop.chart.market.clone(),
            generation: desktop.market_generation,
            latest: desktop.chart.latest().copied(),
            candles: desktop.chart.len(),
            epoch: desktop.chart.epoch,
            next_before: desktop.chart.next_before,
            inspection: desktop.chart.inspection,
            in_flight: desktop.history.in_flight,
            pending_sync: desktop.history.pending_sync,
            sync_anchor: desktop.history.anchor,
            failed: desktop.history.failed,
            retry_ticks: desktop.history.retry_ticks,
            upstream_connected: desktop.upstream_connected,
            status: desktop.status.clone(),
            history_error: desktop.history.error.clone(),
            catalog_loading: desktop.selector.loading,
            catalog_error: desktop.selector.catalog_error.clone(),
        }
    }

    fn data_message(market: &Market, generation: u64, event: MarketEvent) -> Message {
        Message::MarketData(feeds::Event {
            scope: Arc::new(feeds::Scope {
                market: market.clone(),
                generation,
                flow_generation: 0,
                cvd: false,
            }),
            events: vec![event].into_boxed_slice(),
        })
    }

    fn assert_stale_callbacks_ignored(desktop: &mut Desktop, market: &Market, generation: u64) {
        let mut stale_page = page(30_000, Some(0));
        stale_page.market = market.clone();
        let snapshot = pipeline_snapshot(desktop);
        let events = [
            Message::History {
                market: market.clone(),
                generation,
                history_id: desktop.history.generation.wrapping_sub(1),
                request: Request::Recent,
                epoch: 0,
                result: Err("old native request failure".into()),
            },
            Message::History {
                market: market.clone(),
                generation,
                history_id: desktop.history.generation.wrapping_sub(1),
                request: Request::Older { before: 30_000 },
                epoch: 0,
                result: Ok(stale_page),
            },
            Message::Catalog {
                exchange: market.exchange,
                product: market.product,
                generation: desktop.catalog_generation.wrapping_sub(1),
                result: Err("old catalog failure".into()),
            },
            Message::Catalog {
                exchange: market.exchange,
                product: market.product,
                generation: desktop.catalog_generation.wrapping_sub(1),
                result: Ok(Arc::new(Catalog {
                    exchange: market.exchange,
                    product: market.product,
                    intervals: vec![mexc_model::Timeframe::Min5],
                    flow_support: flow_support(),
                    instruments: Vec::new(),
                })),
            },
            data_message(
                market,
                generation,
                MarketEvent::Status {
                    connected: false,
                    message: "old upstream status".into(),
                },
            ),
            data_message(market, generation, MarketEvent::Resync),
            data_message(
                market,
                generation,
                MarketEvent::Candle {
                    candle: page(60_000, None).candles[0],
                },
            ),
        ];
        for event in events {
            let _ = desktop.update(event);
            assert_eq!(pipeline_snapshot(desktop), snapshot);
            assert!(desktop.selector.active_instrument(&market.symbol).is_some());
        }
    }

    #[test]
    fn rapid_a_b_a_and_retry_discard_old_callbacks_before_pipeline_side_effects() {
        let (mut desktop, _) = Desktop::new();
        let a = Market::default();
        let b = Market {
            exchange: a.exchange,
            product: a.product,
            symbol: "ETH_USDC".into(),
            interval: mexc_model::Timeframe::Min5,
        };
        desktop.selector.set_catalog(
            a.exchange,
            a.product,
            Ok(Arc::new(Catalog {
                exchange: a.exchange,
                product: a.product,
                flow_support: flow_support(),
                intervals: vec![mexc_model::Timeframe::Min5],
                instruments: [&a, &b]
                    .into_iter()
                    .map(|market| mexc_model::Instrument {
                        exchange: market.exchange,
                        product: market.product,
                        symbol: market.symbol.clone(),
                        base_coin: market.symbol.split('_').next().unwrap().into(),
                        quote_coin: mexc_model::Quote::USDC,
                        settle_coin: "USDC".into(),
                        price_scale: 2,
                        tick_size: None,
                        contract_size: 1.0,
                        inverse: false,
                        volume_unit: "contracts".into(),
                    })
                    .collect(),
            })),
        );
        let old_a_generation = desktop.market_generation;
        let _ = desktop.update(Message::Action(Action::SelectMarket(b)));
        let _ = desktop.update(Message::Action(Action::SelectMarket(a.clone())));
        assert!(desktop.market_generation > old_a_generation);
        assert_eq!(desktop.chart.market, a);
        assert_eq!(desktop.chart.len(), 0);
        assert!(desktop.history.in_flight);
        let current_generation = desktop.market_generation;
        let _ = desktop.update(Message::History {
            market: a.clone(),
            generation: current_generation,
            history_id: desktop.history.generation,
            request: Request::Recent,
            epoch: 0,
            result: Ok(page(300, None)),
        });
        assert_eq!(desktop.chart.latest().unwrap().time, 300);
        let _ = desktop.update(data_message(
            &a,
            current_generation,
            MarketEvent::Status {
                connected: true,
                message: "current upstream".into(),
            },
        ));
        let _ = desktop.update(data_message(&a, current_generation, MarketEvent::Resync));
        let _ = desktop.load_catalog(a.exchange, a.product);
        assert_eq!(desktop.history.anchor, Some(300));
        assert!(desktop.history.in_flight);
        assert_stale_callbacks_ignored(&mut desktop, &a, old_a_generation);

        let _ = desktop.update(Message::Action(Action::Retry));
        assert!(desktop.market_generation > current_generation);
        assert!(desktop.history.in_flight);
        assert_stale_callbacks_ignored(&mut desktop, &a, current_generation);
    }

    #[test]
    fn maximizing_another_pane_moves_keyboard_target_to_the_visible_plot() {
        let (mut desktop, _) = Desktop::new();
        desktop
            .chart
            .dispatch(Action::AddIndicator(IndicatorSpec::Rsi { period: 14 }));
        let id = desktop.chart.indicators.instances[0].id;
        assert_eq!(desktop.chart.active_pane, None);
        let _ = desktop.update(Message::Action(Action::TogglePaneMaximized(Some(id))));
        assert_eq!(desktop.chart.maximized_target(), Some(Some(id)));
        assert_eq!(desktop.chart.active_pane, Some(id));
        assert!(!desktop.chart.pane_should_render(None));
    }

    #[test]
    fn repeated_tool_shortcut_is_scoped_to_drawings_and_escape_disarms_it() {
        let (mut desktop, _) = Desktop::new();
        desktop.drawing_controls.open_menu(&desktop.drawings);
        let repeat_key = || Message::Key {
            key: keyboard::Key::Character("R".into()),
            modifiers: keyboard::Modifiers::CTRL | keyboard::Modifiers::SHIFT,
            captured: true,
        };
        let _ = desktop.update(repeat_key());
        assert!(desktop.drawing_interaction.repeat());
        let _ = desktop.update(Message::Key {
            key: keyboard::Key::Named(keyboard::key::Named::Escape),
            modifiers: keyboard::Modifiers::default(),
            captured: true,
        });
        assert!(!desktop.drawing_controls.is_open());
        assert!(!desktop.drawing_interaction.repeat());
        desktop.manager.open = true;
        let _ = desktop.update(repeat_key());
        assert!(!desktop.drawing_interaction.repeat());
    }

    #[test]
    fn utc_jump_pages_chronologically_until_the_requested_native_range_is_loaded() {
        let (mut desktop, _) = Desktop::new();
        desktop.apply_history_page(Arc::new(page(900, Some(900))), Request::Recent, 0);
        desktop.history.in_flight = false;
        let _ = desktop.update(Message::GotoTimeOpen);
        let _ = desktop.update(Message::GotoTime(goto_time::Event::Changed(
            "1970-01-01 00:05:00".into(),
        )));
        let _ = desktop.update(Message::GotoTime(goto_time::Event::Submit));
        assert_eq!(desktop.goto_target, Some(300));
        assert_eq!(desktop.goto_page_oldest, Some(900));
        assert!(desktop.history.in_flight);
        let center = desktop.chart.viewport.left + desktop.chart.viewport.span * 0.5;
        for (before, oldest, cursor) in [(900, 600, Some(600)), (600, 300, None)] {
            let _ = desktop.update(Message::History {
                request: Request::Older { before },
                market: desktop.chart.market.clone(),
                generation: desktop.market_generation,
                history_id: desktop.history.generation,
                epoch: 0,
                result: Ok(page(oldest, cursor)),
            });
            assert_eq!(desktop.chart.oldest(), Some(oldest));
            assert_eq!(
                desktop.chart.viewport.left + desktop.chart.viewport.span * 0.5,
                center
            );
            if cursor.is_some() {
                assert_eq!(desktop.goto_target, Some(300));
                assert_eq!(desktop.goto_page_oldest, Some(600));
                assert!(desktop.history.in_flight);
            }
        }
        assert_eq!(center, 300.0);
        assert_eq!(desktop.goto_target, None);
        assert!(!desktop.history.in_flight);
        assert!(!desktop.chart.viewport.live);
        assert_eq!(desktop.chart.inspection, None);
        assert_eq!(
            desktop
                .chart
                .candles()
                .iter()
                .map(|candle| candle.time)
                .collect::<Vec<_>>(),
            [300, 600, 900]
        );
    }

    #[test]
    fn utc_jump_rejects_a_nonadvancing_native_page_without_looping() {
        let (mut desktop, _) = Desktop::new();
        desktop.apply_history_page(Arc::new(page(900, Some(900))), Request::Recent, 0);
        desktop.history.in_flight = false;
        let _ = desktop.update(Message::GotoTimeOpen);
        let _ = desktop.update(Message::GotoTime(goto_time::Event::Changed(
            "1970-01-01 00:05:00".into(),
        )));
        let _ = desktop.update(Message::GotoTime(goto_time::Event::Submit));
        let _ = desktop.update(Message::History {
            request: Request::Older { before: 900 },
            market: desktop.chart.market.clone(),
            generation: desktop.market_generation,
            history_id: desktop.history.generation,
            epoch: 0,
            result: Ok(page(900, Some(900))),
        });
        assert_eq!(desktop.goto_target, None);
        assert!(!desktop.history.in_flight);
        assert!(desktop.goto_time.open);
        assert_eq!(desktop.chart.candles()[0].time, 900);
        assert_eq!(desktop.chart.inspection, None);
    }

    #[test]
    fn modal_help_preserves_chart_state_from_keyboard_and_canvas_input() {
        use keyboard::key::Named;
        let (mut desktop, _) = Desktop::new();
        let press = |key, modifiers| Message::Key {
            key: keyboard::Key::Named(key),
            modifiers,
            captured: true,
        };
        let plain = keyboard::Modifiers::default();
        let _ = desktop.update(Message::History {
            request: Request::Recent,
            market: desktop.chart.market.clone(),
            generation: desktop.market_generation,
            history_id: desktop.history.generation,
            epoch: 0,
            result: Ok(page(300, None)),
        });
        let _ = desktop.update(Message::Action(Action::Inspect(Some((300.0, 1.0)))));
        assert_eq!(desktop.chart.inspection, Some((300, 1.0)));
        let original = desktop.chart.viewport;

        let _ = desktop.update(press(Named::F1, plain));
        let _ = desktop.update(Message::Action(Action::Inspect(Some((300.0, 1.05)))));
        let _ = desktop.update(Message::Action(Action::Pan {
            time: -0.12,
            price: 0.12,
        }));
        let _ = desktop.update(Message::Action(zoom(1.2, 1.2)));
        let _ = desktop.update(press(Named::ArrowLeft, plain));
        let _ = desktop.update(press(Named::ArrowUp, plain));
        let _ = desktop.update(press(Named::PageDown, plain));
        let _ = desktop.update(press(Named::Tab, plain));
        let _ = desktop.update(press(Named::Tab, keyboard::Modifiers::SHIFT));
        assert!(desktop.help_open);
        let _ = desktop.update(press(Named::Enter, plain));
        assert!(!desktop.help_open);

        let _ = desktop.update(press(Named::F1, plain));
        let _ = desktop.update(press(Named::Tab, keyboard::Modifiers::SHIFT));
        let _ = desktop.update(press(Named::Tab, plain));
        let _ = desktop.update(press(Named::Space, plain));
        assert!(!desktop.help_open);
        assert!(!desktop.selector.open);
        assert!(!desktop.manager.open);
        assert_eq!(desktop.chart.viewport.left, original.left);
        assert_eq!(desktop.chart.viewport.span, original.span);
        assert_eq!(desktop.chart.viewport.low, original.low);
        assert_eq!(desktop.chart.viewport.high, original.high);
        assert_eq!(desktop.chart.viewport.live, original.live);
        assert_eq!(desktop.chart.viewport.auto_fit, original.auto_fit);
        assert_eq!(desktop.chart.inspection, Some((300, 1.0)));
    }

    #[test]
    fn stale_cvd_history_completion_cannot_finish_a_newer_request() {
        let (mut desktop, _) = Desktop::new();
        let old_history_id = desktop.history.generation;
        let old_market_generation = desktop.market_generation;
        let old_flow_generation = desktop.flow_generation;
        let market = desktop.chart.market.clone();
        let _ = desktop.update(Message::Action(Action::AddIndicator(IndicatorSpec::Cvd {
            reset: mexc_indicators::CvdReset::Continuous,
        })));
        let _ = desktop.fetch(Request::Recent);
        let active_history_id = desktop.history.generation;
        assert!(desktop.history.in_flight);

        let _ = desktop.update(Message::HistoryFlow {
            request: Request::Recent,
            market,
            generation: old_market_generation,
            history_id: old_history_id,
            epoch: 0,
            flow_generation: old_flow_generation,
            events: Ok(vec![HistoryEvent::Finished].into_boxed_slice()),
        });
        assert!(desktop.history.in_flight);
        assert_eq!(desktop.history.generation, active_history_id);
    }

    #[test]
    fn retry_resumes_native_backfill_and_rebases_previously_missing_cvd_prefix() {
        let (mut desktop, _) = Desktop::new();
        let mut loaded = page(300, Some(300));
        loaded.candles.push(page(600, None).candles[0]);
        loaded.candles.push(page(900, None).candles[0]);
        desktop.chart.merge_page(&loaded, 0);
        desktop
            .chart
            .dispatch(Action::AddIndicator(IndicatorSpec::Cvd {
                reset: mexc_indicators::CvdReset::Continuous,
            }));
        desktop.chart.viewport.live = false;
        desktop.chart.viewport.left = 900.0;
        desktop.chart.viewport.span = 300.0;
        desktop.restart_connection();
        let _ = desktop.fetch(Request::Recent);
        let recent_id = desktop.history.generation;
        let deliver = |desktop: &mut Desktop, request, history_id, event| {
            let _ = desktop.update(Message::HistoryFlow {
                request,
                market: desktop.chart.market.clone(),
                generation: desktop.market_generation,
                history_id,
                epoch: 0,
                flow_generation: desktop.flow_generation,
                events: Ok(vec![event].into_boxed_slice()),
            });
        };
        let flow = |time, delta: f64| HistoryEvent::Flow {
            bar: mexc_model::FlowBar {
                time,
                flow: mexc_model::VolumeFlow {
                    delta: Some(delta),
                    volume: Some(delta.abs()),
                    quality: mexc_model::FlowQuality::Exact,
                    covered_from: time,
                    through_ms: (time + 300) * 1000,
                },
            },
        };
        let mut recent = page(600, Some(600));
        recent.candles.push(page(900, None).candles[0]);
        deliver(
            &mut desktop,
            Request::Recent,
            recent_id,
            HistoryEvent::History {
                page: Arc::new(recent),
            },
        );
        deliver(&mut desktop, Request::Recent, recent_id, flow(600, 2.0));
        deliver(&mut desktop, Request::Recent, recent_id, flow(900, 1.0));
        assert_eq!(
            desktop.chart.indicators.instances[0].points[0].values[0],
            None
        );
        deliver(
            &mut desktop,
            Request::Recent,
            recent_id,
            HistoryEvent::Finished,
        );

        let older = Request::FlowBackfill {
            before: 600,
            target: 300,
        };
        deliver(
            &mut desktop,
            older,
            recent_id + 1,
            HistoryEvent::History {
                page: Arc::new(page(300, Some(300))),
            },
        );
        deliver(&mut desktop, older, recent_id + 1, flow(300, 1.0));
        assert_eq!(
            desktop.chart.indicators.instances[0].points[0].values[0],
            Some(1.0)
        );
        assert_eq!(
            desktop.chart.indicators.instances[0].points[2].values[0],
            Some(4.0)
        );
        assert_eq!(
            desktop.chart.indicators.instances[0]
                .cvd_context(2)
                .unwrap()
                .origin,
            300
        );
    }
    #[test]
    fn control_arrows_zoom_the_target_pane_without_moving_other_scales() {
        let (mut desktop, _) = Desktop::new();
        desktop
            .chart
            .dispatch(Action::AddIndicator(IndicatorSpec::Rsi { period: 2 }));
        let id = desktop.chart.indicators.instances.last().unwrap().id;
        let _ = desktop.update(Message::Action(Action::ActivatePane(Some(id))));
        let before = desktop.chart.pane_scale(id);
        let price = desktop.chart.viewport;
        let key = |key| Message::Key {
            key: keyboard::Key::Named(key),
            modifiers: keyboard::Modifiers::CTRL,
            captured: false,
        };
        let _ = desktop.update(key(keyboard::key::Named::ArrowUp));
        let after = desktop.chart.pane_scale(id);
        assert!(!after.auto_fit);
        assert!(((after.high - after.low) / (before.high - before.low) - 0.8).abs() < 1e-12);
        assert!((after.low + after.high - before.low - before.high).abs() < 1e-12);
        assert_eq!(desktop.chart.viewport.left, price.left);
        assert_eq!(desktop.chart.viewport.span, price.span);
        assert_eq!(desktop.chart.viewport.low, price.low);
        assert_eq!(desktop.chart.viewport.high, price.high);
        let _ = desktop.update(key(keyboard::key::Named::ArrowDown));
        let restored = desktop.chart.pane_scale(id);
        assert!((restored.low - before.low).abs() < 1e-12);
        assert!((restored.high - before.high).abs() < 1e-12);
    }

    fn open_interest_message(desktop: &Desktop, events: Vec<OpenInterestEvent>) -> Message {
        Message::OpenInterest(feeds::OpenInterestBatch {
            scope: Arc::new(feeds::OpenInterestScope {
                market: desktop.chart.market.clone(),
                generation: desktop.market_generation,
                open_interest_generation: desktop.open_interest_history.generation,
            }),
            events: events.into_boxed_slice(),
        })
    }

    fn open_interest_page(desktop: &Desktop, cursor: Option<i64>) -> OpenInterestPage {
        OpenInterestPage {
            market: desktop.chart.market.clone(),
            support: mexc_model::OpenInterestSupport {
                unit: "contracts".into(),
                history_interval_ms: Some(300_000),
                live_interval_ms: Some(10_000),
                description: "Native historical and live quantities".into(),
            },
            samples: Vec::new(),
            next_before_ms: cursor,
        }
    }

    #[test]
    fn late_open_interest_cannot_cross_removed_and_readded_instance_lifetimes() {
        let (mut desktop, _) = Desktop::new();
        let _ = desktop.update(Message::Action(Action::AddIndicator(
            IndicatorSpec::OpenInterest,
        )));
        let id = desktop.chart.indicators.instances.last().unwrap().id;
        let sample = OpenInterestSample {
            timestamp_ms: 1_000_000,
            quantity: 12.0,
            interval_ms: 10_000,
            timestamp_source: mexc_model::OpenInterestTimestamp::Venue,
        };
        let stale = open_interest_message(&desktop, vec![OpenInterestEvent::Sample(sample)]);
        let stale_history = Message::OpenInterestHistory {
            market: desktop.chart.market.clone(),
            generation: desktop.market_generation,
            open_interest_generation: desktop.open_interest_history.generation,
            before_ms: 1_000_000,
            result: Err("old market history failed".into()),
        };
        let _ = desktop.update(Message::Action(Action::RemoveIndicator(id)));
        let _ = desktop.update(Message::Action(Action::AddIndicator(
            IndicatorSpec::OpenInterest,
        )));
        let current = OpenInterestSample {
            quantity: 34.0,
            ..sample
        };
        let message = open_interest_message(&desktop, vec![OpenInterestEvent::Sample(current)]);
        let _ = desktop.update(message);
        let _ = desktop.update(stale);
        let _ = desktop.update(stale_history);
        assert_eq!(desktop.chart.open_interest.samples, vec![current]);
        assert_eq!(desktop.open_interest_history.error, None);
    }

    #[test]
    fn refreshed_recent_oi_history_never_rewinds_or_reopens_the_older_cursor() {
        let (mut desktop, _) = Desktop::new();
        let _ = desktop.update(Message::Action(Action::AddIndicator(
            IndicatorSpec::OpenInterest,
        )));
        let page = open_interest_page(&desktop, Some(5_000_000));
        let message =
            open_interest_message(&desktop, vec![OpenInterestEvent::History(Arc::new(page))]);
        let _ = desktop.update(message);
        desktop.open_interest_history.next_before_ms = Some(1_000_000);
        let page = open_interest_page(&desktop, Some(6_000_000));
        let message =
            open_interest_message(&desktop, vec![OpenInterestEvent::History(Arc::new(page))]);
        let _ = desktop.update(message);
        assert_eq!(
            desktop.open_interest_history.next_before_ms,
            Some(1_000_000)
        );
        desktop.open_interest_history.next_before_ms = None;
        let page = open_interest_page(&desktop, Some(7_000_000));
        let message =
            open_interest_message(&desktop, vec![OpenInterestEvent::History(Arc::new(page))]);
        let _ = desktop.update(message);
        assert_eq!(desktop.open_interest_history.next_before_ms, None);
    }

    #[test]
    fn external_source_lifetimes_preserve_shared_totals_and_reject_old_samples() {
        use mexc_indicators::{IndicatorSource, SourceMode};
        use mexc_model::{OpenInterestSupport, OpenInterestTimestamp};
        let (mut desktop, _) = Desktop::new();
        let binance = Market {
            exchange: Exchange::Binance,
            symbol: "BTCUSDT".into(),
            ..Market::default()
        };
        let mexc = desktop.chart.market.clone();
        let apply = |markets| Action::ApplyIndicator {
            id: None,
            spec: IndicatorSpec::OpenInterest,
            source: IndicatorSource {
                mode: SourceMode::Selected,
                markets,
            },
        };
        let _ = desktop.update(Message::Action(apply(vec![binance.clone()])));
        let single = desktop.chart.indicators.instances[0].id;
        let _ = desktop.update(Message::Action(apply(vec![binance.clone(), mexc.clone()])));
        let aggregate = desktop.chart.indicators.instances[1].id;
        let batch = |desktop: &Desktop, market: &Market, quantity| {
            let now = source_controller::now_ms();
            Message::SourceData(source_feeds::Batch {
                scope: Arc::new(source_feeds::Scope {
                    market: market.clone(),
                    kind: source_feeds::Kind::OpenInterest,
                    generation: desktop.source_requests
                        [&(market.clone(), source_feeds::Kind::OpenInterest)]
                        .generation,
                    base_asset: "BTC".into(),
                }),
                events: vec![
                    source_feeds::Event::Metadata {
                        base_asset: "BTC".into(),
                    },
                    source_feeds::Event::OpenInterest(OpenInterestEvent::Support(
                        OpenInterestSupport {
                            unit: "BTC".into(),
                            history_interval_ms: None,
                            live_interval_ms: Some(5000),
                            description: String::new(),
                        },
                    )),
                    source_feeds::Event::OpenInterest(OpenInterestEvent::Sample(
                        OpenInterestSample {
                            timestamp_ms: now,
                            timestamp_source: OpenInterestTimestamp::Venue,
                            quantity,
                            interval_ms: 5000,
                        },
                    )),
                    source_feeds::Event::OpenInterest(OpenInterestEvent::Status {
                        connected: true,
                        message: String::new(),
                    }),
                ]
                .into_boxed_slice(),
            })
        };
        let first = batch(&desktop, &binance, 5.0);
        let second = batch(&desktop, &mexc, 10.0);
        let stale = batch(&desktop, &mexc, 999.0);
        let _ = desktop.update(first);
        let _ = desktop.update(second);
        assert_eq!(
            desktop.chart.indicators.instances[1]
                .external
                .as_ref()
                .unwrap()
                .oi
                .last()
                .unwrap()
                .quantity,
            15.0
        );
        let old_binance = batch(&desktop, &binance, 999.0);
        let mexc_generation =
            desktop.source_requests[&(mexc.clone(), source_feeds::Kind::OpenInterest)].generation;
        let _ = desktop.update(Message::RetryIndicatorSources(single));
        assert!(
            !desktop.chart.indicators.instances[0]
                .external
                .as_ref()
                .unwrap()
                .complete
        );
        assert!(
            !desktop.chart.indicators.instances[1]
                .external
                .as_ref()
                .unwrap()
                .complete
        );
        assert_eq!(
            desktop.source_requests[&(mexc.clone(), source_feeds::Kind::OpenInterest)].generation,
            mexc_generation
        );
        let _ = desktop.update(old_binance);
        assert!(
            !desktop.chart.indicators.instances[0]
                .external
                .as_ref()
                .unwrap()
                .complete
        );
        let recovered = batch(&desktop, &binance, 5.0);
        let _ = desktop.update(recovered);
        assert_eq!(
            desktop.chart.indicators.instances[1]
                .external
                .as_ref()
                .unwrap()
                .oi
                .last()
                .unwrap()
                .quantity,
            15.0
        );
        let _ = desktop.update(Message::EditorClose);
        let _ = desktop.update(Message::Action(Action::RemoveIndicator(aggregate)));
        let _ = desktop.update(stale.clone());
        assert_eq!(desktop.chart.indicators.instances[0].id, single);
        assert_eq!(
            desktop.chart.indicators.instances[0]
                .external
                .as_ref()
                .unwrap()
                .oi
                .last()
                .unwrap()
                .quantity,
            5.0
        );
        let _ = desktop.update(Message::Action(apply(vec![binance, mexc.clone()])));
        let current = batch(&desktop, &mexc, 17.0);
        let _ = desktop.update(current);
        let _ = desktop.update(stale);
        let external = desktop.chart.indicators.instances[1]
            .external
            .as_ref()
            .unwrap();
        assert!(external.complete);
        assert_eq!(external.oi.last().unwrap().quantity, 22.0);
    }

    #[test]
    fn native_resync_repairs_cvd_coverage_without_falsely_disconnecting() {
        use mexc_indicators::{CvdReset, IndicatorSource, SourceMode};
        use mexc_model::{FlowQuality, VolumeFlow};
        let (mut desktop, _) = Desktop::new();
        let market = Market {
            exchange: Exchange::Binance,
            symbol: "BTCUSDT".into(),
            ..Market::default()
        };
        let _ = desktop.update(Message::Action(Action::ApplyIndicator {
            id: None,
            spec: IndicatorSpec::Cvd {
                reset: CvdReset::Continuous,
            },
            source: IndicatorSource {
                mode: SourceMode::Single,
                markets: vec![market.clone()],
            },
        }));
        let now = source_controller::now_ms();
        let time = now.div_euclid(300_000) * 300;
        let generation =
            desktop.source_requests[&(market.clone(), source_feeds::Kind::Cvd)].generation;
        let _ = desktop.update(Message::SourceData(source_feeds::Batch {
            scope: Arc::new(source_feeds::Scope {
                market,
                generation,
                kind: source_feeds::Kind::Cvd,
                base_asset: "BTC".into(),
            }),
            events: vec![
                source_feeds::Event::Metadata {
                    base_asset: "BTC".into(),
                },
                source_feeds::Event::Market(MarketEvent::Status {
                    connected: true,
                    message: String::new(),
                }),
                source_feeds::Event::Market(MarketEvent::Resync),
                source_feeds::Event::Market(MarketEvent::Flow {
                    bar: FlowBar {
                        time,
                        flow: VolumeFlow {
                            delta: Some(4.0),
                            volume: Some(8.0),
                            quality: FlowQuality::Exact,
                            covered_from: time,
                            through_ms: now,
                        },
                    },
                }),
            ]
            .into_boxed_slice(),
        }));
        let instance = &desktop.chart.indicators.instances[0];
        assert!(instance.external.as_ref().unwrap().complete);
        assert_eq!(instance.points.last().unwrap().values[0], Some(4.0));
    }

    #[test]
    fn direct_interval_uses_active_catalog_after_cancelled_foreign_search() {
        use mexc_model::{Instrument, OpenInterestTimestamp};
        let (mut desktop, _) = Desktop::new();
        let market = desktop.chart.market.clone();
        let catalog = Arc::new(Catalog {
            exchange: market.exchange,
            product: market.product,
            intervals: vec![Timeframe::Min5, Timeframe::Hour8],
            flow_support: flow_support(),
            instruments: vec![Instrument {
                exchange: market.exchange,
                product: market.product,
                symbol: market.symbol.clone(),
                base_coin: "BTC".into(),
                quote_coin: Quote::USDC,
                settle_coin: "USDC".into(),
                price_scale: 2,
                tick_size: None,
                contract_size: 0.0001,
                inverse: false,
                volume_unit: "contracts".into(),
            }],
        });
        let _ = desktop.update(Message::Catalog {
            exchange: market.exchange,
            product: market.product,
            generation: desktop.catalog_generation,
            result: Ok(catalog),
        });
        desktop.chart.merge_page(&page(300, None), 0);
        let sample = OpenInterestSample {
            timestamp_ms: 312_345,
            timestamp_source: OpenInterestTimestamp::Observed,
            quantity: 7.5,
            interval_ms: 5000,
        };
        desktop.chart.open_interest.samples.push(sample);
        let _ = desktop.update(Message::Action(Action::AddIndicator(
            IndicatorSpec::OpenInterest,
        )));
        let id = desktop.chart.indicators.instances[0].id;
        // Cancelled search belongs to another venue/product. It must not supply
        // intervals or metadata for the still-active price market.
        desktop.selector.exchange = Exchange::Okx;
        desktop.selector.product = Product::Spot;
        desktop.selector.set_catalog(
            Exchange::Okx,
            Product::Spot,
            Ok(Arc::new(Catalog {
                exchange: Exchange::Okx,
                product: Product::Spot,
                intervals: vec![Timeframe::Day1],
                flow_support: flow_support(),
                instruments: Vec::new(),
            })),
        );
        let before = pipeline_snapshot(&desktop);
        let _ = desktop.update(Message::SelectInterval(Timeframe::Day1));
        assert_eq!(pipeline_snapshot(&desktop), before);
        assert_eq!(desktop.chart.open_interest.samples, vec![sample]);
        let _ = desktop.update(Message::SelectInterval(Timeframe::Hour8));
        assert_eq!(
            desktop.chart.market,
            Market {
                interval: Timeframe::Hour8,
                ..market
            }
        );
        assert_eq!(desktop.chart.base_asset, "BTC");
        assert_eq!(desktop.native_quote, "USDC");
        assert_eq!(desktop.chart.open_interest.samples, vec![sample]);
        assert_eq!(desktop.chart.indicators.instances[0].id, id);
        assert_eq!(
            desktop.chart.indicators.instances[0].spec,
            IndicatorSpec::OpenInterest
        );
        assert_eq!(desktop.chart.len(), 0);
    }

    #[test]
    fn dialogs_block_navigation_without_stopping_native_data() {
        let (mut desktop, _) = Desktop::new();
        let market = desktop.chart.market.clone();
        let generation = desktop.market_generation;
        desktop
            .chart
            .merge_page(&page(300, None), desktop.chart.epoch);
        desktop.chart.dispatch(Action::Pan {
            time: -0.12,
            price: 0.12,
        });
        let viewport = desktop.chart.viewport;
        for dialog in 0..3 {
            desktop.manager.open = dialog == 0;
            desktop.selector.open = dialog == 1;
            desktop.details = (dialog == 2).then_some(Details::Status);
            for action in [
                Action::Live,
                Action::Reset,
                Action::Pan {
                    time: 0.5,
                    price: 0.5,
                },
                Action::Inspect(Some((300.0, 1.0))),
            ] {
                let _ = desktop.update(Message::Action(action));
            }
            let current = desktop.chart.viewport;
            assert_eq!(
                (
                    current.left,
                    current.span,
                    current.low,
                    current.high,
                    current.auto_fit,
                    current.live
                ),
                (
                    viewport.left,
                    viewport.span,
                    viewport.low,
                    viewport.high,
                    viewport.auto_fit,
                    viewport.live
                )
            );
            assert_eq!(desktop.chart.inspection, None);
            let _ = desktop.update(data_message(
                &market,
                generation,
                MarketEvent::Candle {
                    candle: page(600 + dialog * 300, None).candles[0],
                },
            ));
            assert_eq!(desktop.chart.latest().unwrap().time, 600 + dialog * 300);
        }
        let _ = desktop.update(Message::DetailsClose);
        let _ = desktop.update(Message::Action(Action::Live));
        assert!(desktop.chart.viewport.live);
        assert_eq!(desktop.chart.viewport.low, viewport.low);
        assert_eq!(desktop.chart.viewport.high, viewport.high);
    }

    #[test]
    fn source_repair_changes_only_the_named_instance_and_releases_its_feed() {
        use mexc_indicators::{CvdReset, SourceMode};
        let (mut desktop, _) = Desktop::new();
        let cvd_market = Market {
            exchange: Exchange::Binance,
            symbol: "BTCUSDT".into(),
            ..Market::default()
        };
        let oi_market = Market {
            exchange: Exchange::Okx,
            symbol: "BTC-USDT-SWAP".into(),
            ..Market::default()
        };
        for (spec, market) in [
            (
                IndicatorSpec::Cvd {
                    reset: CvdReset::UtcDay,
                },
                cvd_market.clone(),
            ),
            (IndicatorSpec::OpenInterest, oi_market.clone()),
        ] {
            let _ = desktop.update(Message::Action(Action::ApplyIndicator {
                id: None,
                spec,
                source: IndicatorSource {
                    mode: SourceMode::Single,
                    markets: vec![market],
                },
            }));
        }
        let cvd = desktop.chart.indicators.instances[0].id;
        let other = desktop.chart.indicators.instances[1].source.clone();
        let oi_generation = desktop.source_requests
            [&(oi_market.clone(), source_feeds::Kind::OpenInterest)]
            .generation;
        desktop.chart.dispatch(Action::ToggleIndicator(cvd));
        // A native context change invalidates BTC contributors, never substitutes ETH.
        desktop.chart.base_asset = "ETH".into();
        desktop.chart.market.symbol = "ETH_USDC".into();
        desktop.sync_indicator_sources();
        let _ = desktop.update(Message::UseChartSource(cvd));
        let repaired = &desktop.chart.indicators.instances[0];
        assert_eq!(repaired.id, cvd);
        assert_eq!(
            repaired.spec,
            IndicatorSpec::Cvd {
                reset: CvdReset::UtcDay
            }
        );
        assert!(!repaired.visible);
        assert!(repaired.source.is_chart());
        assert_eq!(desktop.chart.market.symbol, "ETH_USDC");
        assert_eq!(desktop.chart.indicators.instances[1].source, other);
        assert!(
            !desktop
                .source_requests
                .contains_key(&(cvd_market, source_feeds::Kind::Cvd))
        );
        assert_eq!(
            desktop.source_requests[&(oi_market, source_feeds::Kind::OpenInterest)].generation,
            oi_generation
        );
    }

    #[test]
    fn all_context_change_rechecks_newly_eligible_exchanges() {
        use mexc_indicators::{CvdReset, SourceMode};
        use mexc_model::{FlowAvailability, FlowQuality, FlowSupport, Instrument, VolumeFlow};
        let (mut desktop, _) = Desktop::new();
        desktop.chart.market.interval = Timeframe::Hour8;
        let market = |exchange, symbol: &str| Market {
            exchange,
            symbol: symbol.into(),
            ..desktop.chart.market.clone()
        };
        let a = market(Exchange::Binance, "BTCUSDC");
        let b = market(Exchange::Mexc, "BTC_USDC");
        for exchange in Exchange::ALL {
            desktop.source_catalogs.insert(
                (exchange, Product::Perpetual),
                Ok(Arc::new(Catalog {
                    exchange,
                    product: Product::Perpetual,
                    intervals: if exchange == Exchange::Okx {
                        vec![Timeframe::Min5]
                    } else {
                        vec![Timeframe::Hour8, Timeframe::Min5]
                    },
                    flow_support: FlowSupport {
                        history: FlowAvailability::Exact,
                        live: FlowAvailability::Exact,
                        description: String::new(),
                    },
                    instruments: if exchange == Exchange::Coinbase {
                        Vec::new()
                    } else {
                        vec![Instrument {
                            exchange,
                            product: Product::Perpetual,
                            symbol: match exchange {
                                Exchange::Binance => "BTCUSDC",
                                Exchange::Mexc => "BTC_USDC",
                                _ => "BTC-USDC-SWAP",
                            }
                            .into(),
                            base_coin: "BTC".into(),
                            quote_coin: Quote::USDC,
                            settle_coin: "USDC".into(),
                            price_scale: 2,
                            tick_size: None,
                            contract_size: 1.0,
                            inverse: false,
                            volume_unit: "BTC".into(),
                        }]
                    },
                })),
            );
        }
        desktop.active_catalog = desktop
            .source_catalogs
            .get(&(Exchange::Mexc, Product::Perpetual))
            .unwrap()
            .as_ref()
            .ok()
            .cloned();
        let _ = desktop.update(Message::Action(Action::ApplyIndicator {
            id: None,
            spec: IndicatorSpec::Cvd {
                reset: CvdReset::Continuous,
            },
            source: IndicatorSource {
                mode: SourceMode::All,
                markets: vec![a.clone(), b.clone()],
            },
        }));
        let seed = |desktop: &mut Desktop| {
            for (mut market, delta) in [(a.clone(), 5.0), (b.clone(), 7.0)] {
                market.interval = desktop.chart.market.interval;
                let mut source = SourceData::new(market.clone(), "BTC".into());
                source.flow_connected = true;
                source.merge_flows(
                    &[FlowBar {
                        time: 0,
                        flow: VolumeFlow {
                            delta: Some(delta),
                            volume: Some(delta),
                            quality: FlowQuality::Exact,
                            covered_from: 0,
                            through_ms: 1_000,
                        },
                    }],
                    true,
                );
                desktop.source_inputs.insert(market, source);
            }
            desktop
                .chart
                .refresh_indicator_sources(&desktop.source_inputs, 1_000);
        };
        seed(&mut desktop);
        assert!(
            desktop.chart.indicators.instances[0]
                .external
                .as_ref()
                .unwrap()
                .complete
        );
        assert_eq!(
            desktop.chart.indicators.instances[0]
                .points
                .last()
                .unwrap()
                .values[0],
            Some(12.0)
        );
        let membership = desktop.chart.indicators.instances[0].source.clone();
        let _ = desktop.update(Message::SelectInterval(Timeframe::Min5));
        seed(&mut desktop);
        assert_eq!(desktop.chart.indicators.instances[0].source, membership);
        // OKX is now eligible, but it was never explicitly selected.
        assert!(
            !desktop.chart.indicators.instances[0]
                .external
                .as_ref()
                .unwrap()
                .complete
        );
        assert!(
            desktop.chart.indicators.instances[0]
                .points
                .iter()
                .all(|point| point.values[0].is_none())
        );
        assert!(
            desktop
                .indicator_source_health(&desktop.chart.indicators.instances[0])
                .needs_repair()
        );
        let id = desktop.chart.indicators.instances[0].id;
        let okx = Market {
            exchange: Exchange::Okx,
            symbol: "BTC-USDC-SWAP".into(),
            ..desktop.chart.market.clone()
        };
        let mut repaired = membership.clone();
        repaired.markets.push(okx.clone());
        let _ = desktop.update(Message::Action(Action::ApplyIndicator {
            id: Some(id),
            spec: IndicatorSpec::Cvd {
                reset: CvdReset::Continuous,
            },
            source: repaired.clone(),
        }));
        let mut source = SourceData::new(okx.clone(), "BTC".into());
        source.flow_connected = true;
        source.merge_flows(
            &[FlowBar {
                time: 0,
                flow: VolumeFlow {
                    delta: Some(3.0),
                    volume: Some(3.0),
                    quality: FlowQuality::Exact,
                    covered_from: 0,
                    through_ms: 1_000,
                },
            }],
            true,
        );
        desktop.source_inputs.insert(okx, source);
        desktop
            .chart
            .refresh_indicator_sources(&desktop.source_inputs, 1_000);
        assert_eq!(desktop.chart.indicators.instances[0].id, id);
        assert!(
            desktop.chart.indicators.instances[0]
                .external
                .as_ref()
                .unwrap()
                .complete
        );
        assert_eq!(
            desktop.chart.indicators.instances[0]
                .points
                .last()
                .unwrap()
                .values[0],
            Some(15.0)
        );
        assert!(
            !desktop
                .indicator_source_health(&desktop.chart.indicators.instances[0])
                .needs_repair()
        );

        let _ = desktop.update(Message::SelectInterval(Timeframe::Hour8));
        seed(&mut desktop);
        // A now-ineligible explicit OKX choice is not silently dropped to recover a subset.
        assert_eq!(desktop.chart.indicators.instances[0].source, repaired);
        assert!(
            !desktop.chart.indicators.instances[0]
                .external
                .as_ref()
                .unwrap()
                .complete
        );
        assert!(
            desktop.chart.indicators.instances[0]
                .points
                .iter()
                .all(|point| point.values[0].is_none())
        );
    }

    #[test]
    fn all_discovery_fails_closed_and_rejects_superseded_catalog_callbacks() {
        use mexc_indicators::SourceMode;
        use mexc_model::{Instrument, OpenInterestTimestamp};
        let (mut desktop, _) = Desktop::new();
        let market = desktop.chart.market.clone();
        let native = Arc::new(Catalog {
            exchange: market.exchange,
            product: market.product,
            intervals: vec![Timeframe::Min5],
            flow_support: flow_support(),
            instruments: vec![Instrument {
                exchange: market.exchange,
                product: market.product,
                symbol: market.symbol.clone(),
                base_coin: "BTC".into(),
                quote_coin: Quote::USDC,
                settle_coin: "USDC".into(),
                price_scale: 2,
                tick_size: None,
                contract_size: 1.0,
                inverse: false,
                volume_unit: "BTC".into(),
            }],
        });
        let empty = |exchange| {
            Arc::new(Catalog {
                exchange,
                product: Product::Perpetual,
                intervals: vec![Timeframe::Min5],
                flow_support: flow_support(),
                instruments: Vec::new(),
            })
        };
        let membership = IndicatorSource {
            mode: SourceMode::All,
            markets: vec![market.clone()],
        };
        let _ = desktop.update(Message::Action(Action::ApplyIndicator {
            id: None,
            spec: IndicatorSpec::OpenInterest,
            source: membership.clone(),
        }));
        assert!(!desktop.manager.open);
        let generation = desktop.source_catalog_generation;
        let mut input = SourceData::new(market.clone(), "BTC".into());
        input.oi_connected = true;
        input.oi_cadence_ms = Some(5_000);
        input.merge_oi(&[OpenInterestSample {
            timestamp_ms: 1_000,
            quantity: 5.0,
            interval_ms: 5_000,
            timestamp_source: OpenInterestTimestamp::Venue,
        }]);
        desktop.source_inputs.insert(market.clone(), input);
        desktop
            .chart
            .refresh_indicator_sources(&desktop.source_inputs, 1_000);
        assert!(
            !desktop.chart.indicators.instances[0]
                .external
                .as_ref()
                .unwrap()
                .complete
        );
        for exchange in [Exchange::Binance, Exchange::Okx, Exchange::Coinbase] {
            let _ = desktop.update(Message::SourceCatalog {
                exchange,
                product: Product::Perpetual,
                generation,
                result: if exchange == Exchange::Binance {
                    Err("discovery failed".into())
                } else {
                    Ok(empty(exchange))
                },
            });
        }
        // Native discovery fulfills the concurrent MEXC source request.
        let _ = desktop.update(Message::Catalog {
            exchange: market.exchange,
            product: market.product,
            generation: desktop.catalog_generation,
            result: Ok(Arc::clone(&native)),
        });
        desktop
            .chart
            .refresh_indicator_sources(&desktop.source_inputs, 1_000);
        assert!(
            !desktop.chart.indicators.instances[0]
                .external
                .as_ref()
                .unwrap()
                .complete
        );
        let _ = desktop.update(Message::Manager(indicator_manager::Event::SourceRetry));
        let current_generation = desktop.source_catalog_generation;
        assert_ne!(generation, current_generation);
        let _ = desktop.update(Message::SourceCatalog {
            exchange: Exchange::Binance,
            product: Product::Perpetual,
            generation,
            result: Ok(empty(Exchange::Binance)),
        });
        desktop
            .chart
            .refresh_indicator_sources(&desktop.source_inputs, 1_000);
        assert!(
            !desktop.chart.indicators.instances[0]
                .external
                .as_ref()
                .unwrap()
                .complete
        );
        let _ = desktop.update(Message::SourceCatalog {
            exchange: Exchange::Binance,
            product: Product::Perpetual,
            generation: current_generation,
            result: Ok(empty(Exchange::Binance)),
        });
        desktop
            .chart
            .refresh_indicator_sources(&desktop.source_inputs, 1_000);
        assert!(
            desktop.chart.indicators.instances[0]
                .external
                .as_ref()
                .unwrap()
                .complete
        );
        assert_eq!(
            desktop.chart.indicators.instances[0]
                .external
                .as_ref()
                .unwrap()
                .oi
                .last()
                .unwrap()
                .quantity,
            5.0
        );
        assert_eq!(desktop.chart.indicators.instances[0].source, membership);
        // Both redundant source completion and stale native discovery must leave accepted coverage intact.
        let _ = desktop.update(Message::SourceCatalog {
            exchange: market.exchange,
            product: market.product,
            generation: current_generation,
            result: Err("redundant old request".into()),
        });
        let _ = desktop.update(Message::Catalog {
            exchange: market.exchange,
            product: market.product,
            generation: desktop.catalog_generation.wrapping_sub(1),
            result: Ok(empty(market.exchange)),
        });
        desktop
            .chart
            .refresh_indicator_sources(&desktop.source_inputs, 1_000);
        assert!(
            desktop.chart.indicators.instances[0]
                .external
                .as_ref()
                .unwrap()
                .complete
        );
        assert_eq!(
            desktop.chart.indicators.instances[0]
                .external
                .as_ref()
                .unwrap()
                .oi
                .last()
                .unwrap()
                .quantity,
            5.0
        );
        assert_eq!(desktop.chart.indicators.instances[0].source, membership);
    }

    #[test]
    fn drawing_transactions_leave_price_pipeline_scales_and_cvd_unchanged() {
        use crate::drawing_types::{Anchor, Anchors, Shape, Style};
        use mexc_indicators::CvdReset;
        let (mut desktop, _) = Desktop::new();
        let mut history = page(300, None);
        history.candles[0].flow = Some(mexc_model::VolumeFlow {
            delta: Some(1.25),
            volume: Some(3.0),
            quality: mexc_model::FlowQuality::Exact,
            covered_from: 300,
            through_ms: 600_000,
        });
        desktop.chart.merge_page(&history, 0);
        desktop
            .chart
            .dispatch(Action::AddIndicator(IndicatorSpec::Cvd {
                reset: CvdReset::Continuous,
            }));
        desktop.chart.dispatch(Action::Pan {
            time: -0.1,
            price: 0.2,
        });
        let before = pipeline_snapshot(&desktop);
        let viewport = desktop.chart.viewport;
        let points = desktop.chart.indicators.instances[0].points[0].values;
        let origin = desktop.chart.indicators.instances[0]
            .cvd_context(0)
            .unwrap()
            .origin;
        let generation = desktop.drawings.generation();
        let _ = desktop.update(Message::DrawingCommand {
            generation,
            command: DrawingCommand::Create {
                shape: Shape::Horizontal(Anchor {
                    utc_ms: 300_000,
                    price: 1.0,
                }),
                style: Style::default(),
            },
        });
        let id = desktop.drawings.selected().unwrap();
        for command in [
            DrawingCommand::Move {
                id,
                anchors: Anchors::One(Anchor {
                    utc_ms: 300_000,
                    price: 1.25,
                }),
            },
            DrawingCommand::SetLocked { id, locked: true },
            DrawingCommand::Undo,
        ] {
            let _ = desktop.update(Message::DrawingCommand {
                generation,
                command,
            });
        }
        assert_eq!(
            desktop.drawings.find(id).unwrap().shape,
            Shape::Horizontal(Anchor {
                utc_ms: 300_000,
                price: 1.25
            })
        );
        assert_eq!(pipeline_snapshot(&desktop), before);
        let after = desktop.chart.viewport;
        assert_eq!(
            (
                after.left,
                after.span,
                after.low,
                after.high,
                after.live,
                after.auto_fit
            ),
            (
                viewport.left,
                viewport.span,
                viewport.low,
                viewport.high,
                viewport.live,
                viewport.auto_fit
            )
        );
        assert_eq!(
            desktop.chart.indicators.instances[0].points[0].values,
            points
        );
        assert_eq!(
            desktop.chart.indicators.instances[0]
                .cvd_context(0)
                .unwrap()
                .origin,
            origin
        );
    }

    #[test]
    fn modal_cancels_drawing_transaction_and_rejects_queued_release_while_prices_advance() {
        use crate::drawing_types::{Anchor, Hit, Part, Shape, Style};
        let (mut desktop, _) = Desktop::new();
        desktop.chart.merge_page(&page(300, None), 0);
        let anchor = Anchor {
            utc_ms: 300_000,
            price: 1.0,
        };
        desktop
            .drawings
            .apply(DrawingCommand::Create {
                shape: Shape::Horizontal(anchor),
                style: Style::default(),
            })
            .unwrap();
        let id = desktop.drawings.selected().unwrap();
        let projection = drawing_geometry::Projection::new(
            desktop.chart.viewport,
            iced::Rectangle::with_size(iced::Size::new(500.0, 300.0)),
        )
        .unwrap();
        let start = projection.point(anchor);
        let end = iced::Point::new(start.x, start.y - 30.0);
        let generation = desktop.drawing_interaction.generation();
        let _ = desktop.update(Message::DrawingInput {
            generation,
            event: drawing_interaction::Input::Down {
                point: start,
                projection,
                hit: Some(Hit {
                    id,
                    part: Part::Body,
                }),
                snap: None,
            },
        });
        let _ = desktop.update(Message::DrawingInput {
            generation,
            event: drawing_interaction::Input::Move {
                point: end,
                projection,
                hit: None,
                snap: None,
            },
        });
        assert!(desktop.drawing_interaction.is_dragging());
        let revision = desktop.drawings.revision();
        let _ = desktop.update(Message::DrawingMenu);
        assert!(!desktop.drawing_interaction.is_dragging());
        let market = desktop.chart.market.clone();
        let _ = desktop.update(data_message(
            &market,
            desktop.market_generation,
            MarketEvent::Candle {
                candle: page(600, None).candles[0],
            },
        ));
        assert_eq!(desktop.chart.latest().unwrap().time, 600);
        for closed in [false, true] {
            if closed {
                let _ = desktop.update(Message::DrawingClose);
            }
            let _ = desktop.update(Message::DrawingInput {
                generation,
                event: drawing_interaction::Input::Up {
                    point: end,
                    projection,
                    snap: None,
                },
            });
            assert_eq!(
                desktop.drawings.find(id).unwrap().shape,
                Shape::Horizontal(anchor)
            );
            assert_eq!(desktop.drawings.revision(), revision);
        }
    }

    #[test]
    fn drawing_modal_owns_captured_menu_shortcuts_without_reinterpreting_properties_text() {
        use crate::drawing_types::{Anchor, Shape, Style};
        use iced::keyboard::{Key, key::Named};
        let (mut desktop, _) = Desktop::new();
        desktop.chart.merge_page(&page(300, None), 0);
        let previous = Shape::Horizontal(Anchor {
            utc_ms: 300_000,
            price: 0.875,
        });
        desktop
            .drawings
            .apply(DrawingCommand::Create {
                shape: previous.clone(),
                style: Style::default(),
            })
            .unwrap();
        let previous_id = desktop.drawings.selected().unwrap();
        let before = pipeline_snapshot(&desktop);
        let _ = desktop.update(Message::DrawingMenu);
        let _ = desktop.update(Message::Key {
            key: Key::Character("1".into()),
            modifiers: keyboard::Modifiers::default(),
            captured: true,
        });
        assert_eq!(desktop.drawings.selected(), None);
        assert_eq!(desktop.drawings.find(previous_id).unwrap().shape, previous);
        let generation = desktop.drawing_interaction.generation();
        let anchor = Anchor {
            utc_ms: 300_000,
            price: 1.125,
        };
        let _ = desktop.update(Message::DrawingInput {
            generation,
            event: drawing_interaction::Input::Key {
                key: Key::Named(Named::Enter),
                modifiers: keyboard::Modifiers::default(),
                seed: Some(anchor),
                step_ms: 300_000,
                price_step: 0.01,
            },
        });
        let id = desktop.drawings.selected().unwrap();
        assert_eq!(
            desktop.drawings.find(id).unwrap().shape,
            Shape::Horizontal(anchor)
        );
        assert_eq!(pipeline_snapshot(&desktop), before);
        let _ = desktop.update(Message::DrawingProperties(id));
        let _ = desktop.update(Message::DrawingControl(drawing_controls::Event::Field(
            drawing_controls::Field::PriceFirst,
            "2.25".into(),
        )));
        let _ = desktop.update(Message::Key {
            key: Key::Character("7".into()),
            modifiers: keyboard::Modifiers::default(),
            captured: true,
        });
        let _ = desktop.update(Message::Key {
            key: Key::Named(Named::Enter),
            modifiers: keyboard::Modifiers::CTRL,
            captured: true,
        });
        assert_eq!(
            desktop.drawings.find(id).unwrap().shape,
            Shape::Horizontal(Anchor {
                price: 2.25,
                ..anchor
            })
        );
        assert!(!desktop.drawing_controls.is_open());
    }

    #[test]
    fn window_cancellation_discards_preview_and_rejects_stale_pointer_release() {
        use crate::drawing_types::{Anchor, Hit, Part, Shape, Style};
        let (mut desktop, startup) = Desktop::new();
        drop(startup);
        desktop.chart.merge_page(&page(300, None), 0);
        desktop.commit_drawing(DrawingCommand::Create {
            shape: Shape::Horizontal(Anchor {
                utc_ms: 300_000,
                price: 1.0,
            }),
            style: Style::default(),
        });
        let id = desktop.drawings.selected().unwrap();
        let original = desktop.drawings.find(id).unwrap().shape.clone();
        let view = desktop.chart.viewport;
        let projection = drawing_geometry::Projection::new(
            view,
            iced::Rectangle::with_size(iced::Size::new(1000.0, 800.0)),
        )
        .unwrap();
        let generation = desktop.drawing_interaction.generation();
        let _ = desktop.update(Message::DrawingInput {
            generation,
            event: drawing_interaction::Input::Down {
                point: iced::Point::new(500.0, 400.0),
                projection,
                hit: Some(Hit {
                    id,
                    part: Part::Body,
                }),
                snap: None,
            },
        });
        let moved = iced::Point::new(500.0, 500.0);
        let _ = desktop.update(Message::DrawingInput {
            generation,
            event: drawing_interaction::Input::Move {
                point: moved,
                projection,
                hit: None,
                snap: None,
            },
        });
        let _ = desktop.update(Message::Action(Action::Pan {
            time: 0.2,
            price: 0.2,
        }));
        assert_eq!(
            (
                desktop.chart.viewport.left,
                desktop.chart.viewport.span,
                desktop.chart.viewport.low,
                desktop.chart.viewport.high,
                desktop.chart.viewport.auto_fit,
                desktop.chart.viewport.live
            ),
            (
                view.left,
                view.span,
                view.low,
                view.high,
                view.auto_fit,
                view.live
            ),
        );
        let _ = desktop.update(Message::PointerCancelled);
        let _ = desktop.update(Message::DrawingInput {
            generation,
            event: drawing_interaction::Input::Up {
                point: moved,
                projection,
                snap: None,
            },
        });
        assert_eq!(desktop.drawings.find(id).unwrap().shape, original);
        assert!(!desktop.drawing_interaction.is_dragging());
        assert_eq!(desktop.drawings.selected(), Some(id));
    }
}
