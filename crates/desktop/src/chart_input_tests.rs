use super::*;
use crate::{
    Desktop,
    drawing_types::{Command, Shape, Style},
    state::Viewport,
};
use iced::widget::canvas::Program;

fn fixture() -> (Desktop, u64, Rectangle) {
    let (mut app, startup) = Desktop::new();
    drop(startup);
    app.chart.merge_stream(mexc_model::Candle {
        time: 300,
        open: 50.0,
        high: 60.0,
        low: 40.0,
        close: 50.0,
        volume: 1.0,
        flow: None,
    });
    app.chart.viewport = Viewport {
        left: 0.0,
        span: 1000.0,
        low: 0.0,
        high: 100.0,
        auto_fit: false,
        live: false,
    };
    app.commit_drawing(Command::Create {
        shape: Shape::Horizontal(Anchor {
            utc_ms: 200_000,
            price: 50.0,
        }),
        style: Style::default(),
    });
    let id = app.drawings.selected().unwrap();
    app.drawings.select(None);
    let bounds = Rectangle::new(
        Point::new(50.0, 70.0),
        Size::new(400.0 + axis_width(&app.chart, "USDC", "base"), 242.0),
    );
    (app, id, bounds)
}

fn event(
    app: &mut Desktop,
    state: &mut Interaction,
    bounds: Rectangle,
    event: canvas::Event,
    cursor: Point,
) -> (canvas::event::Status, mouse::Interaction) {
    let (status, message, interaction) = {
        let chart = Chart {
            data: &app.chart,
            cache: &app.cache,
            palette: Palette::default(),
            indicator: None,
            quote: "USDC",
            volume_unit: "base",
            metrics: None,
            profiles: None,
            profile_range: None,
            drawings: Some(drawing_layer::View {
                store: &app.drawings,
                interaction: &app.drawing_interaction,
                scene: &app.drawing_scene,
                projection: &app.drawing_projection,
                native_candles: app.chart.candles(),
            }),
        };
        let cursor = mouse::Cursor::Available(cursor);
        let (status, message) = chart.update(state, event, bounds, cursor);
        (
            status,
            message,
            chart.mouse_interaction(state, bounds, cursor),
        )
    };
    if let Some(message) = message {
        let _ = app.update(message);
    }
    (status, interaction)
}

fn mouse(
    app: &mut Desktop,
    state: &mut Interaction,
    bounds: Rectangle,
    position: Point,
    input: mouse::Event,
) -> (canvas::event::Status, mouse::Interaction) {
    event(app, state, bounds, canvas::Event::Mouse(input), position)
}

fn space(pressed: bool) -> canvas::Event {
    let key = keyboard::Key::Named(keyboard::key::Named::Space);
    canvas::Event::Keyboard(if pressed {
        keyboard::Event::KeyPressed {
            modified_key: key.clone(),
            key,
            physical_key: keyboard::key::Physical::Code(keyboard::key::Code::Space),
            location: keyboard::Location::Standard,
            modifiers: keyboard::Modifiers::empty(),
            text: None,
        }
    } else {
        keyboard::Event::KeyReleased {
            key,
            location: keyboard::Location::Standard,
            modifiers: keyboard::Modifiers::empty(),
        }
    })
}

fn price(app: &Desktop, id: u64) -> f64 {
    app.drawings.find(id).unwrap().shape.anchors().first().price
}

