use crate::{drawing_types::*, palette::Palette, state::Viewport};
use iced::advanced::graphics::text::{self as native_text, cosmic_text};
use iced::widget::canvas::{self, Path, Stroke};
use iced::{Color, Font, Point, Rectangle, Size, Vector};
use mexc_model::Candle;
use std::{
    cell::RefCell,
    collections::VecDeque,
    hash::{Hash, Hasher},
};

const PICK_RADIUS: f32 = 7.0;
const HANDLE_RADIUS: f32 = 8.0;
const MAGNET_RADIUS: f32 = 12.0;

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Projection {
    pub area: Rectangle,
    pub left: f64,
    pub span: f64,
    pub low: f64,
    pub high: f64,
}

impl Projection {
    pub fn new(viewport: Viewport, area: Rectangle) -> Option<Self> {
        let projection = Self {
            area,
            left: viewport.left,
            span: viewport.span,
            low: viewport.low,
            high: viewport.high,
        };
        (area.x == 0.0
            && area.y == 0.0
            && area.width.is_finite()
            && area.height.is_finite()
            && area.width > 0.0
            && area.height > 0.0
            && projection.left.is_finite()
            && projection.span.is_finite()
            && projection.span > 0.0
            && (projection.left + projection.span).is_finite()
            && projection.left + projection.span > projection.left
            && projection.low.is_finite()
            && projection.high.is_finite()
            && projection.high > projection.low
            && (projection.high - projection.low).is_finite())
        .then_some(projection)
    }

    pub fn anchor(self, point: Point) -> Option<Anchor> {
        if !finite_point(point) {
            return None;
        }
        let seconds =
            (f64::from(point.x) / f64::from(self.area.width)).mul_add(self.span, self.left);
        let price = (1.0 - f64::from(point.y) / f64::from(self.area.height))
            .mul_add(self.high - self.low, self.low);
        let utc_ms = (seconds * 1000.0).round();
        // i64::MAX as f64 rounds upward, so the upper bound is exclusive.
        if !utc_ms.is_finite()
            || utc_ms < i64::MIN as f64
            || utc_ms >= -(i64::MIN as f64)
            || !price.is_finite()
        {
            return None;
        }
        Some(Anchor {
            utc_ms: utc_ms as i64,
            price,
        })
    }

    pub fn point(self, anchor: Anchor) -> Point {
        self.data_point(DataPoint::from(anchor))
    }

    pub fn contains(self, point: Point) -> bool {
        finite_point(point) && self.area.contains(point)
    }

    fn data_point(self, point: DataPoint) -> Point {
        Point::new(
            (((point.x - self.left) / self.span) * f64::from(self.area.width)) as f32,
            (((self.high - point.y) / (self.high - self.low)) * f64::from(self.area.height)) as f32,
        )
    }

    fn includes(self, point: DataPoint) -> bool {
        point.finite()
            && point.x >= self.left
            && point.x <= self.left + self.span
            && point.y >= self.low
            && point.y <= self.high
    }
}

fn finite_point(point: Point) -> bool {
    point.x.is_finite() && point.y.is_finite()
}

#[derive(Clone, Copy, Debug)]
struct DataPoint {
    x: f64,
    y: f64,
}
impl From<Anchor> for DataPoint {
    fn from(anchor: Anchor) -> Self {
        Self {
            x: anchor.utc_ms as f64 / 1000.0,
            y: anchor.price,
        }
    }
}
impl DataPoint {
    fn finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }
}

/// Intersect with the four data-space edges, not an enormous pixel-space path.
/// The fixed candidate array also bounds ray work independently of its length.
fn clip_line(
    projection: Projection,
    a: DataPoint,
    b: DataPoint,
    ray: bool,
) -> Option<(Point, Point)> {
    if !a.finite() || !b.finite() {
        return None;
    }
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    if !dx.is_finite() || !dy.is_finite() {
        return None;
    }
    if dx == 0.0 && dy == 0.0 {
        return projection.includes(a).then(|| {
            let p = projection.data_point(a);
            (p, p)
        });
    }
    let mut candidates = [a; 6];
    let mut count = 0;
    let along_x = dx.abs() >= dy.abs();
    let origin = if along_x { a.x } else { a.y };
    let end = if along_x { b.x } else { b.y };
    let direction = end - origin;
    let mut add = |point: DataPoint| {
        if !projection.includes(point) {
            return;
        }
        let value = if along_x { point.x } else { point.y };
        let forward = if direction > 0.0 {
            value >= origin
        } else {
            value <= origin
        };
        let before_end = if direction > 0.0 {
            value <= end
        } else {
            value >= end
        };
        if forward && (ray || before_end) {
            candidates[count] = point;
            count += 1;
        }
    };
    add(a);
    if !ray {
        add(b);
    }
    if dx != 0.0 {
        for x in [projection.left, projection.left + projection.span] {
            add(DataPoint {
                x,
                y: (dy / dx).mul_add(x - a.x, a.y),
            });
        }
    }
    if dy != 0.0 {
        for y in [projection.low, projection.high] {
            add(DataPoint {
                x: (dx / dy).mul_add(y - a.y, a.x),
                y,
            });
        }
    }
    if count == 0 {
        return None;
    }
    let key = |p: DataPoint| if along_x { p.x } else { p.y };
    let mut first = candidates[0];
    let mut last = first;
    for point in &candidates[1..count] {
        if key(*point) < key(first) {
            first = *point;
        }
        if key(*point) > key(last) {
            last = *point;
        }
    }
    Some((projection.data_point(first), projection.data_point(last)))
}

pub fn fib_price(a: f64, b: f64, ratio: f64, reverse: bool) -> Option<f64> {
    if !a.is_finite() || !b.is_finite() || !ratio.is_finite() {
        return None;
    }
    let (start, end) = if reverse { (b, a) } else { (a, b) };
    let value = ratio.mul_add(start - end, end);
    value.is_finite().then_some(value)
}

#[derive(Clone, Copy)]
struct ShapeView<'a> {
    tool: Tool,
    anchors: Anchors,
    levels: &'a [f64],
    reverse: bool,
    extend_right: bool,
    text: Option<&'a str>,
    vwap: Option<&'a crate::drawings::AnchoredVwap>,
}
impl<'a> ShapeView<'a> {
    fn drawing(drawing: &'a Drawing, anchors: Anchors) -> Self {
        let (levels, reverse, extend_right, text) = match &drawing.shape {
            Shape::Fibonacci {
                levels,
                reverse,
                extend_right,
                ..
            } => (levels.as_slice(), *reverse, *extend_right, None),
            Shape::Text { text, .. } => (&[][..], false, false, Some(text.as_str())),
            _ => (&[][..], false, false, None),
        };
        Self {
            tool: drawing.shape.tool(),
            anchors,
            levels,
            reverse,
            extend_right,
            text,
            vwap: None,
        }
    }
    fn create(tool: Tool, anchors: Anchors) -> Self {
        Self {
            tool,
            anchors,
            levels: &FIB_LEVELS,
            reverse: false,
            extend_right: true,
            text: None,
            vwap: None,
        }
    }

    fn cached(
        drawing: &'a Drawing,
        anchors: Anchors,
        store: Option<&'a crate::drawings::Store>,
    ) -> Self {
        let mut view = Self::drawing(drawing, anchors);
        if matches!(drawing.shape, Shape::AnchoredVwap(_)) {
            view.vwap = store.and_then(|store| store.vwap(drawing.id));
            if let Some(start) = view.vwap.and_then(crate::drawings::AnchoredVwap::start) {
                view.anchors = Anchors::One(Anchor {
                    price: start.price,
                    ..anchors.first()
                });
            }
        }
        view
    }
}

