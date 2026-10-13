use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use arrayvec::ArrayVec;
use iced::{Subscription, Task};
use mexc_indicators::{IndicatorKind, IndicatorSource, IndicatorSpec, SourceData, SourceMode};
use mexc_model::{
    Catalog, Exchange, FlowAvailability, FlowBar, HistoryEvent, Instrument, Market, MarketEvent,
    OpenInterestEvent, Product, Timeframe,
};
use tokio::sync::watch;

use crate::{Desktop, Message, source_feeds};

pub struct Request {
    pub generation: u64,
    pub target: watch::Sender<i64>,
}

pub type Requests = HashMap<(Market, source_feeds::Kind), Request>;
pub type Catalogs = HashMap<(Exchange, Product), Result<Arc<Catalog>, String>>;
pub type PendingCatalogs = HashSet<(Exchange, Product)>;

/// The composer and running All aggregates must use the same native eligibility rules.
pub fn eligible_source_instruments<'a>(
    catalog: &'a Catalog,
    kind: IndicatorKind,
    product: Product,
    interval: Timeframe,
    base_asset: &'a str,
) -> impl Iterator<Item = &'a Instrument> {
    let supported = catalog.product == product
        && match kind {
            IndicatorKind::Cvd => {
                catalog.intervals.contains(&interval)
                    && (catalog.flow_support.history != FlowAvailability::Unavailable
                        || catalog.flow_support.live != FlowAvailability::Unavailable)
            }
            IndicatorKind::OpenInterest => product == Product::Perpetual,
            _ => false,
        };
    catalog.instruments.iter().filter(move |item| {
        supported
            && item.exchange == catalog.exchange
            && item.product == product
            && item.base_coin == base_asset
            && (mexc_data::base_quantity_factor(item).is_ok()
                || item.inverse && matches!(item.exchange, Exchange::Binance | Exchange::Okx))
    })
}

pub(crate) fn validated_source_catalog(
    exchange: Exchange,
    product: Product,
    result: Result<Arc<Catalog>, String>,
) -> Result<Arc<Catalog>, String> {
    let catalog = result?;
    if let Some(error) = source_catalog_scope_error(&catalog, exchange, product) {
        return Err(error);
    }
    Ok(catalog)
}

fn source_catalog_scope_error(
    catalog: &Catalog,
    exchange: Exchange,
    product: Product,
) -> Option<String> {
    if catalog.exchange != exchange || catalog.product != product {
        return Some(format!(
            "{exchange} {product} discovery returned a catalog for {} {}",
            catalog.exchange, catalog.product
        ));
    }
    catalog
        .instruments
        .iter()
        .find(|item| item.exchange != exchange || item.product != product)
        .map(|item| {
            format!(
                "{exchange} {product} discovery returned an out-of-scope instrument: {} {} {}",
                item.exchange, item.product, item.symbol
            )
        })
}

fn all_source_error(
    catalogs: &Catalogs,
    pending: &PendingCatalogs,
    source: &IndicatorSource,
    kind: IndicatorKind,
    chart: &Market,
    base_asset: &str,
) -> Option<String> {
    if base_asset.is_empty() {
        return Some("All sources await catalog-confirmed chart underlying metadata".into());
    }
    // The checklist exposes every venue; the legend needs one actionable blocker.
    // Coverage is accepted only after all four scopes and every explicit choice pass.
    for exchange in Exchange::ALL {
        let scope = (exchange, chart.product);
        let catalog = match catalogs.get(&scope) {
            _ if pending.contains(&scope) => {
                return Some(format!(
                    "Awaiting {exchange} {} native catalog discovery (chart {base_asset} / {})",
                    chart.product, chart.interval
                ));
            }
            Some(Ok(catalog)) => catalog,
            Some(Err(_)) => {
                return Some(format!(
                    "{exchange} {} native discovery failed. Repair source to retry discovery.",
                    chart.product
                ));
            }
            None => {
                return Some(format!(
                    "Awaiting {exchange} {} native catalog discovery (chart {base_asset} / {})",
                    chart.product, chart.interval
                ));
            }
        };
        if let Some(error) = source_catalog_scope_error(catalog, exchange, chart.product) {
            return Some(error);
        }
        let eligible = || {
            eligible_source_instruments(catalog, kind, chart.product, chart.interval, base_asset)
        };
        for selected in source
            .markets
            .iter()
            .filter(|market| market.exchange == exchange)
        {
            if selected.product != chart.product
                || !eligible().any(|item| item.symbol == selected.symbol)
            {
                return Some(format!(
                    "Selected {exchange} {} {} is not eligible for {kind} on chart {base_asset} {} / {}",
                    selected.product, selected.symbol, chart.product, chart.interval
                ));
            }
        }
        if eligible().next().is_some()
            && !source
                .markets
                .iter()
                .any(|selected| selected.exchange == exchange)
        {
            return Some(format!(
                "Choose an eligible native {exchange} {base_asset} {} market for All sources (chart {})",
                chart.product, chart.interval
            ));
        }
    }
    None
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}

