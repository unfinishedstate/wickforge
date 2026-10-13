use crate::{
    Message,
    palette::Palette,
    state::{Action, bounded_plot_height},
};
use iced::widget::canvas::{self, Frame, Geometry};
use iced::{Element, Length, Point, Rectangle, Renderer, Size, Theme, mouse};
use std::time::{Duration, Instant};

const HIT_HEIGHT: f32 = 14.0;
const GEOMETRY_TOLERANCE: f32 = 0.75;

#[derive(Clone, Copy)]
struct Drag {
    id: Option<u64>,
    pointer_y: f32,
    initial_height: f32,
    initial_bounds: Rectangle,
    expected_height: f32,
}

impl Drag {
    fn valid(&self, handle: &Handle, bounds: Rectangle) -> bool {
        self.id == handle.id
            && self.expected_height == handle.height
            && (bounds.x - self.initial_bounds.x).abs() <= GEOMETRY_TOLERANCE
            && (bounds.width - self.initial_bounds.width).abs() <= GEOMETRY_TOLERANCE
            && (bounds.height - self.initial_bounds.height).abs() <= GEOMETRY_TOLERANCE
            && (bounds.y - self.initial_bounds.y - (handle.height - self.initial_height)).abs()
                <= GEOMETRY_TOLERANCE
    }

    fn height_at(&self, pointer_y: f32) -> Option<f32> {
        // Both positions are in window coordinates. Local bounds move while the
        // plot grows, so using the current local pointer would cause feedback jitter.
        bounded_plot_height(self.id, self.initial_height + (pointer_y - self.pointer_y))
    }
}

#[derive(Clone, Copy)]
struct Press {
    id: Option<u64>,
    when: Instant,
    pointer: Point,
    height: f32,
    bounds: Rectangle,
}

#[derive(Default)]
struct Interaction {
    drag: Option<Drag>,
    last_press: Option<Press>,
}

struct Handle {
    id: Option<u64>,
    height: f32,
    palette: Palette,
}

// Distinct Canvas State types deliberately reset iced's widget state tag while
// dialogs block input. Release may never reach the background Canvas, so merely
// guarding Program::update would leave a stale drag alive after closing a dialog.
struct BlockedHandle(Palette);

pub fn view<'a>(
    id: Option<u64>,
    height: f32,
    palette: Palette,
    blocked: bool,
) -> Element<'a, Message> {
    if blocked {
        canvas::Canvas::new(BlockedHandle(palette))
            .width(Length::Fill)
            .height(HIT_HEIGHT)
            .into()
    } else {
        canvas::Canvas::new(Handle {
            id,
            height,
            palette,
        })
        .width(Length::Fill)
        .height(HIT_HEIGHT)
        .into()
    }
}

fn draw_grip(
    renderer: &Renderer,
    bounds: Rectangle,
    palette: Palette,
    active: bool,
) -> Vec<Geometry> {
    let mut frame = Frame::new(renderer, bounds.size());
    frame.fill_rectangle(
        Point::new(0.0, HIT_HEIGHT * 0.5),
        Size::new(bounds.width, 1.0),
        palette.muted,
    );
    let width = bounds.width.min(40.0);
    frame.fill_rectangle(
        Point::new((bounds.width - width) * 0.5, HIT_HEIGHT * 0.5 - 1.0),
        Size::new(width, 3.0),
        if active {
            palette.foreground
        } else {
            palette.accent
        },
    );
    vec![frame.into_geometry()]
}

impl canvas::Program<Message> for BlockedHandle {
    type State = ();

    fn draw(
        &self,
        _: &(),
        renderer: &Renderer,
        _: &Theme,
        bounds: Rectangle,
        _: mouse::Cursor,
    ) -> Vec<Geometry> {
        draw_grip(renderer, bounds, self.0, false)
    }
}

impl canvas::Program<Message> for Handle {
    type State = Interaction;