fn corners(anchors: Anchors) -> Option<[Anchor; 4]> {
    let a = anchors.first();
    let b = anchors.second()?;
    let left = a.utc_ms.min(b.utc_ms);
    let right = a.utc_ms.max(b.utc_ms);
    let top = a.price.max(b.price);
    let bottom = a.price.min(b.price);
    Some([
        Anchor {
            utc_ms: left,
            price: top,
        },
        Anchor {
            utc_ms: right,
            price: top,
        },
        Anchor {
            utc_ms: right,
            price: bottom,
        },
        Anchor {
            utc_ms: left,
            price: bottom,
        },
    ])
}

fn visit_handles(view: ShapeView<'_>, projection: Projection, mut visit: impl FnMut(Point, Part)) {
    if view.tool == Tool::AnchoredVwap
        && view
            .vwap
            .and_then(crate::drawings::AnchoredVwap::start)
            .is_none()
    {
        return;
    }
    if view.tool == Tool::Rectangle {
        if let Some(corners) = corners(view.anchors) {
            for (index, anchor) in corners.into_iter().enumerate() {
                let point = projection.point(anchor);
                if projection.contains(point) {
                    visit(point, Part::Corner(index as u8));
                }
            }
        }
        return;
    }
    let mut first = projection.point(view.anchors.first());
    match view.tool {
        Tool::Horizontal => first.x = first.x.clamp(0.0, projection.area.width),
        Tool::Vertical => first.y = first.y.clamp(0.0, projection.area.height),
        _ => {}
    }
    if projection.contains(first) {
        visit(first, Part::First);
    }
    if let Some(anchor) = view.anchors.second() {
        let second = projection.point(anchor);
        if projection.contains(second) {
            visit(second, Part::Second);
        }
    }
}

fn handle_hit(view: ShapeView<'_>, projection: Projection, point: Point) -> Option<Part> {
    let mut nearest = None;
    let mut distance = HANDLE_RADIUS * HANDLE_RADIUS;
    visit_handles(view, projection, |handle, part| {
        let d = distance_squared(point, handle);
        if d <= distance {
            nearest = Some(part);
            distance = d;
        }
    });
    nearest
}

#[cfg(test)]
pub fn hit_test(
    objects: &[Drawing],
    selected: Option<DrawingId>,
    projection: Projection,
    point: Point,
) -> Option<Hit> {
    hit_test_cached(objects, selected, projection, point, None)
}

pub fn hit_test_cached(
    objects: &[Drawing],
    selected: Option<DrawingId>,
    projection: Projection,
    point: Point,
    store: Option<&crate::drawings::Store>,
) -> Option<Hit> {
    if !projection.contains(point) {
        return None;
    }
    if let Some(drawing) = selected.and_then(|id| objects.iter().find(|d| d.id == id && d.visible))
    {
        let view = ShapeView::cached(drawing, drawing.shape.anchors(), store);
        if let Some(text) = view.text {
            pin_text_layout(text, font_size(drawing.style));
        }
        if let Some(part) = handle_hit(view, projection, point) {
            return Some(Hit {
                id: drawing.id,
                part,
            });
        }
    }
    for drawing in objects.iter().rev().filter(|d| d.visible) {
        let view = ShapeView::cached(drawing, drawing.shape.anchors(), store);
        if body_hit(view, drawing.style, projection, point) {
            if selected.is_none()
                && let Some(text) = view.text
            {
                pin_text_layout(text, font_size(drawing.style));
            }
            return Some(Hit {
                id: drawing.id,
                part: Part::Body,
            });
        }
    }
    None
}

fn distance_squared(a: Point, b: Point) -> f32 {
    (a.x - b.x).powi(2) + (a.y - b.y).powi(2)
}
fn segment_distance(point: Point, a: Point, b: Point) -> f32 {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let length = dx * dx + dy * dy;
    if length == 0.0 {
        return distance_squared(point, a).sqrt();
    }
    let t = (((point.x - a.x) * dx + (point.y - a.y) * dy) / length).clamp(0.0, 1.0);
    distance_squared(point, Point::new(dx.mul_add(t, a.x), dy.mul_add(t, a.y))).sqrt()
}

fn visit_lines(view: ShapeView<'_>, projection: Projection, mut visit: impl FnMut(Point, Point)) {
    if view.tool == Tool::AnchoredVwap {
        if let Some(cache) = view.vwap {
            let start = cache
                .points
                .partition_point(|point| (point.time as f64) < projection.left)
                .saturating_sub(1);
            let end = cache
                .points
                .partition_point(|point| point.time as f64 <= projection.left + projection.span)
                .saturating_add(1);
            let mut previous: Option<(DataPoint, i64)> = None;
            for point in &cache.points[start..end.min(cache.points.len())] {
                let Some(price) = point.values[0] else {
                    previous = None;
                    continue;
                };
                let next = DataPoint {
                    x: point.time as f64,
                    y: price,
                };
                if let Some((previous_point, time)) = previous
                    && point.time.saturating_sub(time).saturating_mul(1000) == cache.step_ms
                    && let Some((a, b)) = clip_line(projection, previous_point, next, false)
                {
                    visit(a, b);
                }
                previous = Some((next, point.time));
            }
        }
        return;
    }
    let a = DataPoint::from(view.anchors.first());
    let b = view.anchors.second().map(DataPoint::from);
    let mut line = |a, b, ray| {
        if let Some((a, b)) = clip_line(projection, a, b, ray) {
            visit(a, b);
        }
    };
    match view.tool {
        Tool::Horizontal => line(
            DataPoint {
                x: projection.left,
                y: a.y,
            },
            DataPoint {
                x: projection.left + projection.span,
                y: a.y,
            },
            false,
        ),
        Tool::Vertical => line(
            DataPoint {
                x: a.x,
                y: projection.low,
            },
            DataPoint {
                x: a.x,
                y: projection.high,
            },
            false,
        ),
        Tool::Trend | Tool::Ray | Tool::Measure => {
            if let Some(b) = b {
                line(a, b, view.tool == Tool::Ray);
            }
        }
        Tool::Rectangle => {
            if let Some(corners) = corners(view.anchors) {
                for index in 0..4 {
                    line(
                        corners[index].into(),
                        corners[(index + 1) % 4].into(),
                        false,
                    );
                }
            }
        }
        Tool::Fibonacci => {
            if let Some(b) = b {
                line(a, b, false);
                let right = if view.extend_right {
                    projection.left + projection.span
                } else {
                    a.x.max(b.x)
                };
                if a.x.min(b.x) > right {
                    return;
                }
                for ratio in view.levels {
                    if let Some(y) = fib_price(a.y, b.y, *ratio, view.reverse) {
                        line(
                            DataPoint { x: a.x.min(b.x), y },
                            DataPoint { x: right, y },
                            false,
                        );
                    }
                }
            }
        }
        _ => {}
    }
}

fn rectangle_bounds(view: ShapeView<'_>, projection: Projection) -> Option<Rectangle> {
    let a = DataPoint::from(view.anchors.first());
    let b = DataPoint::from(view.anchors.second()?);
    if !a.finite() || !b.finite() {
        return None;
    }
    let left = a.x.min(b.x).max(projection.left);
    let right = a.x.max(b.x).min(projection.left + projection.span);
    let low = a.y.min(b.y).max(projection.low);
    let high = a.y.max(b.y).min(projection.high);
    if left > right || low > high {
        return None;
    }
    let nw = projection.data_point(DataPoint { x: left, y: high });
    let se = projection.data_point(DataPoint { x: right, y: low });
    Some(Rectangle::new(nw, Size::new(se.x - nw.x, se.y - nw.y)))
}

