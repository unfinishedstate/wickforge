mod executions;
mod open_interest;
mod providers;

use std::{collections::BTreeMap, sync::Arc, time::Duration};

use anyhow::{Context, Result, ensure};
use mexc_model::{
    Catalog, Exchange, FlowBar, FlowQuality, HistoryEvent, HistoryPage, Instrument, Market,
    MarketEvent, Product, QuantityMode, VolumeFlow,
};
use tokio::{
    sync::{Mutex, OnceCell, mpsc, watch},
    time::{Instant, timeout},
};

const IO_TIMEOUT: Duration = Duration::from_secs(5);
const CATALOG_TTL: Duration = Duration::from_secs(60);
const FAILED_CATALOG_TTL: Duration = Duration::from_secs(1);
const CATALOG_SCOPE_COUNT: usize = Exchange::ALL.len() * Product::ALL.len();
type CatalogSlot = Mutex<Arc<OnceCell<CachedCatalog>>>;

/// Shared native exchange client and scope-local, single-flight catalogs.
/// Dropping a request future cancels its work; no service tasks are detached.
#[derive(Clone)]
pub struct DataService {
    state: Arc<State>,
}

struct State {
    http: reqwest::Client,
    flow_http: reqwest::Client,
    catalogs: [CatalogSlot; CATALOG_SCOPE_COUNT],
}

struct CachedCatalog {
    loaded: Instant,
    result: std::result::Result<Arc<Catalog>, Arc<anyhow::Error>>,
}

#[derive(Debug)]
struct CatalogError(Arc<anyhow::Error>);

impl std::fmt::Display for CatalogError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("native catalog fetch failed")
    }
}

impl std::error::Error for CatalogError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.0.as_ref().as_ref())
    }
}

