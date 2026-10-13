#[path = "coinbase_flow.rs"]
mod cvd;

use std::time::Duration;

use anyhow::{Context, Result, bail};
use mexc_model::{
    Candle, Catalog, Exchange, FlowBar, HistoryEvent, HistoryPage, Instrument, Market, MarketEvent,
    Product, Quote, Timeframe,
};
use reqwest::Client;
use serde::Deserialize;
use serde_json::Value;
use tokio::{
    sync::{mpsc, watch},
    time,
};

use super::{get_json, history_end, history_page, integer, number, unix_seconds};

const SPOT_BASE: &str = "https://api.exchange.coinbase.com";
const INTX_BASE: &str = "https://api.international.coinbase.com/api/v1";
const SPOT_INTERVALS: [Timeframe; 6] = [
    Timeframe::Min1,
    Timeframe::Min5,
    Timeframe::Min15,
    Timeframe::Min60,
    Timeframe::Hour6,
    Timeframe::Day1,
];
const PERP_INTERVALS: [Timeframe; 7] = [
    Timeframe::Min1,
    Timeframe::Min5,
    Timeframe::Min15,
    Timeframe::Min30,
    Timeframe::Min60,
    Timeframe::Hour6,
    Timeframe::Day1,
];

#[derive(Deserialize)]
struct SpotProduct {
    id: String,
    base_currency: String,
    quote_currency: String,
    quote_increment: String,
    status: String,
    trading_disabled: bool,
}

#[derive(Deserialize)]
struct IntxInstrument {
    symbol: String,
    #[serde(rename = "type")]
    kind: String,
    trading_state: String,
    base_asset_name: String,
    quote_asset_name: String,
    quote_increment: String,
    #[serde(default = "one")]
    base_asset_multiplier: String,
}

fn one() -> String {
    "1".to_owned()
}

fn quote(value: &str) -> Option<Quote> {
    match value {
        "USD" => Some(Quote::USD),
        "USDT" => Some(Quote::USDT),
        "USDC" => Some(Quote::USDC),
        _ => None,
    }
}

fn scale(increment: &str) -> Result<u32> {
    let decimals = increment
        .split_once('.')
        .map_or(0, |(_, tail)| tail.trim_end_matches('0').len());
    u32::try_from(decimals).context("price precision exceeds supported range")
}

fn spot_symbol(symbol: &str) -> Result<()> {
    if symbol.is_empty() || symbol.contains('/') || symbol.contains('?') || symbol.contains('#') {
        bail!("invalid Coinbase spot product id")
    }
    Ok(())
}

pub async fn catalog(client: &Client, product: Product) -> Result<Catalog> {
    let (intervals, instruments) = match product {
        Product::Spot => {
            let rows: Vec<SpotProduct> =
                get_json(client, &format!("{SPOT_BASE}/products"), &[]).await?;
            let instruments = rows
                .into_iter()
                .filter(|row| row.status == "online" && !row.trading_disabled)
                .filter_map(|row| {
                    let quote_coin = quote(&row.quote_currency)?;
                    Some(Instrument {
                        exchange: Exchange::Coinbase,
                        product: Product::Spot,
                        symbol: row.id,
                        base_coin: row.base_currency.clone(),
                        quote_coin,
                        settle_coin: row.quote_currency,
                        price_scale: scale(&row.quote_increment).ok()?,
                        contract_size: 1.0,
                        inverse: false,
                        volume_unit: row.base_currency,
                    })
                })
                .collect();
            (SPOT_INTERVALS.to_vec(), instruments)
        }
        Product::Perpetual => {
            let rows: Vec<IntxInstrument> =
                get_json(client, &format!("{INTX_BASE}/instruments"), &[]).await?;
            let instruments = rows
                .into_iter()
                .filter(|row| row.kind == "PERP" && row.trading_state == "TRADING")
                .filter_map(|row| {
                    let quote_coin = quote(&row.quote_asset_name)?;
                    let contract_size = row.base_asset_multiplier.parse::<f64>().ok()?;
                    if !contract_size.is_finite() || contract_size <= 0.0 {
                        return None;
                    }
                    Some(Instrument {
                        exchange: Exchange::Coinbase,
                        product: Product::Perpetual,
                        symbol: row.symbol,
                        base_coin: row.base_asset_name.clone(),
                        quote_coin,
                        settle_coin: row.quote_asset_name,
                        price_scale: scale(&row.quote_increment).ok()?,
                        contract_size,
                        inverse: false,
                        volume_unit: row.base_asset_name,
                    })
                })
                .collect();
            (PERP_INTERVALS.to_vec(), instruments)
        }
    };
    Ok(Catalog {
        exchange: Exchange::Coinbase,
        product,
        intervals,
        instruments,
        flow_support: super::flow_support(Exchange::Coinbase, product),
    })
}