#[test]
fn anchored_vwap_places_on_native_time_selects_curve_and_drags_only_start_without_pan() {
    let (mut app, line, bounds) = fixture();
    app.commit_drawing(Command::Delete(line));
    app.chart.merge_stream(mexc_model::Candle {
        time: 600,
        open: 70.0,
        high: 70.0,
        low: 70.0,
        close: 70.0,
        volume: 3.0,
        flow: None,
    });
    app.chart.viewport.low = 0.0;
    app.chart.viewport.high = 100.0;
    let view = app.chart.viewport;
    app.drawing_interaction.choose_tool(Tool::AnchoredVwap);
    let mut state = Interaction::default();
    let (status, _) = mouse(
        &mut app,
        &mut state,
        bounds,
        Point::new(181.0, 110.0),
        mouse::Event::ButtonPressed(mouse::Button::Left),
    );
    assert_eq!(status, canvas::event::Status::Captured);
    let id = app.drawings.selected().unwrap();
    assert_eq!(
        app.drawings.find(id).unwrap().shape,
        Shape::AnchoredVwap(Anchor {
            utc_ms: 300_000,
            price: 0.0
        })
    );
    assert_eq!(
        app.drawings.vwap(id).unwrap().points[1].values[0],
        Some(65.0)
    );
    mouse(
        &mut app,
        &mut state,
        bounds,
        Point::new(181.0, 110.0),
        mouse::Event::ButtonReleased(mouse::Button::Left),
    );
    app.drawings.select(None);
    let curve = Point::new(230.0, 155.0);
    let (status, _) = mouse(
        &mut app,
        &mut state,
        bounds,
        curve,
        mouse::Event::ButtonPressed(mouse::Button::Left),
    );
    assert_eq!(status, canvas::event::Status::Captured);
    assert_eq!(app.drawings.selected(), Some(id));
    assert!(!app.drawing_interaction.is_dragging());
    let elsewhere = Point::new(260.0, 230.0);
    mouse(
        &mut app,
        &mut state,
        bounds,
        elsewhere,
        mouse::Event::CursorMoved {
            position: elsewhere,
        },
    );
    mouse(
        &mut app,
        &mut state,
        bounds,
        elsewhere,
        mouse::Event::ButtonReleased(mouse::Button::Left),
    );
    assert_eq!(
        app.drawings
            .find(id)
            .unwrap()
            .shape
            .anchors()
            .first()
            .utc_ms,
        300_000
    );
    let handle = Point::new(170.0, 170.0);
    mouse(
        &mut app,
        &mut state,
        bounds,
        handle,
        mouse::Event::ButtonPressed(mouse::Button::Left),
    );
    assert!(app.drawing_interaction.is_dragging());
    let moved = Point::new(286.0, 240.0);
    mouse(
        &mut app,
        &mut state,
        bounds,
        moved,
        mouse::Event::CursorMoved { position: moved },
    );
    assert_eq!(
        app.drawings
            .find(id)
            .unwrap()
            .shape
            .anchors()
            .first()
            .utc_ms,
        300_000
    );
    assert_eq!(app.drawings.vwap(id).unwrap().points[0].values[0], None);
    mouse(
        &mut app,
        &mut state,
        bounds,
        moved,
        mouse::Event::ButtonReleased(mouse::Button::Left),
    );
    assert_eq!(
        app.drawings.find(id).unwrap().shape,
        Shape::AnchoredVwap(Anchor {
            utc_ms: 600_000,
            price: 0.0
        })
    );
    assert_eq!(
        app.drawings.vwap(id).unwrap().points[1].values[0],
        Some(70.0)
    );
    let generation = app.drawings.generation();
    let _ = app.update(Message::DrawingCommand {
        generation,
        command: Command::Undo,
    });
    assert_eq!(
        app.drawings
            .find(id)
            .unwrap()
            .shape
            .anchors()
            .first()
            .utc_ms,
        300_000
    );
    assert_eq!(
        app.drawings.vwap(id).unwrap().points[1].values[0],
        Some(65.0)
    );
    assert_view_unchanged(&app, view);
}

#[test]
fn first_drawing_press_replaces_old_pan_and_drag_never_navigates_chart() {
    let (mut app, id, bounds) = fixture();
    let mut state = Interaction::default();
    let view = app.chart.viewport;
    mouse(
        &mut app,
        &mut state,
        bounds,
        Point::new(90.0, 110.0),
        mouse::Event::ButtonPressed(mouse::Button::Left),
    );
    // A new press must be hit-tested even if the previous release was lost.
    let start = Point::new(250.0, 170.0);
    let (status, cursor) = mouse(
        &mut app,
        &mut state,
        bounds,
        start,
        mouse::Event::ButtonPressed(mouse::Button::Left),
    );
    assert_eq!(status, canvas::event::Status::Captured);
    assert_eq!(cursor, mouse::Interaction::Grabbing);
    assert_eq!(app.drawings.selected(), Some(id));
    let moved = Point::new(250.0, 210.0);
    mouse(
        &mut app,
        &mut state,
        bounds,
        moved,
        mouse::Event::CursorMoved { position: moved },
    );
    mouse(
        &mut app,
        &mut state,
        bounds,
        moved,
        mouse::Event::ButtonReleased(mouse::Button::Left),
    );
    assert!((price(&app, id) - 30.0).abs() < 1e-9);
    assert_view_unchanged(&app, view);
}