impl DataService {
    pub fn new() -> Result<Self> {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(20))
            .user_agent("mexc-chart/0.1")
            .build()
            .context("constructing native exchange HTTP client")?;
        // Archives can exceed the ordinary response deadline while still downloading.
        let flow_http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .read_timeout(Duration::from_secs(20))
            .user_agent("mexc-chart/0.1")
            .build()
            .context("constructing native execution-history HTTP client")?;
        Ok(Self {
            state: Arc::new(State {
                http,
                flow_http,
                catalogs: std::array::from_fn(|_| Mutex::new(Arc::new(OnceCell::new()))),
            }),
        })
    }

    pub async fn catalog(&self, exchange: Exchange, product: Product) -> Result<Arc<Catalog>> {
        let cell = {
            let index = exchange.index() * Product::ALL.len() + product.index();
            let mut cell = self.state.catalogs[index].lock().await;
            if let Some(cached) = cell.get() {
                let ttl = if cached.result.is_ok() {
                    CATALOG_TTL
                } else {
                    FAILED_CATALOG_TTL
                };
                if cached.loaded.elapsed() >= ttl {
                    *cell = Arc::new(OnceCell::new());
                }
            }
            Arc::clone(&cell)
        };
        let cached = cell
            .get_or_init(|| async {
                let result = providers::catalog(&self.state.http, exchange, product)
                    .await
                    .map(Arc::new)
                    .map_err(Arc::new);
                CachedCatalog {
                    loaded: Instant::now(),
                    result,
                }
            })
            .await;
        match &cached.result {
            Ok(catalog) => Ok(Arc::clone(catalog)),
            Err(error) => Err(CatalogError(Arc::clone(error)).into()),
        }
    }

    /// Explicit retry bypasses only a completed failed discovery in this exact scope.
    /// Successful catalogs and shared in-flight discovery remain reusable.
    pub async fn retry_catalog(
        &self,
        exchange: Exchange,
        product: Product,
    ) -> Result<Arc<Catalog>> {
        self.invalidate_failed_catalog(exchange, product).await;
        self.catalog(exchange, product).await
    }

    async fn invalidate_failed_catalog(&self, exchange: Exchange, product: Product) {
        let index = exchange.index() * Product::ALL.len() + product.index();
        let mut cell = self.state.catalogs[index].lock().await;
        if cell.get().is_some_and(|cached| cached.result.is_err()) {
            *cell = Arc::new(OnceCell::new());
        }
    }

    async fn validate_market(&self, market: &Market) -> Result<(Arc<Catalog>, usize)> {
        let (catalog, index) = self.validate_instrument(market).await?;
        ensure!(
            catalog.intervals.contains(&market.interval),
            "{} {} does not natively support interval {}",
            market.exchange,
            market.product,
            market.interval
        );
        Ok((catalog, index))
    }

    async fn validate_instrument(&self, market: &Market) -> Result<(Arc<Catalog>, usize)> {
        let catalog = self.catalog(market.exchange, market.product).await?;
        let index = catalog
            .instruments
            .iter()
            .position(|instrument| instrument.symbol == market.symbol)
            .with_context(|| {
                format!(
                    "Unsupported {} {} market symbol {:?}",
                    market.exchange, market.product, market.symbol
                )
            })?;
        Ok((catalog, index))
    }

    /// Priced native executions, independent of the displayed candle interval.
    /// Missing replay and unproved source continuity are reported in coverage.
    pub async fn execution_history(
        &self,
        market: &Market,
        start_ms: i64,
        end_ms: i64,
    ) -> Result<mexc_model::executions::ExecutionPage> {
        ensure!(
            start_ms >= 0 && end_ms > start_ms,
            "invalid execution history range"
        );
        let (catalog, index) = self.validate_instrument(market).await?;
        providers::execution_history(
            &self.state.flow_http,
            &catalog.instruments[index],
            start_ms,
            end_ms,
        )
        .await
    }

    /// One shared native tape per venue/product/symbol. Dropping this future or
    /// closing the receiver cancels its owned connection, including reconnects.
    pub async fn execution_stream(
        &self,
        market: Market,
        sender: mpsc::Sender<mexc_model::executions::ExecutionEvent>,
        shutdown: watch::Receiver<bool>,
    ) -> Result<()> {
        if *shutdown.borrow() || sender.is_closed() {
            return Ok(());
        }
        let operation = async {
            let (catalog, index) = self.validate_instrument(&market).await?;
            executions::stream(
                &self.state.flow_http,
                &catalog.instruments[index],
                sender.clone(),
                shutdown.clone(),
            )
            .await
        };
        tokio::select! {
            _ = executions::cancelled(shutdown.clone()) => Ok(()),
            _ = sender.closed() => Ok(()),
            result = operation => result,
        }
    }

    pub async fn history(&self, market: &Market, before: Option<i64>) -> Result<HistoryPage> {
        let (catalog, index) = self.validate_market(market).await?;
        providers::history(
            &self.state.http,
            &catalog.instruments[index],
            market,
            before,
            QuantityMode::Native,
        )
        .await
    }

    /// Flow quantities are in the catalog-confirmed base asset; OHLC and candle
    /// volume retain the venue's native semantics.
    pub async fn history_in_base(
        &self,
        market: &Market,
        before: Option<i64>,
    ) -> Result<HistoryPage> {
        let (catalog, index) = self.validate_market(market).await?;
        providers::history(
            &self.state.http,
            &catalog.instruments[index],
            market,
            before,
            QuantityMode::Base,
        )
        .await
    }

    /// Native price is delivered before optional flow enrichment. An active receiver
    /// gets exactly one Finished or Error terminal event, including on request errors.
    pub async fn history_with_flow(
        &self,
        market: Market,
        before: Option<i64>,
        sender: mpsc::Sender<HistoryEvent>,
    ) -> Result<()> {
        self.history_with_quantity(market, before, sender, QuantityMode::Native)
            .await
    }

    pub async fn history_with_flow_in_base(
        &self,
        market: Market,
        before: Option<i64>,
        sender: mpsc::Sender<HistoryEvent>,
    ) -> Result<()> {
        self.history_with_quantity(market, before, sender, QuantityMode::Base)
            .await
    }

    async fn history_with_quantity(
        &self,
        market: Market,
        before: Option<i64>,
        sender: mpsc::Sender<HistoryEvent>,
        quantity: QuantityMode,
    ) -> Result<()> {
        if sender.is_closed() {
            return Ok(());
        }
        let result = tokio::select! {
            _ = sender.closed() => return Ok(()),
            result = self.enriched_history(&market, before, &sender, quantity) => result,
        };
        let terminal = match &result {
            Ok(()) => HistoryEvent::Finished,
            Err(error) => HistoryEvent::Error {
                message: format!("{error:#}"),
            },
        };
        if sender.send(terminal).await.is_err() {
            return Ok(());
        }
        result
    }

    async fn enriched_history(
        &self,
        market: &Market,
        before: Option<i64>,
        sender: &mpsc::Sender<HistoryEvent>,
        quantity: QuantityMode,
    ) -> Result<()> {
        let (catalog, index) = self.validate_market(market).await?;
        let instrument = &catalog.instruments[index];
        let page = Arc::new(
            providers::history(&self.state.http, instrument, market, before, quantity).await?,
        );
        sender
            .send(HistoryEvent::History {
                page: Arc::clone(&page),
            })
            .await
            .context("history consumer closed")?;
        if quantity == QuantityMode::Base {
            sender
                .send(HistoryEvent::FlowStatus {
                    message: format!(
                        "Base quantity {} · {}",
                        instrument.base_coin,
                        base_flow_description(instrument)
                    ),
                })
                .await
                .context("history consumer closed")?;
        }
        let (flow_sender, mut flow_receiver) = mpsc::channel(32);
        let enrichment = providers::flow_history(
            &self.state.flow_http,
            instrument,
            market,
            &page.candles,
            &flow_sender,
            quantity,
        );
        tokio::pin!(enrichment);
        let mut completed: Option<Result<()>> = None;
        loop {
            tokio::select! {
                biased;
                event = flow_receiver.recv() => {
                    let Some(mut event) = event else {
                        return completed.take()
                            .unwrap_or_else(|| Err(anyhow::anyhow!("native flow-history relay closed unexpectedly")));
                    };
                    if let HistoryEvent::Flow { bar } = &event {
                        providers::validate_flow(*bar, market.interval)?;
                    }
                    if quantity == QuantityMode::Base
                        && let HistoryEvent::FlowStatus { message } = &mut event
                    {
                        message.push_str(" · base quantity ");
                        message.push_str(&instrument.base_coin);
                    }
                    sender.send(event).await.context("history consumer closed")?;
                }
                result = &mut enrichment, if completed.is_none() => {
                    completed = Some(result);
                    flow_receiver.close();
                }
            }
        }
    }

    pub async fn stream(
        &self,
        market: Market,
        cvd: bool,
        sender: mpsc::Sender<MarketEvent>,
        shutdown: watch::Receiver<bool>,
    ) -> Result<()> {
        self.stream_with_quantity(market, cvd, sender, shutdown, QuantityMode::Native)
            .await
    }

    pub async fn stream_in_base(
        &self,
        market: Market,
        cvd: bool,
        sender: mpsc::Sender<MarketEvent>,
        shutdown: watch::Receiver<bool>,
    ) -> Result<()> {
        self.stream_with_quantity(market, cvd, sender, shutdown, QuantityMode::Base)
            .await
    }

    async fn stream_with_quantity(
        &self,
        market: Market,
        cvd: bool,
        sender: mpsc::Sender<MarketEvent>,
        mut shutdown: watch::Receiver<bool>,
        quantity: QuantityMode,
    ) -> Result<()> {
        if *shutdown.borrow() || sender.is_closed() {
            return Ok(());
        }
        let upstream_shutdown = shutdown.clone();
        let request = async {
            let (catalog, index) = self.validate_market(&market).await?;
            publish(
                &sender,
                MarketEvent::Status {
                    connected: false,
                    message: format!(
                        "Connecting to {} {} · {} · {}",
                        market.exchange, market.product, market.symbol, market.interval
                    ),
                },
            )
            .await?;
            run_upstream(
                &self.state.http,
                &catalog.instruments[index],
                &market,
                cvd,
                sender.clone(),
                upstream_shutdown,
                quantity,
            )
            .await
        };
        tokio::select! {
            _ = sender.closed() => Ok(()),
            _ = shutdown.changed() => Ok(()),
            result = request => result,
        }
    }
}