impl Desktop {
    pub(crate) fn ensure_source_catalogs(&mut self) -> Task<Message> {
        let all_sources_needed = self
            .chart
            .indicators
            .instances
            .iter()
            .any(|instance| instance.source.mode == SourceMode::All);
        if !all_sources_needed && (!self.manager.open || !self.manager.source_catalogs_needed()) {
            return Task::none();
        }
        let product = self.chart.market.product;
        let mut tasks = Vec::new();
        for exchange in Exchange::ALL {
            let scope = (exchange, product);
            if let Some(result) = self.source_catalogs.get(&scope) {
                self.manager
                    .accept_source_catalog(exchange, product, result.clone());
                continue;
            }
            if !self.source_catalog_pending.insert(scope) {
                continue;
            }
            let data = self.data.clone();
            let generation = self.source_catalog_generation;
            tasks.push(Task::perform(
                async move {
                    data.catalog(exchange, product)
                        .await
                        .map_err(|error| format!("{error:#}"))
                },
                move |result| Message::SourceCatalog {
                    exchange,
                    product,
                    generation,
                    result,
                },
            ));
        }
        Task::batch(tasks)
    }

    pub(crate) fn reset_source_discovery(&mut self) {
        self.source_catalog_generation = self.source_catalog_generation.wrapping_add(1);
        self.source_catalog_pending.clear();
        self.manager
            .reset_source_catalogs(self.chart.market.product);
        for exchange in Exchange::ALL {
            if let Some(result) = self
                .source_catalogs
                .get(&(exchange, self.chart.market.product))
            {
                self.manager.accept_source_catalog(
                    exchange,
                    self.chart.market.product,
                    result.clone(),
                );
            }
        }
    }

    pub(crate) fn retry_source_discovery(&mut self) {
        let product = self.chart.market.product;
        let failed = self
            .source_catalogs
            .iter()
            .any(|((_, scope_product), result)| *scope_product == product && result.is_err());
        if !failed {
            return;
        }
        self.source_catalogs
            .retain(|(_, scope_product), result| *scope_product != product || result.is_ok());
        self.source_catalog_generation = self.source_catalog_generation.wrapping_add(1);
        self.source_catalog_pending.clear();
    }

    pub(crate) fn revalidate_all_sources(&mut self) {
        for index in 0..self.chart.indicators.instances.len() {
            let instance = &self.chart.indicators.instances[index];
            if instance.source.mode != SourceMode::All {
                continue;
            }
            let id = instance.id;
            let error = all_source_error(
                &self.source_catalogs,
                &self.source_catalog_pending,
                &instance.source,
                instance.spec.kind(),
                &self.chart.market,
                &self.chart.base_asset,
            );
            // The instance ID comes from the same set and membership is never changed here.
            let _ = self.chart.indicators.set_all_source_error(id, error);
        }
    }

