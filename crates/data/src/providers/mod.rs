mod binance;
mod coinbase;
pub(crate) mod flow;
mod mexc;
mod okx;

use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail, ensure};
use mexc_model::{
    Candle, Catalog, Exchange, FlowAvailability, FlowBar, FlowQuality, FlowSupport, HistoryEvent,
    HistoryPage, Instrument, Market, MarketEvent, PAGE_SIZE, Product, QuantityMode, Timeframe,
};
use reqwest::Client;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::sync::{mpsc, watch};

/// A native socket reader must not outlive a cancelled service request.
pub(super) struct AbortOnDrop(pub(super) tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub fn flow_support(exchange: Exchange, product: Product) -> FlowSupport {
    let (history, live, description) = match (exchange, product) {
        (Exchange::Binance, _) => (
            FlowAvailability::Exact,
            FlowAvailability::Exact,
            "Native kline taker-buy minus taker-sell volume; native base/contract units.",
        ),
        (Exchange::Okx, _) => (
            FlowAvailability::LimitedExact,
            FlowAvailability::Exact,
            "Native taker trades and available tick archives; unavailable archive/range gaps remain missing.",
        ),
        (Exchange::Coinbase, Product::Spot) => (
            FlowAvailability::LimitedExact,
            FlowAvailability::Exact,
            "Native trades (side is maker; inverted for aggressor flow); deep replay can take time.",
        ),
        (Exchange::Coinbase, Product::Perpetual) => (
            FlowAvailability::Unavailable,
            FlowAvailability::Unavailable,
            "No public signed-trade history; International MATCH requires credentials not configured here.",
        ),
        (Exchange::Mexc, Product::Spot) => (
            FlowAvailability::LimitedExact,
            FlowAvailability::Reported,
            "Recent maker-flag REST trades where completeness is proven; live reported trade side is unconfirmed as aggressor.",
        ),
        (Exchange::Mexc, Product::Perpetual) => (
            FlowAvailability::Unavailable,
            FlowAvailability::Reported,
            "Session-only native reported buy/sell in contracts, not confirmed aggressor side; no public historical replay.",
        ),
    };
    FlowSupport {
        history,
        live,
        description: description.into(),
    }
}

pub async fn catalog(client: &Client, exchange: Exchange, product: Product) -> Result<Catalog> {
    let mut catalog = match exchange {
        Exchange::Binance => binance::catalog(client, product).await?,
        Exchange::Okx => okx::catalog(client, product).await?,
        Exchange::Coinbase => coinbase::catalog(client, product).await?,
        Exchange::Mexc => mexc::catalog(client, product).await?,
    };
    ensure!(
        catalog.exchange == exchange && catalog.product == product,
        "catalog scope mismatch"
    );
    ensure!(
        !catalog.intervals.is_empty(),
        "{exchange} {product} has no supported native intervals"
    );
    catalog
        .intervals
        .sort_unstable_by_key(|interval| interval.seconds());
    catalog.intervals.dedup();
    catalog
        .instruments
        .sort_unstable_by(|a, b| a.symbol.cmp(&b.symbol));
    for instrument in &catalog.instruments {
        ensure!(
            instrument.exchange == exchange && instrument.product == product,
            "instrument scope mismatch"
        );
        ensure!(
            !instrument.symbol.is_empty()
                && !instrument.base_coin.is_empty()
                && !instrument.volume_unit.is_empty()
                && (product == Product::Spot || !instrument.settle_coin.is_empty()),
            "incomplete instrument metadata"
        );
        ensure!(
            instrument.contract_size.is_finite() && instrument.contract_size > 0.0,
            "invalid native contract size"
        );
    }
    ensure!(
        catalog
            .instruments
            .windows(2)
            .all(|pair| pair[0].symbol != pair[1].symbol),
        "duplicate native instrument identity"
    );
    Ok(catalog)
}

pub async fn history(
    client: &Client,
    instrument: &Instrument,
    market: &Market,
    before: Option<i64>,
    quantity: QuantityMode,
) -> Result<HistoryPage> {
    match market.exchange {
        Exchange::Binance => binance::history(client, instrument, market, before, quantity).await,
        Exchange::Okx => okx::history(client, instrument, market, before, quantity).await,
        Exchange::Coinbase => coinbase::history(client, instrument, market, before).await,
        Exchange::Mexc => mexc::history(client, instrument, market, before).await,
    }
}
pub async fn flow_history(
    client: &Client,
    instrument: &Instrument,
    market: &Market,
    candles: &[Candle],
    sender: &mpsc::Sender<HistoryEvent>,
    quantity: QuantityMode,
) -> Result<()> {
    match market.exchange {
        Exchange::Binance => Ok(()),
        Exchange::Okx => {
            okx::flow_history(client, instrument, market, candles, sender, quantity).await
        }
        Exchange::Coinbase => {
            coinbase::flow_history(client, instrument, market, candles, sender).await
        }
        Exchange::Mexc => mexc::flow_history(client, instrument, market, candles, sender).await,
    }
}

pub async fn stream(
    client: &Client,
    instrument: &Instrument,
    market: &Market,
    cvd: bool,
    sender: &mpsc::Sender<MarketEvent>,
    shutdown: watch::Receiver<bool>,
    quantity: QuantityMode,
) -> Result<()> {
    match market.exchange {
        Exchange::Binance => {
            binance::stream(client, instrument, market, cvd, sender, shutdown, quantity).await
        }
        Exchange::Okx => {
            okx::stream(client, instrument, market, cvd, sender, shutdown, quantity).await
        }
        Exchange::Coinbase => {
            coinbase::stream(client, instrument, market, cvd, sender, shutdown).await
        }
        Exchange::Mexc => {
            mexc::stream(client, instrument, market, cvd, sender, shutdown, quantity).await
        }
    }
}

pub async fn get_json<T: DeserializeOwned>(
    client: &Client,
    url: &str,
    query: &[(&str, String)],
) -> Result<T> {
    const MAX_BODY: usize = 16 * 1024 * 1024;
    let mut response = client
        .get(url)
        .query(query)
        .send()
        .await
        .with_context(|| format!("GET {url}"))?;
    let status = response.status();
    if response
        .content_length()
        .is_some_and(|length| length > MAX_BODY as u64)
    {
        bail!("GET {url}: response exceeds {MAX_BODY} bytes");
    }
    let mut bytes = Vec::with_capacity(response.content_length().unwrap_or(0) as usize);
    while let Some(chunk) = response
        .chunk()
        .await
        .with_context(|| format!("reading {url}"))?
    {
        ensure!(
            bytes.len() + chunk.len() <= MAX_BODY,
            "GET {url}: response exceeds {MAX_BODY} bytes"
        );
        bytes.extend_from_slice(&chunk);
    }
    if !status.is_success() {
        let body = String::from_utf8_lossy(&bytes);
        let excerpt: String = body.chars().take(2048).collect();
        bail!("GET {url}: HTTP {status}: {excerpt}");
    }
    serde_json::from_slice(&bytes).with_context(|| format!("invalid JSON from {url}"))
}

pub fn number(value: &Value) -> Result<f64> {
    let number = match value {
        Value::Number(number) => number.as_f64().context("unrepresentable number")?,
        Value::String(text) => text.parse::<f64>().context("invalid decimal string")?,
        _ => bail!("expected a number or decimal string"),
    };
    ensure!(number.is_finite(), "non-finite native number");
    Ok(number)
}

pub fn integer(value: &Value) -> Result<i64> {
    match value {
        Value::Number(number) => number.as_i64().context("expected a signed integer"),
        Value::String(text) => text.parse::<i64>().context("invalid integer string"),
        _ => bail!("expected an integer or integer string"),
    }
}

pub fn unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock predates Unix epoch")
        .as_secs() as i64
}

