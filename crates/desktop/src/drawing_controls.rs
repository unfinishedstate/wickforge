use crate::{
    Message,
    drawing_types::{
        Anchor, Command, Drawing, DrawingId, FIB_LEVELS, Ink, Pattern, Shape, Style, Tool,
    },
    drawings::Store,
    drawings::{validate_shape, validate_style},
    palette::Palette,
};
use chrono::{DateTime, SecondsFormat, Utc};
use iced::widget::{
    button, checkbox, column, container, pick_list, row, scrollable, text, text_input,
};
use iced::{Element, Length, keyboard};

pub const BODY_ID: &str = "drawing-controls-body";

#[derive(Debug, Clone)]
pub enum Event {
    ChooseTool(Tool),
    SelectedVolumeProfile,
    Select(DrawingId),
    Properties(DrawingId),
    Field(Field, String),
    Ink(Ink),
    Pattern(Pattern),
    QuickInk(Ink),
    QuickPattern(Pattern),
    QuickWidth(f32),
    Repeat(bool),
    FibReverse(bool),
    FibExtendRight(bool),
    Apply,
    Cancel,
    Navigate(i32),
    Hide(DrawingId),
    Lock(DrawingId),
    Delete(DrawingId),
    Undo,
    Redo,
    Magnet(bool),
    ToggleMagnet,
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    UtcFirst,
    PriceFirst,
    UtcSecond,
    PriceSecond,
    Text,
    Width,
    FontSize,
    FillOpacity,
    FibLevels,
}

#[derive(Debug, Clone)]
pub enum Effect {
    None,
    ChooseTool(Tool),
    SelectedVolumeProfile,
    Select(Option<DrawingId>),
    Magnet(bool),
    ToggleMagnet,
    Repeat(bool),
    Commit(Command),
    Close,
}

#[derive(Debug, Clone)]
enum DraftKind {
    NewText,
    Edit(DrawingId),
}

#[derive(Debug, Clone)]
struct Draft {
    kind: DraftKind,
    shape: Shape,
    style: Style,
    visible: bool,
    locked: bool,
    utc_first: String,
    price_first: String,
    utc_second: String,
    price_second: String,
    text: String,
    width: String,
    font_size: String,
    fill_opacity: String,
    levels: String,
    error: Option<String>,
}

impl Draft {
    fn new_text(anchor: Anchor) -> Self {
        Self::from_shape(
            DraftKind::NewText,
            Shape::Text {
                at: anchor,
                text: String::new(),
            },
            Style::default(),
            true,
            false,
        )
    }

    fn from_drawing(drawing: &Drawing) -> Self {
        Self::from_shape(
            DraftKind::Edit(drawing.id),
            drawing.shape.clone(),
            drawing.style,
            drawing.visible,
            drawing.locked,
        )
    }

    fn from_shape(
        kind: DraftKind,
        shape: Shape,
        style: Style,
        visible: bool,
        locked: bool,
    ) -> Self {
        let anchors = shape.anchors();
        let first = anchors.first();
        let second = anchors.second().unwrap_or(first);
        let text = match &shape {
            Shape::Text { text, .. } => text.clone(),
            _ => String::new(),
        };
        let levels = match &shape {
            Shape::Fibonacci { levels, .. } => levels
                .iter()
                .map(f64::to_string)
                .collect::<Vec<_>>()
                .join(", "),
            _ => FIB_LEVELS
                .iter()
                .map(f64::to_string)
                .collect::<Vec<_>>()
                .join(", "),
        };
        Self {
            kind,
            shape,
            style,
            visible,
            locked,
            utc_first: utc_field(first.utc_ms),
            price_first: first.price.to_string(),
            utc_second: utc_field(second.utc_ms),
            price_second: second.price.to_string(),
            text,
            width: style.width.to_string(),
            font_size: style.font_size.to_string(),
            fill_opacity: style.fill_opacity.to_string(),
            levels,
            error: None,
        }
    }

    fn field(&mut self, field: Field, value: String) {
        match field {
            Field::UtcFirst => self.utc_first = value,
            Field::PriceFirst => self.price_first = value,
            Field::UtcSecond => self.utc_second = value,
            Field::PriceSecond => self.price_second = value,
            Field::Text => self.text = value,
            Field::Width => self.width = value,
            Field::FontSize => self.font_size = value,
            Field::FillOpacity => self.fill_opacity = value,
            Field::FibLevels => self.levels = value,
        }
        self.error = None;
    }

    fn anchor(utc: &str, price: &str, name: &str) -> Result<Anchor, String> {
        let utc_text = utc.trim();
        let utc_ms = utc_text
            .parse::<i64>()
            .ok()
            .or_else(|| {
                DateTime::parse_from_rfc3339(utc_text)
                    .ok()
                    .map(|date| date.timestamp_millis())
            })
            .ok_or_else(|| format!("{name} UTC must be ISO-8601 UTC or integer milliseconds"))?;
        let price = price
            .trim()
            .parse::<f64>()
            .map_err(|_| format!("{name} price must be a number"))?;
        Ok(Anchor { utc_ms, price })
    }

