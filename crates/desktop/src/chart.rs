use crate::{
    Message,
    drawing_geometry::{self, Projection},
    drawing_interaction::Input as DrawingInput,
    drawing_layer,
    drawing_types::{Anchor, Tool},
    palette::Palette,
    presentation::{self, TimeTicks, ValueTicks, quantity, quantity_exact},
    state::{Action, ChartState},
};
use iced::widget::canvas::{Cache, Frame, Geometry, Path, Stroke, Text};
use iced::widget::{Column, Row, button, canvas, column, mouse_area, row, text};
use iced::{
    Alignment, Color, Element, Length, Point, Rectangle, Renderer, Size, Theme, keyboard, mouse,
};
use mexc_indicators::{IndicatorInstance, IndicatorPoint, IndicatorSpec};
use mexc_model::{FlowQuality, OpenInterestSample, OpenInterestTimestamp, Product};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub struct Chart<'a> {
    pub data: &'a ChartState,
    pub cache: &'a Cache,
    pub palette: Palette,
    pub indicator: Option<&'a IndicatorInstance>,
    pub quote: &'a str,
    pub volume_unit: &'a str,
    pub drawings: Option<drawing_layer::View<'a>>,
}

#[derive(Default)]
pub struct Interaction {
    drag: Option<(Point, Region, (f64, f64))>,
    last_press: Option<(Instant, Point, Region)>,
    modifiers: keyboard::Modifiers,
    space_pan: bool,
    drawing_claim: Option<u64>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Region {
    Plot,
    TimeAxis,
    PriceAxis,
}

fn region(point: Point, area: Rectangle) -> Region {
    if point.y > area.height {
        Region::TimeAxis
    } else if point.x > area.width {
        Region::PriceAxis
    } else {
        Region::Plot
    }
}

fn plot(size: Size, axis_width: f32) -> Rectangle {
    Rectangle {
        x: 0.0,
        y: 0.0,
        width: (size.width - axis_width).max(1.0),
        height: (size.height - 42.0).max(1.0),
    }
}

pub fn utc(time: i64, format: &str) -> String {
    chrono::DateTime::from_timestamp(time, 0)
        .map(|time| time.format(format).to_string())
        .unwrap_or_else(|| "—".into())
}
fn utc_ms(time: i64) -> String {
    chrono::DateTime::from_timestamp_millis(time)
        .map(|time| time.format("%Y-%m-%d %H:%M:%S%.3f UTC").to_string())
        .unwrap_or_else(|| "—".into())
}
fn open_interest_time(sample: &OpenInterestSample) -> String {
    let kind = match sample.timestamp_source {
        OpenInterestTimestamp::Venue => "venue as-of",
        OpenInterestTimestamp::Observed => "observed",
    };
    format!("{kind} {}", utc_ms(sample.timestamp_ms))
}

fn open_interest_status(data: &ChartState) -> String {
    if data.market.product == Product::Spot {
        return "Unsupported: open interest is only available for perpetual markets".into();
    }
    let native = &data.open_interest;
    if !native.message.is_empty() {
        let message = native
            .support
            .as_ref()
            .and_then(|support| native.message.strip_prefix(&support.description))
            .unwrap_or(&native.message)
            .trim_start_matches([' ', '·']);
        let status = if native.connected { "LIVE" } else { "OFFLINE" };
        return if message.is_empty() {
            format!("{status} · native open-interest observations")
        } else {
            format!("{status} · {message}")
        };
    }
    if let Some(support) = &native.support {
        if support.live_interval_ms.is_none() && support.history_interval_ms.is_none() {
            return support.description.clone();
        }
        if native.samples.is_empty() {
            return "No native open-interest observations received".into();
        }
        if native.connected {
            "Live native observations".into()
        } else {
            "Live disconnected · retained native observations".into()
        }
    } else {
        "Waiting for native open-interest support".into()
    }
}

fn open_interest_contiguous(before: &OpenInterestSample, after: &OpenInterestSample) -> bool {
    let cadence = before.interval_ms.max(after.interval_ms);
    cadence > 0
        && after.timestamp_ms > before.timestamp_ms
        && after.timestamp_ms.saturating_sub(before.timestamp_ms)
            <= cadence.saturating_add(cadence / 2)
}

fn instance_unit<'a>(
    data: &'a ChartState,
    instance: &'a IndicatorInstance,
    volume_unit: &'a str,
) -> &'a str {
    if !instance.source.is_chart() {
        return instance
            .external
            .as_ref()
            .map_or(data.base_asset.as_str(), |output| output.unit.as_str());
    }
    if instance.spec == IndicatorSpec::OpenInterest {
        data.open_interest
            .support
            .as_ref()
            .map_or("native", |support| support.unit.as_str())
    } else {
        volume_unit
    }
}

fn oi_segment_contiguous(
    instance: &IndicatorInstance,
    before: &OpenInterestSample,
    after: &OpenInterestSample,
) -> bool {
    // An external zero cadence marks a proven segment restart at a real observation,
    // never a fabricated zero quantity or intermediate timestamp.
    (instance.source.is_chart() || after.interval_ms > 0) && open_interest_contiguous(before, after)
}

fn line(frame: &mut Frame, a: Point, b: Point, color: Color, width: f32) {
    frame.stroke(
        &Path::line(a, b),
        Stroke::default().with_color(color).with_width(width),
    );
}

fn label(frame: &mut Frame, content: String, position: Point, color: Color, size: f32) {
    frame.fill_text(Text {
        content,
        position,
        color,
        size: size.into(),
        ..Text::default()
    });
}

fn axis_label(frame: &mut Frame, content: String, position: Point, color: Color, gutter: f32) {
    let size = ((gutter - 16.0) / (content.len().max(1) as f32 * 0.62)).clamp(8.0, 12.0);
    label(frame, content, position, color, size);
}

/// Shared gutter derived only from root instrument/readout context.
pub fn axis_width(data: &ChartState, quote: &str, volume_unit: &str) -> f32 {
    let magnitude = data.latest().map_or(1.0, |candle| candle.close.abs());
    let integer_digits = if magnitude >= 1.0 {
        magnitude.log10().floor() + 1.0
    } else {
        1.0
    };
    let price_chars = integer_digits as f32 + 1.0 + data.price_scale.clamp(2, 12) as f32;
    let unit_chars = volume_unit
        .len()
        .max(data.base_asset.len())
        .max(quote.len());
    (price_chars * 7.2 + 16.0)
        .max(96.0)
        .max((unit_chars + 6) as f32 * 6.0 + 12.0)
        .min(158.0)
}
const TIME_BADGE_WIDTH: f32 = 176.0;

fn inspection_badge_x(x: f32, area_width: f32) -> f32 {
    (x - TIME_BADGE_WIDTH * 0.5).clamp(0.0, (area_width - TIME_BADGE_WIDTH).max(0.0))
}

fn draw_inspection_time_badge(
    frame: &mut Frame,
    time: i64,
    x: f32,
    area: Rectangle,
    palette: Palette,
) {
    let left = inspection_badge_x(x, area.width);
    frame.fill_rectangle(
        Point::new(left, area.height + 21.0),
        Size::new(TIME_BADGE_WIDTH, 19.0),
        palette.accent,
    );
    label(
        frame,
        utc(time, "%Y-%m-%d %H:%M UTC"),
        Point::new(left + 6.0, area.height + 23.0),
        palette.background,
        12.0,
    );
}

fn indicator_value(data: &ChartState, spec: IndicatorSpec, value: f64, minimum: u32) -> String {
    if matches!(
        spec,
        IndicatorSpec::Cvd { .. } | IndicatorSpec::OpenInterest
    ) {
        return quantity(value);
    }
    let precision = if matches!(
        spec,
        IndicatorSpec::Rsi { .. } | IndicatorSpec::Stochastic { .. }
    ) {
        minimum
    } else {
        data.price_scale.clamp(minimum, 12)
    };
    if value != 0.0 && value.abs() < 10.0_f64.powi(-(precision as i32)) {
        return format!("{value:.3e}");
    }
    format!("{value:.precision$}", precision = precision as usize)
}

impl Chart<'_> {
    fn x(&self, time: f64, area: Rectangle) -> f32 {
        ((time - self.data.viewport.left) / self.data.viewport.span) as f32 * area.width
    }
    fn y(&self, price: f64, area: Rectangle) -> f32 {
        let view = self.data.viewport;
        (1.0 - presentation::fraction(price, view.low, view.high)) as f32 * area.height
    }

    fn price_tick_label(&self, value: f64, step: f64) -> String {
        let mut text = self.data.format_price(value);
        if text.contains('.') {
            while text.ends_with('0') {
                text.pop();
            }
            if text.ends_with('.') {
                text.pop();
            }
        }
        if text.len() > 20 {
            let precision = (value.abs().log10() - step.log10()).ceil().clamp(2.0, 15.0) as usize;
            format!("{value:.precision$e}")
        } else {
            text
        }
    }

    fn draw_time_ticks(&self, frame: &mut Frame, area: Rectangle, grid: Color) {
        let view = self.data.viewport;
        let ticks = TimeTicks::new(view.left, view.span, area.width);
        let half_width = ticks.label_width * 0.5;
        let format = ticks.format;
        let mut previous_right = f32::NEG_INFINITY;
        for time in ticks {
            let x = self.x(time as f64, area);
            // Omit edge labels instead of clamping several labels to one point.
            if x < half_width
                || x + half_width > area.width
                || x - half_width < previous_right + 12.0
            {
                continue;
            }
            previous_right = x + half_width;
            line(
                frame,
                Point::new(x, 0.0),
                Point::new(x, area.height),
                grid,
                1.0,
            );
            label(
                frame,
                utc(time, format),
                Point::new(x - half_width, area.height + 7.0),
                self.palette.foreground,
                12.0,
            );
        }
    }

    fn pan_action(&self, time: f64, value: f64) -> Action {
        match self.indicator {
            Some(instance) => Action::PanePan {
                id: instance.id,
                time,
                value,
            },
            None => Action::Pan { time, price: value },
        }
    }

    fn zoom_action(&self, time: f64, value: f64, x: f64, y: f64) -> Action {
        match self.indicator {
            Some(instance) => Action::PaneZoom {
                id: instance.id,
                time,
                value,
                x,
                y,
            },
            None => Action::Zoom {
                time,
                price: value,
                x,
                y,
            },
        }
    }
}

