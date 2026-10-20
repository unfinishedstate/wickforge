use crate::{Message, state::Action, ui, workspace::ResetKind};
use iced::advanced::{
    Clipboard, Layout, Shell, Widget, layout, renderer,
    widget::{Operation, Tree, tree},
};
use iced::widget::{button, column, container, mouse_area, row, scrollable, text};
use iced::{Alignment, Element, Length};
use iced::{Event, Rectangle, Renderer, Size, Theme, Vector, event, mouse};

pub const LIST_SCROLL_ID: &str = "keyboard-help-list";

#[derive(Debug, Clone, Copy)]
pub enum Context {
    Chart,
    Market,
    Indicators,
    Details,
    Drawings,
}

struct Entry {
    keys: &'static str,
    description: &'static str,
    action: Option<Action>,
}

static GLOBAL_HELP: Entry = Entry {
    keys: "F1 / ? · Ctrl+K",
    description: "Open keyboard help / command palette",
    action: None,
};

static CHART: &[Entry] = &[
    Entry {
        keys: "",
        description: "Chart actions",
        action: None,
    },
    Entry {
        keys: "Esc",
        description: "Clear chart inspection",
        action: Some(Action::Inspect(None)),
    },
    Entry {
        keys: "Left",
        description: "Pan time left",
        action: Some(Action::Pan {
            time: -0.12,
            price: 0.0,
        }),
    },
    Entry {
        keys: "Right",
        description: "Pan time right",
        action: Some(Action::Pan {
            time: 0.12,
            price: 0.0,
        }),
    },
    Entry {
        keys: "Up",
        description: "Pan active value scale up (price or indicator)",
        action: Some(Action::Pan {
            time: 0.0,
            price: 0.12,
        }),
    },
    Entry {
        keys: "Down",
        description: "Pan active value scale down (price or indicator)",
        action: Some(Action::Pan {
            time: 0.0,
            price: -0.12,
        }),
    },
    Entry {
        keys: "Shift+Left",
        description: "Pan time left farther",
        action: Some(Action::Pan {
            time: -0.5,
            price: 0.0,
        }),
    },
    Entry {
        keys: "Shift+Right",
        description: "Pan time right farther",
        action: Some(Action::Pan {
            time: 0.5,
            price: 0.0,
        }),
    },
    Entry {
        keys: "Shift+Up",
        description: "Pan active value scale up farther",
        action: Some(Action::Pan {
            time: 0.0,
            price: 0.5,
        }),
    },
    Entry {
        keys: "Shift+Down",
        description: "Pan active value scale down farther",
        action: Some(Action::Pan {
            time: 0.0,
            price: -0.5,
        }),
    },
    Entry {
        keys: "= / +",
        description: "Zoom in on time",
        action: Some(Action::Zoom {
            time: 0.8,
            price: 1.0,
            x: 0.5,
            y: 0.5,
        }),
    },
    Entry {
        keys: "- / _",
        description: "Zoom out on time",
        action: Some(Action::Zoom {
            time: 1.25,
            price: 1.0,
            x: 0.5,
            y: 0.5,
        }),
    },
    Entry {
        keys: "Shift+= / +",
        description: "Zoom in on active value scale",
        action: Some(Action::Zoom {
            time: 1.0,
            price: 0.8,
            x: 0.5,
            y: 0.5,
        }),
    },
    Entry {
        keys: "Shift+- / _",
        description: "Zoom out on active value scale",
        action: Some(Action::Zoom {
            time: 1.0,
            price: 1.25,
            x: 0.5,
            y: 0.5,
        }),
    },
    Entry {
        keys: "Ctrl+Up",
        description: "Zoom in on active value scale",
        action: Some(Action::Zoom {
            time: 1.0,
            price: 0.8,
            x: 0.5,
            y: 0.5,
        }),
    },
    Entry {
        keys: "Ctrl+Down",
        description: "Zoom out on active value scale",
        action: Some(Action::Zoom {
            time: 1.0,
            price: 1.25,
            x: 0.5,
            y: 0.5,
        }),
    },
    Entry {
        keys: "Ctrl+Alt+Up",
        description: "Make the active plot taller (when lower panes are visible)",
        action: Some(Action::AdjustPaneHeight {
            id: None,
            delta: 32.0,
        }),
    },
    Entry {
        keys: "Ctrl+Alt+Down",
        description: "Make the active plot shorter",
        action: Some(Action::AdjustPaneHeight {
            id: None,
            delta: -32.0,
        }),
    },
    Entry {
        keys: "Ctrl+Alt+0",
        description: "Restore the active plot's default height",
        action: Some(Action::ResetPaneHeight(None)),
    },
    Entry {
        keys: "Ctrl+Alt+C / Collapse / Expand",
        description: "Collapse or expand the active lower plot; keep values, feeds and scale",
        action: None,
    },
    Entry {
        keys: "Drag / double-click pane grip",
        description: "Resize the plot above the grip / restore its default height",
        action: None,
    },
    Entry {
        keys: "[ / ,",
        description: "Inspect previous candle",
        action: Some(Action::InspectStep(-1)),
    },
    Entry {
        keys: "] / .",
        description: "Inspect next candle",
        action: Some(Action::InspectStep(1)),
    },
    Entry {
        keys: "Home / R",
        description: "Reset time and auto-fit every scale; keep CVD origin",
        action: Some(Action::Reset),
    },
    Entry {
        keys: "F",
        description: "Restore auto-fit on active indicator / toggle price auto-fit",
        action: Some(Action::AutoFit),
    },
    Entry {
        keys: "End / L",
        description: "Return to realtime; preserve manual value scales",
        action: Some(Action::Live),
    },
    Entry {
        keys: "B",
        description: "Load older history",
        action: Some(Action::Older),
    },
    Entry {
        keys: "PgUp",
        description: "Scroll indicator panes up",
        action: Some(Action::ScrollPanes {
            amount: -280.0,
            legends: false,
        }),
    },
    Entry {
        keys: "PgDn",
        description: "Scroll indicator panes down",
        action: Some(Action::ScrollPanes {
            amount: 280.0,
            legends: false,
        }),
    },
    Entry {
        keys: "Shift+PgUp",
        description: "Scroll indicator legends up",
        action: Some(Action::ScrollPanes {
            amount: -280.0,
            legends: true,
        }),
    },
    Entry {
        keys: "Shift+PgDn",
        description: "Scroll indicator legends down",
        action: Some(Action::ScrollPanes {
            amount: 280.0,
            legends: true,
        }),
    },
    Entry {
        keys: "S / Ctrl+J",
        description: "Status details / active lower indicator details",
        action: None,
    },
    Entry {
        keys: "Y",
        description: "Retry history",
        action: Some(Action::Retry),
    },
    Entry {
        keys: "I",
        description: "Open indicator editor",
        action: Some(Action::ManageIndicators),
    },
    Entry {
        keys: "M",
        description: "Open market selector",
        action: Some(Action::ManageMarket),
    },
    Entry {
        keys: "Ctrl+Left / Ctrl+Right (chart)",
        description: "Choose previous / next native interval; same exact market",
        action: None,
    },
    Entry {
        keys: "Ctrl+E / Ctrl+H / Ctrl+D (chart)",
        description: "Settings / hide / remove active lower indicator; Ctrl+Tab selects target",
        action: None,
    },
    Entry {
        keys: "Indicator title controls",
        description: "Settings / Hide / Remove for that exact overlay or pane; I manages hidden instances",
        action: None,
    },
    Entry {
        keys: "Right-click chart",
        description: "Clear chart inspection",
        action: Some(Action::Inspect(None)),
    },
    Entry {
        keys: "Drag price / lower plot",
        description: "Pan shared time and the plot's own value scale",
        action: None,
    },
    Entry {
        keys: "Drag bottom / right axis",
        description: "Zoom shared time / that plot's value scale",
        action: None,
    },
    Entry {
        keys: "Wheel / Shift-wheel (any plot)",
        description: "Zoom shared time / that plot's value scale",
        action: None,
    },
    Entry {
        keys: "Ctrl-wheel (any plot)",
        description: "Zoom shared time at the pointer",
        action: None,
    },
    Entry {
        keys: "Wheel bottom / right axis",
        description: "Zoom shared time / that plot's value scale",
        action: None,
    },
    Entry {
        keys: "Double-click right axis",
        description: "Restore that plot's auto-fit",
        action: None,
    },
    Entry {
        keys: "Hover / interact with a plot",
        description: "Target its value scale for keyboard controls and F",
        action: None,
    },
    Entry {
        keys: "Ctrl+Tab / Ctrl+Shift+Tab",
        description: "Cycle value-scale target: price and visible indicator panes",
        action: None,
    },
    Entry {
        keys: "Click indicator legend",
        description: "Target that plot's value scale for keyboard controls",
        action: None,
    },
    Entry {
        keys: "Alt+wheel",
        description: "Scroll the pane stack over plots; axes keep their scale gestures",
        action: None,
    },
    Entry {
        keys: "Trackpad / Ctrl-scroll",
        description: "Any plot: pixel scrolling pans shared time; Ctrl zooms time, Shift zooms values",
        action: None,
    },
    Entry {
        keys: "Ctrl+G",
        description: "Open UTC date and time jump",
        action: None,
    },
    Entry {
        keys: "Ctrl+Alt+Shift+Up / Down",
        description: "Move active lower pane in the visible pane order",
        action: None,
    },
    Entry {
        keys: "Ctrl+Alt+M",
        description: "Maximize or restore the active pane",
        action: None,
    },
    Entry {
        keys: "8",
        description: "Select the permanent two-anchor measurement drawing tool",
        action: None,
    },
];