fn body_hit(view: ShapeView<'_>, style: Style, projection: Projection, point: Point) -> bool {
    if view.tool == Tool::AnchoredVwap
        && let Some(start) = view.vwap.and_then(crate::drawings::AnchoredVwap::start)
        && distance_squared(point, projection.point(start)) <= PICK_RADIUS * PICK_RADIUS
    {
        return true;
    }
    if view.tool == Tool::Rectangle
        && rectangle_bounds(view, projection).is_some_and(|r| r.contains(point))
    {
        return true;
    }
    if let Some(text) = view.text {
        let at = projection.point(view.anchors.first());
        if !finite_point(at) {
            return false;
        }
        let size = font_size(style);
        let contains = |bounds: Rectangle| {
            Rectangle {
                x: at.x + bounds.x - 3.0,
                y: at.y + bounds.y - 3.0,
                width: bounds.width + 6.0,
                height: bounds.height + 6.0,
            }
            .contains(point)
        };
        if let Some(bounds) = cached_text_bounds(text, size) {
            return contains(bounds);
        }
        // Reject distant uncached annotations before native shaping. This is an
        // intentionally generous font-em envelope, not the final pick bounds.
        let margin = size * 8.0;
        let rows = text.bytes().filter(|byte| *byte == b'\n').count() + 1;
        let envelope = Rectangle::new(
            Point::new(at.x - margin, at.y - margin),
            Size::new(
                text.len() as f32 * margin + margin * 2.0,
                rows as f32 * margin + margin * 2.0,
            ),
        );
        if !envelope.contains(point) {
            return false;
        }
        return with_text_layout(text, size, |layout| contains(layout.bounds));
    }
    let tolerance = PICK_RADIUS + line_width(style) * 0.5;
    let mut hit = false;
    visit_lines(view, projection, |a, b| {
        hit |= segment_distance(point, a, b) <= tolerance;
    });
    hit
}

pub fn magnet<'a>(
    projection: Projection,
    point: Point,
    candles: impl Iterator<Item = &'a Candle>,
) -> Option<Anchor> {
    if !projection.contains(point) {
        return None;
    }
    let mut best = None;
    let mut distance = MAGNET_RADIUS * MAGNET_RADIUS;
    for candle in candles {
        let Some(utc_ms) = candle.time.checked_mul(1000) else {
            continue;
        };
        let x = projection
            .point(Anchor {
                utc_ms,
                price: projection.low,
            })
            .x;
        if !x.is_finite() || (x - point.x).abs() > MAGNET_RADIUS {
            continue;
        }
        for price in [candle.open, candle.high, candle.low, candle.close] {
            let anchor = Anchor { utc_ms, price };
            let candidate = projection.point(anchor);
            if !price.is_finite() || !projection.contains(candidate) {
                continue;
            }
            let d = distance_squared(point, candidate);
            if d <= distance && (best.is_none() || d < distance) {
                distance = d;
                best = Some(anchor);
            }
        }
    }
    best
}

fn ink(style: Style, palette: Palette) -> Color {
    match style.ink {
        Ink::Accent => palette.accent,
        Ink::Foreground => palette.foreground,
        Ink::Up => palette.green,
        Ink::Down => palette.red,
    }
}
fn line_width(style: Style) -> f32 {
    if style.width.is_finite() {
        style.width.clamp(0.5, 20.0)
    } else {
        2.0
    }
}
fn font_size(style: Style) -> f32 {
    if style.font_size.is_finite() {
        style.font_size.clamp(8.0, 128.0)
    } else {
        14.0
    }
}
fn stroke(frame: &mut canvas::Frame, a: Point, b: Point, style: Style, color: Color) {
    // Clip first: dash expansion is bounded by the plot, not off-screen anchors.
    frame.stroke(&Path::line(a, b), stroke_style(style, color));
}

fn stroke_style(style: Style, color: Color) -> Stroke<'static> {
    let segments: &'static [f32] = match style.pattern {
        Pattern::Solid => &[],
        Pattern::Dashed => &[8.0, 5.0],
        Pattern::Dotted => &[2.0, 4.0],
    };
    Stroke {
        line_dash: canvas::LineDash {
            segments,
            offset: 0,
        },
        ..Stroke::default()
            .with_color(color)
            .with_width(line_width(style))
    }
}

pub struct NativeLabels<'a> {
    pub price_scale: u32,
    pub quote: &'a str,
    pub candles: &'a [Candle],
}

pub fn native_anchor(projection: Projection, point: Point, candles: &[Candle]) -> Option<Anchor> {
    let at = projection.anchor(point)?;
    let next = candles.partition_point(|candle| candle.time.saturating_mul(1000) < at.utc_ms);
    let before = next.checked_sub(1).and_then(|index| candles.get(index));
    let after = candles.get(next);
    let candle = match (before, after) {
        (Some(a), Some(b)) => {
            if at.utc_ms.abs_diff(a.time.saturating_mul(1000))
                <= at.utc_ms.abs_diff(b.time.saturating_mul(1000))
            {
                a
            } else {
                b
            }
        }
        (Some(candle), None) | (None, Some(candle)) => candle,
        (None, None) => return None,
    };
    Some(Anchor {
        utc_ms: candle.time.checked_mul(1000)?,
        price: 0.0,
    })
}

pub fn draw_committed(
    frame: &mut canvas::Frame,
    objects: &[Drawing],
    projection: Projection,
    palette: Palette,
    labels: &NativeLabels<'_>,
    skip: Option<DrawingId>,
    store: Option<&crate::drawings::Store>,
) {
    frame.with_clip(projection.area, |frame| {
        for drawing in objects.iter().filter(|d| d.visible && Some(d.id) != skip) {
            render(
                frame,
                ShapeView::cached(drawing, drawing.shape.anchors(), store),
                drawing.style,
                projection,
                palette,
                labels,
            );
        }
    });
}

#[allow(clippy::too_many_arguments)]
pub fn draw_overlay(
    frame: &mut canvas::Frame,
    objects: &[Drawing],
    selected: Option<DrawingId>,
    hover: Option<Hit>,
    preview: Option<Preview<'_>>,
    projection: Projection,
    palette: Palette,
    price_scale: u32,
    quote: &str,
    native_candles: &[Candle],
    store: Option<&crate::drawings::Store>,
) {
    let labels = NativeLabels {
        price_scale,
        quote,
        candles: native_candles,
    };
    frame.with_clip(projection.area, |frame| {
        let edited = match &preview {
            Some(Preview::Edit { drawing, .. }) => Some(drawing.id),
            _ => None,
        };
        if let Some(hit) = hover.filter(|hit| Some(hit.id) != selected && Some(hit.id) != edited)
            && let Some(drawing) = objects.iter().find(|d| d.id == hit.id && d.visible)
        {
            selection(
                frame,
                ShapeView::cached(drawing, drawing.shape.anchors(), store),
                drawing.style,
                projection,
                palette,
                false,
            );
        }
        if let Some(drawing) = selected.and_then(|id| {
            objects
                .iter()
                .find(|d| d.id == id && d.visible && Some(id) != edited)
        }) {
            selection(
                frame,
                ShapeView::cached(drawing, drawing.shape.anchors(), store),
                drawing.style,
                projection,
                palette,
                true,
            );
        }
        match preview {
            Some(Preview::Create { tool, anchors }) => {
                let view = ShapeView::create(tool, anchors);
                render(frame, view, Style::default(), projection, palette, &labels);
                selection(frame, view, Style::default(), projection, palette, true);
            }
            Some(Preview::Edit { drawing, anchors }) if drawing.visible => {
                let view = ShapeView::cached(drawing, anchors, store);
                render(frame, view, drawing.style, projection, palette, &labels);
                selection(frame, view, drawing.style, projection, palette, true);
            }
            _ => {}
        }
    });
}