/// Distinct state tags discard held pointer gestures when a modal blocks input.
struct BlockedChart<'a>(Chart<'a>);

pub fn view<'a>(chart: Chart<'a>, height: Length, blocked: bool) -> Element<'a, Message> {
    if blocked {
        canvas(BlockedChart(chart))
            .width(Length::Fill)
            .height(height)
            .into()
    } else {
        canvas(chart).width(Length::Fill).height(height).into()
    }
}

impl canvas::Program<Message> for BlockedChart<'_> {
    type State = ();

    fn draw(
        &self,
        _: &(),
        renderer: &Renderer,
        theme: &Theme,
        bounds: Rectangle,
        _: mouse::Cursor,
    ) -> Vec<Geometry> {
        self.0.draw(
            &Interaction::default(),
            renderer,
            theme,
            bounds,
            mouse::Cursor::Unavailable,
        )
    }
}

impl canvas::Program<Message> for Chart<'_> {
    type State = Interaction;

    fn update(
        &self,
        state: &mut Interaction,
        event: canvas::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> (canvas::event::Status, Option<Message>) {
        use canvas::event::Status::{Captured, Ignored};
        let area = plot(
            bounds.size(),
            axis_width(self.data, self.quote, self.volume_unit),
        );
        let local = cursor
            .position()
            .map(|point| Point::new(point.x - bounds.x, point.y - bounds.y));
        let inside = cursor.position_in(bounds);
        let mut clear_drawing_selection = None;
        if let Some(drawings) = &self.drawings {
            let generation = drawings.interaction.generation();
            if state.drawing_claim.is_some_and(|claim| claim != generation) {
                state.drawing_claim = None;
            }
            let projection = self
                .data
                .latest()
                .and_then(|_| Projection::new(self.data.viewport, area));
            let message = |event| Some(Message::DrawingInput { generation, event });
            match &event {
                canvas::Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) => {
                    if matches!(
                        key.as_ref(),
                        keyboard::Key::Named(keyboard::key::Named::Space)
                    ) && !modifiers.control()
                        && !modifiers.alt()
                    {
                        state.space_pan = true;
                        return (Ignored, None);
                    }
                    let nudging = drawings.store.selected().is_some()
                        && self.data.active_pane.is_none()
                        && matches!(
                            key.as_ref(),
                            keyboard::Key::Named(
                                keyboard::key::Named::ArrowLeft
                                    | keyboard::key::Named::ArrowRight
                                    | keyboard::key::Named::ArrowUp
                                    | keyboard::key::Named::ArrowDown
                            )
                        );
                    if (drawings.interaction.tool() != Tool::Select
                        || drawings.interaction.captures_pointer()
                        || nudging)
                        && crate::drawing_interaction::Interaction::accepts_key(
                            &key.as_ref(),
                            *modifiers,
                        )
                        && projection.is_some()
                    {
                        let seed =
                            self.data
                                .inspected()
                                .or_else(|| self.data.latest())
                                .map(|candle| Anchor {
                                    utc_ms: candle.time.saturating_mul(1000),
                                    price: self
                                        .data
                                        .inspection
                                        .map_or(candle.close, |(_, price)| price),
                                });
                        return (
                            Captured,
                            message(DrawingInput::Key {
                                key: key.clone(),
                                modifiers: *modifiers,
                                seed,
                                step_ms: self.data.market.interval.seconds().saturating_mul(1000),
                                price_step: 10.0_f64
                                    .powi(-(self.data.price_scale.clamp(2, 12) as i32)),
                            }),
                        );
                    }
                }
                canvas::Event::Keyboard(keyboard::Event::KeyReleased { key, .. })
                    if matches!(
                        key.as_ref(),
                        keyboard::Key::Named(keyboard::key::Named::Space)
                    ) =>
                {
                    state.space_pan = false;
                    return (Ignored, None);
                }
                canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
                    if !state.space_pan && state.drag.is_none() =>
                {
                    if let (Some(point), Some(projection)) = (inside, projection)
                        && projection.contains(point)
                    {
                        let hit = drawing_geometry::hit_test(
                            drawings.store.objects(),
                            drawings.store.selected(),
                            projection,
                            point,
                        );
                        if drawings.interaction.tool() != Tool::Select || hit.is_some() {
                            state.drawing_claim = Some(generation);
                            let snap = drawings
                                .interaction
                                .magnet()
                                .then(|| {
                                    drawing_geometry::magnet(projection, point, self.data.visible())
                                })
                                .flatten();
                            return (
                                Captured,
                                message(DrawingInput::Down {
                                    point,
                                    projection,
                                    hit,
                                    snap,
                                }),
                            );
                        }
                        clear_drawing_selection = drawings.store.selected().map(|_| generation);
                    }
                }
                canvas::Event::Mouse(mouse::Event::CursorMoved { .. }) if state.drag.is_none() => {
                    if let (Some(point), Some(projection)) = (local, projection) {
                        let claimed =
                            state.drawing_claim.is_some() || drawings.interaction.is_dragging();
                        if claimed || (inside.is_some() && projection.contains(point)) {
                            let hit = projection
                                .contains(point)
                                .then(|| {
                                    drawing_geometry::hit_test(
                                        drawings.store.objects(),
                                        drawings.store.selected(),
                                        projection,
                                        point,
                                    )
                                })
                                .flatten();
                            let snap = (drawings.interaction.magnet()
                                && projection.contains(point))
                            .then(|| {
                                drawing_geometry::magnet(projection, point, self.data.visible())
                            })
                            .flatten();
                            let captured = claimed || drawings.interaction.tool() != Tool::Select;
                            return (
                                if captured { Captured } else { Ignored },
                                message(DrawingInput::Move {
                                    point,
                                    projection,
                                    hit,
                                    snap,
                                }),
                            );
                        }
                    }
                }
                canvas::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left))
                    if state.drawing_claim.take().is_some()
                        || drawings.interaction.is_dragging() =>
                {
                    if let (Some(point), Some(projection)) = (local, projection) {
                        let snap = (drawings.interaction.magnet() && projection.contains(point))
                            .then(|| {
                                drawing_geometry::magnet(projection, point, self.data.visible())
                            })
                            .flatten();
                        return (
                            Captured,
                            message(DrawingInput::Up {
                                point,
                                projection,
                                snap,
                            }),
                        );
                    }
                    return (Captured, message(DrawingInput::Leave));
                }
                canvas::Event::Mouse(mouse::Event::CursorLeft) => {
                    state.drawing_claim = None;
                    state.space_pan = false;
                    state.drag = None;
                    state.last_press = None;
                    return (Ignored, message(DrawingInput::Leave));
                }
                _ => {}
            }
        }
        let action = match event {
            canvas::Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers)) => {
                state.modifiers = modifiers;
                return (Ignored, None);
            }
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right))
                if inside.is_some() =>
            {
                Action::Inspect(None)
            }
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
                if inside.is_some() =>
            {
                let point = inside.unwrap();
                let region = region(point, area);
                let now = Instant::now();
                let double = state
                    .last_press
                    .is_some_and(|(when, previous, previous_region)| {
                        now.duration_since(when) <= Duration::from_millis(350)
                            && previous.distance(point) <= 5.0
                            && region == previous_region
                    });
                state.last_press = if double {
                    None
                } else {
                    Some((now, point, region))
                };
                if double && region == Region::PriceAxis {
                    state.drag = None;
                    match self.indicator {
                        Some(instance) => Action::FitPane(instance.id),
                        None if !self.data.viewport.auto_fit => Action::AutoFit,
                        None => Action::ActivatePane(None),
                    }
                } else {
                    let anchor = (
                        f64::from(point.x / area.width).clamp(0.0, 1.0),
                        (1.0 - f64::from(point.y / area.height)).clamp(0.0, 1.0),
                    );
                    state.drag = Some((point, region, anchor));
                    Action::ActivatePane(self.indicator.map(|instance| instance.id))
                }
            }
            canvas::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                if state.drag.take().is_some() {
                    return (Captured, None);
                }
                return (Ignored, None);
            }
            canvas::Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                let Some(point) = local else {
                    return (Ignored, None);
                };
                if let Some((previous, region, (x, y))) = state.drag {
                    state.drag = Some((point, region, (x, y)));
                    if state
                        .last_press
                        .is_some_and(|(_, initial, _)| initial.distance(point) > 5.0)
                    {
                        state.last_press = None;
                    }
                    let dx = f64::from(point.x - previous.x) / f64::from(area.width);
                    let dy = f64::from(point.y - previous.y) / f64::from(area.height);
                    match region {
                        Region::Plot => self.pan_action(-dx, dy),
                        Region::TimeAxis => self.zoom_action((dx * 3.0).exp(), 1.0, x, y),
                        Region::PriceAxis => self.zoom_action(1.0, (dy * 3.0).exp(), x, y),
                    }
                } else if area.contains(point) && inside.is_some() {
                    let view = self.data.viewport;
                    Action::InspectPane {
                        id: self.indicator.map(|instance| instance.id),
                        point: Some((
                            view.left + f64::from(point.x / area.width) * view.span,
                            if self.indicator.is_none() {
                                view.high
                                    - f64::from(point.y / area.height) * (view.high - view.low)
                            } else {
                                (view.high + view.low) * 0.5
                            },
                        )),
                    }
                } else {
                    if inside.is_some() {
                        Action::ActivatePane(self.indicator.map(|instance| instance.id))
                    } else {
                        return (Ignored, None);
                    }
                }
            }
            canvas::Event::Mouse(mouse::Event::CursorLeft) => {
                if state.drag.is_some() {
                    return (Ignored, None);
                }
                Action::Inspect(None)
            }
            canvas::Event::Mouse(mouse::Event::WheelScrolled { delta }) if inside.is_some() => {
                let point = inside.unwrap();
                let region = region(point, area);
                // Axes have priority over modifiers; Alt yields only on the plot.
                if region == Region::Plot && state.modifiers.alt() {
                    return (Ignored, None);
                }
                let x = f64::from(point.x / area.width).clamp(0.0, 1.0);
                let y = (1.0 - f64::from(point.y / area.height)).clamp(0.0, 1.0);
                let (dx, dy, pixels) = match delta {
                    mouse::ScrollDelta::Lines { x, y } => (f64::from(x), f64::from(y), false),
                    mouse::ScrollDelta::Pixels { x, y } => (f64::from(x), f64::from(y), true),
                };
                let scroll = if dy == 0.0 { dx } else { dy };
                let factor = (-scroll * if pixels { 0.008 } else { 0.12 })
                    .clamp(-20.0, 20.0)
                    .exp();
                if region == Region::PriceAxis {
                    self.zoom_action(1.0, factor, x, y)
                } else if region == Region::TimeAxis {
                    self.zoom_action(factor, 1.0, x, y)
                } else if pixels && !state.modifiers.control() && !state.modifiers.shift() {
                    // A vertical trackpad gesture also navigates time, never values.
                    let pan = if dx.abs() > dy.abs() { dx } else { dy };
                    self.pan_action(-pan / f64::from(area.width), 0.0)
                } else if state.modifiers.shift() {
                    self.zoom_action(1.0, factor, x, y)
                } else {
                    self.zoom_action(factor, 1.0, x, y)
                }
            }
            _ => return (Ignored, None),
        };
        if let Some(generation) = clear_drawing_selection {
            return (Captured, Some(Message::DrawingBlank { generation }));
        }
        (Captured, Some(Message::Action(action)))
    }

    fn draw(
        &self,
        _state: &Interaction,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        if let Some(instance) = self.indicator {
            return self.draw_lower(instance, renderer, bounds);
        }
        let area = plot(
            bounds.size(),
            axis_width(self.data, self.quote, self.volume_unit),
        );
        let projection = self
            .data
            .latest()
            .and_then(|_| Projection::new(self.data.viewport, area));
        if let Some(drawings) = &self.drawings {
            drawings.projection.set(projection);
        }
        let palette = self.palette;
        let view = self.data.viewport;
        let base = self.cache.draw(renderer, bounds.size(), |frame| {
            // Keep the background in its own layer: iced 0.13 appends a parent
            // frame's pending meshes after clipped child meshes.
            frame.with_clip(Rectangle::with_size(bounds.size()), |frame| {
                frame.fill_rectangle(Point::ORIGIN, bounds.size(), palette.background);
                let grid = Color {
                    a: 0.3,
                    ..palette.muted
                };
                let latest_y = self
                    .data
                    .latest()
                    .map(|candle| self.y(candle.close, area))
                    .filter(|y| (0.0..=area.height).contains(y));
                if self.data.latest().is_some() {
                    let minimum = 10.0_f64.powi(-(self.data.price_scale.min(12) as i32));
                    let ticks = ValueTicks::new(view.low, view.high, area.height, minimum);
                    let step = ticks.step;
                    for price in ticks {
                        let y = (1.0 - presentation::fraction(price, view.low, view.high)) as f32
                            * area.height;
                        line(
                            frame,
                            Point::new(0.0, y),
                            Point::new(area.width, y),
                            grid,
                            1.0,
                        );
                        // Canvas text remains above the badge mesh; omit nearby
                        // ticks rather than overprinting a precise live price.
                        if y >= 8.0
                            && y <= area.height - 8.0
                            && latest_y.is_none_or(|latest_y| (latest_y - y).abs() > 24.0)
                        {
                            axis_label(
                                frame,
                                self.price_tick_label(price, step),
                                Point::new(area.width + 8.0, y - 7.0),
                                palette.foreground,
                                bounds.width - area.width,
                            );
                        }
                    }
                    self.draw_time_ticks(frame, area, grid);
                }
                label(
                    frame,
                    format!("UTC / {}", self.quote),
                    Point::new(area.width + 6.0, area.height + 8.0),
                    palette.muted,
                    11.0,
                );
            });
            let width = ((self.data.step() / view.span) as f32 * area.width * 0.72).max(1.0);
            frame.with_clip(area, |frame| {
                for candle in self.data.visible() {
                    let x = self.x(candle.time as f64, area);
                    let color = if candle.close >= candle.open {
                        palette.green
                    } else {
                        palette.red
                    };
                    line(
                        frame,
                        Point::new(x, self.y(candle.high, area)),
                        Point::new(x, self.y(candle.low, area)),
                        color,
                        1.0,
                    );
                    let open = self.y(candle.open, area);
                    let close = self.y(candle.close, area);
                    let position = Point::new(x - width * 0.5, open.min(close));
                    let size = Size::new(width, (open - close).abs().max(1.0));
                    if candle.close >= candle.open && width >= 3.0 {
                        // Hollow rising / filled falling remains honest in monochrome.
                        frame.fill_rectangle(position, size, palette.background);
                        frame.stroke(
                            &Path::rectangle(position, size),
                            Stroke::default().with_color(color).with_width(1.2),
                        );
                    } else {
                        let body_width = if candle.close < candle.open {
                            width.max(2.0)
                        } else {
                            width
                        };
                        frame.fill_rectangle(
                            Point::new(x - body_width * 0.5, position.y),
                            Size::new(body_width, size.height),
                            color,
                        );
                    }
                }
                for instance in self
                    .data
                    .indicators
                    .instances
                    .iter()
                    .filter(|item| item.visible && item.spec.is_overlay())
                {
                    self.draw_series(frame, instance, area, view.low, view.high);
                }
            });
        });
        let mut overlay = Frame::new(renderer, bounds.size());
        let inspection_x = self
            .data
            .inspection
            .map(|(time, _)| self.x(time as f64, area))
            .filter(|x| (0.0..=area.width).contains(x));
        if let (Some((time, _)), Some(x)) = (self.data.inspection, inspection_x) {
            draw_inspection_time_badge(&mut overlay, time, x, area, palette);
        }
        if let Some(latest) = self.data.latest() {
            let y = self.y(latest.close, area);
            if y >= 0.0 && y <= area.height {
                let color = if latest.close >= latest.open {
                    palette.green
                } else {
                    palette.red
                };
                line(
                    &mut overlay,
                    Point::new(0.0, y),
                    Point::new(area.width, y),
                    Color { a: 0.45, ..color },
                    1.0,
                );
                overlay.fill_rectangle(
                    Point::new(area.width, (y - 9.0).max(0.0)),
                    Size::new(bounds.width - area.width, 20.0),
                    color,
                );
                label(
                    &mut overlay,
                    self.data.format_price(latest.close),
                    Point::new(area.width + 6.0, (y - 8.0).max(0.0)),
                    palette.background,
                    12.0,
                );
            }
        }
        if let Some((time, price)) = self.data.inspection {
            let x = self.x(time as f64, area);
            let y = self.y(price, area);
            if (0.0..=area.width).contains(&x) {
                line(
                    &mut overlay,
                    Point::new(x, 0.0),
                    Point::new(x, area.height),
                    palette.accent,
                    1.0,
                );
            }
            if (0.0..=area.width).contains(&x) && (0.0..=area.height).contains(&y) {
                line(
                    &mut overlay,
                    Point::new(0.0, y),
                    Point::new(area.width, y),
                    palette.accent,
                    1.0,
                );
                let content = self.data.format_price(price);
                let width = content.len() as f32 * 7.2 + 10.0;
                let position = Point::new(
                    (x + 8.0).min((area.width - width).max(0.0)),
                    (y - 22.0).max(0.0),
                );
                overlay.fill_rectangle(position, Size::new(width, 20.0), palette.background);
                label(
                    &mut overlay,
                    content,
                    Point::new(position.x + 4.0, position.y + 2.0),
                    palette.foreground,
                    12.0,
                );
            }
        }
        let mut layers = Vec::with_capacity(if self.drawings.is_some() && projection.is_some() {
            4
        } else {
            2
        });
        layers.push(base);
        if let (Some(drawings), Some(projection)) = (&self.drawings, projection) {
            let local = cursor
                .position()
                .map(|point| Point::new(point.x - bounds.x, point.y - bounds.y));
            layers.extend(drawings.draw(
                renderer,
                bounds.size(),
                projection,
                local,
                self.palette,
                self.data.price_scale,
                self.quote,
            ));
        }
        layers.push(overlay.into_geometry());
        layers
    }

    fn mouse_interaction(
        &self,
        state: &Interaction,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        let area = plot(
            bounds.size(),
            axis_width(self.data, self.quote, self.volume_unit),
        );
        if !state.space_pan
            && state.drag.is_none()
            && let Some(drawings) = &self.drawings
            && let Some(projection) = self
                .data
                .latest()
                .and_then(|_| Projection::new(self.data.viewport, area))
        {
            if drawings.interaction.is_dragging() {
                return mouse::Interaction::Grabbing;
            }
            if let Some(point) = cursor.position_in(bounds)
                && projection.contains(point)
            {
                if drawings.interaction.tool() != Tool::Select {
                    return mouse::Interaction::Crosshair;
                }
                if let Some(hit) = drawing_geometry::hit_test(
                    drawings.store.objects(),
                    drawings.store.selected(),
                    projection,
                    point,
                ) {
                    return if hit.part == crate::drawing_types::Part::Body {
                        mouse::Interaction::Pointer
                    } else {
                        mouse::Interaction::Crosshair
                    };
                }
            }
        }
        let region = state
            .drag
            .map(|(_, region, _)| region)
            .or_else(|| cursor.position_in(bounds).map(|point| region(point, area)));
        match region {
            Some(Region::PriceAxis) => mouse::Interaction::ResizingVertically,
            Some(Region::TimeAxis) => mouse::Interaction::ResizingHorizontally,
            Some(Region::Plot) if state.drag.is_some() => mouse::Interaction::Grabbing,
            Some(Region::Plot) => mouse::Interaction::Crosshair,
            None => mouse::Interaction::default(),
        }
    }
}