#[test]
fn drawing_drag_keeps_its_press_frame_when_layout_and_live_scale_change() {
    let (mut app, id, bounds) = fixture();
    let mut state = Interaction::default();
    let start = Point::new(250.0, 170.0);
    mouse(
        &mut app,
        &mut state,
        bounds,
        start,
        mouse::Event::CursorMoved { position: start },
    );
    assert!(app.chart.inspection.is_some());
    mouse(
        &mut app,
        &mut state,
        bounds,
        Point::new(250.0, 170.0),
        mouse::Event::ButtonPressed(mouse::Button::Left),
    );
    assert!(app.chart.inspection.is_none());
    app.chart.viewport.left = 100.0;
    app.chart.viewport.span = 2000.0;
    app.chart.viewport.low = -100.0;
    app.chart.viewport.high = 200.0;
    let view = app.chart.viewport;
    let resized = Rectangle::new(
        Point::new(50.0, 110.0),
        Size::new(bounds.width + 200.0, 342.0),
    );
    let moved = Point::new(290.0, 190.0);
    event(
        &mut app,
        &mut state,
        resized,
        canvas::Event::Mouse(mouse::Event::CursorMoved { position: moved }),
        moved,
    );
    mouse(
        &mut app,
        &mut state,
        resized,
        moved,
        mouse::Event::ButtonReleased(mouse::Button::Left),
    );
    assert!((price(&app, id) - 40.0).abs() < 1e-9);
    assert_view_unchanged(&app, view);
}

#[test]
fn space_during_a_drawing_drag_does_not_transfer_ownership_or_cursor_to_pan() {
    let (mut app, id, bounds) = fixture();
    let mut state = Interaction::default();
    let view = app.chart.viewport;
    let start = Point::new(250.0, 170.0);
    mouse(
        &mut app,
        &mut state,
        bounds,
        start,
        mouse::Event::ButtonPressed(mouse::Button::Left),
    );
    let (_, cursor) = event(&mut app, &mut state, bounds, space(true), start);
    assert_eq!(cursor, mouse::Interaction::Grabbing);
    let moved = Point::new(250.0, 190.0);
    mouse(
        &mut app,
        &mut state,
        bounds,
        moved,
        mouse::Event::CursorMoved { position: moved },
    );
    mouse(
        &mut app,
        &mut state,
        bounds,
        moved,
        mouse::Event::ButtonReleased(mouse::Button::Left),
    );
    assert!((price(&app, id) - 40.0).abs() < 1e-9);
    assert_view_unchanged(&app, view);
}

#[test]
fn space_before_the_press_pans_instead_of_moving_or_deselecting_the_drawing() {
    let (mut app, id, bounds) = fixture();
    app.drawings.select(Some(id));
    let mut state = Interaction::default();
    let start = Point::new(250.0, 170.0);
    event(&mut app, &mut state, bounds, space(true), start);
    mouse(
        &mut app,
        &mut state,
        bounds,
        start,
        mouse::Event::ButtonPressed(mouse::Button::Left),
    );
    let moved = Point::new(290.0, 170.0);
    mouse(
        &mut app,
        &mut state,
        bounds,
        moved,
        mouse::Event::CursorMoved { position: moved },
    );
    mouse(
        &mut app,
        &mut state,
        bounds,
        moved,
        mouse::Event::ButtonReleased(mouse::Button::Left),
    );
    assert_eq!(price(&app, id), 50.0);
    assert_eq!(app.drawings.selected(), Some(id));
    assert!((app.chart.viewport.left + 100.0).abs() < 1e-9);
}