    pub(crate) fn sync_indicator_sources(&mut self) {
        if self
            .source_inputs
            .keys()
            .any(|market| market.interval != self.chart.market.interval)
        {
            self.source_inputs = self
                .source_inputs
                .drain()
                .map(|(mut market, source)| {
                    market.interval = self.chart.market.interval;
                    let mut rebound = SourceData::new(market.clone(), source.base_asset);
                    rebound.oi = source.oi;
                    rebound.oi_cadence_ms = source.oi_cadence_ms;
                    rebound.oi_status = source.oi_status;
                    (market, rebound)
                })
                .collect();
        }
        let mut desired = HashSet::new();
        for instance in &self.chart.indicators.instances {
            if instance.source.is_chart() {
                continue;
            }
            let kind = match instance.spec {
                IndicatorSpec::Cvd { .. } => source_feeds::Kind::Cvd,
                IndicatorSpec::OpenInterest => source_feeds::Kind::OpenInterest,
                _ => continue,
            };
            for market in &instance.source.markets {
                if market.product != self.chart.market.product {
                    continue;
                }
                let mut market = market.clone();
                market.interval = self.chart.market.interval;
                if self.source_inputs.get(&market).is_some_and(|data| {
                    !data.base_asset.is_empty() && data.base_asset != self.chart.base_asset
                }) {
                    continue;
                }
                desired.insert((market, kind));
            }
        }
        let now = now_ms();
        self.source_requests.retain(|(market, kind), _| {
            let keep = desired.contains(&(market.clone(), *kind));
            if !keep && let Some(source) = self.source_inputs.get_mut(market) {
                match kind {
                    source_feeds::Kind::Cvd => {
                        source.flow_connected = false;
                        source.revoke_unfinished(now);
                    }
                    source_feeds::Kind::OpenInterest => source.oi_connected = false,
                }
                source.revision = source.revision.wrapping_add(1);
            }
            keep
        });
        self.source_inputs
            .retain(|market, _| desired.iter().any(|(wanted, _)| wanted == market));
        let target = self.chart.oldest().unwrap_or(i64::MAX);
        for key in desired {
            if self.source_requests.contains_key(&key) {
                continue;
            }
            self.next_source_generation = self.next_source_generation.wrapping_add(1);
            let (target, _) = watch::channel(target);
            self.source_inputs
                .entry(key.0.clone())
                .or_insert_with(|| SourceData::new(key.0.clone(), String::new()));
            self.source_requests.insert(
                key,
                Request {
                    generation: self.next_source_generation,
                    target,
                },
            );
        }
        self.revalidate_all_sources();
        self.refresh_source_panes();
    }

    pub(crate) fn update_source_history_target(&self) {
        let Some(target) = self.chart.oldest() else {
            return;
        };
        for request in self.source_requests.values() {
            request.target.send_if_modified(|previous| {
                if *previous == target {
                    false
                } else {
                    *previous = target;
                    true
                }
            });
        }
    }

    pub(crate) fn refresh_source_panes(&mut self) {
        for id in self
            .chart
            .refresh_indicator_sources(&self.source_inputs, now_ms())
        {
            if let Some(cache) = self.pane_caches.get(&id) {
                cache.clear();
            }
        }
    }

    pub(crate) fn source_subscription(&self) -> Subscription<Message> {
        Subscription::batch(
            self.source_requests
                .iter()
                .map(|((market, kind), request)| {
                    Subscription::run_with_id(
                        ("base_source", market.clone(), *kind, request.generation),
                        source_feeds::source(
                            self.data.clone(),
                            source_feeds::Scope {
                                market: market.clone(),
                                generation: request.generation,
                                kind: *kind,
                                base_asset: self.chart.base_asset.clone(),
                            },
                            request.target.subscribe(),
                        ),
                    )
                    .map(Message::SourceData)
                }),
        )
    }

