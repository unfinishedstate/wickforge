//! Native-candle HLC3 VWAP, not execution-price/tick VWAP.
//!
//! Checkpoints contain only accumulator/coverage state. The consumer owns its
//! candle-aligned points, so drawing and indicator caches need no duplicate series.

use std::fmt;

use mexc_model::{Candle, Timeframe};

use crate::{
    IndicatorPoint, neumaier_add,
    profiles::{self, Period},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VwapReset {
    Session(Period),
    Anchored { start_ms: i64 },
}

impl VwapReset {
    pub fn validate(self) -> Result<(), String> {
        match self {
            Self::Session(Period::Custom {
                start_minute,
                end_minute,
                ..
            }) if start_minute >= 1440 || end_minute >= 1440 => {
                Err("Session minutes must be between 00:00 and 23:59".into())
            }
            Self::Anchored { start_ms }
                if chrono::DateTime::from_timestamp_millis(start_ms).is_none() =>
            {
                Err("VWAP anchor is outside the supported calendar".into())
            }
            _ => Ok(()),
        }
    }
}

impl fmt::Display for VwapReset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Session(Period::Custom {
                timezone,
                start_minute,
                end_minute,
            }) => write!(
                f,
                "{timezone} {:02}:{:02}–{:02}:{:02}",
                start_minute / 60,
                start_minute % 60,
                end_minute / 60,
                end_minute % 60,
            ),
            Self::Session(period) => period.fmt(f),
            Self::Anchored { start_ms } => write!(f, "anchored at {start_ms} ms"),
        }
    }
}

/// Coverage refers to native candle continuity, not trade completeness or whether
/// the current candle has closed. Partial history retains the true requested origin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VwapContext {
    pub origin_ms: i64,
    pub covered: bool,
}

/// Mantissa sums and binary exponents keep both volume and price × volume
/// bounded, without losing a representable product to an intermediate overflow
/// or underflow. Rescaling is exact except for negligible subnormal terms.
#[derive(Debug, Clone, Copy, Default)]
struct ScaledSum {
    sum: f64,
    error: f64,
    exponent: i32,
}

impl ScaledSum {
    fn add(&mut self, mantissa: f64, exponent: i32) {
        if mantissa == 0.0 {
            return;
        }
        if self.sum == 0.0 && self.error == 0.0 {
            self.exponent = exponent;
        } else if exponent > self.exponent {
            self.sum = scale_binary(self.sum, self.exponent - exponent);
            self.error = scale_binary(self.error, self.exponent - exponent);
            self.exponent = exponent;
        }
        neumaier_add(
            &mut self.sum,
            &mut self.error,
            scale_binary(mantissa, exponent - self.exponent),
        );
    }
}

/// A finite nonzero value as a signed [1, 2) mantissa and binary exponent.
fn binary_parts(value: f64) -> (f64, i32) {
    let bits = value.to_bits();
    let exponent = ((bits >> 52) & 0x7ff) as i32;
    if exponent == 0 {
        let (mantissa, exponent) = binary_parts(value * 4_503_599_627_370_496.0);
        return (mantissa, exponent - 52);
    }
    (
        f64::from_bits((bits & 0x800f_ffff_ffff_ffff) | (1023_u64 << 52)),
        exponent - 1023,
    )
}

fn scale_binary(mut value: f64, mut exponent: i32) -> f64 {
    while exponent > 1023 {
        value *= f64::from_bits(2046_u64 << 52);
        exponent -= 1023;
    }
    while exponent < -1022 {
        value *= f64::from_bits(1_u64 << 52);
        exponent += 1022;
    }
    value * f64::from_bits(((exponent + 1023) as u64) << 52)
}

#[derive(Debug, Clone, Copy, Default)]
struct Weighted {
    volume: ScaledSum,
    priced: ScaledSum,
    low: f64,
    high: f64,
}

impl Weighted {
    fn add(&mut self, price: f64, volume: f64) {
        if volume == 0.0 {
            return;
        }
        if self.volume.sum == 0.0 {
            self.low = price;
            self.high = price;
        } else {
            self.low = self.low.min(price);
            self.high = self.high.max(price);
        }
        let (weight, weight_exponent) = binary_parts(volume);
        self.volume.add(weight, weight_exponent);
        if price != 0.0 {
            let (value, price_exponent) = binary_parts(price);
            self.priced
                .add(value * weight, price_exponent + weight_exponent);
        }
    }