#[test]
fn locked_drawing_claims_the_press_without_promising_resize_or_panning_behind_it() {
    let (mut app, id, bounds) = fixture();
    app.commit_drawing(Command::SetLocked { id, locked: true });
    app.drawings.select(None);
    let mut state = Interaction::default();
    let view = app.chart.viewport;
    let start = Point::new(250.0, 170.0);
    let (_, cursor) = mouse(
        &mut app,
        &mut state,
        bounds,
        start,
        mouse::Event::CursorMoved { position: start },
    );
    assert_eq!(cursor, mouse::Interaction::NotAllowed);
    let (_, cursor) = mouse(
        &mut app,
        &mut state,
        bounds,
        start,
        mouse::Event::ButtonPressed(mouse::Button::Left),
    );
    assert_eq!(cursor, mouse::Interaction::NotAllowed);
    let moved = Point::new(310.0, 210.0);
    mouse(
        &mut app,
        &mut state,
        bounds,
        moved,
        mouse::Event::CursorMoved { position: moved },
    );
    assert_eq!(app.drawing_interaction.hover().map(|hit| hit.id), Some(id));
    mouse(
        &mut app,
        &mut state,
        bounds,
        moved,
        mouse::Event::ButtonReleased(mouse::Button::Left),
    );
    assert_eq!(price(&app, id), 50.0);
    assert_view_unchanged(&app, view);
    assert_eq!(app.drawings.selected(), Some(id));
}

#[test]
fn wheel_navigation_cannot_change_the_coordinate_frame_of_a_held_drawing() {
    let (mut app, id, bounds) = fixture();
    let mut state = Interaction::default();
    let view = app.chart.viewport;
    let start = Point::new(250.0, 170.0);
    mouse(
        &mut app,
        &mut state,
        bounds,
        start,
        mouse::Event::ButtonPressed(mouse::Button::Left),
    );
    mouse(
        &mut app,
        &mut state,
        bounds,
        start,
        mouse::Event::WheelScrolled {
            delta: mouse::ScrollDelta::Lines { x: 0.0, y: 3.0 },
        },
    );
    mouse(
        &mut app,
        &mut state,
        bounds,
        start,
        mouse::Event::ButtonReleased(mouse::Button::Left),
    );
    assert_view_unchanged(&app, view);
    assert_eq!(price(&app, id), 50.0);
}

#[test]
fn chart_pan_measures_physical_motion_not_a_changed_canvas_origin() {
    let (mut app, _, bounds) = fixture();
    let mut state = Interaction::default();
    let start = Point::new(250.0, 110.0);
    mouse(
        &mut app,
        &mut state,
        bounds,
        start,
        mouse::Event::ButtonPressed(mouse::Button::Left),
    );
    let shifted = Rectangle {
        x: bounds.x - 50.0,
        y: bounds.y - 20.0,
        ..bounds
    };
    let moved = Point::new(290.0, 110.0);
    mouse(
        &mut app,
        &mut state,
        shifted,
        moved,
        mouse::Event::CursorMoved { position: moved },
    );
    mouse(
        &mut app,
        &mut state,
        shifted,
        moved,
        mouse::Event::ButtonReleased(mouse::Button::Left),
    );
    assert!((app.chart.viewport.left + 100.0).abs() < 1e-9);
    assert_eq!(
        (app.chart.viewport.low, app.chart.viewport.high),
        (0.0, 100.0)
    );
}

#[test]
fn leaving_the_window_cancels_lower_pane_drag_instead_of_resuming_it_on_return() {
    let mut data = super::tests::lower_state();
    let mut state = Interaction::default();
    let start = Point::new(100.0, 100.0);
    let action = super::tests::input(
        &data,
        &mut state,
        start,
        mouse::Event::ButtonPressed(mouse::Button::Left),
    )
    .unwrap();
    data.dispatch(action);
    let action = super::tests::input(&data, &mut state, start, mouse::Event::CursorLeft).unwrap();
    data.dispatch(action);
    let returned = Point::new(150.0, 130.0);
    assert!(matches!(
        super::tests::input(
            &data,
            &mut state,
            returned,
            mouse::Event::CursorMoved { position: returned }
        ),
        Some(Action::InspectPane { .. })
    ));
}

