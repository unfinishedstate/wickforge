use crate::{
    Message, drawing_controls,
    drawing_types::{DrawingId, Tool},
    key_help::{self, Context},
    profile_controls,
    state::Action,
    ui,
    workspace::ResetKind,
};
use iced::widget::{button, column, container, row, scrollable, text, text_input};
use iced::{Element, Length, Task, keyboard};
use mexc_indicators::IndicatorKind;
use mexc_model::Market;

const INPUT_ID: &str = "command-palette-query";
const RESULTS_ID: &str = "command-palette-results";

/// Runtime information supplied by the window owner when opening the palette.
/// Editor contexts are deliberately treated as modal: chart-mutating commands
/// are not added to the palette at all.
#[derive(Debug, Clone)]
pub struct IndicatorTarget {
    pub id: u64,
    pub label: String,
    pub external_sources: bool,
}

#[derive(Debug, Clone)]
pub struct ProfileTarget {
    pub id: u64,
    pub label: String,
    pub visible: bool,
    pub fixed: bool,
}

#[derive(Debug, Clone)]
pub struct Snapshot {
    pub context: Context,
    pub active_pane: Option<u64>,
    pub selected_drawing: Option<DrawingId>,
    pub pane_ids: Vec<u64>,
    pub hidden_pane_ids: Vec<u64>,
    pub indicators: Vec<IndicatorTarget>,
    pub profiles: Vec<ProfileTarget>,
    pub recent_markets: Vec<Market>,
    pub favorite_markets: Vec<Market>,
}

impl Snapshot {
    pub fn new(context: Context) -> Self {
        Self {
            context,
            active_pane: None,
            selected_drawing: None,
            pane_ids: Vec::new(),
            hidden_pane_ids: Vec::new(),
            indicators: Vec::new(),
            profiles: Vec::new(),
            recent_markets: Vec::new(),
            favorite_markets: Vec::new(),
        }
    }

    fn chart_context(&self) -> bool {
        matches!(self.context, Context::Chart)
    }
}

#[derive(Debug, Clone)]
pub enum Event {
    Query(String),
    Move(i32),
    Tab,
    Enter,
    Escape,
    Execute(usize),
}

#[derive(Debug, Clone)]
struct Target {
    title: String,
    detail: String,
    search: String,
    message: Message,
}

#[derive(Debug, Clone)]
pub struct Commands {
    snapshot: Snapshot,
    query: String,
    selected: usize,
    targets: Vec<Target>,
    matches: Vec<usize>,
}

impl Commands {
    pub fn show(snapshot: Snapshot) -> Self {
        let mut commands = Self {
            snapshot,
            query: String::new(),
            selected: 0,
            targets: Vec::new(),
            matches: Vec::new(),
        };
        commands.build_targets();
        commands.filter();
        commands
    }

    pub fn selected(&self) -> Option<usize> {
        self.matches.get(self.selected).copied()
    }

    pub fn is_empty(&self) -> bool {
        self.matches.is_empty()
    }

    pub fn update(&mut self, event: Event) -> Option<Message> {
        match event {
            Event::Query(query) => {
                self.query = query;
                self.selected = 0;
                self.filter();
                None
            }
            Event::Move(delta) => {
                if !self.matches.is_empty() {
                    self.selected = if delta < 0 {
                        self.selected.saturating_sub(delta.unsigned_abs() as usize)
                    } else {
                        (self.selected + delta as usize).min(self.matches.len() - 1)
                    };
                } else {
                    self.selected = 0;
                }
                None
            }
            Event::Tab => {
                if !self.matches.is_empty() {
                    self.selected = (self.selected + 1) % self.matches.len();
                } else {
                    self.selected = 0;
                }
                None
            }
            Event::Enter => self.execute_selected(),
            Event::Escape => None,
            Event::Execute(index) => self.targets.get(index).map(|target| target.message.clone()),
        }
    }

