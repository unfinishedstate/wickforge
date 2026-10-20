//! Explicit synthetic load scenarios for measuring the real iced window, not exchange data.
//! Normal launches collect no samples and use no fixture data.
use crate::{
    drawing_types::{Anchor, Command, Shape, Style},
    drawings::Store,
    state::{Action, ChartState},
};
use iced::widget::canvas;
use mexc_indicators::{IndicatorKind, IndicatorSpec};
use mexc_model::{
    Candle, FlowQuality, HistoryPage, OpenInterestSample, OpenInterestTimestamp, VolumeFlow,
};
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

const SAMPLE_LIMIT: usize = 4096;

#[derive(Clone, Copy)]
pub struct Scenario {
    pub bars: usize,
    pub drawings: usize,
    pub indicators: usize,
}
impl Scenario {
    pub fn from_args() -> Option<Self> {
        std::env::args().skip(1).find_map(|arg| match arg.as_str() {
            "--stress" => Some(Self {
                bars: 100_000,
                drawings: 1_000,
                indicators: 24,
            }),
            "--stress-small" => Some(Self {
                bars: 1_000,
                drawings: 10,
                indicators: 3,
            }),
            _ => None,
        })
    }
}

struct Samples {
    values: Vec<u64>,
    next: usize,
}
impl Samples {
    fn new() -> Self {
        Self {
            values: Vec::with_capacity(SAMPLE_LIMIT),
            next: 0,
        }
    }
    fn push(&mut self, duration: Duration) {
        let value = duration.as_nanos().min(u64::MAX as u128) as u64;
        if self.values.len() < SAMPLE_LIMIT {
            self.values.push(value);
        } else {
            self.values[self.next] = value;
            self.next = (self.next + 1) % SAMPLE_LIMIT;
        }
    }
    fn report(&self, label: &str) {
        if self.values.is_empty() {
            println!("{label}: no samples");
            return;
        }
        let mut sorted = self.values.clone();
        sorted.sort_unstable();
        let median = sorted[(sorted.len() - 1) / 2] as f64 / 1e6;
        let p95 = sorted[((sorted.len() - 1) * 95).div_ceil(100)] as f64 / 1e6;
        let maximum = *sorted.last().unwrap() as f64 / 1e6;
        println!(
            "{label}: samples={} median_ms={median:.4} p95_ms={p95:.4} max_ms={maximum:.4}",
            sorted.len()
        );
    }
}

pub struct Metrics {
    pub scenario: Scenario,
    started: Instant,
    updates: RefCell<Samples>,
    views: RefCell<Samples>,
    canvases: RefCell<Samples>,
    redraws: RefCell<Samples>,
    pending_input: Cell<Option<Instant>>,
    pulse: Cell<bool>,
}
impl Metrics {
    pub fn new(scenario: Scenario) -> Self {
        Self {
            scenario,
            started: Instant::now(),
            updates: RefCell::new(Samples::new()),
            views: RefCell::new(Samples::new()),
            canvases: RefCell::new(Samples::new()),
            redraws: RefCell::new(Samples::new()),
            pending_input: Cell::new(None),
            pulse: Cell::new(false),
        }
    }
    pub fn input(&self) {
        self.pending_input.set(Some(Instant::now()));
    }
    pub fn update_sample(&self, duration: Duration) {
        self.updates.borrow_mut().push(duration);
    }
    pub fn view_sample(&self, duration: Duration) {
        self.views.borrow_mut().push(duration);
    }
    pub fn canvas_timer(&self) -> Timer<'_> {
        Timer {
            metrics: self,
            started: Instant::now(),
        }
    }
    pub fn frame(&self, at: Instant) {
        if let Some(input) = self.pending_input.get()
            && let Some(elapsed) = at.checked_duration_since(input)
        {
            self.pending_input.set(None);
            self.redraws.borrow_mut().push(elapsed);
        }
    }
    pub fn pan(&self) -> f64 {
        let next = !self.pulse.get();
        self.pulse.set(next);
        if next { -0.02 } else { 0.02 }
    }
    pub fn label(&self) -> String {
        format!(
            "SYNTHETIC STRESS · {} bars · {} drawings · {} indicators · 20s · no native feeds",
            self.scenario.bars, self.scenario.drawings, self.scenario.indicators
        )
    }
    pub fn report(&self) {
        println!(
            "STRESS synthetic=true bars={} drawings={} indicators={} wall_sec={:.2}",
            self.scenario.bars,
            self.scenario.drawings,
            self.scenario.indicators,
            self.started.elapsed().as_secs_f64()
        );
        self.updates.borrow().report("input_handler_cpu");
        self.views.borrow().report("widget_tree_cpu");
        self.canvases
            .borrow()
            .report("canvas_build_or_cache_lookup_cpu");
        self.redraws
            .borrow()
            .report("input_to_redraw_event_not_gpu_presentation");
        #[cfg(target_os = "linux")]
        match std::fs::read_to_string("/proc/self/status") {
            Ok(status) => {
                for line in status
                    .lines()
                    .filter(|line| line.starts_with("VmRSS:") || line.starts_with("VmHWM:"))
                {
                    println!("{line}");
                }
            }
            Err(error) => println!("process_memory: unavailable ({error})"),
        }
        #[cfg(not(target_os = "linux"))]
        println!("process_memory: unavailable on this platform (Linux /proc metrics only)");
        println!(
            "CPU/cache timings and event-loop latency only; no GPU presentation timing or memory-leak claim."
        );
    }
}
pub struct Timer<'a> {
    metrics: &'a Metrics,
    started: Instant,
}
impl Drop for Timer<'_> {
    fn drop(&mut self) {
        self.metrics
            .canvases
            .borrow_mut()
            .push(self.started.elapsed());
    }
}