    fn shape_and_style(&self) -> Result<(Shape, Style), String> {
        let anchored = matches!(self.shape, Shape::AnchoredVwap(_));
        let first = Self::anchor(
            &self.utc_first,
            if anchored { "0" } else { &self.price_first },
            "First",
        )?;
        let second = if anchored {
            first
        } else {
            Self::anchor(&self.utc_second, &self.price_second, "Second")?
        };
        let shape = match &self.shape {
            Shape::Horizontal(_) => Shape::Horizontal(first),
            Shape::Vertical(_) => Shape::Vertical(first),
            Shape::AnchoredVwap(_) => Shape::AnchoredVwap(Anchor {
                price: 0.0,
                ..first
            }),
            Shape::Trend(_, _) => Shape::Trend(first, second),
            Shape::Ray(_, _) => Shape::Ray(first, second),
            Shape::Rectangle(_, _) => Shape::Rectangle(first, second),
            Shape::Measure(_, _) => Shape::Measure(first, second),
            Shape::Fibonacci {
                reverse,
                extend_right,
                ..
            } => {
                let levels = self
                    .levels
                    .split(',')
                    .map(|v| {
                        v.trim().parse::<f64>().map_err(|_| {
                            "Fibonacci levels must be comma-separated numbers".to_string()
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Shape::Fibonacci {
                    a: first,
                    b: second,
                    levels,
                    reverse: *reverse,
                    extend_right: *extend_right,
                }
            }
            Shape::Text { .. } => Shape::Text {
                at: first,
                text: self.text.clone(),
            },
        };
        let style = Style {
            ink: self.style.ink,
            pattern: self.style.pattern,
            width: self
                .width
                .trim()
                .parse::<f32>()
                .map_err(|_| "Width must be a number".to_string())?,
            fill_opacity: self
                .fill_opacity
                .trim()
                .parse::<f32>()
                .map_err(|_| "Rectangle fill opacity must be a number".to_string())?,
            font_size: self
                .font_size
                .trim()
                .parse::<f32>()
                .map_err(|_| "Text size must be a number".to_string())?,
        };
        validate_shape(&shape)?;
        validate_style(&style)?;
        Ok((shape, style))
    }
}

#[derive(Debug, Clone, Default)]
pub struct Controls {
    open: bool,
    generation: u64,
    draft: Option<Draft>,
    error: Option<String>,
}

impl Controls {
    pub fn is_open(&self) -> bool {
        self.open
    }
    pub fn is_editing(&self) -> bool {
        self.draft.is_some()
    }
    pub fn is_new_text(&self) -> bool {
        matches!(
            self.draft.as_ref().map(|draft| &draft.kind),
            Some(DraftKind::NewText)
        )
    }
    pub fn context_generation(&self) -> u64 {
        self.generation
    }
    pub fn focus_id(&self) -> Option<text_input::Id> {
        let draft = self.draft.as_ref()?;
        let field = match &draft.shape {
            Shape::Text { .. } => Field::Text,
            Shape::Vertical(_) | Shape::AnchoredVwap(_) => Field::UtcFirst,
            Shape::Horizontal(_)
            | Shape::Trend(_, _)
            | Shape::Ray(_, _)
            | Shape::Measure(_, _)
            | Shape::Rectangle(_, _)
            | Shape::Fibonacci { .. } => Field::PriceFirst,
        };
        Some(field_id(field))
    }

    pub fn open_menu(&mut self, store: &Store) {
        self.open = true;
        self.generation = store.generation();
        self.draft = None;
        self.error = None;
    }

    pub fn open_properties(&mut self, store: &Store, id: DrawingId) -> bool {
        let Some(drawing) = store.find(id) else {
            return false;
        };
        self.open = true;
        self.generation = store.generation();
        self.draft = Some(Draft::from_drawing(drawing));
        self.error = None;
        true
    }

    pub fn open_text(&mut self, store: &Store, anchor: Anchor) {
        self.open = true;
        self.generation = store.generation();
        self.draft = Some(Draft::new_text(anchor));
        self.error = None;
    }

    pub fn close(&mut self) {
        self.open = false;
        self.draft = None;
        self.error = None;
    }

    pub fn set_error(&mut self, error: String) {
        self.error = Some(error);
    }

    pub fn update(&mut self, event: Event, store: &Store) -> Effect {
        if self.generation != store.generation()
            && !matches!(
                &event,
                Event::QuickInk(_)
                    | Event::QuickPattern(_)
                    | Event::QuickWidth(_)
                    | Event::Repeat(_)
            )
        {
            return Effect::None;
        }
        if !self.open
            && !matches!(
                &event,
                Event::QuickInk(_)
                    | Event::QuickPattern(_)
                    | Event::QuickWidth(_)
                    | Event::Repeat(_)
            )
        {
            return Effect::None;
        }
        match event {
            Event::QuickInk(ink) => style_effect(store, |style| style.ink = ink),
            Event::QuickPattern(pattern) => style_effect(store, |style| style.pattern = pattern),
            Event::QuickWidth(width) => style_effect(store, |style| style.width = width),
            Event::Repeat(repeat) => Effect::Repeat(repeat),
            Event::ChooseTool(tool) => Effect::ChooseTool(tool),
            Event::SelectedVolumeProfile => Effect::SelectedVolumeProfile,
            Event::Select(id) => {
                if self.draft.is_none() && store.find(id).is_some() {
                    Effect::Select(Some(id))
                } else {
                    Effect::None
                }
            }
            Event::Navigate(direction) => {
                if self.draft.is_some() {
                    return Effect::None;
                }
                let objects = store.objects();
                if objects.is_empty() {
                    return Effect::None;
                }
                let next = match (
                    store
                        .selected()
                        .and_then(|id| objects.iter().position(|d| d.id == id)),
                    direction.signum(),
                ) {
                    (Some(index), 1) => (index + 1) % objects.len(),
                    (Some(index), -1) => (index + objects.len() - 1) % objects.len(),
                    (_, 1) => 0,
                    _ => objects.len() - 1,
                };
                Effect::Select(Some(objects[next].id))
            }
            Event::Properties(id) => {
                if self.open_properties(store, id) {
                    Effect::Select(Some(id))
                } else {
                    self.error = Some("Drawing is no longer available".into());
                    Effect::None
                }
            }
            Event::Field(field, value) => {
                if let Some(draft) = &mut self.draft {
                    draft.field(field, value);
                }
                Effect::None
            }
            Event::Ink(ink) => {
                if let Some(draft) = &mut self.draft {
                    draft.style.ink = ink;
                }
                Effect::None
            }
            Event::Pattern(pattern) => {
                if let Some(draft) = &mut self.draft {
                    draft.style.pattern = pattern;
                }
                Effect::None
            }
            Event::Apply => self.apply_draft(store),
            Event::Cancel => {
                if self.draft.is_some() {
                    self.draft = None;
                    self.error = None;
                    Effect::None
                } else {
                    self.close();
                    Effect::Close
                }
            }
            Event::Hide(id) => {
                if let Some(drawing) = store.find(id) {
                    Effect::Commit(Command::SetVisible {
                        id,
                        visible: !drawing.visible,
                    })
                } else {
                    Effect::None
                }
            }
            Event::Lock(id) => {
                if let Some(drawing) = store.find(id) {
                    Effect::Commit(Command::SetLocked {
                        id,
                        locked: !drawing.locked,
                    })
                } else {
                    Effect::None
                }
            }
            Event::Delete(id) => {
                if store.find(id).is_some() {
                    Effect::Commit(Command::Delete(id))
                } else {
                    Effect::None
                }
            }
            Event::Undo => {
                if self.draft.is_none() && store.can_undo() {
                    Effect::Commit(Command::Undo)
                } else {
                    Effect::None
                }
            }
            Event::Redo => {
                if self.draft.is_none() && store.can_redo() {
                    Effect::Commit(Command::Redo)
                } else {
                    Effect::None
                }
            }
            Event::FibReverse(reverse) => {
                if let Some(Draft {
                    shape:
                        Shape::Fibonacci {
                            reverse: current, ..
                        },
                    ..
                }) = &mut self.draft
                {
                    *current = reverse;
                }
                Effect::None
            }
            Event::FibExtendRight(extend_right) => {
                if let Some(Draft {
                    shape:
                        Shape::Fibonacci {
                            extend_right: current,
                            ..
                        },
                    ..
                }) = &mut self.draft
                {
                    *current = extend_right;
                }
                Effect::None
            }
            Event::Magnet(enabled) => Effect::Magnet(enabled),
            Event::ToggleMagnet => Effect::ToggleMagnet,
            Event::Close => {
                self.close();
                Effect::Close
            }
        }
    }

    fn apply_draft(&mut self, store: &Store) -> Effect {
        let Some(draft) = self.draft.as_mut() else {
            return Effect::None;
        };
        let (shape, style) = match draft.shape_and_style() {
            Ok(value) => value,
            Err(error) => {
                draft.error = Some(error);
                return Effect::None;
            }
        };
        let command = match &draft.kind {
            DraftKind::NewText => Command::Create { shape, style },
            DraftKind::Edit(id) => {
                let Some(original) = store.find(*id) else {
                    draft.error = Some("Drawing is no longer available".into());
                    return Effect::None;
                };
                if original.locked && original.shape != shape {
                    draft.error = Some("Unlock this drawing before changing its shape".into());
                    return Effect::None;
                }
                Command::Replace(Drawing {
                    id: *id,
                    shape,
                    style,
                    visible: draft.visible,
                    locked: draft.locked,
                })
            }
        };
        draft.error = None;
        Effect::Commit(command)
    }

    pub fn key(
        &self,
        key: keyboard::Key<&str>,
        modifiers: keyboard::Modifiers,
        store: &Store,
    ) -> Option<Event> {
        use keyboard::{Key, key::Named};
        if self.draft.is_none()
            && modifiers.control()
            && modifiers.alt()
            && let Some(drawing) = store.selected_drawing()
        {
            if drawing.locked {
                return None;
            }
            return match key {
                Key::Character("k" | "K") => {
                    let index = Ink::ALL
                        .iter()
                        .position(|ink| *ink == drawing.style.ink)
                        .unwrap_or(0);
                    Some(Event::QuickInk(Ink::ALL[(index + 1) % Ink::ALL.len()]))
                }
                Key::Character("p" | "P") if !matches!(drawing.shape, Shape::Text { .. }) => {
                    let index = Pattern::ALL
                        .iter()
                        .position(|pattern| *pattern == drawing.style.pattern)
                        .unwrap_or(0);
                    Some(Event::QuickPattern(
                        Pattern::ALL[(index + 1) % Pattern::ALL.len()],
                    ))
                }
                Key::Character("[") => {
                    Some(Event::QuickWidth((drawing.style.width - 0.5).max(0.5)))
                }
                Key::Character("]") => {
                    Some(Event::QuickWidth((drawing.style.width + 0.5).min(20.0)))
                }
                _ => None,
            };
        }
        if !self.open {
            return None;
        }
        if modifiers.control() && !modifiers.alt() {
            match key {
                Key::Named(Named::Enter) if self.draft.is_some() => return Some(Event::Apply),
                Key::Character("p" | "P") if self.draft.is_none() => {
                    return Some(Event::SelectedVolumeProfile);
                }
                Key::Character("z" | "Z") if self.draft.is_none() && modifiers.shift() => {
                    return store.can_redo().then_some(Event::Redo);
                }
                Key::Character("z" | "Z") if self.draft.is_none() => {
                    return store.can_undo().then_some(Event::Undo);
                }
                _ => {}
            }
        }
        if modifiers.control()
            && modifiers.alt()
            && let Some(draft) = &self.draft
        {
            return match key {
                Key::Character("k" | "K") => {
                    let index = Ink::ALL
                        .iter()
                        .position(|ink| *ink == draft.style.ink)
                        .unwrap_or(0);
                    Some(Event::Ink(Ink::ALL[(index + 1) % Ink::ALL.len()]))
                }
                Key::Character("p" | "P") if !matches!(draft.shape, Shape::Text { .. }) => {
                    let index = Pattern::ALL
                        .iter()
                        .position(|pattern| *pattern == draft.style.pattern)
                        .unwrap_or(0);
                    Some(Event::Pattern(
                        Pattern::ALL[(index + 1) % Pattern::ALL.len()],
                    ))
                }
                Key::Character("r" | "R") => match &draft.shape {
                    Shape::Fibonacci { reverse, .. } => Some(Event::FibReverse(!reverse)),
                    _ => None,
                },
                Key::Character("e" | "E") => match &draft.shape {
                    Shape::Fibonacci { extend_right, .. } => {
                        Some(Event::FibExtendRight(!extend_right))
                    }
                    _ => None,
                },
                _ => None,
            };
        }
        if matches!(key, Key::Named(Named::Escape)) {
            return Some(if self.draft.is_some() {
                Event::Cancel
            } else {
                Event::Close
            });
        }
        if self.draft.is_some() {
            return None;
        }
        if modifiers.control() && !modifiers.alt() && matches!(key, Key::Character("m" | "M")) {
            return Some(Event::ToggleMagnet);
        }
        if modifiers.control() && !modifiers.alt() {
            let selected = store.selected()?;
            return match key {
                Key::Character("e" | "E") => Some(Event::Properties(selected)),
                Key::Character("h" | "H") => Some(Event::Hide(selected)),
                Key::Character("l" | "L") => Some(Event::Lock(selected)),
                _ => None,
            };
        }
        match key {
            Key::Named(Named::ArrowUp) => Some(Event::Navigate(-1)),
            Key::Named(Named::ArrowDown) => Some(Event::Navigate(1)),
            Key::Named(Named::Enter) => store.selected().map(Event::Properties),
            Key::Named(Named::Delete) => store.selected().map(Event::Delete),
            Key::Character("0") => Some(Event::ChooseTool(Tool::Select)),
            Key::Character("1") => Some(Event::ChooseTool(Tool::Horizontal)),
            Key::Character("2") => Some(Event::ChooseTool(Tool::Vertical)),
            Key::Character("3") => Some(Event::ChooseTool(Tool::Trend)),
            Key::Character("4") => Some(Event::ChooseTool(Tool::Ray)),
            Key::Character("5") => Some(Event::ChooseTool(Tool::Rectangle)),
            Key::Character("6") => Some(Event::ChooseTool(Tool::Fibonacci)),
            Key::Character("7") => Some(Event::ChooseTool(Tool::Text)),
            Key::Character("8") => Some(Event::ChooseTool(Tool::Measure)),
            Key::Character("9") => Some(Event::ChooseTool(Tool::AnchoredVwap)),
            _ => None,
        }
    }
    pub fn context_style_view(&self, store: &Store) -> Element<'_, Message> {
        let Some(drawing) = store.selected_drawing() else {
            return iced::widget::Space::new(Length::Shrink, Length::Shrink).into();
        };
        let id = drawing.id;
        let generation = store.generation();
        let locked = drawing.locked;
        let is_text = matches!(drawing.shape, Shape::Text { .. });
        let style = drawing.style;
        let ink: Element<'_, Message> = if locked {
            text(style.ink.to_string()).into()
        } else {
            pick_list(Ink::ALL, Some(style.ink), move |value| {
                Message::DrawingCommand {
                    generation,
                    command: Command::Style {
                        id,
                        style: Style {
                            ink: value,
                            ..style
                        },
                    },
                }
            })
            .style(crate::ui::pick)
            .text_size(12)
            .padding([4, 6])
            .into()
        };
        let pattern: Element<'_, Message> = if locked || is_text {
            text(style.pattern.to_string()).into()
        } else {
            pick_list(Pattern::ALL, Some(style.pattern), move |value| {
                Message::DrawingCommand {
                    generation,
                    command: Command::Style {
                        id,
                        style: Style {
                            pattern: value,
                            ..style
                        },
                    },
                }
            })
            .style(crate::ui::pick)
            .text_size(12)
            .padding([4, 6])
            .into()
        };
        if is_text {
            return row![text("Ink").size(11), ink]
                .spacing(6)
                .align_y(iced::Alignment::Center)
                .into();
        }
        row![
            text("Ink").size(11),
            ink,
            text("Stroke").size(11),
            pattern,
            button("−")
                .style(crate::ui::quiet)
                .on_press_maybe((!locked).then_some(Message::DrawingCommand {
                    generation,
                    command: Command::Style {
                        id,
                        style: Style {
                            width: (style.width - 0.5).max(0.5),
                            ..style
                        }
                    }
                })),
            text(format!("{:.1}", style.width)).size(11),
            button("+")
                .style(crate::ui::quiet)
                .on_press_maybe((!locked).then_some(Message::DrawingCommand {
                    generation,
                    command: Command::Style {
                        id,
                        style: Style {
                            width: (style.width + 0.5).min(20.0),
                            ..style
                        }
                    }
                })),
        ]
        .spacing(6)
        .align_y(iced::Alignment::Center)
        .into()
    }

    #[allow(clippy::too_many_arguments)]
    pub fn view<'a>(
        &'a self,
        store: &'a Store,
        profiles: &'a [mexc_indicators::profiles::Instance],
        tool: Tool,
        magnet: bool,
        ready: bool,
        _palette: Palette,
        price_scale: u32,
        quote: &'a str,
        repeat: bool,
    ) -> Element<'a, Message> {
        iced::widget::responsive(move |size| {
            self.dialog_view(
                store,
                profiles,
                tool,
                magnet,
                ready,
                price_scale,
                quote,
                repeat,
                size.width >= 600.0,
            )
        })
        .into()
    }

    #[allow(clippy::too_many_arguments)]
    fn dialog_view<'a>(
        &'a self,
        store: &'a Store,
        profiles: &'a [mexc_indicators::profiles::Instance],
        tool: Tool,
        magnet: bool,
        ready: bool,
        price_scale: u32,
        quote: &'a str,
        repeat: bool,
        wide: bool,
    ) -> Element<'a, Message> {
        let editing = self.draft.is_some();
        let header = column![
            text(if editing {
                "Drawing properties"
            } else {
                "Drawings"
            })
            .size(18),
            text(format!(
                "{} · {}{}",
                store.key(),
                tool,
                if ready {
                    ""
                } else {
                    " · waiting for native candles"
                }
            ))
            .size(11)
            .style(crate::ui::muted_text),
        ]
        .spacing(4);

        let mut body = column![].spacing(10).width(Length::Fill);
        if let Some(draft) = &self.draft {
            let is_vwap = matches!(draft.shape, Shape::AnchoredVwap(_));
            let mut first_fields = column![field(
                "UTC · ISO-8601 or milliseconds",
                Field::UtcFirst,
                &draft.utc_first
            ),]
            .spacing(8);
            if is_vwap {
                first_fields = first_fields.push(text(
                    "Start time only; price is derived from native HLC3 × candle volume. A typed mid-bar start uses the first later opening; unavailable history remains partial."
                ).size(11).style(crate::ui::muted_text));
                if let DraftKind::Edit(id) = draft.kind
                    && let Some(cache) = store.vwap(id)
                {
                    first_fields = first_fields.push(text(cache.status()).size(11));
                }
            } else {
                first_fields = first_fields.push(field(
                    format!("Price · {quote} · precision {price_scale}"),
                    Field::PriceFirst,
                    &draft.price_first,
                ));
            }
            let first = section(
                if is_vwap { "VWAP start" } else { "Anchor A" },
                first_fields.into(),
            );
            let anchors: Element<'a, Message> = if draft.shape.anchors().second().is_some() {
                let second = section(
                    "Anchor B",
                    column![
                        field(
                            "UTC · ISO-8601 or milliseconds",
                            Field::UtcSecond,
                            &draft.utc_second
                        ),
                        field(
                            format!("Price · {quote} · precision {price_scale}"),
                            Field::PriceSecond,
                            &draft.price_second
                        ),
                    ]
                    .spacing(8)
                    .into(),
                );
                if wide {
                    row![first, second].spacing(10).width(Length::Fill).into()
                } else {
                    column![first, second]
                        .spacing(10)
                        .width(Length::Fill)
                        .into()
                }
            } else {
                first
            };
            body = body.push(anchors);
            let mut appearance = column![].spacing(8).width(Length::Fill);
            if matches!(&draft.shape, Shape::Text { .. }) {
                appearance = appearance
                    .push(field("Text", Field::Text, &draft.text))
                    .push(field("Text size", Field::FontSize, &draft.font_size));
            }
            appearance = appearance.push(
                row![
                    text("Ink").size(12),
                    pick_list(Ink::ALL, Some(draft.style.ink), |v| {
                        Message::DrawingControl(Event::Ink(v))
                    })
                    .style(crate::ui::pick)
                    .width(Length::Fill),
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center),
            );
            if !matches!(&draft.shape, Shape::Text { .. }) {
                appearance = appearance.push(
                    row![
                        column![
                            text("Stroke").size(11).style(crate::ui::muted_text),
                            pick_list(Pattern::ALL, Some(draft.style.pattern), |v| {
                                Message::DrawingControl(Event::Pattern(v))
                            })
                            .style(crate::ui::pick)
                            .text_size(13)
                            .width(Length::Fill),
                        ]
                        .spacing(4)
                        .width(Length::FillPortion(2)),
                        container(field("Width", Field::Width, &draft.width))
                            .width(Length::FillPortion(1)),
                    ]
                    .spacing(10),
                );
            }
            if matches!(&draft.shape, Shape::Fibonacci { .. }) {
                appearance = appearance
                    .push(field("Label size", Field::FontSize, &draft.font_size))
                    .push(field(
                        "Fib levels · comma-separated ratios",
                        Field::FibLevels,
                        &draft.levels,
                    ));
            }
            if matches!(&draft.shape, Shape::Rectangle(_, _)) {
                appearance = appearance.push(field(
                    "Fill opacity (0–1)",
                    Field::FillOpacity,
                    &draft.fill_opacity,
                ));
            }
            if let Shape::Fibonacci {
                reverse,
                extend_right,
                ..
            } = &draft.shape
            {
                appearance = appearance.push(
                    row![
                        checkbox("Reverse", *reverse)
                            .on_toggle(|v| Message::DrawingControl(Event::FibReverse(v))),
                        checkbox("Extend right", *extend_right)
                            .on_toggle(|v| Message::DrawingControl(Event::FibExtendRight(v))),
                    ]
                    .spacing(16)
                    .wrap(),
                );
            }
            body = body.push(section("Appearance", appearance.into()));
        } else {
            body = body.push(text("TOOLS").size(11).style(crate::ui::muted_text));
            let columns = if wide { 4 } else { 2 };
            let choices = Tool::ALL;
            let mut tool_rows = column![].spacing(5).width(Length::Fill);
            for group in choices.chunks(columns) {
                let mut tool_row = row![].spacing(5).width(Length::Fill);
                for choice in group {
                    let index = Tool::ALL
                        .iter()
                        .position(|candidate| candidate == choice)
                        .unwrap_or(0);
                    let label = match choice {
                        Tool::Select => "Select",
                        Tool::Horizontal => "H-line",
                        Tool::Vertical => "V-line",
                        Tool::Trend => "Trend",
                        Tool::Ray => "Ray",
                        Tool::Rectangle => "Zone",
                        Tool::Fibonacci => "Fib",
                        Tool::Text => "Text",
                        Tool::Measure => "Measure",
                        Tool::AnchoredVwap => "Anchored VWAP",
                    };
                    let action = (ready || *choice == Tool::Select)
                        .then_some(Message::DrawingControl(Event::ChooseTool(*choice)));
                    let selected = *choice == tool;
                    tool_row = tool_row.push(
                        button(
                            row![crate::ui::key((index).to_string()), text(label).size(12),]
                                .spacing(6)
                                .align_y(iced::Alignment::Center),
                        )
                        .width(Length::FillPortion(1))
                        .padding([6, 8])
                        .style(move |theme, status| {
                            if selected {
                                crate::ui::selected(theme, status)
                            } else {
                                crate::ui::quiet(theme, status)
                            }
                        })
                        .on_press_maybe(action),
                    );
                }
                for _ in group.len()..columns {
                    tool_row = tool_row.push(iced::widget::Space::new(
                        Length::FillPortion(1),
                        Length::Shrink,
                    ));
                }
                tool_rows = tool_rows.push(tool_row);
            }
            body = body.push(tool_rows);
            body = body.push(
                button("Selected Range Volume Profile · Ctrl+P")
                    .style(crate::ui::quiet)
                    .width(Length::Fill)
                    .on_press_maybe(
                        ready.then_some(Message::DrawingControl(Event::SelectedVolumeProfile)),
                    ),
            );
            body = body.push(
                row![
                    text("OBJECTS").size(11).style(crate::ui::muted_text),
                    iced::widget::Space::new(Length::Fill, Length::Shrink),
                    text("Up/Down select · Enter edit")
                        .size(11)
                        .style(crate::ui::muted_text),
                ]
                .align_y(iced::Alignment::Center),
            );
            let mut objects = column![].spacing(3).width(Length::Fill);
            for drawing in store.objects() {
                let selected = store.selected() == Some(drawing.id);
                let status = format!(
                    "{}{}{}",
                    if drawing.visible { "visible" } else { "hidden" },
                    if drawing.locked { " · locked" } else { "" },
                    if selected { " · selected" } else { "" },
                );
                objects = objects.push(
                    button(
                        row![
                            text(format!("#{}", drawing.id)).size(12),
                            text(drawing.shape.tool().to_string()).size(12),
                            iced::widget::Space::new(Length::Fill, Length::Shrink),
                            text(status).size(11).style(crate::ui::muted_text),
                        ]
                        .spacing(8)
                        .align_y(iced::Alignment::Center),
                    )
                    .width(Length::Fill)
                    .padding([7, 9])
                    .style(move |theme, status| {
                        if selected {
                            crate::ui::selected(theme, status)
                        } else {
                            crate::ui::quiet(theme, status)
                        }
                    })
                    .on_press(Message::DrawingControl(Event::Select(drawing.id))),
                );
            }
            if store.objects().is_empty() {
                objects = objects.push(
                    text("No drawings yet")
                        .size(12)
                        .style(crate::ui::muted_text),
                );
            }
            body = body.push(objects);
            let mut ranges = column![text("PROFILE RANGES").size(11)].spacing(5);
            for profile in profiles.iter().filter(|profile| {
                matches!(
                    profile.spec.range,
                    mexc_indicators::profiles::Range::Fixed { .. }
                )
            }) {
                ranges = ranges.push(
                    row![
                        button(text(format!(
                            "Select {} #{}{}",
                            profile.spec.kind,
                            profile.id,
                            if profile.visible { "" } else { " · hidden" }
                        )))
                        .style(crate::ui::quiet)
                        .on_press(Message::ProfileEdit(
                            crate::profile_layer::Edit::Select(profile.id)
                        )),
                        button("Settings / visibility / delete")
                            .style(crate::ui::quiet)
                            .on_press(Message::Profile(crate::profile_controls::Event::Open(
                                Some(profile.id)
                            ))),
                    ]
                    .spacing(5),
                );
            }
            body = body.push(ranges);
        }

        let mut footer = column![].spacing(7).width(Length::Fill);
        if !editing && let Some(drawing) = store.selected_drawing() {
            let id = drawing.id;
            footer = footer.push(
                row![
                    text(format!("Selected · #{} · {}", id, drawing.shape.tool())).size(12),
                    button("Edit")
                        .style(crate::ui::quiet)
                        .on_press(Message::DrawingControl(Event::Properties(id))),
                    button(if drawing.visible { "Hide" } else { "Show" })
                        .style(crate::ui::quiet)
                        .on_press(Message::DrawingControl(Event::Hide(id))),
                    button(if drawing.locked { "Unlock" } else { "Lock" })
                        .style(crate::ui::quiet)
                        .on_press(Message::DrawingControl(Event::Lock(id))),
                    button("Delete")
                        .style(crate::ui::danger)
                        .on_press(Message::DrawingControl(Event::Delete(id))),
                ]
                .spacing(5)
                .align_y(iced::Alignment::Center)
                .wrap(),
            );
        }
        let ending: Element<'a, Message> = if editing {
            row![
                button("Cancel")
                    .style(crate::ui::quiet)
                    .on_press(Message::DrawingControl(Event::Cancel)),
                button("Apply")
                    .style(crate::ui::primary)
                    .on_press(Message::DrawingControl(Event::Apply)),
            ]
            .spacing(6)
            .into()
        } else {
            button("Close")
                .style(crate::ui::quiet)
                .on_press(Message::DrawingControl(Event::Close))
                .into()
        };
        footer = footer.push(
            row![
                button("Undo").style(crate::ui::quiet).on_press_maybe(
                    (!editing && store.can_undo()).then_some(Message::DrawingControl(Event::Undo))
                ),
                button("Redo").style(crate::ui::quiet).on_press_maybe(
                    (!editing && store.can_redo()).then_some(Message::DrawingControl(Event::Redo))
                ),
                checkbox("Magnet", magnet).on_toggle(|v| Message::DrawingControl(Event::Magnet(v))),
                checkbox("Repeat tool", repeat)
                    .on_toggle(|v| Message::DrawingControl(Event::Repeat(v))),
                iced::widget::Space::new(Length::Fill, Length::Shrink),
                ending,
            ]
            .spacing(6)
            .align_y(iced::Alignment::Center),
        );
        let mut content = column![
            header,
            scrollable(container(body).padding(iced::Padding {
                right: 16.0,
                ..iced::Padding::default()
            }))
            .id(scrollable::Id::new(BODY_ID))
            .height(Length::Fill),
        ]
        .spacing(9)
        .height(Length::Fill);
        if let Some(error) = self
            .draft
            .as_ref()
            .and_then(|draft| draft.error.as_ref())
            .or(self.error.as_ref())
        {
            content = content.push(text(error).size(12));
        }
        content = content.push(crate::ui::separator()).push(footer);
        container(content)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }
}
fn style_effect(store: &Store, update: impl FnOnce(&mut Style)) -> Effect {
    let Some(drawing) = store.selected_drawing() else {
        return Effect::None;
    };
    let mut style = drawing.style;
    update(&mut style);
    Effect::Commit(Command::Style {
        id: drawing.id,
        style,
    })
}