async fn publish(sender: &mpsc::Sender<MarketEvent>, event: MarketEvent) -> Result<()> {
    timeout(IO_TIMEOUT, sender.send(event))
        .await
        .context("viewer event queue stalled")?
        .context("viewer disconnected")
}

fn remember_unfinalized(snapshots: &mut BTreeMap<i64, VolumeFlow>, bar: FlowBar, width: i64) {
    if bar.flow.quality == FlowQuality::Missing
        || bar.flow.through_ms == bar.time.saturating_add(width).saturating_mul(1000)
    {
        snapshots.remove(&bar.time);
    } else {
        snapshots.insert(bar.time, bar.flow);
        while snapshots.len() > 2 {
            snapshots.pop_first();
        }
    }
}

async fn revoke_unfinalized(
    sender: &mpsc::Sender<MarketEvent>,
    snapshots: &mut BTreeMap<i64, VolumeFlow>,
) -> Result<()> {
    for (time, observed) in std::mem::take(snapshots) {
        publish(
            sender,
            MarketEvent::Flow {
                bar: FlowBar {
                    time,
                    flow: providers::flow::missing(time, observed.through_ms),
                },
            },
        )
        .await?;
    }
    Ok(())
}

/// Deterministic native-quantity multiplier for spot and proven linear markets.
/// Inverse quantities need authoritative base fields or per-execution prices,
/// and INTX positions have no documented base unit.
pub fn base_quantity_factor(instrument: &Instrument) -> Result<f64> {
    ensure!(
        !instrument.inverse,
        "inverse base quantities require authoritative native base fields or execution prices"
    );
    let factor = match (instrument.exchange, instrument.product) {
        (_, Product::Spot) | (Exchange::Binance, Product::Perpetual) => 1.0,
        (Exchange::Mexc | Exchange::Okx, Product::Perpetual) => instrument.contract_size,
        (Exchange::Coinbase, Product::Perpetual) => {
            anyhow::bail!("INTX native positions lack an authoritative base quantity unit")
        }
    };
    ensure!(
        factor.is_finite() && factor > 0.0,
        "invalid native base contract size"
    );
    Ok(factor)
}