fn spot_granularity(interval: Timeframe) -> Result<i64> {
    match interval {
        Timeframe::Min1
        | Timeframe::Min5
        | Timeframe::Min15
        | Timeframe::Min60
        | Timeframe::Hour6
        | Timeframe::Day1 => Ok(interval.seconds()),
        _ => bail!(
            "Coinbase Exchange spot does not provide native {} candles",
            interval.label()
        ),
    }
}

fn intx_granularity(interval: Timeframe) -> Result<&'static str> {
    match interval {
        Timeframe::Min1 => Ok("ONE_MINUTE"),
        Timeframe::Min5 => Ok("FIVE_MINUTE"),
        Timeframe::Min15 => Ok("FIFTEEN_MINUTE"),
        Timeframe::Min30 => Ok("THIRTY_MINUTE"),
        Timeframe::Min60 => Ok("ONE_HOUR"),
        Timeframe::Hour6 => Ok("SIX_HOUR"),
        Timeframe::Day1 => Ok("ONE_DAY"),
        _ => bail!(
            "Coinbase International does not provide native {} candles",
            interval.label()
        ),
    }
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += if month <= 2 { 1 } else { 0 };
    (year, month, day)
}

fn iso_time(seconds: i64) -> Result<String> {
    let days = seconds.div_euclid(86_400);
    let within_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = within_day / 3_600;
    let minute = within_day % 3_600 / 60;
    let second = within_day % 60;
    Ok(format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z"
    ))
}

fn parse_spot(values: &[Value]) -> Result<Candle> {
    if values.len() != 6 {
        bail!("unexpected Coinbase spot candle shape")
    }
    Ok(Candle {
        time: integer(&values[0])?,
        low: number(&values[1])?,
        high: number(&values[2])?,
        open: number(&values[3])?,
        close: number(&values[4])?,
        volume: number(&values[5])?,
        flow: None,
    })
}

#[derive(Deserialize)]
struct IntxCandles {
    #[serde(default)]
    aggregations: Vec<IntxAggregation>,
}
#[derive(Deserialize)]
struct IntxAggregation {
    start: String,
    open: String,
    high: String,
    low: String,
    close: String,
    volume: String,
}

fn parse_iso(value: &str) -> Result<i64> {
    let value = value
        .strip_suffix('Z')
        .context("Coinbase candle timestamp is not UTC")?;
    let (date, time) = value
        .split_once('T')
        .context("invalid Coinbase candle timestamp")?;
    let mut date = date.split('-');
    let year: i64 = date.next().context("missing year")?.parse()?;
    let month: i64 = date.next().context("missing month")?.parse()?;
    let day: i64 = date.next().context("missing day")?.parse()?;
    if date.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        bail!("invalid Coinbase candle date")
    }
    let mut time = time.split(':');
    let hour: i64 = time.next().context("missing hour")?.parse()?;
    let minute: i64 = time.next().context("missing minute")?.parse()?;
    let second = time.next().context("missing second")?;
    let second: i64 = second
        .split_once('.')
        .map_or(second, |(seconds, _)| seconds)
        .parse()?;
    if time.next().is_some() || hour > 23 || minute > 59 || second > 59 {
        bail!("invalid Coinbase candle time")
    }
    let year = year - if month <= 2 { 1 } else { 0 };
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let shifted_month = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * shifted_month + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Ok(days * 86_400 + hour * 3_600 + minute * 60 + second)
}

fn parse_intx(aggregation: IntxAggregation) -> Result<Candle> {
    Ok(Candle {
        time: parse_iso(&aggregation.start)?,
        open: aggregation.open.parse()?,
        high: aggregation.high.parse()?,
        low: aggregation.low.parse()?,
        close: aggregation.close.parse()?,
        volume: aggregation.volume.parse()?,
        flow: None,
    })
}