fn visible_points(instance: &IndicatorInstance, left: f64, span: f64) -> &[IndicatorPoint] {
    let start = instance
        .points
        .partition_point(|point| (point.time as f64) < left)
        .saturating_sub(1);
    let end = (instance
        .points
        .partition_point(|point| point.time as f64 <= left + span)
        + 1)
    .min(instance.points.len());
    &instance.points[start..end]
}

fn dash(id: u64, series: usize) -> [f32; 4] {
    // Instance identity and series both survive a monochrome palette.
    [4.0 + id as f32 * 2.0, 3.0, 2.0 + series as f32 * 4.0, 3.0]
}

fn series_color(palette: Palette, series: usize) -> Color {
    match series {
        0 => palette.accent,
        1 => palette.foreground,
        _ => palette.muted,
    }
}

fn series_stroke(color: Color, segments: &[f32]) -> Stroke<'_> {
    Stroke {
        line_dash: canvas::LineDash {
            segments,
            offset: 0,
        },
        ..Stroke::default().with_color(color).with_width(1.7)
    }
}

// Clip in data coordinates before converting to pixels. Renderer clipping happens
// after dashed-path expansion, which is unbounded for far-offscreen manual scales.
fn clip_segment(
    start: (f64, f64),
    end: (f64, f64),
    x: (f64, f64),
    y: (f64, f64),
) -> Option<[(f64, f64); 2]> {
    let dx = end.0 - start.0;
    let dy = end.1 - start.1;
    if !dx.is_finite() || !dy.is_finite() || !start.0.is_finite() || !start.1.is_finite() {
        return None;
    }
    let (mut from, mut to) = (0.0_f64, 1.0_f64);
    let (mut from_edge, mut to_edge) = (None, None);
    for (p, q, vertical, edge) in [
        (-dx, start.0 - x.0, false, x.0),
        (dx, x.1 - start.0, false, x.1),
        (-dy, start.1 - y.0, true, y.0),
        (dy, y.1 - start.1, true, y.1),
    ] {
        if p == 0.0 {
            if q < 0.0 {
                return None;
            }
            continue;
        }
        let crossing = q / p;
        if p < 0.0 && crossing >= from {
            from = crossing;
            from_edge = Some((vertical, edge));
        } else if p > 0.0 && crossing <= to {
            to = crossing;
            to_edge = Some((vertical, edge));
        }
        if from > to {
            return None;
        }
    }
    let position = |t: f64, edge: Option<(bool, f64)>| {
        let mut point = (
            (start.0 + dx * t).clamp(x.0, x.1),
            (start.1 + dy * t).clamp(y.0, y.1),
        );
        // Preserve the exact clipped ordinate even when a very steep segment's
        // two intersection parameters round to the same floating-point value.
        if let Some((vertical, edge)) = edge {
            if vertical {
                point.1 = edge;
            } else {
                point.0 = edge;
            }
        }
        point
    };
    Some([position(from, from_edge), position(to, to_edge)])
}

