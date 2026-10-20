use crate::{
    drawing_geometry::Projection,
    drawing_types::{Anchor, Anchors, Command, DrawingId, Hit, Part, Preview, Style, Tool},
    drawings::{Store, create_shape},
};
use iced::{
    Point, keyboard,
    keyboard::{Key, Modifiers},
};

#[derive(Clone, Debug)]
pub enum Input {
    Down {
        point: Point,
        projection: Projection,
        hit: Option<Hit>,
        snap: Option<Anchor>,
    },
    Move {
        point: Point,
        projection: Projection,
        hit: Option<Hit>,
        snap: Option<Anchor>,
    },
    Up {
        point: Point,
        projection: Projection,
        snap: Option<Anchor>,
    },
    Leave,
    Key {
        key: keyboard::Key,
        modifiers: Modifiers,
        seed: Option<Anchor>,
        step_ms: i64,
        price_step: f64,
    },
}

#[derive(Clone, Debug)]
pub enum Effect {
    None,
    Select(Option<DrawingId>),
    Commit(Command),
    Text(Anchor),
}

#[derive(Clone, Copy, Debug)]
struct Draft {
    tool: Tool,
    first: Anchor,
    floating: Anchor,
    pointer: Option<Point>,
    projection: Option<Projection>,
    keyboard: bool,
    first_fixed: bool,
}
#[derive(Clone, Copy, Debug)]
enum EditPart {
    Body,
    First,
    Second,
    Corner(u8),
}
#[derive(Clone, Copy, Debug)]
struct Drag {
    id: DrawingId,
    tool: Tool,
    part: EditPart,
    original: Anchors,
    start: Anchor,
    anchors: Anchors,
    moved: bool,
}

#[derive(Debug)]
pub struct Interaction {
    generation: u64,
    tool: Tool,
    repeat: bool,
    magnet: bool,
    hover: Option<Hit>,
    draft: Option<Draft>,
    drag: Option<Drag>,
    cursor: Option<Anchor>,
}
impl Default for Interaction {
    fn default() -> Self {
        Self {
            generation: 0,
            tool: Tool::Select,
            repeat: false,
            magnet: false,
            hover: None,
            draft: None,
            drag: None,
            cursor: None,
        }
    }
}