static DRAWINGS: &[Entry] = &[
    Entry {
        keys: "",
        description: "Price-chart drawings · automatically saved · shared native-instrument timeframes",
        action: None,
    },
    Entry {
        keys: "Ctrl+Shift+D / Draw",
        description: "Open drawing tools, objects and properties",
        action: None,
    },
    Entry {
        keys: "0–9 in Draw menu",
        description: "Select / horizontal / vertical / trend / ray / rectangle / Fibonacci / text / measurement / anchored VWAP",
        action: None,
    },
    Entry {
        keys: "Draw → 9 → click / Enter",
        description: "Anchor candle-based VWAP at a native candle; move the start handle or edit UTC; normal drawing undo/redo applies",
        action: None,
    },
    Entry {
        keys: "Ctrl+P in Draw menu",
        description: "Selected Range Volume Profile: place two time anchors; saved as a native-bound profile",
        action: None,
    },
    Entry {
        keys: "Profile range rail / Draw → PROFILE RANGES",
        description: "Select a fixed profile; drag endpoint lines to resize or the rail to translate; Esc cancels drag",
        action: None,
    },
    Entry {
        keys: "Ctrl+E / Ctrl+H / Delete (selected profile)",
        description: "Existing profile settings / show-hide / delete; profile edits are separate from drawing-book undo",
        action: None,
    },
    Entry {
        keys: "Click anchors / arrows + Enter",
        description: "Place each anchor; Repeat keeps the tool armed, otherwise return to Select",
        action: None,
    },
    Entry {
        keys: "Ctrl+Shift+R (Draw menu or active price drawing)",
        description: "Toggle repeated tool creation; Escape returns to Select and disables Repeat",
        action: None,
    },
    Entry {
        keys: "Drag body / handles",
        description: "Move selected drawing or resize endpoints/corners; one undo step on release",
        action: None,
    },
    Entry {
        keys: "Space + drag",
        description: "Pan the chart over drawings or while a drawing tool is active",
        action: None,
    },
    Entry {
        keys: "Arrows / Shift+arrows (selected price drawing)",
        description: "Nudge selected drawing; Shift makes larger steps. With no selection, chart navigation stays unchanged",
        action: None,
    },
    Entry {
        keys: "Ctrl+E / Properties",
        description: "Edit selected price drawing's UTC/native-price anchors and appearance",
        action: None,
    },
    Entry {
        keys: "Delete / Ctrl+Z / Ctrl+Shift+Z",
        description: "Delete selection / undo / redo in this native instrument's drawing book",
        action: None,
    },
    Entry {
        keys: "Esc / Select-clear",
        description: "Cancel draft/drag, then clear selection; committed drawings remain unchanged",
        action: None,
    },
    Entry {
        keys: "Up/Down, Enter (Draw objects)",
        description: "Choose an object, then edit it; hidden/locked/offscreen objects remain accessible",
        action: None,
    },
    Entry {
        keys: "Ctrl+H / Ctrl+L / Ctrl+M (Draw menu)",
        description: "Show-hide selected object / lock-unlock / toggle actual-OHLC magnet",
        action: None,
    },
    Entry {
        keys: "Ctrl+Enter (drawing properties)",
        description: "Apply exact UTC/price, text, style or Fibonacci edits; Escape cancels unapplied edits",
        action: None,
    },
    Entry {
        keys: "Ctrl+Alt+K / P (selected drawing or properties)",
        description: "Cycle semantic ink / stroke pattern; locked committed drawings reject styling",
        action: None,
    },
    Entry {
        keys: "Ctrl+Alt+[ / ] (selected drawing)",
        description: "Decrease / increase line width without opening properties",
        action: None,
    },
    Entry {
        keys: "Ctrl+Alt+R / E (Fibonacci properties)",
        description: "Reverse retracement direction / toggle right extension",
        action: None,
    },
];

