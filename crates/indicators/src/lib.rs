//! Candle-aligned local indicators and native-data indicator configuration.
//!
//! Seeds are deliberately explicit: EMA uses an initial SMA, RSI uses `period`
//! changes, ATR includes the first bar's high-low, and MACD seeds its signal from
//! valid MACD values. These conventions do not promise vendor-specific parity.

use std::collections::VecDeque;
use std::fmt;

use mexc_model::{Candle, FlowQuality, Timeframe};

mod sources;
pub use sources::{
    ContributorStatus, ExternalIndicatorOutput, IndicatorSource, SourceData, SourceMode,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CvdReset {
    Continuous,
    UtcDay,
}

impl CvdReset {
    pub const ALL: [Self; 2] = [Self::Continuous, Self::UtcDay];

    pub fn token(self) -> &'static str {
        match self {
            Self::Continuous => "continuous",
            Self::UtcDay => "utc_day",
        }
    }
}

impl fmt::Display for CvdReset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Continuous => "Continuous",
            Self::UtcDay => "UTC day",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CvdContext {
    pub origin: i64,
    pub quality: FlowQuality,
    pub through_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndicatorKind {
    Sma,
    Ema,
    Bollinger,
    Rsi,
    Macd,
    Atr,
    Stochastic,
    Cvd,
    OpenInterest,
}

impl IndicatorKind {
    pub const ALL: [Self; 9] = [
        Self::Sma,
        Self::Ema,
        Self::Bollinger,
        Self::Rsi,
        Self::Macd,
        Self::Atr,
        Self::Stochastic,
        Self::Cvd,
        Self::OpenInterest,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Sma => "SMA",
            Self::Ema => "EMA",
            Self::Bollinger => "Bollinger Bands",
            Self::Rsi => "RSI",
            Self::Macd => "MACD",
            Self::Atr => "ATR",
            Self::Stochastic => "Stochastic",
            Self::Cvd => "CVD",
            Self::OpenInterest => "Open Interest",
        }
    }

    pub fn default_spec(self) -> IndicatorSpec {
        match self {
            Self::Sma => IndicatorSpec::Sma { period: 20 },
            Self::Ema => IndicatorSpec::Ema { period: 20 },
            Self::Bollinger => IndicatorSpec::Bollinger {
                period: 20,
                deviations: 2.0,
            },
            Self::Rsi => IndicatorSpec::Rsi { period: 14 },
            Self::Macd => IndicatorSpec::Macd {
                fast: 12,
                slow: 26,
                signal: 9,
            },
            Self::Atr => IndicatorSpec::Atr { period: 14 },
            Self::Stochastic => IndicatorSpec::Stochastic {
                period: 14,
                smooth_k: 3,
                smooth_d: 3,
            },
            Self::Cvd => IndicatorSpec::Cvd {
                reset: CvdReset::Continuous,
            },
            Self::OpenInterest => IndicatorSpec::OpenInterest,
        }
    }
}

impl fmt::Display for IndicatorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum IndicatorSpec {
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
        reset: CvdReset,
    },
    OpenInterest,
}

impl IndicatorSpec {
    pub fn kind(self) -> IndicatorKind {
        match self {
            Self::Sma { .. } => IndicatorKind::Sma,
            Self::Ema { .. } => IndicatorKind::Ema,
            Self::Bollinger { .. } => IndicatorKind::Bollinger,
            Self::Rsi { .. } => IndicatorKind::Rsi,
            Self::Macd { .. } => IndicatorKind::Macd,
            Self::Atr { .. } => IndicatorKind::Atr,
            Self::Stochastic { .. } => IndicatorKind::Stochastic,
            Self::Cvd { .. } => IndicatorKind::Cvd,
            Self::OpenInterest => IndicatorKind::OpenInterest,
        }
    }

