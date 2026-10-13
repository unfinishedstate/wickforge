use crate::drawing_types::{
    Anchor, Anchors, Command, Drawing, DrawingId, FIB_LEVELS, InstrumentKey, Shape, Style, Tool,
};
use chrono::{DateTime, Utc};
use mexc_model::Market;
use std::collections::HashMap;

/// Session-owned books use native instrument identity, never the displayed timeframe.
pub struct Store {
    books: HashMap<InstrumentKey, Book>,
    key: InstrumentKey,
    generation: u64,
    next_id: DrawingId,
}

#[derive(Default)]
struct Book {
    objects: Vec<Drawing>,
    selected: Option<DrawingId>,
    revision: u64,
    undo: Vec<Delta>,
    redo: Vec<Delta>,
}

/// An inverse operation owns only the removed/replaced object or changed field.
/// Replaying swaps ownership back into the scene and produces the opposite delta.
enum Delta {
    Insert { index: usize, drawing: Drawing },
    Remove { index: usize },
    Replace { index: usize, drawing: Drawing },
    Move { index: usize, anchors: Anchors },
    Visible { index: usize, visible: bool },
    Locked { index: usize, locked: bool },
}

impl Delta {
    fn replay(self, objects: &mut Vec<Drawing>) -> Self {
        match self {
            Self::Insert { index, drawing } => {
                objects.insert(index, drawing);
                Self::Remove { index }
            }
            Self::Remove { index } => Self::Insert {
                index,
                drawing: objects.remove(index),
            },
            Self::Replace { index, drawing } => Self::Replace {
                index,
                drawing: std::mem::replace(&mut objects[index], drawing),
            },
            Self::Move { index, anchors } => {
                let previous = objects[index].shape.anchors();
                set_anchors(&mut objects[index].shape, anchors);
                Self::Move {
                    index,
                    anchors: previous,
                }
            }
            Self::Visible { index, visible } => Self::Visible {
                index,
                visible: std::mem::replace(&mut objects[index].visible, visible),
            },
            Self::Locked { index, locked } => Self::Locked {
                index,
                locked: std::mem::replace(&mut objects[index].locked, locked),
            },
        }
    }
}

impl Book {
    fn index(&self, id: DrawingId) -> Result<usize, String> {
        self.objects
            .iter()
            .position(|drawing| drawing.id == id)
            .ok_or_else(|| format!("Drawing {id} is not in the active instrument book"))
    }

    fn commit(&mut self, delta: Delta) {
        let inverse = delta.replay(&mut self.objects);
        self.undo.push(inverse);
        self.redo.clear();
        self.changed();
    }

    fn changed(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        if self
            .selected
            .is_some_and(|id| !self.objects.iter().any(|drawing| drawing.id == id))
        {
            self.selected = None;
        }
    }

    fn undo(&mut self) -> bool {
        let Some(delta) = self.undo.pop() else {
            return false;
        };
        self.redo.push(delta.replay(&mut self.objects));
        self.changed();
        true
    }

    fn redo(&mut self) -> bool {
        let Some(delta) = self.redo.pop() else {
            return false;
        };
        self.undo.push(delta.replay(&mut self.objects));
        self.changed();
        true
    }
}

impl Store {
    pub fn new(market: &Market) -> Self {
        let key = InstrumentKey::from(market);
        let mut books = HashMap::new();
        books.insert(key.clone(), Book::default());
        Self {
            books,
            key,
            generation: 0,
            next_id: 1,
        }
    }

    pub fn switch_context(&mut self, market: &Market) {
        self.book_mut().selected = None;
        self.key = InstrumentKey::from(market);
        self.books.entry(self.key.clone()).or_default().selected = None;
        self.generation = self.generation.wrapping_add(1);
    }

