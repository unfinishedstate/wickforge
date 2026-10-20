use iced::Subscription;
use mexc_data::DataService;
use mexc_indicators::profiles::{Engine, Instance, Kind, Range, Snapshot, period_bounds};
use mexc_model::{
    Candle, Exchange, Market, MarketEvent, Product, Timeframe,
    executions::{Coverage, ExecutionEvent, NativeInstrument},
};
use rust_decimal::{Decimal, prelude::ToPrimitive};
use std::{
    collections::BTreeMap,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    sync::{mpsc, watch},
    task::JoinHandle,
};

#[derive(Debug, Clone, Default)]
pub struct SourceStatus {
    pub history: String,
    pub live: String,
}

#[derive(Debug, Clone, Default)]
pub struct Sources {
    tpo: SourceStatus,
    volume: SourceStatus,
}

impl Sources {
    pub fn status(&self, kind: Kind) -> &SourceStatus {
        match kind {
            Kind::Tpo => &self.tpo,
            Kind::Volume => &self.volume,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Event {
    generation: u64,
    request_revision: u64,
    instrument: NativeInstrument,
    pub outputs: Arc<BTreeMap<u64, Vec<Snapshot>>>,
    pub errors: BTreeMap<u64, String>,
    pub sources: Arc<Sources>,
}

#[derive(Debug, Clone, PartialEq)]
struct Request {
    revision: u64,
    instances: Vec<Instance>,
    tick: Decimal,
    visible: (i64, i64),
}

pub struct Runtime {
    pub outputs: Arc<BTreeMap<u64, Vec<Snapshot>>>,
    pub errors: BTreeMap<u64, String>,
    pub sources: Arc<Sources>,
    pub revision: u64,
    generation: u64,
    request_revision: u64,
    instrument: Option<NativeInstrument>,
    request: Option<watch::Sender<Request>>,
}

impl Default for Runtime {
    fn default() -> Self {
        Self {
            outputs: Arc::new(BTreeMap::new()),
            errors: BTreeMap::new(),
            sources: Arc::default(),
            revision: 0,
            generation: 0,
            request_revision: 0,
            instrument: None,
            request: None,
        }
    }
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |value| {
            i64::try_from(value.as_millis()).unwrap_or(i64::MAX)
        })
}

/// Auto-size is an explicit configuration choice, never a render-time change.
pub fn suggested_rows(tick: Decimal, candles: &[Candle]) -> u32 {
    let Some(tick) = tick.to_f64().filter(|value| *value > 0.0) else {
        return 1;
    };
    let low = candles
        .iter()
        .map(|bar| bar.low)
        .fold(f64::INFINITY, f64::min);
    let high = candles
        .iter()
        .map(|bar| bar.high)
        .fold(f64::NEG_INFINITY, f64::max);
    let wanted = ((high - low) / tick / 100.0).ceil();
    if !wanted.is_finite() || wanted <= 1.0 {
        return 1;
    }
    let mut power = 1_u32;
    loop {
        for factor in [1_u32, 2, 5] {
            if let Some(candidate) = power.checked_mul(factor)
                && f64::from(candidate) >= wanted
            {
                return candidate;
            }
        }
        let Some(next) = power.checked_mul(10) else {
            return u32::MAX;
        };
        power = next;
    }
}

impl Runtime {
    pub fn sync(
        &mut self,
        market: &Market,
        tick: Option<Decimal>,
        instances: &[Instance],
        visible: (i64, i64),
    ) {
        if instances.is_empty()
            && self.request.is_none()
            && self.outputs.is_empty()
            && self.errors.is_empty()
        {
            return;
        }
        if let (Some(current), Some(sender)) = (&self.instrument, &self.request)
            && current.exchange == market.exchange
            && current.product == market.product
            && current.symbol == market.symbol
        {
            let old = sender.borrow();
            if tick == Some(old.tick)
                && old.visible == visible
                && old.instances.iter().eq(instances.iter().filter(|instance| {
                    instance
                        .instrument
                        .as_ref()
                        .is_none_or(|key| key == current)
                }))
            {
                return;
            }
        }
        let instrument = NativeInstrument::from_market(market);
        let active = || {
            instances.iter().filter(|instance| {
                instance
                    .instrument
                    .as_ref()
                    .is_none_or(|key| key == &instrument)
            })
        };
        if active().next().is_none() {
            if self.request.take().is_some() || !self.outputs.is_empty() || !self.errors.is_empty()
            {
                self.generation = self.generation.wrapping_add(1);
                self.outputs = Arc::new(BTreeMap::new());
                self.errors.clear();
                self.sources = Arc::default();
                self.revision = self.revision.wrapping_add(1);
            }
            self.instrument = Some(instrument);
            return;
        }
        let Some(tick) = tick.filter(|value| *value > Decimal::ZERO) else {
            if self.instrument.as_ref() != Some(&instrument)
                || self.request.is_some()
                || !active().map(|item| item.id).eq(self.errors.keys().copied())
            {
                self.generation = self.generation.wrapping_add(1);
                self.request = None;
                self.outputs = Arc::new(BTreeMap::new());
                self.errors = active()
                    .map(|item| {
                        (
                            item.id,
                            "Waiting for authoritative native tick-size metadata".into(),
                        )
                    })
                    .collect();
                self.revision = self.revision.wrapping_add(1);
            }
            self.instrument = Some(instrument);
            return;
        };
        // Configuration changed: fence queued outputs before notifying the worker.
        self.request_revision = self.request_revision.wrapping_add(1);
        let request = Request {
            revision: self.request_revision,
            instances: active().cloned().collect(),
            tick,
            visible,
        };
        if self.instrument.as_ref() == Some(&instrument)
            && let Some(sender) = &self.request
        {
            let old = sender.borrow();
            if old.tick == tick {
                // Panning does not invalidate completed session/fixed results.
                // Visible-range results do depend on the viewport bounds.
                Arc::make_mut(&mut self.outputs).retain(|id, _| {
                    request
                        .instances
                        .iter()
                        .find(|item| item.id == *id)
                        .is_some_and(|new| {
                            old.instances
                                .iter()
                                .find(|item| item.id == *id)
                                .is_some_and(|prior| {
                                    prior.spec == new.spec
                                        && prior.instrument == new.instrument
                                        && (!matches!(new.spec.range, Range::Visible)
                                            || old.visible == visible)
                                })
                        })
                });
            } else {
                self.outputs = Arc::new(BTreeMap::new());
            }
        } else {
            self.outputs = Arc::new(BTreeMap::new());
        }
        self.errors.clear();
        self.revision = self.revision.wrapping_add(1);
        if self.instrument.as_ref() != Some(&instrument) || self.request.is_none() {
            self.generation = self.generation.wrapping_add(1);
            self.sources = Arc::default();
            self.instrument = Some(instrument);
            self.request = Some(watch::channel(request).0);
        } else if let Some(sender) = &self.request {
            sender.send_replace(request);
        }
    }

    pub fn accept(&mut self, event: Event) {
        if event.generation == self.generation
            && event.request_revision == self.request_revision
            && self.instrument.as_ref() == Some(&event.instrument)
        {
            self.outputs = event.outputs;
            self.errors = event.errors;
            self.sources = event.sources;
            self.revision = self.revision.wrapping_add(1);
        }
    }

    pub fn retry(&mut self) {
        if let Some(sender) = &self.request {
            let mut request = sender.borrow().clone();
            self.request_revision = self.request_revision.wrapping_add(1);
            request.revision = self.request_revision;
            let estimates = request.instances.iter().any(|instance| {
                instance.spec.kind == Kind::Volume && instance.spec.estimate_missing
            });
            sender.send_replace(request);
            let sources = Arc::make_mut(&mut self.sources);
            sources.tpo.history = "Retrying native bar history; retaining observations".into();
            sources.volume.history = if estimates {
                "Retrying native executions and native 1-minute candle estimates; retaining observations".into()
            } else {
                "Retrying native executions; retaining observations".into()
            };
        }
    }

    pub fn reset(&mut self) {
        self.request = None;
        self.instrument = None;
        self.generation = self.generation.wrapping_add(1);
        self.outputs = Arc::new(BTreeMap::new());
        self.errors.clear();
        self.sources = Arc::default();
        self.revision = self.revision.wrapping_add(1);
    }

    pub fn subscription(&self, data: &DataService) -> Subscription<Event> {
        let (Some(instrument), Some(request)) = (&self.instrument, &self.request) else {
            return Subscription::none();
        };
        let generation = self.generation;
        Subscription::run_with_id(
            ("native-profiles", instrument.clone(), generation),
            worker(
                data.clone(),
                instrument.clone(),
                generation,
                request.subscribe(),
            ),
        )
    }
}

struct Job {
    task: JoinHandle<()>,
    stop: watch::Sender<bool>,
}
impl Drop for Job {
    fn drop(&mut self) {
        let _ = self.stop.send(true);
        self.task.abort();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Feed {
    Bars,
    Executions,
}

enum Input {
    Bars {
        revision: u64,
        bars: Vec<Candle>,
        observed_ms: i64,
    },
    Tape {
        event: ExecutionEvent,
        history: Option<(u64, i64, i64)>,
        live_revision: Option<u64>,
    },
    HistoryBars {
        revision: u64,
        bars: Vec<Candle>,
        observed_ms: i64,
    },
    HistoryDone {
        revision: u64,
        source: Feed,
        error: Option<String>,
    },
    Status {
        source: Feed,
        revision: u64,
        message: String,
    },
    BarGap {
        revision: u64,
        start_ms: i64,
        end_ms: i64,
        reason: String,
    },
}

fn spawn_bars(
    data: DataService,
    market: Market,
    revision: u64,
    sender: mpsc::Sender<Input>,
) -> Job {
    let (stop, mut shutdown) = watch::channel(false);
    let task = tokio::spawn(async move {
        let mut last_bar_ms = now_ms().div_euclid(60_000).saturating_mul(60_000);
        loop {
            let (events, mut receiver) = mpsc::channel(128);
            let source_data = data.clone();
            let source_market = market.clone();
            let source_stop = shutdown.clone();
            let source = tokio::spawn(async move {
                source_data
                    .stream(source_market, false, events, source_stop)
                    .await
            });
            let mut source = AbortTask(source);
            loop {
                tokio::select! {
                    _ = shutdown.changed() => return,
                    item = receiver.recv() => match item {
                        Some(MarketEvent::Candle { candle }) => {
                            last_bar_ms = candle.time.saturating_mul(1000);
                            if sender.send(Input::Bars { revision, bars: vec![candle], observed_ms: now_ms() }).await.is_err() { return; }
                        }
                        Some(MarketEvent::Status { connected: false, message }) => {
                            if sender.send(Input::BarGap { revision, start_ms: last_bar_ms, end_ms: now_ms(), reason: format!("Native 1-minute source: {message}") }).await.is_err() { return; }
                        }
                        Some(MarketEvent::Resync) => {
                            if sender.send(Input::BarGap { revision, start_ms: last_bar_ms, end_ms: now_ms(), reason: "Native 1-minute source resumed; repairing history".into() }).await.is_err() { return; }
                        }
                        Some(_) => {}, None => break,
                    },
                }
            }
            let error = (&mut source.0).await;
            if sender
                .send(Input::Status {
                    source: Feed::Bars,
                    revision,
                    message: format!("Native 1-minute source reconnecting: {error:?}"),
                })
                .await
                .is_err()
            {
                return;
            }
            tokio::select! { _ = shutdown.changed() => return, _ = tokio::time::sleep(Duration::from_secs(2)) => {} }
        }
    });
    Job { task, stop }
}

struct AbortTask<T>(JoinHandle<T>);
impl<T> Drop for AbortTask<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

fn spawn_tape(
    data: DataService,
    market: Market,
    revision: u64,
    sender: mpsc::Sender<Input>,
) -> Job {
    let (stop, mut shutdown) = watch::channel(false);
    let task = tokio::spawn(async move {
        loop {
            let (events, mut receiver) = mpsc::channel(64);
            let source_data = data.clone();
            let source_market = market.clone();
            let source_stop = shutdown.clone();
            let source = tokio::spawn(async move {
                source_data
                    .execution_stream(source_market, events, source_stop)
                    .await
            });
            let mut source = AbortTask(source);
            loop {
                tokio::select! {
                    _ = shutdown.changed() => return,
                    item = receiver.recv() => match item {
                        Some(event) => if sender.send(Input::Tape { event, history: None, live_revision: Some(revision) }).await.is_err() { return; },
                        None => break,
                    },
                }
            }
            let error = (&mut source.0).await;
            if sender
                .send(Input::Status {
                    source: Feed::Executions,
                    revision,
                    message: format!("Native execution source reconnecting: {error:?}"),
                })
                .await
                .is_err()
            {
                return;
            }
            tokio::select! { _ = shutdown.changed() => return, _ = tokio::time::sleep(Duration::from_secs(2)) => {} }
        }
    });
    Job { task, stop }
}

fn effective_visible(spec: &mexc_indicators::profiles::Spec, visible: (i64, i64)) -> (i64, i64) {
    if spec.kind == Kind::Tpo && matches!(spec.range, Range::Visible) {
        let start = visible
            .0
            .div_euclid(60_000)
            .saturating_mul(60_000)
            .saturating_add(if visible.0.rem_euclid(60_000) == 0 {
                0
            } else {
                60_000
            });
        (start, visible.1.div_euclid(60_000).saturating_mul(60_000))
    } else {
        visible
    }
}

fn requested_range(request: &Request, kind: Kind, now: i64) -> Option<(i64, i64)> {
    requested_range_for(request, now, |instance| instance.spec.kind == kind)
}

fn requested_bar_range(request: &Request, now: i64) -> Option<(i64, i64)> {
    requested_range_for(request, now, |instance| {
        instance.spec.kind == Kind::Tpo
            || (instance.spec.kind == Kind::Volume && instance.spec.estimate_missing)
    })
    .map(|(start, end)| (start.div_euclid(60_000).saturating_mul(60_000), end))
}

fn requested_range_for(
    request: &Request,
    now: i64,
    include: impl Fn(&Instance) -> bool,
) -> Option<(i64, i64)> {
    let mut start = i64::MAX;
    let mut end = i64::MIN;
    for instance in request
        .instances
        .iter()
        .filter(|instance| include(instance))
    {
        let mut range = match instance.spec.range {
            Range::Session(period) => {
                let (first, _) = period_bounds(period, request.visible.0.min(now));
                let (previous, _) = period_bounds(period, first.saturating_sub(1));
                let (_, last) = period_bounds(period, request.visible.1.min(now).saturating_sub(1));
                (previous, last.max(request.visible.1).min(now))
            }
            Range::Fixed { start_ms, end_ms } => (start_ms, end_ms.min(now)),
            Range::Visible => (request.visible.0, request.visible.1.min(now)),
        };
        if (instance.spec.kind == Kind::Tpo && instance.spec.display.single_prints)
            || (matches!(instance.spec.range, Range::Session(_)) && instance.spec.display.naked)
        {
            range.1 = now;
        }
        if range.0 < range.1 {
            start = start.min(range.0);
            end = end.max(range.1);
        }
    }
    (start < end).then_some((start, end))
}

fn next_bar_history(
    old: Option<(i64, i64)>,
    wanted: (i64, i64),
    pending: bool,
    repair: bool,
    repair_start: Option<i64>,
    now: i64,
) -> Option<(i64, i64)> {
    let Some(old) = old else {
        return Some(wanted);
    };
    if wanted.0 < old.0 {
        return Some(wanted);
    }
    if pending {
        return None;
    }
    let start = if wanted.1 > old.1.saturating_add(120_000) {
        old.1.min(repair_start.unwrap_or(old.1))
    } else if repair {
        repair_start.unwrap_or_else(|| now.saturating_sub(180_000))
    } else {
        return None;
    };
    let range = (
        wanted
            .0
            .max(start.div_euclid(60_000).saturating_mul(60_000)),
        wanted.1,
    );
    (range.0 < range.1).then_some(range)
}

fn bar_history_cursor(end_ms: i64) -> i64 {
    // History uses an exclusive seconds cursor, bounded by the next native
    // minute. Adding a whole minute incorrectly exceeds that API boundary.
    end_ms.div_euclid(1000).saturating_add(1)
}

fn spawn_history(
    data: DataService,
    market: Market,
    range: (i64, i64),
    native_bars: bool,
    volume: bool,
    revision: u64,
    sender: mpsc::Sender<Input>,
) -> Job {
    let (stop, mut shutdown) = watch::channel(false);
    let task = tokio::spawn(async move {
        let bars_data = data.clone();
        let bars_market = market.clone();
        let bars_sender = sender.clone();
        let bars = async move {
            if !native_bars {
                return;
            }
            let mut before = Some(bar_history_cursor(range.1));
            let observed_ms = now_ms();
            let result: Result<(), String> = async {
                loop {
                    let page = bars_data
                        .history(&bars_market, before)
                        .await
                        .map_err(|e| format!("{e:#}"))?;
                    let oldest = page.candles.first().map(|bar| bar.time);
                    let rows: Vec<_> = page
                        .candles
                        .into_iter()
                        .filter(|bar| {
                            bar.time.saturating_mul(1000) >= range.0
                                && bar.time.saturating_mul(1000) < range.1
                        })
                        .collect();
                    if !rows.is_empty()
                        && bars_sender
                            .send(Input::HistoryBars {
                                revision,
                                bars: rows,
                                observed_ms,
                            })
                            .await
                            .is_err()
                    {
                        return Ok(());
                    }
                    let Some(oldest) = oldest else {
                        break;
                    };
                    if oldest.saturating_mul(1000) <= range.0 {
                        break;
                    }
                    let Some(next) = page.next_before else {
                        break;
                    };
                    if before.is_some_and(|old| next >= old) {
                        return Err("Native 1-minute history cursor did not advance".into());
                    }
                    before = Some(next);
                }
                Ok(())
            }
            .await;
            let _ = bars_sender
                .send(Input::HistoryDone {
                    revision,
                    source: Feed::Bars,
                    error: result.err(),
                })
                .await;
        };
        let tape = async move {
            if !volume {
                return;
            }
            match data.execution_history(&market, range.0, range.1).await {
                Ok(page) => {
                    let _ = sender
                        .send(Input::Tape {
                            event: ExecutionEvent::Page(Arc::new(page)),
                            history: Some((revision, range.0, range.1)),
                            live_revision: None,
                        })
                        .await;
                    let _ = sender
                        .send(Input::HistoryDone {
                            revision,
                            source: Feed::Executions,
                            error: None,
                        })
                        .await;
                }
                Err(error) => {
                    let _ = sender
                        .send(Input::HistoryDone {
                            revision,
                            source: Feed::Executions,
                            error: Some(format!("{error:#}")),
                        })
                        .await;
                }
            }
        };
        tokio::select! { _ = shutdown.changed() => {}, _ = async { tokio::join!(bars, tape); } => {} }
    });
    Job { task, stop }
}

/// Canonical coverage is positive proof, not the absence of an error message.
fn add_coverage(target: &mut Vec<Coverage>, incoming: impl IntoIterator<Item = Coverage>) {
    for span in incoming {
        if span.start_ms >= span.end_ms {
            continue;
        }
        if target
            .iter()
            .any(|old| old.complete && old.start_ms <= span.start_ms && old.end_ms >= span.end_ms)
        {
            continue;
        }
        if span.complete {
            put_coverage(target, span);
        } else {
            // Failure to fetch history cannot revoke already-proven live data.
            // Only the explicit Invalidate event can revoke positive proof.
            let mut pieces = vec![(span.start_ms, span.end_ms)];
            for old in target.iter().filter(|old| old.complete) {
                let mut remaining = Vec::new();
                for (start, end) in pieces {
                    if end <= old.start_ms || start >= old.end_ms {
                        remaining.push((start, end));
                    } else {
                        if start < old.start_ms {
                            remaining.push((start, old.start_ms));
                        }
                        if end > old.end_ms {
                            remaining.push((old.end_ms, end));
                        }
                    }
                }
                pieces = remaining;
                if pieces.is_empty() {
                    break;
                }
            }
            for (start_ms, end_ms) in pieces {
                put_coverage(
                    target,
                    Coverage {
                        start_ms,
                        end_ms,
                        complete: false,
                        reason: span.reason.clone(),
                    },
                );
            }
        }
    }
}

fn put_coverage(target: &mut Vec<Coverage>, span: Coverage) {
    if let Some(last) = target.last_mut()
        && last.complete == span.complete
        && last.reason == span.reason
        && span.start_ms >= last.start_ms
        && span.start_ms <= last.end_ms
    {
        last.end_ms = last.end_ms.max(span.end_ms);
        return;
    }
    let mut next = Vec::with_capacity(target.len() + 2);
    for old in target.drain(..) {
        if old.end_ms <= span.start_ms || old.start_ms >= span.end_ms {
            next.push(old);
        } else {
            if old.start_ms < span.start_ms {
                next.push(Coverage {
                    end_ms: span.start_ms,
                    ..old.clone()
                });
            }
            if old.end_ms > span.end_ms {
                next.push(Coverage {
                    start_ms: span.end_ms,
                    ..old
                });
            }
        }
    }
    next.push(span);
    next.sort_by_key(|span| span.start_ms);
    let mut merged: Vec<Coverage> = Vec::with_capacity(next.len());
    for span in next {
        if let Some(last) = merged.last_mut()
            && last.end_ms == span.start_ms
            && last.complete == span.complete
            && last.reason == span.reason
        {
            last.end_ms = span.end_ms;
        } else {
            merged.push(span);
        }
    }
    *target = merged;
}

fn revoke_coverage(target: &mut Vec<Coverage>, start: i64, end: i64, reason: String) {
    put_coverage(
        target,
        Coverage {
            start_ms: start,
            end_ms: end,
            complete: false,
            reason: Some(reason),
        },
    );
}

fn bar_proof(bars: &[Candle], observed_ms: i64) -> impl Iterator<Item = Coverage> + '_ {
    bars.iter().filter_map(move |bar| {
        let start = bar.time.saturating_mul(1000);
        let end = start.saturating_add(60_000);
        (end <= observed_ms).then_some(Coverage {
            start_ms: start,
            end_ms: end,
            complete: true,
            reason: None,
        })
    })
}

fn source_statuses(request: &Request, bars: &SourceStatus, tape: &SourceStatus) -> Sources {
    let estimates = request
        .instances
        .iter()
        .any(|instance| instance.spec.kind == Kind::Volume && instance.spec.estimate_missing);
    Sources {
        tpo: bars.clone(),
        volume: if estimates {
            SourceStatus {
                history: format!(
                    "Executions: {}; Candle estimates (native 1m): {}",
                    tape.history, bars.history
                ),
                live: format!(
                    "Executions: {}; Candle estimates (native 1m): {}",
                    tape.live, bars.live
                ),
            }
        } else {
            tape.clone()
        },
    }
}

fn retain_current_bars(
    bars: &mut Vec<Candle>,
    observed_ms: i64,
    current_ms: i64,
    observations: &mut BTreeMap<i64, i64>,
) {
    // Only the forming minute and two just-closed minutes need protection
    // against a history page fetched before newer live updates. Older history
    // repairs stay authoritative, and bulk history never grows this guard.
    let current_minute = current_ms.div_euclid(60_000).saturating_mul(60);
    let oldest_guarded = current_minute.saturating_sub(120);
    observations.retain(|time, _| *time >= oldest_guarded && *time <= current_minute);
    bars.retain(|bar| {
        if bar.time < oldest_guarded || bar.time > current_minute {
            return true;
        }
        let observed = observations.entry(bar.time).or_insert(observed_ms);
        if observed_ms < *observed {
            return false;
        }
        *observed = observed_ms;
        true
    });
}

fn worker(
    data: DataService,
    instrument: NativeInstrument,
    generation: u64,
    mut requests: watch::Receiver<Request>,
) -> impl futures_util::Stream<Item = Event> {
    iced::stream::channel(2, move |mut output| async move {
        use futures_util::SinkExt;
        use std::borrow::Cow;
        let market = instrument.market(Timeframe::Min1);
        let separate_namespaces =
            instrument.exchange == Exchange::Mexc && instrument.product == Product::Spot;
        let (sender, mut input) = mpsc::channel(128);
        let mut engine = Engine::default();
        let mut bar_coverage = Vec::new();
        let mut tape_coverage = Vec::new();
        let mut bar_observations = BTreeMap::new();
        let mut bars_job = None;
        let mut tape_job = None;
        let mut bar_history_job = None;
        let mut tape_history_job = None;
        let mut bar_revision = 0_u64;
        let mut tape_revision = 0_u64;
        let mut bar_live_revision = 0_u64;
        let mut tape_live_revision = 0_u64;
        let mut bar_range: Option<(i64, i64)> = None;
        let mut tape_range: Option<(i64, i64)> = None;
        let mut bars_pending = false;
        let mut capture_start = now_ms();
        let mut bar_source = SourceStatus::default();
        let mut tape_source = SourceStatus::default();
        let mut tape_history_error = None;
        let mut sources = Arc::new(Sources::default());
        let mut source_dirty = true;
        let mut request = requests.borrow_and_update().clone();
        let mut frames = tokio::time::interval(Duration::from_millis(16));
        let mut dirty = true;
        let mut reconfigure = true;
        let mut repair_bars = false;
        let mut repair_start: Option<i64> = None;
        let mut previous_minute = now_ms().div_euclid(60_000);
        loop {
            if reconfigure {
                source_dirty = true;
                let volume = request
                    .instances
                    .iter()
                    .any(|item| item.spec.kind == Kind::Volume);
                let now = now_ms();
                let bar_wanted = requested_bar_range(&request, now);
                let tape_wanted = requested_range(&request, Kind::Volume, now);
                let bars_live = bar_wanted.is_some_and(|range| range.1 >= now);
                let tape_live = tape_wanted.is_some_and(|range| range.1 >= now);
                if !bars_live {
                    if bars_job.take().is_some() {
                        bar_live_revision = bar_live_revision.wrapping_add(1);
                    }
                    bar_source.live = "Closed range; live source not required".into();
                }
                if !tape_live {
                    if tape_job.take().is_some() {
                        tape_live_revision = tape_live_revision.wrapping_add(1);
                    }
                    tape_source.live = "Closed range; live source not required".into();
                }
                if bars_live && bars_job.is_none() {
                    bar_live_revision = bar_live_revision.wrapping_add(1);
                    bar_source.live = "Connecting native 1-minute source".into();
                    bars_job = Some(spawn_bars(
                        data.clone(),
                        market.clone(),
                        bar_live_revision,
                        sender.clone(),
                    ));
                }
                if bar_wanted.is_none() {
                    if bar_history_job.take().is_some() {
                        bar_revision = bar_revision.wrapping_add(1);
                    }
                    bars_pending = false;
                    bar_range = None;
                    repair_start = None;
                    repair_bars = false;
                }
                if tape_live && tape_job.is_none() {
                    // Freeze historical/live ownership. In particular MEXC spot
                    // REST aggregates and protobuf fills have unrelated IDs.
                    capture_start = now_ms();
                    tape_live_revision = tape_live_revision.wrapping_add(1);
                    tape_source.live = "Connecting native execution source".into();
                    tape_job = Some(spawn_tape(
                        data.clone(),
                        market.clone(),
                        tape_live_revision,
                        sender.clone(),
                    ));
                    tape_range = None;
                }
                if !volume {
                    tape_job = None;
                    if tape_history_job.take().is_some() {
                        tape_revision = tape_revision.wrapping_add(1);
                    }
                    tape_range = None;
                }
                if let Some(wanted) = bar_wanted
                    && let Some(range) = next_bar_history(
                        bar_range,
                        wanted,
                        bars_pending,
                        repair_bars,
                        repair_start,
                        now,
                    )
                {
                    bar_revision = bar_revision.wrapping_add(1);
                    bar_source.history = "Loading native 1-minute history".into();
                    bar_history_job = Some(spawn_history(
                        data.clone(),
                        market.clone(),
                        range,
                        true,
                        false,
                        bar_revision,
                        sender.clone(),
                    ));
                    bars_pending = true;
                    bar_range = Some(
                        bar_range.map_or(wanted, |old| (old.0.min(wanted.0), old.1.max(wanted.1))),
                    );
                    repair_bars = false;
                }
                if let Some(wanted) = tape_wanted {
                    let historical = (wanted.0, wanted.1.min(capture_start));
                    if volume
                        && historical.0 < historical.1
                        && tape_range.is_none_or(|old| historical.0 < old.0 || historical.1 > old.1)
                    {
                        tape_revision = tape_revision.wrapping_add(1);
                        tape_history_error = None;
                        tape_source.history = "Loading priced native execution history".into();
                        tape_history_job = Some(spawn_history(
                            data.clone(),
                            market.clone(),
                            historical,
                            false,
                            true,
                            tape_revision,
                            sender.clone(),
                        ));
                        tape_range = Some(historical);
                    }
                }
                reconfigure = false;
            }
            tokio::select! {
                changed = requests.changed() => {
                    if changed.is_err() { break; }
                    let next = requests.borrow_and_update().clone();
                    if next.instances == request.instances && next.visible == request.visible && next.tick == request.tick {
                        // Revision-only requests are explicit retries. Keep both
                        // live producers and the canonical engine observations.
                        bar_range = None; tape_range = None;
                    }
                    request = next; reconfigure = true; dirty = true;
                }
                _ = frames.tick() => {
                    let now = now_ms();
                    let minute = now.div_euclid(60_000);
                    if minute != previous_minute {
                        previous_minute = minute; reconfigure = true; repair_bars = true; dirty = true;
                    }
                    if dirty {
                        let mut snapshots = BTreeMap::new(); let mut errors = BTreeMap::new();
                        for instance in &request.instances {
                            match engine.calculate(&instance.spec, request.tick, effective_visible(&instance.spec, request.visible), now) {
                                Ok(rows) => { snapshots.insert(instance.id, rows); }
                                Err(error) => { errors.insert(instance.id, error); }
                            }
                        }
                        if source_dirty {
                            sources = Arc::new(source_statuses(&request, &bar_source, &tape_source));
                            source_dirty = false;
                        }
                        if output.send(Event { generation, request_revision: request.revision, instrument: instrument.clone(),
                            outputs: Arc::new(snapshots), errors, sources: sources.clone() }).await.is_err() { break; }
                        dirty = false;
                    }
                }
                event = input.recv() => {
                    let Some(event) = event else { break; };
                    let live_bars = matches!(&event, Input::Bars { .. });
                    source_dirty |= !matches!(&event,
                        Input::Bars { .. } | Input::HistoryBars { .. }
                        | Input::Tape { event: ExecutionEvent::Page(_), .. });
                    match event {
                        Input::BarGap { revision, .. } | Input::Bars { revision, .. }
                            if bars_job.is_none() || revision != bar_live_revision => {},
                        Input::Tape { history: None, live_revision, .. }
                            if tape_job.is_none() || live_revision != Some(tape_live_revision) => {},
                        Input::BarGap { start_ms, end_ms, reason, .. } => {
                            revoke_coverage(&mut bar_coverage, start_ms, end_ms, reason.clone());
                            engine.set_bar_coverage(bar_coverage.clone());
                            repair_start = Some(repair_start.map_or(start_ms, |old| old.min(start_ms)));
                            repair_bars = true; reconfigure = true; bar_source.live = reason; dirty = true;
                        }
                        Input::HistoryBars { revision, .. } if revision != bar_revision => {},
                        Input::Bars { mut bars, observed_ms, .. } | Input::HistoryBars { mut bars, observed_ms, .. } => {
                            retain_current_bars(&mut bars, observed_ms, now_ms(), &mut bar_observations);
                            if live_bars && bar_source.live != "Native 1-minute updates received" {
                                bar_source.live = "Native 1-minute updates received".into();
                                source_dirty = true;
                            }
                            add_coverage(&mut bar_coverage, bar_proof(&bars, observed_ms));
                            engine.upsert_bars(&bars); engine.set_bar_coverage(bar_coverage.clone()); dirty = true;
                        }
                        Input::Tape { event, history, .. } => {
                            if history.is_some_and(|(revision, _, _)| revision != tape_revision) { continue; }
                            match event {
                                ExecutionEvent::Page(page) => {
                                    let rows = if history.is_none() && page.executions.iter().any(|row| row.time_ms < capture_start) {
                                        Cow::Owned(page.executions.iter().filter(|row| row.time_ms >= capture_start).cloned().collect::<Vec<_>>())
                                    } else { Cow::Borrowed(page.executions.as_slice()) };
                                    let result = if let Some((_, start, end)) = history {
                                        if separate_namespaces || page.coverage.iter().any(|span| span.complete && span.start_ms <= start && span.end_ms >= end) {
                                            engine.replace_executions(start, end, &rows)
                                        } else { engine.ingest_executions(&rows) }
                                    } else { engine.ingest_executions(&rows) };
                                    if let Err(error) = result {
                                        source_dirty = true;
                                        let message = format!("Volume input rejected: {error}");
                                        if history.is_some() {
                                            tape_history_error = Some(message.clone());
                                            tape_source.history = message.clone();
                                        } else { tape_source.live = message.clone(); }
                                        for span in &page.coverage {
                                            revoke_coverage(&mut tape_coverage, span.start_ms, span.end_ms, message.clone());
                                        }
                                    } else {
                                        add_coverage(&mut tape_coverage, page.coverage.iter().filter_map(|span| {
                                            let mut span = span.clone();
                                            if history.is_none() { span.start_ms = span.start_ms.max(capture_start); }
                                            (span.start_ms < span.end_ms).then_some(span)
                                        }));
                                    }
                                    engine.set_execution_coverage(tape_coverage.clone()); dirty = true;
                                }
                                ExecutionEvent::Invalidate { start_ms, end_ms, reason } => {
                                    revoke_coverage(&mut tape_coverage, start_ms, end_ms, reason.clone());
                                    engine.invalidate_executions(start_ms, end_ms, reason.clone());
                                    engine.set_execution_coverage(tape_coverage.clone()); tape_source.live = reason; dirty = true;
                                }
                                ExecutionEvent::Status { connected: _, message } => { tape_source.live = message; dirty = true; }
                            }
                        }
                        Input::HistoryDone { revision, source: Feed::Bars, error } if revision == bar_revision => {
                            bars_pending = false;
                            if error.is_none() { repair_start = None; }
                            bar_source.history = error.map_or_else(|| "Native 1-minute history loaded; coverage checked per profile".into(), |error| format!("Native 1-minute history: {error}"));
                            reconfigure = repair_bars; dirty = true;
                        }
                        Input::HistoryDone { revision, source: Feed::Executions, error } if revision == tape_revision => {
                            tape_source.history = error.or_else(|| tape_history_error.take()).map_or_else(|| "Priced execution history loaded; coverage checked per profile".into(), |error| format!("Volume history: {error}"));
                            dirty = true;
                        }
                        Input::HistoryDone { .. } => {},
                        Input::Status { source: Feed::Bars, revision, message }
                            if bars_job.is_some() && revision == bar_live_revision => {
                            bar_source.live = message; dirty = true;
                        }
                        Input::Status { source: Feed::Executions, revision, message }
                            if tape_job.is_some() && revision == tape_live_revision => {
                            tape_source.live = message; dirty = true;
                        }
                        Input::Status { .. } => {},
                    }
                }
            }
        }
        drop((bars_job, tape_job, bar_history_job, tape_history_job));
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mexc_indicators::profiles::{Period, Spec};
    use mexc_model::executions::Execution;

    fn proof(start_ms: i64, end_ms: i64, complete: bool) -> Coverage {
        Coverage {
            start_ms,
            end_ms,
            complete,
            reason: (!complete).then(|| "native interval unproved".into()),
        }
    }

    #[test]
    fn replay_proof_repairs_gaps_but_history_failure_cannot_revoke_observed_coverage() {
        let mut coverage = vec![proof(0, 60_000, true)];
        add_coverage(&mut coverage, [proof(30_000, 90_000, false)]);
        assert_eq!(
            coverage,
            vec![proof(0, 60_000, true), proof(60_000, 90_000, false)]
        );
        revoke_coverage(&mut coverage, 20_000, 40_000, "disconnect".into());
        let mut engine = Engine::default();
        engine
            .ingest_executions(&[Execution {
                time_ms: 30_000,
                id: "raw:1".into(),
                price: Decimal::from(11),
                quantity: Decimal::from(3),
            }])
            .unwrap();
        let mut spec = Spec::default_for(Kind::Volume);
        spec.range = Range::Fixed {
            start_ms: 0,
            end_ms: 90_000,
        };
        engine.set_execution_coverage(coverage.clone());
        assert!(
            !engine
                .calculate(&spec, Decimal::ONE, (0, 90_000), 90_000)
                .unwrap()[0]
                .complete
        );
        add_coverage(&mut coverage, [proof(0, 90_000, true)]);
        engine.set_execution_coverage(coverage);
        let result = engine
            .calculate(&spec, Decimal::ONE, (0, 90_000), 90_000)
            .unwrap();
        assert!(result[0].complete);
        assert_eq!(result[0].poc, Some(11.0));
        assert_eq!(result[0].rows[0].weight, 3.0);
    }

    fn computed(spec: &Spec) -> Vec<Snapshot> {
        let mut engine = Engine::default();
        engine
            .ingest_executions(&[Execution {
                time_ms: 30_000,
                id: "raw:1".into(),
                price: Decimal::from(11),
                quantity: Decimal::from(3),
            }])
            .unwrap();
        engine.set_execution_coverage(vec![proof(0, 60_000, true)]);
        engine
            .calculate(spec, Decimal::ONE, (0, 60_000), 60_000)
            .unwrap()
    }

    fn update(runtime: &Runtime, snapshots: Vec<Snapshot>) -> Event {
        Event {
            generation: runtime.generation,
            request_revision: runtime.request_revision,
            instrument: NativeInstrument::from_market(&Market::default()),
            outputs: Arc::new(BTreeMap::from([(1, snapshots)])),
            errors: BTreeMap::new(),
            sources: Arc::default(),
        }
    }

    #[test]
    fn changed_row_settings_reject_queued_old_levels_and_publish_new_grid() {
        let mut spec = Spec::default_for(Kind::Volume);
        spec.range = Range::Fixed {
            start_ms: 0,
            end_ms: 60_000,
        };
        let mut instances = vec![Instance {
            id: 1,
            spec: spec.clone(),
            visible: true,
            instrument: None,
        }];
        let mut runtime = Runtime::default();
        runtime.sync(
            &Market::default(),
            Some(Decimal::ONE),
            &instances,
            (0, 60_000),
        );
        let old = update(&runtime, computed(&spec));
        runtime.accept(old.clone());
        assert_eq!(runtime.outputs[&1][0].poc, Some(11.0));
        instances[0].spec.ticks_per_row = 2;
        runtime.sync(
            &Market::default(),
            Some(Decimal::ONE),
            &instances,
            (0, 60_000),
        );
        runtime.accept(old);
        assert!(!runtime.outputs.contains_key(&1));
        runtime.accept(update(&runtime, computed(&instances[0].spec)));
        assert_eq!(runtime.outputs[&1][0].poc, Some(10.0));
    }

    #[test]
    fn session_panning_keeps_valid_levels_while_new_window_is_pending() {
        let mut spec = Spec::default_for(Kind::Volume);
        spec.range = Range::Session(Period::Day);
        let instances = vec![Instance {
            id: 1,
            spec: spec.clone(),
            visible: true,
            instrument: None,
        }];
        let mut runtime = Runtime::default();
        runtime.sync(
            &Market::default(),
            Some(Decimal::ONE),
            &instances,
            (0, 60_000),
        );
        runtime.accept(update(&runtime, computed(&spec)));
        runtime.sync(
            &Market::default(),
            Some(Decimal::ONE),
            &instances,
            (10_000, 70_000),
        );
        assert_eq!(runtime.outputs[&1].last().unwrap().poc, Some(11.0));
    }

    #[test]
    fn visible_tpo_uses_only_whole_native_minutes_without_changing_volume_ranges() {
        let mut spec = Spec::default_for(Kind::Tpo);
        spec.range = Range::Visible;
        let range = effective_visible(&spec, (12_345, 120_789));
        let mut engine = Engine::default();
        engine.upsert_bars(&[Candle {
            time: 60,
            open: 11.0,
            high: 11.0,
            low: 11.0,
            close: 11.0,
            volume: 0.0,
            flow: None,
        }]);
        engine.set_bar_coverage(vec![proof(60_000, 120_000, true)]);
        let out = engine
            .calculate(&spec, Decimal::ONE, range, 120_789)
            .unwrap();
        assert_eq!((out[0].start_ms, out[0].end_ms), (60_000, 120_000));
        assert_eq!(out[0].poc, Some(11.0));
        assert!(out[0].complete);
        spec.kind = Kind::Volume;
        assert_eq!(
            effective_visible(&spec, (12_345, 120_789)),
            (12_345, 120_789)
        );
    }

    #[test]
    fn history_cursor_includes_forming_minute_without_exceeding_native_api_boundary() {
        for end_ms in [1, 1_001, 59_999, 60_000, 60_001, 119_999] {
            let before = bar_history_cursor(end_ms);
            let native_open = (end_ms - 1).div_euclid(60_000) * 60;
            let next_boundary = (end_ms.div_euclid(60_000) + 1) * 60;
            assert!(before > native_open);
            assert!(before <= next_boundary);
        }
    }

    #[test]
    fn source_windows_are_kind_specific_and_closed_tpo_monitors_future_mitigation() {
        let mut tpo = Spec::default_for(Kind::Tpo);
        tpo.range = Range::Fixed {
            start_ms: 0,
            end_ms: 120_000,
        };
        let mut volume = Spec::default_for(Kind::Volume);
        volume.range = Range::Visible;
        let mut request = Request {
            revision: 1,
            tick: Decimal::ONE,
            visible: (86_400_000, 87_000_000),
            instances: vec![
                Instance {
                    id: 1,
                    spec: tpo,
                    visible: true,
                    instrument: None,
                },
                Instance {
                    id: 2,
                    spec: volume,
                    visible: true,
                    instrument: None,
                },
            ],
        };
        assert_eq!(
            requested_range(&request, Kind::Volume, 87_000_000),
            Some((86_400_000, 87_000_000))
        );
        assert_eq!(
            requested_range(&request, Kind::Tpo, 87_000_000),
            Some((0, 87_000_000))
        );
        request.instances[0].spec.display.single_prints = false;
        assert_eq!(
            requested_range(&request, Kind::Tpo, 87_000_000),
            Some((0, 120_000))
        );
    }

    #[test]
    fn tail_repair_preserves_inflight_bulk_and_never_reloads_an_already_loaded_prefix() {
        let old = Some((0, 60_123));
        let wanted = (0, 300_456);
        assert_eq!(
            next_bar_history(old, wanted, true, true, None, 300_456),
            None
        );
        assert_eq!(
            next_bar_history(old, wanted, false, true, None, 300_456),
            Some((60_000, 300_456))
        );
        assert_eq!(
            next_bar_history(old, (-60_000, 300_456), true, true, None, 300_456),
            Some((-60_000, 300_456))
        );
        assert_eq!(
            next_bar_history(
                Some((0, 60_000)),
                (0, 60_000),
                false,
                true,
                None,
                86_400_000
            ),
            None
        );
    }

    fn volume_request() -> Request {
        let mut spec = Spec::default_for(Kind::Volume);
        spec.range = Range::Visible;
        Request {
            revision: 1,
            tick: Decimal::ONE,
            visible: (12_345, 120_789),
            instances: vec![Instance {
                id: 1,
                spec,
                visible: true,
                instrument: None,
            }],
        }
    }

    #[test]
    fn estimated_volume_requests_native_edge_minutes_independently_of_execution_windows() {
        let mut request = volume_request();
        assert_eq!(requested_bar_range(&request, 120_789), Some((0, 120_789)));
        assert_eq!(
            requested_range(&request, Kind::Volume, 120_789),
            Some((12_345, 120_789))
        );
        request.instances[0].spec.estimate_missing = false;
        assert_eq!(requested_bar_range(&request, 120_789), None);
        let mut tpo = Spec::default_for(Kind::Tpo);
        tpo.range = Range::Fixed {
            start_ms: 600_000,
            end_ms: 660_000,
        };
        tpo.display.single_prints = false;
        request.instances.push(Instance {
            id: 2,
            spec: tpo,
            visible: true,
            instrument: None,
        });
        assert_eq!(
            requested_bar_range(&request, 900_000),
            Some((600_000, 660_000))
        );
        request.instances[0].spec.estimate_missing = true;
        assert_eq!(requested_bar_range(&request, 900_000), Some((0, 660_000)));
        assert_eq!(
            requested_range(&request, Kind::Tpo, 900_000),
            Some((600_000, 660_000))
        );
    }

    #[test]
    fn closed_estimated_ranges_repair_disconnects_without_subscribing_to_live_bars() {
        let mut request = volume_request();
        request.instances[0].spec.range = Range::Fixed {
            start_ms: 12_345,
            end_ms: 120_789,
        };
        let wanted = requested_bar_range(&request, 300_000).unwrap();
        assert!(wanted.1 < 300_000);
        assert_eq!(
            next_bar_history(Some(wanted), wanted, false, true, Some(60_123), 300_000),
            Some((60_000, 120_789))
        );
        assert_eq!(
            next_bar_history(Some(wanted), wanted, true, true, Some(60_123), 300_000),
            None
        );
        assert_eq!(
            next_bar_history(Some(wanted), wanted, false, true, None, 900_000),
            None
        );
    }

    #[test]
    fn candle_status_discloses_fallback_without_hiding_execution_failures() {
        let mut request = volume_request();
        let bars = SourceStatus {
            history: "native bars loaded".into(),
            live: "native bars streaming".into(),
        };
        let tape = SourceStatus {
            history: "execution history unavailable".into(),
            live: "execution connection failed".into(),
        };
        let sources = source_statuses(&request, &bars, &tape);
        assert!(
            sources
                .volume
                .history
                .contains("execution history unavailable")
        );
        assert!(sources.volume.history.contains("native bars loaded"));
        assert!(sources.volume.live.contains("execution connection failed"));
        assert!(sources.volume.live.contains("native bars streaming"));
        assert_eq!(sources.tpo.history, bars.history);
        request.instances[0].spec.estimate_missing = false;
        let sources = source_statuses(&request, &bars, &tape);
        assert_eq!(sources.volume.history, tape.history);
        assert_eq!(sources.volume.live, tape.live);
    }

    #[test]
    fn delayed_history_cannot_revert_live_estimates_and_exact_recovery_replaces_them() {
        use mexc_indicators::profiles::VolumeMethod;
        let bar = Candle {
            time: 0,
            open: 11.0,
            high: 11.0,
            low: 11.0,
            close: 11.0,
            volume: 12.0,
            flow: None,
        };
        let mut observations = BTreeMap::new();
        let mut live = vec![bar];
        retain_current_bars(&mut live, 59_000, 59_000, &mut observations);
        let mut engine = Engine::default();
        engine.upsert_bars(&live);
        let mut historical = vec![Candle { volume: 3.0, ..bar }];
        retain_current_bars(&mut historical, 45_000, 59_000, &mut observations);
        assert!(historical.is_empty());
        engine.upsert_bars(&historical);
        let mut spec = Spec::default_for(Kind::Volume);
        spec.range = Range::Fixed {
            start_ms: 0,
            end_ms: 60_000,
        };
        let estimated = engine
            .calculate(&spec, Decimal::ONE, (0, 60_000), 60_000)
            .unwrap();
        assert_eq!(estimated[0].volume_method, VolumeMethod::Estimated);
        assert_eq!(estimated[0].native_volume, Some(Decimal::from(12)));
        assert!(!estimated[0].complete);

        let mut repaired = vec![Candle {
            volume: 15.0,
            ..bar
        }];
        retain_current_bars(&mut repaired, 60_000, 60_000, &mut observations);
        engine.upsert_bars(&repaired);
        let revised = engine
            .calculate(&spec, Decimal::ONE, (0, 60_000), 60_000)
            .unwrap();
        assert_eq!(revised[0].native_volume, Some(Decimal::from(15)));
        engine
            .ingest_executions(&[Execution {
                time_ms: 30_000,
                id: "raw:recovered".into(),
                price: Decimal::from(11),
                quantity: Decimal::from(4),
            }])
            .unwrap();
        engine.set_execution_coverage(vec![proof(0, 60_000, true)]);
        let exact = engine
            .calculate(&spec, Decimal::ONE, (0, 60_000), 60_000)
            .unwrap();
        assert_eq!(exact[0].volume_method, VolumeMethod::Exact);
        assert_eq!(exact[0].native_volume, Some(Decimal::from(4)));
        assert!(exact[0].complete);
    }

    #[test]
    fn observation_guard_expires_old_minutes_and_does_not_track_bulk_history() {
        let bar = Candle {
            time: 0,
            open: 11.0,
            high: 11.0,
            low: 11.0,
            close: 11.0,
            volume: 12.0,
            flow: None,
        };
        let mut observations = BTreeMap::new();
        let mut bulk: Vec<_> = (0..=4)
            .map(|minute| Candle {
                time: minute * 60,
                ..bar
            })
            .collect();
        retain_current_bars(&mut bulk, 299_000, 299_000, &mut observations);
        assert_eq!(bulk.len(), 5);
        assert_eq!(
            observations.keys().copied().collect::<Vec<_>>(),
            [120, 180, 240]
        );

        let mut live = vec![Candle { time: 300, ..bar }];
        retain_current_bars(&mut live, 310_000, 310_000, &mut observations);
        assert_eq!(
            observations.keys().copied().collect::<Vec<_>>(),
            [180, 240, 300]
        );

        // The same delayed page may now repair the expired minute, but must
        // still leave newer observations in the recent protection window alone.
        let mut repair: Vec<_> = [120, 180, 240]
            .into_iter()
            .map(|time| Candle {
                time,
                volume: 7.0,
                ..bar
            })
            .collect();
        retain_current_bars(&mut repair, 250_000, 310_000, &mut observations);
        assert_eq!(
            repair,
            vec![Candle {
                time: 120,
                volume: 7.0,
                ..bar
            }]
        );
        assert_eq!(
            observations.keys().copied().collect::<Vec<_>>(),
            [180, 240, 300]
        );
    }

    #[test]
    fn candle_fallback_still_waits_for_authoritative_tick_metadata() {
        let request = volume_request();
        let mut runtime = Runtime::default();
        runtime.sync(
            &Market::default(),
            None,
            &request.instances,
            request.visible,
        );
        assert!(runtime.request.is_none());
        assert!(runtime.outputs.is_empty());
        assert!(runtime.errors[&1].contains("authoritative native tick-size"));
    }
}