pub fn history_end(before: Option<i64>, now: i64, interval: Timeframe) -> Result<i64> {
    match before {
        Some(before) => {
            ensure!(
                before > 0 && before <= (now / interval.seconds() + 1) * interval.seconds(),
                "before must be a positive Unix timestamp in seconds, no later than the next {} boundary",
                interval.label()
            );
            Ok(before - 1)
        }
        None => Ok(now),
    }
}

pub fn validate(candle: Candle, interval: Timeframe) -> Result<()> {
    ensure!(
        candle.time > 0 && candle.time % interval.seconds() == 0,
        "invalid {} native candle timestamp",
        interval.label()
    );
    ensure!(
        [
            candle.open,
            candle.high,
            candle.low,
            candle.close,
            candle.volume
        ]
        .into_iter()
        .all(f64::is_finite),
        "non-finite candle values"
    );
    ensure!(
        candle.low > 0.0
            && candle.high >= candle.low
            && candle.open >= candle.low
            && candle.open <= candle.high
            && candle.close >= candle.low
            && candle.close <= candle.high
            && candle.volume >= 0.0,
        "invalid native candle OHLC or volume"
    );
    if let Some(flow) = candle.flow {
        validate_flow(
            FlowBar {
                time: candle.time,
                flow,
            },
            interval,
        )?;
    }
    Ok(())
}