static INDICATORS: &[Entry] = &[
    Entry {
        keys: "",
        description: "Indicator editor (when open)",
        action: None,
    },
    Entry {
        keys: "Ctrl+J",
        description: "Open details for the selected exact instance, including overlays and hidden instances",
        action: None,
    },
    Entry {
        keys: "Ctrl+N / N*",
        description: "New instance (N outside fields)",
        action: None,
    },
    Entry {
        keys: "Ctrl+S / Enter",
        description: "Apply values / add instance",
        action: None,
    },
    Entry {
        keys: "Ctrl+H / Space*",
        description: "Show / hide selected (Space outside fields)",
        action: None,
    },
    Entry {
        keys: "Ctrl+D / Delete*",
        description: "Remove selected (Delete outside fields)",
        action: None,
    },
    Entry {
        keys: "Ctrl+Shift+R",
        description: "CVD: cycle Continuous / UTC day; VWAP: UTC day/week/month/custom session",
        action: None,
    },
    Entry {
        keys: "Ctrl+Left / Ctrl+Right",
        description: "Change indicator kind",
        action: None,
    },
    Entry {
        keys: "Indicator chooser / Ctrl+T / Ctrl+V outside text inputs",
        description: "Add TPO / Visible Range Volume Profile through existing native profile settings; normal parameter paste stays unchanged",
        action: None,
    },
    Entry {
        keys: "Ctrl+Up / Ctrl+Down",
        description: "Select existing instance",
        action: None,
    },
    Entry {
        keys: "PageUp / PageDown",
        description: "Scroll indicator parameters and source choices",
        action: None,
    },
    Entry {
        keys: "Ctrl+Shift+M",
        description: "CVD / OI: cycle Chart, Single, Selected, All source mode",
        action: None,
    },
    Entry {
        keys: "Ctrl+Shift+Left / Right",
        description: "Browse exchange checklist without choosing a market",
        action: None,
    },
    Entry {
        keys: "Ctrl+Shift+Up / Down",
        description: "Highlight exact native source market",
        action: None,
    },
    Entry {
        keys: "Ctrl+Shift+Space",
        description: "Select / unselect highlighted native market",
        action: None,
    },
    Entry {
        keys: "Ctrl+R",
        description: "Retry failed source catalog discovery",
        action: None,
    },
    Entry {
        keys: "Left / Right*",
        description: "Change kind outside fields",
        action: None,
    },
    Entry {
        keys: "Up / Down*",
        description: "Select instance outside fields",
        action: None,
    },
    Entry {
        keys: "Tab / Shift+Tab",
        description: "Move between parameter fields and controls",
        action: None,
    },
    Entry {
        keys: "Esc / I*",
        description: "Close editor (I outside fields)",
        action: None,
    },
    Entry {
        keys: "Type / caret keys",
        description: "Edit parameter fields; no chart navigation",
        action: None,
    },
];

