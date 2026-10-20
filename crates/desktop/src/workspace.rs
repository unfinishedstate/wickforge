//! Desktop-only durable configuration; native histories and interaction state never enter this schema.
use crate::drawing_types::{Anchor, Anchors, Drawing, InstrumentKey, Shape, Style, Tool};
use crate::drawings::{validate_shape_parts, validate_style};
use crate::state::{
    MAX_VISIBLE_BARS, MIN_VISIBLE_BARS, PaneScale, Viewport, bounded_plot_height,
    default_plot_height,
};
use mexc_indicators::profiles::{
    Display as ProfileDisplay, Instance as ProfileInstance, Kind as ProfileKind,
    Period as ProfilePeriod, Range as ProfileRange, Spec as ProfileSpec,
};
use mexc_indicators::{CvdReset, IndicatorSource, IndicatorSpec, SourceMode};
use mexc_model::executions::NativeInstrument;
use mexc_model::{Exchange, Market, Product, Timeframe};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub const SCHEMA_VERSION: u32 = 4;
pub const RECENT_MARKETS_LIMIT: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResetKind {
    Current,
    All,
}

// These representations belong to the desktop, not to the feed/calculation model.
macro_rules! saved_enum {
    ($saved:ident, $native:path, {$($variant:ident),+ $(,)?}) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum $saved { $($variant),+ }
        impl From<$native> for $saved {
            fn from(value: $native) -> Self {
                match value { $(<$native>::$variant => Self::$variant),+ }
            }
        }
        impl From<$saved> for $native {
            fn from(value: $saved) -> Self {
                match value { $($saved::$variant => Self::$variant),+ }
            }
        }
    };
}
saved_enum!(SavedExchange, Exchange, {Binance, Okx, Coinbase, Mexc});
saved_enum!(SavedProduct, Product, {Spot, Perpetual});
saved_enum!(SavedTimeframe, Timeframe, {Min1, Min5, Min15, Min30, Min60, Hour4, Hour6, Hour8, Day1});
saved_enum!(SavedCvdReset, CvdReset, {Continuous, UtcDay});
saved_enum!(SavedSourceMode, SourceMode, {Chart, Single, Selected, All});
saved_enum!(SavedInk, crate::drawing_types::Ink, {Accent, Foreground, Up, Down});
saved_enum!(SavedPattern, crate::drawing_types::Pattern, {Solid, Dashed, Dotted});
saved_enum!(SavedProfileKind, ProfileKind, {Tpo, Volume});

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum SavedProfilePeriod {
    Day,
    Week,
    Month,
    Custom {
        timezone: String,
        start: String,
        end: String,
    },
}
impl From<ProfilePeriod> for SavedProfilePeriod {
    fn from(value: ProfilePeriod) -> Self {
        match value {
            ProfilePeriod::Day => Self::Day,
            ProfilePeriod::Week => Self::Week,
            ProfilePeriod::Month => Self::Month,
            ProfilePeriod::Custom {
                timezone,
                start_minute,
                end_minute,
            } => Self::Custom {
                timezone: timezone.name().into(),
                start: format!("{:02}:{:02}", start_minute / 60, start_minute % 60),
                end: format!("{:02}:{:02}", end_minute / 60, end_minute % 60),
            },
        }
    }
}
impl SavedProfilePeriod {
    fn to_period(&self) -> Result<ProfilePeriod, String> {
        Ok(match self {
            Self::Day => ProfilePeriod::Day,
            Self::Week => ProfilePeriod::Week,
            Self::Month => ProfilePeriod::Month,
            Self::Custom {
                timezone,
                start,
                end,
            } => ProfilePeriod::custom(timezone, start, end)?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum SavedProfileRange {
    Session(SavedProfilePeriod),
    Fixed { start_ms: i64, end_ms: i64 },
    Visible,
}
impl From<&ProfileRange> for SavedProfileRange {
    fn from(value: &ProfileRange) -> Self {
        match *value {
            ProfileRange::Session(period) => Self::Session(period.into()),
            ProfileRange::Fixed { start_ms, end_ms } => Self::Fixed { start_ms, end_ms },
            ProfileRange::Visible => Self::Visible,
        }
    }
}
impl SavedProfileRange {
    fn to_range(&self) -> Result<ProfileRange, String> {
        Ok(match self {
            Self::Session(period) => ProfileRange::Session(period.to_period()?),
            Self::Fixed { start_ms, end_ms } => ProfileRange::Fixed {
                start_ms: *start_ms,
                end_ms: *end_ms,
            },
            Self::Visible => ProfileRange::Visible,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedProfileDisplay {
    pub histogram: bool,
    pub levels: bool,
    pub previous: bool,
    pub naked: bool,
    pub single_prints: bool,
    pub initial_balance: bool,
    pub letters: bool,
    pub poor_extremes: bool,
    pub width_percent: u8,
}
impl From<&ProfileDisplay> for SavedProfileDisplay {
    fn from(value: &ProfileDisplay) -> Self {
        Self {
            histogram: value.histogram,
            levels: value.levels,
            previous: value.previous,
            naked: value.naked,
            single_prints: value.single_prints,
            initial_balance: value.initial_balance,
            letters: value.letters,
            poor_extremes: value.poor_extremes,
            width_percent: value.width_percent,
        }
    }
}
impl SavedProfileDisplay {
    fn to_display(&self) -> ProfileDisplay {
        ProfileDisplay {
            histogram: self.histogram,
            levels: self.levels,
            previous: self.previous,
            naked: self.naked,
            single_prints: self.single_prints,
            initial_balance: self.initial_balance,
            letters: self.letters,
            poor_extremes: self.poor_extremes,
            width_percent: self.width_percent,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedProfileSpec {
    pub kind: SavedProfileKind,
    pub range: SavedProfileRange,
    pub block_minutes: u32,
    pub ticks_per_row: u32,
    pub value_area_percent: u8,
    #[serde(default = "default_profile_estimation")]
    pub estimate_missing: bool,
    pub display: SavedProfileDisplay,
}
fn default_profile_estimation() -> bool {
    true
}

impl From<&ProfileSpec> for SavedProfileSpec {
    fn from(value: &ProfileSpec) -> Self {
        Self {
            kind: value.kind.into(),
            range: (&value.range).into(),
            block_minutes: value.block_minutes,
            ticks_per_row: value.ticks_per_row,
            value_area_percent: value.value_area_percent,
            estimate_missing: value.estimate_missing,
            display: (&value.display).into(),
        }
    }
}
impl SavedProfileSpec {
    pub fn to_spec(&self) -> Result<ProfileSpec, String> {
        Ok(ProfileSpec {
            kind: self.kind.into(),
            range: self.range.to_range()?,
            block_minutes: self.block_minutes,
            ticks_per_row: self.ticks_per_row,
            value_area_percent: self.value_area_percent,
            estimate_missing: self.estimate_missing,
            display: self.display.to_display(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedProfile {
    pub id: u64,
    pub spec: SavedProfileSpec,
    pub visible: bool,
    pub instrument: Option<SavedInstrument>,
}
impl From<&ProfileInstance> for SavedProfile {
    fn from(value: &ProfileInstance) -> Self {
        Self {
            id: value.id,
            spec: (&value.spec).into(),
            visible: value.visible,
            instrument: value.instrument.as_ref().map(SavedInstrument::from),
        }
    }
}
impl SavedProfile {
    pub fn to_instance(&self) -> Result<ProfileInstance, String> {
        validate_id(self.id)?;
        let spec = self.spec.to_spec()?;
        spec.validate()?;
        if matches!(spec.range, ProfileRange::Fixed { .. }) != self.instrument.is_some() {
            return Err("Fixed profiles require an exact native instrument; session and visible profiles follow the chart".into());
        }
        Ok(ProfileInstance {
            id: self.id,
            spec,
            visible: self.visible,
            instrument: self
                .instrument
                .as_ref()
                .map(SavedInstrument::to_native)
                .transpose()?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedMarket {
    pub exchange: SavedExchange,
    pub product: SavedProduct,
    pub symbol: String,
    pub interval: SavedTimeframe,
}
impl From<&Market> for SavedMarket {
    fn from(value: &Market) -> Self {
        Self {
            exchange: value.exchange.into(),
            product: value.product.into(),
            symbol: value.symbol.clone(),
            interval: value.interval.into(),
        }
    }
}
pub(crate) fn validate_symbol(symbol: &str) -> Result<(), String> {
    if symbol.is_empty() || symbol.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err("Native instrument symbol must be nonempty and contain no whitespace or control characters".into());
    }
    Ok(())
}
impl SavedMarket {
    pub fn validate(&self) -> Result<(), String> {
        validate_symbol(&self.symbol)
    }

    pub fn to_market(&self) -> Result<Market, String> {
        self.validate()?;
        Ok(Market {
            exchange: self.exchange.into(),
            product: self.product.into(),
            symbol: self.symbol.clone(),
            interval: self.interval.into(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum SavedSpec {
    Sma {
        period: usize,
    },
    Ema {
        period: usize,
    },
    Bollinger {
        period: usize,
        deviations: f64,
    },
    Rsi {
        period: usize,
    },
    Macd {
        fast: usize,
        slow: usize,
        signal: usize,
    },
    Atr {
        period: usize,
    },
    Stochastic {
        period: usize,
        smooth_k: usize,
        smooth_d: usize,
    },
    Cvd {
        reset: SavedCvdReset,
    },
    OpenInterest,
    Vwap {
        reset: SavedProfilePeriod,
    },
}
impl From<IndicatorSpec> for SavedSpec {
    fn from(value: IndicatorSpec) -> Self {
        match value {
            IndicatorSpec::Sma { period } => Self::Sma { period },
            IndicatorSpec::Ema { period } => Self::Ema { period },
            IndicatorSpec::Bollinger { period, deviations } => {
                Self::Bollinger { period, deviations }
            }
            IndicatorSpec::Rsi { period } => Self::Rsi { period },
            IndicatorSpec::Macd { fast, slow, signal } => Self::Macd { fast, slow, signal },
            IndicatorSpec::Atr { period } => Self::Atr { period },
            IndicatorSpec::Stochastic {
                period,
                smooth_k,
                smooth_d,
            } => Self::Stochastic {
                period,
                smooth_k,
                smooth_d,
            },
            IndicatorSpec::Cvd { reset } => Self::Cvd {
                reset: reset.into(),
            },
            IndicatorSpec::OpenInterest => Self::OpenInterest,
            IndicatorSpec::Vwap { reset } => Self::Vwap {
                reset: reset.into(),
            },
        }
    }
}
impl SavedSpec {
    pub fn to_spec(&self) -> Result<IndicatorSpec, String> {
        Ok(match *self {
            Self::Sma { period } => IndicatorSpec::Sma { period },
            Self::Ema { period } => IndicatorSpec::Ema { period },
            Self::Bollinger { period, deviations } => {
                IndicatorSpec::Bollinger { period, deviations }
            }
            Self::Rsi { period } => IndicatorSpec::Rsi { period },
            Self::Macd { fast, slow, signal } => IndicatorSpec::Macd { fast, slow, signal },
            Self::Atr { period } => IndicatorSpec::Atr { period },
            Self::Stochastic {
                period,
                smooth_k,
                smooth_d,
            } => IndicatorSpec::Stochastic {
                period,
                smooth_k,
                smooth_d,
            },
            Self::Cvd { reset } => IndicatorSpec::Cvd {
                reset: reset.into(),
            },
            Self::OpenInterest => IndicatorSpec::OpenInterest,
            Self::Vwap { ref reset } => IndicatorSpec::Vwap {
                reset: reset.to_period()?,
            },
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedSource {
    pub mode: SavedSourceMode,
    pub markets: Vec<SavedMarket>,
}
impl From<&IndicatorSource> for SavedSource {
    fn from(value: &IndicatorSource) -> Self {
        Self {
            mode: value.mode.into(),
            markets: value.markets.iter().map(SavedMarket::from).collect(),
        }
    }
}
impl SavedSource {
    pub fn to_source(&self, spec: IndicatorSpec) -> Result<IndicatorSource, String> {
        let source = IndicatorSource {
            mode: self.mode.into(),
            markets: self
                .markets
                .iter()
                .map(SavedMarket::to_market)
                .collect::<Result<_, _>>()?,
        };
        mexc_indicators::validate_source(&source, spec)?;
        Ok(source)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedIndicator {
    pub id: u64,
    pub spec: SavedSpec,
    pub source: SavedSource,
    pub visible: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedScale {
    pub auto_fit: bool,
    pub low: Option<f64>,
    pub high: Option<f64>,
}
impl SavedScale {
    pub fn from_bounds(auto_fit: bool, low: f64, high: f64) -> Self {
        Self {
            auto_fit,
            low: (!auto_fit).then_some(low),
            high: (!auto_fit).then_some(high),
        }
    }
    pub fn validate(&self) -> Result<(), String> {
        match (self.auto_fit, self.low, self.high) {
            (true, None, None) => Ok(()),
            (false, Some(low), Some(high)) if low.is_finite() && high.is_finite()
                && low.abs() <= 1e100 && high.abs() <= 1e100 && high > low
                && (1e-100..=1e100).contains(&(high - low)) => Ok(()),
            _ => Err("Scale must be auto-fit without bounds or have finite ordered manual bounds within ±1e100".into()),
        }
    }
    pub fn to_pane_scale(&self) -> PaneScale {
        match (self.low, self.high) {
            (Some(low), Some(high)) if !self.auto_fit => PaneScale {
                low,
                high,
                auto_fit: false,
            },
            _ => PaneScale::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedView {
    pub left: Option<f64>,
    pub span: f64,
    pub live: bool,
    pub price_scale: SavedScale,
}
impl SavedView {
    pub fn validate(&self, interval: Timeframe) -> Result<(), String> {
        self.price_scale.validate()?;
        let step = interval.seconds() as f64;
        if !self.span.is_finite()
            || !(MIN_VISIBLE_BARS * step..=MAX_VISIBLE_BARS * step).contains(&self.span)
        {
            return Err(
                "Time span must be finite and between 10 and 10000 native intervals".into(),
            );
        }
        match (self.live, self.left) {
            (true, None) => Ok(()),
            (false, Some(left)) if left.is_finite()
                && chrono::DateTime::from_timestamp(left.floor() as i64, 0).is_some()
                && chrono::DateTime::from_timestamp((left + self.span).ceil() as i64, 0).is_some()
                && left + self.span != left => Ok(()),
            _ => Err("Live views cannot save a left boundary; historical views need a finite supported UTC boundary".into()),
        }
    }
}
impl From<Viewport> for SavedView {
    fn from(value: Viewport) -> Self {
        Self {
            left: (!value.live).then_some(value.left),
            span: value.span,
            live: value.live,
            price_scale: SavedScale::from_bounds(value.auto_fit, value.low, value.high),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedPane {
    pub id: u64,
    pub height: f32,
    pub collapsed: bool,
    pub scale: SavedScale,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedChart {
    pub market: SavedMarket,
    pub indicators: Vec<SavedIndicator>,
    pub profiles: Vec<SavedProfile>,
    pub view: SavedView,
    pub price_height: f32,
    pub panes: Vec<SavedPane>,
    pub active_pane: Option<u64>,
}
impl SavedChart {
    pub fn empty(market: &Market) -> Self {
        Self::empty_market(market.into())
    }
    fn empty_market(market: SavedMarket) -> Self {
        let view = Viewport::for_timeframe(market.interval.into()).into();
        Self {
            market,
            indicators: Vec::new(),
            profiles: Vec::new(),
            view,
            price_height: default_plot_height(None),
            panes: Vec::new(),
            active_pane: None,
        }
    }
    pub fn validate(&self) -> Result<(), String> {
        self.market.validate()?;
        self.view.validate(self.market.interval.into())?;
        if bounded_plot_height(None, self.price_height) != Some(self.price_height) {
            return Err("Price pane height is outside 160..1000".into());
        }
        let mut ids = HashSet::new();
        let mut lower = HashSet::new();
        for indicator in &self.indicators {
            validate_id(indicator.id)?;
            if !ids.insert(indicator.id) {
                return Err("Duplicate indicator ID".into());
            }
            let spec = indicator.spec.to_spec()?;
            spec.validate()?;
            indicator.source.to_source(spec)?;
            if !spec.is_overlay() {
                lower.insert(indicator.id);
            }
        }
        let mut profile_ids = HashSet::new();
        for profile in &self.profiles {
            profile.to_instance()?;
            if !profile_ids.insert(profile.id) {
                return Err("Duplicate profile ID".into());
            }
        }
        let mut panes = HashSet::new();
        for pane in &self.panes {
            if !lower.contains(&pane.id) || !panes.insert(pane.id) {
                return Err("Duplicate pane or invalid indicator pane reference".into());
            }
            if bounded_plot_height(Some(pane.id), pane.height) != Some(pane.height) {
                return Err("Indicator pane height is outside 100..800".into());
            }
            pane.scale.validate()?;
        }
        if panes != lower {
            return Err("Every lower indicator requires exactly one saved pane".into());
        }
        if self.active_pane.is_some_and(|id| !lower.contains(&id)) {
            return Err("Active pane does not reference a lower indicator".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedInstrument {
    pub exchange: SavedExchange,
    pub product: SavedProduct,
    pub symbol: String,
}
impl From<&InstrumentKey> for SavedInstrument {
    fn from(value: &InstrumentKey) -> Self {
        Self {
            exchange: value.exchange.into(),
            product: value.product.into(),
            symbol: value.symbol.clone(),
        }
    }
}
impl From<&NativeInstrument> for SavedInstrument {
    fn from(value: &NativeInstrument) -> Self {
        Self {
            exchange: value.exchange.into(),
            product: value.product.into(),
            symbol: value.symbol.clone(),
        }
    }
}
impl SavedInstrument {
    pub fn to_native(&self) -> Result<NativeInstrument, String> {
        validate_symbol(&self.symbol)?;
        Ok(NativeInstrument {
            exchange: self.exchange.into(),
            product: self.product.into(),
            symbol: self.symbol.clone(),
        })
    }
    pub fn to_key(&self) -> Result<InstrumentKey, String> {
        validate_symbol(&self.symbol)?;
        Ok(InstrumentKey {
            exchange: self.exchange.into(),
            product: self.product.into(),
            symbol: self.symbol.clone(),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedAnchor {
    pub utc_ms: i64,
    pub price: f64,
}
impl From<Anchor> for SavedAnchor {
    fn from(value: Anchor) -> Self {
        Self {
            utc_ms: value.utc_ms,
            price: value.price,
        }
    }
}
impl From<SavedAnchor> for Anchor {
    fn from(value: SavedAnchor) -> Self {
        Self {
            utc_ms: value.utc_ms,
            price: value.price,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum SavedShape {
    Horizontal {
        at: SavedAnchor,
    },
    Vertical {
        at: SavedAnchor,
    },
    Trend {
        a: SavedAnchor,
        b: SavedAnchor,
    },
    Ray {
        a: SavedAnchor,
        b: SavedAnchor,
    },
    Rectangle {
        a: SavedAnchor,
        b: SavedAnchor,
    },
    Measure {
        a: SavedAnchor,
        b: SavedAnchor,
    },
    Fibonacci {
        a: SavedAnchor,
        b: SavedAnchor,
        levels: Vec<f64>,
        reverse: bool,
        extend_right: bool,
    },
    Text {
        at: SavedAnchor,
        text: String,
    },
    AnchoredVwap {
        start_ms: i64,
    },
}
impl From<&Shape> for SavedShape {
    fn from(value: &Shape) -> Self {
        match value {
            Shape::Horizontal(at) => Self::Horizontal { at: (*at).into() },
            Shape::Vertical(at) => Self::Vertical { at: (*at).into() },
            Shape::AnchoredVwap(at) => Self::AnchoredVwap {
                start_ms: at.utc_ms,
            },
            Shape::Trend(a, b) => Self::Trend {
                a: (*a).into(),
                b: (*b).into(),
            },
            Shape::Ray(a, b) => Self::Ray {
                a: (*a).into(),
                b: (*b).into(),
            },
            Shape::Rectangle(a, b) => Self::Rectangle {
                a: (*a).into(),
                b: (*b).into(),
            },
            Shape::Measure(a, b) => Self::Measure {
                a: (*a).into(),
                b: (*b).into(),
            },
            Shape::Fibonacci {
                a,
                b,
                levels,
                reverse,
                extend_right,
            } => Self::Fibonacci {
                a: (*a).into(),
                b: (*b).into(),
                levels: levels.clone(),
                reverse: *reverse,
                extend_right: *extend_right,
            },
            Shape::Text { at, text } => Self::Text {
                at: (*at).into(),
                text: text.clone(),
            },
        }
    }
}
impl SavedShape {
    pub fn validate(&self) -> Result<(), String> {
        let (tool, anchors, text, fibonacci) = match self {
            Self::Horizontal { at } => (Tool::Horizontal, Anchors::One((*at).into()), None, None),
            Self::Vertical { at } => (Tool::Vertical, Anchors::One((*at).into()), None, None),
            Self::AnchoredVwap { start_ms } => (
                Tool::AnchoredVwap,
                Anchors::One(Anchor {
                    utc_ms: *start_ms,
                    price: 0.0,
                }),
                None,
                None,
            ),
            Self::Trend { a, b } => (
                Tool::Trend,
                Anchors::Two((*a).into(), (*b).into()),
                None,
                None,
            ),
            Self::Ray { a, b } => (
                Tool::Ray,
                Anchors::Two((*a).into(), (*b).into()),
                None,
                None,
            ),
            Self::Rectangle { a, b } => (
                Tool::Rectangle,
                Anchors::Two((*a).into(), (*b).into()),
                None,
                None,
            ),
            Self::Measure { a, b } => (
                Tool::Measure,
                Anchors::Two((*a).into(), (*b).into()),
                None,
                None,
            ),
            Self::Fibonacci {
                a,
                b,
                levels,
                reverse,
                ..
            } => (
                Tool::Fibonacci,
                Anchors::Two((*a).into(), (*b).into()),
                None,
                Some((levels.as_slice(), *reverse)),
            ),
            Self::Text { at, text } => (
                Tool::Text,
                Anchors::One((*at).into()),
                Some(text.as_str()),
                None,
            ),
        };
        validate_shape_parts(tool, anchors, text, fibonacci)
    }

    pub fn to_shape(&self) -> Shape {
        match self {
            Self::Horizontal { at } => Shape::Horizontal((*at).into()),
            Self::Vertical { at } => Shape::Vertical((*at).into()),
            Self::AnchoredVwap { start_ms } => Shape::AnchoredVwap(Anchor {
                utc_ms: *start_ms,
                price: 0.0,
            }),
            Self::Trend { a, b } => Shape::Trend((*a).into(), (*b).into()),
            Self::Ray { a, b } => Shape::Ray((*a).into(), (*b).into()),
            Self::Rectangle { a, b } => Shape::Rectangle((*a).into(), (*b).into()),
            Self::Measure { a, b } => Shape::Measure((*a).into(), (*b).into()),
            Self::Fibonacci {
                a,
                b,
                levels,
                reverse,
                extend_right,
            } => Shape::Fibonacci {
                a: (*a).into(),
                b: (*b).into(),
                levels: levels.clone(),
                reverse: *reverse,
                extend_right: *extend_right,
            },
            Self::Text { at, text } => Shape::Text {
                at: (*at).into(),
                text: text.clone(),
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedStyle {
    pub ink: SavedInk,
    pub pattern: SavedPattern,
    pub width: f32,
    pub fill_opacity: f32,
    pub font_size: f32,
}
impl From<Style> for SavedStyle {
    fn from(value: Style) -> Self {
        Self {
            ink: value.ink.into(),
            pattern: value.pattern.into(),
            width: value.width,
            fill_opacity: value.fill_opacity,
            font_size: value.font_size,
        }
    }
}
impl From<SavedStyle> for Style {
    fn from(value: SavedStyle) -> Self {
        Self {
            ink: value.ink.into(),
            pattern: value.pattern.into(),
            width: value.width,
            fill_opacity: value.fill_opacity,
            font_size: value.font_size,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedDrawing {
    pub id: u64,
    pub shape: SavedShape,
    pub style: SavedStyle,
    pub visible: bool,
    pub locked: bool,
}
impl From<&Drawing> for SavedDrawing {
    fn from(value: &Drawing) -> Self {
        Self {
            id: value.id,
            shape: (&value.shape).into(),
            style: value.style.into(),
            visible: value.visible,
            locked: value.locked,
        }
    }
}
impl SavedDrawing {
    pub fn validate(&self) -> Result<(), String> {
        validate_id(self.id)?;
        self.shape.validate()?;
        validate_style(&self.style.into())
    }

    pub fn to_drawing(&self) -> Result<Drawing, String> {
        self.validate()?;
        let shape = self.shape.to_shape();
        let style = self.style.into();
        Ok(Drawing {
            id: self.id,
            shape,
            style,
            visible: self.visible,
            locked: self.locked,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedBook {
    pub instrument: SavedInstrument,
    pub drawings: Vec<SavedDrawing>,
}

pub fn validate_id(id: u64) -> Result<(), String> {
    if id == 0 || id == u64::MAX {
        return Err("Stable IDs must be nonzero and leave room for the next ID".into());
    }
    Ok(())
}
pub fn validate_books(books: &[SavedBook]) -> Result<(), String> {
    let mut keys = HashSet::new();
    let mut ids = HashSet::new();
    for book in books {
        validate_symbol(&book.instrument.symbol)?;
        if !keys.insert(&book.instrument) {
            return Err("Duplicate instrument drawing book".into());
        }
        for drawing in &book.drawings {
            if !ids.insert(drawing.id) {
                return Err("Duplicate drawing ID across workspace".into());
            }
            drawing.validate()?;
        }
    }
    Ok(())
}
pub fn validate_quick_markets(
    recents: &[SavedMarket],
    favorites: &[SavedMarket],
) -> Result<(), String> {
    if recents.len() > RECENT_MARKETS_LIMIT {
        return Err("Recent market list exceeds its MRU limit".into());
    }
    for list in [recents, favorites] {
        let mut entries = HashSet::new();
        for market in list {
            market.validate()?;
            if !entries.insert(market) {
                return Err("Duplicate quick market entry".into());
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub schema_version: u32,
    pub reset_epoch: u64,
    pub chart: SavedChart,
    pub books: Vec<SavedBook>,
    pub recent_markets: Vec<SavedMarket>,
    pub favorite_markets: Vec<SavedMarket>,
    pub magnet: bool,
}
impl Default for Snapshot {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            reset_epoch: 0,
            chart: SavedChart::empty(&Market::default()),
            books: Vec::new(),
            recent_markets: Vec::new(),
            favorite_markets: Vec::new(),
            magnet: false,
        }
    }
}
impl Snapshot {
    /// Decode a known schema and validate the complete replacement before use.
    /// Schemas 1/2/3 predate profiles, estimation policy, and VWAP respectively.
    pub(crate) fn from_value(mut value: serde_json::Value) -> Result<Self, String> {
        let version = value
            .get("schema_version")
            .and_then(serde_json::Value::as_u64)
            .ok_or("schema_version must be an unsigned integer")?;
        match version {
            1 => {
                let chart = value
                    .get_mut("chart")
                    .and_then(serde_json::Value::as_object_mut)
                    .ok_or("Schema 1 chart must be an object")?;
                if chart.contains_key("profiles") {
                    return Err("Schema 1 chart cannot contain profile configuration".into());
                }
                chart.insert("profiles".into(), serde_json::json!([]));
                value["schema_version"] = serde_json::json!(SCHEMA_VERSION);
            }
            2 | 3 => {
                value["schema_version"] = serde_json::json!(SCHEMA_VERSION);
            }
            version if version == u64::from(SCHEMA_VERSION) => {}
            _ => return Err(format!("Unsupported workspace schema {version}")),
        }
        let snapshot: Self = serde_json::from_value(value).map_err(|e| e.to_string())?;
        snapshot.validate()?;
        Ok(snapshot)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(format!(
                "Unsupported workspace schema {} (expected {SCHEMA_VERSION})",
                self.schema_version
            ));
        }
        self.chart.validate()?;
        validate_books(&self.books)?;
        validate_quick_markets(&self.recent_markets, &self.favorite_markets)
    }
    pub fn cleared(&self, kind: ResetKind) -> Result<Self, String> {
        self.validate()?;
        let reset_epoch = self
            .reset_epoch
            .checked_add(1)
            .ok_or("Workspace reset epoch exhausted")?;
        let cleared = match kind {
            ResetKind::All => Self {
                reset_epoch,
                ..Self::default()
            },
            ResetKind::Current => Self {
                schema_version: SCHEMA_VERSION,
                reset_epoch,
                chart: {
                    let mut chart = SavedChart::empty_market(self.chart.market.clone());
                    chart.profiles = self
                        .chart
                        .profiles
                        .iter()
                        .filter(|profile| {
                            matches!(profile.spec.range, SavedProfileRange::Fixed { .. })
                                && profile.instrument.as_ref().is_some_and(|instrument| {
                                    instrument.exchange != self.chart.market.exchange
                                        || instrument.product != self.chart.market.product
                                        || instrument.symbol != self.chart.market.symbol
                                })
                        })
                        .cloned()
                        .collect();
                    chart
                },
                books: self
                    .books
                    .iter()
                    .filter(|book| {
                        book.instrument.exchange != self.chart.market.exchange
                            || book.instrument.product != self.chart.market.product
                            || book.instrument.symbol != self.chart.market.symbol
                    })
                    .cloned()
                    .collect(),
                recent_markets: self.recent_markets.clone(),
                favorite_markets: self.favorite_markets.clone(),
                magnet: self.magnet,
            },
        };
        Ok(cleared)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drawing_types::{Command, Ink, Pattern};
    use crate::drawings::Store;
    use crate::state::ChartState;

    fn populated() -> Snapshot {
        let market = Market {
            exchange: Exchange::Okx,
            product: Product::Spot,
            symbol: "BTC-USDC".into(),
            interval: Timeframe::Hour6,
        };
        let mut state = ChartState::default();
        state.select_market(market.clone(), 8);
        let specs = [
            IndicatorSpec::Sma { period: 21 },
            IndicatorSpec::Ema { period: 12 },
            IndicatorSpec::Bollinger {
                period: 20,
                deviations: 2.5,
            },
            IndicatorSpec::Rsi { period: 14 },
            IndicatorSpec::Macd {
                fast: 12,
                slow: 26,
                signal: 9,
            },
            IndicatorSpec::Atr { period: 7 },
            IndicatorSpec::Stochastic {
                period: 14,
                smooth_k: 3,
                smooth_d: 4,
            },
            IndicatorSpec::Cvd {
                reset: CvdReset::Continuous,
            },
            IndicatorSpec::Cvd {
                reset: CvdReset::UtcDay,
            },
            IndicatorSpec::OpenInterest,
            IndicatorSpec::OpenInterest,
        ];
        for (index, spec) in specs.into_iter().enumerate() {
            let mode = match index {
                7 => SourceMode::Single,
                8 => SourceMode::Selected,
                9 => SourceMode::All,
                _ => SourceMode::Chart,
            };
            let source = IndicatorSource {
                mode,
                markets: if mode == SourceMode::Chart {
                    Vec::new()
                } else {
                    vec![Market {
                        exchange: Exchange::Coinbase,
                        product: Product::Perpetual,
                        symbol: "unavailable-native-symbol".into(),
                        interval: Timeframe::Min1,
                    }]
                },
            };
            state
                .indicators
                .restore_instance(10 + index as u64 * 3, spec, source, index != 4)
                .unwrap();
        }
        for kind in [ProfileKind::Tpo, ProfileKind::Volume] {
            for range in [
                ProfileRange::Session(ProfilePeriod::Day),
                ProfileRange::Session(ProfilePeriod::Week),
                ProfileRange::Session(ProfilePeriod::Month),
                ProfileRange::Fixed {
                    start_ms: 1_704_067_200_000,
                    end_ms: 1_704_153_600_000,
                },
                ProfileRange::Visible,
            ] {
                let mut spec = ProfileSpec::default_for(kind);
                spec.range = range;
                spec.block_minutes = 15;
                spec.ticks_per_row = 7;
                spec.value_area_percent = 68;
                spec.estimate_missing =
                    kind == ProfileKind::Volume && matches!(range, ProfileRange::Visible);
                spec.display = ProfileDisplay {
                    histogram: kind == ProfileKind::Tpo,
                    levels: true,
                    previous: false,
                    naked: true,
                    single_prints: true,
                    initial_balance: true,
                    letters: true,
                    poor_extremes: false,
                    width_percent: 37,
                };
                let instrument = matches!(spec.range, ProfileRange::Fixed { .. })
                    .then(|| NativeInstrument::from_market(&market));
                state.profiles.push(ProfileInstance {
                    id: 1 + state.profiles.len() as u64 * 3,
                    spec,
                    visible: kind == ProfileKind::Volume,
                    instrument,
                });
            }
        }
        let mut store = Store::new(&market);
        let a = Anchor {
            utc_ms: 1_700_000_000_000,
            price: 10.0,
        };
        let b = Anchor {
            utc_ms: a.utc_ms + 60_000,
            price: 20.0,
        };
        let shapes = [
            Shape::Horizontal(a),
            Shape::Vertical(a),
            Shape::Trend(a, b),
            Shape::Ray(a, b),
            Shape::Rectangle(a, b),
            Shape::Measure(a, b),
            Shape::Fibonacci {
                a,
                b,
                levels: vec![-0.5, 0.0, 0.618, 1.0, 1.618],
                reverse: true,
                extend_right: false,
            },
            Shape::Text {
                at: a,
                text: "Native annotation\n第二行".into(),
            },
        ];
        for (index, shape) in shapes.into_iter().enumerate() {
            store
                .apply(Command::Create {
                    shape,
                    style: Style {
                        ink: Ink::ALL[index % 4],
                        pattern: Pattern::ALL[index % 3],
                        width: 3.5,
                        font_size: 18.0,
                        fill_opacity: 0.35,
                    },
                })
                .unwrap();
        }
        store
            .apply(Command::SetVisible {
                id: 1,
                visible: false,
            })
            .unwrap();
        store
            .apply(Command::SetLocked {
                id: 1,
                locked: true,
            })
            .unwrap();
        store.switch_context(&Market::default());
        store
            .apply(Command::Create {
                shape: Shape::Horizontal(b),
                style: Style::default(),
            })
            .unwrap();
        Snapshot {
            schema_version: SCHEMA_VERSION,
            reset_epoch: 7,
            chart: state.workspace_snapshot(),
            books: store.workspace_books(),
            recent_markets: vec![(&market).into(), (&Market::default()).into()],
            favorite_markets: vec![(&market).into()],
            magnet: true,
        }
    }

    #[test]
    fn older_profile_checkpoints_enable_fallback_without_rebinding_ranges() {
        let mut expected = populated();
        let mut encoded = serde_json::to_value(&expected).unwrap();
        encoded["schema_version"] = serde_json::json!(2);
        for profile in encoded["chart"]["profiles"].as_array_mut().unwrap() {
            profile["spec"]
                .as_object_mut()
                .unwrap()
                .remove("estimate_missing");
        }
        for profile in &mut expected.chart.profiles {
            profile.spec.estimate_missing = true;
        }
        let restored = Snapshot::from_value(encoded).unwrap();
        assert_eq!(restored, expected);
    }

    #[test]
    fn vwap_resets_and_native_bound_anchor_restore_without_derived_values() {
        let mut snapshot = populated();
        for (index, reset) in [
            ProfilePeriod::Day,
            ProfilePeriod::Week,
            ProfilePeriod::Month,
            ProfilePeriod::custom("America/New_York", "09:30", "16:00").unwrap(),
        ]
        .into_iter()
        .enumerate()
        {
            snapshot.chart.indicators.push(SavedIndicator {
                id: 100 + index as u64,
                spec: IndicatorSpec::Vwap { reset }.into(),
                source: (&IndicatorSource::default()).into(),
                visible: index % 2 == 0,
            });
        }
        let anchor_ms = 1_700_000_000_123;
        snapshot.books[0].drawings.push(SavedDrawing {
            id: 90,
            shape: SavedShape::AnchoredVwap {
                start_ms: anchor_ms,
            },
            style: Style::default().into(),
            visible: false,
            locked: true,
        });
        snapshot.validate().unwrap();
        let encoded = serde_json::to_value(&snapshot).unwrap();
        let restored = Snapshot::from_value(encoded.clone()).unwrap();
        let mut state = ChartState::default();
        state.restore_workspace(&restored.chart).unwrap();
        assert_eq!(state.workspace_snapshot(), snapshot.chart);
        assert!(
            state
                .indicators
                .instances
                .iter()
                .filter(|instance| matches!(instance.spec, IndicatorSpec::Vwap { .. }))
                .all(|instance| instance.points.is_empty() && instance.vwap_context(0).is_none())
        );
        let mut store = Store::new(&state.market);
        store
            .restore_workspace(&state.market, &restored.books)
            .unwrap();
        let drawing = store.find(90).unwrap();
        assert_eq!(
            drawing.shape,
            Shape::AnchoredVwap(Anchor {
                utc_ms: anchor_ms,
                price: 0.0
            })
        );
        assert!(!drawing.visible && drawing.locked);
        let mut invalid = encoded;
        invalid["chart"]["indicators"]
            .as_array_mut()
            .unwrap()
            .last_mut()
            .unwrap()["spec"]["vwap"]["reset"]["custom"]["timezone"] =
            serde_json::json!("Invalid/Zone");
        assert!(Snapshot::from_value(invalid).is_err());
    }

    #[test]
    fn every_configuration_and_drawing_round_trips_without_native_availability_substitution() {
        let snapshot = populated();
        snapshot.validate().unwrap();
        let encoded = serde_json::to_vec(&snapshot).unwrap();
        let decoded: Snapshot = serde_json::from_slice(&encoded).unwrap();
        decoded.validate().unwrap();
        assert_eq!(decoded, snapshot);
        let mut state = ChartState::default();
        state.restore_workspace(&decoded.chart).unwrap();
        assert_eq!(state.workspace_snapshot(), decoded.chart);
        assert!(
            state
                .indicators
                .instances
                .iter()
                .all(|instance| instance.points.is_empty() && instance.external.is_none())
        );
        let mut store = Store::new(&Market::default());
        store
            .restore_workspace(&state.market, &decoded.books)
            .unwrap();
        assert_eq!(store.workspace_books(), decoded.books);
        assert!(!store.objects()[0].visible);
        assert!(store.objects()[0].locked);
    }

    #[test]
    fn schema_one_migration_preserves_every_existing_field_and_rejects_invalid_legacy_data() {
        let mut expected = populated();
        expected.chart.profiles.clear();
        let mut legacy = serde_json::to_value(&expected).unwrap();
        legacy["schema_version"] = serde_json::json!(1);
        legacy["chart"].as_object_mut().unwrap().remove("profiles");
        assert_eq!(Snapshot::from_value(legacy.clone()).unwrap(), expected);

        let mut invalid = legacy.clone();
        invalid["chart"]["indicators"][1]["id"] = invalid["chart"]["indicators"][0]["id"].clone();
        assert!(Snapshot::from_value(invalid).is_err());
        let mut invalid = legacy.clone();
        invalid["books"][0]["unknown"] = serde_json::json!(true);
        assert!(Snapshot::from_value(invalid).is_err());
        let mut invalid = legacy;
        invalid["chart"]["profiles"] = serde_json::json!([]);
        assert!(Snapshot::from_value(invalid).is_err());

        let mut missing = serde_json::to_value(&expected).unwrap();
        missing["chart"].as_object_mut().unwrap().remove("profiles");
        assert!(Snapshot::from_value(missing).is_err());
    }

    #[test]
    fn profile_config_validation_is_atomic_and_does_not_admit_runtime_outputs() {
        let valid = populated();
        let rejects: &[fn(&mut Snapshot)] = &[
            |s| s.chart.profiles[0].id = 0,
            |s| s.chart.profiles[0].id = u64::MAX,
            |s| s.chart.profiles[1].id = s.chart.profiles[0].id,
            |s| s.chart.profiles[0].spec.block_minutes = 0,
            |s| s.chart.profiles[0].spec.ticks_per_row = 0,
            |s| s.chart.profiles[0].spec.value_area_percent = 0,
            |s| s.chart.profiles[0].spec.value_area_percent = 101,
            |s| s.chart.profiles[0].spec.display.width_percent = 0,
            |s| s.chart.profiles[0].spec.display.width_percent = 101,
            |s| s.chart.profiles[3].instrument = None,
            |s| s.chart.profiles[0].instrument = s.chart.profiles[3].instrument.clone(),
            |s| s.chart.profiles[4].instrument = s.chart.profiles[3].instrument.clone(),
            |s| {
                s.chart.profiles[3]
                    .instrument
                    .as_mut()
                    .unwrap()
                    .symbol
                    .clear()
            },
            |s| s.chart.profiles[3].instrument.as_mut().unwrap().symbol = "bad symbol".into(),
            |s| {
                s.chart.profiles[3].spec.range = SavedProfileRange::Fixed {
                    start_ms: 60_000,
                    end_ms: 60_000,
                }
            },
            |s| {
                s.chart.profiles[3].spec.range = SavedProfileRange::Fixed {
                    start_ms: 60_001,
                    end_ms: 120_000,
                }
            },
        ];
        let mut state = ChartState::default();
        state.restore_workspace(&valid.chart).unwrap();
        for (index, reject) in rejects.iter().enumerate() {
            let mut invalid = valid.clone();
            reject(&mut invalid);
            assert!(invalid.validate().is_err(), "Invalid profile case {index}");
            assert!(state.restore_workspace(&invalid.chart).is_err());
            assert_eq!(state.workspace_snapshot(), valid.chart);
        }

        let value = serde_json::to_value(&valid).unwrap();
        let profile = &value["chart"]["profiles"][0];
        let keys: HashSet<_> = profile
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, HashSet::from(["id", "spec", "visible", "instrument"]));
        for field in [
            "rows",
            "executions",
            "coverage",
            "snapshots",
            "engine",
            "poc",
            "tails",
        ] {
            assert!(profile.get(field).is_none());
            let mut invalid = value.clone();
            invalid["chart"]["profiles"][0][field] = serde_json::json!([]);
            assert!(
                Snapshot::from_value(invalid).is_err(),
                "Transient field {field} accepted"
            );
        }
    }

    #[test]
    fn current_clear_retains_only_other_exact_native_instrument_fixed_profiles() {
        let mut snapshot = populated();
        let template = snapshot.chart.profiles[3].clone();
        let mut retained = Vec::new();
        for index in 0..3 {
            let mut profile = template.clone();
            profile.id = 100 + index;
            let instrument = profile.instrument.as_mut().unwrap();
            match index {
                0 => instrument.exchange = SavedExchange::Binance,
                1 => instrument.product = SavedProduct::Perpetual,
                _ => instrument.symbol = instrument.symbol.to_lowercase(),
            }
            retained.push(profile.clone());
            snapshot.chart.profiles.push(profile);
        }
        let current = snapshot.cleared(ResetKind::Current).unwrap();
        assert_eq!(current.chart.profiles, retained);
        assert!(current.chart.indicators.is_empty());
        assert_eq!(current.recent_markets, snapshot.recent_markets);
        assert_eq!(current.favorite_markets, snapshot.favorite_markets);
        assert_eq!(current.books.len(), 1);
        assert_eq!(current.reset_epoch, snapshot.reset_epoch + 1);
        current.validate().unwrap();
        let mut restored = ChartState::default();
        restored.restore_workspace(&current.chart).unwrap();
        assert_eq!(restored.workspace_snapshot(), current.chart);

        let all = snapshot.cleared(ResetKind::All).unwrap();
        assert!(all.chart.profiles.is_empty());
        assert!(all.books.is_empty());
        assert!(all.recent_markets.is_empty());
        assert!(all.favorite_markets.is_empty());
        assert_eq!(all.reset_epoch, snapshot.reset_epoch + 1);
    }

    #[test]
    fn exact_native_market_enums_and_case_round_trip() {
        for exchange in Exchange::ALL {
            for product in Product::ALL {
                for interval in Timeframe::ALL {
                    let market = Market {
                        exchange,
                        product,
                        interval,
                        symbol: "Case-sensitive_native:Instrument".into(),
                    };
                    let saved = SavedMarket::from(&market);
                    let decoded: SavedMarket =
                        serde_json::from_str(&serde_json::to_string(&saved).unwrap()).unwrap();
                    assert_eq!(decoded.to_market().unwrap(), market);
                }
            }
        }
    }

    #[test]
    fn scopes_clear_all_current_objects_but_retain_other_books_and_exact_quick_order() {
        let snapshot = populated();
        let current = snapshot.cleared(ResetKind::Current).unwrap();
        current.validate().unwrap();
        assert_eq!(current.reset_epoch, snapshot.reset_epoch + 1);
        assert_eq!(current.chart.market, snapshot.chart.market);
        assert_eq!(
            current.chart,
            SavedChart::empty(&snapshot.chart.market.to_market().unwrap())
        );
        assert_eq!(current.recent_markets, snapshot.recent_markets);
        assert_eq!(current.favorite_markets, snapshot.favorite_markets);
        assert!(current.magnet);
        assert_eq!(current.books.len(), 1);
        assert_eq!(
            current.books[0].instrument,
            SavedInstrument::from(&InstrumentKey::from(&Market::default()))
        );
        assert_eq!(current.books[0].drawings.len(), 1);
        let all = snapshot.cleared(ResetKind::All).unwrap();
        assert_eq!(
            all,
            Snapshot {
                reset_epoch: snapshot.reset_epoch + 1,
                ..Snapshot::default()
            }
        );
        let mut exhausted = snapshot;
        exhausted.reset_epoch = u64::MAX;
        assert!(exhausted.cleared(ResetKind::Current).is_err());
        assert!(exhausted.cleared(ResetKind::All).is_err());
    }

    #[test]
    fn structural_invalidity_rejects_whole_snapshot() {
        let valid = populated();
        let rejects: &[fn(&mut Snapshot)] = &[
            |s| s.schema_version += 1,
            |s| s.chart.market.symbol.clear(),
            |s| s.chart.indicators[0].id = 0,
            |s| s.chart.indicators[0].id = u64::MAX,
            |s| s.chart.indicators[1].id = s.chart.indicators[0].id,
            |s| s.chart.indicators[0].spec = SavedSpec::Sma { period: 0 },
            |s| {
                s.chart.indicators[0].spec = SavedSpec::Macd {
                    fast: 20,
                    slow: 10,
                    signal: 9,
                }
            },
            |s| {
                s.chart.indicators[0].spec = SavedSpec::Bollinger {
                    period: 20,
                    deviations: f64::NAN,
                }
            },
            |s| s.chart.indicators[7].source.markets.clear(),
            |s| {
                let mut duplicate = s.chart.indicators[8].source.markets[0].clone();
                duplicate.interval = SavedTimeframe::Hour8;
                s.chart.indicators[8].source.markets.push(duplicate);
            },
            |s| s.chart.indicators[0].source = s.chart.indicators[7].source.clone(),
            |s| s.chart.panes[0].id = s.chart.indicators[0].id,
            |s| {
                s.chart.panes.pop();
            },
            |s| s.chart.panes.push(s.chart.panes[0].clone()),
            |s| s.chart.active_pane = Some(999),
            |s| s.chart.price_height = f32::NAN,
            |s| s.chart.panes[0].height = 900.0,
            |s| s.chart.view.left = Some(0.0),
            |s| s.chart.view.span = f64::INFINITY,
            |s| s.chart.view.span = 1.0,
            |s| {
                s.chart.view.price_scale = SavedScale {
                    auto_fit: false,
                    low: Some(2.0),
                    high: Some(1.0),
                }
            },
            |s| {
                s.chart.panes[0].scale = SavedScale {
                    auto_fit: true,
                    low: Some(0.0),
                    high: Some(1.0),
                }
            },
            |s| s.books.push(s.books[0].clone()),
            |s| s.books[0].drawings[0].id = 0,
            |s| s.books[0].drawings[0].id = u64::MAX,
            |s| s.books[1].drawings[0].id = s.books[0].drawings[0].id,
            |s| s.books[0].drawings[0].style.width = 100.0,
            |s| {
                s.books[0].drawings[0].shape = SavedShape::Horizontal {
                    at: SavedAnchor {
                        utc_ms: i64::MAX,
                        price: 10.0,
                    },
                }
            },
            |s| {
                s.books[0].drawings[0].shape = SavedShape::Horizontal {
                    at: SavedAnchor {
                        utc_ms: 0,
                        price: f64::NAN,
                    },
                }
            },
            |s| {
                s.books[0].drawings[0].shape = SavedShape::Rectangle {
                    a: SavedAnchor {
                        utc_ms: 0,
                        price: 10.0,
                    },
                    b: SavedAnchor {
                        utc_ms: 1,
                        price: 10.0,
                    },
                }
            },
            |s| {
                s.books[0].drawings[0].shape = SavedShape::Text {
                    at: SavedAnchor {
                        utc_ms: 0,
                        price: 10.0,
                    },
                    text: "  ".into(),
                }
            },
            |s| {
                s.books[0].drawings[0].shape = SavedShape::Fibonacci {
                    a: SavedAnchor {
                        utc_ms: 0,
                        price: 10.0,
                    },
                    b: SavedAnchor {
                        utc_ms: 1,
                        price: 20.0,
                    },
                    levels: vec![0.5, 0.5],
                    reverse: false,
                    extend_right: true,
                }
            },
            |s| s.recent_markets.push(s.recent_markets[0].clone()),
            |s| s.favorite_markets.push(s.favorite_markets[0].clone()),
        ];
        for (index, reject) in rejects.iter().enumerate() {
            let mut invalid = valid.clone();
            reject(&mut invalid);
            assert!(
                invalid.validate().is_err(),
                "Invalid case {index} was accepted"
            );
            assert!(invalid.cleared(ResetKind::Current).is_err());
        }
    }

    #[test]
    fn restored_indicator_ids_advance_and_exhaustion_never_wraps_or_creates_unserializable_id() {
        let mut set = mexc_indicators::IndicatorSet::default();
        set.restore_instance(
            41,
            IndicatorSpec::Sma { period: 2 },
            IndicatorSource::default(),
            false,
        )
        .unwrap();
        set.restore_instance(
            9,
            IndicatorSpec::Rsi { period: 3 },
            IndicatorSource::default(),
            true,
        )
        .unwrap();
        assert_eq!(
            set.add(IndicatorSpec::Ema { period: 4 }, &[], Timeframe::Min5)
                .unwrap(),
            42
        );
        assert!(
            set.restore_instance(
                41,
                IndicatorSpec::Sma { period: 2 },
                IndicatorSource::default(),
                true
            )
            .is_err()
        );
        set.restore_instance(
            u64::MAX - 1,
            IndicatorSpec::Atr { period: 4 },
            IndicatorSource::default(),
            true,
        )
        .unwrap();
        let len = set.instances.len();
        assert!(
            set.add(IndicatorSpec::Sma { period: 2 }, &[], Timeframe::Min5)
                .is_err()
        );
        assert_eq!(set.instances.len(), len);
        assert!(
            set.restore_instance(
                u64::MAX,
                IndicatorSpec::Sma { period: 2 },
                IndicatorSource::default(),
                true
            )
            .is_err()
        );
    }

    #[test]
    fn auto_fit_ticks_are_not_durable_and_native_time_zoom_bounds_are_accepted() {
        let mut state = ChartState::default();
        let before = state.workspace_snapshot();
        state.viewport.left = 123456.0;
        state.viewport.low = 100.0;
        state.viewport.high = 200.0;
        assert_eq!(state.workspace_snapshot(), before);
        for bars in [MIN_VISIBLE_BARS, MAX_VISIBLE_BARS] {
            state.viewport.span = bars * state.step();
            state.workspace_snapshot().validate().unwrap();
        }
        let mut historical = before;
        historical.view.live = false;
        historical.view.left = Some(f64::MAX);
        assert!(historical.validate().is_err());
    }
    #[test]
    fn custom_period_dtos_roundtrip_calendar_semantics_and_reject_invalid_fields() {
        for (zone, start, end) in [
            ("UTC", "00:00", "00:00"),
            ("America/New_York", "18:00", "09:30"),
            ("America/New_York", "01:15", "01:45"),
            ("America/New_York", "02:10", "02:40"),
        ] {
            let period = ProfilePeriod::custom(zone, start, end).unwrap();
            let saved = SavedProfilePeriod::from(period);
            let value = serde_json::to_value(&saved).unwrap();
            assert_eq!(
                value,
                serde_json::json!({"custom":{"timezone":zone,"start":start,"end":end}})
            );
            let restored: SavedProfilePeriod = serde_json::from_value(value).unwrap();
            assert_eq!(restored.to_period().unwrap(), period);
        }
        for (zone, start, end) in [
            ("Unknown/Zone", "09:30", "16:00"),
            ("UTC", "9:30", "16:00"),
            ("UTC", "09:30", "24:00"),
        ] {
            let saved = SavedProfilePeriod::Custom {
                timezone: zone.into(),
                start: start.into(),
                end: end.into(),
            };
            assert!(saved.to_period().is_err());
        }
        for custom in [
            serde_json::json!({"timezone":"UTC","start":"09:30"}),
            serde_json::json!({"timezone":"UTC","start":"09:30","end":"16:00","offset":0}),
            serde_json::json!({"timezone":"UTC","start":570,"end":"16:00"}),
        ] {
            assert!(
                serde_json::from_value::<SavedProfilePeriod>(serde_json::json!({"custom":custom}))
                    .is_err()
            );
        }
    }
}
