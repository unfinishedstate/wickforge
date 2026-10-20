//! Deterministic native-tick profiles. Source identity and coverage are independent
//! of the display chart. Prices and volume remain decimal until presentation.
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::fmt;

use chrono::{Datelike, Duration, LocalResult, NaiveDate, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Tz;
use mexc_model::Candle;
use mexc_model::executions::{Coverage, Execution, NativeInstrument};
use rust_decimal::Decimal;
use rust_decimal::prelude::{FromPrimitive, ToPrimitive};

const MINUTE: i64 = 60_000;
const MAX_SESSIONS: usize = 33;
const MAX_ROWS: usize = 200_000;
const MAX_CACHED_ROWS: usize = 1_000_000;
const MAX_BARS: usize = 250_000;
const MAX_EXECUTIONS: usize = 2_000_000;
const MAX_CACHES: usize = 4096;
const MAX_CHANGES: usize = 65_536;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Tpo,
    Volume,
}
impl Kind {
    pub const ALL: [Self; 2] = [Self::Tpo, Self::Volume];
}
impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Tpo => "TPO",
            Self::Volume => "Volume",
        })
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Period {
    Day,
    Week,
    Month,
    Custom {
        timezone: Tz,
        start_minute: u16,
        end_minute: u16,
    },
}
impl Period {
    pub const ALL: [Self; 4] = [
        Self::Day,
        Self::Week,
        Self::Month,
        Self::Custom {
            timezone: chrono_tz::UTC,
            start_minute: 0,
            end_minute: 0,
        },
    ];

    /// Daily IANA-local session with strict `HH:MM` bounds. A close at or before
    /// the open belongs to the following local date; equal times mean a full day.
    /// Ambiguous opens choose the earliest instant and closes the latest.
    /// Nonexistent minutes advance to the first valid local minute. If that
    /// collapses a window to zero duration, that date has no formation.
    pub fn custom(timezone: &str, start: &str, end: &str) -> Result<Self, String> {
        Self::custom_minutes(
            timezone,
            parse_session_time(start)?,
            parse_session_time(end)?,
        )
    }

    pub fn custom_minutes(
        timezone: &str,
        start_minute: u16,
        end_minute: u16,
    ) -> Result<Self, String> {
        if start_minute >= 1440 || end_minute >= 1440 {
            return Err("Session minutes must be between 00:00 and 23:59".into());
        }
        let timezone = timezone
            .parse::<Tz>()
            .map_err(|_| "Enter a valid IANA timezone, for example America/New_York")?;
        Ok(Self::Custom {
            timezone,
            start_minute,
            end_minute,
        })
    }
}
impl fmt::Display for Period {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Day => "UTC day",
            Self::Week => "UTC week",
            Self::Month => "UTC month",
            Self::Custom { .. } => "Custom daily session",
        })
    }
}