static MARKET: &[Entry] = &[
    Entry {
        keys: "",
        description: "Market selector (when open)",
        action: None,
    },
    Entry {
        keys: "Type",
        description: "Search native symbols or base coins",
        action: None,
    },
    Entry {
        keys: "Up / Down",
        description: "Select a matching native market",
        action: None,
    },
    Entry {
        keys: "Alt+Left / Alt+Right",
        description: "Change exchange",
        action: None,
    },
    Entry {
        keys: "Alt+Up / Alt+Down",
        description: "Choose spot / perpetual",
        action: None,
    },
    Entry {
        keys: "Ctrl+Q",
        description: "Cycle quote currency (unlisted quotes remain unavailable)",
        action: None,
    },
    Entry {
        keys: "Ctrl+Left / Ctrl+Right",
        description: "Change native interval (queued while the catalog loads)",
        action: None,
    },
    Entry {
        keys: "Enter / Ctrl+Enter",
        description: "Apply selected market",
        action: None,
    },
    Entry {
        keys: "Ctrl+R",
        description: "Retry scoped market catalog",
        action: None,
    },
    Entry {
        keys: "Ctrl+F",
        description: "Toggle the selected exact market favorite for this session",
        action: None,
    },
    Entry {
        keys: "Ctrl+Shift+Up / Down",
        description: "Choose an exact recent/favorite market and revalidate its native catalog",
        action: None,
    },
    Entry {
        keys: "Tab / Shift+Tab",
        description: "Move through selector controls",
        action: None,
    },
    Entry {
        keys: "Esc",
        description: "Close selector",
        action: None,
    },
];

