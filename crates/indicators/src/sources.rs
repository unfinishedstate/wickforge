//! Base-asset native inputs and strict, per-instance aggregation.
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

use mexc_model::{
    FlowBar, FlowQuality, Market, OpenInterestSample, OpenInterestTimestamp, VolumeFlow,
};

use super::{
    CvdCheckpoint, CvdContext, CvdReset, IndicatorInstance, IndicatorSpec, neumaier_add, point,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SourceMode {
    #[default]
    Chart,
    Single,
    Selected,
    All,
}

impl SourceMode {
    pub const ALL: [Self; 4] = [Self::Chart, Self::Single, Self::Selected, Self::All];
}

impl fmt::Display for SourceMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Chart => "Chart",
            Self::Single => "Single",
            Self::Selected => "Selected",
            Self::All => "All eligible exchanges",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IndicatorSource {
    pub mode: SourceMode,
    pub markets: Vec<Market>,
}

impl IndicatorSource {
    pub fn is_chart(&self) -> bool {
        self.mode == SourceMode::Chart
    }

    pub fn label(&self) -> String {
        if self.is_chart() {
            return self.mode.to_string();
        }
        let markets = self
            .markets
            .iter()
            .map(|m| format!("{} {}", m.exchange, m.symbol))
            .collect::<Vec<_>>()
            .join(" + ");
        format!("{}: {markets}", self.mode)
    }

    pub(crate) fn validate(&self, spec: IndicatorSpec) -> Result<(), String> {
        if self.is_chart() {
            return if self.markets.is_empty() {
                Ok(())
            } else {
                Err("Chart source cannot contain foreign markets".into())
            };
        }
        if !matches!(
            spec,
            IndicatorSpec::Cvd { .. } | IndicatorSpec::OpenInterest
        ) {
            return Err("Only CVD and Open Interest support external sources".into());
        }
        if self.markets.is_empty() || (self.mode == SourceMode::Single && self.markets.len() != 1) {
            return Err("Single requires exactly one market; aggregate modes require at least one explicit market".into());
        }
        let mut identities = HashSet::new();
        for market in &self.markets {
            // Interval is a feed detail, not a distinct selected instrument.
            if !identities.insert((market.exchange, market.product, market.symbol.as_str())) {
                return Err("Duplicate source market".into());
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ContributorStatus {
    pub market: Market,
    pub complete: bool,
    pub through_ms: Option<i64>,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExternalIndicatorOutput {
    pub oi: Vec<OpenInterestSample>,
    /// Exact underlying observation range at each actual aggregate OI event.
    pub oi_asof_ranges_ms: Vec<(i64, i64)>,
    pub unit: String,
    pub status: String,
    pub complete: bool,
    pub asof_range_ms: Option<(i64, i64)>,
    pub contexts: Vec<Option<CvdContext>>,
    /// Exact contributing as-of range for each external CVD point.
    pub context_asof_ranges_ms: Vec<Option<(i64, i64)>>,
    pub contributors: Vec<ContributorStatus>,
}

#[derive(Debug, Clone, Copy)]
struct Mutation {
    revision: u64,
    flow_from: Option<i64>,
    oi_from: Option<i64>,
}

static NEXT_SOURCE_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug)]
pub struct SourceData {
    pub market: Market,
    /// Catalog-confirmed underlying; never inferred from a symbol spelling.
    pub base_asset: String,
    pub flows: BTreeMap<i64, VolumeFlow>,
    pub oi: Vec<OpenInterestSample>,
    pub flow_connected: bool,
    pub oi_connected: bool,
    pub flow_status: String,
    pub oi_status: String,
    pub revision: u64,
    /// Current cadence declared by native OI support, not inferred from arrivals.
    pub oi_cadence_ms: Option<i64>,
    identity: u64,
    mutations: VecDeque<Mutation>,
    mutation_floor: u64,
    live_flows: HashSet<i64>,
    revoked: HashSet<i64>,
    // Proven zero-trade gaps preceding a native bucket stay finalized even if
    // that bucket's unfinished numeric snapshot is later revoked.
    zero_coverage: BTreeMap<i64, (i64, FlowQuality)>,
}

impl SourceData {
    pub fn new(market: Market, base_asset: String) -> Self {
        Self {
            market,
            base_asset,
            flows: BTreeMap::new(),
            oi: Vec::new(),
            flow_connected: false,
            oi_connected: false,
            flow_status: "Waiting for native flow".into(),
            oi_status: "Waiting for native OI".into(),
            revision: 0,
            oi_cadence_ms: None,
            identity: NEXT_SOURCE_ID.fetch_add(1, Ordering::Relaxed),
            mutations: VecDeque::new(),
            live_flows: HashSet::new(),
            revoked: HashSet::new(),
            zero_coverage: BTreeMap::new(),
            mutation_floor: 0,
        }
    }

    fn changed(&mut self, flow_from: Option<i64>, oi_from: Option<i64>) {
        self.revision = self.revision.wrapping_add(1);
        self.mutations.push_back(Mutation {
            revision: self.revision,
            flow_from,
            oi_from,
        });
        if self.mutations.len() > 256 {
            self.mutation_floor = self.mutations.pop_front().unwrap().revision;
        }
    }

    pub fn merge_flows(&mut self, bars: &[FlowBar], live: bool) -> bool {
        let mut earliest = None;
        let step = self.market.interval.seconds();
        for bar in bars {
            if live
                && bar.flow.quality != FlowQuality::Missing
                && self.flows.get(&bar.time).is_some_and(|old| {
                    old.quality != FlowQuality::Missing && old.through_ms > bar.flow.through_ms
                })
            {
                continue;
            }
            let mut incoming = bar.flow;
            if !valid_flow(bar.time, incoming, step) {
                incoming = missing(bar.time);
            }
            let finalized = incoming.quality != FlowQuality::Missing
                && incoming.through_ms == bar.time.saturating_add(step).saturating_mul(1000);
            if !live {
                // History cannot resurrect a revoked unfinished snapshot, or overwrite
                // an authoritative live replacement with an older observation.
                if self.revoked.contains(&bar.time) && !finalized {
                    continue;
                }
                if self.live_flows.contains(&bar.time)
                    && self.flows.get(&bar.time).is_some_and(|old| {
                        old.quality == FlowQuality::Missing && !finalized
                            || old.quality != FlowQuality::Missing
                                && incoming.through_ms <= old.through_ms
                    })
                {
                    continue;
                }
            }
            if live {
                self.live_flows.insert(bar.time);
                if incoming.quality == FlowQuality::Missing {
                    self.revoked.insert(bar.time);
                }
            }
            if incoming.quality != FlowQuality::Missing {
                self.revoked.remove(&bar.time);
            }
            if self.flows.get(&bar.time) != Some(&incoming) {
                let old = self.flows.get(&bar.time);
                let proof_changed = old.is_none_or(|f| {
                    f.covered_from != incoming.covered_from || f.quality != incoming.quality
                });
                let from = if proof_changed {
                    let coverage = old.map_or(incoming.covered_from, |f| {
                        f.covered_from.min(incoming.covered_from)
                    });
                    let after_previous = self
                        .zero_coverage
                        .range(..bar.time)
                        .next_back()
                        .filter(|(_, (from, _))| *from <= coverage)
                        .map_or(coverage, |(&t, _)| t.saturating_add(step));
                    bar.time.min(coverage.max(after_previous))
                } else {
                    bar.time
                };
                self.flows.insert(bar.time, incoming);
                earliest = Some(earliest.map_or(from, |t: i64| t.min(from)));
                if incoming.quality != FlowQuality::Missing && incoming.covered_from < bar.time {
                    self.zero_coverage
                        .entry(bar.time)
                        .and_modify(|proof| {
                            proof.0 = proof.0.min(incoming.covered_from);
                            if incoming.quality == FlowQuality::Reported {
                                proof.1 = FlowQuality::Reported;
                            }
                        })
                        .or_insert((incoming.covered_from, incoming.quality));
                }
            }
        }
        if let Some(from) = earliest {
            self.changed(Some(from), None);
            true
        } else {
            false
        }
    }

    pub fn merge_oi(&mut self, samples: &[OpenInterestSample]) -> bool {
        if let [sample] = samples {
            let sample = *sample;
            if !valid_oi(sample) {
                return false;
            }
            let index = self
                .oi
                .partition_point(|old| old.timestamp_ms < sample.timestamp_ms);
            if self
                .oi
                .get(index)
                .is_some_and(|old| old.timestamp_ms == sample.timestamp_ms)
            {
                let old = self.oi[index];
                if old == sample
                    || (old.timestamp_source == OpenInterestTimestamp::Venue
                        && sample.timestamp_source == OpenInterestTimestamp::Observed)
                {
                    return false;
                }
                self.oi[index] = sample;
            } else {
                self.oi.insert(index, sample);
            }
            self.changed(None, Some(sample.timestamp_ms));
            return true;
        }
        // A page is normally sorted. Sort/deduplicate only the incoming page, then
        // merge once; appending a live sample does not copy the existing history.
        let mut incoming = samples
            .iter()
            .copied()
            .filter(|s| valid_oi(*s))
            .collect::<Vec<_>>();
        incoming.sort_by_key(|s| s.timestamp_ms);
        incoming.dedup_by(|a, b| {
            if a.timestamp_ms != b.timestamp_ms {
                return false;
            }
            *b = *a;
            true
        });
        if incoming.is_empty() {
            return false;
        }
        if self
            .oi
            .last()
            .is_none_or(|last| last.timestamp_ms < incoming[0].timestamp_ms)
        {
            let from = incoming[0].timestamp_ms;
            self.oi.extend(incoming);
            self.changed(None, Some(from));
            return true;
        }
        let start = self
            .oi
            .partition_point(|old| old.timestamp_ms < incoming[0].timestamp_ms);
        let tail = self.oi.split_off(start);
        self.oi.reserve(incoming.len() + tail.len());
        let mut old = tail.into_iter().peekable();
        let mut new = incoming.into_iter().peekable();
        let mut earliest = None;
        while let Some(sample) = new.peek().copied() {
            while old
                .peek()
                .is_some_and(|s| s.timestamp_ms < sample.timestamp_ms)
            {
                self.oi.push(old.next().unwrap());
            }
            let previous = old
                .peek()
                .copied()
                .filter(|s| s.timestamp_ms == sample.timestamp_ms);
            let chosen = if let Some(previous) = previous {
                old.next();
                if previous.timestamp_source == OpenInterestTimestamp::Venue
                    && sample.timestamp_source == OpenInterestTimestamp::Observed
                {
                    previous
                } else {
                    sample
                }
            } else {
                sample
            };
            if previous != Some(chosen) {
                earliest =
                    Some(earliest.map_or(sample.timestamp_ms, |t: i64| t.min(sample.timestamp_ms)));
            }
            self.oi.push(chosen);
            new.next();
        }
        self.oi.extend(old);
        if let Some(from) = earliest {
            self.changed(None, Some(from));
            true
        } else {
            false
        }
    }

    pub fn revoke_unfinished(&mut self, now_ms: i64) -> bool {
        let step = self.market.interval.seconds();
        let mut earliest = None;
        for (&time, flow) in &mut self.flows {
            let end_ms = time.saturating_add(step).saturating_mul(1000);
            if flow.quality != FlowQuality::Missing && (flow.through_ms < end_ms || end_ms > now_ms)
            {
                *flow = missing(time);
                self.revoked.insert(time);
                self.live_flows.insert(time);
                earliest = Some(earliest.map_or(time, |t: i64| t.min(time)));
            }
        }
        if let Some(from) = earliest {
            self.changed(Some(from), None);
            true
        } else {
            false
        }
    }

    fn dirty_since(&self, revision: u64, oi: bool) -> Option<i64> {
        if revision == self.revision {
            return None;
        }
        if revision < self.mutation_floor {
            return Some(i64::MIN);
        }
        self.mutations
            .iter()
            .filter(|m| m.revision > revision)
            .filter_map(|m| if oi { m.oi_from } else { m.flow_from })
            .min()
    }
}

fn missing(time: i64) -> VolumeFlow {
    VolumeFlow {
        delta: None,
        volume: None,
        quality: FlowQuality::Missing,
        covered_from: time,
        through_ms: time.saturating_mul(1000),
    }
}

fn valid_flow(time: i64, f: VolumeFlow, step: i64) -> bool {
    if time.rem_euclid(step) != 0 {
        return false;
    }
    if f.quality == FlowQuality::Missing {
        return f.delta.is_none() && f.volume.is_none();
    }
    f.delta.is_some_and(f64::is_finite)
        && f.volume.is_some_and(|v| v.is_finite() && v >= 0.0)
        && f.delta
            .zip(f.volume)
            .is_some_and(|(d, v)| d.abs() <= v + v.abs() * 1e-12)
        && f.covered_from <= time
        && f.through_ms >= time.saturating_mul(1000)
        && f.through_ms <= time.saturating_add(step).saturating_mul(1000)
}

fn valid_oi(s: OpenInterestSample) -> bool {
    s.quantity.is_finite() && s.quantity >= 0.0 && s.interval_ms > 0
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FeedKey {
    market: Market,
    identity: Option<u64>,
    underlying: Option<String>,
    revision: u64,
    cadence: Option<i64>,
    connected: bool,
    fresh: bool,
    status: String,
}

#[derive(Debug, Default)]
pub(crate) struct SourceCache {
    keys: Vec<FeedKey>,
    base_asset: String,
    chart: Option<Market>,
    all_source_revision: u64,
    checkpoints: Vec<CvdCheckpoint>,
    finalized: Vec<bool>,
    valid: bool,
}

pub(crate) fn invalidate_numeric(instance: &mut IndicatorInstance) {
    instance.points.clear();
    instance.source_cache.checkpoints.clear();
    instance.source_cache.finalized.clear();
    instance.source_cache.valid = false;
    if let super::History::Cvd(checkpoints) = &mut instance.history {
        checkpoints.clear();
    }
    if let Some(output) = &mut instance.external {
        clear_numeric_output(output);
    }
}

fn clear_numeric_output(output: &mut ExternalIndicatorOutput) {
    output.oi.clear();
    output.contexts.clear();
    output.context_asof_ranges_ms.clear();
    output.oi_asof_ranges_ms.clear();
    output.asof_range_ms = None;
    output.complete = false;
}

fn oi_fresh(sample: OpenInterestSample, at: i64, cadence: i64) -> bool {
    cadence > 0
        && sample.timestamp_ms <= at
        && at.saturating_sub(sample.timestamp_ms) <= cadence.saturating_mul(3)
}

fn feed_fresh(data: &SourceData, oi: bool, now_ms: i64) -> bool {
    if oi {
        data.oi
            .last()
            .is_some_and(|s| oi_fresh(*s, now_ms, data.oi_cadence_ms.unwrap_or(s.interval_ms)))
    } else {
        data.flows.last_key_value().is_some_and(|(&time, flow)| {
            time.saturating_mul(1000) <= now_ms
                && time
                    .saturating_add(data.market.interval.seconds())
                    .saturating_mul(1000)
                    > now_ms
                && flow.quality != FlowQuality::Missing
        })
    }
}
fn metadata_error(
    data: Option<&SourceData>,
    market: &Market,
    product: mexc_model::Product,
    base_asset: &str,
) -> Option<&'static str> {
    match data {
        None => Some("Required native source is not loaded"),
        Some(data) if data.base_asset.is_empty() => {
            Some("Awaiting catalog-confirmed native source metadata")
        }
        Some(data) if data.market != *market => {
            Some("Native source identity or CVD interval mismatch")
        }
        Some(data) if data.base_asset != base_asset || data.market.product != product => {
            Some("Native source underlying or product mismatch")
        }
        Some(_) => None,
    }
}

pub(crate) fn refresh(
    instance: &mut IndicatorInstance,
    sources: &HashMap<Market, SourceData>,
    chart: &Market,
    base_asset: &str,
    now_ms: i64,
) -> bool {
    let oi = instance.spec == IndicatorSpec::OpenInterest;
    // The timer's unchanged path borrows feed identities/statuses and allocates
    // nothing. Expiring freshness is itself a cache-key transition.
    if instance.external.is_some()
        && instance.source_cache.base_asset == base_asset
        && instance.source_cache.chart.as_ref() == Some(chart)
        && instance.source_cache.all_source_revision == instance.all_source_revision
        && instance.source.markets.len() == instance.source_cache.keys.len()
        && instance
            .source
            .markets
            .iter()
            .zip(&instance.source_cache.keys)
            .all(|(selected, key)| {
                if selected.exchange != key.market.exchange
                    || selected.product != key.market.product
                    || selected.symbol != key.market.symbol
                {
                    return false;
                }
                let data = sources.get(&key.market);
                data.map(|d| d.identity) == key.identity
                    && data.map(|d| d.base_asset.as_str()) == key.underlying.as_deref()
                    && data.map_or(0, |d| d.revision) == key.revision
                    && data.and_then(|d| d.oi_cadence_ms) == key.cadence
                    && data.is_some_and(|d| if oi { d.oi_connected } else { d.flow_connected })
                        == key.connected
                    && data.is_some_and(|d| feed_fresh(d, oi, now_ms)) == key.fresh
                    && data
                        .map(|d| {
                            if oi {
                                d.oi_status.as_str()
                            } else {
                                d.flow_status.as_str()
                            }
                        })
                        .is_none_or(|status| status == key.status)
            })
    {
        return false;
    }
    let keys = instance
        .source
        .markets
        .iter()
        .map(|selected| {
            let mut market = selected.clone();
            market.interval = chart.interval;
            let data = sources.get(&market);
            let fresh = data.is_some_and(|d| feed_fresh(d, oi, now_ms));
            FeedKey {
                market,
                identity: data.map(|d| d.identity),
                revision: data.map_or(0, |d| d.revision),
                underlying: data.map(|d| d.base_asset.clone()),
                cadence: data.and_then(|d| d.oi_cadence_ms),
                connected: data.is_some_and(|d| if oi { d.oi_connected } else { d.flow_connected }),
                fresh,
                status: data.map_or_else(
                    || "Source unavailable (including unsupported native interval)".into(),
                    |d| {
                        if oi {
                            d.oi_status.clone()
                        } else {
                            d.flow_status.clone()
                        }
                    },
                ),
            }
        })
        .collect::<Vec<_>>();
    let scope_changed = instance.external.is_none()
        || !instance.source_cache.valid
        || instance.source_cache.chart.as_ref() != Some(chart)
        || instance.source_cache.base_asset != base_asset
        || instance.source_cache.all_source_revision != instance.all_source_revision
        || keys.len() != instance.source_cache.keys.len()
        || keys.iter().zip(&instance.source_cache.keys).any(|(a, b)| {
            a.market != b.market
                || a.identity != b.identity
                || a.underlying != b.underlying
                || (oi && a.cadence != b.cadence)
        });
    let mut dirty = scope_changed.then_some(i64::MIN);
    let mut inputs = Vec::with_capacity(keys.len());
    let mut validation_error = instance.all_source_error().map(str::to_owned);
    for (i, key) in keys.iter().enumerate() {
        let data = sources.get(&key.market);
        let error = metadata_error(data, &key.market, chart.product, base_asset);
        let valid = error.is_none();
        if let Some(error) = error.filter(|_| validation_error.is_none()) {
            validation_error = Some(error.to_owned());
        }
        if let Some(data) = data.filter(|_| valid) {
            if !scope_changed {
                if let Some(from) = data.dirty_since(instance.source_cache.keys[i].revision, oi) {
                    dirty = Some(dirty.map_or(from, |old| old.min(from)));
                }
                if !oi
                    && key.connected != instance.source_cache.keys[i].connected
                    && let Some((&time, _)) = data.flows.iter().find(|(t, f)| {
                        f.through_ms
                            < t.saturating_add(chart.interval.seconds())
                                .saturating_mul(1000)
                    })
                {
                    dirty = Some(dirty.map_or(time, |old| old.min(time)));
                }
            }
            inputs.push(data);
        }
    }
    instance.source_cache.valid = validation_error.is_none();
    if validation_error.is_some() {
        invalidate_numeric(instance);
    }
    let mut output = instance
        .external
        .take()
        .unwrap_or_else(|| ExternalIndicatorOutput {
            oi: Vec::new(),
            unit: base_asset.into(),
            status: String::new(),
            complete: false,
            asof_range_ms: None,
            contexts: Vec::new(),
            context_asof_ranges_ms: Vec::new(),
            oi_asof_ranges_ms: Vec::new(),
            contributors: Vec::new(),
        });
    if let Some(error) = validation_error {
        clear_numeric_output(&mut output);
        output.status = error;
        output.contributors = keys
            .iter()
            .map(|key| {
                let data = sources.get(&key.market);
                let error = metadata_error(data, &key.market, chart.product, base_asset);
                let through_ms = data.and_then(|data| {
                    if oi {
                        data.oi.last().map(|sample| sample.timestamp_ms)
                    } else {
                        data.flows.last_key_value().map(|(_, flow)| flow.through_ms)
                    }
                });
                ContributorStatus {
                    market: key.market.clone(),
                    complete: error.is_none() && key.connected && key.fresh,
                    through_ms,
                    message: error.map_or_else(|| key.status.clone(), str::to_owned),
                }
            })
            .collect();
    } else {
        if let Some(from) = dirty {
            if oi {
                rebuild_oi(&mut output, &inputs, from);
            } else {
                rebuild_cvd(
                    instance,
                    &mut output,
                    &inputs,
                    from,
                    chart.interval.seconds(),
                );
            }
        }
        current_status(instance, &mut output, &inputs, &keys, chart, now_ms, oi);
    }
    output.unit = base_asset.into();
    instance.external = Some(output);
    instance.source_cache.keys = keys;
    instance.source_cache.base_asset = base_asset.into();
    instance.source_cache.chart = Some(chart.clone());
    instance.source_cache.all_source_revision = instance.all_source_revision;
    true
}

#[derive(Clone, Copy)]
struct Bucket {
    delta: f64,
    quality: FlowQuality,
    through_ms: i64,
    covered_from: i64,
    finalized: bool,
}

// A later explicit coverage observation proves absent native buckets zero-trade.
// A present Missing observation is a revocation and always wins over that proof.
fn source_bucket(data: &SourceData, time: i64, step: i64) -> Option<Bucket> {
    let end = time.saturating_add(step).saturating_mul(1000);
    if let Some(f) = data.flows.get(&time) {
        if !valid_flow(time, *f, step) || f.quality == FlowQuality::Missing {
            return None;
        }
        let finalized = f.through_ms == end;
        if !finalized && !data.flow_connected {
            return None;
        }
        return Some(Bucket {
            delta: f.delta?,
            quality: f.quality,
            through_ms: f.through_ms,
            covered_from: f.covered_from,
            finalized,
        });
    }
    let (_, &(from, quality)) = data
        .zero_coverage
        .range(time.saturating_add(step)..)
        .find(|(_, (from, _))| *from <= time)?;
    Some(Bucket {
        delta: 0.0,
        quality,
        through_ms: end,
        covered_from: from,
        finalized: true,
    })
}

fn empty_checkpoint() -> CvdCheckpoint {
    CvdCheckpoint {
        sum: 0.0,
        compensation: 0.0,
        origin: 0,
        quality: FlowQuality::Exact,
        through_ms: 0,
        started: false,
        valid: false,
    }
}

fn rebuild_cvd(
    instance: &mut IndicatorInstance,
    output: &mut ExternalIndicatorOutput,
    inputs: &[&SourceData],
    from: i64,
    step: i64,
) {
    let IndicatorSpec::Cvd { reset } = instance.spec else {
        return;
    };
    let start = instance.points.partition_point(|p| p.time < from);
    instance.points.truncate(start);
    instance.source_cache.checkpoints.truncate(start);
    instance.source_cache.finalized.truncate(start);
    output.contexts.truncate(start);
    output.context_asof_ranges_ms.truncate(start);
    let mut times = BTreeSet::new();
    for source in inputs {
        times.extend(source.flows.range(from..).map(|(&time, _)| time));
    }
    for time in times {
        let day_start = time.div_euclid(86_400) * 86_400;
        let previous_time = instance.points.last().map(|p| p.time);
        let previous = instance.source_cache.checkpoints.last().copied();
        let day_changed = reset == CvdReset::UtcDay
            && previous_time.is_none_or(|t| t.div_euclid(86_400) != time.div_euclid(86_400));
        let mut state = if day_changed {
            empty_checkpoint()
        } else {
            previous.unwrap_or_else(empty_checkpoint)
        };
        if day_changed {
            state.started = true;
            state.origin = day_start;
        }
        let can_start = match reset {
            CvdReset::Continuous => !state.started,
            CvdReset::UtcDay => day_changed,
        };
        let mut sum = 0.0;
        let mut compensation = 0.0;
        let mut quality = FlowQuality::Exact;
        let mut through = i64::MIN;
        let mut earliest_through = i64::MAX;
        let mut all_final = true;
        let mut acceptable = true;
        for source in inputs {
            let Some(bucket) = source_bucket(source, time, step) else {
                acceptable = false;
                break;
            };
            if can_start && reset == CvdReset::UtcDay && bucket.covered_from > day_start {
                acceptable = false;
                break;
            }
            if !can_start {
                let bridge = previous_time.is_some_and(|t| {
                    previous.is_some_and(|p| p.valid)
                        && instance
                            .source_cache
                            .finalized
                            .last()
                            .copied()
                            .unwrap_or(false)
                        && bucket.covered_from <= t.saturating_add(step)
                });
                if !bridge {
                    acceptable = false;
                    break;
                }
            }
            neumaier_add(&mut sum, &mut compensation, bucket.delta);
            if bucket.quality == FlowQuality::Reported {
                quality = FlowQuality::Reported;
            }
            through = through.max(bucket.through_ms);
            earliest_through = earliest_through.min(bucket.through_ms);
            all_final &= bucket.finalized;
        }
        if acceptable && (can_start || state.valid) && (sum + compensation).is_finite() {
            if can_start {
                state = empty_checkpoint();
                state.started = true;
                state.origin = if reset == CvdReset::UtcDay {
                    day_start
                } else {
                    time
                };
            }
            // Preserve the bucket sum's low term across the shared accumulator.
            neumaier_add(&mut state.sum, &mut state.compensation, sum);
            neumaier_add(&mut state.sum, &mut state.compensation, compensation);
            state.valid = (state.sum + state.compensation).is_finite();
            state.through_ms = through;
            if quality == FlowQuality::Reported {
                state.quality = quality;
            }
        } else if state.started {
            state.valid = false;
        }
        instance.points.push(point(
            time,
            [
                state.valid.then_some(state.sum + state.compensation),
                None,
                None,
            ],
        ));
        instance.source_cache.checkpoints.push(state);
        instance
            .source_cache
            .finalized
            .push(acceptable && all_final);
        output.contexts.push(state.valid.then_some(CvdContext {
            origin: state.origin,
            quality: state.quality,
            through_ms: state.through_ms,
        }));
        output
            .context_asof_ranges_ms
            .push(state.valid.then_some((earliest_through, through)));
    }
}

fn rebuild_oi(external: &mut ExternalIndicatorOutput, inputs: &[&SourceData], from: i64) {
    let output = &mut external.oi;
    let start = output.partition_point(|s| s.timestamp_ms < from);
    output.truncate(start);
    external.oi_asof_ranges_ms.truncate(start);
    // Replay omitted native events since the preceding valid total as well:
    // a correction may otherwise hide a freshness gap before the dirty event.
    let from = output
        .last()
        .map_or(from, |s| s.timestamp_ms.saturating_add(1));
    let mut positions = inputs
        .iter()
        .map(|d| d.oi.partition_point(|s| s.timestamp_ms < from))
        .collect::<Vec<_>>();
    let mut latest = inputs
        .iter()
        .zip(&positions)
        .map(|(d, &p)| p.checked_sub(1).map(|i| d.oi[i]))
        .collect::<Vec<_>>();
    let mut broken = output.last().is_none();
    loop {
        let next = inputs
            .iter()
            .zip(&positions)
            .filter_map(|(d, &p)| d.oi.get(p).map(|s| s.timestamp_ms))
            .min();
        let Some(time) = next else {
            break;
        };
        // Detect freshness expiring between two native observation events as well
        // as at events. A recovered total starts a new segment, never interpolation.
        if output.last().is_some()
            && latest.iter().any(|s| {
                s.is_none_or(|s| {
                    s.timestamp_ms
                        .saturating_add(s.interval_ms.saturating_mul(3))
                        < time
                })
            })
        {
            broken = true;
        }
        for ((data, position), last) in inputs.iter().zip(&mut positions).zip(&mut latest) {
            if let Some(sample) = data.oi.get(*position).filter(|s| s.timestamp_ms == time) {
                *last = Some(*sample);
                *position += 1;
            }
        }
        let valid = latest
            .iter()
            .all(|s| s.is_some_and(|s| valid_oi(s) && oi_fresh(s, time, s.interval_ms)));
        if !valid {
            broken = true;
            continue;
        }
        let mut sum = 0.0;
        let mut compensation = 0.0;
        let mut provenance = OpenInterestTimestamp::Venue;
        let mut cadence = i64::MAX;
        let mut min_asof = i64::MAX;
        let mut max_asof = i64::MIN;
        for sample in latest.iter().flatten() {
            neumaier_add(&mut sum, &mut compensation, sample.quantity);
            if sample.timestamp_source == OpenInterestTimestamp::Observed {
                provenance = OpenInterestTimestamp::Observed;
            }
            cadence = cadence.min(sample.interval_ms);
            min_asof = min_asof.min(sample.timestamp_ms);
            max_asof = max_asof.max(sample.timestamp_ms);
        }
        let quantity = sum + compensation;
        if !quantity.is_finite() {
            broken = true;
            continue;
        }
        output.push(OpenInterestSample {
            timestamp_ms: time,
            timestamp_source: provenance,
            quantity,
            interval_ms: if broken { 0 } else { cadence },
        });
        external.oi_asof_ranges_ms.push((min_asof, max_asof));
        broken = false;
    }
}

fn current_status(
    instance: &IndicatorInstance,
    output: &mut ExternalIndicatorOutput,
    inputs: &[&SourceData],
    keys: &[FeedKey],
    chart: &Market,
    now_ms: i64,
    oi: bool,
) {
    output.contributors.clear();
    let time = instance.points.last().map(|p| p.time);
    let mut min_asof = i64::MAX;
    let mut max_asof = i64::MIN;
    let mut all_complete = true;
    for (source, key) in inputs.iter().zip(keys) {
        let (proven, through) = if oi {
            let latest = source.oi.last().copied();
            (
                latest.is_some_and(|s| {
                    valid_oi(s)
                        && oi_fresh(s, now_ms, source.oi_cadence_ms.unwrap_or(s.interval_ms))
                }),
                latest.map(|s| s.timestamp_ms),
            )
        } else {
            let bucket =
                time.and_then(|time| source_bucket(source, time, chart.interval.seconds()));
            (bucket.is_some() && key.fresh, bucket.map(|b| b.through_ms))
        };
        let complete = proven && key.connected;
        all_complete &= complete;
        if let Some(t) = through {
            min_asof = min_asof.min(t);
            max_asof = max_asof.max(t);
        }
        output.contributors.push(ContributorStatus {
            market: key.market.clone(),
            complete,
            through_ms: through,
            message: if !key.connected {
                format!("Disconnected: {}", key.status)
            } else if !proven {
                format!("Missing or stale native coverage: {}", key.status)
            } else {
                key.status.clone()
            },
        });
    }
    if !oi {
        all_complete &= instance
            .points
            .last()
            .is_some_and(|p| p.values[0].is_some());
    } else {
        all_complete &= output.oi.last().is_some_and(|s| {
            inputs
                .iter()
                .filter_map(|d| d.oi.last())
                .map(|s| s.timestamp_ms)
                .max()
                == Some(s.timestamp_ms)
        });
    }
    output.complete = all_complete;
    output.asof_range_ms = (min_asof != i64::MAX).then_some((min_asof, max_asof));
    output.status = if !all_complete {
        "Incomplete — every selected native source and common-origin continuity are required".into()
    } else if !oi
        && time.is_some_and(|t| {
            max_asof
                < t.saturating_add(chart.interval.seconds())
                    .saturating_mul(1000)
                || min_asof != max_asof
        })
    {
        "Forming native coverage; source as-of range is not a simultaneous snapshot".into()
    } else if min_asof != max_asof {
        "Complete coverage; asynchronous source observations (see as-of range)".into()
    } else {
        "Complete native source coverage".into()
    };
}
