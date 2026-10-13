use mexc_model::{Exchange, Market, Product};
use std::fmt;

pub type DrawingId = u64;
pub const FIB_LEVELS: [f64; 7] = [0.0, 0.236, 0.382, 0.5, 0.618, 0.786, 1.0];

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct InstrumentKey {
    pub exchange: Exchange,
    pub product: Product,
    pub symbol: String,
}

impl From<&Market> for InstrumentKey {
    fn from(market: &Market) -> Self {
        Self {
            exchange: market.exchange,
            product: market.product,
            symbol: market.symbol.clone(),
        }
    }
}

impl fmt::Display for InstrumentKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} · {} · {}", self.exchange, self.product, self.symbol)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Anchor {
    pub utc_ms: i64,
    pub price: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Anchors {
    One(Anchor),
    Two(Anchor, Anchor),
}

impl Anchors {
    pub fn first(self) -> Anchor {
        match self {
            Self::One(a) | Self::Two(a, _) => a,
        }
    }
    pub fn second(self) -> Option<Anchor> {
        match self {
            Self::One(_) => None,
            Self::Two(_, b) => Some(b),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tool {
    #[default]
    Select,
    Horizontal,
    Vertical,
    Trend,
    Ray,
    Rectangle,
    Fibonacci,
    Text,
}

impl Tool {
    pub const ALL: [Self; 8] = [
        Self::Select,
        Self::Horizontal,
        Self::Vertical,
        Self::Trend,
        Self::Ray,
        Self::Rectangle,
        Self::Fibonacci,
        Self::Text,
    ];
    pub fn anchors_required(self) -> usize {
        match self {
            Self::Select => 0,
            Self::Horizontal | Self::Vertical | Self::Text => 1,
            Self::Trend | Self::Ray | Self::Rectangle | Self::Fibonacci => 2,
        }
    }
}

impl fmt::Display for Tool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Select => "Select",
            Self::Horizontal => "Horizontal line",
            Self::Vertical => "Vertical line",
            Self::Trend => "Trend line",
            Self::Ray => "Ray",
            Self::Rectangle => "Rectangle / zone",
            Self::Fibonacci => "Fibonacci retracement",
            Self::Text => "Text annotation",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Ink {
    #[default]
    Accent,
    Foreground,
    Up,
    Down,
}
impl Ink {
    pub const ALL: [Self; 4] = [Self::Accent, Self::Foreground, Self::Up, Self::Down];
}
impl fmt::Display for Ink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Accent => "Accent",
            Self::Foreground => "Foreground",
            Self::Up => "Up",
            Self::Down => "Down",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Pattern {
    #[default]
    Solid,
    Dashed,
    Dotted,
}
impl Pattern {
    pub const ALL: [Self; 3] = [Self::Solid, Self::Dashed, Self::Dotted];
}
impl fmt::Display for Pattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Solid => "Solid",
            Self::Dashed => "Dashed",
            Self::Dotted => "Dotted",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Style {
    pub ink: Ink,
    pub pattern: Pattern,
    pub width: f32,
    pub fill_opacity: f32,
    pub font_size: f32,
}
impl Default for Style {
    fn default() -> Self {
        Self {
            ink: Ink::Accent,
            pattern: Pattern::Solid,
            width: 2.0,
            fill_opacity: 0.08,
            font_size: 14.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Shape {
    Horizontal(Anchor),
    Vertical(Anchor),
    Trend(Anchor, Anchor),
    Ray(Anchor, Anchor),
    Rectangle(Anchor, Anchor),
    Fibonacci {
        a: Anchor,
        b: Anchor,
        levels: Vec<f64>,
        reverse: bool,
        extend_right: bool,
    },
    Text {
        at: Anchor,
        text: String,
    },
}

impl Shape {
    pub fn tool(&self) -> Tool {
        match self {
            Self::Horizontal(_) => Tool::Horizontal,
            Self::Vertical(_) => Tool::Vertical,
            Self::Trend(_, _) => Tool::Trend,
            Self::Ray(_, _) => Tool::Ray,
            Self::Rectangle(_, _) => Tool::Rectangle,
            Self::Fibonacci { .. } => Tool::Fibonacci,
            Self::Text { .. } => Tool::Text,
        }
    }
    pub fn anchors(&self) -> Anchors {
        match self {
            Self::Horizontal(a) | Self::Vertical(a) | Self::Text { at: a, .. } => Anchors::One(*a),
            Self::Trend(a, b)
            | Self::Ray(a, b)
            | Self::Rectangle(a, b)
            | Self::Fibonacci { a, b, .. } => Anchors::Two(*a, *b),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Drawing {
    pub id: DrawingId,
    pub shape: Shape,
    pub style: Style,
    pub visible: bool,
    pub locked: bool,
}

#[derive(Debug, Clone)]
pub enum Command {
    Create { shape: Shape, style: Style },
    Replace(Drawing),
    Move { id: DrawingId, anchors: Anchors },
    Delete(DrawingId),
    SetVisible { id: DrawingId, visible: bool },
    SetLocked { id: DrawingId, locked: bool },
    Undo,
    Redo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    Body,
    First,
    Second,
    Corner(u8),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hit {
    pub id: DrawingId,
    pub part: Part,
}

pub enum Preview<'a> {
    Create {
        tool: Tool,
        anchors: Anchors,
    },
    Edit {
        drawing: &'a Drawing,
        anchors: Anchors,
    },
}