static DETAILS: &[Entry] = &[
    Entry {
        keys: "",
        description: "Status and indicator details",
        action: None,
    },
    Entry {
        keys: "Esc",
        description: "Close details without changing the chart",
        action: None,
    },
    Entry {
        keys: "Up / Down / PageUp / PageDown",
        description: "Scroll details",
        action: None,
    },
    Entry {
        keys: "Tab / Shift+Tab",
        description: "Move between details controls",
        action: None,
    },
    Entry {
        keys: "Ctrl+E / Ctrl+R",
        description: "Indicator details: settings / retry selected sources",
        action: None,
    },
    Entry {
        keys: "Ctrl+Shift+C",
        description: "Indicator details: explicitly switch this instance to Chart source",
        action: None,
    },
    Entry {
        keys: "Y",
        description: "Status details: retry connection, history and theme",
        action: None,
    },
];

static PROFILES: &[Entry] = &[
    Entry {
        keys: "",
        description: "TPO / Volume Profiles",
        action: None,
    },
    Entry {
        keys: "Ctrl+K → profile",
        description: "Add TPO, visible or selected-range Volume Profile; configure and manage profile IDs",
        action: None,
    },
    Entry {
        keys: "Profile settings",
        description: "Session / fixed / visible range, ticks per row, value area and display layers",
        action: None,
    },
    Entry {
        keys: "Ctrl+N / Ctrl+S",
        description: "Profile editor: new draft / apply exact selected profile",
        action: None,
    },
    Entry {
        keys: "Ctrl+Left/Right / Ctrl+Up/Down",
        description: "Profile editor: cycle type / select exact existing instance",
        action: None,
    },
    Entry {
        keys: "Alt+Left/Right / Alt+Up/Down",
        description: "Profile editor: cycle range mode / calendar or custom session",
        action: None,
    },
    Entry {
        keys: "Ctrl+1..8",
        description: "Toggle histogram, levels, previous, naked, Single Prints, IB, letters, poor extremes",
        action: None,
    },
    Entry {
        keys: "Ctrl+9 (Volume editor)",
        description: "Toggle automatic candle-volume fallback; apply with Ctrl+S. Estimated/Mixed never means proven execution coverage",
        action: None,
    },
    Entry {
        keys: "Ctrl+P",
        description: "Profile editor: place fixed range with two clicks or keyboard anchors",
        action: None,
    },
    Entry {
        keys: "Enter / Left / Right",
        description: "Range tool: confirm inspected time / move the keyboard time anchor",
        action: None,
    },
    Entry {
        keys: "Esc",
        description: "Cancel profile range placement; held drawing/chart gestures keep their owner",
        action: None,
    },
    Entry {
        keys: "Ctrl+H / Ctrl+D",
        description: "Profile editor: show/hide / remove only the selected profile ID",
        action: None,
    },
    Entry {
        keys: "Ctrl+R",
        description: "Profile editor: explicitly rebin from the available native price reference",
        action: None,
    },
    Entry {
        keys: "Ctrl+Shift+R",
        description: "Profile editor: retry native history without clearing observed execution data",
        action: None,
    },
    Entry {
        keys: "Tab / Shift+Tab",
        description: "Profile editor: move focus without leaking shortcuts into chart navigation",
        action: None,
    },
    Entry {
        keys: "PageUp / PageDown",
        description: "Profile editor: scroll settings and native coverage/provenance details",
        action: None,
    },
    Entry {
        keys: "Passive graphics",
        description: "Profiles do not change value scales; only fixed-range rails/selected handles claim input; Space-drag pans",
        action: None,
    },
];