impl Chart<'_> {
    fn draw_series(
        &self,
        frame: &mut Frame,
        instance: &IndicatorInstance,
        area: Rectangle,
        low: f64,
        high: f64,
    ) {
        if instance.spec == IndicatorSpec::OpenInterest {
            self.draw_open_interest(frame, instance, area, low, high);
            return;
        }
        let points = visible_points(instance, self.data.viewport.left, self.data.viewport.span);
        let y = |value: f64| (1.0 - presentation::fraction(value, low, high)) as f32 * area.height;
        for series in 0..instance.spec.series_names().len() {
            let color = series_color(self.palette, series);
            if matches!(instance.spec, IndicatorSpec::Macd { .. }) && series == 2 {
                let width =
                    ((self.data.step() / self.data.viewport.span) as f32 * area.width * 0.65)
                        .max(1.0);
                for point in points {
                    if let Some(value) = point.values[series].filter(|value| value.is_finite()) {
                        let bottom_value = value.min(0.0).max(low);
                        let top_value = value.max(0.0).min(high);
                        let left = (self.x(point.time as f64, area) - width * 0.5).max(0.0);
                        let right = (self.x(point.time as f64, area) + width * 0.5).min(area.width);
                        if bottom_value < top_value && left < right {
                            let top = y(top_value);
                            frame.fill_rectangle(
                                Point::new(left, top),
                                Size::new(right - left, (y(bottom_value) - top).max(1.0)),
                                Color { a: 0.8, ..color },
                            );
                        }
                    }
                }
                continue;
            }
            let cvd = matches!(instance.spec, IndicatorSpec::Cvd { .. });
            let point_start = points
                .first()
                .and_then(|first| {
                    instance
                        .points
                        .binary_search_by_key(&first.time, |point| point.time)
                        .ok()
                })
                .unwrap_or(0);
            let path = Path::new(|builder| {
                let mut previous = None;
                let mut previous_origin = None;
                let mut last_end = None;
                for (offset, point) in points.iter().enumerate() {
                    let origin = cvd
                        .then(|| instance.cvd_context(point_start + offset).map(|c| c.origin))
                        .flatten();
                    if let Some(value) = point.values[series].filter(|value| value.is_finite()) {
                        if cvd && (origin.is_none() || previous_origin != origin) {
                            previous = None;
                            last_end = None;
                        }
                        let position = (point.time as f64, value);
                        if let Some(start) = previous {
                            if let Some([start, end]) = clip_segment(
                                start,
                                position,
                                (
                                    self.data.viewport.left,
                                    self.data.viewport.left + self.data.viewport.span,
                                ),
                                (low, high),
                            ) {
                                let start = Point::new(self.x(start.0, area), y(start.1));
                                let end = Point::new(self.x(end.0, area), y(end.1));
                                if last_end != Some(start) {
                                    builder.move_to(start);
                                }
                                builder.line_to(end);
                                last_end = Some(end);
                            } else {
                                last_end = None;
                            }
                        }
                        previous = Some(position);
                        previous_origin = origin;
                    } else {
                        previous = None;
                        previous_origin = None;
                        last_end = None;
                    }
                }
            });
            let segments = dash(instance.id, series);
            frame.stroke(&path, series_stroke(color, &segments));
            if cvd {
                // A proven origin/reset can contain one point and no line segment.
                for (index, point) in points.iter().enumerate() {
                    let Some(value) = point.values[series].filter(|value| value.is_finite()) else {
                        continue;
                    };
                    let Some(context) = instance.cvd_context(point_start + index) else {
                        continue;
                    };
                    let connected = |other: usize| {
                        points.get(other).is_some_and(|point| {
                            point.values[series].is_some_and(f64::is_finite)
                                && instance
                                    .cvd_context(point_start + other)
                                    .is_some_and(|other| other.origin == context.origin)
                        })
                    };
                    let isolated = (index == 0 || !connected(index - 1)) && !connected(index + 1);
                    let seconds = point.time as f64;
                    if (isolated || index + 1 == points.len())
                        && (self.data.viewport.left
                            ..=self.data.viewport.left + self.data.viewport.span)
                            .contains(&seconds)
                        && (low..=high).contains(&value)
                    {
                        frame.fill(
                            &Path::circle(Point::new(self.x(seconds, area), y(value)), 2.5),
                            color,
                        );
                    }
                }
            }
        }
    }
    fn draw_open_interest(
        &self,
        frame: &mut Frame,
        instance: &IndicatorInstance,
        area: Rectangle,
        low: f64,
        high: f64,
    ) {
        let left = self.data.viewport.left;
        let right = left + self.data.viewport.span;
        let samples = self.data.open_interest_samples_for(instance);
        let start = samples
            .partition_point(|sample| (sample.timestamp_ms as f64 / 1000.0) < left)
            .saturating_sub(1);
        let end = samples
            .partition_point(|sample| sample.timestamp_ms as f64 / 1000.0 <= right)
            .saturating_add(1)
            .min(samples.len());
        let samples = &samples[start..end];
        let y = |quantity: f64| {
            (1.0 - presentation::fraction(quantity, low, high)) as f32 * area.height
        };
        let path = Path::new(|builder| {
            let mut last_end = None;
            for pair in samples.windows(2) {
                if !oi_segment_contiguous(instance, &pair[0], &pair[1]) {
                    last_end = None;
                    continue;
                }
                if let Some([start, end]) = clip_segment(
                    (pair[0].timestamp_ms as f64 / 1000.0, pair[0].quantity),
                    (pair[1].timestamp_ms as f64 / 1000.0, pair[1].quantity),
                    (left, right),
                    (low, high),
                ) {
                    let start = Point::new(self.x(start.0, area), y(start.1));
                    let end = Point::new(self.x(end.0, area), y(end.1));
                    if last_end != Some(start) {
                        builder.move_to(start);
                    }
                    builder.line_to(end);
                    last_end = Some(end);
                } else {
                    last_end = None;
                }
            }
        });
        let color = series_color(self.palette, 0);
        let segments = dash(instance.id, 0);
        frame.stroke(&path, series_stroke(color, &segments));
        // Draw isolated observations and the newest live sample, even without history.
        for (index, sample) in samples.iter().enumerate() {
            let seconds = sample.timestamp_ms as f64 / 1000.0;
            let isolated = (index == 0
                || !oi_segment_contiguous(instance, &samples[index - 1], sample))
                && (index + 1 == samples.len()
                    || !oi_segment_contiguous(instance, sample, &samples[index + 1]));
            if (isolated || index + 1 == samples.len())
                && (left..=right).contains(&seconds)
                && (low..=high).contains(&sample.quantity)
            {
                frame.fill(
                    &Path::circle(Point::new(self.x(seconds, area), y(sample.quantity)), 2.5),
                    color,
                );
            }
        }
    }

    fn draw_lower(
        &self,
        instance: &IndicatorInstance,
        renderer: &Renderer,
        bounds: Rectangle,
    ) -> Vec<Geometry> {
        let area = plot(
            bounds.size(),
            axis_width(self.data, self.quote, self.volume_unit),
        );
        let view = self.data.viewport;
        let palette = self.palette;
        let scale = self.data.pane_scale(instance.id);
        let (low, high) = (scale.low, scale.high);
        let base = self.cache.draw(renderer, bounds.size(), |frame| {
            // Extents are cached by the state update path, never fitted while drawing.
            let y =
                |value: f64| (1.0 - presentation::fraction(value, low, high)) as f32 * area.height;
            // Background first, in a separate clipped layer, just like price.
            frame.with_clip(Rectangle::with_size(bounds.size()), |frame| {
                frame.fill_rectangle(Point::ORIGIN, bounds.size(), palette.background);
                let grid = Color {
                    a: 0.3,
                    ..palette.muted
                };
                let has_values = if instance.spec == IndicatorSpec::OpenInterest {
                    !self.data.open_interest_samples_for(instance).is_empty()
                } else {
                    visible_points(instance, self.data.viewport.left, self.data.viewport.span)
                        .iter()
                        .any(|point| point.values.iter().flatten().any(|value| value.is_finite()))
                };
                if has_values {
                    let minimum = if matches!(
                        instance.spec,
                        IndicatorSpec::Cvd { .. } | IndicatorSpec::OpenInterest
                    ) {
                        0.0
                    } else if matches!(
                        instance.spec,
                        IndicatorSpec::Rsi { .. } | IndicatorSpec::Stochastic { .. }
                    ) {
                        0.01
                    } else {
                        10.0_f64.powi(-(self.data.price_scale.min(12) as i32))
                    };
                    let ticks = ValueTicks::new(low, high, area.height, minimum);
                    let step = ticks.step;
                    for value in ticks {
                        let ordinate =
                            (1.0 - presentation::fraction(value, low, high)) as f32 * area.height;
                        line(
                            frame,
                            Point::new(0.0, ordinate),
                            Point::new(area.width, ordinate),
                            grid,
                            1.0,
                        );
                        if ordinate >= 8.0 && ordinate <= area.height - 8.0 {
                            axis_label(
                                frame,
                                if matches!(
                                    instance.spec,
                                    IndicatorSpec::Cvd { .. } | IndicatorSpec::OpenInterest
                                ) {
                                    presentation::quantity_tick(value, step)
                                } else {
                                    indicator_value(self.data, instance.spec, value, 2)
                                },
                                Point::new(area.width + 8.0, ordinate - 7.0),
                                palette.foreground,
                                bounds.width - area.width,
                            );
                        }
                    }
                }
                let references: &[f64] = match instance.spec {
                    IndicatorSpec::Cvd { .. } => &[0.0],
                    IndicatorSpec::Macd { .. } => &[0.0],
                    _ => &[],
                };
                for value in references.iter().filter(|_| has_values) {
                    let ordinate = y(*value);
                    if !(0.0..=area.height).contains(&ordinate) {
                        continue;
                    }
                    frame.stroke(
                        &Path::line(Point::new(0.0, ordinate), Point::new(area.width, ordinate)),
                        series_stroke(
                            Color {
                                a: 0.5,
                                ..palette.foreground
                            },
                            &[3.0, 5.0],
                        ),
                    );
                    if *value != 0.0 {
                        label(
                            frame,
                            format!("{value:.0}"),
                            Point::new(4.0, ordinate - 15.0),
                            palette.muted,
                            10.0,
                        );
                    }
                }
                // Native candles initialize shared time; retained OI alone cannot
                // turn the reset viewport at epoch zero into a valid date axis.
                if self.data.latest().is_some() {
                    self.draw_time_ticks(frame, area, grid);
                }
                label(
                    frame,
                    if instance.spec == IndicatorSpec::OpenInterest
                        || matches!(instance.spec, IndicatorSpec::Cvd { .. })
                    {
                        format!(
                            "UTC / {}",
                            instance_unit(self.data, instance, self.volume_unit)
                        )
                    } else {
                        "UTC".into()
                    },
                    Point::new(area.width + 8.0, area.height + 8.0),
                    palette.muted,
                    11.0,
                );
                label(
                    frame,
                    if scale.auto_fit {
                        "Auto / F"
                    } else {
                        "Manual / F"
                    }
                    .into(),
                    Point::new(area.width + 8.0, area.height + 26.0),
                    if scale.auto_fit {
                        palette.muted
                    } else {
                        palette.accent
                    },
                    10.0,
                );
            });
            frame.with_clip(area, |frame| {
                self.draw_series(frame, instance, area, low, high);
                if instance.spec == IndicatorSpec::OpenInterest {
                    let samples = self.data.open_interest_samples_for(instance);
                    let start = samples.partition_point(|sample| {
                        (sample.timestamp_ms as f64 / 1000.0) < view.left
                    });
                    let has_visible = samples.get(start).is_some_and(|sample| {
                        sample.timestamp_ms as f64 / 1000.0 <= view.left + view.span
                    });
                    if !has_visible {
                        label(
                            frame,
                            if samples.is_empty() {
                                if instance.source.is_chart() {
                                    open_interest_status(self.data)
                                } else {
                                    instance.external.as_ref().map_or_else(
                                        || "Awaiting selected source data".into(),
                                        |output| output.status.clone(),
                                    )
                                }
                            } else {
                                "No native observations in this time range".into()
                            },
                            Point::new(8.0, 8.0),
                            palette.muted,
                            12.0,
                        );
                    }
                }
            });
        });
        let mut crosshair = Frame::new(renderer, bounds.size());
        if let Some(time_ms) = self.data.inspection_time_ms() {
            let x = self.x(time_ms as f64 / 1000.0, area);
            if (0.0..=area.width).contains(&x) {
                draw_inspection_time_badge(
                    &mut crosshair,
                    time_ms.div_euclid(1000),
                    x,
                    area,
                    palette,
                );
                line(
                    &mut crosshair,
                    Point::new(x, 0.0),
                    Point::new(x, area.height),
                    palette.accent,
                    1.0,
                );
                if instance.spec == IndicatorSpec::OpenInterest
                    && let Some(sample) = self.data.inspected_open_interest_for(instance)
                {
                    label(
                        &mut crosshair,
                        format!(
                            "{} · {} {}",
                            open_interest_time(sample),
                            quantity(sample.quantity),
                            instance_unit(self.data, instance, self.volume_unit)
                        ),
                        Point::new(8.0, (area.height - 20.0).max(0.0)),
                        palette.foreground,
                        11.0,
                    );
                }
            }
        }
        vec![base, crosshair.into_geometry()]
    }
}