async fn fetch(
    client: &Client,
    instrument: &Instrument,
    market: &Market,
    start: i64,
    end: i64,
) -> Result<Vec<Candle>> {
    match market.product {
        Product::Spot => {
            spot_symbol(&instrument.symbol)?;
            let rows: Vec<Vec<Value>> = get_json(
                client,
                &format!("{SPOT_BASE}/products/{}/candles", instrument.symbol),
                &[
                    (
                        "granularity",
                        spot_granularity(market.interval)?.to_string(),
                    ),
                    ("start", iso_time(start)?),
                    ("end", iso_time(end)?),
                ],
            )
            .await?;
            rows.iter().map(|row| parse_spot(row)).collect()
        }
        Product::Perpetual => {
            let granularity = intx_granularity(market.interval)?;
            let response: IntxCandles = get_json(
                client,
                &format!("{INTX_BASE}/instruments/{}/candles", instrument.symbol),
                &[
                    ("granularity", granularity.to_owned()),
                    ("start", iso_time(start)?),
                    ("end", iso_time(end)?),
                ],
            )
            .await?;
            response.aggregations.into_iter().map(parse_intx).collect()
        }
    }
}

pub async fn history(
    client: &Client,
    instrument: &Instrument,
    market: &Market,
    before: Option<i64>,
) -> Result<HistoryPage> {
    let end = history_end(before, unix_seconds(), market.interval)?;
    // One native request stays within both Coinbase APIs' 300-candle limit.
    // The shared older cursor lets the caller page without extra hidden I/O.
    let start = end.saturating_sub(market.interval.seconds() * 298).max(0);
    let candles = fetch(client, instrument, market, start, end).await?;
    history_page(candles, end, market.clone())
}

pub async fn flow_history(
    client: &Client,
    instrument: &Instrument,
    market: &Market,
    candles: &[Candle],
    sender: &mpsc::Sender<HistoryEvent>,
) -> Result<()> {
    cvd::flow_history(client, instrument, market, candles, sender).await
}

pub async fn stream(
    client: &Client,
    instrument: &Instrument,
    market: &Market,
    cvd_enabled: bool,
    sender: &mpsc::Sender<MarketEvent>,
    shutdown: watch::Receiver<bool>,
) -> Result<()> {
    if cvd_enabled && market.product == Product::Spot {
        return cvd::stream(client, instrument, market, sender, shutdown).await;
    }
    if cvd_enabled {
        sender
            .send(MarketEvent::FlowStatus {
                connected: false,
                message: "Coinbase International signed public trades are unavailable; no portfolio fills or proxy flow are used".into(),
            })
            .await?;
    }
    price_stream(
        client,
        instrument,
        market,
        cvd_enabled,
        None,
        sender,
        shutdown,
    )
    .await
}

async fn price_stream(
    client: &Client,
    instrument: &Instrument,
    market: &Market,
    cvd_enabled: bool,
    price_coverage: Option<cvd::PriceCoverageHandle>,
    sender: &mpsc::Sender<MarketEvent>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    let width = market.interval.seconds();
    let mut published = 0_i64;
    let mut connected = false;
    loop {
        if *shutdown.borrow() {
            return Ok(());
        }
        let now = unix_seconds();
        let candle_start = now.div_euclid(width) * width;
        let start = if candle_start == published {
            candle_start
        } else {
            candle_start.saturating_sub(width)
        };
        let result = tokio::select! {
            _ = shutdown.changed() => return Ok(()),
            result = fetch(client, instrument, market, start, now) => result,
        };
        let candles = match result {
            Ok(candles) => candles,
            Err(error) if connected => {
                sender
                    .send(MarketEvent::Status {
                        connected: false,
                        message: format!("Coinbase REST candle polling failed: {error:#}"),
                    })
                    .await?;
                return Err(error);
            }
            Err(error) => return Err(error),
        };
        if let Some(coverage) = &price_coverage {
            cvd::record_price_observation(
                coverage,
                &candles,
                start,
                now.saturating_mul(1_000),
                width,
            )
            .await;
        }
        if !connected {
            sender
                .send(MarketEvent::Status {
                    connected: true,
                    message: "Connected; Coinbase native REST candles (polled)".into(),
                })
                .await?;
            connected = true;
        }
        for candle in candles {
            if candle.time < start
                || candle.time > candle_start
                || (candle.time <= published && candle.time != candle_start)
            {
                continue;
            }
            sender.send(MarketEvent::Candle { candle }).await?;
            if cvd_enabled && market.product == Product::Perpetual {
                sender
                    .send(MarketEvent::Flow {
                        bar: FlowBar {
                            time: candle.time,
                            flow: super::flow::missing(
                                candle.time,
                                candle.time.saturating_mul(1_000),
                            ),
                        },
                    })
                    .await?;
            }
            published = candle.time;
        }
        tokio::select! {
            _ = shutdown.changed() => { if *shutdown.borrow() { return Ok(()); } }
            _ = time::sleep(Duration::from_secs(2)) => {}
        }
    }
}