fn selection(
    frame: &mut canvas::Frame,
    view: ShapeView<'_>,
    style: Style,
    projection: Projection,
    palette: Palette,
    handles: bool,
) {
    let color = Color {
        a: if handles { 0.85 } else { 0.5 },
        ..palette.foreground
    };
    let outline = Style {
        width: line_width(style) + 2.0,
        ..style
    };
    if let Some(text) = view.text {
        if handles {
            pin_text_layout(text, font_size(style));
        }
        let at = projection.point(view.anchors.first());
        if finite_point(at) {
            with_text_layout(text, font_size(style), |layout| {
                let rect = Rectangle {
                    x: at.x + layout.bounds.x - 3.0,
                    y: at.y + layout.bounds.y - 3.0,
                    width: layout.bounds.width + 6.0,
                    height: layout.bounds.height + 6.0,
                };
                if let Some(rect) = rect.intersection(&projection.area) {
                    frame.stroke_rectangle(
                        rect.position(),
                        rect.size(),
                        Stroke::default().with_color(color).with_width(1.0),
                    );
                }
            });
        }
    } else {
        visit_lines(view, projection, |a, b| {
            let path = Path::line(a, b);
            frame.stroke(&path, stroke_style(outline, color));
            frame.stroke(&path, stroke_style(style, ink(style, palette)));
        });
    }
    if handles {
        visit_handles(view, projection, |point, _| {
            let handle = Path::rectangle(
                Point::new(point.x - 3.5, point.y - 3.5),
                Size::new(7.0, 7.0),
            );
            frame.fill(&handle, palette.background);
            frame.stroke(
                &handle,
                Stroke::default()
                    .with_color(palette.foreground)
                    .with_width(1.5),
            );
        });
    }
}