const LEGEND_HEADER_HEIGHT: f32 = 26.0;

/// Nominal height at ordinary widths; wrapping may grow the actual widget.
/// Contributor count never affects the permanent legend footprint.
pub fn legend_height(instance: &IndicatorInstance) -> f32 {
    LEGEND_HEADER_HEIGHT
        + if !instance.source.is_chart() {
            112.0
        } else if matches!(
            instance.spec,
            IndicatorSpec::Cvd { .. } | IndicatorSpec::OpenInterest
        ) {
            82.0
        } else {
            22.0
        }
}

fn quality_label(quality: FlowQuality) -> &'static str {
    match quality {
        FlowQuality::Exact => "Exact",
        FlowQuality::Reported => "Reported (unconfirmed side)",
        FlowQuality::Missing => "Missing (incomplete coverage)",
    }
}

fn legend_point_index(data: &ChartState, instance: &IndicatorInstance) -> Option<usize> {
    if instance.source.is_chart() {
        return match data.inspection {
            Some((time, _)) => instance
                .points
                .binary_search_by_key(&time, |point| point.time)
                .ok(),
            None => instance.points.len().checked_sub(1),
        };
    }
    match data.inspection_time_ms() {
        Some(time_ms) => instance
            .points
            .partition_point(|point| point.time <= time_ms.div_euclid(1000))
            .checked_sub(1)
            .filter(|index| {
                let point = &instance.points[*index];
                time_ms.div_euclid(1000) < point.time.saturating_add(data.market.interval.seconds())
                    && instance
                        .cvd_context(*index)
                        .is_some_and(|context| time_ms <= context.through_ms)
            }),
        None if instance
            .external
            .as_ref()
            .is_some_and(|output| output.complete) =>
        {
            instance.points.len().checked_sub(1)
        }
        None => None,
    }
}