#[test]
fn escape_ends_a_pan_before_later_pointer_motion() {
    let (mut app, _, bounds) = fixture();
    let mut state = Interaction::default();
    let start = Point::new(250.0, 110.0);
    mouse(
        &mut app,
        &mut state,
        bounds,
        start,
        mouse::Event::ButtonPressed(mouse::Button::Left),
    );
    let moved = Point::new(290.0, 110.0);
    mouse(
        &mut app,
        &mut state,
        bounds,
        moved,
        mouse::Event::CursorMoved { position: moved },
    );
    let view = app.chart.viewport;
    let key = keyboard::Key::Named(keyboard::key::Named::Escape);
    event(
        &mut app,
        &mut state,
        bounds,
        canvas::Event::Keyboard(keyboard::Event::KeyPressed {
            modified_key: key.clone(),
            key,
            physical_key: keyboard::key::Physical::Code(keyboard::key::Code::Escape),
            location: keyboard::Location::Standard,
            modifiers: keyboard::Modifiers::empty(),
            text: None,
        }),
        moved,
    );
    let returned = Point::new(340.0, 130.0);
    mouse(
        &mut app,
        &mut state,
        bounds,
        returned,
        mouse::Event::CursorMoved { position: returned },
    );
    assert_view_unchanged(&app, view);
}