    pub fn key(&self) -> &InstrumentKey {
        &self.key
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn objects(&self) -> &[Drawing] {
        &self.book().objects
    }

    pub fn find(&self, id: DrawingId) -> Option<&Drawing> {
        self.objects().iter().find(|drawing| drawing.id == id)
    }

    pub fn selected(&self) -> Option<DrawingId> {
        self.book().selected
    }

    pub fn selected_drawing(&self) -> Option<&Drawing> {
        self.selected().and_then(|id| self.find(id))
    }

    /// Returns whether selection changed. Unknown IDs leave selection untouched.
    pub fn select(&mut self, id: Option<DrawingId>) -> bool {
        if id.is_some_and(|id| self.find(id).is_none()) || self.selected() == id {
            return false;
        }
        self.book_mut().selected = id;
        true
    }

    pub fn revision(&self) -> u64 {
        self.book().revision
    }

    pub fn can_undo(&self) -> bool {
        !self.book().undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.book().redo.is_empty()
    }

    pub fn apply(&mut self, command: Command) -> Result<bool, String> {
        let delta = match command {
            Command::Undo => return Ok(self.book_mut().undo()),
            Command::Redo => return Ok(self.book_mut().redo()),
            Command::Create { shape, style } => {
                validate_shape(&shape)?;
                validate_style(&style)?;
                let id = self.next_id;
                let next_id = id
                    .checked_add(1)
                    .ok_or_else(|| "Drawing IDs have been exhausted".to_owned())?;
                let drawing = Drawing {
                    id,
                    shape,
                    style,
                    visible: true,
                    locked: false,
                };
                let index = self.objects().len();
                self.next_id = next_id;
                self.book_mut().commit(Delta::Insert { index, drawing });
                self.book_mut().selected = Some(id);
                return Ok(true);
            }
            Command::Replace(drawing) => {
                let index = self.book().index(drawing.id)?;
                validate_shape(&drawing.shape)?;
                validate_style(&drawing.style)?;
                let old = &self.objects()[index];
                if old.locked && old.shape != drawing.shape {
                    return Err(
                        "Unlock the drawing before changing its geometry or content".to_owned()
                    );
                }
                if old == &drawing {
                    return Ok(false);
                }
                Delta::Replace { index, drawing }
            }
            Command::Move { id, anchors } => {
                let index = self.book().index(id)?;
                let drawing = &self.objects()[index];
                if drawing.locked {
                    return Err("Unlock the drawing before moving or resizing it".to_owned());
                }
                validate_shape_at(&drawing.shape, anchors)?;
                if drawing.shape.anchors() == anchors {
                    return Ok(false);
                }
                Delta::Move { index, anchors }
            }
            Command::Delete(id) => Delta::Remove {
                index: self.book().index(id)?,
            },
            Command::SetVisible { id, visible } => {
                let index = self.book().index(id)?;
                if self.objects()[index].visible == visible {
                    return Ok(false);
                }
                Delta::Visible { index, visible }
            }
            Command::SetLocked { id, locked } => {
                let index = self.book().index(id)?;
                if self.objects()[index].locked == locked {
                    return Ok(false);
                }
                Delta::Locked { index, locked }
            }
        };
        self.book_mut().commit(delta);
        Ok(true)
    }

    fn book(&self) -> &Book {
        self.books
            .get(&self.key)
            .expect("active drawing book exists")
    }

    fn book_mut(&mut self) -> &mut Book {
        self.books
            .get_mut(&self.key)
            .expect("active drawing book exists")
    }
}

pub fn validate_style(style: &Style) -> Result<(), String> {
    if !style.width.is_finite() || !(0.5..=20.0).contains(&style.width) {
        return Err("Line width must be finite and between 0.5 and 20".to_owned());
    }
    if !style.font_size.is_finite() || !(8.0..=128.0).contains(&style.font_size) {
        return Err("Font size must be finite and between 8 and 128".to_owned());
    }
    if !style.fill_opacity.is_finite() || !(0.0..=1.0).contains(&style.fill_opacity) {
        return Err("Fill opacity must be finite and between 0 and 1".to_owned());
    }
    Ok(())
}

pub fn validate_shape(shape: &Shape) -> Result<(), String> {
    validate_shape_at(shape, shape.anchors())
}

pub fn create_shape(tool: Tool, anchors: Anchors, text: Option<String>) -> Result<Shape, String> {
    let shape = match (tool, anchors) {
        (Tool::Horizontal, Anchors::One(a)) => Shape::Horizontal(a),
        (Tool::Vertical, Anchors::One(a)) => Shape::Vertical(a),
        (Tool::Trend, Anchors::Two(a, b)) => Shape::Trend(a, b),
        (Tool::Ray, Anchors::Two(a, b)) => Shape::Ray(a, b),
        (Tool::Rectangle, Anchors::Two(a, b)) => Shape::Rectangle(a, b),
        (Tool::Fibonacci, Anchors::Two(a, b)) => Shape::Fibonacci {
            a,
            b,
            levels: FIB_LEVELS.to_vec(),
            reverse: false,
            extend_right: true,
        },
        (Tool::Text, Anchors::One(at)) => Shape::Text {
            at,
            text: text.ok_or_else(|| "Enter text for the annotation".to_owned())?,
        },
        (Tool::Select, _) => return Err("Select is not a drawing creation tool".to_owned()),
        _ => return Err("The tool received the wrong number of anchors".to_owned()),
    };
    validate_shape(&shape)?;
    Ok(shape)
}

fn validate_anchor(anchor: Anchor) -> Result<(), String> {
    if DateTime::<Utc>::from_timestamp_millis(anchor.utc_ms).is_none() {
        return Err("Anchor time is outside the supported UTC calendar".to_owned());
    }
    if !anchor.price.is_finite() || anchor.price.abs() > 1e100 {
        return Err("Anchor price must be finite and within ±1e100".to_owned());
    }
    Ok(())
}

/// Validate alternate anchors against borrowed payloads, without copying text or levels.
fn validate_shape_at(shape: &Shape, anchors: Anchors) -> Result<(), String> {
    validate_anchor(anchors.first())?;
    if let Some(second) = anchors.second() {
        validate_anchor(second)?;
    }
    match (shape, anchors) {
        (Shape::Horizontal(_) | Shape::Vertical(_), Anchors::One(_)) => {}
        (Shape::Text { text, .. }, Anchors::One(_)) => {
            if text.trim().is_empty() {
                return Err("Annotation text must not be empty".to_owned());
            }
        }
        (Shape::Trend(_, _) | Shape::Ray(_, _), Anchors::Two(a, b)) => {
            if a == b {
                return Err("Line anchors must be distinct".to_owned());
            }
        }
        (Shape::Rectangle(_, _), Anchors::Two(a, b)) => {
            if a.utc_ms == b.utc_ms || a.price == b.price {
                return Err("A rectangle needs different times and prices".to_owned());
            }
        }
        (
            Shape::Fibonacci {
                levels, reverse, ..
            },
            Anchors::Two(a, b),
        ) => {
            if a.price == b.price {
                return Err("A Fibonacci swing needs different prices".to_owned());
            }
            if levels.is_empty() {
                return Err("A Fibonacci drawing needs at least one level".to_owned());
            }
            let (start, end) = if *reverse {
                (b.price, a.price)
            } else {
                (a.price, b.price)
            };
            for (index, level) in levels.iter().copied().enumerate() {
                if !level.is_finite() || !(level * 100.0).is_finite() {
                    return Err("Fibonacci levels and percentage labels must be finite".to_owned());
                }
                if levels[..index].contains(&level) {
                    return Err("Fibonacci levels must be unique".to_owned());
                }
                if !(end + level * (start - end)).is_finite() {
                    return Err("A Fibonacci level produces a non-finite price".to_owned());
                }
            }
        }
        _ => return Err("The drawing received the wrong number of anchors".to_owned()),
    }
    Ok(())
}

/// Call only after validation, or when replaying an already-valid history delta.
fn set_anchors(shape: &mut Shape, anchors: Anchors) {
    match (shape, anchors) {
        (Shape::Horizontal(at) | Shape::Vertical(at) | Shape::Text { at, .. }, Anchors::One(a)) => {
            *at = a
        }
        (
            Shape::Trend(first, second)
            | Shape::Ray(first, second)
            | Shape::Rectangle(first, second)
            | Shape::Fibonacci {
                a: first,
                b: second,
                ..
            },
            Anchors::Two(a, b),
        ) => {
            *first = a;
            *second = b;
        }
        _ => unreachable!("validated drawing anchor arity"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mexc_model::{Exchange, Product, Timeframe};

    fn anchor(utc_ms: i64, price: f64) -> Anchor {
        Anchor { utc_ms, price }
    }

    fn add(store: &mut Store, shape: Shape) -> DrawingId {
        assert!(
            store
                .apply(Command::Create {
                    shape,
                    style: Style::default()
                })
                .unwrap()
        );
        store.selected().unwrap()
    }

    fn line(store: &mut Store, price: f64) -> DrawingId {
        add(store, Shape::Horizontal(anchor(1_700_000_000_000, price)))
    }

    #[test]
    fn native_books_isolate_venue_product_and_exact_symbol_but_reuse_timeframes() {
        let original = Market::default();
        let mut store = Store::new(&original);
        let id = line(&mut store, 100.0);
        let original_drawing = store.find(id).unwrap().clone();
        let revision = store.revision();
        let variants = [
            Market {
                exchange: Exchange::Binance,
                ..original.clone()
            },
            Market {
                product: Product::Spot,
                ..original.clone()
            },
            Market {
                symbol: "btc_usdc".into(),
                ..original.clone()
            },
            Market {
                symbol: "BTCUSDC".into(),
                ..original.clone()
            },
        ];
        let mut ids = vec![id];
        for market in &variants {
            let generation = store.generation();
            store.switch_context(market);
            assert_eq!(store.generation(), generation + 1);
            assert!(store.objects().is_empty());
            assert!(store.apply(Command::Delete(id)).is_err());
            let other = line(&mut store, 50.0);
            assert!(!ids.contains(&other));
            ids.push(other);
        }
        for (market, id) in variants.iter().zip(&ids[1..]) {
            store.switch_context(market);
            assert_eq!(store.objects().len(), 1);
            assert_eq!(store.objects()[0].id, *id);
            assert!(store.selected().is_none());
        }
        store.switch_context(&Market {
            interval: Timeframe::Hour4,
            ..original.clone()
        });
        assert_eq!(store.objects(), &[original_drawing]);
        assert_eq!(store.revision(), revision);
        assert!(store.selected().is_none());
        assert!(store.can_undo());
        assert!(store.apply(Command::Undo).unwrap());
        assert!(store.objects().is_empty());
        store.switch_context(&original);
        assert!(store.can_redo());
        assert!(store.apply(Command::Redo).unwrap());
        assert_eq!(store.objects()[0].id, id);
    }

    #[test]
    fn selection_is_non_scene_state_and_survives_hidden_and_locked_objects() {
        let mut store = Store::new(&Market::default());
        let id = line(&mut store, 100.0);
        store
            .apply(Command::SetVisible { id, visible: false })
            .unwrap();
        store
            .apply(Command::SetLocked { id, locked: true })
            .unwrap();
        let revision = store.revision();
        assert!(store.select(None));
        assert!(store.select(Some(id)));
        assert!(!store.select(Some(u64::MAX)));
        assert_eq!(store.selected_drawing().unwrap().id, id);
        assert_eq!(store.revision(), revision);
        let generation = store.generation();
        store.switch_context(&Market::default());
        assert_eq!(store.generation(), generation + 1);
        assert!(store.selected().is_none());
        assert_eq!(store.objects().len(), 1);
        assert_eq!(store.revision(), revision);
    }

    #[test]
    fn deletion_undo_restores_ids_order_and_valid_selection() {
        let mut store = Store::new(&Market::default());
        let first = line(&mut store, 10.0);
        let middle = line(&mut store, 20.0);
        let last = line(&mut store, 30.0);
        store.select(Some(middle));
        let expected = store.objects().to_vec();
        let revision = store.revision();
        store.apply(Command::Delete(middle)).unwrap();
        assert_eq!(
            store.objects().iter().map(|d| d.id).collect::<Vec<_>>(),
            [first, last]
        );
        assert!(store.selected().is_none());
        store.apply(Command::Undo).unwrap();
        assert_eq!(store.objects(), expected);
        store.select(Some(middle));
        store.apply(Command::Redo).unwrap();
        assert!(store.selected().is_none());
        assert_eq!(store.revision(), revision + 3);
        let new = line(&mut store, 40.0);
        assert!(new > last);
        assert!(!store.can_redo());
        assert!(store.find(middle).is_none());
    }

    #[test]
    fn no_ops_and_invalid_commands_preserve_redo_and_revision() {
        let mut store = Store::new(&Market::default());
        assert!(!store.apply(Command::Undo).unwrap());
        assert!(!store.apply(Command::Redo).unwrap());
        let id = line(&mut store, 100.0);
        store
            .apply(Command::SetVisible { id, visible: false })
            .unwrap();
        store.apply(Command::Undo).unwrap();
        let before = store.objects().to_vec();
        let revision = store.revision();
        let drawing = store.find(id).unwrap().clone();
        for command in [
            Command::Replace(drawing.clone()),
            Command::Move {
                id,
                anchors: drawing.shape.anchors(),
            },
            Command::SetVisible { id, visible: true },
            Command::SetLocked { id, locked: false },
        ] {
            assert!(!store.apply(command).unwrap());
        }
        assert!(
            store
                .apply(Command::Move {
                    id,
                    anchors: Anchors::One(anchor(0, f64::NAN))
                })
                .is_err()
        );
        assert!(store.apply(Command::Delete(u64::MAX)).is_err());
        assert_eq!(store.objects(), before);
        assert_eq!(store.revision(), revision);
        assert!(store.can_redo());
        store.apply(Command::Redo).unwrap();
        assert!(!store.find(id).unwrap().visible);
        store.apply(Command::Undo).unwrap();
        store
            .apply(Command::Move {
                id,
                anchors: Anchors::One(anchor(10, 120.0)),
            })
            .unwrap();
        assert!(!store.can_redo());
        store.apply(Command::Undo).unwrap();
        assert_eq!(store.objects(), before);
    }

    #[test]
    fn replacement_is_one_atomic_transaction_and_lock_needs_explicit_unlock() {
        let mut store = Store::new(&Market::default());
        let id = add(
            &mut store,
            Shape::Rectangle(anchor(10, 100.0), anchor(20, 200.0)),
        );
        let old = store.find(id).unwrap().clone();
        let mut replacement = old.clone();
        replacement.shape = Shape::Text {
            at: anchor(30, 150.0),
            text: "swing high".into(),
        };
        replacement.style.width = 3.0;
        replacement.visible = false;
        replacement.locked = true;
        let revision = store.revision();
        store.apply(Command::Replace(replacement.clone())).unwrap();
        assert_eq!(store.revision(), revision + 1);
        store.apply(Command::Undo).unwrap();
        assert_eq!(store.find(id), Some(&old));
        store.apply(Command::Redo).unwrap();
        assert_eq!(store.find(id), Some(&replacement));
        let revision = store.revision();
        let mut invalid = replacement.clone();
        invalid.locked = false;
        invalid.shape = Shape::Horizontal(anchor(30, 999.0));
        assert!(store.apply(Command::Replace(invalid)).is_err());
        assert!(
            store
                .apply(Command::Move {
                    id,
                    anchors: Anchors::One(anchor(40, 151.0))
                })
                .is_err()
        );
        assert_eq!(store.find(id), Some(&replacement));
        assert_eq!(store.revision(), revision);
        let mut styling = replacement.clone();
        styling.style.font_size = 24.0;
        assert!(store.apply(Command::Replace(styling)).unwrap());
        store
            .apply(Command::SetVisible { id, visible: true })
            .unwrap();
        store
            .apply(Command::SetLocked { id, locked: false })
            .unwrap();
        store
            .apply(Command::Move {
                id,
                anchors: Anchors::One(anchor(40, 151.0)),
            })
            .unwrap();
        assert_eq!(
            store.find(id).unwrap().shape.anchors(),
            Anchors::One(anchor(40, 151.0))
        );
    }

    #[test]
    fn invalid_multi_field_replacement_does_not_partially_apply() {
        let mut store = Store::new(&Market::default());
        let id = line(&mut store, 100.0);
        let old = store.find(id).unwrap().clone();
        let revision = store.revision();
        let mut invalid = old.clone();
        invalid.style.fill_opacity = 2.0;
        invalid.visible = false;
        invalid.locked = true;
        invalid.shape = Shape::Horizontal(anchor(0, 200.0));
        assert!(store.apply(Command::Replace(invalid)).is_err());
        assert_eq!(store.find(id), Some(&old));
        assert_eq!(store.revision(), revision);
        store.apply(Command::Undo).unwrap();
        assert!(store.objects().is_empty());
        store.apply(Command::Redo).unwrap();
        assert_eq!(store.find(id), Some(&old));
    }

    #[test]
    fn anchor_moves_and_history_keep_owned_payloads_in_place() {
        let mut store = Store::new(&Market::default());
        let text = add(
            &mut store,
            Shape::Text {
                at: anchor(10, 10.0),
                text: "a real annotation".into(),
            },
        );
        let text_ptr = match &store.find(text).unwrap().shape {
            Shape::Text { text, .. } => text.as_ptr(),
            _ => unreachable!(),
        };
        store
            .apply(Command::Move {
                id: text,
                anchors: Anchors::One(anchor(20, 20.0)),
            })
            .unwrap();
        store.apply(Command::Undo).unwrap();
        store.apply(Command::Redo).unwrap();
        match &store.find(text).unwrap().shape {
            Shape::Text { at, text } => {
                assert_eq!(*at, anchor(20, 20.0));
                assert_eq!(text.as_ptr(), text_ptr);
            }
            _ => unreachable!(),
        }
        let fib = add(
            &mut store,
            create_shape(
                Tool::Fibonacci,
                Anchors::Two(anchor(1, 10.0), anchor(2, 20.0)),
                None,
            )
            .unwrap(),
        );
        let levels_ptr = match &store.find(fib).unwrap().shape {
            Shape::Fibonacci { levels, .. } => levels.as_ptr(),
            _ => unreachable!(),
        };
        store
            .apply(Command::Move {
                id: fib,
                anchors: Anchors::Two(anchor(3, 30.0), anchor(4, 40.0)),
            })
            .unwrap();
        store.apply(Command::Undo).unwrap();
        match &store.find(fib).unwrap().shape {
            Shape::Fibonacci { a, b, levels, .. } => {
                assert_eq!((*a, *b), (anchor(1, 10.0), anchor(2, 20.0)));
                assert_eq!(levels.as_ptr(), levels_ptr);
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn failed_creates_and_wrong_anchor_edits_preserve_history_selection_and_ids() {
        let mut store = Store::new(&Market::default());
        let id = add(
            &mut store,
            Shape::Rectangle(anchor(10, 10.0), anchor(20, 20.0)),
        );
        store
            .apply(Command::SetVisible { id, visible: false })
            .unwrap();
        store.apply(Command::Undo).unwrap();
        let before = store.objects().to_vec();
        let revision = store.revision();
        for command in [
            Command::Create {
                shape: Shape::Text {
                    at: anchor(10, 10.0),
                    text: " \t".into(),
                },
                style: Style::default(),
            },
            Command::Create {
                shape: Shape::Horizontal(anchor(10, 10.0)),
                style: Style {
                    width: f32::NAN,
                    ..Style::default()
                },
            },
            Command::Move {
                id,
                anchors: Anchors::One(anchor(10, 10.0)),
            },
            Command::Move {
                id,
                anchors: Anchors::Two(anchor(10, 10.0), anchor(10, 20.0)),
            },
            Command::Move {
                id,
                anchors: Anchors::Two(anchor(10, 10.0), anchor(20, 10.0)),
            },
        ] {
            assert!(store.apply(command).is_err());
            assert_eq!(store.objects(), before);
            assert_eq!(store.selected(), Some(id));
            assert_eq!(store.revision(), revision);
            assert!(store.can_redo());
        }
        let next = line(&mut store, 30.0);
        assert_eq!(next, id + 1);
        assert!(!store.can_redo());
        store.apply(Command::Undo).unwrap();
        assert_eq!(store.objects(), before);
    }

    #[test]
    fn every_command_rejects_stale_ids_without_touching_other_books() {
        let original = Market::default();
        let mut store = Store::new(&original);
        let id = line(&mut store, 100.0);
        let stale = store.find(id).unwrap().clone();
        store.switch_context(&Market {
            symbol: "ETH_USDC".into(),
            ..original.clone()
        });
        let current = line(&mut store, 200.0);
        let before = store.objects().to_vec();
        let revision = store.revision();
        for command in [
            Command::Replace(stale.clone()),
            Command::Move {
                id,
                anchors: Anchors::One(anchor(10, 300.0)),
            },
            Command::Delete(id),
            Command::SetVisible { id, visible: false },
            Command::SetLocked { id, locked: true },
        ] {
            assert!(store.apply(command).is_err());
            assert_eq!(store.objects(), before);
            assert_eq!(store.selected(), Some(current));
            assert_eq!(store.revision(), revision);
        }
        store.switch_context(&original);
        assert_eq!(store.find(id), Some(&stale));
        store.apply(Command::Delete(id)).unwrap();
        assert!(store.apply(Command::Replace(stale)).is_err());
    }

    #[test]
    fn geometry_validates_calendar_finite_bounds_and_actual_degeneracy() {
        for a in [
            anchor(i64::MAX, 1.0),
            anchor(i64::MIN, 1.0),
            anchor(0, f64::INFINITY),
            anchor(0, f64::NAN),
            anchor(0, 1.1e100),
            anchor(0, -1.1e100),
        ] {
            assert!(validate_shape(&Shape::Horizontal(a)).is_err());
        }
        for a in [
            anchor(DateTime::<Utc>::MIN_UTC.timestamp_millis(), -1e100),
            anchor(DateTime::<Utc>::MAX_UTC.timestamp_millis(), 1e100),
            anchor(0, -10.0),
        ] {
            assert!(validate_shape(&Shape::Horizontal(a)).is_ok());
        }
        let a = anchor(10, 10.0);
        assert!(validate_shape(&Shape::Trend(a, a)).is_err());
        assert!(validate_shape(&Shape::Ray(a, a)).is_err());
        assert!(validate_shape(&Shape::Trend(a, anchor(10, 20.0))).is_ok());
        assert!(validate_shape(&Shape::Ray(a, anchor(20, 10.0))).is_ok());
        assert!(validate_shape(&Shape::Rectangle(a, anchor(10, 20.0))).is_err());
        assert!(validate_shape(&Shape::Rectangle(a, anchor(20, 10.0))).is_err());
        assert!(validate_shape(&Shape::Rectangle(a, anchor(0, 20.0))).is_ok());
        assert!(create_shape(Tool::Trend, Anchors::One(a), None).is_err());
        assert!(create_shape(Tool::Text, Anchors::One(a), None).is_err());
        assert!(create_shape(Tool::Text, Anchors::One(a), Some(" \n\t".into())).is_err());
        let shape =
            create_shape(Tool::Text, Anchors::One(a), Some("  level\nnotes  ".into())).unwrap();
        assert_eq!(
            shape,
            Shape::Text {
                at: a,
                text: "  level\nnotes  ".into()
            }
        );
    }

    #[test]
    fn fibonacci_accepts_same_time_and_extensions_but_rejects_invalid_math() {
        let a = anchor(10, 100.0);
        let b = anchor(10, 200.0);
        for reverse in [false, true] {
            let shape = Shape::Fibonacci {
                a,
                b,
                levels: vec![-0.5, 0.0, 0.618, 1.0, 1.618],
                reverse,
                extend_right: false,
            };
            assert!(validate_shape(&shape).is_ok());
            for levels in [
                vec![],
                vec![0.0, -0.0],
                vec![0.618, 0.618],
                vec![f64::NAN],
                vec![f64::INFINITY],
                vec![f64::MAX / 10.0],
            ] {
                let invalid = Shape::Fibonacci {
                    a,
                    b,
                    levels,
                    reverse,
                    extend_right: true,
                };
                assert!(validate_shape(&invalid).is_err());
            }
            let overflow = Shape::Fibonacci {
                a: anchor(10, -1e100),
                b: anchor(20, 1e100),
                levels: vec![1e210],
                reverse,
                extend_right: true,
            };
            assert!(validate_shape(&overflow).is_err());
            let flat = Shape::Fibonacci {
                a,
                b: anchor(20, a.price),
                levels: vec![0.5],
                reverse,
                extend_right: true,
            };
            assert!(validate_shape(&flat).is_err());
        }
    }

    #[test]
    fn style_rejects_nonfinite_and_outside_ranges_with_inclusive_boundaries() {
        for (width, font_size, fill_opacity) in [(0.5, 8.0, 0.0), (20.0, 128.0, 1.0)] {
            assert!(
                validate_style(&Style {
                    width,
                    font_size,
                    fill_opacity,
                    ..Style::default()
                })
                .is_ok()
            );
        }
        for width in [0.49, 20.01, f32::NAN, f32::INFINITY] {
            assert!(
                validate_style(&Style {
                    width,
                    ..Style::default()
                })
                .is_err()
            );
        }
        for font_size in [7.99, 128.01, f32::NAN, f32::NEG_INFINITY] {
            assert!(
                validate_style(&Style {
                    font_size,
                    ..Style::default()
                })
                .is_err()
            );
        }
        for fill_opacity in [-0.01, 1.01, f32::NAN, f32::INFINITY] {
            assert!(
                validate_style(&Style {
                    fill_opacity,
                    ..Style::default()
                })
                .is_err()
            );
        }
    }
}