pub fn parse_session_time(value: &str) -> Result<u16, String> {
    let bytes = value.as_bytes();
    if bytes.len() != 5
        || bytes[2] != b':'
        || ![bytes[0], bytes[1], bytes[3], bytes[4]]
            .iter()
            .all(u8::is_ascii_digit)
    {
        return Err("Enter session times as HH:MM (00:00–23:59)".into());
    }
    let hour = u16::from(bytes[0] - b'0') * 10 + u16::from(bytes[1] - b'0');
    let minute = u16::from(bytes[3] - b'0') * 10 + u16::from(bytes[4] - b'0');
    if hour > 23 || minute > 59 {
        return Err("Enter session times as HH:MM (00:00–23:59)".into());
    }
    Ok(hour * 60 + minute)
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Range {
    Session(Period),
    Fixed { start_ms: i64, end_ms: i64 },
    Visible,
}
impl fmt::Display for Range {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Session(p) => p.fmt(f),
            Self::Fixed { .. } => f.write_str("Fixed range"),
            Self::Visible => f.write_str("Visible range"),
        }
    }
}
#[derive(Debug, Clone, PartialEq)]
pub struct Display {
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
impl Default for Display {
    fn default() -> Self {
        Self {
            histogram: true,
            levels: true,
            previous: true,
            naked: false,
            single_prints: true,
            initial_balance: true,
            letters: false,
            poor_extremes: true,
            width_percent: 20,
        }
    }
}
#[derive(Debug, Clone, PartialEq)]
pub struct Spec {
    pub kind: Kind,
    pub range: Range,
    pub block_minutes: u32,
    pub ticks_per_row: u32,
    pub value_area_percent: u8,
    /// Fill unproved execution minutes from native 1m candles, without certifying coverage.
    pub estimate_missing: bool,
    pub display: Display,
}
impl Spec {
    pub fn default_for(kind: Kind) -> Self {
        let mut display = Display::default();
        if kind == Kind::Volume {
            display.single_prints = false;
            display.initial_balance = false;
            display.poor_extremes = false;
        }
        Self {
            kind,
            range: Range::Session(Period::Day),
            block_minutes: 30,
            ticks_per_row: 1,
            value_area_percent: 70,
            estimate_missing: true,
            display,
        }
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.block_minutes == 0 || self.block_minutes > 44_640 {
            return Err("TPO block duration must be 1–44640 whole minutes".into());
        }
        if self.ticks_per_row == 0 {
            return Err("Row width must be at least one native tick".into());
        }
        if !(1..=100).contains(&self.value_area_percent) {
            return Err("Value area must be between 1 and 100 percent".into());
        }
        if !(1..=100).contains(&self.display.width_percent) {
            return Err("Histogram width must be between 1 and 100 percent".into());
        }
        if let Range::Session(Period::Custom {
            start_minute,
            end_minute,
            ..
        }) = self.range
            && (start_minute >= 1440 || end_minute >= 1440)
        {
            return Err("Session minutes must be between 00:00 and 23:59".into());
        }
        if let Range::Fixed { start_ms, end_ms } = self.range {
            validate_bounds(self.kind, start_ms, end_ms)?;
        }
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq)]
pub struct Instance {
    pub id: u64,
    pub spec: Spec,
    pub visible: bool,
    pub instrument: Option<NativeInstrument>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub index: i64,
    pub low: f64,
    pub high: f64,
    pub weight: f64,
    pub blocks: Vec<u32>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Touch {
    Untouched,
    Partial,
    Filled,
    Unknown,
}
impl Touch {
    pub const ALL: [Self; 4] = [Self::Untouched, Self::Partial, Self::Filled, Self::Unknown];
}
impl fmt::Display for Touch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Untouched => "Untouched",
            Self::Partial => "Partially filled",
            Self::Filled => "Filled",
            Self::Unknown => "Unknown",
        })
    }
}
#[derive(Debug, Clone, PartialEq)]
pub struct Zone {
    pub low: f64,
    pub high: f64,
    pub touch: Touch,
    pub block: Option<u32>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct Level {
    pub price: f64,
    pub low: f64,
    pub high: f64,
    pub origin_ms: i64,
    pub touch: Touch,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeMethod {
    Exact,
    Estimated,
    Mixed,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub start_ms: i64,
    pub end_ms: i64,
    pub as_of_ms: i64,
    pub complete: bool,
    /// Volume: execution coverage through as_of_ms. Developing TPO: all closed
    /// native minutes covered and the current minute observed provisionally.
    pub covered: bool,
    pub reason: Option<String>,
    pub rows: Vec<Row>,
    /// Total contributed native quantity, including explicitly disclosed candle estimates.
    pub native_volume: Option<Decimal>,
    pub volume_method: VolumeMethod,
    pub poc: Option<f64>,
    pub vah: Option<f64>,
    pub val: Option<f64>,
    pub single_prints: Vec<Zone>,
    /// Auction tails are distinct from interior single prints, including their owner block.
    pub tails: Vec<Zone>,
    pub naked_pocs: Vec<Level>,
    pub initial_balance: Option<(f64, f64)>,
    pub high: Option<f64>,
    pub low: Option<f64>,
    pub poor_high: bool,
    pub poor_low: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Print {
    time: i64,
    price: Decimal,
    quantity: Decimal,
}
impl From<&Execution> for Print {
    fn from(e: &Execution) -> Self {
        Self {
            time: e.time_ms,
            price: e.price,
            quantity: e.quantity,
        }
    }
}
#[derive(Debug, Clone)]
enum Change {
    Bar {
        old: Option<Candle>,
        new: Candle,
    },
    Print {
        old: Option<Print>,
        new: Option<Print>,
    },
    Proof {
        start: i64,
        end: i64,
    },
}
#[derive(Debug, Clone, PartialEq, Eq)]
struct Key {
    kind: Kind,
    start: i64,
    end: i64,
    block: u32,
    width: u32,
    tick: Decimal,
    clip: Option<(i64, i64)>,
    estimate_missing: bool,
}
#[derive(Debug, Clone, Default)]
struct Bin {
    quantity: Decimal,
    blocks: BTreeMap<u32, u32>,
}
#[derive(Debug, Clone, Copy, PartialEq)]
struct Estimate {
    low: i64,
    high: i64,
    quantity: Decimal,
}

#[derive(Debug, Clone)]
struct Formation {
    key: Key,
    revision: u64,
    horizon: i64,
    last_used: u64,
    rows: BTreeMap<i64, Bin>,
    poc: Option<i64>,
    dirty: bool,
    initial_balance: Option<(f64, f64)>,
    ib_dirty: bool,
    estimates: BTreeMap<i64, Estimate>,
    estimated_quantity: Decimal,
}

/// Native source storage is bounded at 250,000 minute bars / 2,000,000
/// executions. A profile admits at most 200,000 rows; all cached contributions
/// together admit 1,000,000 rows / 4,096 configurations. Limits return explicit
/// errors, never rebinning, truncation, or synthetic coverage. Only changed
/// contributions enter the bounded revision journal; live updates do not clone
/// or reprocess the historical tape.
#[derive(Debug, Clone, Default)]
pub struct Engine {
    bars: BTreeMap<i64, Candle>,
    executions: BTreeMap<(i64, String), Execution>,
    identities: HashMap<String, i64>,
    bar_coverage: Vec<Coverage>,
    execution_coverage: Vec<Coverage>,
    proven_executions: Vec<(i64, i64)>,
    revision: u64,
    changes: VecDeque<(u64, Change)>,
    caches: Vec<Formation>,
    cycle: u64,
    cached_rows: usize,
    bar_error: Option<String>,
}
impl Engine {
    pub fn upsert_bars(&mut self, bars: &[Candle]) {
        for &bar in bars {
            if self.bars.get(&bar.time) == Some(&bar) {
                continue;
            }
            if !self.bars.contains_key(&bar.time) && self.bars.len() >= MAX_BARS {
                self.bar_error = Some(format!("Native bar limit ({MAX_BARS}) exceeded"));
                return;
            }
            let old = self.bars.insert(bar.time, bar);
            self.changed(Change::Bar { old, new: bar });
        }
    }
    pub fn ingest_executions(&mut self, executions: &[Execution]) -> Result<(), String> {
        self.validate_executions(executions, None)?;
        for e in executions {
            self.upsert_execution(e);
        }
        Ok(())
    }
    pub fn replace_executions(
        &mut self,
        start_ms: i64,
        end_ms: i64,
        executions: &[Execution],
    ) -> Result<(), String> {
        if start_ms >= end_ms {
            return Err("Replacement range must be nonempty".into());
        }
        self.validate_executions(executions, Some((start_ms, end_ms)))?;
        let keys: Vec<_> = self
            .executions
            .range((start_ms, String::new())..(end_ms, String::new()))
            .map(|(k, _)| k.clone())
            .collect();
        for key in keys {
            if let Some(old) = self.executions.remove(&key) {
                self.identities.remove(&old.id);
                self.changed(Change::Print {
                    old: Some(Print::from(&old)),
                    new: None,
                });
            }
        }
        for e in executions {
            self.upsert_execution(e);
        }
        Ok(())
    }
    pub fn set_bar_coverage(&mut self, coverage: Vec<Coverage>) {
        self.bar_coverage = coverage;
    }
    pub fn set_execution_coverage(&mut self, coverage: Vec<Coverage>) {
        let proven = proven_intervals(&coverage);
        for (start, end) in changed_intervals(&self.proven_executions, &proven) {
            self.changed(Change::Proof { start, end });
        }
        self.proven_executions = proven;
        self.execution_coverage = coverage;
    }
    pub fn invalidate_executions(&mut self, start_ms: i64, end_ms: i64, reason: String) {
        if start_ms >= end_ms {
            return;
        }
        let mut coverage = Vec::new();
        for c in self.execution_coverage.drain(..) {
            if c.end_ms <= start_ms || c.start_ms >= end_ms {
                coverage.push(c);
                continue;
            }
            if c.start_ms < start_ms {
                coverage.push(Coverage {
                    end_ms: start_ms,
                    ..c.clone()
                });
            }
            if c.end_ms > end_ms {
                coverage.push(Coverage {
                    start_ms: end_ms,
                    ..c
                });
            }
        }
        coverage.push(Coverage {
            start_ms,
            end_ms,
            complete: false,
            reason: Some(reason),
        });
        self.set_execution_coverage(coverage);
    }
    fn validate_executions(
        &self,
        executions: &[Execution],
        range: Option<(i64, i64)>,
    ) -> Result<(), String> {
        let mut new = BTreeSet::new();
        for e in executions {
            if e.id.is_empty() || e.quantity <= Decimal::ZERO {
                return Err(
                    "Executions require a native identity and positive native quantity".into(),
                );
            }
            if let Some((start, end)) = range
                && (e.time_ms < start || e.time_ms >= end)
            {
                return Err("Execution is outside replacement's half-open range".into());
            }
            if !self.identities.contains_key(&e.id) {
                new.insert(&e.id);
            }
        }
        let removed = range.map_or(0, |(start, end)| {
            self.executions
                .range((start, String::new())..(end, String::new()))
                .count()
        });
        let retained_ids = executions
            .iter()
            .filter(|e| {
                self.identities.get(&e.id).is_some_and(|time| {
                    range.is_some_and(|(start, end)| *time >= start && *time < end)
                })
            })
            .map(|e| &e.id)
            .collect::<BTreeSet<_>>()
            .len();
        if self
            .executions
            .len()
            .saturating_sub(removed)
            .saturating_add(new.len())
            .saturating_add(retained_ids)
            > MAX_EXECUTIONS
        {
            return Err(format!(
                "Native execution limit ({MAX_EXECUTIONS}) exceeded"
            ));
        }
        Ok(())
    }
    fn upsert_execution(&mut self, e: &Execution) {
        let old_time = self.identities.get(&e.id).copied();
        if old_time == Some(e.time_ms) && self.executions.get(&(e.time_ms, e.id.clone())) == Some(e)
        {
            return;
        }
        let old = old_time.and_then(|time| self.executions.remove(&(time, e.id.clone())));
        self.identities.insert(e.id.clone(), e.time_ms);
        self.executions.insert((e.time_ms, e.id.clone()), e.clone());
        self.changed(Change::Print {
            old: old.as_ref().map(Print::from),
            new: Some(Print::from(e)),
        });
    }
    fn changed(&mut self, change: Change) {
        self.revision += 1;
        if self.caches.is_empty() {
            return;
        }
        self.changes.push_back((self.revision, change));
        // A large history replacement invalidates caches rather than retaining
        // another full historical tape in the change journal.
        if self.changes.len() > MAX_CHANGES {
            self.caches.clear();
            self.changes.clear();
            self.cached_rows = 0;
        }
    }

    pub fn calculate(
        &mut self,
        spec: &Spec,
        tick: Decimal,
        visible: (i64, i64),
        now_ms: i64,
    ) -> Result<Vec<Snapshot>, String> {
        spec.validate()?;
        if tick <= Decimal::ZERO {
            return Err("An authoritative positive native tick size is required".into());
        }
        if visible.0 >= visible.1 {
            return Err("Visible range must be nonempty".into());
        }
        if (spec.kind == Kind::Tpo || spec.estimate_missing)
            && let Some(error) = &self.bar_error
        {
            return Err(error.clone());
        }
        let SessionRanges {
            bounds,
            output_start,
            truncated,
        } = self.ranges(spec, visible)?;
        self.cycle = self.cycle.wrapping_add(1);
        // Evict only inactive configurations, before any indices are borrowed.
        // Repeated live calculations keep all their contribution caches.
        if self.caches.len() > MAX_CACHES * 3 / 4 || self.cached_rows > MAX_CACHED_ROWS * 3 / 4 {
            self.caches
                .retain(|c| self.cycle.wrapping_sub(c.last_used) < 64);
            self.cached_rows = self.caches.iter().map(Formation::cached_rows).sum();
        }
        let mut formations = Vec::with_capacity(bounds.len());
        for (start, end) in bounds {
            validate_bounds(spec.kind, start, end)?;
            let key = Key {
                kind: spec.kind,
                start,
                end,
                block: spec.block_minutes,
                width: spec.ticks_per_row,
                tick,
                clip: None,
                estimate_missing: spec.kind == Kind::Volume && spec.estimate_missing,
            };
            let index = self.formation(key, now_ms)?;
            let complete =
                now_ms >= end && coverage_reason(self.coverage(spec.kind), start, end).is_none();
            formations.push((index, self.caches[index].poc, complete));
        }
        // Older origins contribute only reference metadata. Do not allocate
        // and copy their histograms on every live/inspection calculation.
        let first_output = formations
            .partition_point(|(index, _, _)| self.caches[*index].key.start < output_start);
        let mut snapshots = Vec::with_capacity(formations.len() - first_output);
        for &(index, poc_index, _) in &formations[first_output..] {
            snapshots.push(self.snapshot(&self.caches[index], spec, now_ms, poc_index)?);
        }
        if matches!(spec.range, Range::Session(_)) {
            for i in 0..snapshots.len() {
                let end = if i + 1 == snapshots.len() {
                    now_ms
                } else {
                    snapshots[i].end_ms.min(now_ms)
                };
                // Replay lifecycle state after corrections. Only certified
                // closed formations may mint naked levels.
                let mut levels = Vec::new();
                for &(index, poc_index, complete) in &formations[..first_output + i] {
                    if !complete {
                        continue;
                    }
                    let key = self.caches[index].key.clone();
                    if let Some(row) = poc_index {
                        let cap = if i + 1 == snapshots.len() {
                            i64::MAX
                        } else {
                            snapshots[i].end_ms
                        };
                        let touch = self.touch(row, row, key.end, end, cap, &key)?;
                        if touch != Touch::Filled {
                            let (low, high, price) = prices(&key, row)?;
                            levels.push(Level {
                                price,
                                low,
                                high,
                                origin_ms: key.start,
                                touch,
                            });
                        }
                    }
                }
                snapshots[i].naked_pocs = levels;
                if truncated && spec.display.naked {
                    append_reason(
                        &mut snapshots[i].reason,
                        "Naked reference history is limited to 33 sessions",
                    );
                }
            }
        }
        // Every range's SP/tail lifecycle extends to the observation horizon;
        // previous/naked references above remain session-only.
        for (snapshot, &(index, _, _)) in snapshots.iter_mut().zip(&formations[first_output..]) {
            self.mitigate_zones(snapshot, index, now_ms)?;
        }
        if let Some(min_revision) = self.caches.iter().map(|c| c.revision).min() {
            while self
                .changes
                .front()
                .is_some_and(|(r, _)| *r <= min_revision)
            {
                self.changes.pop_front();
            }
        }
        Ok(snapshots)
    }
    fn ranges(&self, spec: &Spec, visible: (i64, i64)) -> Result<SessionRanges, String> {
        match spec.range {
            Range::Fixed { start_ms, end_ms } => Ok(SessionRanges {
                bounds: vec![(start_ms, end_ms)],
                output_start: start_ms,
                truncated: false,
            }),
            Range::Visible => {
                validate_bounds(spec.kind, visible.0, visible.1)?;
                Ok(SessionRanges {
                    bounds: vec![visible],
                    output_start: visible.0,
                    truncated: false,
                })
            }
            Range::Session(period) => {
                let first = checked_period_bounds(period, visible.0)?.0;
                let output_start = checked_period_bounds(
                    period,
                    first.checked_sub(1).ok_or("Period timestamp underflow")?,
                )?
                .0;
                let last = checked_period_bounds(
                    period,
                    visible
                        .1
                        .checked_sub(1)
                        .ok_or("Period timestamp underflow")?,
                )?;
                let earliest = match spec.kind {
                    Kind::Tpo => self
                        .bars
                        .first_key_value()
                        .and_then(|(&t, _)| t.checked_mul(1000)),
                    Kind::Volume => {
                        let execution = self.executions.first_key_value().map(|((t, _), _)| *t);
                        let candle = spec
                            .estimate_missing
                            .then(|| {
                                self.bars
                                    .first_key_value()
                                    .and_then(|(&t, _)| t.checked_mul(1000))
                            })
                            .flatten();
                        execution.into_iter().chain(candle).min()
                    }
                }
                .unwrap_or(output_start)
                .min(output_start);
                let mut bounds = Vec::new();
                let mut current = last;
                while current.1 > earliest && bounds.len() < MAX_SESSIONS {
                    bounds.push(current);
                    let previous = checked_period_bounds(
                        period,
                        current
                            .0
                            .checked_sub(1)
                            .ok_or("Period timestamp underflow")?,
                    )?;
                    if previous.0 >= current.0 {
                        return Err("Session calendar did not advance backward".into());
                    }
                    current = previous;
                }
                let end = bounds.last().map_or(last.0, |bound| bound.0);
                if end > output_start {
                    return Err("Profile range exceeds the 32 visible sessions plus one preceding reference limit".into());
                }
                let truncated = end > earliest;
                bounds.reverse();
                Ok(SessionRanges {
                    bounds,
                    output_start,
                    truncated,
                })
            }
        }
    }
    fn formation(&mut self, key: Key, now: i64) -> Result<usize, String> {
        let horizon = now.clamp(key.start, key.end);
        if let Some(index) = self.caches.iter().position(|c| c.key == key) {
            self.caches[index].last_used = self.cycle;
            if self.caches[index].revision == self.revision && self.caches[index].horizon == horizon
            {
                return Ok(index);
            }
            let old_rows = self.caches[index].cached_rows();
            // A backwards inspection rebuilds only this contribution, never a
            // historical tape clone. Forward/live updates replay changed rows.
            if horizon < self.caches[index].horizon {
                self.caches[index].rows.clear();
                self.caches[index].dirty = true;
                self.caches[index].ib_dirty = true;
                self.caches[index].horizon = key.start;
                self.caches[index].revision = self.revision;
                self.caches[index].estimates.clear();
                self.caches[index].estimated_quantity = Decimal::ZERO;
            }
            let result = (|| {
                if key.estimate_missing {
                    self.update_estimated(index, horizon, now)?;
                } else {
                    let revision = self.caches[index].revision;
                    let first = self.changes.partition_point(|(r, _)| *r <= revision);
                    for (_, change) in self.changes.range(first..) {
                        self.caches[index].apply(change)?;
                    }
                    let old_horizon = self.caches[index].horizon;
                    self.caches[index].horizon = horizon;
                    match key.kind {
                        Kind::Tpo => {
                            let start = ceil_seconds(old_horizon);
                            let end = ceil_seconds(horizon);
                            for (_, bar) in self.bars.range(start..end) {
                                self.caches[index].bar(bar, true)?;
                            }
                        }
                        Kind::Volume => {
                            for (_, e) in self
                                .executions
                                .range((old_horizon, String::new())..(horizon, String::new()))
                            {
                                self.caches[index].print(Print::from(e), true)?;
                            }
                        }
                    }
                }
                if self.caches[index].dirty && key.clip.is_none() {
                    self.caches[index].poc = poc_row(&self.caches[index].rows, key.kind)?;
                    if self.caches[index].ib_dirty {
                        self.caches[index].initial_balance =
                            native_initial_balance(&self.bars, &key, horizon);
                        self.caches[index].ib_dirty = false;
                    }
                }
                self.caches[index].dirty = false;
                self.caches[index].revision = self.revision;
                Ok::<_, String>(())
            })();
            if let Err(error) = result {
                // Failed incremental updates must not leave half-applied rows
                // available for a subsequent calculation.
                self.cached_rows -= old_rows;
                self.caches.remove(index);
                return Err(error);
            }
            self.cached_rows = self.cached_rows - old_rows + self.caches[index].cached_rows();
            if self.cached_rows > MAX_CACHED_ROWS {
                self.cached_rows -= self.caches[index].cached_rows();
                self.caches.remove(index);
                return Err(format!(
                    "Profile contribution row budget ({MAX_CACHED_ROWS}) exceeded"
                ));
            }
            return Ok(index);
        }
        if self.caches.len() >= MAX_CACHES {
            return Err(format!(
                "Profile contribution cache limit ({MAX_CACHES}) exceeded"
            ));
        }
        let mut formation = Formation {
            key,
            revision: self.revision,
            horizon,
            last_used: self.cycle,
            rows: BTreeMap::new(),
            poc: None,
            dirty: false,
            initial_balance: None,
            ib_dirty: false,
            estimates: BTreeMap::new(),
            estimated_quantity: Decimal::ZERO,
        };
        match formation.key.kind {
            Kind::Tpo => {
                let start = ceil_seconds(formation.key.start);
                let end = ceil_seconds(horizon);
                for (_, bar) in self.bars.range(start..end) {
                    formation.bar(bar, true)?;
                }
            }
            Kind::Volume if formation.key.estimate_missing => {
                let first = minute_start(formation.key.start);
                for (&time, _) in self
                    .bars
                    .range(first.div_euclid(1000)..ceil_seconds(horizon))
                {
                    let minute = time.checked_mul(1000).ok_or("Bar timestamp overflow")?;
                    if let Some(estimate) = bucket_estimate(
                        &self.bars,
                        &self.proven_executions,
                        &formation,
                        minute,
                        now,
                    )? {
                        formation.estimate(estimate, true)?;
                        formation.estimates.insert(minute, estimate);
                    }
                }
                for (_, e) in self
                    .executions
                    .range((formation.key.start, String::new())..(horizon, String::new()))
                {
                    if !formation.estimates.contains_key(&minute_start(e.time_ms)) {
                        formation.print(Print::from(e), true)?;
                    }
                }
            }
            Kind::Volume => {
                for (_, e) in self
                    .executions
                    .range((formation.key.start, String::new())..(horizon, String::new()))
                {
                    formation.print(Print::from(e), true)?;
                }
            }
        }
        if formation.dirty && formation.key.clip.is_none() {
            formation.poc = poc_row(&formation.rows, formation.key.kind)?;
            formation.initial_balance = native_initial_balance(&self.bars, &formation.key, horizon);
            formation.ib_dirty = false;
        }
        formation.dirty = false;
        if self.cached_rows + formation.cached_rows() > MAX_CACHED_ROWS {
            return Err(format!(
                "Profile contribution row budget ({MAX_CACHED_ROWS}) exceeded"
            ));
        }
        self.cached_rows += formation.cached_rows();
        self.caches.push(formation);
        Ok(self.caches.len() - 1)
    }
    fn update_estimated(&mut self, index: usize, horizon: i64, now: i64) -> Result<(), String> {
        let formation = &mut self.caches[index];
        let mut changed = BTreeSet::new();
        let first = self
            .changes
            .partition_point(|(r, _)| *r <= formation.revision);
        for (_, change) in self.changes.range(first..) {
            match change {
                Change::Print { old, new } => {
                    for (print, add) in old
                        .iter()
                        .map(|p| (p, false))
                        .chain(new.iter().map(|p| (p, true)))
                    {
                        if !formation.estimates.contains_key(&minute_start(print.time)) {
                            formation.print(*print, add)?;
                        }
                    }
                }
                Change::Bar { new, .. } => {
                    let minute = new.time.checked_mul(1000).ok_or("Bar timestamp overflow")?;
                    changed.insert(minute);
                }
                Change::Proof { start, end } => {
                    let start = minute_start((*start).max(formation.key.start));
                    let end = (*end).min(horizon);
                    if start < end {
                        for (&time, _) in self.bars.range(start.div_euclid(1000)..ceil_seconds(end))
                        {
                            changed.insert(time.checked_mul(1000).ok_or("Bar timestamp overflow")?);
                        }
                    }
                }
            }
        }
        let old_horizon = formation.horizon;
        formation.horizon = horizon;
        for (_, e) in self
            .executions
            .range((old_horizon, String::new())..(horizon, String::new()))
        {
            if !formation.estimates.contains_key(&minute_start(e.time_ms)) {
                formation.print(Print::from(e), true)?;
            }
        }
        if old_horizon < horizon {
            let start = minute_start(old_horizon);
            for (&time, _) in self
                .bars
                .range(start.div_euclid(1000)..ceil_seconds(horizon))
            {
                changed.insert(time.checked_mul(1000).ok_or("Bar timestamp overflow")?);
            }
        }
        for minute in changed {
            if minute >= horizon || minute.saturating_add(MINUTE) <= formation.key.start {
                continue;
            }
            let next =
                bucket_estimate(&self.bars, &self.proven_executions, formation, minute, now)?;
            if formation.estimates.get(&minute).copied() == next {
                continue;
            }
            let start = minute.max(formation.key.start);
            let end = minute.saturating_add(MINUTE).min(horizon);
            if let Some(old) = formation.estimates.remove(&minute) {
                formation.estimate(old, false)?;
            } else {
                for (_, e) in self
                    .executions
                    .range((start, String::new())..(end, String::new()))
                {
                    formation.print(Print::from(e), false)?;
                }
            }
            if let Some(estimate) = next {
                formation.estimate(estimate, true)?;
                formation.estimates.insert(minute, estimate);
            } else {
                for (_, e) in self
                    .executions
                    .range((start, String::new())..(end, String::new()))
                {
                    formation.print(Print::from(e), true)?;
                }
            }
        }
        Ok(())
    }
    fn coverage(&self, kind: Kind) -> &[Coverage] {
        match kind {
            Kind::Tpo => &self.bar_coverage,
            Kind::Volume => &self.execution_coverage,
        }
    }
    fn snapshot(
        &self,
        formation: &Formation,
        spec: &Spec,
        now: i64,
        poc_index: Option<i64>,
    ) -> Result<Snapshot, String> {
        let key = &formation.key;
        let as_of = now.min(key.end);
        let proof_end = if key.kind == Kind::Tpo && now < key.end {
            as_of.div_euclid(MINUTE).saturating_mul(MINUTE)
        } else {
            as_of
        };
        let mut source_reason = coverage_reason(self.coverage(key.kind), key.start, proof_end);
        if now < key.start {
            append_reason(&mut source_reason, "Formation has not started");
        } else if key.kind == Kind::Tpo
            && as_of > proof_end
            && !self.bars.contains_key(&proof_end.div_euclid(1000))
        {
            append_reason(
                &mut source_reason,
                "Waiting for the current native 1-minute bar",
            );
        }
        let covered = source_reason.is_none();
        let complete = now >= key.end && covered;
        let mut reason = source_reason;
        if now >= key.start && now < key.end {
            append_reason(
                &mut reason,
                "Formation is open; levels and structure are provisional",
            );
        }
        if !formation.estimates.is_empty() {
            append_reason(
                &mut reason,
                "Candle-volume estimate: unproved native minutes are distributed uniformly across price rows; partial edges are time-prorated",
            );
        }
        let mut rows = Vec::with_capacity(formation.rows.len());
        for (&index, bin) in &formation.rows {
            let (low, high, _) = prices(key, index)?;
            rows.push(Row {
                index,
                low,
                high,
                weight: decimal_f64(weight(bin, key.kind))?,
                blocks: bin.blocks.keys().copied().collect(),
            });
        }
        let (poc, val, vah, total) = if let Some(index) = poc_index {
            let (lo, hi, total) =
                value_area(&formation.rows, key.kind, index, spec.value_area_percent)?;
            (
                Some(prices(key, index)?.2),
                Some(prices(key, lo)?.0),
                Some(prices(key, hi)?.1),
                total,
            )
        } else {
            (None, None, None, Decimal::ZERO)
        };
        let mut single_prints = Vec::new();
        let mut tails = Vec::new();
        if key.kind == Kind::Tpo {
            for (low, high, block, interior) in single_runs(&formation.rows) {
                let zone = Zone {
                    low: prices(key, low)?.0,
                    high: prices(key, high)?.1,
                    touch: if complete {
                        Touch::Untouched
                    } else {
                        Touch::Unknown
                    },
                    block: Some(block),
                };
                if interior {
                    single_prints.push(zone);
                } else {
                    tails.push(zone);
                }
            }
        }
        let initial_balance = formation.initial_balance;
        let low = rows.first().map(|r| r.low);
        let high = rows.last().map(|r| r.high);
        let poor_low = key.kind == Kind::Tpo
            && formation
                .rows
                .first_key_value()
                .is_some_and(|(_, b)| b.blocks.len() >= 2);
        let poor_high = key.kind == Kind::Tpo
            && formation
                .rows
                .last_key_value()
                .is_some_and(|(_, b)| b.blocks.len() >= 2);
        Ok(Snapshot {
            start_ms: key.start,
            end_ms: key.end,
            as_of_ms: as_of,
            complete,
            covered,
            reason,
            rows,
            native_volume: (key.kind == Kind::Volume).then_some(total),
            volume_method: if formation.estimates.is_empty() {
                VolumeMethod::Exact
            } else if total == formation.estimated_quantity {
                VolumeMethod::Estimated
            } else {
                VolumeMethod::Mixed
            },
            poc,
            vah,
            val,
            single_prints,
            tails,
            naked_pocs: Vec::new(),
            initial_balance,
            high,
            low,
            poor_high,
            poor_low,
        })
    }
    fn mitigate_zones(
        &mut self,
        snapshot: &mut Snapshot,
        index: usize,
        now: i64,
    ) -> Result<(), String> {
        if !snapshot.complete {
            return Ok(());
        }
        let key = self.caches[index].key.clone();
        // Keep integer row identities throughout lifecycle calculations. A
        // rendered f64 boundary cannot safely be converted back to a tick.
        let runs = single_runs(&self.caches[index].rows);
        let mut singles = snapshot.single_prints.iter_mut();
        let mut tails = snapshot.tails.iter_mut();
        for (low, high, _, interior) in runs {
            let zone = if interior {
                singles.next()
            } else {
                tails.next()
            };
            if let Some(zone) = zone {
                zone.touch = self.touch(low, high, key.end, now, i64::MAX, &key)?;
            }
        }
        Ok(())
    }
    fn touch(
        &mut self,
        low: i64,
        high: i64,
        start: i64,
        end: i64,
        cap: i64,
        key: &Key,
    ) -> Result<Touch, String> {
        if end <= start {
            return Ok(Touch::Untouched);
        }
        check_span(low, high)?;
        let touch_key = Key {
            start,
            end: cap,
            clip: Some((low, high)),
            estimate_missing: false,
            ..key.clone()
        };
        let index = self.formation(touch_key, end)?;
        let count = self.caches[index].rows.len();
        if count as i128 == high as i128 - low as i128 + 1 {
            return Ok(Touch::Filled);
        }
        // Positive observations survive coverage gaps. Unknown means no
        // positive evidence, never erasure of an already observed row touch.
        if count > 0 {
            return Ok(Touch::Partial);
        }
        if coverage_reason(self.coverage(key.kind), start, end).is_some() {
            Ok(Touch::Unknown)
        } else {
            Ok(Touch::Untouched)
        }
    }
}

fn native_initial_balance(
    bars: &BTreeMap<i64, Candle>,
    key: &Key,
    horizon: i64,
) -> Option<(f64, f64)> {
    if key.kind != Kind::Tpo || key.clip.is_some() {
        return None;
    }
    let end = key
        .start
        .saturating_add(i64::from(key.block) * 2 * MINUTE)
        .min(key.end)
        .min(horizon);
    bars.range(ceil_seconds(key.start)..ceil_seconds(end))
        .fold(None, |bounds, (_, bar)| {
            Some(
                bounds.map_or((bar.low, bar.high), |(low, high): (f64, f64)| {
                    (low.min(bar.low), high.max(bar.high))
                }),
            )
        })
}

impl Formation {
    fn cached_rows(&self) -> usize {
        // Minute replacements retain source contributions even when zero-volume
        // estimates have no occupied price rows. Charge them to the same budget.
        self.rows.len() + self.estimates.len()
    }

    fn estimate(&mut self, estimate: Estimate, add: bool) -> Result<(), String> {
        check_span(estimate.low, estimate.high)?;
        let count = (estimate.high as i128 - estimate.low as i128 + 1) as u128;
        // Keep every input decimal unit, adding up to eight fractional digits
        // for uniform splitting. Reserve coefficient headroom for the bounded
        // source population instead of filling Decimal's 96 bits per candle.
        let mut units = estimate.quantity.mantissa() as u128;
        let mut scale = estimate.quantity.scale();
        let target_scale = (scale + 8).min(28);
        let limit = ((1u128 << 96) - 1) / (MAX_BARS + MAX_EXECUTIONS) as u128;
        while scale < target_scale && units <= limit / 10 {
            units *= 10;
            scale += 1;
        }
        let share = units / count;
        let remainder = units % count;
        for (offset, index) in (estimate.low..=estimate.high).enumerate() {
            let units = share + u128::from((offset as u128) < remainder);
            if units == 0 {
                continue;
            }
            let quantity = Decimal::from_i128_with_scale(units as i128, scale);
            if add {
                if !self.rows.contains_key(&index) && self.rows.len() >= MAX_ROWS {
                    return Err(
                        "Profile exceeds 200000 occupied rows; choose an explicit wider row".into(),
                    );
                }
                let bin = self.rows.entry(index).or_default();
                bin.quantity = exact_add(bin.quantity, quantity)?;
            } else if let Some(bin) = self.rows.get_mut(&index) {
                bin.quantity = exact_add(bin.quantity, -quantity)?;
                if bin.quantity == Decimal::ZERO {
                    self.rows.remove(&index);
                }
            }
        }
        self.estimated_quantity = exact_add(
            self.estimated_quantity,
            if add {
                estimate.quantity
            } else {
                -estimate.quantity
            },
        )?;
        self.dirty = true;
        Ok(())
    }
    fn apply(&mut self, change: &Change) -> Result<(), String> {
        match change {
            Change::Bar { old, new } if self.key.kind == Kind::Tpo => {
                if let Some(old) = old {
                    self.bar(old, false)?;
                }
                self.bar(new, true)?;
            }
            Change::Print { old, new } if self.key.kind == Kind::Volume => {
                if let Some(old) = old {
                    self.print(*old, false)?;
                }
                if let Some(new) = new {
                    self.print(*new, true)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    fn bar(&mut self, bar: &Candle, add: bool) -> Result<(), String> {
        let time = bar.time.checked_mul(1000).ok_or("Bar timestamp overflow")?;
        if time < self.key.start || time >= self.key.end || time >= self.horizon {
            return Ok(());
        }
        if time.rem_euclid(MINUTE) != 0 {
            return Err("TPO requires minute-aligned native 1m bars".into());
        }
        let duration = i64::from(self.key.block) * MINUTE;
        let block = if self.key.clip.is_some() {
            0
        } else {
            (time - self.key.start) / duration
        };
        let block_id =
            u32::try_from(block).map_err(|_| "Profile exceeds the supported TPO block count")?;
        let close = time.checked_add(MINUTE).ok_or("Bar timestamp overflow")?;
        if close > self.key.end
            || (self.key.clip.is_none() && (close - 1 - self.key.start) / duration != block)
        {
            return Err("A native bar straddles a TPO block or range boundary".into());
        }
        let (mut lo, mut hi) = candle_rows(bar, &self.key)?;
        if let Some((low, high)) = self.key.clip {
            lo = lo.max(low);
            hi = hi.min(high);
        }
        if lo > hi {
            return Ok(());
        }
        check_span(lo, hi)?;
        self.dirty = true;
        if block_id < 2 && self.key.clip.is_none() {
            self.ib_dirty = true;
        }
        for index in lo..=hi {
            if add {
                if !self.rows.contains_key(&index) && self.rows.len() >= MAX_ROWS {
                    return Err(
                        "Profile exceeds 200000 occupied rows; choose an explicit wider row".into(),
                    );
                }
                let bin = self.rows.entry(index).or_default();
                *bin.blocks.entry(block_id).or_default() += 1;
            } else if let Some(bin) = self.rows.get_mut(&index) {
                if let Some(count) = bin.blocks.get_mut(&block_id) {
                    *count -= 1;
                    if *count == 0 {
                        bin.blocks.remove(&block_id);
                    }
                }
                if bin.blocks.is_empty() {
                    self.rows.remove(&index);
                }
            }
        }
        Ok(())
    }
    fn print(&mut self, print: Print, add: bool) -> Result<(), String> {
        if print.time < self.key.start || print.time >= self.key.end || print.time >= self.horizon {
            return Ok(());
        }
        let index = row_index(print.price, &self.key)?;
        if self
            .key
            .clip
            .is_some_and(|(low, high)| index < low || index > high)
        {
            return Ok(());
        }
        self.dirty = true;
        if add {
            if !self.rows.contains_key(&index) && self.rows.len() >= MAX_ROWS {
                return Err(
                    "Profile exceeds 200000 occupied rows; choose an explicit wider row".into(),
                );
            }
            let bin = self.rows.entry(index).or_default();
            bin.quantity = exact_add(bin.quantity, print.quantity)?;
        } else if let Some(bin) = self.rows.get_mut(&index) {
            bin.quantity = bin
                .quantity
                .checked_sub(print.quantity)
                .ok_or("Native quantity subtraction overflow")?;
            if bin.quantity == Decimal::ZERO {
                self.rows.remove(&index);
            }
        }
        Ok(())
    }
}
fn minute_start(time: i64) -> i64 {
    time.div_euclid(MINUTE).saturating_mul(MINUTE)
}

fn bucket_estimate(
    bars: &BTreeMap<i64, Candle>,
    proven: &[(i64, i64)],
    formation: &Formation,
    minute: i64,
    now: i64,
) -> Result<Option<Estimate>, String> {
    if minute.rem_euclid(MINUTE) != 0 {
        return Ok(None);
    }
    let close = minute.checked_add(MINUTE).ok_or("Bar timestamp overflow")?;
    let start = minute.max(formation.key.start);
    let end = close.min(formation.horizon);
    if start >= end {
        return Ok(None);
    }
    let index = proven.partition_point(|&(_, hi)| hi <= start);
    if proven
        .get(index)
        .is_some_and(|&(lo, hi)| lo <= start && hi >= end)
    {
        return Ok(None);
    }
    let Some(bar) = bars.get(&minute.div_euclid(1000)) else {
        return Ok(None);
    };
    if !bar.volume.is_finite()
        || bar.volume < 0.0
        || !bar.low.is_finite()
        || !bar.high.is_finite()
        || bar.low > bar.high
        || !bar.open.is_finite()
        || !bar.close.is_finite()
        || bar.open < bar.low
        || bar.open > bar.high
        || bar.close < bar.low
        || bar.close > bar.high
    {
        return Ok(None);
    }
    let Some(mut quantity) = Decimal::from_f64(bar.volume) else {
        return Ok(None);
    };
    // A forming candle reports elapsed volume, not a forecast of its minute.
    // Only the requested fraction of that elapsed interval may contribute.
    let duration = if now < close {
        now.saturating_sub(minute)
    } else {
        MINUTE
    };
    if duration <= 0 {
        return Ok(None);
    }
    let overlap = end - start;
    if overlap < duration {
        quantity = prorated_quantity(quantity, overlap as u128, duration as u128);
    }
    let (low, high) = candle_rows(bar, &formation.key)?;
    check_span(low, high)?;
    Ok(Some(Estimate {
        low,
        high,
        quantity,
    }))
}

fn prorated_quantity(quantity: Decimal, overlap: u128, duration: u128) -> Decimal {
    // Native minute durations fit 16 bits, so a 96-bit Decimal coefficient
    // times the duration fits u128 even when Decimal multiplication would not.
    let limit = ((1u128 << 96) - 1) / (MAX_BARS + MAX_EXECUTIONS) as u128 * duration;
    let mut numerator = quantity.mantissa() as u128 * overlap;
    let mut scale = quantity.scale();
    let target_scale = (scale + 8).min(28);
    while scale < target_scale && numerator <= limit / 10 {
        numerator *= 10;
        scale += 1;
    }
    let mut units = numerator / duration;
    let remainder_twice = (numerator % duration) * 2;
    if remainder_twice > duration || (remainder_twice == duration && !units.is_multiple_of(2)) {
        units += 1;
    }
    Decimal::from_i128_with_scale(units as i128, scale)
}

/// Canonical certified execution intervals, subtracting explicit invalidations.
fn proven_intervals(coverage: &[Coverage]) -> Vec<(i64, i64)> {
    let mut events = Vec::with_capacity(coverage.len() * 2);
    for c in coverage.iter().filter(|c| c.start_ms < c.end_ms) {
        let delta = if c.complete { (1, 0) } else { (0, 1) };
        events.push((c.start_ms, delta.0, delta.1));
        events.push((c.end_ms, -delta.0, -delta.1));
    }
    sweep_intervals(events, false)
}

/// Only proof transitions enter the revision journal; advancing one live
/// coverage endpoint does not rebuild the certified historical profile.
fn changed_intervals(old: &[(i64, i64)], new: &[(i64, i64)]) -> Vec<(i64, i64)> {
    let mut events = Vec::with_capacity((old.len() + new.len()) * 2);
    for &(start, end) in old {
        events.push((start, 1, 0));
        events.push((end, -1, 0));
    }
    for &(start, end) in new {
        events.push((start, 0, 1));
        events.push((end, 0, -1));
    }
    sweep_intervals(events, true)
}

fn sweep_intervals(mut events: Vec<(i64, i32, i32)>, changed: bool) -> Vec<(i64, i64)> {
    events.sort_unstable_by_key(|event| event.0);
    let mut result: Vec<(i64, i64)> = Vec::new();
    let (mut first, mut second) = (0, 0);
    let mut previous = None;
    for (time, a, b) in events {
        let include = if changed {
            (first > 0) != (second > 0)
        } else {
            first > 0 && second == 0
        };
        if let Some(start) = previous
            && start < time
            && include
        {
            if let Some(last) = result.last_mut()
                && last.1 == start
            {
                last.1 = time;
            } else {
                result.push((start, time));
            }
        }
        first += a;
        second += b;
        previous = Some(time);
    }
    result
}

fn ceil_seconds(time_ms: i64) -> i64 {
    time_ms.div_euclid(1000) + i64::from(time_ms.rem_euclid(1000) != 0)
}
fn validate_bounds(kind: Kind, start: i64, end: i64) -> Result<(), String> {
    if start >= end {
        return Err("Profile range must have start before end".into());
    }
    if kind == Kind::Tpo && (start.rem_euclid(MINUTE) != 0 || end.rem_euclid(MINUTE) != 0) {
        return Err("TPO range endpoints must be aligned to UTC minutes".into());
    }
    if end.checked_sub(start).is_none() {
        return Err("Profile range duration overflow".into());
    }
    Ok(())
}
fn check_span(low: i64, high: i64) -> Result<(), String> {
    if high < low || high as i128 - low as i128 + 1 > MAX_ROWS as i128 {
        return Err("Profile price span exceeds 200000 rows; choose an explicit wider row".into());
    }
    Ok(())
}
fn row_index(price: Decimal, key: &Key) -> Result<i64, String> {
    let width = key
        .tick
        .checked_mul(Decimal::from(key.width))
        .ok_or("Native row width overflow")?;
    // Decimal division rounds at scale 28. Remove the exact remainder first,
    // so a price infinitesimally below a boundary cannot round up a row.
    let remainder = price
        .checked_rem(width)
        .ok_or("Native tick remainder overflow")?;
    let integer = price
        .checked_sub(remainder)
        .and_then(|v| v.checked_div(width))
        .and_then(|v| v.to_i64())
        .ok_or("Native row index overflow")?;
    if remainder < Decimal::ZERO {
        integer
            .checked_sub(1)
            .ok_or("Native row index overflow".into())
    } else {
        Ok(integer)
    }
}
fn candle_rows(bar: &Candle, key: &Key) -> Result<(i64, i64), String> {
    if !bar.low.is_finite() || !bar.high.is_finite() || bar.low > bar.high {
        return Err("Native bar has invalid price extrema".into());
    }
    let low = Decimal::from_f64(bar.low).ok_or("Native bar low is outside decimal range")?;
    let high = Decimal::from_f64(bar.high).ok_or("Native bar high is outside decimal range")?;
    Ok((row_index(low, key)?, row_index(high, key)?))
}
fn decimal_f64(value: Decimal) -> Result<f64, String> {
    value
        .to_f64()
        .filter(|v| v.is_finite())
        .ok_or("Profile value is outside rendering range".into())
}
fn prices(key: &Key, index: i64) -> Result<(f64, f64, f64), String> {
    let base = Decimal::from(index)
        .checked_mul(Decimal::from(key.width))
        .ok_or("Native tick index overflow")?;
    let low = base.checked_mul(key.tick).ok_or("Native price overflow")?;
    let high = base
        .checked_add(Decimal::from(key.width - 1))
        .and_then(|v| v.checked_mul(key.tick))
        .ok_or("Native price overflow")?;
    let middle = base
        .checked_add(Decimal::from((key.width - 1) / 2))
        .and_then(|v| v.checked_mul(key.tick))
        .ok_or("Native price overflow")?;
    Ok((decimal_f64(low)?, decimal_f64(high)?, decimal_f64(middle)?))
}
fn weight(bin: &Bin, kind: Kind) -> Decimal {
    match kind {
        Kind::Tpo => Decimal::from(bin.blocks.len() as u64),
        Kind::Volume => bin.quantity,
    }
}
fn poc_row(rows: &BTreeMap<i64, Bin>, kind: Kind) -> Result<Option<i64>, String> {
    let Some((&low, _)) = rows.first_key_value() else {
        return Ok(None);
    };
    let high = *rows.last_key_value().expect("nonempty").0;
    let midpoint_twice = low as i128 + high as i128;
    Ok(rows
        .iter()
        .max_by(|(ai, a), (bi, b)| {
            weight(a, kind)
                .cmp(&weight(b, kind))
                .then_with(|| {
                    ((**bi as i128 * 2) - midpoint_twice)
                        .abs()
                        .cmp(&((**ai as i128 * 2) - midpoint_twice).abs())
                })
                .then_with(|| bi.cmp(ai))
        })
        .map(|(&index, _)| index))
}
fn value_area(
    rows: &BTreeMap<i64, Bin>,
    kind: Kind,
    poc: i64,
    percent: u8,
) -> Result<(i64, i64, Decimal), String> {
    let mut total = Decimal::ZERO;
    for bin in rows.values() {
        total = exact_add(total, weight(bin, kind))?;
    }
    let mut low = poc;
    let mut high = poc;
    let mut covered = weight(&rows[&poc], kind);
    let min = *rows.first_key_value().expect("nonempty").0;
    let max = *rows.last_key_value().expect("nonempty").0;
    check_span(min, max)?;
    while !percent_reached(covered, total, percent) && (low > min || high < max) {
        let lower = if low > min {
            rows.get(&(low - 1))
                .map(|b| weight(b, kind))
                .unwrap_or(Decimal::ZERO)
        } else {
            Decimal::NEGATIVE_ONE
        };
        let upper = if high < max {
            rows.get(&(high + 1))
                .map(|b| weight(b, kind))
                .unwrap_or(Decimal::ZERO)
        } else {
            Decimal::NEGATIVE_ONE
        };
        let next = if lower >= upper {
            low -= 1;
            lower
        } else {
            high += 1;
            upper
        };
        covered = exact_add(covered, next)?;
    }
    Ok((low, high, total))
}
fn single_runs(rows: &BTreeMap<i64, Bin>) -> Vec<(i64, i64, u32, bool)> {
    let mut result = Vec::new();
    let mut pending: Vec<(i64, i64, u32)> = Vec::new();
    let finish = |pending: &mut Vec<(i64, i64, u32)>, result: &mut Vec<_>| {
        let (Some(first), Some(last)) = (pending.first(), pending.last()) else {
            return;
        };
        let lo = first.0;
        let hi = last.1;
        let below = lo
            .checked_sub(1)
            .and_then(|i| rows.get(&i))
            .is_some_and(|b| b.blocks.len() >= 2);
        let above = hi
            .checked_add(1)
            .and_then(|i| rows.get(&i))
            .is_some_and(|b| b.blocks.len() >= 2);
        let interior = below && above;
        let tail = (above && rows.first_key_value().is_some_and(|(&i, _)| i == lo))
            || (below && rows.last_key_value().is_some_and(|(&i, _)| i == hi));
        if interior || tail {
            result.extend(
                pending
                    .drain(..)
                    .map(|(lo, hi, owner)| (lo, hi, owner, interior)),
            );
        } else {
            pending.clear();
        }
    };
    for (&index, bin) in rows {
        if bin.blocks.len() != 1 {
            finish(&mut pending, &mut result);
            continue;
        }
        let owner = *bin.blocks.first_key_value().expect("one block").0;
        if pending
            .last()
            .is_some_and(|(_, hi, _)| hi.checked_add(1) != Some(index))
        {
            finish(&mut pending, &mut result);
        }
        if let Some((_, hi, previous_owner)) = pending.last_mut()
            && *previous_owner == owner
        {
            *hi = index;
            continue;
        }
        pending.push((index, index, owner));
    }
    finish(&mut pending, &mut result);
    result
}
fn exact_add(a: Decimal, b: Decimal) -> Result<Decimal, String> {
    let sum = a.checked_add(b).ok_or("Native quantity sum overflow")?;
    if sum.checked_sub(a) != Some(b) || sum.checked_sub(b) != Some(a) {
        return Err("Native quantity sum cannot be represented exactly".into());
    }
    Ok(sum)
}
// Decimal's rounded division can prematurely satisfy a VA target for tiny
// quantities. Compare exact fixed-width integer products instead, on stack.
fn percent_reached(covered: Decimal, total: Decimal, percent: u8) -> bool {
    fn scaled(value: Decimal, factor: u32, scale: u32) -> [u64; 4] {
        let mantissa = value.mantissa() as u128;
        let mut words = [mantissa as u64, (mantissa >> 64) as u64, 0, 0];
        for multiplier in
            std::iter::once(factor).chain(std::iter::repeat_n(10, (scale - value.scale()) as usize))
        {
            let mut carry = 0u128;
            for word in &mut words {
                let product = u128::from(*word) * u128::from(multiplier) + carry;
                *word = product as u64;
                carry = product >> 64;
            }
        }
        words
    }
    let scale = covered.scale().max(total.scale());
    scaled(covered, 100, scale)
        .iter()
        .rev()
        .cmp(scaled(total, u32::from(percent), scale).iter().rev())
        .is_ge()
}
fn append_reason(reason: &mut Option<String>, message: &str) {
    if let Some(reason) = reason {
        reason.push_str("; ");
        reason.push_str(message);
    } else {
        *reason = Some(message.into());
    }
}
fn coverage_reason(coverage: &[Coverage], start: i64, end: i64) -> Option<String> {
    if start >= end {
        return None;
    }
    if let Some(gap) = coverage
        .iter()
        .find(|c| !c.complete && c.start_ms < end && c.end_ms > start && c.start_ms < c.end_ms)
    {
        return Some(format!(
            "Native source coverage unproved from {}: {}",
            start.max(gap.start_ms),
            gap.reason
                .as_deref()
                .unwrap_or("source interval is incomplete")
        ));
    }
    let mut spans: Vec<_> = coverage
        .iter()
        .filter(|c| c.complete && c.end_ms > start && c.start_ms < end)
        .map(|c| (c.start_ms, c.end_ms))
        .collect();
    spans.sort_unstable();
    let mut through = start;
    for (lo, hi) in spans {
        if lo > through {
            break;
        }
        through = through.max(hi);
        if through >= end {
            return None;
        }
    }
    let diagnostic = coverage
        .iter()
        .rev()
        .find(|c| !c.complete && c.end_ms > through && c.start_ms < end)
        .and_then(|c| c.reason.as_deref());
    Some(match diagnostic {
        Some(reason) => format!("Native source coverage unproved from {through}: {reason}"),
        None => format!("Native source coverage unavailable from {through} to {end}"),
    })
}
/// Bounds of the latest nonempty session opening at or before `time_ms`. Custom
/// sessions may end before `time_ms` in a gap or overlap at a DST transition.
/// The latest opening wins in an overlap. Request the previous session at
/// `start - 1`, never at `end`; preserve its actual end rather than filling gaps.
/// Invalid calendar timestamps produce an empty interval; calculate rejects them.
pub fn period_bounds(period: Period, time_ms: i64) -> (i64, i64) {
    checked_period_bounds(period, time_ms).unwrap_or((time_ms, time_ms))
}
fn checked_period_bounds(period: Period, time_ms: i64) -> Result<(i64, i64), String> {
    if let Period::Custom {
        timezone,
        start_minute,
        end_minute,
    } = period
    {
        if start_minute >= 1440 || end_minute >= 1440 {
            return Err("Session minutes must be between 00:00 and 23:59".into());
        }
        let mut date = Utc
            .timestamp_millis_opt(time_ms)
            .single()
            .ok_or("Timestamp outside supported calendar")?
            .with_timezone(&timezone)
            .date_naive();
        loop {
            let bounds = custom_date_bounds(timezone, date, start_minute, end_minute)?;
            if bounds.0 <= time_ms && bounds.0 < bounds.1 {
                return Ok(bounds);
            }
            date = date.pred_opt().ok_or("Session calendar underflow")?;
        }
    }
    let date = Utc
        .timestamp_millis_opt(time_ms)
        .single()
        .ok_or("UTC timestamp outside supported calendar")?
        .date_naive();
    let start = match period {
        Period::Day => date,
        Period::Week => date
            .checked_sub_signed(Duration::days(i64::from(
                date.weekday().num_days_from_monday(),
            )))
            .ok_or("UTC week underflow")?,
        Period::Month => date.with_day(1).ok_or("Invalid UTC month")?,
        Period::Custom { .. } => unreachable!("custom sessions handled above"),
    };
    let end = match period {
        Period::Day => start.checked_add_signed(Duration::days(1)),
        Period::Week => start.checked_add_signed(Duration::days(7)),
        Period::Month => {
            if start.month() == 12 {
                start
                    .year()
                    .checked_add(1)
                    .and_then(|y| chrono::NaiveDate::from_ymd_opt(y, 1, 1))
            } else {
                chrono::NaiveDate::from_ymd_opt(start.year(), start.month() + 1, 1)
            }
        }
        Period::Custom { .. } => unreachable!("custom sessions handled above"),
    }
    .ok_or("UTC period overflow")?;
    let start = start
        .and_hms_opt(0, 0, 0)
        .ok_or("Invalid UTC day")?
        .and_utc()
        .timestamp_millis();
    let end = end
        .and_hms_opt(0, 0, 0)
        .ok_or("Invalid UTC day")?
        .and_utc()
        .timestamp_millis();
    Ok((start, end))
}

struct SessionRanges {
    bounds: Vec<(i64, i64)>,
    output_start: i64,
    truncated: bool,
}

fn custom_date_bounds(
    timezone: Tz,
    date: NaiveDate,
    start_minute: u16,
    end_minute: u16,
) -> Result<(i64, i64), String> {
    let end_date = if end_minute <= start_minute {
        date.succ_opt().ok_or("Session calendar overflow")?
    } else {
        date
    };
    let local = |date: NaiveDate, minute: u16| {
        date.and_hms_opt(u32::from(minute / 60), u32::from(minute % 60), 0)
            .ok_or_else(|| "Invalid session local time".to_string())
    };
    Ok((
        resolve_local(timezone, local(date, start_minute)?, false)?,
        resolve_local(timezone, local(end_date, end_minute)?, true)?,
    ))
}

fn resolve_local(timezone: Tz, mut local: NaiveDateTime, close: bool) -> Result<i64, String> {
    // Includes whole skipped local dates (for example Pacific/Apia in 2011).
    // The bounded search never silently falls back to an invented UTC offset.
    for _ in 0..=2880 {
        match timezone.from_local_datetime(&local) {
            LocalResult::Single(time) => return Ok(time.timestamp_millis()),
            LocalResult::Ambiguous(a, b) => {
                return Ok(if close { a.max(b) } else { a.min(b) }.timestamp_millis());
            }
            LocalResult::None => {
                local = local
                    .checked_add_signed(Duration::minutes(1))
                    .ok_or("Session calendar overflow")?;
            }
        }
    }
    Err("No valid local session minute within two days".into())
}

#[cfg(test)]
mod tests;
