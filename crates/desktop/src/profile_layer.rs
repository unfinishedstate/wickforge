use crate::{drawing_geometry::Projection, palette::Palette};
use iced::widget::canvas::{Cache, Frame, Geometry, Path, Stroke, Text};
use iced::{Color, Font, Point, Rectangle, Renderer, Size};
use mexc_indicators::profiles::{Instance, Kind, Range, Snapshot, Touch, Zone};
use std::{cell::Cell, collections::BTreeMap};

#[derive(Debug, Clone, Copy)]
pub enum Edit {
    Select(u64),
    Range { id: u64, start_ms: i64, end_ms: i64 },
}

#[derive(Debug, Clone, Copy)]
pub enum RangePart {
    Start,
    End,
    Body,
}

#[derive(Debug, Clone, Copy)]
pub struct RangeDrag {
    pub id: u64,
    pub part: RangePart,
    pub origin_ms: i64,
    pub quantum_ms: i64,
    pub start_ms: i64,
    pub end_ms: i64,
    pub preview_start: i64,
    pub preview_end: i64,
}

impl RangeDrag {
    pub fn update(&mut self, time_ms: i64) {
        let time_ms = time_ms
            .div_euclid(self.quantum_ms)
            .saturating_mul(self.quantum_ms);
        match self.part {
            RangePart::Start => {
                self.preview_start = time_ms.min(self.end_ms.saturating_sub(self.quantum_ms))
            }
            RangePart::End => {
                self.preview_end = time_ms.max(self.start_ms.saturating_add(self.quantum_ms))
            }
            RangePart::Body => {
                let delta = time_ms.saturating_sub(self.origin_ms);
                self.preview_start = self.start_ms.saturating_add(delta);
                self.preview_end = self.end_ms.saturating_add(delta);
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct RangePlacement {
    pub first: Option<i64>,
    pub cursor_ms: Option<i64>,
}

#[derive(Clone, Copy, PartialEq)]
struct Key {
    revision: u64,
    projection: Projection,
    palette: Palette,
}

#[derive(Default)]
pub struct Scene {
    cache: Cache,
    key: Cell<Option<Key>>,
}

pub struct View<'a> {
    pub instances: &'a [Instance],
    pub outputs: &'a BTreeMap<u64, Vec<Snapshot>>,
    pub scene: &'a Scene,
    pub revision: u64,
    pub selected: Option<u64>,
    pub market: &'a mexc_model::Market,
}

impl Scene {
    pub fn clear(&self) {
        self.key.set(None);
        self.cache.clear();
    }
}

impl View<'_> {
    pub fn draw(
        &self,
        renderer: &Renderer,
        size: Size,
        projection: Projection,
        palette: Palette,
    ) -> Geometry {
        let key = Key {
            revision: self.revision,
            projection,
            palette,
        };
        if self.scene.key.get() != Some(key) {
            self.scene.key.set(Some(key));
            self.scene.cache.clear();
        }
        self.scene.cache.draw(renderer, size, |frame| {
            frame.with_clip(projection.area, |frame| {
                let mut labels = Labels::default();
                for instance in self.instances.iter().filter(|instance| instance.visible) {
                    let Some(snapshots) = self.outputs.get(&instance.id) else {
                        continue;
                    };
                    for (index, snapshot) in snapshots.iter().enumerate() {
                        if instance.spec.display.naked
                            && matches!(instance.spec.range, Range::Session(_))
                        {
                            let end = if index + 1 == snapshots.len() {
                                snapshot
                                    .end_ms
                                    .max(((projection.left + projection.span) * 1000.0) as i64)
                            } else {
                                snapshot.end_ms
                            };
                            if let Some((left, right)) =
                                time_span(projection, snapshot.start_ms, end)
                            {
                                draw_naked(
                                    frame,
                                    instance,
                                    snapshot,
                                    projection,
                                    palette,
                                    left,
                                    right,
                                    &mut labels,
                                );
                            }
                        }
                        let Some((left, right)) = profile_span(
                            instance.spec.kind,
                            instance.spec.range,
                            snapshot.start_ms,
                            snapshot.end_ms,
                            projection,
                        ) else {
                            if snapshot.complete
                                && instance.spec.display.single_prints
                                && instance.spec.kind == Kind::Tpo
                            {
                                zones(
                                    frame,
                                    &snapshot.single_prints,
                                    "SP",
                                    instance,
                                    snapshot,
                                    projection,
                                    palette,
                                    projection.area.width,
                                    &mut labels,
                                );
                            }
                            if index + 1 == snapshots.len()
                                && snapshot.complete
                                && instance.spec.display.levels
                                && matches!(instance.spec.range, Range::Session(_))
                            {
                                for (name, price) in [
                                    ("closed POC", snapshot.poc),
                                    ("closed VAH", snapshot.vah),
                                    ("closed VAL", snapshot.val),
                                ] {
                                    if let Some(price) = price {
                                        level(
                                            frame,
                                            projection,
                                            palette.muted,
                                            0.0,
                                            projection.area.width,
                                            0.0,
                                            price,
                                            &format!("#{} {name}", instance.id),
                                            true,
                                            &mut labels,
                                        );
                                    }
                                }
                            }
                            continue;
                        };
                        draw_snapshot(
                            frame,
                            instance,
                            snapshot,
                            projection,
                            palette,
                            left,
                            right,
                            &mut labels,
                        );
                        if instance.spec.display.previous
                            && matches!(instance.spec.range, Range::Session(_))
                            && let Some(previous) = index
                                .checked_sub(1)
                                .and_then(|index| snapshots.get(index))
                                .filter(|previous| previous.complete)
                        {
                            let label_left = annotation_left(instance, left, right);
                            for (name, value) in [
                                ("pPOC", previous.poc),
                                ("pVAH", previous.vah),
                                ("pVAL", previous.val),
                            ] {
                                if let Some(price) = value {
                                    level(
                                        frame,
                                        projection,
                                        palette.muted,
                                        left,
                                        right,
                                        label_left,
                                        price,
                                        &format!("#{0} {name}", instance.id),
                                        true,
                                        &mut labels,
                                    );
                                }
                            }
                        }
                    }
                }
                labels.draw(frame);
            });
        })
    }

    fn editable(&self, instance: &Instance) -> bool {
        instance.visible
            && instance.instrument.as_ref().is_some_and(|native| {
                native.exchange == self.market.exchange
                    && native.product == self.market.product
                    && native.symbol == self.market.symbol
            })
            && matches!(instance.spec.range, Range::Fixed { .. })
    }

    pub fn hit_range(&self, projection: Projection, point: Point) -> Option<RangeDrag> {
        // Only the explicit range rail/handles participate: histogram bodies stay passive.
        self.instances
            .iter()
            .rev()
            .filter(|instance| self.editable(instance))
            .find_map(|instance| {
                let Range::Fixed { start_ms, end_ms } = instance.spec.range else {
                    return None;
                };
                let left = x(projection, start_ms);
                let right = x(projection, end_ms);
                let y = 8.0 + (instance.id % 5) as f32 * 20.0;
                let selected = self.selected == Some(instance.id);
                let part = if selected && (point.x - left).abs() <= 6.0 {
                    RangePart::Start
                } else if selected && (point.x - right).abs() <= 6.0 {
                    RangePart::End
                } else if point.y >= y
                    && point.y <= y + 18.0
                    && point.x >= left.max(0.0)
                    && point.x <= right.min(projection.area.width)
                {
                    RangePart::Body
                } else {
                    return None;
                };
                let quantum_ms = if instance.spec.kind == Kind::Tpo {
                    60_000
                } else {
                    1
                };
                let origin_ms = projection
                    .anchor(point)?
                    .utc_ms
                    .div_euclid(quantum_ms)
                    .saturating_mul(quantum_ms);
                Some(RangeDrag {
                    id: instance.id,
                    part,
                    origin_ms,
                    quantum_ms,
                    start_ms,
                    end_ms,
                    preview_start: start_ms,
                    preview_end: end_ms,
                })
            })
    }

    pub fn draw_ranges(
        &self,
        frame: &mut Frame,
        projection: Projection,
        palette: Palette,
        drag: Option<RangeDrag>,
    ) {
        frame.with_clip(projection.area, |frame| {
            for instance in self
                .instances
                .iter()
                .filter(|instance| self.editable(instance))
            {
                let Range::Fixed {
                    mut start_ms,
                    mut end_ms,
                } = instance.spec.range
                else {
                    continue;
                };
                if let Some(drag) = drag.filter(|drag| drag.id == instance.id) {
                    start_ms = drag.preview_start;
                    end_ms = drag.preview_end;
                }
                let left = x(projection, start_ms);
                let right = x(projection, end_ms);
                if right <= 0.0 || left >= projection.area.width {
                    continue;
                }
                let y = 8.0 + (instance.id % 5) as f32 * 20.0;
                let selected = self.selected == Some(instance.id);
                let color = if selected {
                    palette.accent
                } else {
                    palette.muted
                };
                frame.fill_rectangle(
                    Point::new(left.max(0.0), y),
                    Size::new(
                        (right.min(projection.area.width) - left.max(0.0)).max(0.0),
                        18.0,
                    ),
                    Color { a: 0.18, ..color },
                );
                frame.fill_text(Text {
                    content: format!("{} #{} · drag range", instance.spec.kind, instance.id),
                    position: Point::new(left.max(0.0) + 4.0, y + 2.0),
                    color,
                    size: 11.0.into(),
                    ..Text::default()
                });
                if selected {
                    for edge in [left, right] {
                        frame.stroke(
                            &Path::line(
                                Point::new(edge, 0.0),
                                Point::new(edge, projection.area.height),
                            ),
                            Stroke::default().with_color(color),
                        );
                        frame.fill_rectangle(
                            Point::new(edge - 4.0, y),
                            Size::new(8.0, 18.0),
                            color,
                        );
                    }
                }
            }
        });
    }
}

fn profile_span(
    kind: Kind,
    range: Range,
    start: i64,
    end: i64,
    projection: Projection,
) -> Option<(f32, f32)> {
    if kind == Kind::Volume && !matches!(range, Range::Session(_)) {
        Some((0.0, projection.area.width))
    } else {
        time_span(projection, start, end)
    }
}

fn alpha(color: Color, a: f32) -> Color {
    Color { a, ..color }
}
fn x(projection: Projection, time: i64) -> f32 {
    (((time as f64 / 1000.0 - projection.left) / projection.span)
        * f64::from(projection.area.width)) as f32
}
fn y(projection: Projection, price: f64) -> f32 {
    (((projection.high - price) / (projection.high - projection.low))
        * f64::from(projection.area.height)) as f32
}
fn time_span(projection: Projection, start: i64, end: i64) -> Option<(f32, f32)> {
    let left = x(projection, start).max(0.0);
    let right = x(projection, end).min(projection.area.width);
    (left.is_finite() && right.is_finite() && right > left).then_some((left, right))
}
fn price_span(projection: Projection, low: f64, high: f64) -> Option<(f32, f32)> {
    if !low.is_finite() || !high.is_finite() || high < projection.low || low > projection.high {
        return None;
    }
    let top = y(projection, high.min(projection.high)).max(0.0);
    let bottom = y(projection, low.max(projection.low)).min(projection.area.height);
    (bottom >= top).then_some((top, bottom))
}

struct Label {
    rect: Rectangle,
    content: String,
    color: Color,
    background: Color,
}

#[derive(Default)]
struct Labels {
    placed: Vec<Label>,
}
impl Labels {
    fn place(
        &mut self,
        content: String,
        position: Point,
        available: f32,
        color: Color,
        background: Color,
    ) {
        if available < 28.0 || position.y < 0.0 {
            return;
        }
        let max_chars = ((available - 6.0) / 6.6).floor() as usize;
        let content = if content.chars().count() > max_chars {
            let mut shortened: String = content.chars().take(max_chars.saturating_sub(1)).collect();
            shortened.push('…');
            shortened
        } else {
            content
        };
        let width = (content.chars().count() as f32 * 6.6 + 6.0).min(available);
        let rect = Rectangle {
            x: position.x,
            y: position.y,
            width,
            height: 15.0,
        };
        if self.placed.iter().any(|other| other.rect.intersects(&rect)) {
            return;
        }
        self.placed.push(Label {
            rect,
            content,
            color,
            background,
        });
    }

    fn draw(self, frame: &mut Frame) {
        // Paint annotations after every profile fill, without cloning their text.
        for Label {
            rect,
            content,
            color,
            background,
        } in self.placed
        {
            if background.a > 0.0 {
                frame.fill_rectangle(
                    rect.position(),
                    rect.size(),
                    alpha(background, background.a * 0.9),
                );
            }
            frame.fill_text(Text {
                content,
                position: Point::new(rect.x + 3.0, rect.y),
                color,
                size: 11.0.into(),
                font: Font::MONOSPACE,
                ..Text::default()
            });
        }
    }
}

fn annotation_left(instance: &Instance, left: f32, right: f32) -> f32 {
    if instance.spec.kind == Kind::Tpo
        && (instance.spec.display.histogram || instance.spec.display.letters)
    {
        // Keep reference text out of the reserved histogram/letter column.
        (left + (right - left) * f32::from(instance.spec.display.width_percent) / 100.0 + 4.0)
            .min(right)
    } else {
        left
    }
}

#[allow(clippy::too_many_arguments)]
fn level(
    frame: &mut Frame,
    projection: Projection,
    color: Color,
    left: f32,
    right: f32,
    label_left: f32,
    price: f64,
    name: &str,
    dashed: bool,
    labels: &mut Labels,
) {
    if !price.is_finite() || !(projection.low..=projection.high).contains(&price) || right <= left {
        return;
    }
    let ordinate = y(projection, price);
    let path = Path::line(Point::new(left, ordinate), Point::new(right, ordinate));
    let mut stroke = Stroke::default()
        .with_color(alpha(color, 0.75))
        .with_width(1.0);
    if dashed {
        stroke.line_dash.segments = &[4.0, 4.0];
    }
    frame.stroke(&path, stroke);
    if ordinate >= 8.0 && ordinate + 8.0 < projection.area.height {
        labels.place(
            name.to_owned(),
            Point::new(label_left + 2.0, ordinate - 7.0),
            right - label_left - 4.0,
            color,
            Color::TRANSPARENT,
        );
    }
}

fn touch_label(touch: Touch) -> &'static str {
    match touch {
        Touch::Untouched => "open",
        Touch::Partial => "partial",
        Touch::Filled => "filled",
        Touch::Unknown => "unproved",
    }
}

#[allow(clippy::too_many_arguments)]
fn zones(
    frame: &mut Frame,
    zones: &[Zone],
    name: &str,
    instance: &Instance,
    snapshot: &Snapshot,
    projection: Projection,
    palette: Palette,
    right: f32,
    labels: &mut Labels,
) {
    let label_left = time_span(projection, snapshot.start_ms, snapshot.end_ms)
        .map_or(0.0, |(left, right)| annotation_left(instance, left, right));
    for zone in zones {
        let start = zone.block.map_or(snapshot.start_ms, |block| {
            snapshot.start_ms.saturating_add(
                i64::from(block)
                    .saturating_mul(i64::from(instance.spec.block_minutes))
                    .saturating_mul(60_000),
            )
        });
        let left = x(projection, start).clamp(0.0, projection.area.width);
        let right = if zone.touch == Touch::Filled {
            right.min(x(projection, snapshot.end_ms).clamp(0.0, projection.area.width))
        } else {
            right
        };
        let Some((top, bottom)) = price_span(projection, zone.low, zone.high) else {
            continue;
        };
        if right <= left {
            continue;
        }
        let opacity = match zone.touch {
            Touch::Untouched => 0.15,
            Touch::Partial => 0.09,
            Touch::Filled => 0.025,
            Touch::Unknown => 0.055,
        };
        frame.fill_rectangle(
            Point::new(left, top),
            Size::new(right - left, (bottom - top).max(1.0)),
            alpha(palette.accent, opacity),
        );
        // Boundaries and explicit state distinguish mitigation even with monochrome themes.
        let color = if zone.touch == Touch::Untouched {
            palette.accent
        } else {
            palette.muted
        };
        level(
            frame,
            projection,
            color,
            left,
            right,
            label_left.max(left),
            zone.high,
            &format!("#{} {name} {}", instance.id, touch_label(zone.touch)),
            zone.touch != Touch::Untouched,
            labels,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_snapshot(
    frame: &mut Frame,
    instance: &Instance,
    snapshot: &Snapshot,
    projection: Projection,
    palette: Palette,
    left: f32,
    right: f32,
    labels: &mut Labels,
) {
    let display = &instance.spec.display;
    let label_left = annotation_left(instance, left, right);
    if display.levels {
        for (name, price) in [
            ("POC", snapshot.poc),
            ("VAH", snapshot.vah),
            ("VAL", snapshot.val),
        ] {
            if let Some(price) = price {
                level(
                    frame,
                    projection,
                    palette.accent,
                    left,
                    right,
                    label_left,
                    price,
                    &format!("#{} {name}", instance.id),
                    name != "POC",
                    labels,
                );
            }
        }
    }
    if display.histogram || (display.letters && instance.spec.kind == Kind::Tpo) {
        let maximum = snapshot
            .rows
            .iter()
            .map(|row| row.weight)
            .fold(0.0_f64, f64::max);
        let width = (right - left) * f32::from(display.width_percent) / 100.0;
        if maximum > 0.0 && maximum.is_finite() {
            for row in &snapshot.rows {
                let Some((top, bottom)) = price_span(projection, row.low, row.high) else {
                    continue;
                };
                let height = (bottom - top).max(0.6);
                let row_width = (row.weight / maximum).clamp(0.0, 1.0) as f32 * width;
                let in_value = row_in_value(row.low, row.high, snapshot.val, snapshot.vah);
                let color = if in_value {
                    palette.accent
                } else {
                    palette.muted
                };
                if display.histogram {
                    frame.fill_rectangle(
                        Point::new(
                            if instance.spec.kind == Kind::Volume {
                                right - row_width
                            } else {
                                left
                            },
                            top,
                        ),
                        Size::new(row_width, height),
                        alpha(color, if in_value { 0.29 } else { 0.17 }),
                    );
                }
                if instance.spec.kind == Kind::Tpo && (display.histogram || display.letters) {
                    let cycle = row.blocks.last().copied().unwrap_or(0) / 52 + 1;
                    let characters = if cycle > 1 { cycle.ilog10() + 2 } else { 1 };
                    let cell = if display.letters {
                        characters as f32 * 6.1 + 2.0
                    } else {
                        4.0
                    };
                    let cells = (width / cell).floor() as usize;
                    if height >= (if display.letters { 11.0 } else { 3.0 }) && cells > 0 {
                        let count = if display.letters && row.blocks.len() > cells {
                            cells.saturating_sub(1)
                        } else {
                            cells
                        };
                        for (column, block) in row.blocks.iter().take(count).enumerate() {
                            let position = Point::new(left + column as f32 * cell, top);
                            if display.letters {
                                frame.fill_text(Text {
                                    content: block_letter(*block),
                                    position,
                                    color: palette.foreground,
                                    size: 10.0.into(),
                                    font: Font::MONOSPACE,
                                    ..Text::default()
                                });
                            } else {
                                frame.stroke(
                                    &Path::rectangle(
                                        position,
                                        Size::new(cell - 0.8, height.min(14.0)),
                                    ),
                                    Stroke::default()
                                        .with_color(alpha(color, 0.5))
                                        .with_width(0.6),
                                );
                            }
                        }
                        if display.letters && row.blocks.len() > cells {
                            frame.fill_text(Text {
                                content: "+".into(),
                                position: Point::new(
                                    left + cells.saturating_sub(1) as f32 * cell,
                                    top,
                                ),
                                color: palette.foreground,
                                size: 10.0.into(),
                                ..Text::default()
                            });
                        }
                    }
                }
            }
        }
    }
    if display.single_prints && instance.spec.kind == Kind::Tpo {
        zones(
            frame,
            &snapshot.single_prints,
            "SP",
            instance,
            snapshot,
            projection,
            palette,
            if snapshot.complete {
                projection.area.width
            } else {
                right
            },
            labels,
        );
        zones(
            frame,
            &snapshot.tails,
            "tail",
            instance,
            snapshot,
            projection,
            palette,
            right,
            labels,
        );
    }
    if display.initial_balance
        && instance.spec.kind == Kind::Tpo
        && let Some((low, high)) = snapshot.initial_balance
    {
        for (name, price) in [("IBH", high), ("IBL", low)] {
            level(
                frame,
                projection,
                palette.muted,
                left,
                right,
                label_left,
                price,
                &format!("#{} {name}", instance.id),
                true,
                labels,
            );
        }
    }
    if display.poor_extremes && instance.spec.kind == Kind::Tpo {
        for (poor, name, price) in [
            (snapshot.poor_high, "poor high", snapshot.high),
            (snapshot.poor_low, "poor low", snapshot.low),
        ] {
            if poor && let Some(price) = price {
                level(
                    frame,
                    projection,
                    palette.foreground,
                    left,
                    right,
                    label_left,
                    price,
                    &format!("#{} {name}", instance.id),
                    true,
                    labels,
                );
            }
        }
    }
}

fn row_in_value(low: f64, high: f64, val: Option<f64>, vah: Option<f64>) -> bool {
    val.zip(vah)
        .is_some_and(|(val, vah)| high >= val && low <= vah)
}

#[allow(clippy::too_many_arguments)]
fn draw_naked(
    frame: &mut Frame,
    instance: &Instance,
    snapshot: &Snapshot,
    projection: Projection,
    palette: Palette,
    left: f32,
    right: f32,
    labels: &mut Labels,
) {
    let display = &instance.spec.display;
    let label_left = if instance.spec.kind == Kind::Tpo {
        time_span(projection, snapshot.start_ms, snapshot.end_ms)
            .map_or(left, |(left, right)| annotation_left(instance, left, right))
    } else {
        left
    };
    if display.naked && matches!(instance.spec.range, Range::Session(_)) {
        for poc in &snapshot.naked_pocs {
            let origin = x(projection, poc.origin_ms)
                .clamp(0.0, projection.area.width)
                .max(left);
            let color = if poc.touch == Touch::Untouched {
                palette.accent
            } else {
                palette.muted
            };
            level(
                frame,
                projection,
                color,
                origin,
                right,
                label_left.max(origin),
                poc.price,
                &format!("#{} nPOC {}", instance.id, touch_label(poc.touch)),
                true,
                labels,
            );
        }
    }
}

fn block_letter(block: u32) -> String {
    const LETTERS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let index = block as usize % LETTERS.len();
    let cycle = block as usize / LETTERS.len();
    if cycle == 0 {
        char::from(LETTERS[index]).to_string()
    } else {
        format!("{}{}", char::from(LETTERS[index]), cycle + 1)
    }
}

pub fn draw_range(
    frame: &mut Frame,
    placement: RangePlacement,
    projection: Projection,
    cursor: Option<Point>,
    palette: Palette,
) {
    frame.with_clip(projection.area, |frame| {
        let candidate = placement
            .cursor_ms
            .map(|time| x(projection, time))
            .or_else(|| {
                cursor
                    .filter(|point| projection.contains(*point))
                    .map(|point| point.x)
            })
            .map(|x| x.clamp(0.0, projection.area.width));
        if let Some(candidate) = candidate {
            frame.stroke(
                &Path::line(
                    Point::new(candidate, 0.0),
                    Point::new(candidate, projection.area.height),
                ),
                Stroke::default().with_color(palette.accent),
            );
        }
        if let Some(first) = placement.first {
            let first_x = x(projection, first).clamp(0.0, projection.area.width);
            if let Some(candidate) = candidate {
                frame.fill_rectangle(
                    Point::new(first_x.min(candidate), 0.0),
                    Size::new((first_x - candidate).abs(), projection.area.height),
                    alpha(palette.accent, 0.09),
                );
            }
            frame.stroke(
                &Path::line(
                    Point::new(first_x, 0.0),
                    Point::new(first_x, projection.area.height),
                ),
                Stroke::default().with_color(palette.accent),
            );
        }
        let content = if placement.first.is_some() {
            "Profile range: choose end · Esc cancel"
        } else {
            "Profile range: choose start · Esc cancel"
        };
        let mut labels = Labels::default();
        labels.place(
            content.into(),
            Point::new(8.0, (projection.area.height - 22.0).max(0.0)),
            projection.area.width - 16.0,
            palette.foreground,
            palette.background,
        );
        labels.draw(frame);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn range_drag_preserves_duration_and_clamps_endpoints_without_crossing() {
        let mut drag = RangeDrag {
            id: 7,
            part: RangePart::Body,
            origin_ms: 120_000,
            quantum_ms: 60_000,
            start_ms: 60_000,
            end_ms: 180_000,
            preview_start: 60_000,
            preview_end: 180_000,
        };
        drag.update(241_111);
        assert_eq!((drag.preview_start, drag.preview_end), (180_000, 300_000));
        drag.part = RangePart::Start;
        drag.update(999_999);
        assert_eq!(drag.preview_start, 120_000);
        drag.part = RangePart::End;
        drag.update(-1);
        assert_eq!(drag.preview_end, 120_000);
    }

    #[test]
    fn range_hit_targets_are_explicit_and_native_bound() {
        let market = mexc_model::Market::default();
        let mut spec = mexc_indicators::profiles::Spec::default_for(Kind::Volume);
        spec.range = Range::Fixed {
            start_ms: 60_000,
            end_ms: 180_000,
        };
        let mut instances = [Instance {
            id: 7,
            spec,
            visible: true,
            instrument: Some(mexc_model::executions::NativeInstrument::from_market(
                &market,
            )),
        }];
        let outputs = BTreeMap::new();
        let scene = Scene::default();
        let projection = Projection {
            area: Rectangle::with_size(Size::new(800.0, 600.0)),
            left: 0.0,
            span: 240.0,
            low: 100.0,
            high: 200.0,
        };
        let view = View {
            instances: &instances,
            outputs: &outputs,
            scene: &scene,
            revision: 0,
            selected: None,
            market: &market,
        };
        assert!(
            view.hit_range(projection, Point::new(400.0, 300.0))
                .is_none()
        );
        assert!(matches!(
            view.hit_range(projection, Point::new(400.0, 50.0)),
            Some(RangeDrag {
                id: 7,
                part: RangePart::Body,
                ..
            })
        ));
        let view = View {
            selected: Some(7),
            ..view
        };
        assert!(matches!(
            view.hit_range(projection, Point::new(200.0, 300.0)),
            Some(RangeDrag {
                part: RangePart::Start,
                ..
            })
        ));
        instances[0]
            .instrument
            .as_mut()
            .unwrap()
            .symbol
            .push_str("-other");
        let view = View {
            instances: &instances,
            outputs: &outputs,
            scene: &scene,
            revision: 0,
            selected: Some(7),
            market: &market,
        };
        assert!(
            view.hit_range(projection, Point::new(400.0, 50.0))
                .is_none()
        );
    }

    #[test]
    fn block_letters_remain_unambiguous_after_first_cycle() {
        assert_eq!(block_letter(0), "A");
        assert_eq!(block_letter(25), "Z");
        assert_eq!(block_letter(26), "a");
        assert_eq!(block_letter(51), "z");
        assert_eq!(block_letter(52), "A2");
    }

    #[test]
    fn value_area_includes_single_tick_rows_and_both_boundary_rows() {
        assert!(row_in_value(100.0, 100.0, Some(100.0), Some(100.0)));
        assert!(row_in_value(100.0, 109.0, Some(100.0), Some(129.0)));
        assert!(row_in_value(120.0, 129.0, Some(100.0), Some(129.0)));
        assert!(!row_in_value(90.0, 99.0, Some(100.0), Some(129.0)));
        assert!(!row_in_value(100.0, 109.0, None, None));
    }

    #[test]
    fn fixed_volume_remains_price_anchored_when_formation_time_is_offscreen() {
        let projection = Projection {
            area: Rectangle::with_size(Size::new(800.0, 600.0)),
            left: 3_600.0,
            span: 3_600.0,
            low: 100.0,
            high: 200.0,
        };
        let range = Range::Fixed {
            start_ms: 0,
            end_ms: 60_000,
        };
        assert_eq!(
            profile_span(Kind::Volume, range, 0, 60_000, projection),
            Some((0.0, 800.0))
        );
        assert_eq!(profile_span(Kind::Tpo, range, 0, 60_000, projection), None);
        assert_eq!(
            profile_span(
                Kind::Volume,
                Range::Session(mexc_indicators::profiles::Period::Day),
                0,
                60_000,
                projection
            ),
            None
        );
    }
}