    pub(crate) fn accept_source_batch(&mut self, batch: source_feeds::Batch) {
        if !self
            .source_requests
            .get(&(batch.scope.market.clone(), batch.scope.kind))
            .is_some_and(|request| request.generation == batch.scope.generation)
        {
            return;
        }
        let Some(source) = self.source_inputs.get_mut(&batch.scope.market) else {
            return;
        };
        let now = now_ms();
        for event in batch.events.into_vec() {
            match event {
                source_feeds::Event::Metadata { base_asset } => {
                    if source.base_asset != base_asset {
                        source.base_asset = base_asset;
                        source.revision = source.revision.wrapping_add(1);
                    }
                }
                source_feeds::Event::HistoryProblem { message } => {
                    if source.oi_status != message {
                        source.oi_status = message;
                        source.revision = source.revision.wrapping_add(1);
                    }
                }
                source_feeds::Event::Market(MarketEvent::Candle { candle }) => {
                    if let Some(flow) = candle.flow {
                        source.merge_flows(
                            &[FlowBar {
                                time: candle.time,
                                flow,
                            }],
                            true,
                        );
                    }
                }
                source_feeds::Event::Market(MarketEvent::Flow { bar }) => {
                    source.merge_flows(&[bar], true);
                }
                source_feeds::Event::Market(MarketEvent::FlowStatus { connected, message }) => {
                    flow_status(source, connected, message, now);
                }
                source_feeds::Event::Market(MarketEvent::Status { connected, message }) => {
                    if source.market.exchange == Exchange::Binance || !connected {
                        flow_status(source, connected, message, now);
                    }
                }
                source_feeds::Event::Market(MarketEvent::Resync) => {
                    source.revoke_unfinished(now);
                    let message = "Repairing native source coverage after resync";
                    if source.flow_status != message {
                        source.flow_status = message.into();
                        source.revision = source.revision.wrapping_add(1);
                    }
                }
                source_feeds::Event::History(HistoryEvent::History { page }) => {
                    if page.market != source.market {
                        flow_status(
                            source,
                            false,
                            "Native source returned history for another market".into(),
                            now,
                        );
                        continue;
                    }
                    let mut flows = ArrayVec::<FlowBar, 64>::new();
                    for candle in &page.candles {
                        if let Some(flow) = candle.flow {
                            flows.push(FlowBar {
                                time: candle.time,
                                flow,
                            });
                            if flows.is_full() {
                                source.merge_flows(&flows, false);
                                flows.clear();
                            }
                        }
                    }
                    source.merge_flows(&flows, false);
                }
                source_feeds::Event::History(HistoryEvent::Flow { bar }) => {
                    source.merge_flows(&[bar], false);
                }
                source_feeds::Event::History(
                    HistoryEvent::FlowStatus { message } | HistoryEvent::Error { message },
                ) => {
                    if source.flow_status != message {
                        source.flow_status = message;
                        source.revision = source.revision.wrapping_add(1);
                    }
                }
                source_feeds::Event::History(HistoryEvent::Finished) => {}
                source_feeds::Event::OpenInterest(OpenInterestEvent::Support(support)) => {
                    if source.oi_cadence_ms != support.live_interval_ms
                        || source.oi_status != support.description
                    {
                        source.oi_cadence_ms = support.live_interval_ms;
                        source.oi_status = support.description;
                        source.revision = source.revision.wrapping_add(1);
                    }
                }
                source_feeds::Event::OpenInterest(OpenInterestEvent::History(page)) => {
                    if page.market == source.market {
                        source.merge_oi(&page.samples);
                    } else {
                        oi_status(
                            source,
                            false,
                            "Native OI returned history for another market".into(),
                        );
                    }
                }
                source_feeds::Event::OpenInterest(OpenInterestEvent::Sample(sample)) => {
                    source.merge_oi(&[sample]);
                }
                source_feeds::Event::OpenInterest(OpenInterestEvent::Status {
                    connected,
                    message,
                }) => {
                    oi_status(source, connected, message);
                }
            }
        }
        self.refresh_source_panes();
    }
}

fn flow_status(source: &mut SourceData, connected: bool, message: String, now: i64) {
    if !connected {
        source.revoke_unfinished(now);
    }
    if source.flow_connected != connected || source.flow_status != message {
        source.flow_connected = connected;
        source.flow_status = message;
        source.revision = source.revision.wrapping_add(1);
    }
}