/// Registry entry exposed to the command palette so keyboard-help commands stay
/// the single source of truth for chart navigation.
#[derive(Debug, Clone)]
pub struct CommandEntry {
    pub keys: &'static str,
    pub description: &'static str,
    pub action: Action,
}

pub fn command_entries(context: Context) -> impl Iterator<Item = CommandEntry> {
    entries(context).filter_map(move |entry| {
        enabled_action(context, entry).map(|action| CommandEntry {
            keys: entry.keys,
            description: entry.description,
            action: action.clone(),
        })
    })
}

fn groups(context: Context) -> [&'static [Entry]; 5] {
    match context {
        Context::Chart => [CHART, DRAWINGS, INDICATORS, PROFILES, MARKET],
        Context::Indicators => [INDICATORS, PROFILES, MARKET, CHART, DRAWINGS],
        Context::Market => [MARKET, INDICATORS, PROFILES, CHART, DRAWINGS],
        Context::Details => [DETAILS, PROFILES, CHART, DRAWINGS, &[]],
        Context::Drawings => [DRAWINGS, CHART, INDICATORS, PROFILES, MARKET],
    }
}

fn entries(context: Context) -> impl Iterator<Item = &'static Entry> {
    std::iter::once(&GLOBAL_HELP).chain(groups(context).into_iter().flat_map(|group| group.iter()))
}

fn enabled_action(context: Context, entry: &Entry) -> Option<&Action> {
    entry.action.as_ref().filter(|action| {
        matches!(context, Context::Chart)
            || matches!(action, Action::ManageIndicators | Action::ManageMarket)
    })
}

