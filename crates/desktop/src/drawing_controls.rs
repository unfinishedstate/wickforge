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
    Select(DrawingId),
    Properties(DrawingId),
    Field(Field, String),
    Ink(Ink),
    Pattern(Pattern),
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
    Select(Option<DrawingId>),
    Magnet(bool),
    ToggleMagnet,
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
        let first = Self::anchor(&self.utc_first, &self.price_first, "First")?;
        let second = Self::anchor(&self.utc_second, &self.price_second, "Second")?;
        let shape = match &self.shape {
            Shape::Horizontal(_) => Shape::Horizontal(first),
            Shape::Vertical(_) => Shape::Vertical(first),
            Shape::Trend(_, _) => Shape::Trend(first, second),
            Shape::Ray(_, _) => Shape::Ray(first, second),
            Shape::Rectangle(_, _) => Shape::Rectangle(first, second),
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
    pub fn context_generation(&self) -> u64 {
        self.generation
    }
    pub fn focus_id(&self) -> Option<text_input::Id> {
        let draft = self.draft.as_ref()?;
        let field = match &draft.shape {
            Shape::Text { .. } => Field::Text,
            Shape::Horizontal(_)
            | Shape::Trend(_, _)
            | Shape::Ray(_, _)
            | Shape::Rectangle(_, _)
            | Shape::Fibonacci { .. } => Field::PriceFirst,
            Shape::Vertical(_) => Field::UtcFirst,
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
        if !self.open || self.generation != store.generation() {
            return Effect::None;
        }
        match event {
            Event::ChooseTool(tool) => Effect::ChooseTool(tool),
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
        if !self.open {
            return None;
        }
        if modifiers.control() && !modifiers.alt() {
            match key {
                Key::Named(Named::Enter) if self.draft.is_some() => return Some(Event::Apply),
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
            _ => None,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn view<'a>(
        &'a self,
        store: &'a Store,
        tool: Tool,
        magnet: bool,
        ready: bool,
        palette: Palette,
        price_scale: u32,
        quote: &'a str,
    ) -> Element<'a, Message> {
        let mut head = row![
            text("Draw").size(18),
            text(format!("{} · {}", store.key(), tool)).size(12)
        ]
        .spacing(10)
        .align_y(iced::Alignment::Center);
        if ready {
            head = head.push(text("Ready").size(12));
        }
        let mut body = column![].spacing(8);
        if let Some(draft) = &self.draft {
            body = body.push(text("Ctrl+Enter apply · Ctrl+Alt+K ink · Ctrl+Alt+P stroke · Ctrl+Alt+R/E Fibonacci reverse/extend").size(12));
            body = body.push(
                text(match &draft.kind {
                    DraftKind::NewText => "New text",
                    DraftKind::Edit(_) => "Drawing properties",
                })
                .size(15),
            );
            body = body.push(field(
                "UTC · first (ISO-8601 or milliseconds)",
                Field::UtcFirst,
                &draft.utc_first,
            ));
            body = body.push(field(
                format!("Price · {quote} (precision {price_scale}) · first"),
                Field::PriceFirst,
                &draft.price_first,
            ));
            if draft.shape.anchors().second().is_some() {
                body = body.push(field(
                    "UTC · second (ISO-8601 or milliseconds)",
                    Field::UtcSecond,
                    &draft.utc_second,
                ));
                body = body.push(field(
                    format!("Price · {quote} (precision {price_scale}) · second"),
                    Field::PriceSecond,
                    &draft.price_second,
                ));
            }
            if matches!(&draft.shape, Shape::Text { .. }) {
                body = body.push(field("Text", Field::Text, &draft.text));
                body = body.push(field("Text size", Field::FontSize, &draft.font_size));
                body = body.push(
                    row![
                        text("Ink"),
                        pick_list(
                            Ink::ALL,
                            Some(draft.style.ink),
                            |v| Message::DrawingControl(Event::Ink(v))
                        )
                    ]
                    .spacing(8),
                );
            } else {
                body = body.push(
                    row![
                        text("Ink"),
                        pick_list(
                            Ink::ALL,
                            Some(draft.style.ink),
                            |v| Message::DrawingControl(Event::Ink(v))
                        )
                    ]
                    .spacing(8),
                );
                body = body.push(
                    row![
                        text("Stroke"),
                        pick_list(Pattern::ALL, Some(draft.style.pattern), |v| {
                            Message::DrawingControl(Event::Pattern(v))
                        }),
                        field("Width", Field::Width, &draft.width)
                    ]
                    .spacing(8)
                    .wrap(),
                );
                if matches!(&draft.shape, Shape::Rectangle(_, _)) {
                    body = body.push(field(
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
                    body = body.push(field("Label size", Field::FontSize, &draft.font_size));
                    body = body.push(field(
                        "Fib levels · comma separated ratios",
                        Field::FibLevels,
                        &draft.levels,
                    ));
                    body = body.push(
                        checkbox("Reverse", *reverse)
                            .on_toggle(|v| Message::DrawingControl(Event::FibReverse(v))),
                    );
                    body = body.push(
                        checkbox("Extend right", *extend_right)
                            .on_toggle(|v| Message::DrawingControl(Event::FibExtendRight(v))),
                    );
                }
            }
        } else {
            body = body.push(text("Tools · 0–7").size(14));
            body = body.push(text("Up/Down objects · Enter edit · Ctrl+H show/hide · Ctrl+L lock · Delete · Ctrl+M magnet").size(12));
            let mut tools = row![].spacing(4);
            for choice in Tool::ALL {
                let action = (ready || choice == Tool::Select)
                    .then_some(Message::DrawingControl(Event::ChooseTool(choice)));
                tools = tools.push(button(text(choice.to_string())).on_press_maybe(action));
            }
            body = body.push(tools.wrap()).push(text("Objects").size(14));
            let mut rows = column![].spacing(4);
            for drawing in store.objects() {
                let selected = store.selected() == Some(drawing.id);
                let status = format!(
                    "{}{}{}",
                    if drawing.visible { "shown" } else { "hidden" },
                    if drawing.locked { " · locked" } else { "" },
                    if selected { " · selected" } else { "" }
                );
                let label = format!("{} #{}\n{}", drawing.shape.tool(), drawing.id, status);
                rows = rows.push(
                    row![
                        button(text(label).size(12))
                            .width(Length::Fill)
                            .on_press(Message::DrawingControl(Event::Select(drawing.id))),
                        button("Edit")
                            .on_press(Message::DrawingControl(Event::Properties(drawing.id))),
                        button(if drawing.visible { "Hide" } else { "Show" })
                            .on_press(Message::DrawingControl(Event::Hide(drawing.id))),
                        button(if drawing.locked { "Unlock" } else { "Lock" })
                            .on_press(Message::DrawingControl(Event::Lock(drawing.id))),
                        button("Delete")
                            .on_press(Message::DrawingControl(Event::Delete(drawing.id))),
                    ]
                    .spacing(4)
                    .align_y(iced::Alignment::Center)
                    .wrap(),
                );
            }
            if store.objects().is_empty() {
                rows = rows.push(text("No drawings yet.").size(12));
            }
            body = body.push(rows);
        }
        let editing = self.draft.is_some();
        let mut footer = row![
            button(if store.can_undo() { "Undo" } else { "Undo ·" }).on_press_maybe(
                (!editing && store.can_undo()).then_some(Message::DrawingControl(Event::Undo))
            ),
            button(if store.can_redo() { "Redo" } else { "Redo ·" }).on_press_maybe(
                (!editing && store.can_redo()).then_some(Message::DrawingControl(Event::Redo))
            ),
            checkbox("Magnet", magnet).on_toggle(|v| Message::DrawingControl(Event::Magnet(v))),
        ]
        .spacing(5);
        if editing {
            footer = footer
                .push(button("Apply").on_press(Message::DrawingControl(Event::Apply)))
                .push(button("Cancel edit").on_press(Message::DrawingControl(Event::Cancel)));
        }
        let footer = footer
            .push(button("Close").on_press(Message::DrawingControl(Event::Close)))
            .wrap();
        let mut content = column![
            head,
            scrollable(container(body).padding(iced::Padding {
                right: 16.0,
                ..iced::Padding::default()
            }))
            .id(scrollable::Id::new(BODY_ID))
            .height(Length::Fill),
        ]
        .spacing(8)
        .height(Length::Fill);
        if let Some(error) = self
            .draft
            .as_ref()
            .and_then(|draft| draft.error.as_ref())
            .or(self.error.as_ref())
        {
            content = content.push(text(error).size(12));
        }
        container(content.push(footer))
            .padding(10)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(move |_| iced::widget::container::Style {
                background: Some(palette.background.into()),
                text_color: Some(palette.foreground),
                border: iced::Border {
                    color: palette.muted,
                    width: 1.0,
                    radius: 6.0.into(),
                },
                ..Default::default()
            })
            .into()
    }
}

fn field<'a>(label: impl Into<String>, field: Field, value: &'a str) -> Element<'a, Message> {
    column![
        text(label.into()).size(12),
        text_input("", value)
            .id(field_id(field))
            .on_input(move |value| Message::DrawingControl(Event::Field(field, value)))
            .width(Length::Fill)
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