pub fn populate(
    chart: &mut ChartState,
    drawings: &mut Store,
    caches: &mut BTreeMap<u64, canvas::Cache>,
    scenario: Scenario,
) {
    let step = chart.market.interval.seconds();
    let latest = crate::source_controller::now_ms()
        .div_euclid(1000)
        .div_euclid(step)
        * step;
    let first = latest - (scenario.bars as i64 - 1) * step;
    let candles = (0..scenario.bars)
        .map(|index| {
            let time = first + index as i64 * step;
            let phase = index as f64 * 0.07;
            let open = 80_000.0 + phase.sin() * 200.0;
            let close = 80_000.0 + (phase + 0.04).sin() * 200.0;
            Candle {
                time,
                open,
                high: open.max(close) + 4.0,
                low: open.min(close) - 4.0,
                close,
                volume: 100.0,
                flow: Some(VolumeFlow {
                    delta: Some(phase.sin() * 4.0),
                    volume: Some(100.0),
                    quality: FlowQuality::Exact,
                    covered_from: time,
                    through_ms: (time + step).saturating_mul(1000),
                }),
            }
        })
        .collect();
    chart.price_scale = 2;
    chart.merge_page(
        &HistoryPage {
            market: chart.market.clone(),
            candles,
            next_before: None,
        },
        chart.epoch,
    );
    for index in 0..scenario.indicators {
        let kind = IndicatorKind::ALL[index % IndicatorKind::ALL.len()];
        let spec = if kind == IndicatorKind::Ema {
            IndicatorSpec::Ema { period: 20 + index }
        } else {
            kind.default_spec()
        };
        chart.dispatch(Action::AddIndicator(spec));
    }
    let observations: Vec<_> = chart
        .candles()
        .iter()
        .step_by(10)
        .map(|candle| OpenInterestSample {
            timestamp_ms: candle.time.saturating_mul(1000),
            quantity: 20_000.0 + (candle.close - 80_000.0),
            interval_ms: step.saturating_mul(10_000),
            timestamp_source: OpenInterestTimestamp::Observed,
        })
        .collect();
    chart.merge_open_interest_samples(&observations);
    chart.open_interest.connected = true;
    chart.open_interest.message = "Synthetic stress observations; not venue data".into();
    for instance in &chart.indicators.instances {
        caches.insert(instance.id, canvas::Cache::new());
    }
    for index in 0..scenario.drawings {
        let a = Anchor {
            utc_ms: (latest - (110 - index % 100) as i64 * step).saturating_mul(1000),
            price: 79_810.0 + (index % 380) as f64,
        };
        let b = Anchor {
            utc_ms: a.utc_ms + (10 + index % 20) as i64 * step * 1000,
            price: a.price + 20.0,
        };
        let shape = match index % 4 {
            0 => Shape::Trend(a, b),
            1 => Shape::Rectangle(a, b),
            2 => Shape::Horizontal(a),
            _ => Shape::Ray(a, b),
        };
        drawings
            .apply(Command::Create {
                shape,
                style: Style::default(),
            })
            .expect("valid synthetic stress geometry");
    }
    drawings.select(None);
}