    /// Handle palette navigation and report consumption even when no
    /// application message is emitted. This prevents chart keys leaking through.
    pub fn key(
        &mut self,
        key: keyboard::Key,
        modifiers: keyboard::Modifiers,
    ) -> (bool, Option<Message>) {
        use keyboard::{Key, key::Named};
        if modifiers.control() && matches!(key.as_ref(), Key::Character("k" | "K")) {
            return (false, None);
        }
        match key {
            Key::Named(Named::ArrowDown) if !modifiers.shift() => {
                self.update(Event::Move(1));
                (true, None)
            }
            Key::Named(Named::ArrowUp) => {
                self.update(Event::Move(-1));
                (true, None)
            }
            Key::Named(Named::PageDown) => {
                self.update(Event::Move(7));
                (true, None)
            }
            Key::Named(Named::PageUp) => {
                self.update(Event::Move(-7));
                (true, None)
            }
            Key::Named(Named::Tab) => {
                if modifiers.shift() && !self.matches.is_empty() {
                    self.selected = (self.selected + self.matches.len() - 1) % self.matches.len();
                } else {
                    self.update(Event::Tab);
                }
                (true, None)
            }
            Key::Named(Named::Enter) => (true, self.update(Event::Enter)),
            Key::Named(Named::Escape) => (true, None),
            _ => (false, None),
        }
    }

    pub fn execute_selected(&self) -> Option<Message> {
        self.selected()
            .and_then(|index| self.targets.get(index))
            .map(|target| target.message.clone())
    }