    pub fn validate(self) -> Result<(), String> {
        fn period(name: &str, value: usize) -> Result<(), String> {
            if (1..=10_000).contains(&value) {
                Ok(())
            } else {
                Err(format!("{name} must be between 1 and 10000"))
            }
        }
        match self {
            Self::Sma { period: n }
            | Self::Ema { period: n }
            | Self::Rsi { period: n }
            | Self::Atr { period: n } => period("Period", n),
            Self::Bollinger {
                period: n,
                deviations,
            } => {
                period("Period", n)?;
                if !deviations.is_finite() || deviations <= 0.0 || deviations > 100.0 {
                    return Err("Deviations must be finite, greater than 0, and at most 100".into());
                }
                Ok(())
            }
            Self::Macd { fast, slow, signal } => {
                period("Fast period", fast)?;
                period("Slow period", slow)?;
                period("Signal period", signal)?;
                if fast >= slow {
                    return Err("Fast period must be less than slow period".into());
                }
                Ok(())
            }
            Self::Stochastic {
                period: n,
                smooth_k,
                smooth_d,
            } => {
                period("Period", n)?;
                period("K smoothing", smooth_k)?;
                period("D smoothing", smooth_d)
            }
            Self::Cvd { .. } | Self::OpenInterest => Ok(()),
        }
    }

    pub fn is_overlay(self) -> bool {
        matches!(
            self,
            Self::Sma { .. } | Self::Ema { .. } | Self::Bollinger { .. }
        )
    }

    pub fn label(self) -> String {
        match self {
            Self::Sma { period }
            | Self::Ema { period }
            | Self::Rsi { period }
            | Self::Atr { period } => format!("{}({period})", self.kind()),
            Self::Bollinger { period, deviations } => format!("BB({period}, {deviations})"),
            Self::Macd { fast, slow, signal } => format!("MACD({fast}, {slow}, {signal})"),
            Self::Stochastic {
                period,
                smooth_k,
                smooth_d,
            } => format!("Stochastic({period}, {smooth_k}, {smooth_d})"),
            Self::Cvd { reset } => format!("CVD ({reset})"),
            Self::OpenInterest => "Open Interest (native)".into(),
        }
    }