    fn value(self) -> Option<f64> {
        let volume = self.volume.sum + self.volume.error;
        if volume <= 0.0 {
            return None;
        }
        let ratio = (self.priced.sum + self.priced.error) / volume;
        // Rounding at the finite range boundary must not overflow a convex mean.
        let value = scale_binary(ratio, self.priced.exponent - self.volume.exponent)
            .clamp(self.low, self.high);
        value.is_finite().then_some(value)
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct Checkpoint {
    weighted: Weighted,
    context: Option<VwapContext>,
    time: i64,
}

#[derive(Debug, Default)]
pub struct VwapState {
    checkpoints: Vec<Checkpoint>,
    configuration: Option<(Timeframe, VwapReset)>,
    first_time: Option<i64>,
}

impl VwapState {
    pub fn context(&self, index: usize) -> Option<VwapContext> {
        self.checkpoints.get(index)?.context
    }

    /// Replaces a forming bar or recalculates a corrected suffix from the prior
    /// checkpoint. Historical prepends must pass zero, as with IndicatorSet.
    /// Changed first/boundary timestamps also force a reseed rather than reusing
    /// checkpoints belonging to a different native candle sequence.
    pub fn update(
        &mut self,
        candles: &[Candle],
        points: &mut Vec<IndicatorPoint>,
        from: usize,
        timeframe: Timeframe,
        reset: VwapReset,
    ) {
        let mut from = from
            .min(candles.len())
            .min(points.len())
            .min(self.checkpoints.len());
        let first_time = candles.first().map(|bar| bar.time);
        if self.configuration != Some((timeframe, reset))
            || self.first_time != first_time
            || from
                .checked_sub(1)
                .is_some_and(|i| self.checkpoints[i].time != candles[i].time)
        {
            from = 0;
        }
        self.configuration = Some((timeframe, reset));
        self.first_time = first_time;
        points.resize(
            candles.len(),
            IndicatorPoint {
                time: 0,
                values: [None; 3],
            },
        );
        self.checkpoints
            .resize(candles.len(), Checkpoint::default());
        let reset_valid = reset.validate().is_ok();
        let step = timeframe.seconds();
        for i in from..candles.len() {
            let bar = candles[i];
            let previous = i.checked_sub(1).map(|j| self.checkpoints[j]);
            let mut current = Checkpoint {
                time: bar.time,
                ..Checkpoint::default()
            };
            let interval = bar.time.checked_mul(1000).and_then(|start| {
                chrono::DateTime::from_timestamp_millis(start)?;
                bar.time
                    .checked_add(step)?
                    .checked_mul(1000)
                    .map(|end| (start, end))
            });
            let mut output = None;
            if reset_valid && let Some((start, end)) = interval {
                let origin = match reset {
                    VwapReset::Session(period) => {
                        let (open, close) = profiles::period_bounds(period, start);
                        let same_window = !matches!(period, Period::Custom { .. })
                            || profiles::period_bounds(period, end - 1).0 == open;
                        (open <= start && start < close)
                            .then_some((open, end <= close && same_window))
                    }
                    VwapReset::Anchored { start_ms } => {
                        (start >= start_ms).then_some((start_ms, true))
                    }
                };
                if let Some((origin_ms, within_window)) = origin {
                    let same_origin =
                        previous.filter(|p| p.context.is_some_and(|c| c.origin_ms == origin_ms));
                    if let Some(prior) = same_origin {
                        current.weighted = prior.weighted;
                    }
                    let continuous =
                        same_origin.is_none_or(|p| p.time.checked_add(step) == Some(bar.time));
                    let covered = same_origin
                        .map_or(start == origin_ms, |p| p.context.unwrap().covered)
                        && continuous
                        && within_window;
                    let ordered = previous.is_none_or(|p| p.time < bar.time);
                    let valid = within_window && ordered && valid_bar(bar);
                    current.context = Some(VwapContext {
                        origin_ms,
                        covered: covered && valid,
                    });
                    if valid {
                        current.weighted.add(hlc3(bar), bar.volume);
                        output = current.weighted.value();
                    }
                }
            }
            self.checkpoints[i] = current;
            points[i] = IndicatorPoint {
                time: bar.time,
                values: [output, None, None],
            };
        }
    }
}

fn valid_bar(bar: Candle) -> bool {
    bar.volume.is_finite()
        && bar.volume >= 0.0
        && bar.low.is_finite()
        && bar.high.is_finite()
        && bar.low <= bar.high
        && bar.open.is_finite()
        && bar.close.is_finite()
        && bar.open >= bar.low
        && bar.open <= bar.high
        && bar.close >= bar.low
        && bar.close <= bar.high
}

fn hlc3(bar: Candle) -> f64 {
    let mut sum = ScaledSum::default();
    for price in [bar.high, bar.low, bar.close] {
        if price != 0.0 {
            let (mantissa, exponent) = binary_parts(price);
            sum.add(mantissa, exponent);
        }
    }
    scale_binary((sum.sum + sum.error) / 3.0, sum.exponent).clamp(bar.low, bar.high)
}