    fn update(
        &self,
        state: &mut Interaction,
        event: canvas::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> (canvas::event::Status, Option<Message>) {
        use canvas::event::Status::{Captured, Ignored};
        if state.drag.is_some_and(|drag| !drag.valid(self, bounds)) {
            state.drag = None;
            state.last_press = None;
        }
        if state.last_press.is_some_and(|press| {
            press.id != self.id || press.height != self.height || press.bounds != bounds
        }) {
            state.last_press = None;
        }
        match event {
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
                if cursor.is_over(bounds)
                    && bounded_plot_height(self.id, self.height).is_some() =>
            {
                let pointer = cursor.position().unwrap();
                let now = Instant::now();
                let double = state.last_press.is_some_and(|press| {
                    now.duration_since(press.when) <= Duration::from_millis(350)
                        && press.pointer.distance(pointer) <= 5.0
                });
                if double {
                    state.last_press = None;
                    state.drag = None;
                    return (
                        Captured,
                        Some(Message::Action(Action::ResetPaneHeight(self.id))),
                    );
                }
                state.last_press = Some(Press {
                    id: self.id,
                    when: now,
                    pointer,
                    height: self.height,
                    bounds,
                });
                state.drag = Some(Drag {
                    id: self.id,
                    pointer_y: pointer.y,
                    initial_height: self.height,
                    initial_bounds: bounds,
                    expected_height: self.height,
                });
                (Captured, None)
            }
            canvas::Event::Mouse(mouse::Event::CursorMoved { position }) => {
                let Some(drag) = state.drag.as_mut() else {
                    return (Ignored, None);
                };
                if state
                    .last_press
                    .is_some_and(|press| press.pointer.distance(position) > 5.0)
                {
                    state.last_press = None;
                }
                let Some(height) = drag.height_at(position.y) else {
                    return (Captured, None);
                };
                if height == drag.expected_height {
                    return (Captured, None);
                }
                drag.expected_height = height;
                (
                    Captured,
                    Some(Message::Action(Action::SetPaneHeight {
                        id: self.id,
                        height,
                    })),
                )
            }
            canvas::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                if state.drag.take().is_some() {
                    (Captured, None)
                } else {
                    (Ignored, None)
                }
            }
            canvas::Event::Mouse(mouse::Event::CursorLeft) => {
                // This event means leaving the window, not leaving these bounds.
                // Without a window release event it is unsafe to resume that drag.
                state.last_press = None;
                if state.drag.take().is_some() {
                    (Captured, None)
                } else {
                    (Ignored, None)
                }
            }
            _ => (Ignored, None),
        }
    }

    fn draw(
        &self,
        state: &Interaction,
        renderer: &Renderer,
        _: &Theme,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        draw_grip(
            renderer,
            bounds,
            self.palette,
            cursor.is_over(bounds) || state.drag.is_some_and(|drag| drag.valid(self, bounds)),
        )
    }