fn assert_view_unchanged(app: &Desktop, view: Viewport) {
    let actual = app.chart.viewport;
    assert_eq!(
        (
            actual.left,
            actual.span,
            actual.low,
            actual.high,
            actual.auto_fit,
            actual.live
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
}

#[test]
fn a_new_chart_command_cancels_preview_instead_of_committing_a_stale_drag_afterward() {
    let (mut app, id, bounds) = fixture();
    let mut state = Interaction::default();
    let start = Point::new(250.0, 170.0);
    mouse(
        &mut app,
        &mut state,
        bounds,
        start,
        mouse::Event::ButtonPressed(mouse::Button::Left),
    );
    let moved = Point::new(250.0, 210.0);
    mouse(
        &mut app,
        &mut state,
        bounds,
        moved,
        mouse::Event::CursorMoved { position: moved },
    );
    let _ = app.update(Message::Action(Action::AutoFit));
    mouse(
        &mut app,
        &mut state,
        bounds,
        moved,
        mouse::Event::ButtonReleased(mouse::Button::Left),
    );
    assert_eq!(price(&app, id), 50.0);
    assert!(app.chart.viewport.auto_fit);
    assert!(!app.drawing_interaction.is_dragging());
}

fn surface(app: &Desktop, toolbar: bool) -> Element<'_, Message> {
    view(
        Chart {
            data: &app.chart,
            cache: &app.cache,
            palette: Palette::default(),
            indicator: None,
            quote: "USDC",
            volume_unit: "base",
            metrics: None,
            profiles: None,
            profile_range: None,
            drawings: Some(drawing_layer::View {
                store: &app.drawings,
                interaction: &app.drawing_interaction,
                scene: &app.drawing_scene,
                projection: &app.drawing_projection,
                native_candles: app.chart.candles(),
            }),
        },
        Length::Fill,
        false,
        toolbar.then(|| app.drawing_controls.context_style_view(&app.drawings)),
        app.pointer_generation,
    )
}

#[test]
fn surface_keeps_a_drag_when_chrome_appears_but_cancels_held_input_on_focus_epoch_change() {
    use iced::advanced::widget::Tree;
    let (mut app, id, bounds) = fixture();
    let initial = surface(&app, false);
    let mut tree = Tree::new(initial.as_widget());
    drop(initial);
    let start = Point::new(250.0, 170.0);
    mouse(
        &mut app,
        tree.children[0].state.downcast_mut::<Interaction>(),
        bounds,
        start,
        mouse::Event::ButtonPressed(mouse::Button::Left),
    );
    let selected = surface(&app, true);
    tree.diff(selected.as_widget());
    drop(selected);
    let moved = Point::new(250.0, 210.0);
    mouse(
        &mut app,
        tree.children[0].state.downcast_mut::<Interaction>(),
        bounds,
        moved,
        mouse::Event::CursorMoved { position: moved },
    );
    mouse(
        &mut app,
        tree.children[0].state.downcast_mut::<Interaction>(),
        bounds,
        moved,
        mouse::Event::ButtonReleased(mouse::Button::Left),
    );
    assert!((price(&app, id) - 30.0).abs() < 1e-9);
    let unchanged = app.chart.viewport;

    event(
        &mut app,
        tree.children[0].state.downcast_mut::<Interaction>(),
        bounds,
        space(true),
        moved,
    );
    mouse(
        &mut app,
        tree.children[0].state.downcast_mut::<Interaction>(),
        bounds,
        moved,
        mouse::Event::ButtonPressed(mouse::Button::Left),
    );
    let _ = app.update(Message::PointerCancelled);
    let refocused = surface(&app, true);
    tree.diff(refocused.as_widget());
    drop(refocused);
    let after_focus = Point::new(270.0, 210.0);
    mouse(
        &mut app,
        tree.children[0].state.downcast_mut::<Interaction>(),
        bounds,
        after_focus,
        mouse::Event::CursorMoved {
            position: after_focus,
        },
    );
    assert_view_unchanged(&app, unchanged);

    mouse(
        &mut app,
        tree.children[0].state.downcast_mut::<Interaction>(),
        bounds,
        moved,
        mouse::Event::ButtonPressed(mouse::Button::Left),
    );
    let end = Point::new(250.0, 190.0);
    mouse(
        &mut app,
        tree.children[0].state.downcast_mut::<Interaction>(),
        bounds,
        end,
        mouse::Event::CursorMoved { position: end },
    );
    mouse(
        &mut app,
        tree.children[0].state.downcast_mut::<Interaction>(),
        bounds,
        end,
        mouse::Event::ButtonReleased(mouse::Button::Left),
    );
    assert!((price(&app, id) - 40.0).abs() < 1e-9);
    assert_view_unchanged(&app, unchanged);
}

#[test]
fn scrolled_cursor_keeps_crosshair_hover_and_edit_preview_at_the_physical_pointer() {
    let (mut app, id, bounds) = fixture();
    let mut state = Interaction::default();
    let scroll_y = 60.0;
    let physical = Point::new(250.0, 110.0);
    let routed = Point::new(physical.x, physical.y + scroll_y);
    let view = app.chart.viewport;
    event(
        &mut app,
        &mut state,
        bounds,
        canvas::Event::Mouse(mouse::Event::CursorMoved { position: physical }),
        routed,
    );
    let (_, inspected_price) = app.chart.inspection.unwrap();
    let projection = Projection::new(view, Rectangle::with_size(Size::new(400.0, 200.0))).unwrap();
    let screen_y = projection
        .point(Anchor {
            utc_ms: 300_000,
            price: inspected_price,
        })
        .y
        + bounds.y
        - scroll_y;
    assert!((screen_y - physical.y).abs() < 1e-5);
    assert_eq!(app.drawing_interaction.hover().map(|hit| hit.id), Some(id));

    mouse(
        &mut app,
        &mut state,
        bounds,
        routed,
        mouse::Event::ButtonPressed(mouse::Button::Left),
    );
    let physical = Point::new(250.0, 130.0);
    let routed = Point::new(physical.x, physical.y + scroll_y);
    event(
        &mut app,
        &mut state,
        bounds,
        canvas::Event::Mouse(mouse::Event::CursorMoved { position: physical }),
        routed,
    );
    let Some(crate::drawing_types::Preview::Edit { anchors, .. }) = app
        .drawing_interaction
        .preview(&app.drawings, projection, None)
    else {
        panic!("missing dragged drawing preview");
    };
    assert!((anchors.first().price - 40.0).abs() < 1e-9);
    mouse(
        &mut app,
        &mut state,
        bounds,
        routed,
        mouse::Event::ButtonReleased(mouse::Button::Left),
    );
    assert!((price(&app, id) - 40.0).abs() < 1e-9);
    assert_view_unchanged(&app, view);
}

#[test]
fn a_clipped_chart_cannot_steal_inspection_using_a_raw_mouse_event() {
    let (mut app, _, bounds) = fixture();
    app.chart
        .dispatch(Action::AddIndicator(IndicatorSpec::Rsi { period: 14 }));
    let pane = app.chart.indicators.instances[0].id;
    app.chart.dispatch(Action::InspectPane {
        id: Some(pane),
        point: Some((300.0, 73.0)),
    });
    let inspection = app.chart.inspection;
    let mut state = Interaction::default();
    let message = {
        let chart = Chart {
            data: &app.chart,
            cache: &app.cache,
            palette: Palette::default(),
            indicator: None,
            quote: "USDC",
            volume_unit: "base",
            metrics: None,
            profiles: None,
            profile_range: None,
            drawings: Some(drawing_layer::View {
                store: &app.drawings,
                interaction: &app.drawing_interaction,
                scene: &app.drawing_scene,
                projection: &app.drawing_projection,
                native_candles: app.chart.candles(),
            }),
        };
        chart
            .update(
                &mut state,
                canvas::Event::Mouse(mouse::Event::CursorMoved {
                    position: Point::new(250.0, 170.0),
                }),
                bounds,
                mouse::Cursor::Unavailable,
            )
            .1
    };
    if let Some(message) = message {
        let _ = app.update(message);
    }
    assert_eq!(app.chart.active_pane, Some(pane));
    assert_eq!(app.chart.inspection, inspection);
}

#[test]
fn profile_range_uses_content_cursor_and_does_not_claim_axes_or_held_pan() {
    let (app, _, bounds) = fixture();
    let chart = Chart {
        data: &app.chart,
        cache: &app.cache,
        palette: Palette::default(),
        indicator: None,
        quote: "USDC",
        volume_unit: "base",
        metrics: None,
        profiles: None,
        profile_range: Some(profile_layer::RangePlacement {
            first: None,
            cursor_ms: None,
        }),
        drawings: None,
    };
    let mut state = Interaction::default();
    let cursor = mouse::Cursor::Available(Point::new(bounds.x + 100.0, bounds.y + 60.0));
    let (status, message) = chart.update(
        &mut state,
        canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
        bounds,
        cursor,
    );
    assert_eq!(status, canvas::event::Status::Captured);
    assert!(matches!(
        message,
        Some(Message::Profile(
            crate::profile_controls::Event::RangePoint(250_000)
        ))
    ));
    assert!(!state.owns_pointer(None));

    let axis = mouse::Cursor::Available(Point::new(bounds.x + bounds.width - 5.0, bounds.y + 60.0));
    let (_, message) = chart.update(
        &mut state,
        canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
        bounds,
        axis,
    );
    assert!(!matches!(message, Some(Message::Profile(_))));
    assert!(state.owns_pointer(None));
    let (_, message) = chart.update(
        &mut state,
        canvas::Event::Mouse(mouse::Event::CursorMoved {
            position: Point::new(9999.0, 9999.0),
        }),
        bounds,
        cursor,
    );
    assert!(matches!(
        message,
        Some(Message::Action(Action::Zoom { .. }))
    ));
    let (_, message) = chart.update(
        &mut state,
        canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
        bounds,
        cursor,
    );
    assert!(!matches!(message, Some(Message::Profile(_))));
}

#[test]
fn arming_profile_range_does_not_replace_a_drawing_pointer_claim() {
    let (app, _, bounds) = fixture();
    let chart = Chart {
        data: &app.chart,
        cache: &app.cache,
        palette: Palette::default(),
        indicator: None,
        quote: "USDC",
        volume_unit: "base",
        metrics: None,
        profiles: None,
        profile_range: None,
        drawings: Some(drawing_layer::View {
            store: &app.drawings,
            interaction: &app.drawing_interaction,
            scene: &app.drawing_scene,
            projection: &app.drawing_projection,
            native_candles: app.chart.candles(),
        }),
    };
    let mut state = Interaction::default();
    let cursor = mouse::Cursor::Available(Point::new(bounds.x + 200.0, bounds.y + 100.0));
    let (_, message) = chart.update(
        &mut state,
        canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
        bounds,
        cursor,
    );
    assert!(matches!(message, Some(Message::DrawingInput { .. })));
    assert!(state.owns_pointer(Some(app.drawing_interaction.generation())));
    let chart = Chart {
        profile_range: Some(profile_layer::RangePlacement {
            first: None,
            cursor_ms: None,
        }),
        ..chart
    };
    let (_, message) = chart.update(
        &mut state,
        canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
        bounds,
        cursor,
    );
    assert!(matches!(message, Some(Message::DrawingInput { .. })));
}
