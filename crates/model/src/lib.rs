pub mod executions;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Exchange {
    Binance,
    Okx,
    Coinbase,
    #[default]
    Mexc,
}

impl Exchange {
    pub const ALL: [Self; 4] = [Self::Binance, Self::Okx, Self::Coinbase, Self::Mexc];

    pub const fn index(self) -> usize {
        match self {
            Self::Binance => 0,
            Self::Okx => 1,
            Self::Coinbase => 2,
            Self::Mexc => 3,
        }
    }
}

impl std::fmt::Display for Exchange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Binance => "Binance",
            Self::Okx => "OKX",
            Self::Coinbase => "Coinbase",
            Self::Mexc => "MEXC",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Product {
    Spot,
    #[default]
    Perpetual,
}

impl Product {
    pub const ALL: [Self; 2] = [Self::Spot, Self::Perpetual];

    pub const fn index(self) -> usize {
        match self {
            Self::Spot => 0,
            Self::Perpetual => 1,
        }
    }
}

impl std::fmt::Display for Product {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Spot => "Spot",
            Self::Perpetual => "Perpetual",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Quote {
    USD,
    USDT,
    #[default]
    USDC,
}

impl Quote {
    pub const ALL: [Self; 3] = [Self::USD, Self::USDT, Self::USDC];

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "USD" => Some(Self::USD),
            "USDT" => Some(Self::USDT),
            "USDC" => Some(Self::USDC),
            _ => None,
        }
    }
}

impl std::fmt::Display for Quote {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::USD => "USD",
            Self::USDT => "USDT",
            Self::USDC => "USDC",
        })
    }
}

/// Fixed-duration UTC intervals; each instrument declares its native subset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Timeframe {
    Min1,
    #[default]
    Min5,
    Min15,
    Min30,
    Min60,
    Hour4,
    Hour6,
    Hour8,
    Day1,
}

impl Timeframe {
    pub const ALL: [Self; 9] = [
        Self::Min1,
        Self::Min5,
        Self::Min15,
        Self::Min30,
        Self::Min60,
        Self::Hour4,
        Self::Hour6,
        Self::Hour8,
        Self::Day1,
    ];

    pub fn seconds(self) -> i64 {
        match self {
            Self::Min1 => 60,
            Self::Min5 => 300,
            Self::Min15 => 900,
            Self::Min30 => 1800,
            Self::Min60 => 3600,
            Self::Hour4 => 14400,
            Self::Hour6 => 21600,
            Self::Hour8 => 28800,
            Self::Day1 => 86400,
        }
    }

    pub fn query_code(self) -> &'static str {
        match self {
            Self::Min1 => "Min1",
            Self::Min5 => "Min5",
            Self::Min15 => "Min15",
            Self::Min30 => "Min30",
            Self::Min60 => "Min60",
            Self::Hour4 => "Hour4",
            Self::Hour8 => "Hour8",
            Self::Hour6 => "Hour6",
            Self::Day1 => "Day1",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Min1 => "1m",
            Self::Min5 => "5m",
            Self::Min15 => "15m",
            Self::Min30 => "30m",
            Self::Min60 => "1h",
            Self::Hour4 => "4h",
            Self::Hour6 => "6h",
            Self::Hour8 => "8h",
            Self::Day1 => "1d",
        }
    }
}

impl std::fmt::Display for Timeframe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Market {
    pub exchange: Exchange,
    pub product: Product,
    pub symbol: String,
    pub interval: Timeframe,
}

impl Default for Market {
    fn default() -> Self {
        Self {
            exchange: Exchange::Mexc,
            product: Product::Perpetual,
            symbol: "BTC_USDC".into(),
            interval: Timeframe::Min5,
        }
    }
}

/// Active, native exchange instrument; quote and settlement are distinct.
#[derive(Debug, Clone, PartialEq)]
pub struct Instrument {
    pub exchange: Exchange,
    pub product: Product,
    pub symbol: String,
    pub base_coin: String,
    pub quote_coin: Quote,
    pub settle_coin: String,
    pub price_scale: u32,
    /// Authoritative native executable price increment; never inferred from display precision.
    pub tick_size: Option<rust_decimal::Decimal>,
    pub contract_size: f64,
    pub inverse: bool,
    pub volume_unit: String,
}

