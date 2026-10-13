use mexc_indicators::{
    IndicatorInstance, IndicatorSet, IndicatorSource, IndicatorSpec, SourceData,
};
use mexc_model::{
    Candle, FlowBar, FlowQuality, HistoryPage, Market, OpenInterestSample, OpenInterestSupport,
    Product, Timeframe, VolumeFlow,
};
use std::collections::{BTreeMap, HashMap};

/// Input adapters only translate into these renderer-independent commands.
#[derive(Debug, Clone)]
pub enum Action {
    Pan {
        time: f64,
        price: f64,
    },
    Zoom {
        time: f64,
        price: f64,
        x: f64,
        y: f64,
    },
    PanePan {
        id: u64,
        time: f64,
        value: f64,
    },
    PaneZoom {
        id: u64,
        time: f64,
        value: f64,
        x: f64,
        y: f64,
    },
    FitPane(u64),
    ActivatePane(Option<u64>),
    SetPaneHeight {
        id: Option<u64>,
        height: f32,
    },
    AdjustPaneHeight {
        id: Option<u64>,
        delta: f32,
    },
    ResetPaneHeight(Option<u64>),
    TogglePaneCollapsed(u64),
    InspectPane {
        id: Option<u64>,
        point: Option<(f64, f64)>,
    },
    Reset,
    AutoFit,
    Live,
    Inspect(Option<(f64, f64)>),
    InspectStep(i32),
    Older,
    Retry,
    ManageIndicators,
    AddIndicator(IndicatorSpec),
    ConfigureIndicator {
        id: u64,
        spec: IndicatorSpec,
    },
    ApplyIndicator {
        id: Option<u64>,
        spec: IndicatorSpec,
        source: IndicatorSource,
    },
    ToggleIndicator(u64),
    RemoveIndicator(u64),
    ScrollPanes {
        amount: f32,
        legends: bool,
    },
    ManageMarket,
    SelectMarket(Market),
}

#[derive(Debug, Clone, Copy)]
pub struct Viewport {
    pub left: f64,
    pub span: f64,
    pub low: f64,
    pub high: f64,
    pub auto_fit: bool,
    pub live: bool,
}

impl Default for Viewport {
    fn default() -> Self {
        Self::for_timeframe(Timeframe::default())
    }
}

impl Viewport {
    pub fn for_timeframe(timeframe: Timeframe) -> Self {
        Self {
            left: 0.0,
            span: 120.0 * timeframe.seconds() as f64,
            low: 0.0,
            high: 1.0,
            auto_fit: true,
            live: true,
        }
    }
}

/// Cached value viewport belonging to a stable indicator instance, not its position.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PaneScale {
    pub low: f64,
    pub high: f64,
    pub auto_fit: bool,
}

impl Default for PaneScale {
    fn default() -> Self {
        Self {
            low: -1.0,
            high: 1.0,
            auto_fit: true,
        }
    }
}

impl PaneScale {
    const LIMIT: f64 = 1e100;
    const MIN_SPAN: f64 = 1e-100;

    fn pan(&mut self, amount: f64) {
        if !amount.is_finite() || amount == 0.0 {
            return;
        }
        let span = self.high - self.low;
        let limit = (span / (f64::EPSILON * 16.0)).min(Self::LIMIT);
        let delta = (span * amount).clamp(-limit - self.low, limit - self.high);
        self.low += delta;
        self.high += delta;
        self.auto_fit = false;
    }

    /// `anchor` is the fraction measured upward from the low bound.
    fn zoom(&mut self, factor: f64, anchor: f64) {
        if !factor.is_finite() || factor <= 0.0 || factor == 1.0 || !anchor.is_finite() {
            return;
        }
        let anchor = anchor.clamp(0.0, 1.0);
        let old = self.high - self.low;
        let value = self.low + old * anchor;
        // A relative floor keeps both endpoints representable even far from zero,
        // without imposing a price tick on fractional volume or MACD values.
        let minimum = (value.abs() * f64::EPSILON * 16.0).max(Self::MIN_SPAN);
        let span = (old * factor).clamp(minimum, Self::LIMIT);
        let low = (value - span * anchor).clamp(-Self::LIMIT, Self::LIMIT - span);
        self.low = low;
        self.high = low + span;
        self.auto_fit = false;
    }
}

/// Session presentation only: independent of feeds, numerical scales and visibility.
#[derive(Debug, Clone, Copy)]
struct PaneLayout {
    height: f32,
    collapsed: bool,
}

impl Default for PaneLayout {
    fn default() -> Self {
        Self {
            height: default_plot_height(Some(0)),
            collapsed: false,
        }
    }
}

fn default_plot_height(id: Option<u64>) -> f32 {
    if id.is_some() { 190.0 } else { 340.0 }
}

