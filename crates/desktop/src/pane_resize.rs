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
    requested_height: f32,
}

impl Drag {
    fn valid(&self, handle: &Handle, bounds: Rectangle) -> bool {
        // The presented view can lag any number of pointer messages. Its height
        // need not equal requested_height; its layout must still match that height.
        self.id == handle.id
            && (bounds.x - self.initial_bounds.x).abs() <= GEOMETRY_TOLERANCE
            && (bounds.width - self.initial_bounds.width).abs() <= GEOMETRY_TOLERANCE
            && (bounds.height - self.initial_bounds.height).abs() <= GEOMETRY_TOLERANCE
            && (bounds.y - self.initial_bounds.y - (handle.height - self.initial_height)).abs()
                <= GEOMETRY_TOLERANCE
    }

    fn height_at(&self, pointer_y: f32) -> Option<f32> {
        // Both positions use the routed content coordinates of the scrollable.
        // Local bounds move while the plot grows; do not feed that motion back in.
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
    input_generation: u64,
) -> Element<'a, Message> {
    let canvas = if blocked {
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
    };
    crate::chart_surface::view(
        canvas,
        None,
        Length::Fixed(HIT_HEIGHT),
        blocked,
        None,
        (input_generation, id),
    )
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
                    requested_height: self.height,
                });
                (Captured, None)
            }
            canvas::Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                let Some(drag) = state.drag.as_mut() else {
                    return (Ignored, None);
                };
                let Some(position) = cursor.position() else {
                    return (Captured, None);
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
                if height == drag.requested_height {
                    return (Captured, None);
                }
                drag.requested_height = height;
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
    fn drag_uses_original_content_pointer_despite_moving_bounds_and_captures_outside_release() {
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
    fn motion_bursts_continue_before_render_and_through_partial_view_catch_up() {
        for (id, initial_height) in [(None, 340.0), (Some(7), 190.0)] {
            let mut state = Interaction::default();
            let initial = handle(id, initial_height);
            press(&initial, &mut state, 400.0);

            // iced can deliver the whole burst to one view before applying messages.
            for delta in (2..=40).step_by(2) {
                let position = Point::new(900.0, 407.0 + delta as f32);
                let (status, action) = input(
                    &initial,
                    &mut state,
                    bounds(400.0),
                    mouse::Event::CursorMoved { position },
                    position,
                );
                assert_eq!(status, canvas::event::Status::Captured);
                assert!(
                    matches!(action, Some(Action::SetPaneHeight { id: target, height })
                        if target == id && height == initial_height + delta as f32)
                );
                assert_eq!(
                    initial.mouse_interaction(&state, bounds(400.0), mouse::Cursor::Unavailable),
                    mouse::Interaction::ResizingVertically,
                );
            }

            // Intermediate views, and a view skipping multiple acknowledgements,
            // must not re-anchor the pointer or require the latest requested height.
            for (presented_delta, pointer_delta) in
                [(8.0, 44.0), (24.0, 48.0), (40.0, 52.0), (52.0, 56.0)]
            {
                let position = Point::new(900.0, 407.0 + pointer_delta);
                let (status, action) = input(
                    &handle(id, initial_height + presented_delta),
                    &mut state,
                    bounds(400.0 + presented_delta),
                    mouse::Event::CursorMoved { position },
                    position,
                );
                assert_eq!(status, canvas::event::Status::Captured);
                assert!(
                    matches!(action, Some(Action::SetPaneHeight { id: target, height })
                        if target == id && height == initial_height + pointer_delta)
                );
            }

            let caught_up = handle(id, initial_height + 56.0);
            for delta in [60.0, 54.0, 70.0, 56.0, 72.0] {
                let position = Point::new(900.0, 407.0 + delta);
                let (status, action) = input(
                    &caught_up,
                    &mut state,
                    bounds(456.0),
                    mouse::Event::CursorMoved { position },
                    position,
                );
                assert_eq!(status, canvas::event::Status::Captured);
                assert!(
                    matches!(action, Some(Action::SetPaneHeight { id: target, height })
                        if target == id && height == initial_height + delta)
                );
            }
            let (status, action) = input(
                &handle(id, initial_height + 72.0),
                &mut state,
                bounds(472.0),
                mouse::Event::ButtonReleased(mouse::Button::Left),
                Point::new(900.0, 900.0),
            );
            assert_eq!(status, canvas::event::Status::Captured);
            assert!(action.is_none());
            let position = Point::new(100.0, 500.0);
            let (_, action) = input(
                &handle(id, initial_height + 72.0),
                &mut state,
                bounds(472.0),
                mouse::Event::CursorMoved { position },
                position,
            );
            assert!(action.is_none());
        }
    }

    #[test]
    fn scrolled_drag_uses_routed_motion_and_ignores_unavailable_cursor() {
        let mut state = Interaction::default();
        let initial = handle(Some(7), 190.0);
        press(&initial, &mut state, 800.0);
        let scroll_offset = 450.0;
        for delta in [3.0, 7.0, 19.0, 37.0] {
            let position = Point::new(100.0, 807.0 + delta);
            let raw_position = Point::new(position.x, position.y - scroll_offset);
            let (status, action) = input(
                &initial,
                &mut state,
                bounds(800.0),
                mouse::Event::CursorMoved {
                    position: raw_position,
                },
                position,
            );
            assert_eq!(status, canvas::event::Status::Captured);
            assert!(
                matches!(action, Some(Action::SetPaneHeight { id: Some(7), height })
                    if height == 190.0 + delta)
            );
        }

        let caught_up = handle(Some(7), 227.0);
        let (status, action) = caught_up.update(
            &mut state,
            canvas::Event::Mouse(mouse::Event::CursorMoved {
                position: Point::new(100.0, 600.0),
            }),
            bounds(837.0),
            mouse::Cursor::Unavailable,
        );
        assert_eq!(status, canvas::event::Status::Captured);
        assert!(action.is_none());

        let position = Point::new(100.0, 862.0);
        let (status, action) = input(
            &caught_up,
            &mut state,
            bounds(837.0),
            mouse::Event::CursorMoved {
                position: Point::new(position.x, position.y - scroll_offset),
            },
            position,
        );
        assert_eq!(status, canvas::event::Status::Captured);
        assert!(matches!(
            action,
            Some(Action::SetPaneHeight {
                id: Some(7),
                height: 245.0
            })
        ));
        let (status, action) = caught_up.update(
            &mut state,
            canvas::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
            bounds(837.0),
            mouse::Cursor::Unavailable,
        );
        assert_eq!(status, canvas::event::Status::Captured);
        assert!(action.is_none());
        assert!(state.drag.is_none());
    }

    #[test]
    fn bounded_heights_remain_anchored_during_bursts_and_after_view_catch_up() {
        for (id, initial_height, minimum, maximum) in
            [(None, 340.0, 160.0, 1000.0), (Some(7), 190.0, 100.0, 800.0)]
        {
            let mut state = Interaction::default();
            press(&handle(id, initial_height), &mut state, 400.0);
            for (presented_height, delta, expected) in [
                (initial_height, 1000.0, Some(maximum)),
                (initial_height, 1100.0, None),
                (initial_height, 1200.0, None),
                (maximum, 10.0, Some(initial_height + 10.0)),
                (initial_height + 10.0, -1000.0, Some(minimum)),
                (initial_height + 10.0, -1100.0, None),
                (minimum, 0.0, Some(initial_height)),
            ] {
                let position = Point::new(100.0, 407.0 + delta);
                let (status, action) = input(
                    &handle(id, presented_height),
                    &mut state,
                    bounds(400.0 + presented_height - initial_height),
                    mouse::Event::CursorMoved { position },
                    position,
                );
                assert_eq!(status, canvas::event::Status::Captured);
                match expected {
                    Some(expected) => assert!(
                        matches!(action, Some(Action::SetPaneHeight { id: target, height })
                            if target == id && height == expected)
                    ),
                    None => assert!(action.is_none()),
                }
            }
        }
    }

    #[test]
    fn stale_instance_and_unrelated_layout_changes_cancel_drag() {
        for (replacement, moved_bounds) in [
            (handle(Some(8), 190.0), bounds(400.0)),
            (handle(Some(7), 222.0), bounds(400.0)),
            (handle(Some(7), 190.0), bounds(370.0)),
            (
                handle(Some(7), 190.0),
                Rectangle {
                    width: 300.0,
                    ..bounds(400.0)
                },
            ),
            (
                handle(Some(7), 190.0),
                Rectangle {
                    x: 40.0,
                    ..bounds(400.0)
                },
            ),
            (
                handle(Some(7), 190.0),
                Rectangle {
                    height: 20.0,
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

    #[test]
    fn pointer_updates_keep_capture_but_keyboard_resize_ends_the_old_gesture() {
        use iced::advanced::widget::Tree;
        let (mut app, startup) = crate::Desktop::new();
        drop(startup);
        let palette = Palette::default();
        let initial_height = app.chart.plot_height(None);
        let initial = view(None, initial_height, palette, false, app.pointer_generation);
        let mut tree = Tree::new(initial.as_widget());
        drop(initial);
        press(
            &handle(None, initial_height),
            tree.children[0].state.downcast_mut::<Interaction>(),
            400.0,
        );
        let position = Point::new(100.0, 447.0);
        let (_, action) = input(
            &handle(None, initial_height),
            tree.children[0].state.downcast_mut::<Interaction>(),
            bounds(400.0),
            mouse::Event::CursorMoved { position },
            position,
        );
        let _ = app.update(Message::Action(action.unwrap()));
        let height = app.chart.plot_height(None);
        let acknowledged = view(None, height, palette, false, app.pointer_generation);
        tree.diff(acknowledged.as_widget());
        drop(acknowledged);
        let position = Point::new(100.0, 467.0);
        let (_, action) = input(
            &handle(None, height),
            tree.children[0].state.downcast_mut::<Interaction>(),
            bounds(400.0 + height - initial_height),
            mouse::Event::CursorMoved { position },
            position,
        );
        let _ = app.update(Message::Action(action.unwrap()));
        assert_eq!(app.chart.plot_height(None), initial_height + 60.0);

        let _ = app.update(Message::Action(Action::AdjustPaneHeight {
            id: None,
            delta: 20.0,
        }));
        let height = app.chart.plot_height(None);
        assert_eq!(height, initial_height + 80.0);
        let interrupted = view(None, height, palette, false, app.pointer_generation);
        tree.diff(interrupted.as_widget());
        drop(interrupted);
        let position = Point::new(100.0, 487.0);
        let (_, action) = input(
            &handle(None, height),
            tree.children[0].state.downcast_mut::<Interaction>(),
            bounds(400.0 + height - initial_height),
            mouse::Event::CursorMoved { position },
            position,
        );
        if let Some(action) = action {
            let _ = app.update(Message::Action(action));
        }
        assert_eq!(app.chart.plot_height(None), height);
    }
}