    pub fn view(&self) -> Element<'_, Message> {
        let input = text_input("Search commands…", &self.query)
            .id(text_input::Id::new(INPUT_ID))
            .on_input(|query| Message::CommandPalette(Event::Query(query)))
            .padding(12)
            .size(18)
            .style(ui::input);
        let mut results = column![].spacing(3);
        for (match_index, &target_index) in self.matches.iter().enumerate() {
            let target = &self.targets[target_index];
            let result = row![
                column![text(&target.title).size(15), text(&target.detail).size(12)]
                    .spacing(2)
                    .width(Length::Fill),
            ]
            .padding([8, 10])
            .align_y(iced::Alignment::Center);
            let style = if match_index == self.selected {
                ui::selected
            } else {
                ui::quiet
            };
            results = results.push(
                button(result)
                    .width(Length::Fill)
                    .style(style)
                    .on_press(Message::CommandPalette(Event::Execute(target_index))),
            );
        }
        let body = if self.is_empty() {
            results.push(text("No matching commands. Try another word.").size(13))
        } else {
            results
        };
        let context = if self.snapshot.chart_context() {
            "Chart context · Esc closes without changing the chart"
        } else {
            "Editor context · chart navigation and drawing mutations are unavailable"
        };
        let content = column![
            input,
            text("Up/Down / Tab select · Enter run · Esc close")
                .size(11)
                .style(ui::muted_text),
            text(context).size(11).style(ui::muted_text),
            scrollable(container(body).padding(iced::Padding {
                right: 16.0,
                ..Default::default()
            }))
            .id(scrollable::Id::new(RESULTS_ID))
            .height(Length::Fill)
            .width(Length::Fill),
        ]
        .spacing(10)
        .padding(16)
        .width(Length::Fill)
        .height(Length::Fill);
        container(content)
            .width(Length::Fill)
            .max_width(720)
            .height(Length::Fill)
            .max_height(620)
            .style(ui::panel)
            .into()
    }

    pub fn focus(&self) -> Task<Message> {
        text_input::focus(text_input::Id::new(INPUT_ID))
    }

    pub fn follow_selection(&self) -> Task<Message> {
        scrollable::snap_to(
            scrollable::Id::new(RESULTS_ID),
            scrollable::RelativeOffset {
                x: 0.0,
                y: if self.matches.len() <= 1 {
                    0.0
                } else {
                    self.selected as f32 / (self.matches.len() - 1) as f32
                },
            },
        )
    }

    fn filter(&mut self) {
        let words: Vec<String> = self
            .query
            .split_whitespace()
            .map(str::to_lowercase)
            .collect();
        self.matches = self
            .targets
            .iter()
            .enumerate()
            .filter(|(_, target)| words.iter().all(|word| target.search.contains(word)))
            .map(|(index, _)| index)
            .collect();
        if self.matches.is_empty() {
            self.selected = 0;
        } else {
            self.selected = self.selected.min(self.matches.len() - 1);
        }
    }

    fn add(
        &mut self,
        title: impl Into<String>,
        detail: impl Into<String>,
        terms: impl AsRef<str>,
        message: Message,
    ) {
        let title = title.into();
        let detail = detail.into();
        let search = format!("{title} {detail} {}", terms.as_ref()).to_lowercase();
        self.targets.push(Target {
            title,
            detail,
            search,
            message,
        });
    }

    fn add_action(&mut self, title: impl Into<String>, detail: impl Into<String>, action: Action) {
        self.add(title, detail, "", Message::Action(action));
    }

    fn build_targets(&mut self) {
        let chart = self.snapshot.chart_context();
        for item in key_help::command_entries(self.snapshot.context) {
            self.add(
                item.description,
                item.keys,
                "keyboard shortcut chart navigation",
                Message::Action(item.action),
            );
        }
        self.add(
            "Clear current chart",
            "Confirm before clearing indicators, current profiles, and this instrument's drawings",
            "clear reset current chart indicators drawings profiles workspace",
            Message::Workspace(crate::workspace_controls::Event::Open(ResetKind::Current)),
        );
        self.add(
            "Reset everything",
            "Confirm before restoring defaults and removing all workspace data",
            "reset everything all workspace defaults preferences layout profiles favorites recents",
            Message::Workspace(crate::workspace_controls::Event::Open(ResetKind::All)),
        );
        if !chart {
            self.add(
                "Open keyboard help",
                "F1",
                "help shortcuts",
                Message::HelpOpen,
            );
            return;
        }
        self.add(
            "Manage TPO / Volume Profiles",
            "Settings, native coverage and provenance for each profile instance",
            "profile manager tpo volume market auction distribution poc vah val single prints initial balance",
            Message::Profile(profile_controls::Event::Open(None)),
        );
        self.add(
            "Add Visible Range Volume Profile",
            "Follow the visible chart range using the existing native profile engine",
            "indicator visible range volume profile vpvr histogram poc vah val",
            Message::ProfileEntry(crate::indicator_manager::ProfileEntry::VisibleVolume),
        );
        self.add(
            "Draw Selected Range Volume Profile",
            "Place two time anchors; select its rail or Draw-menu object to edit endpoints or translate",
            "drawing selected fixed range volume profile vp anchors keyboard placement",
            Message::ProfileEntry(crate::indicator_manager::ProfileEntry::SelectedVolume),
        );
        for kind in [
            mexc_indicators::profiles::Kind::Tpo,
            mexc_indicators::profiles::Kind::Volume,
        ] {
            self.add(
                format!("Add {kind} profile"),
                "UTC day · native tick rows resolved from current price · 70% value area",
                format!("profile add {kind} poc vah val"),
                Message::Profile(profile_controls::Event::QuickAdd(kind)),
            );
            self.add(
                format!("Configure new {kind} profile"),
                "Choose UTC sessions, visible range, or a fixed range with mouse / UTC bounds",
                "profile new range fixed visible weekly monthly keyboard placement",
                Message::Profile(profile_controls::Event::New(kind)),
            );
        }
        for index in 0..self.snapshot.profiles.len() {
            let profile = &self.snapshot.profiles[index];
            let id = profile.id;
            let label = profile.label.clone();
            let visibility = if profile.visible { "Hide" } else { "Show" };
            let fixed = profile.fixed;
            self.add(
                format!("#{id} {label} profile settings"),
                "Configure this exact profile · coverage, as-of time and native source details",
                "profile settings provenance details",
                Message::Profile(profile_controls::Event::Open(Some(id))),
            );
            if fixed {
                self.add(
                    format!("Select range of profile #{id} {label}"),
                    "Select this fixed profile; Ctrl+E settings, Ctrl+H visibility, Delete removes",
                    "profile fixed range select handles",
                    Message::ProfileEdit(crate::profile_layer::Edit::Select(id)),
                );
            }
            self.add(
                format!("{visibility} profile #{id} {label}"),
                "Change only this profile's visibility; preserve its configuration",
                "profile visibility show hide",
                Message::Profile(profile_controls::Event::ToggleVisible(id)),
            );
            self.add(
                format!("Remove profile #{id} {label}"),
                "Delete only this exact profile instance",
                "profile remove delete",
                Message::Profile(profile_controls::Event::Remove(id)),
            );
        }

        for kind in IndicatorKind::ALL {
            self.add(
                format!("Add {}", kind.name()),
                format!(
                    "{} · Default parameters · Chart source",
                    kind.default_spec().label()
                ),
                format!(
                    "indicator {} {} add default",
                    kind.name(),
                    kind.default_spec().label()
                ),
                Message::Action(Action::AddIndicator(kind.default_spec())),
            );
        }
        for tool in Tool::ALL {
            self.add(
                format!("Drawing tool: {tool}"),
                format!("Select drawing tool · Draw menu key {}", tool_key(tool)),
                match tool {
                    Tool::Measure => {
                        "measure measurement ruler distance price percent time bars".into()
                    }
                    Tool::Rectangle => "rectangle box zone".into(),
                    _ => tool.to_string(),
                },
                Message::DrawingControl(drawing_controls::Event::ChooseTool(tool)),
            );
        }
        if let Some(id) = self.snapshot.selected_drawing {
            self.add(
                "Edit selected drawing",
                "Open drawing properties",
                "drawing edit style",
                Message::DrawingControl(drawing_controls::Event::Properties(id)),
            );
            self.add(
                "Hide selected drawing",
                "Hide drawing",
                "drawing visibility",
                Message::DrawingControl(drawing_controls::Event::Hide(id)),
            );
            self.add(
                "Lock selected drawing",
                "Prevent drawing edits",
                "drawing lock",
                Message::DrawingControl(drawing_controls::Event::Lock(id)),
            );
            self.add(
                "Delete selected drawing",
                "Remove drawing",
                "drawing delete",
                Message::DrawingControl(drawing_controls::Event::Delete(id)),
            );
        }
        self.add(
            "Undo drawing",
            "Undo last drawing change",
            "drawing undo",
            Message::DrawingControl(drawing_controls::Event::Undo),
        );
        self.add(
            "Redo drawing",
            "Redo drawing change",
            "drawing redo",
            Message::DrawingControl(drawing_controls::Event::Redo),
        );

        if let Some(id) = self.snapshot.active_pane {
            self.add_action("Fit active pane", "Fit pane contents", Action::FitPane(id));
            self.add(
                "Show active pane details",
                "Ctrl+J · units, quality and provenance",
                "indicator active details",
                Message::IndicatorDetails(id),
            );
            self.add_action(
                "Toggle active pane visibility",
                "Show or hide pane",
                Action::ToggleIndicator(id),
            );
            self.add_action(
                "Move active pane up",
                "Reorder lower panes · Ctrl+Alt+Shift+Up",
                Action::MovePane { id, direction: -1 },
            );
            self.add_action(
                "Move active pane down",
                "Reorder lower panes · Ctrl+Alt+Shift+Down",
                Action::MovePane { id, direction: 1 },
            );
            self.add_action(
                "Maximize or restore active pane",
                "Toggle pane layout · Ctrl+Alt+M",
                Action::TogglePaneMaximized(Some(id)),
            );
            self.add_action(
                "Increase active pane height",
                "Adjust pane layout",
                Action::AdjustPaneHeight {
                    id: Some(id),
                    delta: 32.0,
                },
            );
            self.add_action(
                "Decrease active pane height",
                "Adjust pane layout",
                Action::AdjustPaneHeight {
                    id: Some(id),
                    delta: -32.0,
                },
            );
            self.add_action(
                "Reset active pane height",
                "Restore default pane layout",
                Action::ResetPaneHeight(Some(id)),
            );
            self.add_action(
                "Activate active pane",
                "Focus pane controls",
                Action::ActivatePane(Some(id)),
            );
        }
        for index in 0..self.snapshot.pane_ids.len() {
            let id = self.snapshot.pane_ids[index];
            if Some(id) == self.snapshot.active_pane {
                continue;
            }
            self.add_action(
                format!("Activate pane {id}"),
                "Focus pane controls",
                Action::ActivatePane(Some(id)),
            );
        }
        for index in 0..self.snapshot.hidden_pane_ids.len() {
            let id = self.snapshot.hidden_pane_ids[index];
            self.add_action(
                format!("Show hidden pane {id}"),
                "Restore pane to the chart",
                Action::ToggleIndicator(id),
            );
        }
        self.add_action(
            "Maximize or restore price chart",
            "Toggle pane layout · Ctrl+Alt+M",
            Action::TogglePaneMaximized(None),
        );
        self.add(
            "Open UTC date jump",
            "Jump chart to a date and time · Ctrl+G",
            "date time utc jump",
            Message::GotoTimeOpen,
        );
        self.add_action("Fit chart", "Fit all visible chart data", Action::AutoFit);
        self.add_action("Go live", "Return to latest data", Action::Live);
        self.add_action(
            "Retry data sources",
            "Retry current market contributors",
            Action::Retry,
        );
        self.add(
            "Open status details",
            "Connection and source health",
            "health status details",
            Message::StatusDetails,
        );
        let mut seen_markets = std::collections::HashSet::new();
        let markets: Vec<_> = self
            .snapshot
            .favorite_markets
            .iter()
            .chain(self.snapshot.recent_markets.iter())
            .filter(|market| seen_markets.insert((**market).clone()))
            .cloned()
            .collect();
        for market in markets {
            let identity = format!(
                "{} {} {} · {}",
                market.exchange, market.product, market.symbol, market.interval
            );
            self.add(
                format!("Open {identity}"),
                "Saved quick market · exact exchange, product, symbol, and interval",
                format!("market recent favorite quick {identity}"),
                Message::QuickMarket(market),
            );
        }
        for index in 0..self.snapshot.indicators.len() {
            let indicator = &self.snapshot.indicators[index];
            let id = indicator.id;
            let external = indicator.external_sources;
            let settings = format!("#{} {} settings", id, indicator.label);
            let details = format!("#{} {} details", id, indicator.label);
            let terms: String = indicator
                .label
                .chars()
                .filter(|character| character.is_alphanumeric())
                .collect();
            self.add(
                settings,
                "Ctrl+E · configure this exact instance",
                &terms,
                Message::IndicatorSettings(id),
            );
            self.add(
                details,
                "Ctrl+J · units, quality and provenance",
                &terms,
                Message::IndicatorDetails(id),
            );
            if external {
                self.add(
                    format!("Retry indicator #{id} sources"),
                    "Retry this instance's explicitly selected contributors",
                    "indicator source retry",
                    Message::RetryIndicatorSources(id),
                );
            }
        }
        self.add(
            "Open command palette",
            "Ctrl+K · Search and run a command",
            "palette command",
            Message::CommandOpen,
        );
    }
}
fn tool_key(tool: Tool) -> usize {
    Tool::ALL
        .iter()
        .position(|candidate| *candidate == tool)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_search_edits_existing_instance_instead_of_adding_another() {
        let mut snapshot = Snapshot::new(Context::Chart);
        snapshot.indicators.push(IndicatorTarget {
            id: 42,
            label: "MACD(12, 26, 9)".into(),
            external_sources: false,
        });
        let mut commands = Commands::show(snapshot);
        commands.update(Event::Query("MACD settings".into()));
        assert!(matches!(
            commands.update(Event::Enter),
            Some(Message::IndicatorSettings(42))
        ));
    }

    #[test]
    fn ema_query_executes_the_native_default_indicator() {
        let mut commands = Commands::show(Snapshot::new(Context::Chart));
        commands.update(Event::Query("EMA".into()));
        assert!(matches!(
            commands.execute_selected(),
            Some(Message::Action(Action::AddIndicator(
                mexc_indicators::IndicatorSpec::Ema { period: 20 }
            )))
        ));
    }

    #[test]
    fn editor_context_and_zero_matches_never_select_chart_actions() {
        let mut commands = Commands::show(Snapshot::new(Context::Market));
        commands.update(Event::Query("pan".into()));
        assert!(commands.is_empty());
        assert_eq!(commands.selected(), None);
        assert!(commands.execute_selected().is_none());
        commands.update(Event::Tab);
        assert_eq!(commands.selected(), None);
    }
    #[test]
    fn fibonacci_and_measurement_queries_choose_their_real_tools() {
        let mut commands = Commands::show(Snapshot::new(Context::Chart));
        commands.update(Event::Query("Fibonacci".into()));
        assert!(matches!(
            commands.execute_selected(),
            Some(Message::DrawingControl(
                drawing_controls::Event::ChooseTool(Tool::Fibonacci)
            ))
        ));

        commands.update(Event::Query("measurement".into()));
        assert!(matches!(
            commands.execute_selected(),
            Some(Message::DrawingControl(
                drawing_controls::Event::ChooseTool(Tool::Measure)
            ))
        ));
    }

    #[test]
    fn drawing_modal_context_excludes_chart_and_selection_mutations() {
        let mut snapshot = Snapshot::new(Context::Drawings);
        snapshot.selected_drawing = Some(41);
        let mut commands = Commands::show(snapshot);
        for query in ["pan", "delete", "lock", "fibonacci"] {
            commands.update(Event::Query(query.into()));
            assert!(commands.is_empty(), "{query} must not mutate chart state");
            assert!(commands.execute_selected().is_none());
        }
    }

    #[test]
    fn fixed_profile_settings_search_does_not_select_the_range_instead() {
        let mut snapshot = Snapshot::new(Context::Chart);
        snapshot.profiles.push(ProfileTarget {
            id: 4,
            label: "Volume · Fixed range".into(),
            visible: false,
            fixed: true,
        });
        let mut commands = Commands::show(snapshot);
        commands.update(Event::Query("4 Volume settings".into()));
        assert!(matches!(
            commands.execute_selected(),
            Some(Message::Profile(profile_controls::Event::Open(Some(4))))
        ));
        commands.update(Event::Query("4 Volume select range".into()));
        assert!(matches!(
            commands.execute_selected(),
            Some(Message::ProfileEdit(crate::profile_layer::Edit::Select(4)))
        ));
    }

    #[test]
    fn search_disambiguates_same_kind_profile_instances_by_id() {
        let mut snapshot = Snapshot::new(Context::Chart);
        snapshot.profiles = vec![
            ProfileTarget {
                id: 7,
                label: "TPO Day".into(),
                visible: true,
                fixed: false,
            },
            ProfileTarget {
                id: 12,
                label: "TPO Day".into(),
                visible: false,
                fixed: false,
            },
        ];
        let mut commands = Commands::show(snapshot);
        for (query, expected) in [
            ("#12 settings", 0),
            ("show profile #12", 1),
            ("remove profile #7", 2),
        ] {
            commands.update(Event::Query(query.into()));
            match expected {
                0 => assert!(matches!(
                    commands.execute_selected(),
                    Some(Message::Profile(profile_controls::Event::Open(Some(12))))
                )),
                1 => assert!(matches!(
                    commands.execute_selected(),
                    Some(Message::Profile(profile_controls::Event::ToggleVisible(12)))
                )),
                _ => assert!(matches!(
                    commands.execute_selected(),
                    Some(Message::Profile(profile_controls::Event::Remove(7)))
                )),
            }
        }
    }
}