pub(crate) fn bounded_plot_height(id: Option<u64>, height: f32) -> Option<f32> {
    height.is_finite().then(|| {
        if id.is_some() {
            height.clamp(100.0, 800.0)
        } else {
            height.clamp(160.0, 1000.0)
        }
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FlowSource {
    History,
    Live,
}

fn merge_flow_snapshot(
    current: Option<VolumeFlow>,
    mut incoming: VolumeFlow,
    source: FlowSource,
    time: i64,
) -> Option<VolumeFlow> {
    if source == FlowSource::History && incoming.quality == FlowQuality::Missing {
        // Unavailable history has no measured as-of and cannot block a live forming snapshot.
        incoming.through_ms = time.saturating_mul(1000);
    }
    let Some(current) = current else {
        return Some(incoming);
    };
    if incoming.quality == FlowQuality::Missing {
        return match source {
            FlowSource::Live if incoming.through_ms >= current.through_ms => Some(incoming),
            FlowSource::History
                if current.quality == FlowQuality::Missing
                    && incoming.through_ms >= current.through_ms =>
            {
                Some(incoming)
            }
            _ => Some(current),
        };
    }
    if incoming.through_ms > current.through_ms
        || (incoming.through_ms == current.through_ms
            && flow_quality_rank(incoming.quality) >= flow_quality_rank(current.quality))
    {
        Some(incoming)
    } else {
        Some(current)
    }
}

fn flow_quality_rank(quality: FlowQuality) -> u8 {
    match quality {
        FlowQuality::Missing => 0,
        FlowQuality::Reported => 1,
        FlowQuality::Exact => 2,
    }
}

fn same_price(a: Candle, b: Candle) -> bool {
    a.time == b.time
        && a.open == b.open
        && a.high == b.high
        && a.low == b.low
        && a.close == b.close
        && a.volume == b.volume
}

#[derive(Debug, Default)]
pub struct OpenInterestState {
    pub support: Option<OpenInterestSupport>,
    pub samples: Vec<OpenInterestSample>,
    pub connected: bool,
    pub message: String,
}

pub struct ChartState {
    pub market: Market,
    pub price_scale: u32,
    pub base_asset: String,
    // Contiguous chronological storage lets calculations borrow candles without
    // cloning the entire history on each live tick.
    candles: Vec<Candle>,
    stream_epochs: BTreeMap<i64, u64>,
    pending_flow: BTreeMap<i64, (VolumeFlow, FlowSource)>,
    pub indicators: IndicatorSet,
    pub open_interest: OpenInterestState,
    pub indicator_error: Option<String>,
    pub viewport: Viewport,
    pub pane_scales: BTreeMap<u64, PaneScale>,
    price_plot_height: f32,
    pane_layouts: BTreeMap<u64, PaneLayout>,
    pub active_pane: Option<u64>,
    pub inspection: Option<(i64, f64)>,
    // Native inspection retains sub-second source timestamps independently of candles.
    native_inspection_ms: Option<i64>,
    pub epoch: u64,
    pub next_before: Option<i64>,
    pub initialized: bool,
}

impl Default for ChartState {
    fn default() -> Self {
        Self {
            market: Market::default(),
            price_scale: 1,
            base_asset: "BTC".into(),
            candles: Vec::new(),
            stream_epochs: BTreeMap::new(),
            pending_flow: BTreeMap::new(),
            indicators: IndicatorSet::default(),
            open_interest: OpenInterestState::default(),
            indicator_error: None,
            viewport: Viewport::default(),
            pane_scales: BTreeMap::new(),
            price_plot_height: default_plot_height(None),
            pane_layouts: BTreeMap::new(),
            active_pane: None,
            inspection: None,
            native_inspection_ms: None,
            epoch: 0,
            next_before: None,
            initialized: false,
        }
    }
}

impl ChartState {
    pub fn step(&self) -> f64 {
        self.market.interval.seconds() as f64
    }

    pub fn format_price(&self, value: f64) -> String {
        format!(
            "{value:.precision$}",
            precision = self.price_scale.clamp(2, 12) as usize
        )
    }

    fn price_tick(&self) -> f64 {
        10.0_f64.powi(-(self.price_scale.min(12) as i32))
    }

    pub fn select_market(&mut self, market: Market, price_scale: u32) {
        let same_instrument = self.market.exchange == market.exchange
            && self.market.product == market.product
            && self.market.symbol == market.symbol;
        self.viewport = Viewport::for_timeframe(market.interval);
        self.pane_scales.clear();
        self.active_pane = None;
        self.market = market;
        self.price_scale = price_scale;
        self.candles.clear();
        self.stream_epochs.clear();
        self.pending_flow.clear();
        if !same_instrument {
            self.open_interest = OpenInterestState::default();
        }
        self.native_inspection_ms = None;
        self.indicators.recalculate(&[], 0, self.market.interval);
        for instance in &mut self.indicators.instances {
            if !instance.source.is_chart() {
                instance.points.clear();
                instance.external = None;
            }
        }
        self.indicator_error = None;
        self.inspection = None;
        self.epoch = 0;
        self.next_before = None;
        self.initialized = false;
        self.fit_panes(false);
    }

    pub fn latest(&self) -> Option<&Candle> {
        self.candles.last()
    }

    pub fn oldest(&self) -> Option<i64> {
        self.candles.first().map(|candle| candle.time)
    }

    pub fn len(&self) -> usize {
        self.candles.len()
    }

    pub fn visible(&self) -> impl Iterator<Item = &Candle> {
        let left = (self.viewport.left - self.step()).floor() as i64;
        let right = (self.viewport.left + self.viewport.span + self.step()).ceil() as i64;
        let start = self.candles.partition_point(|candle| candle.time < left);
        let end = self.candles.partition_point(|candle| candle.time <= right);
        self.candles[start..end].iter()
    }

    pub fn inspected(&self) -> Option<&Candle> {
        let (time, _) = self.inspection?;
        self.candles
            .binary_search_by_key(&time, |c| c.time)
            .ok()
            .map(|index| &self.candles[index])
    }

    fn nearest(&self, time: i64) -> Option<&Candle> {
        let next = self.candles.partition_point(|candle| candle.time < time);
        let before = next.checked_sub(1).and_then(|i| self.candles.get(i));
        let after = self.candles.get(next);
        match (before, after) {
            (Some(a), Some(b)) => Some(if time.abs_diff(a.time) <= time.abs_diff(b.time) {
                a
            } else {
                b
            }),
            (a, b) => a.or(b),
        }
    }
    pub fn inspection_time_ms(&self) -> Option<i64> {
        self.native_inspection_ms
            .or_else(|| self.inspection.map(|(time, _)| time.saturating_mul(1000)))
    }

    pub fn open_interest_samples_for<'a>(
        &'a self,
        instance: &'a IndicatorInstance,
    ) -> &'a [OpenInterestSample] {
        if instance.source.is_chart() {
            &self.open_interest.samples
        } else {
            instance
                .external
                .as_ref()
                .map_or(&[], |output| output.oi.as_slice())
        }
    }

    pub fn inspected_open_interest_for<'a>(
        &'a self,
        instance: &'a IndicatorInstance,
    ) -> Option<&'a OpenInterestSample> {
        if instance.source.is_chart() {
            return self.inspected_open_interest();
        }
        let output = instance.external.as_ref()?;
        match self.inspection_time_ms() {
            Some(time) => {
                let index = output
                    .oi
                    .partition_point(|sample| sample.timestamp_ms <= time)
                    .checked_sub(1)?;
                let sample = &output.oi[index];
                // Only actual observations inside their proven cadence are inspectable.
                // Do not carry an old total through an excluded contributor gap or into the future.
                let next = output.oi.get(index + 1);
                let cadence = sample.interval_ms.max(
                    next.filter(|next| next.interval_ms > 0)
                        .map_or(0, |next| next.interval_ms),
                );
                let in_segment =
                    next.is_some_and(|next| next.interval_ms > 0 && time < next.timestamp_ms);
                (time == sample.timestamp_ms
                    || (in_segment
                        && cadence > 0
                        && time.saturating_sub(sample.timestamp_ms) <= cadence.saturating_mul(3)))
                .then_some(sample)
            }
            None => output.complete.then(|| output.oi.last()).flatten(),
        }
    }

    pub fn refresh_indicator_sources(
        &mut self,
        sources: &HashMap<Market, SourceData>,
        now_ms: i64,
    ) -> Vec<u64> {
        let changed =
            self.indicators
                .refresh_sources(sources, &self.market, &self.base_asset, now_ms);
        if !changed.is_empty() {
            self.fit_panes_filtered(false, Some(&changed));
        }
        changed
    }

    pub fn inspected_open_interest(&self) -> Option<&OpenInterestSample> {
        let samples = &self.open_interest.samples;
        match self.inspection_time_ms() {
            Some(time) => samples
                .partition_point(|sample| sample.timestamp_ms <= time)
                .checked_sub(1)
                .and_then(|index| samples.get(index)),
            None => samples.last(),
        }
    }

    /// Merge source observations, never aligning to candles or filling gaps.
    /// Shorter-cadence live observations win timestamp collisions; delayed history
    /// cannot overwrite them. Same-cadence repeats preserve the first observation.
    pub fn merge_open_interest_samples(&mut self, incoming: &[OpenInterestSample]) -> bool {
        if self.market.product != Product::Perpetual || incoming.is_empty() {
            return false;
        }
        let sorted;
        let incoming = if incoming
            .windows(2)
            .all(|pair| pair[0].timestamp_ms <= pair[1].timestamp_ms)
        {
            incoming
        } else {
            sorted = {
                let mut samples = incoming.to_vec();
                samples.sort_by_key(|sample| sample.timestamp_ms);
                samples
            };
            &sorted
        };
        let samples = &mut self.open_interest.samples;
        if incoming.first().is_some_and(|sample| {
            samples
                .last()
                .is_none_or(|last| last.timestamp_ms < sample.timestamp_ms)
        }) {
            let old_len = samples.len();
            for &sample in incoming {
                if sample.quantity.is_finite()
                    && sample.quantity >= 0.0
                    && sample.interval_ms > 0
                    && samples
                        .last()
                        .is_none_or(|last| last.timestamp_ms < sample.timestamp_ms)
                {
                    samples.push(sample);
                }
            }
            if samples.len() == old_len {
                return false;
            }
        } else {
            let mut missing = Vec::new();
            let mut replaced = false;
            let mut cursor =
                samples.partition_point(|sample| sample.timestamp_ms < incoming[0].timestamp_ms);
            for &sample in incoming {
                if !sample.quantity.is_finite()
                    || sample.quantity < 0.0
                    || sample.interval_ms <= 0
                    || missing.last().is_some_and(|last: &OpenInterestSample| {
                        last.timestamp_ms == sample.timestamp_ms
                    })
                {
                    continue;
                }
                while cursor < samples.len() && samples[cursor].timestamp_ms < sample.timestamp_ms {
                    cursor += 1;
                }
                if let Some(current) = samples
                    .get_mut(cursor)
                    .filter(|current| current.timestamp_ms == sample.timestamp_ms)
                {
                    if sample.interval_ms < current.interval_ms {
                        *current = sample;
                        replaced = true;
                    }
                } else {
                    missing.push(sample);
                }
            }
            if missing.is_empty() {
                if replaced {
                    self.fit_open_interest_panes(None);
                }
                return replaced;
            }
            // One backwards merge in existing storage; no repeated vector insertion
            // and no scratch copy of the entire accumulated history.
            let mut old = samples.len();
            let mut new = missing.len();
            samples.resize(old + new, missing[0]);
            let mut output = samples.len();
            while new > 0 {
                output -= 1;
                if old > 0 && samples[old - 1].timestamp_ms > missing[new - 1].timestamp_ms {
                    old -= 1;
                    samples[output] = samples[old];
                } else {
                    new -= 1;
                    samples[output] = missing[new];
                }
            }
        }
        self.fit_open_interest_panes(None);
        true
    }

    pub fn merge_stream(&mut self, candle: Candle) -> bool {
        self.stream_epochs.insert(candle.time, self.epoch);
        let (index, price_changed, flow_changed) =
            match self.candles.binary_search_by_key(&candle.time, |c| c.time) {
                Ok(index) => {
                    let current = self.candles[index];
                    let mut merged = candle;
                    merged.flow = current.flow;
                    if let Some(flow) = candle.flow {
                        merged.flow =
                            merge_flow_snapshot(merged.flow, flow, FlowSource::Live, candle.time);
                    }
                    if let Some((flow, source)) = self.pending_flow.remove(&candle.time) {
                        merged.flow = merge_flow_snapshot(merged.flow, flow, source, candle.time);
                    }
                    let price_changed = !same_price(current, candle);
                    let flow_changed = current.flow != merged.flow;
                    if price_changed || flow_changed {
                        self.candles[index] = merged;
                    }
                    (index, price_changed, flow_changed)
                }
                Err(index) => {
                    let mut inserted = candle;
                    if let Some((flow, source)) = self.pending_flow.remove(&candle.time) {
                        inserted.flow =
                            merge_flow_snapshot(inserted.flow, flow, source, candle.time);
                    }
                    self.candles.insert(index, inserted);
                    (index, true, inserted.flow.is_some())
                }
            };
        if price_changed {
            self.indicators
                .recalculate(&self.candles, index, self.market.interval);
        } else if flow_changed {
            self.indicators
                .recalculate_flow(&self.candles, index, self.market.interval);
        } else {
            return false;
        }
        // A corrected past contribution can affect every later visible CVD value.
        let visible =
            self.viewport.live || candle.time as f64 <= self.viewport.left + self.viewport.span;
        if visible {
            if price_changed {
                self.refresh_view();
            } else {
                self.fit_panes(true);
            }
        }
        visible
    }

    /// Apply independent snapshots without changing price history or viewport state.
    /// `live` identifies explicit live Missing revocations versus incomplete history coverage.
    /// Ordered history enrichment walks candles once and recalculates one CVD suffix.
    /// Unordered input remains correct without allocating or sorting a copied batch.
    pub fn merge_flows<I: IntoIterator<Item = FlowBar>>(&mut self, bars: I, live: bool) -> bool {
        let source = if live {
            FlowSource::Live
        } else {
            FlowSource::History
        };
        let mut cursor = 0;
        let mut previous = None;
        let mut from = self.candles.len();
        for bar in bars {
            if previous.is_none_or(|time| bar.time < time) {
                cursor = self
                    .candles
                    .partition_point(|candle| candle.time < bar.time);
            } else {
                while cursor < self.candles.len() && self.candles[cursor].time < bar.time {
                    cursor += 1;
                }
            }
            previous = Some(bar.time);
            if self
                .candles
                .get(cursor)
                .is_some_and(|candle| candle.time == bar.time)
            {
                let current = self.candles[cursor].flow;
                let merged = merge_flow_snapshot(current, bar.flow, source, bar.time);
                if current != merged {
                    self.candles[cursor].flow = merged;
                    from = from.min(cursor);
                }
            } else if self
                .candles
                .last()
                .is_none_or(|latest| bar.time > latest.time)
            {
                let cutoff = bar.time.saturating_sub(2 * self.market.interval.seconds());
                self.pending_flow.retain(|time, _| *time >= cutoff);
                let current = self.pending_flow.get(&bar.time).copied();
                let incoming =
                    merge_flow_snapshot(current.map(|(flow, _)| flow), bar.flow, source, bar.time)
                        .unwrap();
                // Rejected stale input must not relabel a queued live revocation as history.
                if current.is_none_or(|(flow, _)| flow != incoming) {
                    self.pending_flow.insert(bar.time, (incoming, source));
                }
            }
        }
        if from == self.candles.len() {
            return false;
        }
        self.indicators
            .recalculate_flow(&self.candles, from, self.market.interval);
        let visible = self.viewport.live
            || self.candles[from].time as f64 <= self.viewport.left + self.viewport.span;
        if visible {
            self.fit_panes(true);
        }
        visible
    }

    pub fn revoke_live_flow(&mut self) -> bool {
        let width = self.market.interval.seconds();
        for (&time, (flow, source)) in &mut self.pending_flow {
            if flow.quality != FlowQuality::Missing
                && flow.through_ms < time.saturating_add(width).saturating_mul(1000)
            {
                *flow = VolumeFlow {
                    delta: None,
                    volume: None,
                    quality: FlowQuality::Missing,
                    ..*flow
                };
                *source = FlowSource::Live;
            }
        }
        let mut from = self.candles.len();
        for index in self.candles.len().saturating_sub(2)..self.candles.len() {
            let candle = &mut self.candles[index];
            if let Some(flow) = &mut candle.flow
                && flow.quality != FlowQuality::Missing
                && flow.through_ms < candle.time.saturating_add(width).saturating_mul(1000)
            {
                *flow = VolumeFlow {
                    delta: None,
                    volume: None,
                    quality: FlowQuality::Missing,
                    ..*flow
                };
                from = from.min(index);
            }
        }
        if from == self.candles.len() {
            return false;
        }
        self.indicators
            .recalculate_flow(&self.candles, from, self.market.interval);
        self.fit_panes(true);
        true
    }

    /// A REST snapshot never overwrites a candle observed on the stream in this
    /// sync generation, even if the HTTP request began after the stream update.
    pub fn merge_page(&mut self, page: &HistoryPage, epoch: u64) {
        if page.market != self.market {
            return;
        }
        let mut price_changed_time = i64::MAX;
        let mut flow_changed_time = i64::MAX;
        let mut missing = Vec::new();
        for &candle in &page.candles {
            let protected = self
                .stream_epochs
                .get(&candle.time)
                .is_some_and(|&seen| seen >= epoch);
            if !protected {
                self.stream_epochs.remove(&candle.time);
            }
            match self.candles.binary_search_by_key(&candle.time, |c| c.time) {
                Ok(index) => {
                    let current = self.candles[index];
                    let price_changed = !protected && !same_price(current, candle);
                    let mut merged = if price_changed { candle } else { current };
                    merged.flow = current.flow;
                    if let Some(flow) = candle.flow {
                        merged.flow = merge_flow_snapshot(
                            merged.flow,
                            flow,
                            FlowSource::History,
                            candle.time,
                        );
                    }
                    if let Some((flow, source)) = self.pending_flow.remove(&candle.time) {
                        merged.flow = merge_flow_snapshot(merged.flow, flow, source, candle.time);
                    }
                    let flow_changed = current.flow != merged.flow;
                    if price_changed {
                        price_changed_time = price_changed_time.min(candle.time);
                    }
                    if flow_changed || price_changed {
                        self.candles[index] = merged;
                    }
                    if flow_changed {
                        flow_changed_time = flow_changed_time.min(candle.time);
                    }
                }
                Err(_) => {
                    let mut inserted = candle;
                    if let Some((flow, source)) = self.pending_flow.remove(&candle.time) {
                        inserted.flow =
                            merge_flow_snapshot(inserted.flow, flow, source, candle.time);
                    }
                    missing.push(inserted);
                    price_changed_time = price_changed_time.min(candle.time);
                }
            }
        }
        if !missing.is_empty() {
            if self
                .candles
                .last()
                .is_none_or(|last| missing[0].time > last.time)
            {
                self.candles.extend(missing);
            } else {
                // REST pages are sorted/unique. Merge once, rather than shifting
                // the existing history for every inserted older candle.
                let mut merged = Vec::with_capacity(self.candles.len() + missing.len());
                let mut old = self.candles.iter().copied().peekable();
                for candle in missing {
                    while old.peek().is_some_and(|c| c.time < candle.time) {
                        merged.push(old.next().unwrap());
                    }
                    merged.push(candle);
                }
                merged.extend(old);
                self.candles = merged;
            }
        }
        if price_changed_time != i64::MAX {
            let from = self
                .candles
                .partition_point(|c| c.time < price_changed_time.min(flow_changed_time));
            self.indicators
                .recalculate(&self.candles, from, self.market.interval);
        } else if flow_changed_time != i64::MAX {
            let from = self.candles.partition_point(|c| c.time < flow_changed_time);
            self.indicators
                .recalculate_flow(&self.candles, from, self.market.interval);
        }
        if !self.initialized && !self.candles.is_empty() {
            self.initialized = true;
            self.follow_latest();
        }
        self.refresh_view();
    }

    pub fn in_view(&self, time: i64) -> bool {
        let time = time as f64;
        time + self.step() >= self.viewport.left && time <= self.viewport.left + self.viewport.span
    }

    pub fn needs_older(&self) -> bool {
        self.next_before.is_some()
            && self.oldest().is_some_and(|oldest| {
                self.viewport.left <= oldest as f64 + self.viewport.span * 0.1
            })
    }

    fn follow_latest(&mut self) {
        if let Some(latest) = self.latest() {
            self.viewport.left = latest.time as f64 + self.step() * 4.0 - self.viewport.span;
        }
    }

    fn fit(&mut self) {
        let mut low = f64::INFINITY;
        let mut high = f64::NEG_INFINITY;
        for candle in self.visible() {
            low = low.min(candle.low);
            high = high.max(candle.high);
        }
        for indicator in &self.indicators.instances {
            if !indicator.visible || !indicator.spec.is_overlay() {
                continue;
            }
            let start = indicator
                .points
                .partition_point(|point| (point.time as f64) < self.viewport.left);
            for point in indicator.points[start..]
                .iter()
                .take_while(|point| point.time as f64 <= self.viewport.left + self.viewport.span)
            {
                for value in point.values.iter().flatten() {
                    low = low.min(*value);
                    high = high.max(*value);
                }
            }
        }
        if low.is_finite() && high.is_finite() {
            let pad = ((high - low) * 0.08)
                .max(high.abs() * 0.0001)
                .max(self.price_tick());
            self.viewport.low = low - pad;
            self.viewport.high = high + pad;
        }
    }

    pub fn pane_scale(&self, id: u64) -> PaneScale {
        self.pane_scales.get(&id).copied().unwrap_or_default()
    }

    pub fn plot_height(&self, id: Option<u64>) -> f32 {
        match id {
            None => self.price_plot_height,
            Some(id) if self.lower_pane(id) => self
                .pane_layouts
                .get(&id)
                .map_or(default_plot_height(Some(id)), |layout| layout.height),
            Some(id) => default_plot_height(Some(id)),
        }
    }

    pub fn pane_collapsed(&self, id: u64) -> bool {
        self.lower_pane(id)
            && self
                .pane_layouts
                .get(&id)
                .is_some_and(|layout| layout.collapsed)
    }

    fn set_plot_height(&mut self, id: Option<u64>, height: f32) {
        if id.is_some_and(|id| !self.lower_pane(id)) {
            return;
        }
        let Some(height) = bounded_plot_height(id, height) else {
            return;
        };
        if self.plot_height(id) == height {
            return;
        }
        match id {
            None => self.price_plot_height = height,
            Some(id) => self.pane_layouts.entry(id).or_default().height = height,
        }
    }

    fn lower_pane(&self, id: u64) -> bool {
        self.indicators
            .instances
            .iter()
            .any(|instance| instance.id == id && !instance.spec.is_overlay())
    }

    fn activate_pane(&mut self, id: Option<u64>) {
        self.active_pane = id.filter(|id| self.lower_pane(*id));
    }

    fn fit_open_interest_panes(&mut self, only: Option<&[u64]>) {
        let left = self.viewport.left;
        let right = left + self.viewport.span;
        let extent = |samples: &[OpenInterestSample]| {
            let start =
                samples.partition_point(|sample| (sample.timestamp_ms as f64 / 1000.0) < left);
            let end =
                samples.partition_point(|sample| sample.timestamp_ms as f64 / 1000.0 <= right);
            samples[start..end]
                .iter()
                .fold((f64::INFINITY, f64::NEG_INFINITY), |(low, high), sample| {
                    (low.min(sample.quantity), high.max(sample.quantity))
                })
        };
        let mut native_extent = None;
        for instance in &self.indicators.instances {
            if instance.spec != IndicatorSpec::OpenInterest
                || only.is_some_and(|ids| !ids.contains(&instance.id))
            {
                continue;
            }
            if !instance.visible || !self.pane_scale(instance.id).auto_fit {
                continue;
            }
            let (low, high) = if instance.source.is_chart() {
                *native_extent.get_or_insert_with(|| extent(&self.open_interest.samples))
            } else {
                extent(self.open_interest_samples_for(instance))
            };
            let scale = self.pane_scales.entry(instance.id).or_default();
            if low.is_finite() && high.is_finite() {
                let pad = if high == 0.0 {
                    1.0
                } else {
                    ((high - low) * 0.12)
                        .max(high.abs() * 0.001)
                        .max(PaneScale::MIN_SPAN)
                };
                let pad = pad.min(PaneScale::LIMIT * 0.5);
                scale.low = (low - pad).clamp(0.0, PaneScale::LIMIT * 0.5);
                scale.high = (high + pad).min(PaneScale::LIMIT).max(scale.low + pad);
            }
        }
    }

    fn fit_panes(&mut self, flow_only: bool) {
        self.fit_panes_filtered(flow_only, None);
    }

    fn fit_panes_filtered(&mut self, flow_only: bool, only: Option<&[u64]>) {
        if !flow_only {
            self.fit_open_interest_panes(only);
        }
        for instance in &self.indicators.instances {
            if instance.spec.is_overlay()
                || instance.spec == IndicatorSpec::OpenInterest
                || (flow_only && !matches!(instance.spec, IndicatorSpec::Cvd { .. }))
                || only.is_some_and(|ids| !ids.contains(&instance.id))
            {
                continue;
            }
            let scale = self.pane_scales.entry(instance.id).or_default();
            if !scale.auto_fit || !instance.visible {
                continue;
            }
            if matches!(
                instance.spec,
                IndicatorSpec::Rsi { .. } | IndicatorSpec::Stochastic { .. }
            ) {
                scale.low = 0.0;
                scale.high = 100.0;
                continue;
            }
            let start = instance
                .points
                .partition_point(|point| (point.time as f64) < self.viewport.left)
                .saturating_sub(1);
            let end = (instance.points.partition_point(|point| {
                point.time as f64 <= self.viewport.left + self.viewport.span
            }) + 1)
                .min(instance.points.len());
            let mut extent = 0.0_f64;
            for point in &instance.points[start..end] {
                for value in point
                    .values
                    .iter()
                    .flatten()
                    .filter(|value| value.is_finite())
                {
                    extent = extent.max(value.abs());
                }
            }
            let extent = if extent > 0.0 {
                (extent * 1.12).clamp(PaneScale::MIN_SPAN, PaneScale::LIMIT * 0.5)
            } else {
                1.0
            };
            scale.low = if matches!(
                instance.spec,
                IndicatorSpec::Macd { .. } | IndicatorSpec::Cvd { .. }
            ) {
                -extent
            } else {
                0.0
            };
            scale.high = extent;
        }
    }

    fn pan_time(&mut self, amount: f64) {
        if amount.is_finite() && amount != 0.0 {
            self.viewport.left =
                (self.viewport.left + self.viewport.span * amount).clamp(-1e12, 1e12);
            self.viewport.live = false;
        }
    }

    fn zoom_time(&mut self, factor: f64, anchor: f64) {
        if factor.is_finite() && factor > 0.0 && factor != 1.0 && anchor.is_finite() {
            let span =
                (self.viewport.span * factor).clamp(10.0 * self.step(), 10000.0 * self.step());
            self.viewport.left = (self.viewport.left
                + (self.viewport.span - span) * anchor.clamp(0.0, 1.0))
            .clamp(-1e12, 1e12);
            self.viewport.span = span;
            self.viewport.live = false;
        }
    }

    fn refresh_view(&mut self) {
        if self.viewport.live {
            self.follow_latest();
        }
        if self.viewport.auto_fit {
            self.fit();
        }
        self.fit_panes(false);
    }

    /// Returns whether the cached candles/axes need redrawing (inspection does not).
    pub fn dispatch(&mut self, action: Action) -> bool {
        match action {
            // Canvas caches already invalidate on changed bounds. Presentation-only
            // commands must not refit values, rebuild data, or clear unrelated caches.
            Action::SetPaneHeight { id, height } => {
                self.set_plot_height(id, height);
                return false;
            }
            Action::AdjustPaneHeight { id, delta } => {
                if delta.is_finite() {
                    self.set_plot_height(id, self.plot_height(id) + delta);
                }
                return false;
            }
            Action::ResetPaneHeight(id) => {
                self.set_plot_height(id, default_plot_height(id));
                return false;
            }
            Action::TogglePaneCollapsed(id) => {
                if self.lower_pane(id) {
                    let layout = self.pane_layouts.entry(id).or_default();
                    layout.collapsed = !layout.collapsed;
                }
                return false;
            }
            Action::Pan { time, price } => {
                self.pan_time(time);
                if price != 0.0 {
                    self.viewport.auto_fit = false;
                    let delta = (self.viewport.high - self.viewport.low) * price;
                    self.viewport.low += delta;
                    self.viewport.high += delta;
                }
                self.refresh_view();
            }
            Action::Zoom { time, price, x, y } => {
                self.zoom_time(time, x);
                if price != 1.0 {
                    self.viewport.auto_fit = false;
                    let old = self.viewport.high - self.viewport.low;
                    let new = (old * price).clamp(self.price_tick(), 1e9);
                    self.viewport.low += (old - new) * y.clamp(0.0, 1.0);
                    self.viewport.high = self.viewport.low + new;
                }
                self.refresh_view();
            }
            Action::PanePan { id, time, value } => {
                if !self.lower_pane(id) {
                    return false;
                }
                self.active_pane = Some(id);
                self.pan_time(time);
                self.pane_scales.entry(id).or_default().pan(value);
                if time != 0.0 {
                    self.refresh_view();
                }
            }
            Action::PaneZoom {
                id,
                time,
                value,
                x,
                y,
            } => {
                if !self.lower_pane(id) {
                    return false;
                }
                self.active_pane = Some(id);
                self.zoom_time(time, x);
                self.pane_scales.entry(id).or_default().zoom(value, y);
                if time != 1.0 {
                    self.refresh_view();
                }
            }
            Action::FitPane(id) => {
                if !self.lower_pane(id) {
                    return false;
                }
                self.active_pane = Some(id);
                self.pane_scales.entry(id).or_default().auto_fit = true;
                self.fit_panes(false);
            }
            Action::ActivatePane(id) => {
                self.activate_pane(id);
                return false;
            }
            Action::InspectPane { id, point } => {
                self.activate_pane(id);
                self.native_inspection_ms = if id.is_some_and(|id| {
                    self.indicators.instances.iter().any(|instance| {
                        instance.id == id
                            && (instance.spec == IndicatorSpec::OpenInterest
                                || !instance.source.is_chart())
                    })
                }) {
                    point
                        .filter(|(time, _)| time.is_finite())
                        .map(|(time, _)| (time * 1000.0).round() as i64)
                } else {
                    None
                };
                self.inspection = point
                    .filter(|_| {
                        !id.is_some_and(|id| {
                            self.indicators
                                .instances
                                .iter()
                                .any(|instance| instance.id == id && !instance.source.is_chart())
                        })
                    })
                    .and_then(|(time, price)| self.nearest(time as i64).map(|c| (c.time, price)));
                return false;
            }
            Action::Reset => {
                self.viewport = Viewport::for_timeframe(self.market.interval);
                for scale in self.pane_scales.values_mut() {
                    scale.auto_fit = true;
                }
                self.inspection = None;
                self.native_inspection_ms = None;
                self.refresh_view();
            }
            Action::AutoFit => {
                if let Some(id) = self.active_pane.filter(|id| self.lower_pane(*id)) {
                    let scale = self.pane_scales.entry(id).or_default();
                    scale.auto_fit = !scale.auto_fit;
                } else {
                    self.viewport.auto_fit = !self.viewport.auto_fit;
                }
                self.refresh_view();
            }
            Action::Live => {
                self.viewport.live = true;
                self.refresh_view();
            }
            Action::Inspect(point) => {
                self.native_inspection_ms = None;
                self.inspection = point
                    .and_then(|(time, price)| self.nearest(time as i64).map(|c| (c.time, price)));
                return false;
            }
            Action::InspectStep(direction) => {
                if self.active_pane.is_some_and(|id| {
                    self.indicators.instances.iter().any(|instance| {
                        instance.id == id
                            && (instance.spec == IndicatorSpec::OpenInterest
                                || !instance.source.is_chart())
                    })
                }) {
                    let time = self.inspection_time_ms().unwrap_or(
                        ((self.viewport.left + self.viewport.span * 0.5) * 1000.0) as i64,
                    );
                    let instance = self
                        .indicators
                        .instances
                        .iter()
                        .find(|instance| Some(instance.id) == self.active_pane)
                        .unwrap();
                    if matches!(instance.spec, IndicatorSpec::Cvd { .. }) {
                        let seconds = time.div_euclid(1000);
                        let index = if direction < 0 {
                            instance
                                .points
                                .partition_point(|point| point.time < seconds)
                                .checked_sub(1)
                        } else {
                            Some(
                                instance
                                    .points
                                    .partition_point(|point| point.time <= seconds),
                            )
                        };
                        if let Some(point) = index.and_then(|index| instance.points.get(index)) {
                            let seconds = point.time as f64;
                            self.native_inspection_ms = Some(point.time.saturating_mul(1000));
                            if seconds < self.viewport.left
                                || seconds > self.viewport.left + self.viewport.span
                            {
                                self.viewport.live = false;
                                self.viewport.left = seconds - self.viewport.span * 0.5;
                                self.refresh_view();
                                return true;
                            }
                        }
                        return false;
                    }
                    let samples = self.open_interest_samples_for(instance);
                    let index = if direction < 0 {
                        samples
                            .partition_point(|sample| sample.timestamp_ms < time)
                            .checked_sub(1)
                    } else {
                        Some(samples.partition_point(|sample| sample.timestamp_ms <= time))
                    };
                    if let Some(sample) = index.and_then(|index| samples.get(index)).copied() {
                        self.native_inspection_ms = Some(sample.timestamp_ms);
                        self.inspection = self
                            .nearest(sample.timestamp_ms.div_euclid(1000))
                            .map(|candle| (candle.time, candle.close));
                        let seconds = sample.timestamp_ms as f64 / 1000.0;
                        if seconds < self.viewport.left
                            || seconds > self.viewport.left + self.viewport.span
                        {
                            self.viewport.live = false;
                            self.viewport.left = seconds - self.viewport.span * 0.5;
                            self.refresh_view();
                            return true;
                        }
                    }
                    return false;
                }
                self.native_inspection_ms = None;
                let time = self
                    .inspection
                    .map(|(time, _)| time)
                    .unwrap_or((self.viewport.left + self.viewport.span * 0.5) as i64);
                let index = if direction < 0 {
                    self.candles
                        .partition_point(|c| c.time < time)
                        .checked_sub(1)
                } else {
                    Some(self.candles.partition_point(|c| c.time <= time))
                };
                let candle = index.and_then(|index| self.candles.get(index)).copied();
                if let Some(candle) = candle {
                    self.inspection = Some((candle.time, candle.close));
                    if !self.in_view(candle.time) {
                        self.viewport.live = false;
                        self.viewport.left = candle.time as f64 - self.viewport.span * 0.5;
                        self.refresh_view();
                        return true;
                    }
                }
                return false;
            }
            Action::AddIndicator(spec) => {
                self.indicator_error = self
                    .indicators
                    .add(spec, &self.candles, self.market.interval)
                    .err();
                self.refresh_view();
            }
            Action::ConfigureIndicator { id, spec } => {
                self.indicator_error = self
                    .indicators
                    .configure(id, spec, &self.candles, self.market.interval)
                    .err();
                if self.indicator_error.is_none() && spec.is_overlay() {
                    self.pane_scales.remove(&id);
                    self.pane_layouts.remove(&id);
                    if self.active_pane == Some(id) {
                        self.active_pane = None;
                    }
                }
                self.refresh_view();
            }
            Action::ApplyIndicator { id, spec, source } => {
                // Validate with the same library APIs before touching the live instance.
                // This avoids cloning calculation histories and guarantees rejected edits are atomic.
                let validation = (|| {
                    spec.validate()?;
                    if id.is_some_and(|id| {
                        !self.indicators.instances.iter().any(|item| item.id == id)
                    }) {
                        return Err(format!("Unknown indicator #{}", id.unwrap()));
                    }
                    let mut candidate = IndicatorSet::default();
                    let candidate_id = candidate.add(spec, &[], self.market.interval)?;
                    candidate.configure_source(candidate_id, source.clone())
                })();
                if let Err(error) = validation {
                    self.indicator_error = Some(error);
                    return false;
                }
                let returning_to_chart = source.is_chart()
                    && id.is_some_and(|id| {
                        self.indicators
                            .instances
                            .iter()
                            .any(|item| item.id == id && !item.source.is_chart())
                    });
                let candles = if source.is_chart() {
                    self.candles.as_slice()
                } else {
                    &[]
                };
                let applied = match id {
                    Some(id) => self
                        .indicators
                        .configure(id, spec, candles, self.market.interval)
                        .map(|_| id),
                    None => self.indicators.add(spec, candles, self.market.interval),
                };
                match applied {
                    Ok(id) => {
                        self.indicator_error = self.indicators.configure_source(id, source).err();
                        if returning_to_chart && self.indicator_error.is_none() {
                            // The first configure skipped native calculation while the old external
                            // source was still installed. Rebuild only this instance after cutover.
                            self.indicator_error = self
                                .indicators
                                .configure(id, spec, &self.candles, self.market.interval)
                                .err();
                        }
                        if spec.is_overlay() {
                            self.pane_scales.remove(&id);
                            self.pane_layouts.remove(&id);
                            if self.active_pane == Some(id) {
                                self.active_pane = None;
                            }
                        }
                    }
                    Err(error) => self.indicator_error = Some(error),
                }
                self.refresh_view();
            }
            Action::ToggleIndicator(id) => {
                self.indicator_error = self.indicators.toggle(id, &self.candles).err();
                if self.active_pane == Some(id)
                    && self
                        .indicators
                        .instances
                        .iter()
                        .any(|instance| instance.id == id && !instance.visible)
                {
                    self.active_pane = None;
                }
                self.refresh_view();
            }
            Action::RemoveIndicator(id) => {
                self.indicators.remove(id);
                self.pane_scales.remove(&id);
                self.pane_layouts.remove(&id);
                if self.active_pane == Some(id) {
                    self.active_pane = None;
                }
                self.indicator_error = None;
                self.refresh_view();
            }
            Action::Older
            | Action::Retry
            | Action::ManageIndicators
            | Action::ManageMarket
            | Action::SelectMarket(_)
            | Action::ScrollPanes { .. } => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mexc_indicators::CvdReset;
    use mexc_model::{Exchange, FlowBar, FlowQuality, Product, VolumeFlow};
    fn candle(time: i64, close: f64) -> Candle {
        Candle {
            time,
            open: 10.0,
            high: close.max(11.0),
            low: 9.0,
            close,
            volume: 1.0,
            flow: None,
        }
    }
    fn page(candles: Vec<Candle>) -> HistoryPage {
        HistoryPage {
            market: Market::default(),
            candles,
            next_before: None,
        }
    }

    #[test]
    fn apply_rejects_source_and_spec_together_without_partial_edits() {
        let mut state = ChartState::default();
        state.dispatch(Action::AddIndicator(IndicatorSpec::Sma { period: 10 }));
        let id = state.indicators.instances[0].id;
        state.dispatch(Action::ApplyIndicator {
            id: Some(id),
            spec: IndicatorSpec::OpenInterest,
            source: IndicatorSource {
                mode: mexc_indicators::SourceMode::Single,
                markets: vec![],
            },
        });
        assert!(state.indicator_error.is_some());
        assert_eq!(
            state.indicators.instances[0].spec,
            IndicatorSpec::Sma { period: 10 }
        );
        assert!(state.indicators.instances[0].source.is_chart());
        assert_eq!(state.indicators.instances.len(), 1);
    }

    #[test]
    fn returning_external_cvd_to_chart_rebuilds_native_points_and_keeps_identity() {
        let mut state = ChartState::default();
        let mut bar = candle(0, 11.0);
        bar.flow = Some(exact_flow(3.0, 0, 300_000));
        state.candles = vec![bar];
        let spec = IndicatorSpec::Cvd {
            reset: CvdReset::Continuous,
        };
        state.dispatch(Action::ApplyIndicator {
            id: None,
            spec,
            source: IndicatorSource {
                mode: mexc_indicators::SourceMode::Single,
                markets: vec![Market::default()],
            },
        });
        let id = state.indicators.instances[0].id;
        assert!(state.indicators.instances[0].points.is_empty());
        state.dispatch(Action::ApplyIndicator {
            id: Some(id),
            spec,
            source: IndicatorSource::default(),
        });
        assert!(state.indicator_error.is_none());
        assert_eq!(state.indicators.instances[0].id, id);
        assert_eq!(state.indicators.instances[0].points[0].values[0], Some(3.0));
        assert!(state.indicators.instances[0].source.is_chart());
    }

    #[test]
    fn external_oi_current_incomplete_hides_total_but_exact_history_remains_inspectable() {
        let mut state = ChartState::default();
        state.dispatch(Action::ApplyIndicator {
            id: None,
            spec: IndicatorSpec::OpenInterest,
            source: IndicatorSource {
                mode: mexc_indicators::SourceMode::Single,
                markets: vec![Market::default()],
            },
        });
        let id = state.indicators.instances[0].id;
        state.indicators.instances[0].external = Some(mexc_indicators::ExternalIndicatorOutput {
            oi: vec![OpenInterestSample {
                timestamp_ms: 10_123,
                timestamp_source: mexc_model::OpenInterestTimestamp::Venue,
                quantity: 0.0,
                interval_ms: 10_000,
            }],
            unit: "BTC".into(),
            status: "disconnected".into(),
            complete: false,
            asof_range_ms: None,
            contexts: vec![],
            context_asof_ranges_ms: vec![],
            oi_asof_ranges_ms: vec![(10_123, 10_123)],
            contributors: vec![],
        });
        assert!(
            state
                .inspected_open_interest_for(&state.indicators.instances[0])
                .is_none()
        );
        state.dispatch(Action::InspectPane {
            id: Some(id),
            point: Some((10.123, 0.0)),
        });
        assert_eq!(
            state
                .inspected_open_interest_for(&state.indicators.instances[0])
                .unwrap()
                .quantity,
            0.0
        );
        state.dispatch(Action::InspectPane {
            id: Some(id),
            point: Some((11.0, 0.0)),
        });
        assert!(
            state
                .inspected_open_interest_for(&state.indicators.instances[0])
                .is_none()
        );
        state.pane_scales.insert(
            id,
            PaneScale {
                low: 1.0,
                high: 4.0,
                auto_fit: false,
            },
        );
        state.fit_panes(false);
        assert_eq!(
            state.pane_scale(id),
            PaneScale {
                low: 1.0,
                high: 4.0,
                auto_fit: false
            }
        );
    }

    fn exact_flow(delta: f64, covered_from: i64, through_ms: i64) -> VolumeFlow {
        VolumeFlow {
            delta: Some(delta),
            volume: Some(delta.abs() + 0.25),
            quality: FlowQuality::Exact,
            covered_from,
            through_ms,
        }
    }

    fn missing_flow(covered_from: i64, through_ms: i64) -> VolumeFlow {
        VolumeFlow {
            delta: None,
            volume: None,
            quality: FlowQuality::Missing,
            covered_from,
            through_ms,
        }
    }

    #[test]
    fn prepend_and_live_update_keep_exploration_anchored() {
        let mut state = ChartState::default();
        state.merge_page(&page(vec![candle(600, 10.0)]), 0);
        state.dispatch(Action::Pan {
            time: -0.5,
            price: 0.0,
        });
        let left = state.viewport.left;
        state.merge_page(&page(vec![candle(0, 10.0), candle(300, 10.0)]), 0);
        state.merge_stream(candle(900, 12.0));
        assert_eq!(state.viewport.left, left);
        assert_eq!(state.len(), 4);
    }

    #[test]
    fn stream_wins_over_rest_until_next_sync_generation() {
        let mut state = ChartState {
            epoch: 1,
            ..ChartState::default()
        };
        state.merge_stream(candle(300, 12.0));
        state.merge_page(&page(vec![candle(300, 11.0)]), 1);
        assert_eq!(state.latest().unwrap().close, 12.0);
        state.epoch = 2;
        state.merge_page(&page(vec![candle(300, 13.0)]), 2);
        assert_eq!(state.latest().unwrap().close, 13.0);
        assert_eq!(state.len(), 1);
    }

    #[test]
    fn price_and_time_controls_are_independent() {
        let mut state = ChartState::default();
        state.merge_page(&page(vec![candle(300, 10.0)]), 0);
        let old = state.viewport;
        state.dispatch(Action::Zoom {
            time: 0.5,
            price: 1.0,
            x: 0.25,
            y: 0.5,
        });
        assert_eq!(
            state.viewport.left + state.viewport.span * 0.25,
            old.left + old.span * 0.25
        );
        state.dispatch(Action::Pan {
            time: 0.0,
            price: 0.5,
        });
        assert!(!state.viewport.auto_fit);
        state.dispatch(Action::Reset);
        assert!(state.viewport.live && state.viewport.auto_fit);
    }

    #[test]
    fn indicator_values_follow_protected_merges_and_history_reseeding() {
        let mut state = ChartState::default();
        state.merge_page(
            &page(vec![
                candle(300, 10.0),
                candle(600, 20.0),
                candle(900, 30.0),
            ]),
            0,
        );
        state.dispatch(Action::AddIndicator(IndicatorSpec::Ema { period: 2 }));
        state.epoch = 1;
        state.merge_stream(candle(900, 40.0));
        let value = |state: &ChartState| {
            state.indicators.instances[0].points.last().unwrap().values[0].unwrap()
        };
        assert!((value(&state) - (15.0 + (40.0 - 15.0) * 2.0 / 3.0)).abs() < 1e-10);
        let before_stale_page = value(&state);
        state.merge_page(&page(vec![candle(900, 25.0)]), 1);
        assert_eq!(value(&state), before_stale_page);
        state.dispatch(Action::Pan {
            time: -0.5,
            price: 0.0,
        });
        let left = state.viewport.left;
        state.merge_page(&page(vec![candle(0, 12.0)]), 1);
        assert_eq!(state.viewport.left, left);
        let expected = 17.0 + (40.0 - 17.0) * 2.0 / 3.0;
        assert!((value(&state) - expected).abs() < 1e-10);
        assert_eq!(state.indicators.instances[0].points[0].values[0], None);
        assert_eq!(
            state.indicators.instances[0].points[1].values[0],
            Some(11.0)
        );
    }

    #[test]
    fn overlay_fit_and_instance_actions_preserve_independent_parameters() {
        let mut state = ChartState::default();
        state.merge_page(
            &page(vec![
                candle(300, 100.0),
                candle(600, 10.0),
                candle(900, 10.0),
            ]),
            0,
        );
        state.dispatch(Action::AddIndicator(IndicatorSpec::Sma { period: 3 }));
        let id = state.indicators.instances[0].id;
        state.dispatch(Action::AddIndicator(IndicatorSpec::Sma { period: 2 }));
        let second_id = state.indicators.instances[1].id;
        state.viewport.live = false;
        state.viewport.left = 900.0;
        state.viewport.span = 300.0;
        state.refresh_view();
        assert!(state.viewport.high > 40.0);
        state.dispatch(Action::ToggleIndicator(id));
        assert!(state.viewport.high < 20.0);
        state.dispatch(Action::ConfigureIndicator {
            id: second_id,
            spec: IndicatorSpec::Sma { period: 0 },
        });
        assert!(state.indicator_error.is_some());
        assert_eq!(
            state.indicators.instances[1].spec,
            IndicatorSpec::Sma { period: 2 }
        );
        state.dispatch(Action::RemoveIndicator(id));
        assert_eq!(state.indicators.instances.len(), 1);
        assert_eq!(state.indicators.instances[0].id, second_id);
        assert_eq!(
            state.indicators.instances[0].points.last().unwrap().values[0],
            Some(10.0)
        );
    }

    #[test]
    fn switching_markets_preserves_instances_and_reseeds_without_old_prices() {
        let mut state = ChartState::default();
        state.merge_page(
            &page(vec![
                candle(300, 10.0),
                candle(600, 20.0),
                candle(900, 30.0),
            ]),
            0,
        );
        state.dispatch(Action::AddIndicator(IndicatorSpec::Ema { period: 2 }));
        state.dispatch(Action::AddIndicator(IndicatorSpec::Rsi { period: 2 }));
        state.dispatch(Action::AddIndicator(IndicatorSpec::Cvd {
            reset: CvdReset::UtcDay,
        }));
        let ema_id = state.indicators.instances[0].id;
        let rsi_id = state.indicators.instances[1].id;
        let cvd_id = state.indicators.instances[2].id;
        state.dispatch(Action::ToggleIndicator(ema_id));
        state.dispatch(Action::Inspect(Some((900.0, 30.0))));
        state.dispatch(Action::ToggleIndicator(cvd_id));
        state.viewport.live = false;
        state.viewport.auto_fit = false;
        state.next_before = Some(300);
        state.epoch = 10;

        let market = Market {
            exchange: Exchange::Mexc,
            product: Product::Perpetual,
            symbol: "PEPE_USDC".into(),
            interval: Timeframe::Day1,
        };
        state.select_market(market.clone(), 9);
        assert_eq!(state.len(), 0);
        assert_eq!(state.next_before, None);
        assert_eq!(state.inspection, None);
        assert_eq!(state.epoch, 0);
        assert!(state.viewport.live && state.viewport.auto_fit);
        assert_eq!(state.viewport.span, 120.0 * 86400.0);
        assert_eq!(state.indicators.instances[0].id, ema_id);
        assert_eq!(
            state.indicators.instances[0].spec,
            IndicatorSpec::Ema { period: 2 }
        );
        assert!(!state.indicators.instances[0].visible);
        assert_eq!(state.indicators.instances[1].id, rsi_id);
        assert_eq!(
            state.indicators.instances[1].spec,
            IndicatorSpec::Rsi { period: 2 }
        );
        assert!(state.indicators.instances[1].visible);
        assert_eq!(state.indicators.instances[2].id, cvd_id);
        assert_eq!(
            state.indicators.instances[2].spec,
            IndicatorSpec::Cvd {
                reset: CvdReset::UtcDay
            }
        );
        assert!(!state.indicators.instances[2].visible);
        assert!(
            state
                .indicators
                .instances
                .iter()
                .all(|instance| instance.points.is_empty())
        );
        state.merge_page(&page(vec![candle(900, 30.0)]), 0);
        assert_eq!(state.len(), 0);
        state.merge_page(
            &HistoryPage {
                market,
                candles: [0.000001, 0.000002, 0.000003]
                    .into_iter()
                    .enumerate()
                    .map(|(index, close)| Candle {
                        time: (index as i64 + 1) * 86400,
                        open: close,
                        high: close * 1.01,
                        low: close * 0.99,
                        close,
                        volume: 1.0,
                        flow: None,
                    })
                    .collect(),
                next_before: None,
            },
            0,
        );
        assert_eq!(state.indicators.instances[0].points[0].values[0], None);
        assert!(
            (state.indicators.instances[0].points[2].values[0].unwrap() - 0.0000025).abs() < 1e-15
        );
        assert_eq!(state.indicators.instances[1].points[1].values[0], None);
        assert_eq!(
            state.indicators.instances[1].points[2].values[0],
            Some(100.0)
        );
        assert!(state.viewport.high - state.viewport.low < 0.00001);
        assert_eq!(state.format_price(0.000001234), "0.000001234");
        state.dispatch(Action::AddIndicator(IndicatorSpec::Sma { period: 3 }));
        assert!(state.indicators.instances[3].id > cvd_id);
    }

    #[test]
    fn selected_interval_sets_zoom_bounds_and_reset_bar_count() {
        let mut state = ChartState::default();
        for interval in Timeframe::ALL {
            state.select_market(
                Market {
                    exchange: Exchange::Mexc,
                    product: Product::Perpetual,
                    symbol: "ETH_USDC".into(),
                    interval,
                },
                2,
            );
            state.dispatch(Action::Zoom {
                time: 1e-9,
                price: 1.0,
                x: 0.5,
                y: 0.5,
            });
            assert_eq!(state.viewport.span, 10.0 * interval.seconds() as f64);
            state.dispatch(Action::Zoom {
                time: 1e9,
                price: 1.0,
                x: 0.5,
                y: 0.5,
            });
            assert_eq!(state.viewport.span, 10000.0 * interval.seconds() as f64);
            state.dispatch(Action::Reset);
            assert_eq!(state.viewport.span, 120.0 * interval.seconds() as f64);
            assert!(state.viewport.auto_fit && state.viewport.live);
        }
    }
    #[test]
    fn flow_snapshots_merge_independently_with_freshness_revocation_and_recovery() {
        let mut state = ChartState::default();
        state.merge_page(
            &page(vec![
                candle(0, 10.0),
                candle(300, 10.0),
                candle(600, 10.0),
                candle(900, 10.0),
            ]),
            0,
        );
        state.dispatch(Action::AddIndicator(IndicatorSpec::Cvd {
            reset: CvdReset::Continuous,
        }));
        state.viewport.live = false;
        state.viewport.auto_fit = false;
        state.viewport.left = 0.0;
        state.viewport.span = 1200.0;
        state.viewport.low = 8.0;
        state.viewport.high = 12.0;
        let view = state.viewport;
        for (time, delta, through_ms) in [
            (0, 1.0, 300_000),
            (300, 2.25, 600_000),
            (600, -1.5, 900_000),
            (900, 1.0, 1_200_000),
        ] {
            state.merge_flows(
                [FlowBar {
                    time,
                    flow: exact_flow(delta, 0, through_ms),
                }],
                true,
            );
        }
        let value = |state: &ChartState, index: usize| {
            state.indicators.instances[0].points[index].values[0]
        };
        assert_eq!(state.len(), 4);
        assert_eq!(value(&state, 3), Some(2.75));

        state.merge_flows(
            [FlowBar {
                time: 600,
                flow: exact_flow(-2.75, 0, 900_000),
            }],
            true,
        );
        assert_eq!(value(&state, 2), Some(0.5));
        assert_eq!(value(&state, 3), Some(1.5));
        state.merge_flows(
            [FlowBar {
                time: 600,
                flow: exact_flow(4.0, 0, 899_999),
            }],
            true,
        );
        assert_eq!(value(&state, 3), Some(1.5));

        state.merge_flows(
            [FlowBar {
                time: 600,
                flow: missing_flow(0, 900_000),
            }],
            false,
        );
        assert_eq!(value(&state, 3), Some(1.5));
        state.merge_flows(
            [FlowBar {
                time: 600,
                flow: missing_flow(0, 900_000),
            }],
            true,
        );
        assert_eq!(value(&state, 2), None);
        assert_eq!(value(&state, 3), None);
        state.merge_flows(
            [FlowBar {
                time: 600,
                flow: exact_flow(-2.75, 0, 900_000),
            }],
            true,
        );
        assert_eq!(value(&state, 2), Some(0.5));
        assert_eq!(value(&state, 3), Some(1.5));

        state.epoch = 1;
        state.merge_stream(candle(600, 20.0));
        let mut stale_price = candle(600, 11.0);
        stale_price.flow = Some(missing_flow(0, 900_000));
        state.merge_page(&page(vec![stale_price]), 1);
        state.inspection = Some((600, 20.0));
        assert_eq!(state.inspected().unwrap().close, 20.0);
        assert_eq!(value(&state, 3), Some(1.5));

        state.merge_flows(
            [FlowBar {
                time: 300,
                flow: exact_flow(3.0, 0, 600_000),
            }],
            true,
        );
        assert_eq!(value(&state, 2), Some(1.25));
        assert_eq!(value(&state, 3), Some(2.25));
        assert_eq!(state.viewport.left, view.left);
        assert_eq!(state.viewport.span, view.span);
        assert_eq!(state.viewport.low, view.low);
        assert_eq!(state.viewport.high, view.high);
    }

    #[test]
    fn unavailable_history_allows_measured_forming_flow_but_live_revocation_blocks_stale_flow() {
        let mut state = ChartState::default();
        state.merge_page(&page(vec![candle(300, 10.0)]), 0);
        state.dispatch(Action::AddIndicator(IndicatorSpec::Cvd {
            reset: CvdReset::Continuous,
        }));
        state.merge_flows(
            [FlowBar {
                time: 300,
                flow: missing_flow(300, 600_000),
            }],
            false,
        );
        assert_eq!(state.indicators.instances[0].points[0].values[0], None);
        state.merge_flows(
            [FlowBar {
                time: 300,
                flow: exact_flow(-0.125, 300, 450_000),
            }],
            true,
        );
        assert_eq!(
            state.indicators.instances[0].points[0].values[0],
            Some(-0.125)
        );
        assert_eq!(
            state.indicators.instances[0]
                .cvd_context(0)
                .unwrap()
                .through_ms,
            450_000
        );

        state.merge_flows(
            [FlowBar {
                time: 300,
                flow: missing_flow(300, 500_000),
            }],
            true,
        );
        state.merge_flows(
            [FlowBar {
                time: 300,
                flow: exact_flow(0.25, 300, 475_000),
            }],
            true,
        );
        assert_eq!(state.indicators.instances[0].points[0].values[0], None);
        state.merge_flows(
            [FlowBar {
                time: 300,
                flow: exact_flow(0.5, 300, 550_000),
            }],
            true,
        );
        assert_eq!(state.indicators.instances[0].points[0].values[0], Some(0.5));
    }

    #[test]
    fn connection_loss_revokes_forming_and_pending_flow_without_losing_closed_history() {
        let mut state = ChartState::default();
        state.merge_page(&page(vec![candle(0, 10.0), candle(300, 10.0)]), 0);
        state.dispatch(Action::AddIndicator(IndicatorSpec::Cvd {
            reset: CvdReset::Continuous,
        }));
        state.merge_flows(
            [FlowBar {
                time: 0,
                flow: exact_flow(1.0, 0, 300_000),
            }],
            true,
        );
        state.merge_flows(
            [FlowBar {
                time: 300,
                flow: exact_flow(2.0, 300, 450_000),
            }],
            true,
        );
        state.merge_flows(
            [FlowBar {
                time: 600,
                flow: exact_flow(0.75, 600, 650_000),
            }],
            true,
        );
        assert_eq!(state.indicators.instances[0].points[1].values[0], Some(3.0));

        state.revoke_live_flow();
        assert_eq!(state.indicators.instances[0].points[0].values[0], Some(1.0));
        assert_eq!(state.indicators.instances[0].points[1].values[0], None);
        state.merge_stream(candle(600, 10.0));
        assert_eq!(state.indicators.instances[0].points[2].values[0], None);
        state.merge_flows(
            [FlowBar {
                time: 300,
                flow: exact_flow(2.0, 300, 600_000),
            }],
            true,
        );
        state.merge_flows(
            [FlowBar {
                time: 600,
                flow: exact_flow(0.75, 600, 750_000),
            }],
            true,
        );
        assert_eq!(
            state.indicators.instances[0].points[2].values[0],
            Some(3.75)
        );
    }

    #[test]
    fn flow_waits_for_its_native_price_bar_without_synthesizing_candles() {
        let mut state = ChartState::default();
        state.dispatch(Action::AddIndicator(IndicatorSpec::Cvd {
            reset: CvdReset::Continuous,
        }));
        assert!(!state.merge_flows(
            [FlowBar {
                time: 300,
                flow: exact_flow(2.5, 300, 600_000),
            }],
            true,
        ));
        assert_eq!(state.len(), 0);
        state.merge_page(&page(vec![candle(0, 10.0), candle(300, 10.0)]), 0);
        assert_eq!(state.len(), 2);
        assert_eq!(state.indicators.instances[0].points[1].values[0], Some(2.5));
    }

    fn add_pane(state: &mut ChartState, spec: IndicatorSpec) -> u64 {
        state.dispatch(Action::AddIndicator(spec));
        state.indicators.instances.last().unwrap().id
    }

    #[test]
    fn layout_commands_preserve_manual_values_time_inspection_and_cvd_storage() {
        let mut state = ChartState::default();
        state.merge_page(&page(vec![candle(0, 10.0)]), 0);
        let id = add_pane(
            &mut state,
            IndicatorSpec::Cvd {
                reset: CvdReset::Continuous,
            },
        );
        state.merge_flows(
            [FlowBar {
                time: 0,
                flow: exact_flow(-0.000125, 0, 300_000),
            }],
            true,
        );
        state.dispatch(Action::PanePan {
            id,
            time: -0.25,
            value: 0.5,
        });
        state.dispatch(Action::Pan {
            time: 0.0,
            price: 0.25,
        });
        state.dispatch(Action::Inspect(Some((0.0, 10.0))));
        let viewport = state.viewport;
        let scale = state.pane_scale(id);
        let inspection = state.inspection;
        let active = state.active_pane;
        let points = state.indicators.instances[0].points.as_ptr();
        let value = state.indicators.instances[0].points[0].values;
        let origin = state.indicators.instances[0].cvd_context(0).unwrap().origin;
        let candles = state.candles.as_ptr();
        for action in [
            Action::SetPaneHeight {
                id: None,
                height: 456.0,
            },
            Action::AdjustPaneHeight {
                id: Some(id),
                delta: 32.0,
            },
            Action::TogglePaneCollapsed(id),
            Action::ResetPaneHeight(None),
        ] {
            assert!(!state.dispatch(action));
            assert_eq!(
                (
                    state.viewport.left,
                    state.viewport.span,
                    state.viewport.low,
                    state.viewport.high
                ),
                (viewport.left, viewport.span, viewport.low, viewport.high),
            );
            assert_eq!(
                (state.viewport.live, state.viewport.auto_fit),
                (viewport.live, viewport.auto_fit)
            );
            assert_eq!(state.pane_scale(id), scale);
            assert_eq!(state.inspection, inspection);
            assert_eq!(state.active_pane, active);
            assert_eq!(state.candles.as_ptr(), candles);
            assert_eq!(state.indicators.instances[0].points.as_ptr(), points);
            assert_eq!(state.indicators.instances[0].points[0].values, value);
            assert_eq!(
                state.indicators.instances[0].cvd_context(0).unwrap().origin,
                origin
            );
        }
        // A collapsed instance remains visible to the producer and receives new values.
        assert!(state.pane_collapsed(id));
        assert!(state.indicators.instances[0].visible);
        state.merge_stream(candle(300, 11.0));
        state.merge_flows(
            [FlowBar {
                time: 300,
                flow: exact_flow(0.00025, 300, 600_000),
            }],
            true,
        );
        state.dispatch(Action::TogglePaneCollapsed(id));
        assert!(!state.pane_collapsed(id));
        assert_eq!(
            state.indicators.instances[0].points[1].values[0],
            Some(0.000125)
        );
        assert_eq!(state.pane_scale(id), scale);
    }

    #[test]
    fn layout_survives_visibility_edits_and_native_transitions_but_not_overlay_or_removal() {
        let mut state = ChartState::default();
        let id = add_pane(&mut state, IndicatorSpec::Rsi { period: 2 });
        state.dispatch(Action::SetPaneHeight {
            id: Some(id),
            height: 333.0,
        });
        state.dispatch(Action::SetPaneHeight {
            id: None,
            height: 444.0,
        });
        state.dispatch(Action::TogglePaneCollapsed(id));
        state.dispatch(Action::ToggleIndicator(id));
        state.dispatch(Action::ConfigureIndicator {
            id,
            spec: IndicatorSpec::Rsi { period: 3 },
        });
        state.dispatch(Action::ApplyIndicator {
            id: Some(id),
            spec: IndicatorSpec::Cvd {
                reset: CvdReset::UtcDay,
            },
            source: IndicatorSource {
                mode: mexc_indicators::SourceMode::Single,
                markets: vec![state.market.clone()],
            },
        });
        assert!(state.indicator_error.is_none());
        state.dispatch(Action::ToggleIndicator(id));
        for market in [
            Market {
                interval: Timeframe::Min15,
                ..state.market.clone()
            },
            Market {
                symbol: "OTHERUSDT".into(),
                ..state.market.clone()
            },
        ] {
            state.select_market(market, 8);
            assert_eq!(state.plot_height(None), 444.0);
            assert_eq!(state.plot_height(Some(id)), 333.0);
            assert!(state.pane_collapsed(id));
        }
        state.dispatch(Action::ConfigureIndicator {
            id,
            spec: IndicatorSpec::Ema { period: 2 },
        });
        assert!(!state.pane_layouts.contains_key(&id));
        state.dispatch(Action::ConfigureIndicator {
            id,
            spec: IndicatorSpec::Rsi { period: 2 },
        });
        assert_eq!(state.plot_height(Some(id)), 190.0);
        assert!(!state.pane_collapsed(id));
        state.dispatch(Action::SetPaneHeight {
            id: Some(id),
            height: 345.0,
        });
        state.dispatch(Action::RemoveIndicator(id));
        let new_id = add_pane(&mut state, IndicatorSpec::Rsi { period: 2 });
        assert_ne!(new_id, id);
        assert!(!state.pane_layouts.contains_key(&id));
        state.dispatch(Action::SetPaneHeight {
            id: Some(id),
            height: 600.0,
        });
        state.dispatch(Action::TogglePaneCollapsed(id));
        assert_eq!(state.plot_height(Some(new_id)), 190.0);
        assert!(!state.pane_collapsed(new_id));
    }

    #[test]
    fn height_bounds_and_invalid_targets_are_deterministic_and_atomic() {
        let mut state = ChartState::default();
        let id = add_pane(&mut state, IndicatorSpec::Rsi { period: 2 });
        let overlay = add_pane(&mut state, IndicatorSpec::Ema { period: 2 });
        for (target, minimum, maximum) in [(None, 160.0, 1000.0), (Some(id), 100.0, 800.0)] {
            state.dispatch(Action::SetPaneHeight {
                id: target,
                height: -f32::MAX,
            });
            assert_eq!(state.plot_height(target), minimum);
            state.dispatch(Action::AdjustPaneHeight {
                id: target,
                delta: f32::MAX,
            });
            assert_eq!(state.plot_height(target), maximum);
            state.dispatch(Action::AdjustPaneHeight {
                id: target,
                delta: -32.0,
            });
            assert_eq!(state.plot_height(target), maximum - 32.0);
            for invalid in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
                state.dispatch(Action::SetPaneHeight {
                    id: target,
                    height: invalid,
                });
                state.dispatch(Action::AdjustPaneHeight {
                    id: target,
                    delta: invalid,
                });
                assert_eq!(state.plot_height(target), maximum - 32.0);
            }
            state.dispatch(Action::ResetPaneHeight(target));
            assert_eq!(state.plot_height(target), default_plot_height(target));
        }
        for wrong in [overlay, u64::MAX] {
            state.dispatch(Action::SetPaneHeight {
                id: Some(wrong),
                height: 600.0,
            });
            state.dispatch(Action::AdjustPaneHeight {
                id: Some(wrong),
                delta: 32.0,
            });
            state.dispatch(Action::ResetPaneHeight(Some(wrong)));
            state.dispatch(Action::TogglePaneCollapsed(wrong));
            assert!(!state.pane_layouts.contains_key(&wrong));
            assert!(!state.pane_collapsed(wrong));
        }
        state.dispatch(Action::SetPaneHeight {
            id: Some(id),
            height: 320.0,
        });
        state.dispatch(Action::TogglePaneCollapsed(id));
        state.dispatch(Action::ConfigureIndicator {
            id,
            spec: IndicatorSpec::Rsi { period: 0 },
        });
        assert_eq!(state.plot_height(Some(id)), 320.0);
        assert!(state.pane_collapsed(id));
        state.dispatch(Action::ApplyIndicator {
            id: Some(id),
            spec: IndicatorSpec::Ema { period: 0 },
            source: IndicatorSource::default(),
        });
        assert_eq!(state.plot_height(Some(id)), 320.0);
        assert!(state.pane_collapsed(id));
        state.dispatch(Action::ApplyIndicator {
            id: Some(id),
            spec: IndicatorSpec::Ema { period: 2 },
            source: IndicatorSource::default(),
        });
        assert!(!state.pane_layouts.contains_key(&id));
    }

    #[test]
    fn pane_zoom_preserves_value_and_shared_time_anchors_independently() {
        let mut state = ChartState::default();
        state.merge_page(&page(vec![candle(0, 10.0)]), 0);
        let id = add_pane(&mut state, IndicatorSpec::Rsi { period: 2 });
        let other = add_pane(
            &mut state,
            IndicatorSpec::Stochastic {
                period: 2,
                smooth_k: 2,
                smooth_d: 2,
            },
        );
        let price = (state.viewport.low, state.viewport.high);
        let before = state.viewport;
        state.dispatch(Action::PaneZoom {
            id,
            time: 0.5,
            value: 0.4,
            x: 0.3,
            y: 0.75,
        });
        let scale = state.pane_scale(id);
        assert!((scale.low + (scale.high - scale.low) * 0.75 - 75.0).abs() < 1e-12);
        assert!(
            (state.viewport.left + state.viewport.span * 0.3 - (before.left + before.span * 0.3))
                .abs()
                < 1e-9
        );
        assert_eq!((state.viewport.low, state.viewport.high), price);
        assert_eq!(
            (state.pane_scale(other).low, state.pane_scale(other).high),
            (0.0, 100.0)
        );
        let left = state.viewport.left;
        state.dispatch(Action::PanePan {
            id,
            time: 0.0,
            value: -0.5,
        });
        assert_eq!(state.viewport.left, left);
        assert_eq!(state.pane_scale(id).low, scale.low - 20.0);
        assert_eq!(state.active_pane, Some(id));
    }

    #[test]
    fn fractional_negative_scales_remain_finite_without_price_tick_quantization() {
        for (low, high) in [(-2e-18, 1e-18), (-20.0, -10.0), (0.0, 100.0)] {
            let mut scale = PaneScale {
                low,
                high,
                auto_fit: true,
            };
            let anchor = low + (high - low) * 0.8;
            scale.zoom(0.2, 0.8);
            let current = scale.low + (scale.high - scale.low) * 0.8;
            assert!((current - anchor).abs() <= (high - low) * 1e-14);
            assert!((scale.high - scale.low - (high - low) * 0.2).abs() <= (high - low) * 1e-14);
            scale.pan(-0.25);
            let unchanged = scale;
            scale.zoom(f64::NAN, 0.5);
            scale.zoom(-1.0, 0.5);
            scale.pan(f64::INFINITY);
            assert_eq!(scale, unchanged);
            for _ in 0..200 {
                scale.zoom(0.01, 0.8);
            }
            scale.pan(f64::MAX);
            assert!(scale.low.is_finite() && scale.high.is_finite() && scale.high > scale.low);
            scale.zoom(f64::MAX, 0.1);
            assert!(scale.low.is_finite() && scale.high.is_finite() && scale.high > scale.low);
        }
    }

    #[test]
    fn manual_panes_survive_ticks_navigation_hide_show_and_configuration() {
        let mut state = ChartState::default();
        state.merge_page(&page(vec![candle(0, 10.0), candle(300, 12.0)]), 0);
        let id = add_pane(&mut state, IndicatorSpec::Rsi { period: 2 });
        state.dispatch(Action::PaneZoom {
            id,
            time: 1.0,
            value: 0.5,
            x: 0.5,
            y: 0.25,
        });
        let manual = state.pane_scale(id);
        state.merge_stream(candle(600, 25.0));
        state.dispatch(Action::PanePan {
            id,
            time: -0.5,
            value: 0.0,
        });
        state.merge_page(&page(vec![candle(-300, 15.0)]), 0);
        state.dispatch(Action::ToggleIndicator(id));
        state.merge_stream(candle(900, 30.0));
        state.dispatch(Action::ConfigureIndicator {
            id,
            spec: IndicatorSpec::Rsi { period: 3 },
        });
        state.dispatch(Action::ToggleIndicator(id));
        assert_eq!(state.pane_scale(id), manual);
        assert_eq!(state.indicators.instances[0].points.len(), state.len());
        state.dispatch(Action::FitPane(id));
        assert_eq!(
            state.pane_scale(id),
            PaneScale {
                low: 0.0,
                high: 100.0,
                auto_fit: true
            }
        );
    }

    #[test]
    fn reset_market_switch_and_removal_manage_scales_without_resetting_cvd() {
        let mut state = ChartState::default();
        state.merge_page(&page(vec![candle(0, 10.0)]), 0);
        let id = add_pane(
            &mut state,
            IndicatorSpec::Cvd {
                reset: CvdReset::Continuous,
            },
        );
        state.merge_flows(
            [FlowBar {
                time: 0,
                flow: exact_flow(-0.000125, 0, 300_000),
            }],
            true,
        );
        let point = state.indicators.instances[0].points[0];
        let origin = state.indicators.instances[0].cvd_context(0).unwrap().origin;
        let fitted = state.pane_scale(id);
        assert!(fitted.low < 0.0 && fitted.high < 0.001);
        state.dispatch(Action::PaneZoom {
            id,
            time: 1.0,
            value: 0.2,
            x: 0.5,
            y: 0.8,
        });
        state.dispatch(Action::Reset);
        assert_eq!(state.pane_scale(id), fitted);
        assert_eq!(state.indicators.instances[0].points[0].values, point.values);
        assert_eq!(
            state.indicators.instances[0].cvd_context(0).unwrap().origin,
            origin
        );
        state.dispatch(Action::PanePan {
            id,
            time: 0.0,
            value: 0.25,
        });
        let market = Market {
            symbol: "OTHERUSDT".into(),
            ..state.market.clone()
        };
        state.select_market(market, 8);
        assert_eq!(state.indicators.instances[0].id, id);
        assert!(state.pane_scale(id).auto_fit);
        assert_eq!(state.active_pane, None);
        assert_eq!(state.pane_scale(id), PaneScale::default());
        state.dispatch(Action::ActivatePane(Some(id)));
        state.dispatch(Action::RemoveIndicator(id));
        assert!(!state.pane_scales.contains_key(&id));
        assert_eq!(state.active_pane, None);
    }

    #[test]
    fn targeted_autofit_and_overlay_reconfiguration_do_not_leave_stale_panes() {
        let mut state = ChartState::default();
        let id = add_pane(&mut state, IndicatorSpec::Rsi { period: 2 });
        state.dispatch(Action::PanePan {
            id,
            time: 0.0,
            value: 0.5,
        });
        state.dispatch(Action::AutoFit);
        assert!(state.pane_scale(id).auto_fit);
        assert!(state.viewport.auto_fit);
        state.dispatch(Action::AutoFit);
        assert!(!state.pane_scale(id).auto_fit);
        state.dispatch(Action::ConfigureIndicator {
            id,
            spec: IndicatorSpec::Ema { period: 2 },
        });
        assert_eq!(state.active_pane, None);
        assert!(!state.pane_scales.contains_key(&id));
        assert!(!state.dispatch(Action::FitPane(id)));
    }

    #[test]
    fn batched_enrichment_matches_individual_freshness_revocation_and_recovery() {
        let mut batch = ChartState::default();
        let mut singles = ChartState::default();
        for state in [&mut batch, &mut singles] {
            state.merge_page(
                &page(vec![candle(0, 10.0), candle(300, 10.0), candle(600, 10.0)]),
                0,
            );
            add_pane(
                state,
                IndicatorSpec::Cvd {
                    reset: CvdReset::Continuous,
                },
            );
        }
        let runs = [
            (
                false,
                vec![
                    FlowBar {
                        time: 0,
                        flow: exact_flow(1.0, 0, 300_000),
                    },
                    FlowBar {
                        time: 300,
                        flow: exact_flow(-0.25, 0, 600_000),
                    },
                    FlowBar {
                        time: 600,
                        flow: exact_flow(2.0, 0, 900_000),
                    },
                ],
            ),
            (
                true,
                vec![
                    FlowBar {
                        time: 300,
                        flow: missing_flow(0, 600_000),
                    },
                    FlowBar {
                        time: 300,
                        flow: exact_flow(10.0, 0, 599_999),
                    },
                    FlowBar {
                        time: 600,
                        flow: exact_flow(3.0, 0, 900_000),
                    },
                ],
            ),
            (
                false,
                vec![FlowBar {
                    time: 300,
                    flow: missing_flow(0, 900_000),
                }],
            ),
            (
                true,
                vec![FlowBar {
                    time: 300,
                    flow: exact_flow(-0.5, 0, 600_000),
                }],
            ),
        ];
        for (live, bars) in runs {
            let mut visible = false;
            for &bar in &bars {
                visible |= singles.merge_flows([bar], live);
            }
            assert_eq!(batch.merge_flows(bars, live), visible);
            for (a, b) in batch.indicators.instances[0]
                .points
                .iter()
                .zip(&singles.indicators.instances[0].points)
            {
                assert_eq!(a.values, b.values);
            }
            assert_eq!(batch.pane_scale(1), singles.pane_scale(1));
        }
        assert_eq!(batch.indicators.instances[0].points[2].values[0], Some(3.5));
    }

    #[test]
    fn page_price_correction_does_not_skip_an_earlier_flow_correction() {
        let mut state = ChartState::default();
        state.merge_page(&page(vec![candle(0, 10.0), candle(300, 10.0)]), 0);
        add_pane(
            &mut state,
            IndicatorSpec::Cvd {
                reset: CvdReset::Continuous,
            },
        );
        state.merge_flows(
            [
                FlowBar {
                    time: 0,
                    flow: exact_flow(1.0, 0, 300_000),
                },
                FlowBar {
                    time: 300,
                    flow: exact_flow(2.0, 0, 600_000),
                },
            ],
            false,
        );
        let mut earlier = candle(0, 10.0);
        earlier.flow = Some(exact_flow(-1.0, 0, 300_000));
        state.merge_page(&page(vec![earlier, candle(300, 20.0)]), 0);
        assert_eq!(state.indicators.instances[0].points[1].values[0], Some(1.0));
    }
    fn oi(timestamp_ms: i64, quantity: f64, interval_ms: i64) -> OpenInterestSample {
        OpenInterestSample {
            timestamp_ms,
            quantity,
            interval_ms,
            timestamp_source: mexc_model::OpenInterestTimestamp::Venue,
        }
    }

    #[test]
    fn native_open_interest_merges_pages_zero_and_live_without_candle_alignment() {
        let mut state = ChartState::default();
        assert!(state.merge_open_interest_samples(&[oi(20_123, 5.0, 10_000)]));
        assert!(state.merge_open_interest_samples(&[
            oi(1_234, 0.0, 300_000),
            oi(10_123, 3.0, 300_000),
            oi(20_123, 999.0, 300_000),
        ]));
        assert_eq!(
            state.open_interest.samples,
            vec![
                oi(1_234, 0.0, 300_000),
                oi(10_123, 3.0, 300_000),
                oi(20_123, 5.0, 10_000),
            ]
        );
        assert!(!state.merge_open_interest_samples(&[oi(20_123, 9.0, 10_000)]));
        assert!(state.merge_open_interest_samples(&[oi(10_123, 4.0, 10_000)]));
        assert_eq!(state.open_interest.samples[1].quantity, 4.0);
        assert!(!state.merge_open_interest_samples(&[
            oi(30_000, -1.0, 10_000),
            oi(30_001, f64::NAN, 10_000),
            oi(30_002, f64::INFINITY, 10_000),
            oi(30_003, 3.0, 0),
        ]));
        assert!(
            state.merge_open_interest_samples(&[oi(40_000, 8.0, 10_000), oi(30_000, 7.0, 10_000)])
        );
        assert!(
            state
                .open_interest
                .samples
                .windows(2)
                .all(|pair| pair[0].timestamp_ms < pair[1].timestamp_ms)
        );
        assert!(state.candles.is_empty());
    }

    #[test]
    fn native_panes_keep_independent_manual_scales_and_reset_only_on_market_change() {
        let mut state = ChartState::default();
        state.viewport.live = false;
        state.viewport.left = 0.0;
        state.viewport.span = 100.0;
        let id = add_pane(&mut state, IndicatorSpec::OpenInterest);
        let other = add_pane(&mut state, IndicatorSpec::OpenInterest);
        state.merge_open_interest_samples(&[oi(20_123, 20.0, 10_000)]);
        assert!(state.pane_scale(id).low <= 20.0 && state.pane_scale(id).high >= 20.0);
        state.dispatch(Action::PanePan {
            id,
            time: 0.0,
            value: 0.2,
        });
        let manual = state.pane_scale(id);
        let view = state.viewport;
        state.merge_open_interest_samples(&[oi(30_123, 200.0, 10_000)]);
        state.merge_open_interest_samples(&[oi(10_123, 0.0, 300_000)]);
        assert_eq!(state.pane_scale(id), manual);
        assert_eq!(state.viewport.left, view.left);
        assert_eq!(state.viewport.span, view.span);
        assert!(state.pane_scale(other).auto_fit);
        assert!(state.pane_scale(other).high >= 200.0);
        assert!(
            state
                .indicators
                .instances
                .iter()
                .all(|instance| instance.points.is_empty())
        );
        state.dispatch(Action::ToggleIndicator(id));
        state.dispatch(Action::ToggleIndicator(id));
        assert_eq!(state.pane_scale(id), manual);
        let samples = state.open_interest.samples.clone();
        let mut market = state.market.clone();
        market.interval = Timeframe::Min1;
        state.select_market(market.clone(), state.price_scale);
        assert_eq!(state.open_interest.samples, samples);
        market.symbol = "ETH_USDC".into();
        state.select_market(market, state.price_scale);
        assert!(state.open_interest.samples.is_empty());
        assert!(state.open_interest.support.is_none());
        assert!(!state.open_interest.connected);
        assert!(state.open_interest.message.is_empty());
    }

    #[test]
    fn native_cursor_inspects_as_of_and_steps_actual_millisecond_observations() {
        let mut state = ChartState::default();
        state.viewport.left = 0.0;
        state.viewport.span = 100.0;
        let id = add_pane(&mut state, IndicatorSpec::OpenInterest);
        state.merge_open_interest_samples(&[oi(10_123, 0.0, 10_000), oi(20_456, 5.0, 10_000)]);
        state.dispatch(Action::InspectPane {
            id: Some(id),
            point: Some((5.0, 0.0)),
        });
        assert!(state.inspected_open_interest().is_none());
        state.dispatch(Action::InspectPane {
            id: Some(id),
            point: Some((15.999, 0.0)),
        });
        assert_eq!(state.inspection_time_ms(), Some(15_999));
        assert_eq!(
            state.inspected_open_interest().unwrap().timestamp_ms,
            10_123
        );
        state.dispatch(Action::InspectStep(1));
        assert_eq!(state.inspection_time_ms(), Some(20_456));
        state.dispatch(Action::InspectStep(-1));
        assert_eq!(state.inspection_time_ms(), Some(10_123));
        state.market.product = Product::Spot;
        assert!(!state.merge_open_interest_samples(&[oi(30_000, 3.0, 10_000)]));
    }
}