fn section<'a>(title: &'static str, content: Element<'a, Message>) -> Element<'a, Message> {
    container(
        column![text(title).size(13), content]
            .spacing(7)
            .width(Length::Fill),
    )
    .padding([8, 9])
    .width(Length::Fill)
    .style(crate::ui::card)
    .into()
}

fn field<'a>(label: impl Into<String>, field: Field, value: &'a str) -> Element<'a, Message> {
    column![
        text(label.into()).size(11).style(crate::ui::muted_text),
        text_input("", value)
            .id(field_id(field))
            .size(13)
            .on_input(move |value| Message::DrawingControl(Event::Field(field, value)))
            .width(Length::Fill)
            .style(crate::ui::input)
    ]
    .spacing(4)
    .width(Length::Fill)
    .into()
}

fn field_id(field: Field) -> text_input::Id {
    text_input::Id::new(format!("drawing-field-{field:?}"))
}

fn utc_field(utc_ms: i64) -> String {
    DateTime::<Utc>::from_timestamp_millis(utc_ms)
        .map(|timestamp| timestamp.to_rfc3339_opts(SecondsFormat::Millis, true))
        .unwrap_or_else(|| utc_ms.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mexc_model::Market;

    #[test]
    fn anchored_vwap_properties_edit_utc_without_an_independent_price() {
        let drawing = Drawing {
            id: 42,
            shape: Shape::AnchoredVwap(Anchor {
                utc_ms: 300_000,
                price: 0.0,
            }),
            style: Style::default(),
            visible: true,
            locked: false,
        };
        let mut draft = Draft::from_drawing(&drawing);
        draft.field(Field::UtcFirst, "1970-01-01T00:07:30Z".into());
        draft.field(Field::PriceFirst, "not a price".into());
        let (shape, _) = draft.shape_and_style().unwrap();
        assert_eq!(
            shape,
            Shape::AnchoredVwap(Anchor {
                utc_ms: 450_000,
                price: 0.0
            })
        );
        draft.field(Field::UtcFirst, "not a UTC timestamp".into());
        assert!(draft.shape_and_style().is_err());
        let mut controls = Controls::default();
        controls.open_menu(&Store::new(&Market::default()));
        assert!(matches!(
            controls.key(
                keyboard::Key::Character("9"),
                keyboard::Modifiers::default(),
                &Store::new(&Market::default())
            ),
            Some(Event::ChooseTool(Tool::AnchoredVwap))
        ));
    }

    #[test]
    fn empty_text_is_rejected_and_valid_draft_survives_commit_handoff() {
        let store = Store::new(&Market::default());
        let mut controls = Controls::default();
        controls.open_text(
            &store,
            Anchor {
                utc_ms: 0,
                price: 1.25,
            },
        );

        assert!(matches!(
            controls.update(Event::Apply, &store),
            Effect::None
        ));
        assert!(controls.is_editing());

        controls.update(Event::Field(Field::Text, "support".into()), &store);
        let effect = controls.update(Event::Apply, &store);
        assert!(matches!(effect, Effect::Commit(Command::Create {
            shape: Shape::Text { text, .. },
            ..
        }) if text == "support"));
        assert!(controls.is_editing());
        controls.close();
        assert!(!controls.is_editing());
    }

    #[test]
    fn utc_fields_round_trip_in_iso_format_and_accept_milliseconds() {
        assert_eq!(
            Draft::anchor("2024-01-02T03:04:05.006Z", "1.0", "First")
                .unwrap()
                .utc_ms,
            1_704_164_645_006
        );
        assert_eq!(utc_field(1_704_164_645_006), "2024-01-02T03:04:05.006Z");
        assert_eq!(
            Draft::anchor("123456789", "1.0", "First").unwrap().utc_ms,
            123_456_789
        );
    }

    #[test]
    fn stale_context_cannot_apply_a_text_draft() {
        let market = Market::default();
        let store = Store::new(&market);
        let mut controls = Controls::default();
        controls.open_text(
            &store,
            Anchor {
                utc_ms: 0,
                price: 1.0,
            },
        );
        controls.update(
            Event::Field(Field::Text, "belongs to original instrument".into()),
            &store,
        );
        let mut switched = store;
        switched.switch_context(&Market {
            symbol: "OTHER".into(),
            ..market
        });

        if let Effect::Commit(command) = controls.update(Event::Apply, &switched) {
            switched.apply(command).unwrap();
        }
        assert!(switched.objects().is_empty());
        assert!(controls.is_editing());
    }
}