impl Interaction {
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn tool(&self) -> Tool {
        self.tool
    }
    pub fn repeat(&self) -> bool {
        self.repeat
    }
    pub fn set_repeat(&mut self, repeat: bool) {
        self.repeat = repeat;
        if !repeat && self.tool != Tool::Select && self.draft.is_none() {
            self.tool = Tool::Select;
            self.cursor = None;
            self.invalidate();
        }
    }
    pub fn suspend_for_text(&mut self) {
        self.draft = None;
    }
    pub fn suspend_overlay(&mut self) {
        self.invalidate();
        self.drag = None;
        self.hover = None;
    }
    pub fn finish_text(&mut self, applied: bool) {
        if !self.repeat {
            self.cancel();
        } else {
            self.draft = None;
            self.cursor = None;
            if !applied {
                self.hover = None;
            }
        }
    }
    pub fn magnet(&self) -> bool {
        self.magnet
    }
    pub fn set_magnet(&mut self, magnet: bool) {
        self.magnet = magnet;
    }
    pub fn choose_tool(&mut self, tool: Tool) {
        self.invalidate();
        self.tool = tool;
        self.draft = None;
        self.drag = None;
        self.cursor = None;
    }
    pub fn cancel(&mut self) -> bool {
        let changed =
            self.tool != Tool::Select || self.repeat || self.draft.is_some() || self.drag.is_some();
        self.invalidate();
        self.tool = Tool::Select;
        self.repeat = false;
        self.draft = None;
        self.drag = None;
        self.cursor = None;
        self.hover = None;
        changed
    }
    pub fn hover(&self) -> Option<Hit> {
        self.hover
    }
    pub fn dragging_id(&self) -> Option<DrawingId> {
        self.drag.map(|drag| drag.id)
    }
    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }
    pub fn vwap_preview_start(&self) -> Option<(DrawingId, i64)> {
        self.drag
            .filter(|drag| drag.tool == Tool::AnchoredVwap)
            .map(|drag| (drag.id, drag.anchors.first().utc_ms))
    }
    pub fn captures_pointer(&self) -> bool {
        self.draft.is_some() || self.drag.is_some()
    }

    pub fn accepts_key(key: &Key<&str>, modifiers: Modifiers) -> bool {
        if matches!(key, Key::Named(keyboard::key::Named::Escape)) {
            return true;
        }
        if modifiers.control() || modifiers.alt() {
            return false;
        }
        matches!(
            key,
            Key::Named(
                keyboard::key::Named::ArrowLeft
                    | keyboard::key::Named::ArrowRight
                    | keyboard::key::Named::ArrowUp
                    | keyboard::key::Named::ArrowDown
                    | keyboard::key::Named::Enter
                    | keyboard::key::Named::Escape
            )
        )
    }

    /// Native time navigation skips missing candles rather than inventing anchor bars.
    pub fn update_native(
        &mut self,
        mut input: Input,
        store: &Store,
        candles: &[mexc_model::Candle],
    ) -> Effect {
        let anchored = self.tool == Tool::AnchoredVwap
            || (self.tool == Tool::Select
                && store
                    .selected_drawing()
                    .is_some_and(|drawing| drawing.shape.tool() == Tool::AnchoredVwap));
        if anchored
            && let Input::Key {
                key: Key::Named(named),
                modifiers,
                seed,
                step_ms,
                ..
            } = &mut input
            && !modifiers.control()
            && !modifiers.alt()
        {
            use keyboard::key::Named;
            if matches!(named, Named::ArrowUp | Named::ArrowDown) {
                return Effect::None;
            }
            if matches!(named, Named::ArrowLeft | Named::ArrowRight) {
                let current = self
                    .draft
                    .map(|draft| draft.floating)
                    .or_else(|| {
                        store
                            .selected_drawing()
                            .filter(|_| self.tool == Tool::Select)
                            .map(|drawing| drawing.shape.anchors().first())
                    })
                    .or(*seed);
                let Some(current) = current else {
                    return Effect::None;
                };
                let count = if modifiers.shift() { 10 } else { 1 };
                let right = *named == Named::ArrowRight;
                let index = if right {
                    candles
                        .partition_point(|candle| {
                            candle.time.saturating_mul(1000) <= current.utc_ms
                        })
                        .checked_add(count - 1)
                } else {
                    candles
                        .partition_point(|candle| candle.time.saturating_mul(1000) < current.utc_ms)
                        .checked_sub(count)
                };
                let Some(target) = index.and_then(|index| candles.get(index)) else {
                    return Effect::None;
                };
                *step_ms = target
                    .time
                    .saturating_mul(1000)
                    .saturating_sub(current.utc_ms)
                    .saturating_abs();
                *modifiers = Modifiers::default();
            }
        }
        self.update(input, store)
    }

    pub fn update(&mut self, input: Input, store: &Store) -> Effect {
        match input {
            Input::Down {
                point,
                projection,
                hit,
                snap,
            } => {
                self.drag = None;
                self.hover = hit;
                let Some(anchor) = self.pointer_anchor(point, projection, snap) else {
                    return Effect::None;
                };
                self.cursor = Some(anchor);
                if self.tool != Tool::Select {
                    return self.creation_click(anchor);
                }
                let Some(hit) = hit else {
                    return Effect::Select(None);
                };
                let Some(drawing) = store.find(hit.id) else {
                    return Effect::Select(Some(hit.id));
                };
                if !drawing.can_drag(hit.part) {
                    return Effect::Select(Some(hit.id));
                }
                let part = match hit.part {
                    Part::Body => EditPart::Body,
                    Part::First => EditPart::First,
                    Part::Second => EditPart::Second,
                    Part::Corner(n) => EditPart::Corner(n),
                };
                let start = if drawing.shape.tool() == Tool::AnchoredVwap {
                    snap.map(|anchor| Anchor {
                        price: 0.0,
                        ..anchor
                    })
                } else if matches!(part, EditPart::Body) {
                    projection.anchor(point)
                } else {
                    Some(anchor)
                };
                let Some(start) = start else {
                    return Effect::Select(Some(hit.id));
                };
                self.drag = Some(Drag {
                    id: hit.id,
                    tool: drawing.shape.tool(),
                    part,
                    original: drawing.shape.anchors(),
                    anchors: drawing.shape.anchors(),
                    start,
                    moved: false,
                });
                Effect::Select(Some(hit.id))
            }
            Input::Move {
                point,
                projection,
                hit,
                snap,
            } => {
                if let Some(drag) = self.drag {
                    let anchor = self
                        .pointer_anchor(point, projection, snap)
                        .unwrap_or(drag.start);
                    self.drag = Some(Self::transformed(drag, anchor));
                } else if let Some(mut draft) = self.draft {
                    if !draft.keyboard {
                        draft.floating = self
                            .pointer_anchor(point, projection, snap)
                            .unwrap_or(draft.floating);
                        draft.pointer = Some(point);
                        draft.projection = Some(projection);
                        self.draft = Some(draft);
                    }
                } else {
                    self.hover = hit;
                }
                Effect::None
            }
            Input::Up {
                point,
                projection,
                snap,
            } => {
                let Some(mut drag) = self.drag.take() else {
                    return Effect::None;
                };
                // A relayout can change canvas-local coordinates on release without
                // any cursor movement. Selection alone must never edit geometry.
                if !drag.moved {
                    return Effect::None;
                }
                let anchor = if drag.tool == Tool::AnchoredVwap {
                    snap.map(|anchor| Anchor {
                        price: 0.0,
                        ..anchor
                    })
                } else {
                    self.pointer_anchor(point, projection, snap)
                };
                let Some(anchor) = anchor else {
                    return Effect::None;
                };
                drag = Self::transformed(drag, anchor);
                if !drag.moved || drag.anchors == drag.original {
                    return Effect::None;
                }
                Effect::Commit(Command::Move {
                    id: drag.id,
                    anchors: drag.anchors,
                })
            }
            Input::Leave => {
                if self.drag.take().is_some() {
                    self.invalidate();
                }
                self.hover = None;
                Effect::None
            }
            Input::Key {
                key,
                modifiers,
                seed,
                step_ms,
                price_step,
            } => self.keyboard(key, modifiers, seed, step_ms, price_step, store),
        }
    }

    fn creation_click(&mut self, anchor: Anchor) -> Effect {
        let tool = self.tool;
        if tool.anchors_required() == 1 {
            self.invalidate();
            if !self.repeat {
                self.tool = Tool::Select;
            }
            self.cursor = None;
            if tool == Tool::Text {
                return Effect::Text(anchor);
            }
            return match create_shape(tool, Anchors::One(anchor), None) {
                Ok(shape) => Effect::Commit(Command::Create {
                    shape,
                    style: Style::default(),
                }),
                Err(_) => Effect::None,
            };
        }
        if let Some(mut draft) = self.draft {
            if !draft.first_fixed {
                draft.first = anchor;
                draft.floating = anchor;
                draft.first_fixed = true;
                draft.keyboard = false;
                self.draft = Some(draft);
                return Effect::None;
            }
            let Ok(shape) = create_shape(tool, Anchors::Two(draft.first, anchor), None) else {
                return Effect::None;
            };
            self.invalidate();
            self.draft = None;
            if !self.repeat {
                self.tool = Tool::Select;
            }
            self.cursor = None;
            return Effect::Commit(Command::Create {
                shape,
                style: Style::default(),
            });
        }
        self.draft = Some(Draft {
            tool,
            first: anchor,
            floating: anchor,
            pointer: None,
            projection: None,
            keyboard: false,
            first_fixed: true,
        });
        Effect::None
    }

    fn pointer_anchor(
        &self,
        point: Point,
        projection: Projection,
        snap: Option<Anchor>,
    ) -> Option<Anchor> {
        if self.tool == Tool::AnchoredVwap
            || self
                .drag
                .is_some_and(|drag| drag.tool == Tool::AnchoredVwap)
        {
            return snap.map(|anchor| Anchor {
                price: 0.0,
                ..anchor
            });
        }
        if self.magnet && projection.contains(point) {
            snap.or_else(|| projection.anchor(point))
        } else {
            projection.anchor(point)
        }
    }
    fn transformed(mut drag: Drag, at: Anchor) -> Drag {
        let dt = at.utc_ms.saturating_sub(drag.start.utc_ms);
        let dp = at.price - drag.start.price;
        drag.moved |= dt != 0 || dp != 0.0;
        let anchors = match (drag.original, drag.part) {
            (Anchors::One(a), EditPart::Body) => Anchors::One(shift(
                a,
                if drag.tool == Tool::Horizontal { 0 } else { dt },
                if matches!(drag.tool, Tool::Vertical | Tool::AnchoredVwap) {
                    0.0
                } else {
                    dp
                },
            )),
            (Anchors::Two(a, b), EditPart::Body) => {
                let dt = if drag.tool == Tool::Horizontal { 0 } else { dt };
                let dp = if drag.tool == Tool::Vertical { 0.0 } else { dp };
                Anchors::Two(shift(a, dt, dp), shift(b, dt, dp))
            }
            (Anchors::One(a), EditPart::First | EditPart::Second | EditPart::Corner(_)) => {
                Anchors::One(edit_anchor(drag.tool, a, at))
            }
            (Anchors::Two(_, b), EditPart::First) => Anchors::Two(at, b),
            (Anchors::Two(a, _), EditPart::Second) => Anchors::Two(a, at),
            (Anchors::Two(a, b), EditPart::Corner(n)) => rectangle_corner(a, b, at, n),
        };
        drag.anchors = if drag.moved { anchors } else { drag.original };
        drag
    }

    fn keyboard(
        &mut self,
        key: keyboard::Key,
        modifiers: Modifiers,
        seed: Option<Anchor>,
        step_ms: i64,
        price_step: f64,
        store: &Store,
    ) -> Effect {
        let Key::Named(named) = key else {
            return Effect::None;
        };
        use keyboard::key::Named;
        if named == Named::Escape {
            self.cancel();
            return Effect::None;
        }
        if self.drag.is_some() {
            return Effect::None;
        }
        if modifiers.control() || modifiers.alt() {
            return Effect::None;
        }
        if !matches!(
            named,
            Named::ArrowLeft | Named::ArrowRight | Named::ArrowUp | Named::ArrowDown | Named::Enter
        ) {
            return Effect::None;
        }
        if self.tool == Tool::Select {
            if named == Named::Enter {
                return Effect::None;
            }
            let Some(id) = store.selected() else {
                return Effect::None;
            };
            let Some(drawing) = store.find(id) else {
                return Effect::None;
            };
            if drawing.locked {
                return Effect::None;
            }
            let multiplier = if modifiers.shift() { 10.0 } else { 1.0 };
            let dt = match named {
                Named::ArrowLeft => -step_ms,
                Named::ArrowRight => step_ms,
                _ => 0,
            };
            let dp = match named {
                Named::ArrowUp => price_step,
                Named::ArrowDown => -price_step,
                _ => 0.0,
            };
            if dt == 0 && dp == 0.0 {
                return Effect::None;
            }
            let dt = (dt as f64 * multiplier) as i64;
            let dp = dp * multiplier;
            let (dt, dp) = match drawing.shape.tool() {
                Tool::Horizontal => (0, dp),
                Tool::Vertical | Tool::AnchoredVwap => (dt, 0.0),
                _ => (dt, dp),
            };
            if dt == 0 && dp == 0.0 {
                return Effect::None;
            }
            let anchors = match drawing.shape.anchors() {
                Anchors::One(a) => Anchors::One(shift(a, dt, dp)),
                Anchors::Two(a, b) => Anchors::Two(shift(a, dt, dp), shift(b, dt, dp)),
            };
            return Effect::Commit(Command::Move { id, anchors });
        }
        if self.draft.is_none() {
            let Some(anchor) = seed.or(self.cursor) else {
                return Effect::None;
            };
            self.cursor = Some(anchor);
            self.draft = Some(Draft {
                tool: self.tool,
                first: anchor,
                floating: anchor,
                pointer: None,
                projection: None,
                keyboard: true,
                first_fixed: false,
            });
        }
        let Some(mut draft) = self.draft else {
            return Effect::None;
        };
        if matches!(
            named,
            Named::ArrowLeft | Named::ArrowRight | Named::ArrowUp | Named::ArrowDown
        ) {
            let multiplier = if modifiers.shift() { 10.0 } else { 1.0 };
            let dt = match named {
                Named::ArrowLeft => -step_ms,
                Named::ArrowRight => step_ms,
                _ => 0,
            };
            let dp = match named {
                Named::ArrowUp => price_step,
                Named::ArrowDown => -price_step,
                _ => 0.0,
            };
            draft.floating = shift(
                draft.floating,
                (dt as f64 * multiplier) as i64,
                if draft.tool == Tool::AnchoredVwap {
                    0.0
                } else {
                    dp * multiplier
                },
            );
            self.cursor = Some(draft.floating);
            self.draft = Some(draft);
            return Effect::None;
        }
        if draft.tool.anchors_required() == 1 {
            self.invalidate();
            self.draft = None;
            if !self.repeat {
                self.tool = Tool::Select;
            }
            self.cursor = None;
            return if draft.tool == Tool::Text {
                Effect::Text(draft.floating)
            } else {
                match create_shape(draft.tool, Anchors::One(draft.floating), None) {
                    Ok(shape) => Effect::Commit(Command::Create {
                        shape,
                        style: Style::default(),
                    }),
                    Err(_) => Effect::None,
                }
            };
        }
        if !draft.first_fixed {
            draft.first = draft.floating;
            draft.first_fixed = true;
            self.draft = Some(draft);
            return Effect::None;
        }
        let Ok(shape) = create_shape(draft.tool, Anchors::Two(draft.first, draft.floating), None)
        else {
            return Effect::None;
        };
        self.invalidate();
        self.draft = None;
        if !self.repeat {
            self.tool = Tool::Select;
        }
        self.cursor = None;
        Effect::Commit(Command::Create {
            shape,
            style: Style::default(),
        })
    }

    pub fn preview<'a>(
        &self,
        store: &'a Store,
        projection: Projection,
        cursor: Option<Point>,
    ) -> Option<Preview<'a>> {
        if let Some(draft) = self.draft {
            let floating = if draft.keyboard {
                draft.floating
            } else {
                match cursor {
                    Some(point)
                        if draft.pointer == Some(point) && draft.projection == Some(projection) =>
                    {
                        draft.floating
                    }
                    Some(point) => self
                        .pointer_anchor(point, projection, None)
                        .unwrap_or(draft.floating),
                    None => draft.floating,
                }
            };
            return Some(Preview::Create {
                tool: draft.tool,
                anchors: if draft.tool.anchors_required() == 1 || !draft.first_fixed {
                    Anchors::One(floating)
                } else {
                    Anchors::Two(draft.first, floating)
                },
            });
        }
        let drag = self.drag?;
        let drawing = store.find(drag.id)?;
        Some(Preview::Edit {
            drawing,
            anchors: drag.anchors,
        })
    }
    fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }
}

