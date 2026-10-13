use crate::{Message, state::Action};
use iced::advanced::{
    Clipboard, Layout, Shell, Widget, layout, renderer,
    widget::{Operation, Tree, tree},
};
use iced::widget::{button, column, container, mouse_area, row, scrollable, text};
use iced::{Alignment, Element, Length};
use iced::{Event, Rectangle, Renderer, Size, Theme, Vector, event, mouse};

pub const LIST_SCROLL_ID: &str = "keyboard-help-list";
const ENTRY_HEIGHT: f32 = 64.0;

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
    keys: "F1 (all) / ? (chart)",
    description: "Open this help",
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
];

static DRAWINGS: &[Entry] = &[
    Entry {
        keys: "",
        description: "Price-chart drawings · session only · shared native-instrument timeframes",
        action: None,
    },
    Entry {
        keys: "Ctrl+Shift+D / Draw",
        description: "Open drawing tools, objects and properties",
        action: None,
    },
    Entry {
        keys: "0–7 in Draw menu",
        description: "Select / horizontal / vertical / trend / ray / rectangle / Fibonacci / text",
        action: None,
    },
    Entry {
        keys: "Click anchors / arrows + Enter",
        description: "Place each anchor; complete the drawing then return to Select",
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
        keys: "Ctrl+Alt+K / P (drawing properties)",
        description: "Cycle semantic ink / stroke pattern without using the mouse",
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
        description: "CVD: cycle Continuous / UTC day reset",
        action: None,
    },
    Entry {
        keys: "Ctrl+Left / Ctrl+Right",
        description: "Change indicator kind",
        action: None,
    },
    Entry {
        keys: "Ctrl+Up / Ctrl+Down",
        description: "Select existing instance",
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

fn groups(context: Context) -> [&'static [Entry]; 4] {
    match context {
        Context::Chart => [CHART, DRAWINGS, INDICATORS, MARKET],
        Context::Indicators => [INDICATORS, MARKET, CHART, DRAWINGS],
        Context::Market => [MARKET, INDICATORS, CHART, DRAWINGS],
        Context::Details => [DETAILS, CHART, DRAWINGS, &[]],
        Context::Drawings => [DRAWINGS, CHART, INDICATORS, MARKET],
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
    let mut list = column![].spacing(5);
    for entry in entries(context) {
        if entry.keys.is_empty() {
            list =
                list.push(container(text(entry.description).size(15)).height(Length::Fixed(28.0)));
            continue;
        }
        let label =
            column![text(entry.keys).size(13), text(entry.description).size(12),].spacing(3);
        let line: Element<'static, Message> = match enabled_action(context, entry) {
            Some(action) => {
                let style = if selected == choice {
                    button::primary
                } else {
                    button::secondary
                };
                choice += 1;
                button(label)
                    .on_press(Message::HelpAction(action.clone()))
                    .style(style)
                    .width(Length::Fill)
                    .height(Length::Fixed(ENTRY_HEIGHT))
                    .into()
            }
            None => container(label)
                .padding([4, 8])
                .height(Length::Fixed(ENTRY_HEIGHT))
                .width(Length::Fill)
                .into(),
        };
        list = list.push(line);
    }
    let close_style = if selected == 0 {
        button::primary
    } else {
        button::secondary
    };
    let panel = container(
        column![
            row![
                text("Keyboard & mouse actions").size(20),
                iced::widget::Space::new(Length::Fill, Length::Shrink),
                button("Close [Esc]")
                    .on_press(Message::HelpClose)
                    .style(close_style),
            ]
            .align_y(Alignment::Center),
            text("Tab/Shift+Tab: select action · Enter/Space: activate · Up/Down/PageUp/PageDown: scroll")
                .size(12),
            text(if matches!(context, Context::Chart) {
                "Click chart-action rows to run them. Value controls target the last active plot."
            } else {
                "Chart actions are disabled behind dialogs. M / I rows switch editors."
            })
                .size(12),
            scrollable(list)
                .id(scrollable::Id::new(LIST_SCROLL_ID))
                .height(Length::Fill),
        ]
        .spacing(10),
    )
    .padding(14)
    .width(Length::Fill)
    .max_width(590)
    .height(Length::Fill)
    .max_height(650)
    .style(container::rounded_box);
    let backdrop = scrollable(
        mouse_area(container(iced::widget::Space::new(
            Length::Fill,
            Length::Fixed(1600.0),
        )))
        .on_press(Message::HelpClose)
        .on_move(|_| Message::HelpBackdrop),
    )
    .width(Length::Fill)
    .height(Length::Fill);
    iced::widget::stack![
        backdrop,
        container(panel)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Alignment::Center)
            .align_y(Alignment::Center),
    ]
    .into()
}

/// Keep background widget state mounted, but block input and existing dropdown
/// overlays while a foreground dialog owns the window.
pub fn modal_background<'a>(content: Element<'a, Message>, blocked: bool) -> Element<'a, Message> {
    Element::new(ModalBackground { content, blocked })
}

struct ModalBackground<'a> {
    content: Element<'a, Message>,
    blocked: bool,
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
        offset += (if entry.keys.is_empty() {
            28.0
        } else {
            ENTRY_HEIGHT
        }) + 5.0;
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