fn source_asof_range(
    data: &ChartState,
    instance: &IndicatorInstance,
    point_index: Option<usize>,
) -> Option<(i64, i64)> {
    let output = instance.external.as_ref()?;
    if data.inspection_time_ms().is_none() {
        return output.asof_range_ms;
    }
    if instance.spec == IndicatorSpec::OpenInterest {
        let sample = data.inspected_open_interest_for(instance)?;
        let index = output
            .oi
            .partition_point(|point| point.timestamp_ms <= sample.timestamp_ms)
            .checked_sub(1)?;
        output.oi_asof_ranges_ms.get(index).copied()
    } else {
        point_index.and_then(|index| output.context_asof_ranges_ms.get(index).copied().flatten())
    }
}

fn asof_range_label(range: Option<(i64, i64)>) -> String {
    range.map_or_else(
        || "Source as-of range unavailable".into(),
        |(min, max)| format!("Source as-of {} — {}", utc_ms(min), utc_ms(max)),
    )
}

fn observation_age(data: &ChartState, sample: &OpenInterestSample) -> String {
    let inspecting = data.inspection_time_ms();
    let reference = inspecting.unwrap_or_else(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| {
                duration.as_millis().min(i64::MAX as u128) as i64
            })
    });
    let age = reference.saturating_sub(sample.timestamp_ms);
    if age < 0 {
        return "timestamp ahead of reference clock".into();
    }
    format!(
        "{} {:.1} s",
        if inspecting.is_some() {
            "cursor lag"
        } else {
            "age"
        },
        age as f64 / 1000.0
    )
}

fn summary_line<'a>(content: impl Into<String>, color: Color) -> Element<'a, Message> {
    text(content.into())
        .size(11)
        .color(color)
        .width(Length::Fill)
        .into()
}

// A small canvas only for the line/bar key; all disclosure text uses wrapping widgets.
struct SeriesKey {
    id: u64,
    spec: IndicatorSpec,
    series: usize,
    color: Color,
}

impl canvas::Program<Message> for SeriesKey {
    type State = ();

    fn draw(
        &self,
        _: &(),
        renderer: &Renderer,
        _: &Theme,
        bounds: Rectangle,
        _: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        if matches!(self.spec, IndicatorSpec::Macd { .. }) && self.series == 2 {
            frame.fill_rectangle(
                Point::new(0.0, 3.0),
                Size::new(bounds.width, 7.0),
                Color {
                    a: 0.8,
                    ..self.color
                },
            );
        } else {
            let segments = dash(self.id, self.series);
            frame.stroke(
                &Path::line(Point::new(0.0, 7.0), Point::new(bounds.width, 7.0)),
                series_stroke(self.color, &segments),
            );
        }
        vec![frame.into_geometry()]
    }
}

fn indicator_summary_view<'a>(
    data: &'a ChartState,
    instance: &'a IndicatorInstance,
    palette: Palette,
    volume_unit: &'a str,
    exact: bool,
) -> Element<'a, Message> {
    let inspecting = data.inspection_time_ms();
    let point_index = legend_point_index(data, instance);
    let mut body = Column::new().spacing(3).width(Length::Fill);
    let unit = instance_unit(data, instance, volume_unit);
    let mut readouts = Row::new().spacing(12).align_y(Alignment::Center);
    let native = if exact { quantity_exact } else { quantity };
    for (series, name) in instance.spec.series_names().iter().enumerate() {
        let color = series_color(palette, series);
        let value = if instance.spec == IndicatorSpec::OpenInterest {
            data.inspected_open_interest_for(instance).map_or_else(
                || format!("-- {unit} (missing observation)"),
                |sample| format!("{} {unit}", native(sample.quantity)),
            )
        } else {
            point_index
                .and_then(|index| instance.points[index].values[series])
                .map_or_else(
                    || {
                        if matches!(instance.spec, IndicatorSpec::Cvd { .. }) {
                            format!("-- {unit} (incomplete)")
                        } else {
                            "-- (warming up)".into()
                        }
                    },
                    |value| {
                        if matches!(instance.spec, IndicatorSpec::Cvd { .. }) {
                            let value = native(value);
                            format!("{value} {unit}")
                        } else {
                            indicator_value(data, instance.spec, value, 4)
                        }
                    },
                )
        };
        readouts = readouts.push(
            row![
                canvas(SeriesKey {
                    id: instance.id,
                    spec: instance.spec,
                    series,
                    color
                })
                .width(28)
                .height(14),
                text(format!("{name} {value}")).size(12).color(color),
            ]
            .spacing(5)
            .align_y(Alignment::Center)
            .wrap(),
        );
    }
    body = body.push(readouts.width(Length::Fill).wrap());
    if matches!(
        instance.spec,
        IndicatorSpec::Cvd { .. } | IndicatorSpec::OpenInterest
    ) {
        let coverage = if instance
            .external
            .as_ref()
            .is_some_and(|output| output.complete)
        {
            "complete"
        } else {
            "incomplete"
        };
        let mode = match (inspecting, instance.source.is_chart()) {
            (None, true) => "Current".to_owned(),
            (None, false) => format!("Current · current sources {coverage}"),
            (Some(time), true) => format!("Historical inspection · {}", utc_ms(time)),
            (Some(time), false) => format!(
                "Historical inspection · {} · current sources {coverage}",
                utc_ms(time)
            ),
        };
        body = body.push(summary_line(mode, palette.foreground));
    }
    if matches!(instance.spec, IndicatorSpec::Cvd { .. }) {
        let provenance = point_index
            .and_then(|index| instance.cvd_context(index))
            .map_or_else(
                || "Missing coverage · origin unavailable".into(),
                |context| {
                    format!(
                        "{}{} · origin {} UTC · through {}",
                        if instance.source.is_chart() {
                            ""
                        } else {
                            "Weakest: "
                        },
                        quality_label(context.quality),
                        utc(context.origin, "%Y-%m-%d %H:%M"),
                        utc_ms(context.through_ms),
                    )
                },
            );
        body = body.push(summary_line(provenance, palette.foreground));
    } else if instance.spec == IndicatorSpec::OpenInterest {
        let provenance = data.inspected_open_interest_for(instance).map_or_else(
            || "No actual observation at this time".into(),
            |sample| {
                format!(
                    "{} · {}",
                    open_interest_time(sample),
                    observation_age(data, sample)
                )
            },
        );
        body = body.push(summary_line(provenance, palette.foreground));
        if instance.source.is_chart() {
            body = body.push(summary_line(open_interest_status(data), palette.foreground));
        }
    }
    if !instance.source.is_chart() {
        body = body.push(summary_line(
            asof_range_label(source_asof_range(data, instance, point_index)),
            palette.foreground,
        ));
        if instance.spec == IndicatorSpec::OpenInterest {
            body = body.push(summary_line(
                "Not an exact simultaneous snapshot",
                palette.foreground,
            ));
        }
    }
    body.into()
}

fn source_problem_view<'a>(
    id: u64,
    problem: String,
    palette: Palette,
    blocked: bool,
) -> Element<'a, Message> {
    let action = |label: &'static str, message| {
        button(text(label).size(11))
            .style(button::secondary)
            .padding([3, 6])
            .on_press_maybe((!blocked).then_some(message))
    };
    column![
        summary_line(
            format!("Source needs attention: {problem}"),
            palette.foreground
        ),
        row![
            action("Repair source", Message::IndicatorSettings(id)),
            action("Use Chart", Message::UseChartSource(id)),
        ]
        .spacing(4)
        .wrap(),
    ]
    .spacing(3)
    .width(Length::Fill)
    .into()
}

