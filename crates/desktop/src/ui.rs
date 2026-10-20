use crate::Message;
use iced::advanced::widget::{
    self, Operation,
    operation::{Outcome, focusable::Focusable, scrollable::Scrollable},
};
use iced::widget::{Space, button, container, pick_list, scrollable, text, text_input};
use iced::{Border, Color, Element, Length, Rectangle, Shadow, Task, Theme, Vector};

// UI surfaces follow the current palette; chart/data colors remain independent.
fn blend(a: Color, b: Color, amount: f32) -> Color {
    Color {
        r: a.r + (b.r - a.r) * amount,
        g: a.g + (b.g - a.g) * amount,
        b: a.b + (b.b - a.b) * amount,
        a: 1.0,
    }
}
pub fn surface(theme: &Theme) -> Color {
    let p = theme.palette();
    blend(p.background, p.text, 0.035)
}
pub fn border_color(theme: &Theme) -> Color {
    let p = theme.palette();
    blend(p.background, p.text, 0.18)
}

pub fn quiet(theme: &Theme, status: button::Status) -> button::Style {
    let p = theme.palette();
    let background = match status {
        button::Status::Hovered => Some(blend(p.background, p.text, 0.09).into()),
        button::Status::Pressed => Some(blend(p.background, p.text, 0.14).into()),
        _ => None,
    };
    button::Style {
        background,
        text_color: if status == button::Status::Disabled {
            blend(p.background, p.text, 0.38)
        } else {
            p.text
        },
        border: Border {
            radius: 6.0.into(),
            ..Border::default()
        },
        ..button::Style::default()
    }
}
pub fn primary(theme: &Theme, status: button::Status) -> button::Style {
    let mut style = button::primary(theme, status);
    style.border.radius = 7.0.into();
    style.shadow = Shadow::default();
    style
}
pub fn selected(theme: &Theme, status: button::Status) -> button::Style {
    let p = theme.palette();
    let mut style = quiet(theme, status);
    style.background = Some(
        blend(
            p.background,
            p.primary,
            if status == button::Status::Hovered {
                0.20
            } else {
                0.12
            },
        )
        .into(),
    );
    style.border = Border {
        color: p.primary,
        width: 1.0,
        radius: 7.0.into(),
    };
    style
}
pub fn danger(theme: &Theme, status: button::Status) -> button::Style {
    let p = theme.palette();
    let mut style = quiet(theme, status);
    if status != button::Status::Disabled {
        style.text_color = p.danger;
    }
    if matches!(status, button::Status::Hovered | button::Status::Pressed) {
        style.background = Some(blend(p.background, p.danger, 0.12).into());
    }
    style
}
pub fn input(theme: &Theme, status: text_input::Status) -> text_input::Style {
    let mut style = text_input::default(theme, status);
    style.background = surface(theme).into();
    style.border = Border {
        color: if status == text_input::Status::Focused {
            theme.palette().primary
        } else {
            border_color(theme)
        },
        width: 1.0,
        radius: 7.0.into(),
    };
    style
}
pub fn pick(theme: &Theme, status: pick_list::Status) -> pick_list::Style {
    let mut style = pick_list::default(theme, status);
    style.background = surface(theme).into();
    style.border = Border {
        color: if status == pick_list::Status::Opened {
            theme.palette().primary
        } else {
            border_color(theme)
        },
        width: 1.0,
        radius: 7.0.into(),
    };
    style
}
pub fn panel(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(surface(theme).into()),
        text_color: Some(theme.palette().text),
        border: Border {
            color: border_color(theme),
            width: 1.0,
            radius: 12.0.into(),
        },
        shadow: Shadow {
            color: Color::from_rgba(0.0, 0.0, 0.0, 0.18),
            offset: Vector::new(0.0, 8.0),
            blur_radius: 24.0,
        },
    }
}
pub fn card(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(theme.palette().background.into()),
        border: Border {
            color: border_color(theme),
            width: 1.0,
            radius: 8.0.into(),
        },
        ..container::Style::default()
    }
}
pub fn muted_text(theme: &Theme) -> text::Style {
    text::Style {
        color: Some(blend(
            theme.palette().background,
            theme.palette().text,
            0.82,
        )),
    }
}

#[derive(Clone, Copy)]
pub enum Tone {
    Accent,
    Positive,
    Negative,
    Neutral,
}

fn ink(theme: &Theme, tone: Tone) -> Color {
    let p = theme.palette();
    match tone {
        Tone::Accent => p.primary,
        Tone::Positive => p.success,
        Tone::Negative => p.danger,
        Tone::Neutral => p.text,
    }
}

pub fn accent_text(theme: &Theme) -> text::Style {
    text::Style {
        color: Some(ink(theme, Tone::Accent)),
    }
}

pub fn negative_text(theme: &Theme) -> text::Style {
    text::Style {
        color: Some(ink(theme, Tone::Negative)),
    }
}