fn base_flow_description(instrument: &Instrument) -> &'static str {
    match (instrument.exchange, instrument.product, instrument.inverse) {
        (_, Product::Spot, _) | (Exchange::Binance, Product::Perpetual, false) => {
            "native base volume"
        }
        (Exchange::Binance, Product::Perpetual, true) => {
            "authoritative native base-volume/taker-base fields"
        }
        (Exchange::Okx, Product::Perpetual, true) => {
            "each execution contracts × native face value / actual execution price; unavailable replay remains missing"
        }
        (Exchange::Okx, Product::Perpetual, false) => {
            "native contracts × catalog base contract size"
        }
        (Exchange::Mexc, Product::Perpetual, false) => {
            "native contracts × catalog base contract size; session-only reported side retained, history unavailable"
        }
        (Exchange::Mexc, Product::Perpetual, true) => "inverse conversion unsupported",
        (Exchange::Coinbase, Product::Perpetual, _) => {
            "public signed flow unavailable; authenticated INTX MATCH not configured"
        }
    }
}

/// Disclose normalized flow units on the existing owned provider relay.
fn normalize_event(
    mut event: MarketEvent,
    instrument: &Instrument,
    quantity: QuantityMode,
) -> MarketEvent {
    if quantity == QuantityMode::Native {
        return event;
    }
    if let MarketEvent::FlowStatus { message, .. } = &mut event {
        message.push_str(" · base quantity ");
        message.push_str(&instrument.base_coin);
        message.push_str(" · ");
        message.push_str(base_flow_description(instrument));
    }
    event
}