pub fn legend_view<'a>(
    data: &'a ChartState,
    instance: &'a IndicatorInstance,
    palette: Palette,
    volume_unit: &'a str,
    help_open: bool,
    source_problem: Option<String>,
) -> Element<'a, Message> {
    let target = if !instance.spec.is_overlay() && data.active_pane == Some(instance.id) {
        " · scale target"
    } else {
        ""
    };
    let title = if instance.source.is_chart() {
        format!("#{} {}{target}", instance.id, instance.spec.label())
    } else {
        format!(
            "#{} {} · {}{target}",
            instance.id,
            instance.spec.label(),
            instance.source.mode
        )
    };
    let activate = Message::Action(Action::ActivatePane(
        (!instance.spec.is_overlay()).then_some(instance.id),
    ));
    let title = button(
        text(title)
            .size(13)
            .line_height(text::LineHeight::Absolute(20.0.into()))
            .color(palette.foreground),
    )
    .style(button::text)
    .padding([3, 2])
    .on_press_maybe((!help_open).then_some(activate.clone()));
    let control = |label: &'static str, message| {
        button(text(label).size(11))
            .style(button::text)
            .padding([3, 6])
            .height(LEGEND_HEADER_HEIGHT)
            .on_press_maybe((!help_open).then_some(message))
    };
    // Local controls remain siblings of the pane activation target.
    let mut header = row![
        title,
        control("Details", Message::IndicatorDetails(instance.id)),
        control("Settings", Message::IndicatorSettings(instance.id)),
        control(
            "Hide",
            Message::Action(Action::ToggleIndicator(instance.id))
        ),
        control(
            "Remove",
            Message::Action(Action::RemoveIndicator(instance.id))
        ),
    ];
    if !instance.spec.is_overlay() {
        header = header.push(control(
            if data.pane_collapsed(instance.id) {
                "Expand"
            } else {
                "Collapse"
            },
            Message::Action(Action::TogglePaneCollapsed(instance.id)),
        ));
    }
    let header = header
        .spacing(4)
        .align_y(Alignment::Center)
        .width(Length::Fill)
        .wrap();
    let summary = mouse_area(indicator_summary_view(
        data,
        instance,
        palette,
        volume_unit,
        false,
    ));
    let summary = if help_open {
        summary
    } else {
        summary
            .on_press(activate)
            .interaction(mouse::Interaction::Pointer)
    };
    let mut legend = column![header, summary].spacing(3).width(Length::Fill);
    if let Some(problem) = source_problem {
        legend = legend.push(source_problem_view(
            instance.id,
            problem,
            palette,
            help_open,
        ));
    }
    legend.into()
}