pub fn view(context: Context, selected: usize) -> Element<'static, Message> {
    let mut choice = 1;
    let mut list = column![].spacing(3);
    for entry in entries(context) {
        if entry.keys.is_empty() {
            list = list.push(ui::separator()).push(
                text(entry.description)
                    .size(11)
                    .style(ui::muted_text)
                    .width(Length::Fill),
            );
            continue;
        }
        let content = row![
            container(
                text(entry.keys)
                    .size(10)
                    .font(iced::Font::MONOSPACE)
                    .width(Length::Fill)
            )
            .width(Length::Fixed(142.0))
            .padding([2, 4])
            .style(ui::card),
            text(entry.description).size(12).width(Length::Fill),
        ]
        .spacing(12)
        .align_y(Alignment::Center);
        let line: Element<'static, Message> = match enabled_action(context, entry) {
            Some(action) => {
                let style = if selected == choice {
                    ui::selected
                } else {
                    ui::quiet
                };
                choice += 1;
                button(content)
                    .on_press(Message::HelpAction(action.clone()))
                    .style(style)
                    .padding([4, 7])
                    .width(Length::Fill)
                    .into()
            }
            None => container(content)
                .padding([4, 7])
                .width(Length::Fill)
                .into(),
        };
        list = list.push(line);
    }
    let close_style = if selected == 0 {
        ui::selected
    } else {
        ui::quiet
    };
    let panel = container(
        column![
            row![
                column![
                    text(context_title(context)).size(18),
                    text("Keyboard & mouse actions")
                        .size(11)
                        .style(ui::muted_text),
                ]
                .spacing(2),
                iced::widget::Space::new(Length::Fill, Length::Shrink),
                button("Commands · Ctrl+K")
                    .on_press(Message::CommandOpen)
                    .style(ui::quiet)
                    .padding([5, 9]),
                button("Close [Esc]")
                    .on_press(Message::HelpClose)
                    .style(close_style)
                    .padding([5, 9]),
            ]
            .align_y(Alignment::Center),
            row![
                button("Clear current chart")
                    .on_press(Message::Workspace(crate::workspace_controls::Event::Open(
                        ResetKind::Current
                    )))
                    .style(ui::danger)
                    .padding([5, 9]),
                button("Reset everything")
                    .on_press(Message::Workspace(crate::workspace_controls::Event::Open(
                        ResetKind::All
                    )))
                    .style(ui::danger)
                    .padding([5, 9]),
            ]
            .spacing(8),
            text("Tab/Shift+Tab: select · Enter/Space: activate · Up/Down/PageUp/PageDown: scroll")
                .size(11)
                .style(ui::muted_text),
            text(if matches!(context, Context::Chart) {
                "Click chart-action rows to run them. Value controls target the last active plot."
            } else {
                "Chart actions are disabled behind dialogs. M / I rows switch editors."
            })
            .size(11)
            .style(ui::muted_text),
            scrollable(container(list).padding(iced::Padding {
                right: 16.0,
                ..iced::Padding::default()
            }))
            .id(scrollable::Id::new(LIST_SCROLL_ID))
            .height(Length::Fill),
        ]
        .spacing(8),
    )
    .padding(14)
    .width(Length::Fill)
    .max_width(590)
    .height(Length::Fill)
    .max_height(650)
    .style(ui::panel);
    let backdrop = scrollable(
        mouse_area(
            container(iced::widget::Space::new(
                Length::Fill,
                Length::Fixed(1600.0),
            ))
            .style(|_| container::Style {
                background: Some(iced::Color::from_rgba(0.0, 0.0, 0.0, 0.55).into()),
                ..container::Style::default()
            }),
        )
        .on_press(Message::HelpClose)
        .on_move(|_| Message::HelpBackdrop),
    )
    .width(Length::Fill)
    .height(Length::Fill);
    iced::widget::stack![
        backdrop,
        container(iced::widget::opaque(panel))
            .padding(16)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Alignment::Center)
            .align_y(Alignment::Center),
    ]
    .into()
}

fn context_title(context: Context) -> &'static str {
    match context {
        Context::Chart => "Chart",
        Context::Drawings => "Drawings",
        Context::Indicators => "Indicators",
        Context::Market => "Market",
        Context::Details => "Details",
    }
}

/// Keep background widget state mounted, but block input and existing dropdown
/// overlays while a foreground dialog owns the window.
pub fn modal_background<'a>(content: Element<'a, Message>, blocked: bool) -> Element<'a, Message> {
    Element::new(ModalBackground {
        content,
        blocked,
        profile_shortcuts: false,
    })
}