fn oi_status(source: &mut SourceData, connected: bool, message: String) {
    if source.oi_connected != connected || source.oi_status != message {
        source.oi_connected = connected;
        source.oi_status = message;
        source.revision = source.revision.wrapping_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mexc_indicators::{IndicatorSet, SourceMode};
    use mexc_model::{FlowSupport, OpenInterestSample, OpenInterestTimestamp, Quote};

    fn market(exchange: Exchange) -> Market {
        Market {
            exchange,
            product: Product::Perpetual,
            symbol: format!("BTC-native-{}", exchange.index()),
            interval: Timeframe::Min5,
        }
    }

    fn catalog(exchange: Exchange) -> Arc<Catalog> {
        Arc::new(Catalog {
            exchange,
            product: Product::Perpetual,
            intervals: if exchange == Exchange::Okx {
                vec![Timeframe::Min5]
            } else {
                vec![Timeframe::Min5, Timeframe::Hour8]
            },
            instruments: vec![Instrument {
                exchange,
                product: Product::Perpetual,
                symbol: market(exchange).symbol,
                base_coin: "BTC".into(),
                quote_coin: Quote::USDT,
                settle_coin: "USDT".into(),
                price_scale: 2,
                contract_size: 1.0,
                inverse: false,
                volume_unit: "BTC".into(),
            }],
            flow_support: FlowSupport {
                history: FlowAvailability::Exact,
                live: FlowAvailability::Exact,
                description: String::new(),
            },
        })
    }

    fn catalogs() -> Catalogs {
        Exchange::ALL
            .into_iter()
            .map(|exchange| ((exchange, Product::Perpetual), Ok(catalog(exchange))))
            .collect()
    }

    fn all_source(exchanges: &[Exchange]) -> IndicatorSource {
        IndicatorSource {
            mode: SourceMode::All,
            markets: exchanges.iter().copied().map(market).collect(),
        }
    }

    #[test]
    fn discovery_boundaries_never_publish_a_partial_all_total_and_recover_raw_observations() {
        let chart = market(Exchange::Binance);
        let selection = all_source(&[Exchange::Binance, Exchange::Okx, Exchange::Mexc]);
        let mut inputs = HashMap::new();
        for (selected, quantity) in selection.markets.iter().zip([2.0, 3.0, 5.0]) {
            let mut data = SourceData::new(selected.clone(), "BTC".into());
            data.oi_connected = true;
            data.merge_oi(&[OpenInterestSample {
                timestamp_ms: 1_000,
                quantity,
                interval_ms: 1_000,
                timestamp_source: OpenInterestTimestamp::Venue,
            }]);
            inputs.insert(selected.clone(), data);
        }
        let mut set = IndicatorSet::default();
        let id = set
            .add(IndicatorSpec::OpenInterest, &[], chart.interval)
            .unwrap();
        set.configure_source(id, selection.clone()).unwrap();
        let mut discovered = catalogs();
        let mut pending = PendingCatalogs::new();
        let scope = (Exchange::Coinbase, Product::Perpetual);
        pending.insert(scope);
        // Even an ineligible venue must complete discovery, not disappear from All.
        for unavailable in [
            None,
            Some(Err("Discovery failed".into())),
            Some(Ok(Arc::new(Catalog {
                exchange: Exchange::Binance,
                ..catalog(Exchange::Coinbase).as_ref().clone()
            }))),
        ] {
            discovered.remove(&scope);
            if let Some(result) = unavailable {
                discovered.insert(scope, result);
                pending.clear();
            }
            let error = all_source_error(
                &discovered,
                &pending,
                &selection,
                IndicatorKind::OpenInterest,
                &chart,
                "BTC",
            );
            set.set_all_source_error(id, error).unwrap();
            set.refresh_sources(&inputs, &chart, "BTC", 1_000);
            let output = set.instances[0].external.as_ref().unwrap();
            assert!(output.oi.is_empty());
            assert!(output.oi_asof_ranges_ms.is_empty());
            assert!(!output.complete);
            assert_eq!(output.asof_range_ms, None);
            assert_eq!(output.contributors.len(), 3);
            assert_eq!(set.instances[0].source, selection);
            assert!(inputs.values().all(|data| data.oi.len() == 1));
        }
        discovered.insert(scope, Ok(catalog(Exchange::Coinbase)));
        let error = all_source_error(
            &discovered,
            &pending,
            &selection,
            IndicatorKind::OpenInterest,
            &chart,
            "BTC",
        );
        set.set_all_source_error(id, error).unwrap();
        set.refresh_sources(&inputs, &chart, "BTC", 1_000);
        let output = set.instances[0].external.as_ref().unwrap();
        assert!(output.complete);
        assert_eq!(output.oi.len(), 1);
        assert_eq!(output.oi[0].quantity, 10.0);
        assert_eq!(set.instances[0].source, selection);

        // A later discovery revocation also removes historical inspection totals.
        discovered.insert(scope, Err("Rediscovery failed".into()));
        set.set_all_source_error(
            id,
            all_source_error(
                &discovered,
                &pending,
                &selection,
                IndicatorKind::OpenInterest,
                &chart,
                "BTC",
            ),
        )
        .unwrap();
        assert!(set.instances[0].external.as_ref().unwrap().oi.is_empty());
        discovered.insert(scope, Ok(catalog(Exchange::Coinbase)));
        set.set_all_source_error(
            id,
            all_source_error(
                &discovered,
                &pending,
                &selection,
                IndicatorKind::OpenInterest,
                &chart,
                "BTC",
            ),
        )
        .unwrap();
        set.refresh_sources(&inputs, &chart, "BTC", 1_000);
        assert_eq!(
            set.instances[0].external.as_ref().unwrap().oi[0].quantity,
            10.0
        );
        assert!(set.instances[0].external.as_ref().unwrap().complete);
    }

    #[test]
    fn native_context_requires_explicit_repair_but_oi_eligibility_ignores_candle_interval() {
        let discovered = catalogs();
        let pending = PendingCatalogs::new();
        let mut chart = market(Exchange::Binance);
        chart.interval = Timeframe::Hour8;
        let selection = all_source(&[Exchange::Binance, Exchange::Mexc]);
        assert!(
            all_source_error(
                &discovered,
                &pending,
                &selection,
                IndicatorKind::Cvd,
                &chart,
                "BTC",
            )
            .is_none()
        );
        chart.interval = Timeframe::Min5;
        assert!(
            all_source_error(
                &discovered,
                &pending,
                &selection,
                IndicatorKind::Cvd,
                &chart,
                "BTC",
            )
            .is_some()
        );
        assert_eq!(
            selection.markets,
            vec![market(Exchange::Binance), market(Exchange::Mexc)]
        );
        let repaired = all_source(&[Exchange::Binance, Exchange::Okx, Exchange::Mexc]);
        assert!(
            all_source_error(
                &discovered,
                &pending,
                &repaired,
                IndicatorKind::Cvd,
                &chart,
                "BTC",
            )
            .is_none()
        );
        assert!(
            all_source_error(
                &discovered,
                &pending,
                &repaired,
                IndicatorKind::OpenInterest,
                &chart,
                "BTC",
            )
            .is_none()
        );
        chart.interval = Timeframe::Hour8;
        assert!(
            all_source_error(
                &discovered,
                &pending,
                &repaired,
                IndicatorKind::OpenInterest,
                &chart,
                "BTC",
            )
            .is_none()
        );
        assert!(
            all_source_error(
                &discovered,
                &pending,
                &repaired,
                IndicatorKind::OpenInterest,
                &chart,
                "ETH",
            )
            .is_some()
        );
        let mut unknown = repaired.clone();
        unknown.markets[1].symbol = "BTC-guessed-symbol".into();
        assert!(
            all_source_error(
                &discovered,
                &pending,
                &unknown,
                IndicatorKind::OpenInterest,
                &chart,
                "BTC",
            )
            .is_some()
        );
        let mut incompatible = repaired.clone();
        incompatible.markets.push(market(Exchange::Coinbase));
        assert!(
            all_source_error(
                &discovered,
                &pending,
                &incompatible,
                IndicatorKind::OpenInterest,
                &chart,
                "BTC",
            )
            .is_some()
        );
        chart.product = Product::Spot;
        assert!(
            all_source_error(
                &discovered,
                &pending,
                &repaired,
                IndicatorKind::OpenInterest,
                &chart,
                "BTC",
            )
            .is_some()
        );
    }

    #[test]
    fn catalog_validation_rejects_foreign_scope_without_copying_valid_metadata() {
        let valid = catalog(Exchange::Binance);
        let accepted = validated_source_catalog(
            Exchange::Binance,
            Product::Perpetual,
            Ok(Arc::clone(&valid)),
        )
        .unwrap();
        assert!(Arc::ptr_eq(&valid, &accepted));
        assert!(
            validated_source_catalog(Exchange::Okx, Product::Perpetual, Ok(Arc::clone(&valid)),)
                .is_err()
        );
        assert!(
            validated_source_catalog(Exchange::Binance, Product::Spot, Ok(Arc::clone(&valid)),)
                .is_err()
        );
        for (exchange, product) in [
            (Exchange::Okx, Product::Perpetual),
            (Exchange::Binance, Product::Spot),
        ] {
            let mut malformed = valid.as_ref().clone();
            malformed.instruments[0].exchange = exchange;
            malformed.instruments[0].product = product;
            assert!(
                validated_source_catalog(
                    Exchange::Binance,
                    Product::Perpetual,
                    Ok(Arc::new(malformed)),
                )
                .is_err()
            );
        }
        assert!(
            validated_source_catalog(
                Exchange::Binance,
                Product::Perpetual,
                Err("Discovery failed".into()),
            )
            .is_err()
        );
    }

    #[test]
    fn retry_only_revokes_pending_generations_when_failed_discovery_is_removed() {
        let (mut desktop, _) = Desktop::new();
        let product = desktop.chart.market.product;
        let scope = (Exchange::Binance, product);
        let valid = catalog(Exchange::Binance);
        desktop
            .source_catalogs
            .insert(scope, Ok(Arc::clone(&valid)));
        desktop
            .source_catalog_pending
            .insert((Exchange::Okx, product));
        let generation = desktop.source_catalog_generation;
        desktop.retry_source_discovery();
        assert_eq!(desktop.source_catalog_generation, generation);
        assert!(
            desktop
                .source_catalog_pending
                .contains(&(Exchange::Okx, product))
        );
        desktop
            .source_catalogs
            .insert((Exchange::Okx, product), Err("Discovery failed".into()));
        desktop.retry_source_discovery();
        assert_eq!(
            desktop.source_catalog_generation,
            generation.wrapping_add(1)
        );
        assert!(desktop.source_catalog_pending.is_empty());
        assert!(
            !desktop
                .source_catalogs
                .contains_key(&(Exchange::Okx, product))
        );
        let retained = desktop.source_catalogs[&scope].as_ref().unwrap();
        assert!(Arc::ptr_eq(retained, &valid));
    }

    #[test]
    fn oi_interval_rebinding_preserves_every_native_observation_without_false_missing_venues() {
        let (mut desktop, _) = Desktop::new();
        desktop.chart.market = market(Exchange::Binance);
        desktop.chart.base_asset = "BTC".into();
        desktop.source_catalogs = catalogs();
        desktop.chart.indicators = IndicatorSet::default();
        let selection = all_source(&[Exchange::Binance, Exchange::Okx, Exchange::Mexc]);
        let id = desktop
            .chart
            .indicators
            .add(IndicatorSpec::OpenInterest, &[], Timeframe::Min5)
            .unwrap();
        desktop
            .chart
            .indicators
            .configure_source(id, selection.clone())
            .unwrap();
        for (selected, quantity) in selection.markets.iter().zip([2.0, 3.0, 5.0]) {
            let mut data = SourceData::new(selected.clone(), "BTC".into());
            data.oi_connected = true;
            data.oi_cadence_ms = Some(1_000);
            data.merge_oi(&[OpenInterestSample {
                timestamp_ms: 1_000,
                quantity,
                interval_ms: 1_000,
                timestamp_source: OpenInterestTimestamp::Venue,
            }]);
            desktop.source_inputs.insert(selected.clone(), data);
        }
        desktop.sync_indicator_sources();
        assert_eq!(
            desktop.chart.indicators.instances[0]
                .external
                .as_ref()
                .unwrap()
                .oi[0]
                .quantity,
            10.0
        );
        desktop.chart.market.interval = Timeframe::Hour8;
        desktop.sync_indicator_sources();
        let output = desktop.chart.indicators.instances[0]
            .external
            .as_ref()
            .unwrap();
        assert_eq!(output.oi.len(), 1);
        assert_eq!(output.oi[0].quantity, 10.0);
        assert_eq!(desktop.chart.indicators.instances[0].source, selection);
        assert_eq!(desktop.source_inputs.len(), 3);
        for (mut selected, quantity) in selection.markets.into_iter().zip([2.0, 3.0, 5.0]) {
            selected.interval = Timeframe::Hour8;
            let retained = &desktop.source_inputs[&selected];
            assert_eq!(retained.oi.len(), 1);
            assert_eq!(retained.oi[0].timestamp_ms, 1_000);
            assert_eq!(retained.oi[0].quantity, quantity);
            assert_eq!(retained.oi_cadence_ms, Some(1_000));
        }
    }
}
