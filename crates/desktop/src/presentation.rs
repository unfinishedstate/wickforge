use chrono::{Datelike, TimeZone, Utc};

fn trimmed(mut value: String) -> String {
    if value.contains('.') {
        while value.ends_with('0') {
            value.pop();
        }
        if value.ends_with('.') {
            value.pop();
        }
    }
    value
}

fn decimal(value: f64, digits: u32) -> String {
    let magnitude = value.abs();
    if !value.is_finite() {
        return "—".into();
    }
    if value == 0.0 {
        return "0".into();
    }
    let exponent = magnitude.log10().floor() as i32;
    if exponent < -12 {
        return scientific(value, digits);
    }
    let precision = (digits as i32 - exponent - 1).clamp(0, 324) as usize;
    trimmed(format!("{value:.precision$}"))
}

fn scientific(value: f64, digits: u32) -> String {
    let text = format!(
        "{value:.precision$e}",
        precision = digits.saturating_sub(1) as usize
    );
    let (mantissa, exponent) = text.split_once('e').unwrap();
    format!("{}e{exponent}", trimmed(mantissa.into()))
}

/// Native quantities only: suffixes abbreviate magnitude, never convert units.
pub fn quantity(value: f64) -> String {
    quantity_digits(value, 6)
}

fn quantity_digits(value: f64, digits: u32) -> String {
    let suffixes = ["", "K", "M", "B", "T"];
    let mut index = if value.is_finite() && value.abs() >= 1000.0 {
        (value.abs().log10() / 3.0).floor().min(4.0) as usize
    } else {
        0
    };
    let mut scaled = value / 1000.0_f64.powi(index as i32);
    // Promote after rounding too: 999.9999K must become 1M, not 1000K.
    if index < 4 && scaled.abs() >= 1000.0 - 0.5 * 10.0_f64.powi(3 - digits as i32) {
        index += 1;
        scaled /= 1000.0;
    }
    if scaled.is_finite() && scaled.abs() >= 1e9 {
        return scientific(value, digits);
    }
    let mut text = decimal(scaled, digits);
    if value.is_finite() {
        text.push_str(suffixes[index]);
    }
    text
}

/// Unabridged native quantity with fifteen significant digits, without binary tails.
pub fn quantity_exact(value: f64) -> String {
    decimal(value, 15)
}

pub fn quantity_tick(value: f64, step: f64) -> String {
    let digits = if value == 0.0 {
        6
    } else {
        ((value.abs().log10() - step.log10()).ceil() as i32 + 2).clamp(6, 17) as u32
    };
    quantity_digits(value, digits)
}

pub fn fraction(value: f64, low: f64, high: f64) -> f64 {
    let scale = low.abs().max(high.abs()).max(f64::MIN_POSITIVE);
    (value / scale - low / scale) / (high / scale - low / scale)
}

pub struct ValueTicks {
    first: f64,
    pub step: f64,
    high: f64,
    index: u32,
    remaining: u32,
    previous: Option<f64>,
}

impl ValueTicks {
    pub fn new(low: f64, high: f64, pixels: f32, minimum_step: f64) -> Self {
        let mut ticks = Self {
            first: 0.0,
            step: 1.0,
            high,
            index: 0,
            remaining: 0,
            previous: None,
        };
        if !low.is_finite() || !high.is_finite() || low >= high || pixels < 32.0 {
            return ticks;
        }
        let slots = (pixels / 42.0).floor().clamp(2.0, 24.0) as f64;
        let scale = low.abs().max(high.abs()).max(f64::MIN_POSITIVE);
        let target_log = ((high / scale - low / scale) / slots).log10() + scale.log10();
        let decade = 10.0_f64.powf(target_log.floor()).max(f64::from_bits(1));
        let target = 10.0_f64.powf(target_log).min(f64::MAX);
        let multiplier = [1.0, 2.0, 2.5, 5.0, 10.0]
            .into_iter()
            .find(|factor| decade * factor >= target)
            .unwrap_or(10.0);
        ticks.step = (decade * multiplier)
            .min(f64::MAX)
            .max(minimum_step)
            .max(scale * f64::EPSILON * 2.0);
        ticks.first = (low / ticks.step).ceil() * ticks.step;
        if ticks.first < low {
            ticks.first += ticks.step;
        }
        ticks.remaining = 32;
        ticks
    }
}

impl Iterator for ValueTicks {
    type Item = f64;
    fn next(&mut self) -> Option<f64> {
        while self.remaining > 0 {
            self.remaining -= 1;
            let value = self.step.mul_add(f64::from(self.index), self.first);
            self.index += 1;
            if !value.is_finite() || value > self.high {
                return None;
            }
            if self.previous.is_none_or(|previous| value > previous) {
                self.previous = Some(value);
                return Some(if value == 0.0 { 0.0 } else { value });
            }
        }
        None
    }
}

#[derive(Clone, Copy)]
enum TimeStep {
    Seconds(i64),
    Months(i32),
    Years(i32),
}