/// One venue/product catalog, with a shared native interval list.
#[derive(Debug, Clone, PartialEq)]
pub struct Catalog {
    pub exchange: Exchange,
    pub product: Product,
    pub intervals: Vec<Timeframe>,
    pub instruments: Vec<Instrument>,
    pub flow_support: FlowSupport,
}
pub const PAGE_SIZE: usize = 1000;

/// Native volume-flow provenance. Reported means the venue's trade-side field
/// is not authoritatively established as aggressor side; Missing is not zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlowQuality {
    Exact,
    Reported,
    Missing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlowAvailability {
    Exact,
    LimitedExact,
    Reported,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlowSupport {
    pub history: FlowAvailability,
    pub live: FlowAvailability,
    pub description: String,
}

/// Quantity semantics requested from the native data service. Price and candle
/// volume remain native in both modes; Base normalizes only flow and open interest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum QuantityMode {
    #[default]
    Native,
    Base,
}

/// Absolute signed-volume snapshot. Native operations use the instrument's
/// native volume unit; explicit Base operations use its catalog base asset.
/// Exact/Reported cover the bucket from its open through `through_ms`.
/// Missing has no delta/volume and explicitly revokes unproven coverage.
/// Any absent OHLC buckets since `covered_from` must be proven zero-trade.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VolumeFlow {
    pub delta: Option<f64>,
    pub volume: Option<f64>,
    pub quality: FlowQuality,
    pub covered_from: i64,
    pub through_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FlowBar {
    pub time: i64,
    pub flow: VolumeFlow,
}

/// One exchange candle, identified by its opening Unix timestamp (seconds, UTC).
/// Prices use the instrument's quote; volume uses its declared native unit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Candle {
    pub time: i64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
    /// Independent flow observation; None supplies no new flow information.
    pub flow: Option<VolumeFlow>,
}

#[derive(Debug, Clone)]
pub struct HistoryPage {
    pub market: Market,
    /// Ascending time order; `before` queries are exclusive.
    pub candles: Vec<Candle>,
    /// Cursor for an older page. None means the native request returned no candles.
    pub next_before: Option<i64>,
}

/// Optional CVD enrichment follows the native price page without blocking it.
#[derive(Debug, Clone)]
pub enum HistoryEvent {
    History { page: std::sync::Arc<HistoryPage> },
    Flow { bar: FlowBar },
    FlowStatus { message: String },
    Finished,
    Error { message: String },
}

/// Direct native market updates; callers retain their subscription's market.
#[derive(Debug, Clone)]
pub enum MarketEvent {
    Candle {
        candle: Candle,
    },
    Flow {
        bar: FlowBar,
    },
    FlowStatus {
        connected: bool,
        message: String,
    },
    Status {
        connected: bool,
        message: String,
    },
    /// A connection resumed or a consumer fell behind; reload recent history.
    Resync,
}

/// Distinguishes exchange timestamps from receipt time for untimestamped snapshots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenInterestTimestamp {
    Venue,
    Observed,
}

/// An actual OI observation, never candle-aligned or resampled. Quantity uses
/// the accompanying support unit (native or explicitly normalized base asset).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OpenInterestSample {
    pub timestamp_ms: i64,
    pub timestamp_source: OpenInterestTimestamp,
    pub quantity: f64,
    /// Declared observation cadence, used for coverage/freshness and rendering.
    pub interval_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenInterestSupport {
    pub unit: String,
    pub history_interval_ms: Option<i64>,
    pub live_interval_ms: Option<i64>,
    pub description: String,
}

#[derive(Debug, Clone)]
pub struct OpenInterestPage {
    pub market: Market,
    pub support: OpenInterestSupport,
    /// Ascending native observation timestamps; older queries are exclusive.
    pub samples: Vec<OpenInterestSample>,
    /// None indicates the source returned no older observations.
    pub next_before_ms: Option<i64>,
}

#[derive(Debug, Clone)]
pub enum OpenInterestEvent {
    Support(OpenInterestSupport),
    History(std::sync::Arc<OpenInterestPage>),
    Sample(OpenInterestSample),
    Status { connected: bool, message: String },
}