    fn mouse_interaction(
        &self,
        state: &Interaction,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        if cursor.is_over(bounds) || state.drag.is_some_and(|drag| drag.valid(self, bounds)) {
            mouse::Interaction::ResizingVertically
        } else {
            mouse::Interaction::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::widget::canvas::Program;

    fn handle(id: Option<u64>, height: f32) -> Handle {
        Handle {
            id,
            height,
            palette: Palette::default(),
        }
    }

    fn bounds(y: f32) -> Rectangle {
        Rectangle {
            x: 25.0,
            y,
            width: 500.0,
            height: HIT_HEIGHT,
        }
    }

    fn input(
        handle: &Handle,
        state: &mut Interaction,
        bounds: Rectangle,
        event: mouse::Event,
        position: Point,
    ) -> (canvas::event::Status, Option<Action>) {
        let (status, message) = handle.update(
            state,
            canvas::Event::Mouse(event),
            bounds,
            mouse::Cursor::Available(position),
        );
        (
            status,
            message.map(|message| match message {
                Message::Action(action) => action,
                _ => panic!("resize handle emitted a non-layout message"),
            }),
        )
    }

    fn press(handle: &Handle, state: &mut Interaction, y: f32) {
        let position = Point::new(100.0, y + 7.0);
        let (status, action) = input(
            handle,
            state,
            bounds(y),
            mouse::Event::ButtonPressed(mouse::Button::Left),
            position,
        );
        assert_eq!(status, canvas::event::Status::Captured);
        assert!(action.is_none());
    }

    #[test]
    fn drag_uses_original_window_pointer_despite_moving_bounds_and_captures_outside_release() {
        let mut state = Interaction::default();
        press(&handle(Some(7), 190.0), &mut state, 400.0);
        for (height, origin, pointer_y, expected) in [
            (190.0, 400.0, 447.0, 230.0),
            (230.0, 440.0, 467.0, 250.0),
            (250.0, 460.0, 407.0, 190.0),
            (190.0, 400.0, 207.0, 100.0),
            (100.0, 310.0, 447.0, 230.0),
        ] {
            let position = Point::new(900.0, pointer_y);
            let (status, action) = input(
                &handle(Some(7), height),
                &mut state,
                bounds(origin),
                mouse::Event::CursorMoved { position },
                position,
            );
            assert_eq!(status, canvas::event::Status::Captured);
            assert!(
                matches!(action, Some(Action::SetPaneHeight { id: Some(7), height }) if height == expected)
            );
        }
        let (status, action) = input(
            &handle(Some(7), 230.0),
            &mut state,
            bounds(440.0),
            mouse::Event::ButtonReleased(mouse::Button::Left),
            Point::new(900.0, 800.0),
        );
        assert_eq!(status, canvas::event::Status::Captured);
        assert!(action.is_none());
        assert!(state.drag.is_none());
    }

    #[test]
    fn stale_instance_height_and_unrelated_layout_changes_cancel_drag() {
        for (replacement, moved_bounds) in [
            (handle(Some(8), 190.0), bounds(400.0)),
            (handle(Some(7), 222.0), bounds(432.0)),
            (handle(Some(7), 190.0), bounds(370.0)),
            (
                handle(Some(7), 190.0),
                Rectangle {
                    width: 300.0,
                    ..bounds(400.0)
                },
            ),
        ] {
            let mut state = Interaction::default();
            press(&handle(Some(7), 190.0), &mut state, 400.0);
            let position = Point::new(100.0, 450.0);
            let (status, action) = input(
                &replacement,
                &mut state,
                moved_bounds,
                mouse::Event::CursorMoved { position },
                position,
            );
            assert_eq!(status, canvas::event::Status::Ignored);
            assert!(action.is_none());
            assert!(state.drag.is_none());
        }
    }

    #[test]
    fn double_click_resets_exact_target_and_real_drag_cancels_double_click() {
        for id in [None, Some(42)] {
            let mut state = Interaction::default();
            let handle = handle(id, 400.0);
            press(&handle, &mut state, 500.0);
            input(
                &handle,
                &mut state,
                bounds(500.0),
                mouse::Event::ButtonReleased(mouse::Button::Left),
                Point::new(100.0, 507.0),
            );
            let (_, action) = input(
                &handle,
                &mut state,
                bounds(500.0),
                mouse::Event::ButtonPressed(mouse::Button::Left),
                Point::new(100.0, 507.0),
            );
            assert!(matches!(action, Some(Action::ResetPaneHeight(target)) if target == id));
            assert!(state.drag.is_none());

            press(&handle, &mut state, 500.0);
            let position = Point::new(100.0, 530.0);
            input(
                &handle,
                &mut state,
                bounds(500.0),
                mouse::Event::CursorMoved { position },
                position,
            );
            assert!(state.last_press.is_none());
        }
    }

    #[test]
    fn blocked_handle_and_window_exit_do_not_resume_resize() {
        let blocked = BlockedHandle(Palette::default());
        let (status, action) = blocked.update(
            &mut (),
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
            bounds(400.0),
            mouse::Cursor::Available(Point::new(100.0, 407.0)),
        );
        assert_eq!(status, canvas::event::Status::Ignored);
        assert!(action.is_none());
        let mut state = Interaction::default();
        let handle = handle(None, 340.0);
        press(&handle, &mut state, 400.0);
        input(
            &handle,
            &mut state,
            bounds(400.0),
            mouse::Event::CursorLeft,
            Point::new(100.0, 407.0),
        );
        let position = Point::new(100.0, 450.0);
        let (_, action) = input(
            &handle,
            &mut state,
            bounds(400.0),
            mouse::Event::CursorMoved { position },
            position,
        );
        assert!(action.is_none());
        assert!(state.drag.is_none());
    }
}