pub struct TimeTicks {
    next: Option<i64>,
    right: f64,
    step: TimeStep,
    remaining: u32,
    pub format: &'static str,
    pub label_width: f32,
}

impl TimeTicks {
    pub fn new(left: f64, span: f64, width: f32) -> Self {
        let start = chrono::DateTime::from_timestamp(left as i64, 0);
        let end = chrono::DateTime::from_timestamp((left + span) as i64, 0);
        let same_day = start
            .zip(end)
            .is_some_and(|(a, b)| a.date_naive() == b.date_naive());
        let same_year = start.zip(end).is_some_and(|(a, b)| a.year() == b.year());
        let intraday_format = if same_day {
            "%H:%M"
        } else if same_year {
            "%m-%d %H:%M"
        } else {
            "%Y-%m-%d %H:%M"
        };
        let intraday_width = if same_day {
            40.0
        } else if same_year {
            88.0
        } else {
            124.0
        };
        let target = span / f64::from((width / (intraday_width + 24.0)).floor().max(1.0));
        let seconds = [
            60, 120, 300, 600, 900, 1800, 3600, 7200, 14400, 21600, 43200,
        ];
        let (step, format, label_width) = if let Some(seconds) = seconds
            .into_iter()
            .find(|seconds| *seconds as f64 >= target)
        {
            (TimeStep::Seconds(seconds), intraday_format, intraday_width)
        } else {
            let target = span / f64::from((width / 100.0).floor().max(1.0));
            if let Some(days) = [1, 2, 7, 14]
                .into_iter()
                .find(|days| *days as f64 * 86400.0 >= target)
            {
                (TimeStep::Seconds(days * 86400), "%Y-%m-%d", 80.0)
            } else if target <= 180.0 * 86400.0 {
                let months = if target <= 28.0 * 86400.0 {
                    1
                } else if target <= 60.0 * 86400.0 {
                    2
                } else if target <= 90.0 * 86400.0 {
                    3
                } else {
                    6
                };
                (TimeStep::Months(months), "%Y-%m", 60.0)
            } else {
                let years = (target / (365.0 * 86400.0)).ceil().clamp(1.0, 100_000.0) as i32;
                (TimeStep::Years(years), "%Y", 36.0)
            }
        };
        let next = start.and_then(|date| match step {
            TimeStep::Seconds(seconds) => {
                // Whole-week ticks start on Monday, not Unix epoch Thursday.
                let offset = if seconds >= 604800 { 345600 } else { 0 };
                (left as i64)
                    .checked_sub(offset)
                    .and_then(|time| time.div_euclid(seconds).checked_mul(seconds))
                    .and_then(|time| time.checked_add(offset))
                    .and_then(|time| {
                        if time as f64 >= left {
                            Some(time)
                        } else {
                            time.checked_add(seconds)
                        }
                    })
            }
            TimeStep::Months(months) => {
                let month = (date.month0() as i32 / months) * months + 1;
                Utc.with_ymd_and_hms(date.year(), month as u32, 1, 0, 0, 0)
                    .single()
                    .map(|date| date.timestamp())
            }
            TimeStep::Years(years) => Utc
                .with_ymd_and_hms(date.year().div_euclid(years) * years, 1, 1, 0, 0, 0)
                .single()
                .map(|date| date.timestamp()),
        });
        let mut ticks = Self {
            next,
            right: left + span,
            step,
            remaining: if left.is_finite() && span.is_finite() && span > 0.0 {
                64
            } else {
                0
            },
            format,
            label_width,
        };
        if ticks.next.is_some_and(|time| (time as f64) < left) {
            let _ = ticks.next();
        }
        ticks
    }
}