fn shift(anchor: Anchor, dt: i64, dp: f64) -> Anchor {
    Anchor {
        utc_ms: anchor.utc_ms.saturating_add(dt),
        price: anchor.price + dp,
    }
}
fn edit_anchor(tool: Tool, original: Anchor, at: Anchor) -> Anchor {
    match tool {
        Tool::Horizontal => Anchor {
            utc_ms: original.utc_ms,
            price: at.price,
        },
        Tool::Vertical | Tool::AnchoredVwap => Anchor {
            utc_ms: at.utc_ms,
            price: original.price,
        },
        _ => at,
    }
}
fn rectangle_corner(a: Anchor, b: Anchor, at: Anchor, corner: u8) -> Anchors {
    let left = a.utc_ms.min(b.utc_ms);
    let right = a.utc_ms.max(b.utc_ms);
    let bottom = a.price.min(b.price);
    let top = a.price.max(b.price);
    let opposite = match corner {
        0 => Anchor {
            utc_ms: right,
            price: bottom,
        },
        1 => Anchor {
            utc_ms: left,
            price: bottom,
        },
        2 => Anchor {
            utc_ms: left,
            price: top,
        },
        3 => Anchor {
            utc_ms: right,
            price: top,
        },
        _ => Anchor {
            utc_ms: right,
            price: bottom,
        },
    };
    Anchors::Two(at, opposite)
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{drawing_geometry::Projection, drawing_types::Shape, drawings::Store};

    #[test]
    fn anchored_keyboard_moves_between_actual_native_openings_and_never_moves_price() {
        let mut store = Store::new(&mexc_model::Market::default());
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
        let candles = [0, 300, 900].map(|time| mexc_model::Candle {
            time,
            open: 10.0,
            high: 10.0,
            low: 10.0,
            close: 10.0,
            volume: 1.0,
            flow: None,
        });
        let input = |named| Input::Key {
            key: Key::Named(named),
            modifiers: Modifiers::default(),
            seed: None,
            step_ms: 300_000,
            price_step: 1.0,
        };
        let mut interaction = Interaction::default();
        for expected in [300_000, 900_000] {
            let Effect::Commit(command) = interaction.update_native(
                input(keyboard::key::Named::ArrowRight),
                &store,
                &candles,
            ) else {
                panic!("native opening should be reachable");
            };
            store.apply(command).unwrap();
            assert_eq!(
                store.find(id).unwrap().shape,
                Shape::AnchoredVwap(Anchor {
                    utc_ms: expected,
                    price: 0.0
                })
            );
        }
        assert!(matches!(
            interaction.update_native(input(keyboard::key::Named::ArrowUp), &store, &candles),
            Effect::None
        ));
        store
            .apply(Command::SetLocked { id, locked: true })
            .unwrap();
        assert!(matches!(
            interaction.update_native(input(keyboard::key::Named::ArrowLeft), &store, &candles),
            Effect::None
        ));
        interaction.choose_tool(Tool::AnchoredVwap);
        let Effect::Commit(Command::Create { shape, .. }) = interaction.update_native(
            Input::Key {
                key: Key::Named(keyboard::key::Named::Enter),
                modifiers: Modifiers::default(),
                seed: Some(Anchor {
                    utc_ms: 300_000,
                    price: 999.0,
                }),
                step_ms: 300_000,
                price_step: 1.0,
            },
            &store,
            &candles,
        ) else {
            panic!("Enter should place at the native seed");
        };
        assert_eq!(
            shape,
            Shape::AnchoredVwap(Anchor {
                utc_ms: 300_000,
                price: 0.0
            })
        );
    }
    use iced::{Point, Rectangle};
    use mexc_model::Market;

    fn projection() -> Projection {
        Projection {
            area: Rectangle::new(Point::new(0.0, 0.0), iced::Size::new(1000.0, 1000.0)),
            left: 0.0,
            span: 1000.0,
            low: 0.0,
            high: 100.0,
        }
    }
    fn point(projection: Projection, utc_ms: i64, price: f64) -> Point {
        projection.point(Anchor { utc_ms, price })
    }
    fn create_trend(store: &mut Store, a: Anchor, b: Anchor) -> DrawingId {
        let shape = create_shape(Tool::Trend, Anchors::Two(a, b), None).unwrap();
        store
            .apply(Command::Create {
                shape,
                style: Style::default(),
            })
            .unwrap();
        store.objects()[0].id
    }
    fn apply(effect: Effect, store: &mut Store) {
        if let Effect::Commit(command) = effect {
            store.apply(command).unwrap();
        }
    }

    #[test]
    fn repeat_creation_stays_active_and_escape_always_exits() {
        let p = projection();
        let mut store = Store::new(&Market::default());
        let mut interaction = Interaction::default();
        interaction.choose_tool(Tool::Horizontal);
        assert!(!interaction.repeat());
        interaction.set_repeat(true);
        let down = |time, price| Input::Down {
            point: point(p, time, price),
            projection: p,
            hit: None,
            snap: None,
        };
        apply(interaction.update(down(100_000, 20.0), &store), &mut store);
        assert_eq!(interaction.tool(), Tool::Horizontal);
        apply(interaction.update(down(200_000, 30.0), &store), &mut store);
        assert_eq!(store.objects().len(), 2);
        interaction.update(
            Input::Key {
                key: keyboard::Key::Named(keyboard::key::Named::Escape),
                modifiers: Modifiers::default(),
                seed: None,
                step_ms: 1_000,
                price_step: 1.0,
            },
            &store,
        );
        assert_eq!(interaction.tool(), Tool::Select);
        assert!(!interaction.repeat());
    }
    #[test]
    fn repeated_text_apply_and_cancel_return_to_tool_but_escape_exits() {
        let p = projection();
        let store = Store::new(&Market::default());
        let mut interaction = Interaction::default();
        interaction.choose_tool(Tool::Text);
        interaction.set_repeat(true);
        let down = || Input::Down {
            point: point(p, 100_000, 20.0),
            projection: p,
            hit: None,
            snap: None,
        };
        assert!(matches!(
            interaction.update(down(), &store),
            Effect::Text(_)
        ));
        interaction.suspend_for_text();
        interaction.finish_text(true);
        assert_eq!(interaction.tool(), Tool::Text);
        assert!(matches!(
            interaction.update(down(), &store),
            Effect::Text(_)
        ));
        interaction.suspend_for_text();
        interaction.finish_text(false);
        assert_eq!(interaction.tool(), Tool::Text);
        interaction.cancel();
        assert_eq!(interaction.tool(), Tool::Select);
        assert!(!interaction.repeat());
    }
    #[test]
    fn overlay_suspension_invalidates_capture_without_losing_creation_draft() {
        let p = projection();
        let store = Store::new(&Market::default());
        let mut interaction = Interaction::default();
        interaction.choose_tool(Tool::Trend);
        interaction.set_repeat(true);
        interaction.update(
            Input::Down {
                point: point(p, 100_000, 20.0),
                projection: p,
                hit: None,
                snap: None,
            },
            &store,
        );
        let generation = interaction.generation();
        interaction.suspend_overlay();
        assert_eq!(interaction.generation(), generation + 1);
        assert!(interaction.repeat());
        assert_eq!(interaction.tool(), Tool::Trend);
        assert!(matches!(
            interaction.preview(&store, p, None),
            Some(Preview::Create {
                tool: Tool::Trend,
                ..
            })
        ));
    }
    #[test]
    fn body_translation_preview_and_release_commit_agree() {
        let p = projection();
        let mut store = Store::new(&Market::default());
        let a = Anchor {
            utc_ms: 100_000,
            price: 80.0,
        };
        let b = Anchor {
            utc_ms: 300_000,
            price: 30.0,
        };
        let id = create_trend(&mut store, a, b);
        let mut interaction = Interaction::default();
        let start = point(p, 200_000, 55.0);
        interaction.update(
            Input::Down {
                point: start,
                projection: p,
                hit: Some(Hit {
                    id,
                    part: Part::Body,
                }),
                snap: None,
            },
            &store,
        );
        let moved = point(p, 350_000, 65.0);
        interaction.update(
            Input::Move {
                point: moved,
                projection: p,
                hit: None,
                snap: None,
            },
            &store,
        );
        let Some(Preview::Edit { anchors, .. }) = interaction.preview(&store, p, Some(moved))
        else {
            panic!("missing edit preview");
        };
        let Anchors::Two(first, second) = anchors else {
            panic!("missing trend anchors");
        };
        assert_eq!((first.utc_ms, second.utc_ms), (250_000, 450_000));
        assert!((first.price - 90.0).abs() < 1e-12);
        assert!((second.price - 40.0).abs() < 1e-12);
        let relayout = Projection {
            left: 500.0,
            high: 200.0,
            ..p
        };
        let Some(Preview::Edit {
            anchors: redrawn, ..
        }) = interaction.preview(&store, relayout, Some(Point::new(800.0, 100.0)))
        else {
            panic!("missing edit preview after relayout");
        };
        assert_eq!(redrawn, anchors);
        assert!(matches!(
            interaction.update(
                Input::Key {
                    key: Key::Named(keyboard::key::Named::ArrowLeft),
                    modifiers: Modifiers::empty(),
                    seed: None,
                    step_ms: 60_000,
                    price_step: 1.0,
                },
                &store
            ),
            Effect::None
        ));
        let effect = interaction.update(
            Input::Up {
                point: moved,
                projection: p,
                snap: None,
            },
            &store,
        );
        let expected = anchors;
        apply(effect, &mut store);
        assert_eq!(store.find(id).unwrap().shape.anchors(), expected);
    }

    #[test]
    fn endpoint_crossing_changes_only_dragged_endpoint_and_cancel_restores() {
        let p = projection();
        let mut store = Store::new(&Market::default());
        let a = Anchor {
            utc_ms: 200_000,
            price: 70.0,
        };
        let b = Anchor {
            utc_ms: 400_000,
            price: 40.0,
        };
        let id = create_trend(&mut store, a, b);
        let revision = store.revision();
        let mut interaction = Interaction::default();
        let start = point(p, b.utc_ms, b.price);
        interaction.update(
            Input::Down {
                point: start,
                projection: p,
                hit: Some(Hit {
                    id,
                    part: Part::Second,
                }),
                snap: None,
            },
            &store,
        );
        let crossing = point(p, -100_000, 120.0);
        interaction.update(
            Input::Move {
                point: crossing,
                projection: p,
                hit: None,
                snap: None,
            },
            &store,
        );
        let Some(Preview::Edit { anchors, .. }) = interaction.preview(&store, p, Some(crossing))
        else {
            panic!("missing endpoint preview");
        };
        assert_eq!(
            anchors,
            Anchors::Two(
                a,
                Anchor {
                    utc_ms: -100_000,
                    price: 120.0
                }
            )
        );
        let generation = interaction.generation();
        assert!(interaction.cancel());
        assert!(interaction.generation() > generation);
        assert!(matches!(
            interaction.update(
                Input::Up {
                    point: crossing,
                    projection: p,
                    snap: None
                },
                &store
            ),
            Effect::None
        ));
        assert_eq!(store.find(id).unwrap().shape.anchors(), Anchors::Two(a, b));
        assert_eq!(store.revision(), revision);
    }

    #[test]
    fn locked_objects_and_zero_distance_releases_never_commit() {
        let p = projection();
        let mut store = Store::new(&Market::default());
        let a = Anchor {
            utc_ms: 100_000,
            price: 80.0,
        };
        let b = Anchor {
            utc_ms: 300_000,
            price: 30.0,
        };
        let id = create_trend(&mut store, a, b);
        let mut drawing = store.find(id).unwrap().clone();
        drawing.locked = true;
        store.apply(Command::Replace(drawing)).unwrap();
        let mut interaction = Interaction::default();
        let start = point(p, 200_000, 55.0);
        interaction.update(
            Input::Down {
                point: start,
                projection: p,
                hit: Some(Hit {
                    id,
                    part: Part::Body,
                }),
                snap: None,
            },
            &store,
        );
        assert!(!interaction.is_dragging());
        let revision = store.revision();
        assert!(matches!(
            interaction.update(
                Input::Up {
                    point: start,
                    projection: p,
                    snap: None
                },
                &store
            ),
            Effect::None
        ));
        assert_eq!(store.revision(), revision);
    }

    #[test]
    fn keyboard_creation_uses_seed_without_pointer_history() {
        let store = Store::new(&Market::default());
        let seed = Anchor {
            utc_ms: 500_000,
            price: 50.0,
        };
        let mut interaction = Interaction::default();
        interaction.choose_tool(Tool::Trend);
        let key = |named| Input::Key {
            key: keyboard::Key::Named(named),
            modifiers: Modifiers::default(),
            seed: Some(seed),
            step_ms: 60_000,
            price_step: 1.0,
        };
        assert!(matches!(
            interaction.update(key(keyboard::key::Named::Enter), &store),
            Effect::None
        ));
        assert!(interaction.draft.is_some());
        interaction.update(key(keyboard::key::Named::ArrowRight), &store);
        let effect = interaction.update(key(keyboard::key::Named::Enter), &store);
        let Effect::Commit(Command::Create {
            shape: Shape::Trend(a, b),
            ..
        }) = effect
        else {
            panic!("expected seeded trend commit");
        };
        assert_eq!(a, seed);
        assert_eq!(
            b,
            Anchor {
                utc_ms: 560_000,
                price: 50.0
            }
        );
    }
    #[test]
    fn magnet_snaps_one_shot_text_request_without_committing_payload() {
        let p = projection();
        let store = Store::new(&Market::default());
        let mut interaction = Interaction::default();
        interaction.choose_tool(Tool::Text);
        interaction.set_magnet(true);
        let snap = Anchor {
            utc_ms: 260_000,
            price: 60.0,
        };
        let point = point(p, 250_000, 55.0);
        let effect = interaction.update(
            Input::Down {
                point,
                projection: p,
                hit: None,
                snap: Some(snap),
            },
            &store,
        );
        assert!(matches!(effect, Effect::Text(anchor) if anchor == snap));
        assert_eq!(interaction.tool(), Tool::Select);
        assert_eq!(store.objects().len(), 0);
        assert_eq!(store.revision(), 0);
    }

    #[test]
    fn horizontal_and_vertical_body_moves_change_only_their_geometric_dimension() {
        let p = projection();
        let a = Anchor {
            utc_ms: 500_000,
            price: 50.0,
        };
        for (tool, shape) in [
            (Tool::Horizontal, Shape::Horizontal(a)),
            (Tool::Vertical, Shape::Vertical(a)),
        ] {
            let mut store = Store::new(&Market::default());
            store
                .apply(Command::Create {
                    shape,
                    style: Style::default(),
                })
                .unwrap();
            let id = store.selected().unwrap();
            let mut interaction = Interaction::default();
            let start = p.point(a);
            let end = point(p, 650_000, 70.0);
            interaction.update(
                Input::Down {
                    point: start,
                    projection: p,
                    hit: Some(Hit {
                        id,
                        part: Part::Body,
                    }),
                    snap: None,
                },
                &store,
            );
            interaction.update(
                Input::Move {
                    point: end,
                    projection: p,
                    hit: None,
                    snap: None,
                },
                &store,
            );
            apply(
                interaction.update(
                    Input::Up {
                        point: end,
                        projection: p,
                        snap: None,
                    },
                    &store,
                ),
                &mut store,
            );
            let changed = store.find(id).unwrap().shape.anchors().first();
            if tool == Tool::Horizontal {
                assert_eq!(changed.utc_ms, a.utc_ms);
                assert!((changed.price - 70.0).abs() < 1e-8);
            } else {
                assert_eq!(changed.utc_ms, 650_000);
                assert_eq!(changed.price, a.price);
            }
            store.apply(Command::Undo).unwrap();
            assert_eq!(store.find(id).unwrap().shape.anchors().first(), a);
        }
    }

    #[test]
    fn keyboard_can_position_first_anchor_before_confirming_it() {
        let mut store = Store::new(&Market::default());
        let seed = Anchor {
            utc_ms: 500_000,
            price: 50.0,
        };
        let mut interaction = Interaction::default();
        interaction.choose_tool(Tool::Trend);
        let key = |named| Input::Key {
            key: keyboard::Key::Named(named),
            modifiers: Modifiers::default(),
            seed: Some(seed),
            step_ms: 60_000,
            price_step: 1.0,
        };
        interaction.update(key(keyboard::key::Named::ArrowRight), &store);
        interaction.update(key(keyboard::key::Named::ArrowUp), &store);
        assert!(matches!(
            interaction.update(key(keyboard::key::Named::Enter), &store),
            Effect::None
        ));
        interaction.update(key(keyboard::key::Named::ArrowRight), &store);
        interaction.update(key(keyboard::key::Named::ArrowUp), &store);
        apply(
            interaction.update(key(keyboard::key::Named::Enter), &store),
            &mut store,
        );
        assert_eq!(
            store.objects()[0].shape,
            Shape::Trend(
                Anchor {
                    utc_ms: 560_000,
                    price: 51.0
                },
                Anchor {
                    utc_ms: 620_000,
                    price: 52.0
                }
            )
        );
    }

    #[test]
    fn selecting_without_pointer_motion_preserves_anchors_across_layout_changes() {
        let p = projection();
        let mut store = Store::new(&Market::default());
        let a = Anchor {
            utc_ms: 200_000,
            price: 70.0,
        };
        let b = Anchor {
            utc_ms: 400_000,
            price: 40.0,
        };
        let id = create_trend(&mut store, a, b);
        let mut interaction = Interaction::default();
        let start = point(p, b.utc_ms, b.price);
        interaction.update(
            Input::Down {
                point: start,
                projection: p,
                hit: Some(Hit {
                    id,
                    part: Part::Second,
                }),
                snap: None,
            },
            &store,
        );
        // A contextual row can move the canvas without any physical pointer motion.
        let relocated = Point::new(start.x, start.y - 24.0);
        let Some(Preview::Edit { anchors, .. }) = interaction.preview(&store, p, Some(relocated))
        else {
            panic!("missing selection preview");
        };
        assert_eq!(anchors, Anchors::Two(a, b));
        assert!(matches!(
            interaction.update(
                Input::Up {
                    point: relocated,
                    projection: p,
                    snap: None
                },
                &store
            ),
            Effect::None
        ));
    }
}