    pub fn series_names(self) -> &'static [&'static str] {
        match self {
            Self::Sma { .. } => &["SMA"],
            Self::Ema { .. } => &["EMA"],
            Self::Bollinger { .. } => &["Middle", "Upper", "Lower"],
            Self::Rsi { .. } => &["RSI"],
            Self::Macd { .. } => &["MACD", "Signal", "Histogram"],
            Self::Atr { .. } => &["ATR"],
            Self::Stochastic { .. } => &["%K", "%D"],
            Self::Cvd { .. } => &["CVD"],
            Self::OpenInterest => &["OI"],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IndicatorPoint {
    pub time: i64,
    pub values: [Option<f64>; 3],
}

// Only quantities not recoverable from public outputs need a per-bar checkpoint.
// Scratch extrema queues are retained, so replacing a forming bar never allocates.
#[derive(Debug)]
enum History {
    OutputsOnly,
    Rsi(Vec<[f64; 2]>),
    Macd(Vec<[Option<f64>; 2]>),
    Stochastic {
        raw: Vec<f64>,
        highs: VecDeque<usize>,
        lows: VecDeque<usize>,
    },
    Cvd(Vec<CvdCheckpoint>),
}

#[derive(Debug, Clone, Copy)]
struct CvdCheckpoint {
    sum: f64,
    compensation: f64,
    origin: i64,
    quality: FlowQuality,
    through_ms: i64,
    started: bool,
    valid: bool,
}

impl History {
    fn new(spec: IndicatorSpec) -> Self {
        match spec {
            IndicatorSpec::Rsi { .. } => Self::Rsi(Vec::new()),
            IndicatorSpec::Macd { .. } => Self::Macd(Vec::new()),
            IndicatorSpec::Stochastic { period, .. } => Self::Stochastic {
                raw: Vec::new(),
                highs: VecDeque::with_capacity(period),
                lows: VecDeque::with_capacity(period),
            },
            IndicatorSpec::Cvd { .. } => Self::Cvd(Vec::new()),
            _ => Self::OutputsOnly,
        }
    }
}

#[derive(Debug)]
pub struct IndicatorInstance {
    pub id: u64,
    pub spec: IndicatorSpec,
    pub visible: bool,
    pub points: Vec<IndicatorPoint>,
    pub source: IndicatorSource,
    pub external: Option<ExternalIndicatorOutput>,
    all_source_error: Option<String>,
    all_source_revision: u64,
    source_cache: sources::SourceCache,
    history: History,
}

#[derive(Debug, Default)]
pub struct IndicatorSet {
    pub instances: Vec<IndicatorInstance>,
    last_id: u64,
}

impl IndicatorSet {
    pub fn add(
        &mut self,
        spec: IndicatorSpec,
        candles: &[Candle],
        timeframe: Timeframe,
    ) -> Result<u64, String> {
        spec.validate()?;
        let id = self
            .last_id
            .checked_add(1)
            .ok_or("Indicator IDs exhausted")?;
        let mut instance = IndicatorInstance {
            id,
            spec,
            visible: true,
            points: Vec::new(),
            source: IndicatorSource::default(),
            external: None,
            all_source_error: None,
            all_source_revision: 0,
            source_cache: sources::SourceCache::default(),
            history: History::new(spec),
        };
        instance.recalculate(candles, 0, timeframe);
        self.instances.push(instance);
        self.last_id = id;
        Ok(id)
    }

    pub fn configure(
        &mut self,
        id: u64,
        spec: IndicatorSpec,
        candles: &[Candle],
        timeframe: Timeframe,
    ) -> Result<(), String> {
        spec.validate()?;
        let instance = self.instance_mut(id)?;
        instance.spec = spec;
        instance.history = History::new(spec);
        if !matches!(
            spec,
            IndicatorSpec::Cvd { .. } | IndicatorSpec::OpenInterest
        ) {
            instance.source = IndicatorSource::default();
        }
        instance.external = None;
        instance.source_cache = sources::SourceCache::default();
        instance.reset_all_source_validation();
        instance.points.clear();
        instance.recalculate(candles, 0, timeframe);
        Ok(())
    }

    pub fn toggle(&mut self, id: u64, _candles: &[Candle]) -> Result<(), String> {
        let instance = self.instance_mut(id)?;
        // Hidden instances stay current during every data update, avoiding a rebuild on show.
        instance.visible = !instance.visible;
        Ok(())
    }

    pub fn remove(&mut self, id: u64) -> bool {
        if let Some(index) = self.instances.iter().position(|instance| instance.id == id) {
            self.instances.remove(index);
            true
        } else {
            false
        }
    }

    /// Recompute from the earliest changed index. A historical prepend must pass zero.
    pub fn recalculate(&mut self, candles: &[Candle], from: usize, timeframe: Timeframe) {
        for instance in &mut self.instances {
            instance.recalculate(candles, from, timeframe);
        }
    }

    /// Recompute flow-only changes without recalculating price indicators.
    pub fn recalculate_flow(&mut self, candles: &[Candle], from: usize, timeframe: Timeframe) {
        for instance in &mut self.instances {
            if matches!(instance.spec, IndicatorSpec::Cvd { .. }) {
                instance.recalculate(candles, from, timeframe);
            }
        }
    }

    pub fn configure_source(&mut self, id: u64, source: IndicatorSource) -> Result<(), String> {
        let instance = self.instance_mut(id)?;
        source.validate(instance.spec)?;
        if instance.source != source {
            instance.source = source;
            instance.external = None;
            instance.source_cache = sources::SourceCache::default();
            instance.reset_all_source_validation();
            instance.points.clear();
            instance.history = History::new(instance.spec);
        }
        Ok(())
    }

    /// Accept or reject the complete native exchange universe for this All instance.
    /// The caller must revalidate after catalog or native chart context changes.
    pub fn set_all_source_error(&mut self, id: u64, error: Option<String>) -> Result<(), String> {
        let instance = self.instance_mut(id)?;
        let error = if instance.source.mode == SourceMode::All {
            error
        } else {
            None
        };
        if instance.all_source_error != error {
            instance.all_source_error = error;
            instance.all_source_revision = instance.all_source_revision.wrapping_add(1);
            if instance.all_source_error.is_some() {
                sources::invalidate_numeric(instance);
                if let Some(output) = &mut instance.external {
                    output.status.clear();
                    output
                        .status
                        .push_str(instance.all_source_error.as_deref().unwrap());
                }
            }
        }
        Ok(())
    }

    pub fn refresh_sources(
        &mut self,
        sources: &std::collections::HashMap<mexc_model::Market, SourceData>,
        chart: &mexc_model::Market,
        base_asset: &str,
        now_ms: i64,
    ) -> Vec<u64> {
        let mut changed = Vec::new();
        for instance in &mut self.instances {
            if !instance.source.is_chart()
                && sources::refresh(instance, sources, chart, base_asset, now_ms)
            {
                changed.push(instance.id);
            }
        }
        changed
    }

    fn instance_mut(&mut self, id: u64) -> Result<&mut IndicatorInstance, String> {
        self.instances
            .iter_mut()
            .find(|instance| instance.id == id)
            .ok_or_else(|| format!("Unknown indicator #{id}"))
    }
}

impl IndicatorInstance {
    pub fn all_source_error(&self) -> Option<&str> {
        (self.source.mode == SourceMode::All)
            .then_some(self.all_source_error.as_deref())
            .flatten()
    }

    fn reset_all_source_validation(&mut self) {
        self.all_source_error = (self.source.mode == SourceMode::All)
            .then(|| "Awaiting validated native exchange coverage for All sources".into());
        self.all_source_revision = self.all_source_revision.wrapping_add(1);
    }

    fn recalculate(&mut self, candles: &[Candle], from: usize, timeframe: Timeframe) {
        if !self.source.is_chart() {
            return;
        }
        // Native observations belong to ChartState, never to candle-aligned outputs.
        if self.spec == IndicatorSpec::OpenInterest {
            self.points.clear();
            return;
        }
        let from = from.min(self.points.len()).min(candles.len());
        self.points.resize(
            candles.len(),
            IndicatorPoint {
                time: 0,
                values: [None; 3],
            },
        );
        match (&self.spec, &mut self.history) {
            (IndicatorSpec::Sma { period }, _) => sma(candles, &mut self.points, from, *period),
            (IndicatorSpec::Ema { period }, _) => {
                for i in from..candles.len() {
                    let previous = i.checked_sub(1).and_then(|j| self.points[j].values[0]);
                    self.points[i] = point(
                        candles[i].time,
                        [close_ema(candles, i, *period, previous), None, None],
                    );
                }
            }
            (IndicatorSpec::Bollinger { period, deviations }, _) => {
                bollinger(candles, &mut self.points, from, *period, *deviations)
            }
            (IndicatorSpec::Rsi { period }, History::Rsi(averages)) => {
                rsi(candles, &mut self.points, averages, from, *period)
            }
            (IndicatorSpec::Macd { fast, slow, signal }, History::Macd(emas)) => {
                macd(candles, &mut self.points, emas, from, *fast, *slow, *signal)
            }
            (IndicatorSpec::Atr { period }, _) => atr(candles, &mut self.points, from, *period),
            (
                IndicatorSpec::Stochastic {
                    period,
                    smooth_k,
                    smooth_d,
                },
                History::Stochastic { raw, highs, lows },
            ) => stochastic(
                candles,
                &mut self.points,
                raw,
                highs,
                lows,
                from,
                [*period, *smooth_k, *smooth_d],
            ),
            (IndicatorSpec::Cvd { reset }, History::Cvd(checkpoints)) => cvd(
                candles,
                &mut self.points,
                checkpoints,
                from,
                timeframe,
                *reset,
            ),
            _ => unreachable!("Indicator configuration and its private history must agree"),
        }
    }

    pub fn cvd_context(&self, index: usize) -> Option<CvdContext> {
        if let Some(external) = &self.external {
            return external.contexts.get(index).copied().flatten();
        }
        let History::Cvd(checkpoints) = &self.history else {
            return None;
        };
        let checkpoint = checkpoints.get(index)?;
        checkpoint.valid.then_some(CvdContext {
            origin: checkpoint.origin,
            quality: checkpoint.quality,
            through_ms: checkpoint.through_ms,
        })
    }
}

fn cvd(
    candles: &[Candle],
    points: &mut [IndicatorPoint],
    checkpoints: &mut Vec<CvdCheckpoint>,
    from: usize,
    timeframe: Timeframe,
    reset: CvdReset,
) {
    checkpoints.resize(
        candles.len(),
        CvdCheckpoint {
            sum: 0.0,
            compensation: 0.0,
            origin: 0,
            quality: FlowQuality::Exact,
            through_ms: 0,
            started: false,
            valid: false,
        },
    );
    checkpoints.truncate(candles.len());
    let step = timeframe.seconds();
    for i in from..candles.len() {
        let bar = candles[i];
        let day_start = bar.time.div_euclid(86_400) * 86_400;
        let prev = i.checked_sub(1).map(|j| checkpoints[j]);
        let flow = bar.flow;
        let starts_day = reset == CvdReset::UtcDay
            && (i == 0 || candles[i - 1].time.div_euclid(86_400) * 86_400 != day_start);
        let complete = flow.is_some_and(|f| {
            f.delta.is_some()
                && f.covered_from <= bar.time
                && f.through_ms >= bar.time.saturating_mul(1000)
        });
        let can_start = match reset {
            CvdReset::Continuous => prev.is_none_or(|p| !p.started),
            CvdReset::UtcDay => starts_day && flow.is_some_and(|f| f.covered_from <= day_start),
        };
        let mut state = if starts_day {
            CvdCheckpoint {
                sum: 0.0,
                compensation: 0.0,
                origin: day_start,
                quality: FlowQuality::Exact,
                through_ms: 0,
                started: true,
                valid: false,
            }
        } else {
            prev.unwrap_or(CvdCheckpoint {
                sum: 0.0,
                compensation: 0.0,
                origin: 0,
                quality: FlowQuality::Exact,
                through_ms: 0,
                started: false,
                valid: false,
            })
        };
        let bridge = prev.is_none_or(|p| {
            !p.started
                || (p.valid
                    && p.through_ms
                        == candles[i.saturating_sub(1)]
                            .time
                            .saturating_add(step)
                            .saturating_mul(1000)
                    && flow.is_some_and(|f| {
                        f.covered_from <= candles[i.saturating_sub(1)].time.saturating_add(step)
                    }))
        });
        let acceptable = complete
            && (can_start || bridge)
            && flow.is_some_and(|f| f.quality != FlowQuality::Missing);
        if acceptable && (can_start || state.valid) {
            let f = flow.unwrap();
            if can_start {
                state.origin = match reset {
                    CvdReset::Continuous => bar.time,
                    CvdReset::UtcDay => day_start,
                };
                state.sum = 0.0;
                state.compensation = 0.0;
                state.quality = FlowQuality::Exact;
                state.started = true;
            }
            neumaier_add(&mut state.sum, &mut state.compensation, f.delta.unwrap());
            state.quality =
                if state.quality == FlowQuality::Reported || f.quality == FlowQuality::Reported {
                    FlowQuality::Reported
                } else {
                    FlowQuality::Exact
                };
            state.through_ms = f.through_ms;
            state.valid = true;
        } else if state.started {
            state.valid = false;
        }
        checkpoints[i] = state;
        points[i] = point(
            bar.time,
            [
                state.valid.then_some(state.sum + state.compensation),
                None,
                None,
            ],
        );
    }
}

fn neumaier_add(sum: &mut f64, compensation: &mut f64, value: f64) {
    let next = *sum + value;
    if sum.abs() >= value.abs() {
        *compensation += (*sum - next) + value;
    } else {
        *compensation += (value - next) + *sum;
    }
    *sum = next;
}

fn point(time: i64, values: [Option<f64>; 3]) -> IndicatorPoint {
    IndicatorPoint { time, values }
}

fn sma(candles: &[Candle], points: &mut [IndicatorPoint], from: usize, period: usize) {
    let start = from.saturating_sub(period - 1);
    let mut sum = 0.0;
    for i in start..candles.len() {
        if i - start >= period {
            sum -= candles[i - period].close;
        }
        sum += candles[i].close;
        if i >= from {
            let value = (i + 1 >= period).then_some(sum / period as f64);
            points[i] = point(candles[i].time, [value, None, None]);
        }
    }
}

fn close_ema(candles: &[Candle], i: usize, period: usize, previous: Option<f64>) -> Option<f64> {
    if i + 1 < period {
        None
    } else if i + 1 == period {
        Some(candles[..period].iter().map(|bar| bar.close).sum::<f64>() / period as f64)
    } else {
        previous.map(|value| value + 2.0 / (period as f64 + 1.0) * (candles[i].close - value))
    }
}

fn bollinger(
    candles: &[Candle],
    points: &mut [IndicatorPoint],
    from: usize,
    period: usize,
    deviations: f64,
) {
    let start = from.saturating_sub(period - 1);
    let mut count = 0;
    let mut mean = 0.0;
    let mut m2 = 0.0;
    for i in start..candles.len() {
        if count == period {
            if period == 1 {
                count = 0;
                mean = 0.0;
                m2 = 0.0;
            } else {
                let old = candles[i - period].close;
                let next_mean = mean + (mean - old) / (period - 1) as f64;
                m2 = (m2 - (old - mean) * (old - next_mean)).max(0.0);
                mean = next_mean;
                count -= 1;
            }
        }
        count += 1;
        let delta = candles[i].close - mean;
        mean += delta / count as f64;
        m2 += delta * (candles[i].close - mean);
        if i >= from {
            let values = if count == period {
                // Population variance; Welford removal/addition avoids E[x²] - E[x]² cancellation.
                let width = deviations * (m2.max(0.0) / period as f64).sqrt();
                [Some(mean), Some(mean + width), Some(mean - width)]
            } else {
                [None; 3]
            };
            points[i] = point(candles[i].time, values);
        }
    }
}

fn rsi(
    candles: &[Candle],
    points: &mut [IndicatorPoint],
    averages: &mut Vec<[f64; 2]>,
    from: usize,
    period: usize,
) {
    averages.resize(candles.len(), [0.0; 2]);
    for i in from..candles.len() {
        if i == 0 {
            averages[i] = [0.0; 2];
            points[i] = point(candles[i].time, [None; 3]);
            continue;
        }
        let change = candles[i].close - candles[i - 1].close;
        let gain = change.max(0.0);
        let loss = (-change).max(0.0);
        let previous = averages[i - 1];
        averages[i] = if i < period {
            [previous[0] + gain, previous[1] + loss]
        } else if i == period {
            [
                (previous[0] + gain) / period as f64,
                (previous[1] + loss) / period as f64,
            ]
        } else {
            [
                wilder(previous[0], gain, period),
                wilder(previous[1], loss, period),
            ]
        };
        let value = if i < period {
            None
        } else {
            let [gain, loss] = averages[i];
            Some(if gain == 0.0 && loss == 0.0 {
                50.0
            } else if loss == 0.0 {
                100.0
            } else if gain == 0.0 {
                0.0
            } else {
                100.0 * gain / (gain + loss)
            })
        };
        points[i] = point(candles[i].time, [value, None, None]);
    }
}

fn wilder(previous: f64, current: f64, period: usize) -> f64 {
    (previous * (period - 1) as f64 + current) / period as f64
}

fn macd(
    candles: &[Candle],
    points: &mut [IndicatorPoint],
    emas: &mut Vec<[Option<f64>; 2]>,
    from: usize,
    fast: usize,
    slow: usize,
    signal: usize,
) {
    emas.resize(candles.len(), [None; 2]);
    let signal_seed = slow + signal - 2;
    for i in from..candles.len() {
        let previous = if i == 0 { [None; 2] } else { emas[i - 1] };
        emas[i] = [
            close_ema(candles, i, fast, previous[0]),
            close_ema(candles, i, slow, previous[1]),
        ];
        let value = emas[i][0].zip(emas[i][1]).map(|(fast, slow)| fast - slow);
        let signal_value = if i < signal_seed {
            None
        } else if i == signal_seed {
            let sum: f64 = points[slow - 1..i]
                .iter()
                .filter_map(|point| point.values[0])
                .sum();
            value.map(|value| (sum + value) / signal as f64)
        } else {
            points[i - 1].values[1]
                .zip(value)
                .map(|(previous, current)| {
                    previous + 2.0 / (signal as f64 + 1.0) * (current - previous)
                })
        };
        let histogram = value
            .zip(signal_value)
            .map(|(value, signal)| value - signal);
        points[i] = point(candles[i].time, [value, signal_value, histogram]);
    }
}

fn true_range(candles: &[Candle], i: usize) -> f64 {
    let bar = &candles[i];
    let range = bar.high - bar.low;
    if i == 0 {
        range
    } else {
        range
            .max((bar.high - candles[i - 1].close).abs())
            .max((bar.low - candles[i - 1].close).abs())
    }
}

fn atr(candles: &[Candle], points: &mut [IndicatorPoint], from: usize, period: usize) {
    for i in from..candles.len() {
        let value = if i + 1 < period {
            None
        } else if i + 1 == period {
            Some((0..period).map(|j| true_range(candles, j)).sum::<f64>() / period as f64)
        } else {
            points[i - 1].values[0].map(|previous| wilder(previous, true_range(candles, i), period))
        };
        points[i] = point(candles[i].time, [value, None, None]);
    }
}

fn stochastic(
    candles: &[Candle],
    points: &mut [IndicatorPoint],
    raw: &mut Vec<f64>,
    highs: &mut VecDeque<usize>,
    lows: &mut VecDeque<usize>,
    from: usize,
    periods: [usize; 3],
) {
    let [period, smooth_k, smooth_d] = periods;
    raw.resize(candles.len(), 0.0);
    highs.clear();
    lows.clear();
    let start = from.saturating_sub(period - 1);
    let mut sum_k: f64 = raw[from.saturating_sub(smooth_k - 1)..from].iter().sum();
    let mut sum_d: f64 = points[from.saturating_sub(smooth_d - 1)..from]
        .iter()
        .filter_map(|point| point.values[0])
        .sum();
    for i in start..candles.len() {
        let window = (i + 1).saturating_sub(period);
        while highs.front().is_some_and(|&j| j < window) {
            highs.pop_front();
        }
        while lows.front().is_some_and(|&j| j < window) {
            lows.pop_front();
        }
        while highs
            .back()
            .is_some_and(|&j| candles[j].high <= candles[i].high)
        {
            highs.pop_back();
        }
        while lows
            .back()
            .is_some_and(|&j| candles[j].low >= candles[i].low)
        {
            lows.pop_back();
        }
        highs.push_back(i);
        lows.push_back(i);
        if i < from {
            continue;
        }
        raw[i] = if i + 1 < period {
            0.0
        } else {
            let high = candles[*highs.front().unwrap()].high;
            let low = candles[*lows.front().unwrap()].low;
            if high == low {
                50.0
            } else {
                100.0 * (candles[i].close - low) / (high - low)
            }
        };
        if i > from && i >= smooth_k {
            sum_k -= raw[i - smooth_k];
        }
        sum_k += raw[i];
        let k = (i >= period + smooth_k - 2).then_some(sum_k / smooth_k as f64);
        if i > from && i >= smooth_d {
            sum_d -= points[i - smooth_d].values[0].unwrap_or(0.0);
        }
        sum_d += k.unwrap_or(0.0);
        let d = (i >= period + smooth_k + smooth_d - 3).then_some(sum_d / smooth_d as f64);
        points[i] = point(candles[i].time, [k, d, None]);
    }
}

#[cfg(test)]
mod tests;