fn render(
    frame: &mut canvas::Frame,
    view: ShapeView<'_>,
    style: Style,
    projection: Projection,
    palette: Palette,
    labels: &NativeLabels<'_>,
) {
    let price_scale = labels.price_scale;
    let quote = labels.quote;
    let native_candles = labels.candles;
    let color = ink(style, palette);
    if view.tool == Tool::Rectangle
        && let Some(bounds) = rectangle_bounds(view, projection)
    {
        let opacity = if style.fill_opacity.is_finite() {
            style.fill_opacity.clamp(0.0, 1.0)
        } else {
            0.08
        };
        frame.fill_rectangle(
            bounds.position(),
            bounds.size(),
            Color {
                a: opacity,
                ..color
            },
        );
    }
    if let Some(text) = view.text {
        let at = projection.point(view.anchors.first());
        if finite_point(at) {
            paint_text(frame, text, font_size(style), at, color, projection.area);
        }
        return;
    }
    if view.tool == Tool::Text {
        let point = projection.point(view.anchors.first());
        if projection.contains(point) {
            frame.stroke(
                &Path::circle(point, 5.0),
                Stroke::default().with_color(color).with_width(1.5),
            );
        }
        return;
    }
    let mut visible_segment = None;
    visit_lines(view, projection, |a, b| {
        stroke(frame, a, b, style, color);
        visible_segment = Some((a, b));
    });
    if view.tool == Tool::AnchoredVwap {
        if visible_segment.is_none()
            && let Some(start) = view.vwap.and_then(crate::drawings::AnchoredVwap::start)
            && projection.contains(projection.point(start))
        {
            frame.fill(
                &Path::circle(projection.point(start), line_width(style).max(2.0)),
                color,
            );
        }
        let label = view.vwap.map_or(
            "Anchored VWAP · unavailable",
            crate::drawings::AnchoredVwap::label,
        );
        let start = projection.point(view.anchors.first());
        let at = visible_segment.map_or(
            Point::new(
                start.x.clamp(4.0, (projection.area.width - 160.0).max(4.0)),
                16.0,
            ),
            |(_, last)| Point::new((last.x - 240.0).max(4.0), (last.y - 16.0).max(14.0)),
        );
        paint_text(frame, label, 11.0, at, color, projection.area);
    }
    if view.tool == Tool::Measure
        && let Some((pa, pb)) = visible_segment
        && let Some(b) = view.anchors.second()
    {
        let a = view.anchors.first();
        let stats = measure_stats(a, b, native_candles);
        let percent = if a.price == 0.0 {
            "undefined".to_owned()
        } else {
            stats.percent.map_or_else(
                || "out of range".to_owned(),
                |value| format!("{value:+.2}%"),
            )
        };
        let label = format!(
            "Δ {} · {}\n{} · {} native bars{}",
            price_label(stats.price_delta, price_scale, quote),
            percent,
            elapsed_label(stats.elapsed_ms),
            stats.bars,
            if stats.partial {
                " · partial history"
            } else {
                ""
            },
        );
        let midpoint = Point::new((pa.x + pb.x) * 0.5, (pa.y + pb.y) * 0.5);
        with_text_layout(&label, 11.0, |layout| {
            let at = Point::new(
                (midpoint.x - layout.bounds.width * 0.5)
                    .clamp(0.0, (projection.area.width - layout.bounds.width).max(0.0))
                    - layout.bounds.x,
                (midpoint.y - layout.bounds.height - 4.0).clamp(
                    0.0,
                    (projection.area.height - layout.bounds.height).max(0.0),
                ) - layout.bounds.y,
            );
            paint_layout(
                frame,
                layout,
                at,
                palette.foreground,
                projection.area,
                Some(palette.background),
            );
        });
    }
    if view.tool == Tool::Fibonacci {
        paint_fib_labels(frame, view, style, projection, palette, price_scale, quote);
    } else if view.tool == Tool::Horizontal {
        let price = view.anchors.first().price;
        let y = projection.point(view.anchors.first()).y;
        if y.is_finite() && y >= 0.0 && y <= projection.area.height {
            let text = price_label(price, price_scale, quote);
            with_text_layout(&text, 11.0, |layout| {
                let at = Point::new(
                    (projection.area.width - layout.bounds.width - 5.0).max(0.0),
                    (y - layout.bounds.height - 3.0).clamp(
                        0.0,
                        (projection.area.height - layout.bounds.height).max(0.0),
                    ),
                );
                paint_layout(
                    frame,
                    layout,
                    at,
                    palette.foreground,
                    projection.area,
                    Some(palette.background),
                );
            });
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeasureStats {
    pub price_delta: f64,
    pub percent: Option<f64>,
    pub elapsed_ms: i64,
    pub bars: usize,
    pub partial: bool,
}

pub fn measure_stats(a: Anchor, b: Anchor, native_candles: &[Candle]) -> MeasureStats {
    let (start, end) = (a.utc_ms.min(b.utc_ms), a.utc_ms.max(b.utc_ms));
    let first = native_candles
        .first()
        .map(|candle| candle.time.saturating_mul(1000));
    let last = native_candles
        .last()
        .map(|candle| candle.time.saturating_mul(1000));
    let partial = first.is_none_or(|first| start < first) || last.is_none_or(|last| end > last);
    let lower = native_candles.partition_point(|candle| candle.time.saturating_mul(1000) < start);
    let upper = native_candles.partition_point(|candle| candle.time.saturating_mul(1000) <= end);
    let delta = b.price - a.price;
    let percent = (a.price != 0.0)
        .then(|| delta / a.price * 100.0)
        .filter(|value| value.is_finite());
    MeasureStats {
        price_delta: delta,
        percent,
        elapsed_ms: b.utc_ms.saturating_sub(a.utc_ms),
        bars: upper.saturating_sub(lower),
        partial,
    }
}

fn elapsed_label(elapsed_ms: i64) -> String {
    let total = elapsed_ms.unsigned_abs() / 1000;
    let days = total / 86_400;
    let hours = (total / 3_600) % 24;
    let minutes = (total / 60) % 60;
    let seconds = total % 60;
    let sign = if elapsed_ms < 0 { "−" } else { "+" };
    if days > 0 {
        format!("{sign}{days}d {hours:02}:{minutes:02}:{seconds:02}")
    } else {
        format!("{sign}{hours:02}:{minutes:02}:{seconds:02}")
    }
}

fn price_label(price: f64, scale: u32, quote: &str) -> String {
    format!(
        "{price:.precision$} {quote}",
        precision = scale.clamp(2, 12) as usize
    )
}

fn paint_fib_labels(
    frame: &mut canvas::Frame,
    view: ShapeView<'_>,
    style: Style,
    projection: Projection,
    palette: Palette,
    price_scale: u32,
    quote: &str,
) {
    let Some(b) = view.anchors.second() else {
        return;
    };
    let a = view.anchors.first();
    let left = (a.utc_ms.min(b.utc_ms) as f64 / 1000.0).max(projection.left);
    let right = if view.extend_right {
        projection.left + projection.span
    } else {
        (a.utc_ms.max(b.utc_ms) as f64 / 1000.0).min(projection.left + projection.span)
    };
    if left > right {
        return;
    }
    // Levels need not be sorted. Suppress colliding labels, but never their lines.
    let mut occupied = [Rectangle::default(); 64];
    let mut count = 0;
    for ratio in view.levels {
        let Some(price) = fib_price(a.price, b.price, *ratio, view.reverse) else {
            continue;
        };
        if price < projection.low || price > projection.high || !(ratio * 100.0).is_finite() {
            continue;
        }
        let point = projection.data_point(DataPoint { x: right, y: price });
        let text = format!(
            "{}% · {}",
            percent(*ratio),
            price_label(price, price_scale, quote)
        );
        with_text_layout(&text, font_size(style).min(12.0), |layout| {
            let x = (point.x - layout.bounds.width - 4.0).max(0.0);
            let y = (point.y - layout.bounds.height - 2.0).clamp(
                0.0,
                (projection.area.height - layout.bounds.height).max(0.0),
            );
            let bounds = Rectangle::new(
                Point::new(x - 2.0, y - 1.0),
                Size::new(layout.bounds.width + 4.0, layout.bounds.height + 2.0),
            );
            if count == occupied.len()
                || occupied[..count]
                    .iter()
                    .any(|other| bounds.intersects(other))
            {
                return;
            }
            occupied[count] = bounds;
            count += 1;
            paint_layout(
                frame,
                layout,
                Point::new(x, y),
                palette.foreground,
                projection.area,
                Some(palette.background),
            );
        });
    }
}

fn percent(ratio: f64) -> String {
    let value = ratio * 100.0;
    let mut label = format!("{value:.4}");
    if label.contains('.') {
        while label.ends_with('0') {
            label.pop();
        }
        if label.ends_with('.') {
            label.pop();
        }
    }
    label
}

// Native shaping and native fallback rasterization share the same font system as
// iced widgets. Paths (rather than Canvas text overlays) obey plot clipping and
// retain the drawing layer order. Cached payloads are borrowed for moving text.
struct TextLayout {
    hash: u64,
    size: f32,
    glyph_weight: usize,
    buffer: cosmic_text::Buffer,
    bounds: Rectangle,
    paths: Vec<Path>,
    pixels: Vec<(Point, Color)>,
}

// Budget shaped payload, not labels: short native-price labels must not churn
// a small entry-count cache during a dense committed-scene rebuild.
const CACHED_GLYPH_BUDGET: usize = 8_192;
struct TextLayouts {
    entries: VecDeque<TextLayout>,
    glyphs: usize,
}
thread_local! {
    static TEXT_LAYOUTS: RefCell<TextLayouts> = const {
        RefCell::new(TextLayouts { entries: VecDeque::new(), glyphs: 0 })
    };
    static ACTIVE_TEXT_LAYOUT: RefCell<Option<TextLayout>> = const { RefCell::new(None) };
}

fn text_hash(text: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

fn layout_matches(layout: &TextLayout, text: &str, size: f32, hash: u64) -> bool {
    layout.hash == hash && layout.size == size && retained_text_matches(&layout.buffer, text)
}

fn cached_text_bounds(text: &str, size: f32) -> Option<Rectangle> {
    let hash = text_hash(text);
    ACTIVE_TEXT_LAYOUT.with(|active| {
        if let Some(layout) = active
            .borrow()
            .as_ref()
            .filter(|layout| layout_matches(layout, text, size, hash))
        {
            return Some(layout.bounds);
        }
        TEXT_LAYOUTS.with(|cache| {
            cache
                .borrow()
                .entries
                .iter()
                .find(|layout| layout_matches(layout, text, size, hash))
                .map(|layout| layout.bounds)
        })
    })
}

fn take_text_layout(text: &str, size: f32, hash: u64) -> TextLayout {
    TEXT_LAYOUTS.with(|cache| {
        let mut cache = cache.borrow_mut();
        let index = cache
            .entries
            .iter()
            .position(|entry| layout_matches(entry, text, size, hash));
        if let Some(index) = index {
            let layout = cache.entries.remove(index).expect("existing layout index");
            cache.glyphs -= layout.glyph_weight;
            layout
        } else {
            make_text_layout(text, size, hash)
        }
    })
}

fn retain_text_layout(layout: TextLayout) {
    if layout.glyph_weight > CACHED_GLYPH_BUDGET {
        return;
    }
    TEXT_LAYOUTS.with(|cache| {
        let mut cache = cache.borrow_mut();
        while cache.glyphs + layout.glyph_weight > CACHED_GLYPH_BUDGET {
            let removed = cache.entries.pop_front().expect("nonempty weighted cache");
            cache.glyphs -= removed.glyph_weight;
        }
        cache.glyphs += layout.glyph_weight;
        cache.entries.push_back(layout);
    });
}

// The active annotation is pinned separately: a committed-scene rebuild cannot
// evict it and force payload shaping/allocation during its pointer drag.
fn pin_text_layout(text: &str, size: f32) {
    let hash = text_hash(text);
    ACTIVE_TEXT_LAYOUT.with(|active| {
        let mut active = active.borrow_mut();
        if active
            .as_ref()
            .is_some_and(|layout| layout_matches(layout, text, size, hash))
        {
            return;
        }
        let layout = take_text_layout(text, size, hash);
        if let Some(previous) = active.replace(layout) {
            retain_text_layout(previous);
        }
    });
}

fn with_text_layout<R>(text: &str, size: f32, use_layout: impl FnOnce(&TextLayout) -> R) -> R {
    let hash = text_hash(text);
    ACTIVE_TEXT_LAYOUT.with(|active| {
        let active = active.borrow();
        if let Some(layout) = active
            .as_ref()
            .filter(|layout| layout_matches(layout, text, size, hash))
        {
            return use_layout(layout);
        }
        let layout = take_text_layout(text, size, hash);
        let result = use_layout(&layout);
        retain_text_layout(layout);
        result
    })
}

fn retained_text_matches(buffer: &cosmic_text::Buffer, text: &str) -> bool {
    buffer
        .lines
        .iter()
        .flat_map(|line| line.text().bytes().chain(line.ending().as_str().bytes()))
        .eq(text.bytes())
}

fn make_text_layout(text: &str, size: f32, hash: u64) -> TextLayout {
    let mut font_system = native_text::font_system()
        .write()
        .expect("native font system");
    let mut buffer = cosmic_text::Buffer::new(
        font_system.raw(),
        cosmic_text::Metrics::new(size, size * 1.2),
    );
    buffer.set_text(
        font_system.raw(),
        text,
        native_text::to_attributes(Font::DEFAULT),
        cosmic_text::Shaping::Advanced,
    );
    buffer.shape_until_scroll(font_system.raw(), false);
    let mut paths = Vec::new();
    let mut pixels = Vec::new();
    let mut cache = cosmic_text::SwashCache::new();
    let mut min = Point::ORIGIN;
    let mut max = Point::ORIGIN;
    let mut glyphs = 0;
    for run in buffer.layout_runs() {
        glyphs += run.glyphs.len();
        max.x = max.x.max(run.line_w);
        max.y = max.y.max(run.line_top + run.line_height);
        for glyph in run.glyphs {
            let physical = glyph.physical((0.0, run.line_y), 1.0);
            let offset = Point::new(physical.x as f32, physical.y as f32);
            if let Some(commands) =
                cache.get_outline_commands(font_system.raw(), physical.cache_key)
            {
                let mut convert = |x: f32, y: f32| {
                    let point = Point::new(x + offset.x, -y + offset.y);
                    min.x = min.x.min(point.x);
                    min.y = min.y.min(point.y);
                    max.x = max.x.max(point.x);
                    max.y = max.y.max(point.y);
                    point
                };
                paths.push(Path::new(|builder| {
                    for command in commands {
                        match command {
                            cosmic_text::Command::MoveTo(p) => builder.move_to(convert(p.x, p.y)),
                            cosmic_text::Command::LineTo(p) => builder.line_to(convert(p.x, p.y)),
                            cosmic_text::Command::CurveTo(a, b, c) => builder.bezier_curve_to(
                                convert(a.x, a.y),
                                convert(b.x, b.y),
                                convert(c.x, c.y),
                            ),
                            cosmic_text::Command::QuadTo(a, b) => {
                                builder.quadratic_curve_to(convert(a.x, a.y), convert(b.x, b.y))
                            }
                            cosmic_text::Command::Close => builder.close(),
                        }
                    }
                }));
            } else {
                // Emoji/color fonts and bitmap-only glyphs must not disappear.
                cache.with_pixels(
                    font_system.raw(),
                    physical.cache_key,
                    cosmic_text::Color::rgba(255, 255, 255, 255),
                    |x, y, color| {
                        if color.a() == 0 {
                            return;
                        }
                        let point = Point::new(x as f32 + offset.x, y as f32 + offset.y);
                        min.x = min.x.min(point.x);
                        min.y = min.y.min(point.y);
                        max.x = max.x.max(point.x + 1.0);
                        max.y = max.y.max(point.y + 1.0);
                        pixels.push((
                            point,
                            Color::from_rgba8(
                                color.r(),
                                color.g(),
                                color.b(),
                                f32::from(color.a()) / 255.0,
                            ),
                        ));
                    },
                );
            }
        }
    }
    TextLayout {
        hash,
        size,
        glyph_weight: glyphs.max(1),
        buffer,
        bounds: Rectangle::new(min, Size::new(max.x - min.x, max.y - min.y)),
        paths,
        pixels,
    }
}

fn paint_text(
    frame: &mut canvas::Frame,
    text: &str,
    size: f32,
    at: Point,
    color: Color,
    clip: Rectangle,
) {
    with_text_layout(text, size, |layout| {
        paint_layout(frame, layout, at, color, clip, None)
    });
}

fn paint_layout(
    frame: &mut canvas::Frame,
    layout: &TextLayout,
    at: Point,
    color: Color,
    clip: Rectangle,
    background: Option<Color>,
) {
    let bounds = Rectangle {
        x: at.x + layout.bounds.x,
        y: at.y + layout.bounds.y,
        ..layout.bounds
    };
    let Some(visible) = bounds.intersection(&clip) else {
        return;
    };
    if let Some(background) = background {
        frame.fill_rectangle(
            visible.position(),
            visible.size(),
            Color {
                a: 0.92,
                ..background
            },
        );
    }
    frame.with_save(|frame| {
        frame.translate(Vector::new(at.x, at.y));
        for path in &layout.paths {
            frame.fill(path, color);
        }
        for (point, pixel) in &layout.pixels {
            let tinted = if pixel.r == pixel.g && pixel.g == pixel.b {
                Color {
                    a: pixel.a * color.a,
                    ..color
                }
            } else {
                Color {
                    a: pixel.a * color.a,
                    ..*pixel
                }
            };
            frame.fill_rectangle(*point, Size::new(1.0, 1.0), tinted);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchored_curve_remains_pickable_to_a_viewport_edge_between_native_candles() {
        let mut store = crate::drawings::Store::new(&mexc_model::Market::default());
        store
            .apply(Command::Create {
                shape: Shape::AnchoredVwap(Anchor {
                    utc_ms: 0,
                    price: 0.0,
                }),
                style: Style::default(),
            })
            .unwrap();
        let id = store.selected().unwrap();
        let candles = [(0, 10.0), (300, 30.0), (600, 50.0)].map(|(time, price)| Candle {
            time,
            open: price,
            high: price,
            low: price,
            close: price,
            volume: 1.0,
            flow: None,
        });
        store.sync_vwap_preview(&candles, mexc_model::Timeframe::Min5, Some(0), None);
        let projection = Projection::new(
            Viewport {
                left: 150.0,
                span: 300.0,
                low: 0.0,
                high: 40.0,
                auto_fit: false,
                live: false,
            },
            Rectangle::new(Point::ORIGIN, Size::new(400.0, 200.0)),
        )
        .unwrap();
        let point = projection.point(Anchor {
            utc_ms: 449_000,
            price: 20.0 + 149.0 / 300.0 * 10.0,
        });
        assert_eq!(
            hit_test_cached(store.objects(), None, projection, point, Some(&store)),
            Some(Hit {
                id,
                part: Part::Body
            })
        );
    }

    #[test]
    fn anchored_curve_hit_testing_uses_derived_prices_and_does_not_bridge_native_gaps() {
        let mut store = crate::drawings::Store::new(&mexc_model::Market::default());
        store
            .apply(Command::Create {
                shape: Shape::AnchoredVwap(Anchor {
                    utc_ms: 0,
                    price: 999.0,
                }),
                style: Style::default(),
            })
            .unwrap();
        let id = store.selected().unwrap();
        let candles = [(0, 10.0), (300, 30.0), (900, 90.0)].map(|(time, price)| Candle {
            time,
            open: price,
            high: price,
            low: price,
            close: price,
            volume: 1.0,
            flow: None,
        });
        store.sync_vwap_preview(&candles, mexc_model::Timeframe::Min5, Some(0), None);
        let projection = Projection::new(
            Viewport {
                left: 0.0,
                span: 1200.0,
                low: 0.0,
                high: 100.0,
                auto_fit: false,
                live: false,
            },
            Rectangle::new(Point::ORIGIN, Size::new(400.0, 200.0)),
        )
        .unwrap();
        let point = |utc_ms, price| projection.point(Anchor { utc_ms, price });
        let revision = store.vwap_revision();
        assert_eq!(
            hit_test_cached(
                store.objects(),
                None,
                projection,
                point(150_000, 15.0),
                Some(&store)
            ),
            Some(Hit {
                id,
                part: Part::Body
            })
        );
        assert!(
            hit_test_cached(
                store.objects(),
                None,
                projection,
                point(600_000, 95.0 / 3.0),
                Some(&store)
            )
            .is_none()
        );
        assert_eq!(
            hit_test_cached(
                store.objects(),
                Some(id),
                projection,
                point(0, 10.0),
                Some(&store)
            ),
            Some(Hit {
                id,
                part: Part::First
            })
        );
        assert_eq!(store.vwap_revision(), revision);
        store
            .apply(Command::SetVisible { id, visible: false })
            .unwrap();
        assert!(
            hit_test_cached(
                store.objects(),
                Some(id),
                projection,
                point(150_000, 15.0),
                Some(&store)
            )
            .is_none()
        );
    }

    fn projection() -> Projection {
        Projection::new(
            Viewport {
                left: 1000.0,
                span: 100.0,
                low: 10.0,
                high: 20.0,
                ..Viewport::default()
            },
            Rectangle::with_size(Size::new(1000.0, 500.0)),
        )
        .unwrap()
    }
    fn anchor(time: i64, price: f64) -> Anchor {
        Anchor {
            utc_ms: time * 1000,
            price,
        }
    }
    fn drawing(id: u64, shape: Shape) -> Drawing {
        Drawing {
            id,
            shape,
            style: Style::default(),
            visible: true,
            locked: false,
        }
    }
    fn near(a: f32, b: f32) {
        assert!((a - b).abs() < 0.01, "{a} != {b}");
    }

    #[test]
    fn measure_counts_sparse_native_bars_and_preserves_reversed_signed_metrics() {
        let candles = [
            Candle {
                time: 1,
                open: 0.0,
                high: 1.0,
                low: 0.0,
                close: 0.0,
                volume: 0.0,
                flow: None,
            },
            Candle {
                time: 3,
                open: 0.0,
                high: 1.0,
                low: 0.0,
                close: 0.0,
                volume: 0.0,
                flow: None,
            },
            Candle {
                time: 5,
                open: 0.0,
                high: 1.0,
                low: 0.0,
                close: 0.0,
                volume: 0.0,
                flow: None,
            },
        ];
        let stats = measure_stats(anchor(5, 0.0), anchor(1, 3.0), &candles);
        assert_eq!(stats.price_delta, 3.0);
        assert_eq!(stats.percent, None);
        assert_eq!(stats.elapsed_ms, -4_000);
        assert_eq!(stats.bars, 3);
        assert!(!stats.partial);

        let outside = measure_stats(anchor(0, 1.0), anchor(6, 2.0), &candles);
        assert_eq!(outside.bars, 3);
        assert!(outside.partial);
        assert_eq!(outside.percent, Some(100.0));
    }
    #[test]
    fn transforms_native_prices_milliseconds_and_captured_outside_drag() {
        let projection = projection();
        let a = Anchor {
            utc_ms: 1_025_125,
            price: 17.25,
        };
        let point = projection.point(a);
        assert_eq!(projection.anchor(point), Some(a));
        let outside = projection.anchor(Point::new(1250.0, -50.0)).unwrap();
        assert_eq!(outside, anchor(1125, 21.0));
        assert!(!projection.contains(Point::new(1250.0, -50.0)));
        assert!(projection.anchor(Point::new(f32::INFINITY, 0.0)).is_none());
    }

    #[test]
    fn rejects_degenerate_or_nonfinite_projection_and_time_overflow() {
        let area = projection().area;
        assert!(
            Projection::new(
                Viewport {
                    span: 0.0,
                    ..Viewport::default()
                },
                area
            )
            .is_none()
        );
        assert!(
            Projection::new(
                Viewport {
                    high: f64::INFINITY,
                    ..Viewport::default()
                },
                area
            )
            .is_none()
        );
        assert!(
            Projection::new(
                Viewport {
                    low: 1.0,
                    high: 1.0,
                    ..Viewport::default()
                },
                area
            )
            .is_none()
        );
        let huge = Projection::new(
            Viewport {
                left: 1e20,
                span: 1e20,
                ..Viewport::default()
            },
            area,
        )
        .unwrap();
        assert!(huge.anchor(Point::ORIGIN).is_none());
    }

    #[test]
    fn tiny_native_prices_do_not_collapse_or_get_rounded_to_cents() {
        let projection = Projection::new(
            Viewport {
                low: 1e-12,
                high: 2e-12,
                ..Viewport::default()
            },
            projection().area,
        )
        .unwrap();
        let a = Anchor {
            utc_ms: 0,
            price: 1.5e-12,
        };
        near(projection.point(a).y, 250.0);
        assert!((projection.anchor(Point::new(0.0, 250.0)).unwrap().price - a.price).abs() < 1e-27);
        assert_eq!(price_label(0.000000123, 9, "BTC"), "0.000000123 BTC");
        assert_eq!(price_label(15.0, 0, "USDT"), "15.00 USDT");
        assert_eq!(price_label(0.000000123456, 50, "BTC"), "0.000000123456 BTC");
    }

    #[test]
    fn clips_extreme_geometry_before_pixel_projection() {
        let projection = projection();
        let (a, b) = clip_line(
            projection,
            DataPoint {
                x: 1050.0,
                y: -1e100,
            },
            DataPoint {
                x: 1050.0,
                y: 1e100,
            },
            false,
        )
        .unwrap();
        near(a.x, 500.0);
        near(b.x, 500.0);
        near(a.y.min(b.y), 0.0);
        near(a.y.max(b.y), 500.0);
        let (a, b) = clip_line(
            projection,
            DataPoint { x: -1e100, y: 15.0 },
            DataPoint { x: 1e100, y: 15.0 },
            false,
        )
        .unwrap();
        near(a.x, 0.0);
        near(b.x, 1000.0);
        assert!(
            clip_line(
                projection,
                DataPoint {
                    x: 1050.0,
                    y: 1e100
                },
                DataPoint {
                    x: 1060.0,
                    y: 2e100
                },
                false
            )
            .is_none()
        );
    }

    #[test]
    fn reverse_ray_keeps_only_forward_direction_and_enters_from_outside() {
        let projection = projection();
        let (a, b) = clip_line(
            projection,
            anchor(1080, 15.0).into(),
            anchor(1070, 15.0).into(),
            true,
        )
        .unwrap();
        near(a.x, 0.0);
        near(b.x, 800.0);
        let (a, b) = clip_line(
            projection,
            anchor(1200, 15.0).into(),
            anchor(1150, 15.0).into(),
            true,
        )
        .unwrap();
        near(a.x, 0.0);
        near(b.x, 1000.0);
        assert!(
            clip_line(
                projection,
                anchor(1200, 15.0).into(),
                anchor(1250, 15.0).into(),
                true
            )
            .is_none()
        );
    }

    #[test]
    fn rectangle_normalization_and_crossing_edges_are_pickable() {
        let shape = Shape::Rectangle(anchor(1080, 12.0), anchor(1020, 18.0));
        let objects = [drawing(1, shape)];
        let projection = projection();
        assert_eq!(
            hit_test(&objects, Some(1), projection, Point::new(200.0, 100.0)),
            Some(Hit {
                id: 1,
                part: Part::Corner(0)
            })
        );
        assert_eq!(
            hit_test(&objects, Some(1), projection, Point::new(800.0, 400.0)),
            Some(Hit {
                id: 1,
                part: Part::Corner(2)
            })
        );
        assert_eq!(
            hit_test(&objects, None, projection, Point::new(500.0, 250.0))
                .unwrap()
                .part,
            Part::Body
        );
        let crossing = [drawing(
            2,
            Shape::Rectangle(anchor(900, 5.0), anchor(1200, 25.0)),
        )];
        assert!(hit_test(&crossing, None, projection, Point::new(500.0, 250.0)).is_some());
        assert!(hit_test(&crossing, None, projection, Point::new(1010.0, 250.0)).is_none());
    }

    #[test]
    fn topmost_hidden_locked_and_selected_handle_priorities() {
        let projection = projection();
        let mut objects = [
            drawing(1, Shape::Horizontal(anchor(1050, 15.0))),
            drawing(2, Shape::Horizontal(anchor(1050, 15.0))),
        ];
        objects[1].locked = true;
        let point = Point::new(500.0, 250.0);
        assert_eq!(hit_test(&objects, None, projection, point).unwrap().id, 2);
        assert_eq!(
            hit_test(&objects, Some(1), projection, point),
            Some(Hit {
                id: 1,
                part: Part::First
            })
        );
        objects[1].visible = false;
        assert_eq!(
            hit_test(&objects, Some(2), projection, point).unwrap().id,
            1
        );
    }

    #[test]
    fn irrelevant_horizontal_handle_dimension_clamps_but_price_does_not() {
        let projection = projection();
        let objects = [drawing(1, Shape::Horizontal(anchor(900, 15.0)))];
        assert_eq!(
            hit_test(&objects, Some(1), projection, Point::new(0.0, 250.0))
                .unwrap()
                .part,
            Part::First
        );
        let objects = [drawing(1, Shape::Horizontal(anchor(1050, 30.0)))];
        assert!(hit_test(&objects, Some(1), projection, Point::new(500.0, 0.0)).is_none());
    }

    #[test]
    fn fibonacci_both_directions_reverse_extensions_and_level_pick() {
        assert_eq!(fib_price(20.0, 10.0, 0.0, false), Some(10.0));
        assert_eq!(fib_price(20.0, 10.0, 1.0, false), Some(20.0));
        assert_eq!(fib_price(10.0, 20.0, 0.25, false), Some(17.5));
        assert_eq!(fib_price(10.0, 20.0, 0.25, true), Some(12.5));
        assert_eq!(fib_price(20.0, 10.0, 1.5, false), Some(25.0));
        assert!(fib_price(1e308, -1e308, 2.0, false).is_none());
        let objects = [drawing(
            1,
            Shape::Fibonacci {
                a: anchor(1020, 20.0),
                b: anchor(1040, 10.0),
                levels: vec![0.25],
                reverse: false,
                extend_right: true,
            },
        )];
        assert!(hit_test(&objects, None, projection(), Point::new(900.0, 375.0)).is_some());
        assert!(hit_test(&objects, None, projection(), Point::new(900.0, 250.0)).is_none());
        let reversed = [drawing(
            2,
            Shape::Fibonacci {
                a: anchor(1040, 10.0),
                b: anchor(1020, 20.0),
                levels: vec![0.25],
                reverse: true,
                extend_right: true,
            },
        )];
        assert!(hit_test(&reversed, None, projection(), Point::new(900.0, 375.0)).is_some());
        let finite = [drawing(
            3,
            Shape::Fibonacci {
                a: anchor(1020, 20.0),
                b: anchor(1040, 10.0),
                levels: vec![0.25],
                reverse: false,
                extend_right: false,
            },
        )];
        assert!(hit_test(&finite, None, projection(), Point::new(900.0, 375.0)).is_none());
        let future = [drawing(
            4,
            Shape::Fibonacci {
                a: anchor(1200, 20.0),
                b: anchor(1250, 10.0),
                levels: vec![0.25],
                reverse: false,
                extend_right: true,
            },
        )];
        assert!(hit_test(&future, None, projection(), Point::new(1000.0, 375.0)).is_none());
    }

    #[test]
    fn actual_ohlc_magnet_picks_nearest_and_never_fabricates_future_bars() {
        let projection = projection();
        let candles = [
            Candle {
                time: 1050,
                open: 15.0,
                high: 18.0,
                low: 12.0,
                close: 16.0,
                volume: 1.0,
                flow: None,
            },
            Candle {
                time: 1051,
                open: 15.0,
                high: 18.0,
                low: 12.0,
                close: 16.0,
                volume: 1.0,
                flow: None,
            },
        ];
        assert_eq!(
            magnet(projection, Point::new(508.0, 251.0), candles.iter()),
            Some(anchor(1051, 15.0))
        );
        assert_eq!(
            magnet(projection, Point::new(500.0, 201.0), candles.iter()),
            Some(anchor(1050, 16.0))
        );
        assert_eq!(
            magnet(projection, Point::new(500.0, 102.0), candles.iter()),
            Some(anchor(1050, 18.0))
        );
        assert_eq!(
            magnet(projection, Point::new(500.0, 399.0), candles.iter()),
            Some(anchor(1050, 12.0))
        );
        assert!(magnet(projection, Point::new(900.0, 250.0), candles.iter()).is_none());
        assert!(magnet(projection, Point::new(500.0, 275.0), candles.iter()).is_none());
        assert!(magnet(projection, Point::new(-1.0, 250.0), candles.iter()).is_none());
    }

    #[test]
    fn selected_native_text_layout_survives_label_cache_eviction_without_payload_copies() {
        let text = "Pinned annotation λ\n第二行";
        pin_text_layout(text, 14.0);
        let pointer = with_text_layout(text, 14.0, |layout| layout as *const TextLayout);
        for number in 0..80 {
            with_text_layout(&format!("Label {number}"), 11.0, |_| ());
        }
        assert_eq!(
            with_text_layout(text, 14.0, |layout| layout as *const TextLayout),
            pointer
        );
        assert!(with_text_layout(
            text,
            14.0,
            |layout| retained_text_matches(&layout.buffer, text)
        ));
    }

    #[test]
    fn clipped_diagonal_segments_and_reverse_sloped_rays_keep_screen_distance_picking() {
        let projection = projection();
        let objects = [drawing(
            1,
            Shape::Trend(anchor(950, 5.0), anchor(1150, 25.0)),
        )];
        assert!(hit_test(&objects, None, projection, Point::new(500.0, 250.0)).is_some());
        assert!(hit_test(&objects, None, projection, Point::new(500.0, 270.0)).is_none());
        let reverse = [drawing(
            2,
            Shape::Ray(anchor(1080, 18.0), anchor(1070, 17.0)),
        )];
        assert!(hit_test(&reverse, None, projection, Point::new(500.0, 250.0)).is_some());
        assert!(hit_test(&reverse, None, projection, Point::new(900.0, 50.0)).is_none());
    }

    #[test]
    fn distant_uncached_text_is_culled_without_native_layout_work() {
        let text = "Distant cache-miss annotation Ω";
        assert!(cached_text_bounds(text, 14.0).is_none());
        let objects = [drawing(
            1,
            Shape::Text {
                at: anchor(100_000, 15.0),
                text: text.into(),
            },
        )];
        assert!(hit_test(&objects, None, projection(), Point::new(500.0, 250.0)).is_none());
        assert!(cached_text_bounds(text, 14.0).is_none());
    }

    #[test]
    fn native_text_bounds_are_multiline_and_used_for_picking() {
        let size = with_text_layout("Wide annotation\nsecond line", 14.0, |layout| {
            layout.bounds.size()
        });
        assert!(size.width > 30.0);
        assert!(size.height > 28.0);
        let projection = projection();
        let objects = [drawing(
            1,
            Shape::Text {
                at: anchor(1030, 16.0),
                text: "Wide annotation\nsecond line".into(),
            },
        )];
        assert!(hit_test(&objects, None, projection, Point::new(310.0, 215.0)).is_some());
        assert!(
            hit_test(
                &objects,
                None,
                projection,
                Point::new(310.0 + size.width + 15.0, 215.0)
            )
            .is_none()
        );
    }
}