pub fn validate_flow(bar: FlowBar, interval: Timeframe) -> Result<()> {
    ensure!(
        bar.time > 0 && bar.time % interval.seconds() == 0,
        "invalid native flow bucket"
    );
    let start_ms = bar.time.saturating_mul(1000);
    let end_ms = bar
        .time
        .saturating_add(interval.seconds())
        .saturating_mul(1000);
    ensure!(
        bar.flow.covered_from >= 0
            && bar.flow.covered_from <= bar.time
            && bar.flow.through_ms >= start_ms
            && bar.flow.through_ms <= end_ms,
        "invalid native flow coverage"
    );
    match (bar.flow.quality, bar.flow.delta, bar.flow.volume) {
        (FlowQuality::Missing, None, None) => {}
        (FlowQuality::Exact | FlowQuality::Reported, Some(delta), Some(volume)) => {
            ensure!(
                delta.is_finite() && volume.is_finite() && volume >= 0.0 && delta.abs() <= volume,
                "invalid signed native flow"
            );
        }
        _ => bail!("native flow quality and numeric values disagree"),
    }
    Ok(())
}

pub fn history_page(mut candles: Vec<Candle>, end: i64, market: Market) -> Result<HistoryPage> {
    for &candle in &candles {
        validate(candle, market.interval)?;
    }
    // Enforce exclusive cursors even if an upstream rounds its end parameter.
    candles.retain(|candle| candle.time <= end);
    candles.sort_unstable_by_key(|candle| candle.time);
    candles.dedup_by_key(|candle| candle.time);
    if candles.len() > PAGE_SIZE {
        candles.drain(..candles.len() - PAGE_SIZE);
    }
    // Native page limits differ. A short page is not proof of exhaustion.
    let next_before = candles.first().map(|candle| candle.time);
    Ok(HistoryPage {
        market,
        candles,
        next_before,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candle(time: i64) -> Candle {
        Candle {
            time,
            open: 10.0,
            high: 12.0,
            low: 9.0,
            close: 11.0,
            volume: 3.0,
            flow: None,
        }
    }

    #[test]
    fn short_native_pages_keep_an_exclusive_older_cursor() {
        let page = history_page(
            vec![candle(900), candle(600), candle(300)],
            749,
            Market::default(),
        )
        .unwrap();
        assert_eq!(
            page.candles
                .iter()
                .map(|candle| candle.time)
                .collect::<Vec<_>>(),
            [300, 600]
        );
        assert_eq!(page.next_before, Some(300));
        let older = history_page(vec![candle(600), candle(300)], 299, Market::default()).unwrap();
        assert!(older.candles.is_empty());
        assert_eq!(older.next_before, None);
    }

    #[test]
    fn invalid_native_values_cannot_enter_chart_history() {
        let mut invalid = candle(300);
        invalid.volume = f64::INFINITY;
        assert!(history_page(vec![invalid], 300, Market::default()).is_err());
        invalid = candle(300);
        invalid.close = 13.0;
        assert!(history_page(vec![invalid], 300, Market::default()).is_err());
        assert!(history_end(Some(0), 600, Timeframe::Min5).is_err());
        assert!(history_end(Some(901), 600, Timeframe::Min5).is_err());
    }

    #[test]
    fn signed_flow_requires_coherent_values_and_complete_bucket_coverage() {
        let valid = FlowBar {
            time: 300,
            flow: mexc_model::VolumeFlow {
                delta: Some(-1.5),
                volume: Some(3.0),
                quality: FlowQuality::Exact,
                covered_from: 300,
                through_ms: 600_000,
            },
        };
        validate_flow(valid, Timeframe::Min5).unwrap();
        for delta in [f64::NAN, f64::INFINITY, -3.1, 3.1] {
            let mut invalid = valid;
            invalid.flow.delta = Some(delta);
            assert!(validate_flow(invalid, Timeframe::Min5).is_err());
        }
        let mut invalid = valid;
        invalid.flow.quality = FlowQuality::Missing;
        assert!(validate_flow(invalid, Timeframe::Min5).is_err());
        invalid = valid;
        invalid.flow.covered_from = 301;
        assert!(validate_flow(invalid, Timeframe::Min5).is_err());
        invalid = valid;
        invalid.flow.through_ms = 600_001;
        assert!(validate_flow(invalid, Timeframe::Min5).is_err());
    }
}