async fn run_upstream(
    client: &reqwest::Client,
    instrument: &Instrument,
    market: &Market,
    cvd: bool,
    sender: mpsc::Sender<MarketEvent>,
    mut shutdown: watch::Receiver<bool>,
    quantity: QuantityMode,
) -> Result<()> {
    let mut delay = Duration::from_secs(1);
    loop {
        if *shutdown.borrow() || sender.is_closed() {
            return Ok(());
        }
        let started = Instant::now();
        let (provider_sender, mut provider_receiver) = mpsc::channel(256);
        let session = providers::stream(
            client,
            instrument,
            market,
            cvd,
            &provider_sender,
            shutdown.clone(),
            quantity,
        );
        tokio::pin!(session);
        let mut connected = false;
        let mut unfinalized = BTreeMap::new();
        let mut completed: Option<Result<()>> = None;
        let result = loop {
            tokio::select! {
                biased;
                _ = shutdown.changed() => break Ok(()),
                _ = sender.closed() => break Ok(()),
                event = provider_receiver.recv() => {
                    let Some(event) = event else {
                        break completed.take()
                            .unwrap_or_else(|| Err(anyhow::anyhow!("provider stream ended without closing session")));
                    };
                    let event = normalize_event(event, instrument, quantity);
                    let relayed = async {
                        match &event {
                            MarketEvent::Status { connected: true, .. } if !connected => {
                                connected = true;
                                publish(&sender, event).await?;
                                publish(&sender, MarketEvent::Resync).await
                            }
                            MarketEvent::Flow { bar } => {
                                providers::validate_flow(*bar, market.interval)?;
                                if cvd {
                                    remember_unfinalized(&mut unfinalized, *bar, market.interval.seconds());
                                }
                                publish(&sender, event).await
                            }
                            MarketEvent::FlowStatus { connected: false, .. } => {
                                revoke_unfinalized(&sender, &mut unfinalized).await?;
                                publish(&sender, event).await
                            }
                            _ => {
                                if cvd && let MarketEvent::Candle { candle } = &event
                                    && let Some(flow) = candle.flow
                                {
                                    remember_unfinalized(&mut unfinalized,
                                        FlowBar { time: candle.time, flow }, market.interval.seconds());
                                }
                                publish(&sender, event).await
                            }
                        }
                    }.await;
                    if let Err(error) = relayed {
                        break Err(error);
                    }
                }
                result = &mut session, if completed.is_none() => {
                    completed = Some(result);
                    provider_receiver.close();
                }
            }
        };
        if *shutdown.borrow() || sender.is_closed() {
            return Ok(());
        }
        if started.elapsed() >= Duration::from_secs(60) {
            delay = Duration::from_secs(1);
        }
        let error = result
            .err()
            .map_or_else(|| "session ended".into(), |error| format!("{error:#}"));
        let message = format!(
            "{} {} {} disconnected; data stale: {error}. Reconnecting in {}s",
            market.exchange,
            market.product,
            market.symbol,
            delay.as_secs()
        );
        if cvd {
            revoke_unfinalized(&sender, &mut unfinalized).await?;
            publish(
                &sender,
                MarketEvent::FlowStatus {
                    connected: false,
                    message: message.clone(),
                },
            )
            .await?;
        }
        publish(
            &sender,
            MarketEvent::Status {
                connected: false,
                message,
            },
        )
        .await?;
        tokio::select! {
            _ = shutdown.changed() => return Ok(()),
            _ = sender.closed() => return Ok(()),
            _ = tokio::time::sleep(delay) => {}
        }
        delay = (delay * 2).min(Duration::from_secs(30));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn base_interest_validates_instrument_without_requiring_candle_interval() {
        let service = DataService::new().unwrap();
        let market = Market {
            exchange: Exchange::Coinbase,
            product: Product::Perpetual,
            symbol: "BTC-PERP".into(),
            interval: mexc_model::Timeframe::Hour8,
        };
        let instrument = Instrument {
            exchange: market.exchange,
            product: market.product,
            symbol: market.symbol.clone(),
            base_coin: "BTC".into(),
            quote_coin: mexc_model::Quote::USDC,
            settle_coin: "USDC".into(),
            price_scale: 1,
            tick_size: None,
            contract_size: 1.0,
            inverse: false,
            volume_unit: "native positions".into(),
        };
        let catalog = Catalog {
            exchange: market.exchange,
            product: market.product,
            intervals: vec![mexc_model::Timeframe::Min5],
            instruments: vec![instrument],
            flow_support: providers::flow_support(market.exchange, market.product),
        };
        let index = market.exchange.index() * Product::ALL.len() + market.product.index();
        service.state.catalogs[index]
            .lock()
            .await
            .set(CachedCatalog {
                loaded: Instant::now(),
                result: Ok(Arc::new(catalog)),
            })
            .unwrap_or_else(|_| panic!("cache starts empty"));
        let page = service
            .open_interest_history_in_base(&market, None)
            .await
            .unwrap();
        assert_eq!(page.market, market);
        assert!(page.samples.is_empty());
        assert_eq!(page.support.unit, "BTC");
        assert!(page.support.live_interval_ms.is_none());
        assert!(service.history_in_base(&market, None).await.is_err());
        assert!(service.open_interest_history(&market, None).await.is_err());
        let executions = service
            .execution_history(&market, 1000, 2000)
            .await
            .unwrap();
        assert!(executions.executions.is_empty());
        assert!(!executions.coverage.is_empty());
        assert!(executions.coverage.iter().all(|range| !range.complete));
        assert!(
            service
                .execution_history(&market, 2000, 1000)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn normalized_producers_cancel_before_catalog_work() {
        let service = DataService::new().unwrap();
        let (_shutdown, receiver) = watch::channel(true);
        let (sender, mut events) = mpsc::channel(1);
        service
            .stream_in_base(Market::default(), true, sender, receiver)
            .await
            .unwrap();
        assert!(events.recv().await.is_none());
        let (_shutdown, receiver) = watch::channel(true);
        let (sender, mut events) = mpsc::channel(1);
        service
            .open_interest_stream_in_base(Market::default(), sender, receiver)
            .await
            .unwrap();
        assert!(events.recv().await.is_none());
        let (_shutdown, receiver) = watch::channel(true);
        let (sender, mut events) = mpsc::channel(1);
        service
            .execution_stream(Market::default(), sender, receiver)
            .await
            .unwrap();
        assert!(events.recv().await.is_none());
        let (sender, receiver) = mpsc::channel(1);
        drop(receiver);
        service
            .history_with_flow_in_base(Market::default(), None, sender)
            .await
            .unwrap();
        for slot in &service.state.catalogs {
            assert!(slot.lock().await.get().is_none());
        }
    }

    #[test]
    fn unfinished_flow_is_bounded_and_finalized_flow_is_removed() {
        let mut snapshots = BTreeMap::new();
        for time in [300, 600, 900] {
            remember_unfinalized(
                &mut snapshots,
                FlowBar {
                    time,
                    flow: VolumeFlow {
                        delta: Some(1.0),
                        volume: Some(1.0),
                        quality: FlowQuality::Exact,
                        covered_from: time,
                        through_ms: time * 1000 + 1,
                    },
                },
                300,
            );
        }
        assert_eq!(snapshots.keys().copied().collect::<Vec<_>>(), [600, 900]);
        let complete = VolumeFlow {
            through_ms: 1_200_000,
            ..snapshots[&900]
        };
        remember_unfinalized(
            &mut snapshots,
            FlowBar {
                time: 900,
                flow: complete,
            },
            300,
        );
        remember_unfinalized(
            &mut snapshots,
            FlowBar {
                time: 600,
                flow: providers::flow::missing(600, 900_000),
            },
            300,
        );
        assert!(snapshots.is_empty());
    }

    #[tokio::test]
    async fn interruption_revokes_only_unfinished_observations() {
        let (sender, mut receiver) = mpsc::channel(2);
        let mut snapshots = BTreeMap::new();
        remember_unfinalized(
            &mut snapshots,
            FlowBar {
                time: 300,
                flow: VolumeFlow {
                    delta: Some(2.0),
                    volume: Some(3.0),
                    quality: FlowQuality::Exact,
                    covered_from: 300,
                    through_ms: 400_000,
                },
            },
            300,
        );
        revoke_unfinalized(&sender, &mut snapshots).await.unwrap();
        let MarketEvent::Flow { bar } = receiver.recv().await.unwrap() else {
            panic!("expected revocation")
        };
        assert_eq!(bar.time, 300);
        assert_eq!(bar.flow.quality, FlowQuality::Missing);
        assert_eq!(bar.flow.delta, None);
        assert_eq!(bar.flow.volume, None);
        assert_eq!(bar.flow.through_ms, 400_000);
        assert!(snapshots.is_empty());
    }

    #[tokio::test]
    async fn cancelled_stream_never_fetches_catalog() {
        let service = DataService::new().unwrap();
        let (shutdown, receiver) = watch::channel(true);
        let (sender, mut events) = mpsc::channel(1);
        service
            .stream(Market::default(), true, sender, receiver)
            .await
            .unwrap();
        assert!(events.recv().await.is_none());
        drop(shutdown);
        for slot in &service.state.catalogs {
            assert!(slot.lock().await.get().is_none());
        }
    }

    #[tokio::test]
    async fn closed_history_receiver_never_fetches_catalog() {
        let service = DataService::new().unwrap();
        let (sender, receiver) = mpsc::channel(1);
        drop(receiver);
        service
            .history_with_flow(Market::default(), None, sender)
            .await
            .unwrap();
        for slot in &service.state.catalogs {
            assert!(slot.lock().await.get().is_none());
        }
    }

    #[tokio::test]
    async fn catalog_cache_shares_native_rows_and_validation_errors_are_terminal() {
        let service = DataService::new().unwrap();
        let market = Market::default();
        let catalog = Arc::new(Catalog {
            exchange: market.exchange,
            product: market.product,
            intervals: vec![market.interval],
            instruments: Vec::new(),
            flow_support: providers::flow_support(market.exchange, market.product),
        });
        let scope = market.exchange.index() * Product::ALL.len() + market.product.index();
        service.state.catalogs[scope]
            .lock()
            .await
            .set(CachedCatalog {
                loaded: Instant::now(),
                result: Ok(Arc::clone(&catalog)),
            })
            .unwrap_or_else(|_| panic!("cache must start empty"));
        let shared = service
            .clone()
            .catalog(market.exchange, market.product)
            .await
            .unwrap();
        assert!(Arc::ptr_eq(&catalog, &shared));
        let (sender, mut receiver) = mpsc::channel(1);
        let error = service
            .history_with_flow(market, None, sender)
            .await
            .unwrap_err();
        let HistoryEvent::Error { message } = receiver.recv().await.unwrap() else {
            panic!("expected terminal validation error");
        };
        assert_eq!(message, format!("{error:#}"));
        assert!(receiver.recv().await.is_none());
    }

    #[tokio::test]
    async fn explicit_discovery_retry_only_replaces_completed_failed_exact_scope() {
        let service = DataService::new().unwrap();
        let exchange = Exchange::Binance;
        let product = Product::Perpetual;
        let failed_index = exchange.index() * Product::ALL.len() + product.index();
        let failed = Arc::clone(&*service.state.catalogs[failed_index].lock().await);
        failed
            .set(CachedCatalog {
                loaded: Instant::now(),
                result: Err(Arc::new(anyhow::anyhow!("completed discovery failure"))),
            })
            .unwrap_or_else(|_| panic!("cache starts empty"));
        let unrelated_index = Exchange::Okx.index() * Product::ALL.len() + Product::Spot.index();
        let unrelated = Arc::clone(&*service.state.catalogs[unrelated_index].lock().await);
        tokio::join!(
            service.invalidate_failed_catalog(exchange, product),
            service.invalidate_failed_catalog(exchange, product),
        );
        let fresh = Arc::clone(&*service.state.catalogs[failed_index].lock().await);
        assert!(!Arc::ptr_eq(&fresh, &failed));
        assert!(fresh.get().is_none());
        service.invalidate_failed_catalog(exchange, product).await;
        assert!(Arc::ptr_eq(
            &fresh,
            &*service.state.catalogs[failed_index].lock().await
        ));
        assert!(Arc::ptr_eq(
            &unrelated,
            &*service.state.catalogs[unrelated_index].lock().await
        ));
        let catalog = Arc::new(Catalog {
            exchange,
            product,
            intervals: Vec::new(),
            instruments: Vec::new(),
            flow_support: providers::flow_support(exchange, product),
        });
        fresh
            .set(CachedCatalog {
                loaded: Instant::now(),
                result: Ok(Arc::clone(&catalog)),
            })
            .unwrap_or_else(|_| panic!("retry cell starts empty"));
        let result = service.retry_catalog(exchange, product).await.unwrap();
        assert!(Arc::ptr_eq(&catalog, &result));
        assert!(Arc::ptr_eq(
            &fresh,
            &*service.state.catalogs[failed_index].lock().await
        ));
    }

    #[tokio::test]
    async fn explicit_discovery_retry_does_not_split_a_shared_in_flight_scope() {
        let service = DataService::new().unwrap();
        let exchange = Exchange::Okx;
        let product = Product::Perpetual;
        let index = exchange.index() * Product::ALL.len() + product.index();
        let cell = Arc::clone(&*service.state.catalogs[index].lock().await);
        let initializing = Arc::clone(&cell);
        let (started, ready) = tokio::sync::oneshot::channel();
        let (finish, completed) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            initializing
                .get_or_init(|| async {
                    let _ = started.send(());
                    let _ = completed.await;
                    CachedCatalog {
                        loaded: Instant::now(),
                        result: Ok(Arc::new(Catalog {
                            exchange,
                            product,
                            intervals: Vec::new(),
                            instruments: Vec::new(),
                            flow_support: providers::flow_support(exchange, product),
                        })),
                    }
                })
                .await;
        });
        ready.await.unwrap();
        tokio::join!(
            service.invalidate_failed_catalog(exchange, product),
            service.invalidate_failed_catalog(exchange, product),
        );
        assert!(Arc::ptr_eq(
            &cell,
            &*service.state.catalogs[index].lock().await
        ));
        finish.send(()).unwrap();
        task.await.unwrap();
        let expected = cell.get().unwrap().result.as_ref().unwrap();
        assert!(Arc::ptr_eq(
            expected,
            &service.retry_catalog(exchange, product).await.unwrap()
        ));
    }

    #[tokio::test]
    async fn dropping_native_reader_guard_aborts_the_child() {
        let (sender, receiver) = tokio::sync::oneshot::channel::<()>();
        let guard = providers::AbortOnDrop(tokio::spawn(async move {
            let _sender = sender;
            std::future::pending::<()>().await;
        }));
        drop(guard);
        assert!(receiver.await.is_err());
    }
}