impl Iterator for TimeTicks {
    type Item = i64;
    fn next(&mut self) -> Option<i64> {
        if self.remaining == 0 {
            return None;
        }
        self.remaining -= 1;
        let time = self.next?;
        if time as f64 > self.right {
            return None;
        }
        self.next = match self.step {
            TimeStep::Seconds(seconds) => time.checked_add(seconds),
            TimeStep::Months(months) => {
                chrono::DateTime::from_timestamp(time, 0).and_then(|date| {
                    let month = date.month0() as i32 + months;
                    Utc.with_ymd_and_hms(
                        date.year().checked_add(month / 12)?,
                        (month % 12 + 1) as u32,
                        1,
                        0,
                        0,
                        0,
                    )
                    .single()
                    .map(|date| date.timestamp())
                })
            }
            TimeStep::Years(years) => chrono::DateTime::from_timestamp(time, 0).and_then(|date| {
                Utc.with_ymd_and_hms(date.year().checked_add(years)?, 1, 1, 0, 0, 0)
                    .single()
                    .map(|date| date.timestamp())
            }),
        };
        Some(time)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_quantities_round_suffix_boundaries_and_keep_small_nonzero() {
        assert_eq!(quantity(999_999.9), "1M");
        assert_eq!(quantity(-999_999.9), "-1M");
        assert_eq!(quantity(12_345_600.0), "12.3456M");
        assert_eq!(quantity(0.0000000123456), "0.0000000123456");
        assert_eq!(quantity(-0.0), "0");
        assert_eq!(quantity(f64::NAN), "—");
        assert_eq!(quantity_exact(0.1 + 0.2), "0.3");
        assert_eq!(quantity_exact(12_345_678.125), "12345678.125");
        assert_ne!(quantity(f64::from_bits(1)), "0");
    }
    #[test]
    fn value_ticks_are_aligned_bounded_distinct_and_pixel_aware() {
        for (low, high) in [
            (1.1, 9.7),
            (-1e308, 1e308),
            (1e-300, 9e-300),
            (1e20, 1e20 + 1e6),
        ] {
            let ticks = ValueTicks::new(low, high, 190.0, 0.0);
            let step = ticks.step;
            let values: Vec<_> = ticks.collect();
            assert!(values.len() <= 6);
            assert!(
                values
                    .iter()
                    .all(|value| value.is_finite() && *value >= low && *value <= high)
            );
            assert!(values.windows(2).all(|pair| pair[0] < pair[1]
                && quantity_tick(pair[0], step) != quantity_tick(pair[1], step)));
        }
        assert!(
            ValueTicks::new(1.0, 10.0, 800.0, 0.0).count()
                > ValueTicks::new(1.0, 10.0, 100.0, 0.0).count()
        );
        assert!(ValueTicks::new(0.000000001, 0.000000009, 340.0, 1e-9).all(|value| value >= 1e-9));
        assert!(ValueTicks::new(0.0, 100.0, 58.0, 0.01).any(|value| value > 0.0 && value < 100.0));
    }
    #[test]
    fn closely_zoomed_quantity_ticks_remain_numerically_distinct() {
        let low = 1.0_f64;
        let high = f64::from_bits(low.to_bits() + 16);
        let ticks = ValueTicks::new(low, high, 600.0, 0.0);
        let step = ticks.step;
        let mut previous = None;
        let mut compared = false;
        for value in ticks {
            let displayed: f64 = quantity_tick(value, step).parse().unwrap();
            if let Some(previous) = previous {
                assert!(displayed > previous, "{displayed} repeats {previous}");
                compared = true;
            }
            assert!((displayed - value).abs() < step);
            previous = Some(displayed);
        }
        assert!(compared);
    }
    #[test]
    fn calendar_ticks_land_on_utc_month_and_year_boundaries() {
        let left = Utc
            .with_ymd_and_hms(2023, 12, 17, 12, 0, 0)
            .unwrap()
            .timestamp() as f64;
        let months: Vec<_> = TimeTicks::new(left, 200.0 * 86400.0, 700.0).collect();
        assert!(!months.is_empty());
        for time in months {
            let date = chrono::DateTime::from_timestamp(time, 0).unwrap();
            assert_eq!(date.day(), 1);
            assert_eq!(time.rem_euclid(86400), 0);
        }
        let years: Vec<_> = TimeTicks::new(left, 20.0 * 365.0 * 86400.0, 700.0).collect();
        assert!(years.len() <= 10);
        assert!(years.iter().all(|time| {
            chrono::DateTime::from_timestamp(*time, 0)
                .unwrap()
                .ordinal()
                == 1
        }));
        let weeks: Vec<_> = TimeTicks::new(left, 40.0 * 86400.0, 700.0).collect();
        assert!(weeks.iter().all(|time| {
            chrono::DateTime::from_timestamp(*time, 0)
                .unwrap()
                .weekday()
                == chrono::Weekday::Mon
        }));
    }
    #[test]
    fn narrow_quarter_range_has_a_calendar_label_away_from_clipped_edges() {
        let left = Utc
            .with_ymd_and_hms(2026, 7, 2, 0, 0, 0)
            .unwrap()
            .timestamp() as f64;
        let span = 94.0 * 86400.0;
        let width = 380.0;
        let mut ticks = TimeTicks::new(left, span, width);
        let half_width = ticks.label_width * 0.5;
        assert!(ticks.any(|time| {
            let x = ((time as f64 - left) / span) as f32 * width;
            x >= half_width && x + half_width <= width
        }));
    }

    #[test]
    fn time_ticks_scale_with_width_and_stay_bounded_at_extremes() {
        let left = Utc
            .with_ymd_and_hms(2024, 2, 29, 1, 3, 0)
            .unwrap()
            .timestamp() as f64;
        for span in [600.0, 120.0 * 300.0, 10_000.0 * 86400.0] {
            let narrow: Vec<_> = TimeTicks::new(left, span, 300.0).collect();
            let wide: Vec<_> = TimeTicks::new(left, span, 1000.0).collect();
            assert!(narrow.len() <= wide.len());
            assert!(wide.len() <= 64);
            assert!(
                wide.iter()
                    .all(|time| *time as f64 >= left && *time as f64 <= left + span)
            );
            assert!(wide.windows(2).all(|pair| pair[0] < pair[1]));
        }
        assert_eq!(TimeTicks::new(f64::MAX, f64::MAX, 1000.0).count(), 0);
    }
}