/// Body only: the caller owns the modal shell, scrolling and close controls.
pub fn indicator_details_view<'a>(
    data: &'a ChartState,
    instance: &'a IndicatorInstance,
    palette: Palette,
    volume_unit: &'a str,
    source_problem: Option<String>,
) -> Element<'a, Message> {
    let mut details = column![
        indicator_summary_view(data, instance, palette, volume_unit, true),
        text("Source and coverage")
            .size(14)
            .color(palette.foreground),
        summary_line(
            format!("Source mode: {}", instance.source.mode),
            palette.foreground
        ),
    ]
    .spacing(8)
    .width(Length::Fill);
    if let Some(problem) = source_problem {
        details = details.push(source_problem_view(instance.id, problem, palette, false));
    }
    if !instance.source.is_chart() {
        let output = instance.external.as_ref();
        details = details.push(summary_line(
            output.map_or("Awaiting source validation / data", |output| {
                output.status.as_str()
            }),
            palette.foreground,
        ));
        details = details.push(summary_line(
            "Every source is required. Totals are not an exact simultaneous snapshot.",
            palette.foreground,
        ));
        if matches!(instance.spec, IndicatorSpec::Cvd { .. }) {
            details = details.push(summary_line(
                "Shared reset/origin; raw deltas are summed before accumulation. Quality is the weakest contributing coverage.",
                palette.foreground,
            ));
        } else {
            details = details.push(summary_line(
                "Actual observations only; venue as-of and locally observed timestamps retain their provenance. Gaps are not filled.",
                palette.foreground,
            ));
        }
        details = details.push(summary_line(
            format!(
                "Current {}",
                asof_range_label(output.and_then(|output| output.asof_range_ms))
            ),
            palette.foreground,
        ));
        if !output.is_some_and(|output| output.complete) {
            details = details.push(
                button(text("Retry sources").size(12))
                    .style(button::secondary)
                    .on_press(Message::RetryIndicatorSources(instance.id)),
            );
        }
        details = details.push(text("Contributors").size(14).color(palette.foreground));
        for market in &instance.source.markets {
            let contributor = output.and_then(|output| {
                output.contributors.iter().find(|item| {
                    item.market.exchange == market.exchange
                        && item.market.product == market.product
                        && item.market.symbol == market.symbol
                })
            });
            details = details.push(
                column![
                    summary_line(
                        format!(
                            "{} · {} · {} · {} · {}",
                            market.exchange,
                            market.product,
                            market.symbol,
                            data.market.interval,
                            if contributor.is_some_and(|item| item.complete) {
                                "covered"
                            } else {
                                "incomplete"
                            },
                        ),
                        palette.foreground,
                    ),
                    summary_line(
                        contributor.map_or_else(
                            || "Awaiting catalog / source data · as-of unavailable".into(),
                            |item| format!(
                                "As-of {} · {}",
                                item.through_ms.map_or_else(|| "unavailable".into(), utc_ms),
                                item.message,
                            ),
                        ),
                        palette.foreground,
                    ),
                ]
                .spacing(3)
                .width(Length::Fill),
            );
        }
    } else {
        details = details.push(summary_line(
            format!(
                "{} · {} · {} · {} · unit {}",
                data.market.exchange,
                data.market.product,
                data.market.symbol,
                data.market.interval,
                instance_unit(data, instance, volume_unit),
            ),
            palette.foreground,
        ));
        if instance.spec == IndicatorSpec::OpenInterest {
            let native = &data.open_interest;
            details = details.push(summary_line(open_interest_status(data), palette.foreground));
            if let Some(support) = &native.support {
                let cadence = |interval: Option<i64>| {
                    interval.map_or_else(
                        || "unavailable".into(),
                        |interval| format!("{} s", interval as f64 / 1000.0),
                    )
                };
                details = details
                    .push(summary_line(&support.description, palette.foreground))
                    .push(summary_line(
                        format!(
                            "Native history: {} · live cadence: {}",
                            cadence(support.history_interval_ms),
                            cadence(support.live_interval_ms),
                        ),
                        palette.foreground,
                    ));
            } else {
                details = details.push(summary_line(
                    "Native history / live cadence: awaiting source support",
                    palette.foreground,
                ));
            }
            if !native.message.is_empty() {
                details = details.push(summary_line(&native.message, palette.foreground));
            }
            if let Some(sample) = data.inspected_open_interest_for(instance) {
                details = details.push(summary_line(
                    format!(
                        "Observation cadence: {} s",
                        sample.interval_ms as f64 / 1000.0
                    ),
                    palette.foreground,
                ));
            }
            details = details.push(summary_line(
                "Native observations are not aligned to candles or gap-filled. Inspection selects the last actual observation at or before the cursor; its timestamp and age remain visible.",
                palette.foreground,
            ));
        } else if matches!(instance.spec, IndicatorSpec::Cvd { .. }) {
            details = details.push(summary_line(
                "Exact flow confirms trade side; Reported flow has an unconfirmed side. Missing coverage is never treated as zero flow. The displayed origin and through-time bound the cumulative value.",
                palette.foreground,
            ));
        }
    }
    details.into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::widget::canvas::Program;
    #[test]
    fn native_gap_boundary_uses_source_cadence() {
        let before = OpenInterestSample {
            timestamp_ms: 10_123,
            quantity: 0.0,
            interval_ms: 10_000,
            timestamp_source: OpenInterestTimestamp::Venue,
        };
        let mut after = OpenInterestSample {
            timestamp_ms: 25_123,
            ..before
        };
        assert!(open_interest_contiguous(&before, &after));
        after.timestamp_ms += 1;
        assert!(!open_interest_contiguous(&before, &after));
        after.timestamp_ms = before.timestamp_ms;
        assert!(!open_interest_contiguous(&before, &after));
    }

    #[test]
    fn external_oi_segment_marker_breaks_even_a_short_native_cadence_gap() {
        let mut state = ChartState::default();
        state.dispatch(Action::ApplyIndicator {
            id: None,
            spec: IndicatorSpec::OpenInterest,
            source: mexc_indicators::IndicatorSource {
                mode: mexc_indicators::SourceMode::Single,
                markets: vec![mexc_model::Market::default()],
            },
        });
        let instance = &state.indicators.instances[0];
        let before = OpenInterestSample {
            timestamp_ms: 10_000,
            quantity: 12.0,
            interval_ms: 10_000,
            timestamp_source: OpenInterestTimestamp::Venue,
        };
        let after = OpenInterestSample {
            timestamp_ms: 11_000,
            interval_ms: 0,
            ..before
        };
        assert!(open_interest_contiguous(&before, &after));
        assert!(!oi_segment_contiguous(instance, &before, &after));
        let recovered = OpenInterestSample {
            timestamp_ms: 12_000,
            interval_ms: 10_000,
            ..after
        };
        assert!(oi_segment_contiguous(instance, &after, &recovered));
    }

    fn lower_state() -> ChartState {
        let mut data = ChartState::default();
        data.dispatch(Action::AddIndicator(IndicatorSpec::Rsi { period: 2 }));
        data
    }

    fn input(
        data: &ChartState,
        interaction: &mut Interaction,
        point: Point,
        event: mouse::Event,
    ) -> Option<Action> {
        let cache = Cache::default();
        let chart = Chart {
            data,
            cache: &cache,
            palette: Palette::default(),
            indicator: data.indicators.instances.first(),
            quote: "USDT",
            volume_unit: "base",
            drawings: None,
        };
        let (_, message) = chart.update(
            interaction,
            canvas::Event::Mouse(event),
            Rectangle::with_size(Size::new(400.0 + axis_width(data, "USDT", "base"), 242.0)),
            mouse::Cursor::Available(point),
        );
        match message {
            Some(Message::Action(action)) => Some(action),
            None => None,
            _ => panic!("chart emitted a non-chart action"),
        }
    }

    #[test]
    fn lower_plot_drag_pans_time_and_its_own_values() {
        let mut data = lower_state();
        let mut interaction = Interaction::default();
        let action = input(
            &data,
            &mut interaction,
            Point::new(100.0, 100.0),
            mouse::Event::ButtonPressed(mouse::Button::Left),
        )
        .unwrap();
        data.dispatch(action);
        let before = data.viewport.left;
        let action = input(
            &data,
            &mut interaction,
            Point::new(140.0, 120.0),
            mouse::Event::CursorMoved {
                position: Point::new(140.0, 120.0),
            },
        )
        .unwrap();
        assert!(matches!(action, Action::PanePan { id: 1, time, value }
            if (time + 0.1).abs() < 1e-12 && (value - 0.1).abs() < 1e-12));
        data.dispatch(action);
        assert_eq!(data.viewport.left, before - data.viewport.span * 0.1);
        assert_eq!(data.pane_scale(1).low, 10.0);
        assert!(!data.pane_scale(1).auto_fit);
    }

    #[test]
    fn lower_axes_own_plain_wheel_and_keep_the_opposite_dimension_unchanged() {
        let mut data = lower_state();
        let mut interaction = Interaction::default();
        let wheel = || mouse::Event::WheelScrolled {
            delta: mouse::ScrollDelta::Lines { x: 0.0, y: 1.0 },
        };
        interaction.modifiers = keyboard::Modifiers::ALT;
        assert!(input(&data, &mut interaction, Point::new(100.0, 100.0), wheel()).is_none());
        let before = data.viewport;
        let action = input(&data, &mut interaction, Point::new(410.0, 50.0), wheel()).unwrap();
        data.dispatch(action);
        let scale = data.pane_scale(1);
        assert!((scale.low + (scale.high - scale.low) * 0.75 - 75.0).abs() < 1e-12);
        assert_eq!(data.viewport.span, before.span);
        let action = input(&data, &mut interaction, Point::new(100.0, 210.0), wheel()).unwrap();
        data.dispatch(action);
        assert_eq!(data.pane_scale(1), scale);
        assert!(data.viewport.span < before.span);
        assert!(
            (data.viewport.left + data.viewport.span * 0.25 - (before.left + before.span * 0.25))
                .abs()
                < 1e-9
        );
    }

    #[test]
    fn value_axis_double_click_restores_autofit_without_time_reset() {
        let mut data = lower_state();
        data.dispatch(Action::PanePan {
            id: 1,
            time: -0.5,
            value: 0.25,
        });
        let before = data.viewport;
        let mut interaction = Interaction::default();
        let point = Point::new(410.0, 50.0);
        let action = input(
            &data,
            &mut interaction,
            point,
            mouse::Event::ButtonPressed(mouse::Button::Left),
        )
        .unwrap();
        data.dispatch(action);
        let _ = input(
            &data,
            &mut interaction,
            point,
            mouse::Event::ButtonReleased(mouse::Button::Left),
        );
        let action = input(
            &data,
            &mut interaction,
            point,
            mouse::Event::ButtonPressed(mouse::Button::Left),
        )
        .unwrap();
        assert!(matches!(action, Action::FitPane(1)));
        data.dispatch(action);
        assert!(data.pane_scale(1).auto_fit);
        assert_eq!(data.pane_scale(1).low, 0.0);
        assert_eq!(data.pane_scale(1).high, 100.0);
        assert_eq!(data.viewport.left, before.left);
        assert_eq!(data.viewport.span, before.span);
    }

    #[test]
    fn axis_drags_anchor_at_the_press_position_and_use_independent_dimensions() {
        let mut data = lower_state();
        let mut interaction = Interaction::default();
        let right = Point::new(410.0, 50.0);
        let action = input(
            &data,
            &mut interaction,
            right,
            mouse::Event::ButtonPressed(mouse::Button::Left),
        )
        .unwrap();
        data.dispatch(action);
        let before = data.viewport;
        let action = input(
            &data,
            &mut interaction,
            Point::new(410.0, 70.0),
            mouse::Event::CursorMoved {
                position: Point::new(410.0, 70.0),
            },
        )
        .unwrap();
        data.dispatch(action);
        let scale = data.pane_scale(1);
        assert!((scale.low + (scale.high - scale.low) * 0.75 - 75.0).abs() < 1e-12);
        assert_eq!(data.viewport.span, before.span);
        let _ = input(
            &data,
            &mut interaction,
            Point::new(410.0, 70.0),
            mouse::Event::ButtonReleased(mouse::Button::Left),
        );
        let action = input(
            &data,
            &mut interaction,
            Point::new(100.0, 210.0),
            mouse::Event::ButtonPressed(mouse::Button::Left),
        )
        .unwrap();
        data.dispatch(action);
        let action = input(
            &data,
            &mut interaction,
            Point::new(140.0, 210.0),
            mouse::Event::CursorMoved {
                position: Point::new(140.0, 210.0),
            },
        )
        .unwrap();
        data.dispatch(action);
        assert_eq!(data.pane_scale(1), scale);
        assert!(
            (data.viewport.left + data.viewport.span * 0.25 - (before.left + before.span * 0.25))
                .abs()
                < 1e-9
        );
    }

    #[test]
    fn shared_gutter_budgets_precision_without_following_manual_scales() {
        let mut data = lower_state();
        let ordinary = axis_width(&data, "USDT", "contracts");
        assert!((96.0..=105.0).contains(&ordinary));
        data.price_scale = 12;
        let precise = axis_width(&data, "USDT", "contracts");
        assert!(precise > ordinary);
        assert!(precise <= 158.0);
        data.viewport.high = 1e20;
        assert_eq!(axis_width(&data, "USDT", "contracts"), precise);
        assert_eq!(
            axis_width(&data, "USDT", "exceptionally long native unit"),
            158.0
        );
    }

    #[test]
    fn price_ticks_keep_native_small_values_and_distinguish_wide_bounds() {
        let mut data = ChartState::default();
        data.price_scale = 9;
        let cache = Cache::default();
        let chart = Chart {
            data: &data,
            cache: &cache,
            palette: Palette::default(),
            indicator: None,
            quote: "USDC",
            volume_unit: "contracts",
            drawings: None,
        };
        assert_eq!(chart.price_tick_label(0.000000123, 1e-9), "0.000000123");
        assert_eq!(chart.price_tick_label(12_000.0, 1000.0), "12000");
        let first = chart.price_tick_label(1e100, 1e98);
        let second = chart.price_tick_label(1.01e100, 1e98);
        assert_ne!(first, second);
        assert_eq!(first.parse::<f64>().unwrap(), 1e100);
    }

    #[test]
    fn plot_wheel_pixel_and_modifier_policy_matches_price_and_lower_panes() {
        for data in [ChartState::default(), lower_state()] {
            let mut interaction = Interaction::default();
            let point = Point::new(100.0, 100.0);
            let wheel = || mouse::Event::WheelScrolled {
                delta: mouse::ScrollDelta::Lines { x: 0.0, y: 1.0 },
            };
            let time_factor = |action: Action| match action {
                Action::Zoom { time, price, x, y } => (time, price, x, y),
                Action::PaneZoom {
                    time, value, x, y, ..
                } => (time, value, x, y),
                _ => panic!("expected zoom"),
            };
            let (time, value, x, y) =
                time_factor(input(&data, &mut interaction, point, wheel()).unwrap());
            assert!(time < 1.0);
            assert_eq!((value, x, y), (1.0, 0.25, 0.5));
            let pixels = || mouse::Event::WheelScrolled {
                delta: mouse::ScrollDelta::Pixels { x: 0.0, y: 40.0 },
            };
            let action = input(&data, &mut interaction, point, pixels()).unwrap();
            match action {
                Action::Pan { time, price } => assert_eq!((time, price), (-0.1, 0.0)),
                Action::PanePan { time, value, .. } => assert_eq!((time, value), (-0.1, 0.0)),
                _ => panic!("pixel scroll must only pan time"),
            }
            interaction.modifiers = keyboard::Modifiers::CTRL;
            let (time, value, _, _) =
                time_factor(input(&data, &mut interaction, point, pixels()).unwrap());
            assert!(time < 1.0);
            assert_eq!(value, 1.0);
            interaction.modifiers = keyboard::Modifiers::SHIFT;
            let (time, value, _, _) =
                time_factor(input(&data, &mut interaction, point, pixels()).unwrap());
            assert_eq!(time, 1.0);
            assert!(value < 1.0);
            interaction.modifiers = keyboard::Modifiers::ALT | keyboard::Modifiers::SHIFT;
            assert!(input(&data, &mut interaction, point, wheel()).is_none());
            let (time, value, _, _) = time_factor(
                input(&data, &mut interaction, Point::new(100.0, 210.0), wheel()).unwrap(),
            );
            assert!(time < 1.0);
            assert_eq!(value, 1.0);
            let (time, value, _, _) = time_factor(
                input(&data, &mut interaction, Point::new(410.0, 50.0), wheel()).unwrap(),
            );
            assert_eq!(time, 1.0);
            assert!(value < 1.0);
        }
    }
    #[test]
    fn offscreen_and_crossing_series_are_clipped_before_dash_expansion() {
        let x = (0.0, 10.0);
        let y = (-1.0, 1.0);
        assert_eq!(clip_segment((0.0, 1e20), (10.0, 2e20), x, y), None);
        assert_eq!(
            clip_segment((-10.0, 0.0), (20.0, 0.0), x, y),
            Some([(0.0, 0.0), (10.0, 0.0)]),
        );
        assert_eq!(
            clip_segment((0.0, -1e20), (10.0, 1e20), x, y),
            Some([(5.0, -1.0), (5.0, 1.0)]),
        );
        assert_eq!(
            clip_segment((10.0, 1e20), (0.0, -1e20), x, y),
            Some([(5.0, 1.0), (5.0, -1.0)]),
        );
    }
}