pub fn badge<'a>(label: impl text::IntoFragment<'a>, tone: Tone) -> Element<'a, Message> {
    container(text(label).size(11))
        .padding([2, 6])
        .width(Length::Shrink)
        .style(move |theme: &Theme| container::Style {
            // Use the palette's base background, against which semantic ink is contrast-resolved.
            background: Some(theme.palette().background.into()),
            text_color: Some(ink(theme, tone)),
            border: Border {
                color: blend(theme.palette().background, ink(theme, tone), 0.45),
                width: 1.0,
                radius: 5.0.into(),
            },
            ..container::Style::default()
        })
        .into()
}

pub fn hint<'a>(
    content: impl Into<Element<'a, Message>>,
    explanation: impl text::IntoFragment<'a>,
) -> Element<'a, Message> {
    iced::widget::tooltip(
        content,
        container(text(explanation).size(12))
            .padding(10)
            .max_width(360)
            .style(panel),
        iced::widget::tooltip::Position::FollowCursor,
    )
    .into()
}
pub fn key<'a>(label: impl text::IntoFragment<'a>) -> Element<'a, Message> {
    container(text(label).size(10).font(iced::Font::MONOSPACE))
        .padding([2, 5])
        .style(card)
        .into()
}
pub fn separator<'a>() -> Element<'a, Message> {
    container(Space::new(Length::Fill, 1))
        .style(|theme: &Theme| container::Style {
            background: Some(border_color(theme).into()),
            ..container::Style::default()
        })
        .into()
}
struct RevealFocused {
    editor: widget::Id,
    viewport: Option<(Rectangle, Rectangle, Vector)>,
    inside: bool,
    parent: Option<Rectangle>,
    focused: Option<Rectangle>,
}

impl Operation<Option<scrollable::AbsoluteOffset>> for RevealFocused {
    fn container(
        &mut self,
        id: Option<&widget::Id>,
        bounds: Rectangle,
        children: &mut dyn FnMut(&mut dyn Operation<Option<scrollable::AbsoluteOffset>>),
    ) {
        let previous = (self.inside, self.parent);
        self.inside |= id == Some(&self.editor);
        if self.inside {
            self.parent = Some(bounds);
        }
        children(self);
        (self.inside, self.parent) = previous;
    }

    fn scrollable(
        &mut self,
        _: &mut dyn Scrollable,
        id: Option<&widget::Id>,
        bounds: Rectangle,
        content: Rectangle,
        translation: Vector,
    ) {
        if id == Some(&self.editor) {
            self.viewport = Some((bounds, content, translation));
        }
    }

    fn focusable(&mut self, state: &mut dyn Focusable, _: Option<&widget::Id>) {
        if self.inside && state.is_focused() {
            self.focused = self.parent;
        }
    }

    fn finish(&self) -> Outcome<Option<scrollable::AbsoluteOffset>> {
        Outcome::Some(self.viewport.zip(self.focused).and_then(
            |((bounds, content, translation), field)| {
                reveal_offset(bounds, content, translation, field)
            },
        ))
    }
}

fn reveal_offset(
    bounds: Rectangle,
    content: Rectangle,
    translation: Vector,
    field: Rectangle,
) -> Option<scrollable::AbsoluteOffset> {
    let y = if field.y < bounds.y + translation.y {
        field.y - bounds.y
    } else if field.y + field.height > bounds.y + bounds.height + translation.y {
        field.y + field.height - bounds.y - bounds.height
    } else {
        return None;
    };
    Some(scrollable::AbsoluteOffset {
        x: translation.x,
        y: y.clamp(0.0, (content.height - bounds.height).max(0.0)),
    })
}

/// Reveal the focused input from actual scroll/widget bounds, without resetting
/// an already visible field or guessing editor-specific row offsets.
pub fn reveal_focus<Message: Send + 'static>(scroll_id: &'static str) -> Task<Message> {
    widget::operate(RevealFocused {
        editor: widget::Id::new(scroll_id),
        viewport: None,
        inside: false,
        parent: None,
        focused: None,
    })
    .then(move |offset| {
        offset.map_or_else(Task::none, |offset| {
            scrollable::scroll_to(scrollable::Id::new(scroll_id), offset)
        })
    })
}

#[cfg(test)]
mod focus_visibility_tests {
    use super::*;

    #[test]
    fn focused_fields_are_revealed_from_real_layout_bounds_without_resetting_visible_fields() {
        let bounds = Rectangle {
            x: 0.0,
            y: 100.0,
            width: 300.0,
            height: 150.0,
        };
        let content = Rectangle {
            height: 1_000.0,
            ..bounds
        };
        let translation = Vector::new(0.0, 400.0);
        let field = Rectangle {
            y: 800.0,
            height: 60.0,
            ..bounds
        };
        assert_eq!(
            reveal_offset(bounds, content, translation, field)
                .unwrap()
                .y,
            610.0
        );
        assert_eq!(
            reveal_offset(
                bounds,
                content,
                translation,
                Rectangle { y: 200.0, ..field }
            )
            .unwrap()
            .y,
            100.0
        );
        assert!(
            reveal_offset(
                bounds,
                content,
                translation,
                Rectangle { y: 520.0, ..field }
            )
            .is_none()
        );
        assert_eq!(
            reveal_offset(
                bounds,
                content,
                translation,
                Rectangle {
                    y: 1_040.0,
                    ..field
                }
            )
            .unwrap()
            .y,
            850.0
        );
    }
}