/// Numbered profile commands belong to the editor, not its focused text input.
/// iced 0.13 inserts KeyPressed.text for Ctrl+digits; the global command listener
/// still receives the captured key and applies the option exactly once.
pub fn profile_shortcuts<'a>(content: Element<'a, Message>) -> Element<'a, Message> {
    Element::new(ModalBackground {
        content,
        blocked: false,
        profile_shortcuts: true,
    })
}

struct ModalBackground<'a> {
    content: Element<'a, Message>,
    blocked: bool,
    profile_shortcuts: bool,
}

impl Widget<Message, Theme, Renderer> for ModalBackground<'_> {
    fn tag(&self) -> tree::Tag {
        self.content.as_widget().tag()
    }

    fn state(&self) -> tree::State {
        self.content.as_widget().state()
    }

    fn children(&self) -> Vec<Tree> {
        self.content.as_widget().children()
    }

    fn diff(&self, tree: &mut Tree) {
        self.content.as_widget().diff(tree);
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn size_hint(&self) -> Size<Length> {
        self.content.as_widget().size_hint()
    }

    fn layout(
        &self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.content.as_widget().layout(tree, renderer, limits)
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.content
            .as_widget()
            .draw(tree, renderer, theme, style, layout, cursor, viewport);
    }

    fn operate(
        &self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        self.content
            .as_widget()
            .operate(tree, layout, renderer, operation);
    }

    fn on_event(
        &mut self,
        tree: &mut Tree,
        event: Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) -> event::Status {
        if self.blocked && !matches!(event, Event::Window(_)) {
            return event::Status::Captured;
        }
        if self.profile_shortcuts
            && let Event::Keyboard(iced::keyboard::Event::KeyPressed { key, modifiers, .. }) =
                &event
            && modifiers.control()
            && !modifiers.alt()
            && !modifiers.logo()
            && matches!(
                key.as_ref(),
                iced::keyboard::Key::Character("1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
            )
        {
            return event::Status::Captured;
        }
        self.content.as_widget_mut().on_event(
            tree, event, layout, cursor, renderer, clipboard, shell, viewport,
        )
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        if self.blocked {
            mouse::Interaction::None
        } else {
            self.content
                .as_widget()
                .mouse_interaction(tree, layout, cursor, viewport, renderer)
        }
    }

    fn overlay<'a>(
        &'a mut self,
        tree: &'a mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        translation: Vector,
    ) -> Option<iced::advanced::overlay::Element<'a, Message, Theme, Renderer>> {
        if self.blocked {
            None
        } else {
            self.content
                .as_widget_mut()
                .overlay(tree, layout, renderer, translation)
        }
    }
}

pub fn action_count(context: Context) -> usize {
    entries(context)
        .filter(|entry| enabled_action(context, entry).is_some())
        .count()
}
pub fn action_at(context: Context, index: usize) -> Option<Action> {
    entries(context)
        .filter_map(|entry| enabled_action(context, entry))
        .nth(index)
        .cloned()
}

pub fn action_scroll_offset(context: Context, index: usize) -> Option<f32> {
    let mut action_index = 0;
    let mut offset = 0.0;
    for entry in entries(context) {
        if enabled_action(context, entry).is_some() {
            if action_index == index {
                return Some(offset);
            }
            action_index += 1;
        }
        let row_height = if entry.keys.is_empty() {
            20.0
        } else {
            let key_lines = entry.keys.len().div_ceil(22).max(1) as f32;
            let description_lines = entry.description.len().div_ceil(42).max(1) as f32;
            (key_lines * 12.0).max(description_lines * 14.0) + 16.0
        };
        offset += row_height + 3.0;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editor_help_actions_cannot_dispatch_chart_navigation() {
        for context in [Context::Market, Context::Indicators, Context::Drawings] {
            for index in 0..action_count(context) {
                assert!(matches!(
                    action_at(context, index),
                    Some(Action::ManageMarket | Action::ManageIndicators)
                ));
            }
            assert!(action_at(context, action_count(context)).is_none());
        }
    }
}
