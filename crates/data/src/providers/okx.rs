use std::{
    collections::{BTreeMap, HashSet, VecDeque},
    io::{Cursor, Read},
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use futures_util::{SinkExt, StreamExt};
use mexc_model::{
    Candle, Catalog, Exchange, FlowBar, FlowQuality, HistoryEvent, HistoryPage, Instrument, Market,
    MarketEvent, Product, QuantityMode, Quote, Timeframe,
};
use reqwest::Client;
use rust_decimal::{
    Decimal,
    prelude::{FromPrimitive, ToPrimitive},
};
use serde_json::{Value, json};
use tokio::{
    sync::{Mutex, mpsc, watch},
    time::{Instant, sleep_until},
};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use zip::ZipArchive;

use super::{
    flow, flow_support, get_json, history_end, history_page, integer, number, unix_seconds,
};

const REST: &str = "https://www.okx.com/api/v5";
const WS: &str = "wss://ws.okx.com:443/ws/v5/business";

static STAT_REQUESTS: Mutex<Option<Instant>> = Mutex::const_new(None);
static TAPE_REQUESTS: Mutex<Option<Instant>> = Mutex::const_new(None);

async fn pace_request(gate: &Mutex<Option<Instant>>, spacing: Duration) {
    let mut next = gate.lock().await;
    let now = Instant::now();
    let at = next.unwrap_or(now).max(now);
    *next = Some(at + spacing);
    drop(next);
    sleep_until(at).await;
}
const INTERVALS: [Timeframe; 8] = [
    Timeframe::Min1,
    Timeframe::Min5,
    Timeframe::Min15,
    Timeframe::Min30,
    Timeframe::Min60,
    Timeframe::Hour4,
    Timeframe::Hour6,
    Timeframe::Day1,
];

fn bar(interval: Timeframe) -> &'static str {
    match interval {
        Timeframe::Min1 => "1m",
        Timeframe::Min5 => "5m",
        Timeframe::Min15 => "15m",
        Timeframe::Min30 => "30m",
        Timeframe::Min60 => "1H",
        Timeframe::Hour4 => "4H",
        Timeframe::Hour6 => "6Hutc",
        Timeframe::Hour8 => unreachable!("not advertised by OKX"),
        Timeframe::Day1 => "1Dutc",
    }
}

fn decimal_places(text: &str) -> u32 {
    text.split_once('.')
        .map(|(_, fraction)| fraction.trim_end_matches('0').len() as u32)
        .unwrap_or(0)
}

fn parse_envelope(value: Value) -> Result<Vec<Value>> {
    let code = value
        .get("code")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if code != "0" {
        bail!(
            "OKX API error {}: {}",
            code,
            value
                .get("msg")
                .and_then(Value::as_str)
                .unwrap_or("unknown error")
        );
    }
    value
        .get("data")
        .and_then(Value::as_array)
        .cloned()
        .ok_or_else(|| anyhow!("OKX response lacks data array"))
}

pub async fn catalog(client: &Client, product: Product) -> Result<Catalog> {
    let inst_type = match product {
        Product::Spot => "SPOT",
        Product::Perpetual => "SWAP",
    };
    let raw: Value = get_json(
        client,
        &format!("{REST}/public/instruments"),
        &[("instType", inst_type.to_owned())],
    )
    .await?;
    let mut instruments = Vec::new();
    for item in parse_envelope(raw)? {
        if item.get("state").and_then(Value::as_str) != Some("live") {
            continue;
        }
        let symbol = item
            .get("instId")
            .and_then(Value::as_str)
            .context("OKX instrument missing instId")?;
        let (base, quote_text) = if product == Product::Spot {
            let base = item
                .get("baseCcy")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let quote = item
                .get("quoteCcy")
                .and_then(Value::as_str)
                .unwrap_or_default();
            (base, quote)
        } else {
            let mut parts = symbol.split('-');
            let base = parts.next().unwrap_or_default();
            let quote = parts.next().unwrap_or_default();
            (base, quote)
        };
        let Some(quote_coin) = Quote::parse(quote_text) else {
            continue;
        };
        let (price_scale, contract_size, inverse, settle_coin, volume_unit) =
            if product == Product::Spot {
                let tick = item
                    .get("tickSz")
                    .and_then(Value::as_str)
                    .context("OKX spot instrument missing tickSz")?;
                (
                    decimal_places(tick),
                    1.0,
                    false,
                    quote_text.to_owned(),
                    base.to_owned(),
                )
            } else {
                let inverse = item.get("ctType").and_then(Value::as_str) == Some("inverse");
                let settle = item
                    .get("settleCcy")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let ct_val = item
                    .get("ctVal")
                    .and_then(Value::as_str)
                    .unwrap_or("0")
                    .parse::<f64>()
                    .context("invalid OKX ctVal")?;
                let tick = item
                    .get("tickSz")
                    .and_then(Value::as_str)
                    .context("OKX swap instrument missing tickSz")?;
                (
                    decimal_places(tick),
                    ct_val,
                    inverse,
                    settle.to_owned(),
                    "contracts".to_owned(),
                )
            };
        if base.is_empty() || contract_size <= 0.0 {
            continue;
        }
        instruments.push(Instrument {
            exchange: Exchange::Okx,
            product,
            symbol: symbol.to_owned(),
            base_coin: base.to_owned(),
            quote_coin,
            settle_coin,
            price_scale,
            contract_size,
            inverse,
            volume_unit,
        });
    }
    Ok(Catalog {
        exchange: Exchange::Okx,
        product,
        flow_support: flow_support(Exchange::Okx, product),
        intervals: INTERVALS.to_vec(),
        instruments,
    })
}

fn candle_row(row: &Value) -> Result<Candle> {
    let cols = row.as_array().context("OKX candle row is not an array")?;
    if cols.len() < 6 {
        bail!("OKX candle row has fewer than six fields");
    }
    let ms = integer(&cols[0])?;
    Ok(Candle {
        time: ms.div_euclid(1000),
        open: number(&cols[1])?,
        high: number(&cols[2])?,
        low: number(&cols[3])?,
        close: number(&cols[4])?,
        volume: number(&cols[5])?,
        flow: None,
    })
}

/// Select quantities at the execution boundary, never from candle prices.
struct FlowQuantities<'a> {
    instrument: &'a Instrument,
    multiplier: Decimal,
    inverse: bool,
}

impl<'a> FlowQuantities<'a> {
    fn new(instrument: &'a Instrument, quantity: QuantityMode) -> Result<Self> {
        let convert = quantity == QuantityMode::Base && instrument.product == Product::Perpetual;
        let multiplier = if convert {
            Decimal::from_f64(instrument.contract_size)
                .context("unrepresentable OKX contract face value")?
                .normalize()
        } else {
            Decimal::ONE
        };
        if multiplier <= Decimal::ZERO {
            bail!("non-positive OKX contract face value");
        }
        Ok(Self {
            instrument,
            multiplier,
            inverse: convert && instrument.inverse,
        })
    }

    fn execution(&self, native: Decimal, price: Option<&str>) -> Result<Decimal> {
        if self.multiplier == Decimal::ONE && !self.inverse {
            return Ok(native);
        }
        let scaled = native
            .checked_mul(self.multiplier)
            .context("OKX base quantity overflow")?;
        if native != Decimal::ZERO && scaled == Decimal::ZERO {
            bail!("OKX base quantity underflow");
        }
        if !self.inverse {
            return Ok(scaled);
        }
        let price = flow::amount(price.context("OKX inverse execution lacks price")?)?;
        if price <= Decimal::ZERO {
            bail!("non-positive OKX inverse execution price");
        }
        let converted = scaled
            .checked_div(price)
            .context("OKX inverse base quantity overflow")?;
        if native != Decimal::ZERO && converted == Decimal::ZERO {
            bail!("OKX inverse base quantity underflow");
        }
        Ok(converted)
    }
}

fn execution_size(row: &Value, quantities: &FlowQuantities<'_>) -> Result<(Decimal, Decimal)> {
    let native = flow::amount(
        row.get("sz")
            .and_then(Value::as_str)
            .context("OKX trade lacks sz")?,
    )?;
    let converted = quantities.execution(native, row.get("px").and_then(Value::as_str))?;
    Ok((native, converted))
}

fn native_volume_matches(native: Decimal, candle: &Candle) -> bool {
    native.to_f64().is_some_and(|total| {
        total.is_finite() && (total - candle.volume).abs() <= candle.volume.abs().max(1.0) * 1e-8
    })
}

fn quantity_message(instrument: &Instrument, quantity: QuantityMode) -> &'static str {
    match quantity {
        QuantityMode::Native => "Native quantities are preserved.",
        QuantityMode::Base if instrument.product == Product::Spot => {
            "Base quantities: native spot sizes are already base coin."
        }
        QuantityMode::Base if instrument.inverse => {
            "Base quantities: each inverse execution is converted using contract face value / actual execution price; contract totals still reconcile native candle volume."
        }
        QuantityMode::Base => {
            "Base quantities: native contract sizes are scaled by instrument contract size; contract totals still reconcile native candle volume."
        }
    }
}

#[derive(Default)]
struct TradeTotals {
    buy: Decimal,
    sell: Decimal,
    native_volume: Decimal,
    ids: HashSet<u64>,
}

impl TradeTotals {
    fn add(&mut self, native: Decimal, converted: Decimal, buy: bool) -> Result<()> {
        self.native_volume = self
            .native_volume
            .checked_add(native)
            .context("OKX native trade volume overflow")?;
        let side = if buy { &mut self.buy } else { &mut self.sell };
        *side = side
            .checked_add(converted)
            .context("OKX trade side overflow")?;
        Ok(())
    }
}

#[derive(Default)]
struct LiveBucket {
    buy: Decimal,
    sell: Decimal,
    native_volume: Decimal,
    through_ms: i64,
    invalid: bool,
}

#[derive(Debug, PartialEq, Eq)]
enum LiveIdResult {
    New,
    Duplicate,
    UnseenLower,
}

const LIVE_ID_CACHE: usize = 4096;

fn remember_live_id(
    id: u64,
    highwater: &mut Option<u64>,
    recent: &mut HashSet<u64>,
    order: &mut VecDeque<u64>,
) -> LiveIdResult {
    if recent.contains(&id) {
        return LiveIdResult::Duplicate;
    }
    let lower = highwater.is_some_and(|last| id < last);
    recent.insert(id);
    order.push_back(id);
    if order.len() > LIVE_ID_CACHE
        && let Some(expired) = order.pop_front()
    {
        recent.remove(&expired);
    }
    if highwater.is_none_or(|last| id > last) {
        *highwater = Some(id);
    }
    if lower {
        LiveIdResult::UnseenLower
    } else {
        LiveIdResult::New
    }
}

fn header_index(headers: &csv::StringRecord, names: &[&str]) -> Result<usize> {
    headers
        .iter()
        .position(|h| names.iter().any(|n| h.eq_ignore_ascii_case(n)))
        .ok_or_else(|| {
            anyhow!(
                "OKX trade archive lacks expected column {}",
                names.join("/")
            )
        })
}

fn add_archive_records(
    input: impl Read,
    instrument: &Instrument,
    bars: &mut BTreeMap<i64, TradeTotals>,
    interval: Timeframe,
    quantities: &FlowQuantities<'_>,
) -> Result<()> {
    let mut reader = csv::ReaderBuilder::new().flexible(true).from_reader(input);
    let headers = reader.headers()?.clone();
    let instrument_idx = header_index(&headers, &["instrument_name", "instId", "合约名称"])?;
    let id_idx = header_index(&headers, &["trade_id", "tradeId", "成交ID"])?;
    let side_idx = header_index(&headers, &["side", "方向"])?;
    let size_idx = header_index(&headers, &["size", "sz", "成交数量"])?;
    let time_idx = header_index(&headers, &["created_time", "ts", "成交时间"])?;
    let price_idx = if quantities.inverse {
        Some(header_index(&headers, &["price", "px", "成交价格"])?)
    } else {
        None
    };
    for row in reader.records() {
        let row = row?;
        if row.get(instrument_idx) != Some(instrument.symbol.as_str()) {
            continue;
        }
        let ts = row
            .get(time_idx)
            .context("OKX archive row lacks timestamp")?
            .parse::<i64>()?;
        let bucket = ts.div_euclid(interval.seconds().saturating_mul(1000)) * interval.seconds();
        let Some(totals) = bars.get_mut(&bucket) else {
            continue;
        };
        let id = row
            .get(id_idx)
            .context("OKX archive row lacks trade ID")?
            .parse::<u64>()
            .context("invalid OKX archive trade ID")?;
        if !totals.ids.insert(id) {
            continue;
        }
        let native = flow::amount(row.get(size_idx).context("OKX archive row lacks size")?)?;
        let converted = quantities.execution(native, price_idx.and_then(|index| row.get(index)))?;
        let buy = match row
            .get(side_idx)
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "buy" | "买入" => true,
            "sell" | "卖出" => false,
            side => bail!("unknown OKX archive trade side {side}"),
        };
        totals.add(native, converted, buy)?;
    }
    Ok(())
}

fn add_archive_csv(
    bytes: &[u8],
    instrument: &Instrument,
    bars: &mut BTreeMap<i64, TradeTotals>,
    interval: Timeframe,
    quantities: &FlowQuantities<'_>,
) -> Result<()> {
    let mut zip = ZipArchive::new(Cursor::new(bytes)).context("opening OKX trade archive ZIP")?;
    for index in 0..zip.len() {
        let mut file = zip.by_index(index)?;
        if file.name().to_ascii_lowercase().ends_with(".csv") {
            add_archive_records(&mut file, instrument, bars, interval, quantities)?;
        }
    }
    Ok(())
}

async fn archive_trades(
    client: &Client,
    candles: &[Candle],
    interval: Timeframe,
    bars: &mut BTreeMap<i64, TradeTotals>,
    available: &mut HashSet<i64>,
    sender: &mpsc::Sender<HistoryEvent>,
    quantities: &FlowQuantities<'_>,
) -> Result<()> {
    let instrument = quantities.instrument;
    if candles.is_empty() {
        return Ok(());
    }
    let first = candles.iter().map(|c| c.time).min().unwrap();
    let last = candles.iter().map(|c| c.time).max().unwrap();
    const TZ8: i64 = 8 * 60 * 60 * 1000;
    const DAY: i64 = 24 * 60 * 60 * 1000;
    let start = (first * 1000 + TZ8).div_euclid(DAY) * DAY - TZ8;
    let finish = (last * 1000 + TZ8).div_euclid(DAY) * DAY - TZ8;
    let (inst_type, key, value) = if instrument.product == Product::Spot {
        ("SPOT", "instIdList", instrument.symbol.clone())
    } else {
        let family = instrument
            .symbol
            .strip_suffix("-SWAP")
            .context("OKX perpetual symbol lacks -SWAP")?
            .to_owned();
        ("SWAP", "instFamilyList", family)
    };
    let mut day = start;
    while day <= finish && !sender.is_closed() {
        let end = (day + 9 * DAY).min(finish);
        let raw: Value = get_json(
            client,
            &format!("{REST}/public/market-data-history"),
            &[
                ("module", "1".into()),
                ("instType", inst_type.into()),
                (key, value.clone()),
                ("dateAggrType", "daily".into()),
                ("begin", day.to_string()),
                ("end", end.to_string()),
            ],
        )
        .await?;
        for group in parse_envelope(raw)? {
            let details = group
                .get("details")
                .and_then(Value::as_array)
                .context("OKX archive response lacks details")?;
            for detail in details {
                let files = detail
                    .get("groupDetails")
                    .and_then(Value::as_array)
                    .context("OKX archive response lacks groupDetails")?;
                for archive in files {
                    if sender.is_closed() {
                        return Ok(());
                    }
                    let url = archive
                        .get("url")
                        .and_then(Value::as_str)
                        .context("OKX archive lacks URL")?;
                    let response = client.get(url).send().await?.error_for_status()?;
                    let date_ts =
                        integer(archive.get("dateTs").context("OKX archive lacks dateTs")?)?
                            .div_euclid(1000);
                    for candle in candles
                        .iter()
                        .filter(|c| c.time >= date_ts && c.time < date_ts + 86_400)
                    {
                        available.insert(candle.time);
                    }
                    let bytes = response.bytes().await?;
                    add_archive_csv(&bytes, instrument, bars, interval, quantities)?;
                }
            }
        }
        day = end + DAY;
    }
    Ok(())
}

fn stat_period(interval: Timeframe) -> Option<&'static str> {
    Some(match interval {
        Timeframe::Min5 => "5m",
        Timeframe::Min15 => "15m",
        Timeframe::Min30 => "30m",
        Timeframe::Min60 => "1H",
        Timeframe::Hour4 => "4H",
        Timeframe::Hour6 => "6Hutc",
        Timeframe::Day1 => "1Dutc",
        Timeframe::Min1 | Timeframe::Hour8 => return None,
    })
}

fn amount_text(value: &Value) -> Result<Decimal> {
    flow::amount(value.as_str().context("OKX quantity is not a string")?)
}

fn bar_end(candle: &Candle, market: &Market) -> i64 {
    candle.time.saturating_add(market.interval.seconds())
}

fn contract_taker_row(row: &Value) -> Result<(i64, Decimal, Decimal)> {
    let cols = row.as_array().context("invalid OKX taker-stat row")?;
    if cols.len() < 3 {
        bail!("short OKX taker-stat row");
    }
    Ok((
        integer(&cols[0])?.div_euclid(1000),
        amount_text(&cols[2])?,
        amount_text(&cols[1])?,
    ))
}

async fn rest_trade_history(
    client: &Client,
    candles: &[Candle],
    interval: Timeframe,
    bars: &mut BTreeMap<i64, TradeTotals>,
    coverage: &mut HashSet<i64>,
    sender: &mpsc::Sender<HistoryEvent>,
    quantities: &FlowQuantities<'_>,
) -> Result<()> {
    let instrument = quantities.instrument;
    if candles.is_empty() {
        return Ok(());
    }
    let first = candles.iter().map(|c| c.time).min().unwrap();
    let last = candles.iter().map(|c| c.time).max().unwrap();
    let start_ms = first.saturating_mul(1000);
    let end_ms = last.saturating_add(interval.seconds()).saturating_mul(1000);
    let mut query = vec![
        ("instId", instrument.symbol.clone()),
        ("type", "2".to_owned()),
        ("after", end_ms.to_string()),
        ("limit", "100".to_owned()),
    ];
    let mut visited = HashSet::new();
    let mut covered_start = false;
    let mut cursor = None;
    let mut pages = 0usize;
    while !sender.is_closed() {
        pace_request(&TAPE_REQUESTS, Duration::from_millis(110)).await;
        let raw: Value = get_json(client, &format!("{REST}/market/history-trades"), &query).await?;
        let rows = parse_envelope(raw)?;
        if rows.is_empty() {
            break;
        }
        let page_len = rows.len();
        let mut oldest: Option<(i64, u64)> = None;
        for row in rows {
            let id = row
                .get("tradeId")
                .and_then(Value::as_str)
                .context("OKX history trade lacks tradeId")?
                .parse::<u64>()
                .context("invalid OKX history trade ID")?;
            if !visited.insert(id) {
                continue;
            }
            let ts = row
                .get("ts")
                .and_then(Value::as_str)
                .context("OKX history trade lacks ts")?
                .parse::<i64>()?;
            if oldest
                .as_ref()
                .is_none_or(|(old_ts, old_id)| ts < *old_ts || (ts == *old_ts && id < *old_id))
            {
                oldest = Some((ts, id));
            }
            if ts < start_ms {
                covered_start = true;
                continue;
            }
            if ts >= end_ms {
                continue;
            }
            let bucket =
                ts.div_euclid(interval.seconds().saturating_mul(1000)) * interval.seconds();
            let Some(total) = bars.get_mut(&bucket) else {
                continue;
            };
            let (native, converted) = execution_size(&row, quantities)?;
            let buy = match row
                .get("side")
                .and_then(Value::as_str)
                .context("OKX history trade lacks side")?
            {
                "buy" => true,
                "sell" => false,
                side => bail!("unknown OKX history trade side {side}"),
            };
            total.add(native, converted, buy)?;
        }
        pages += 1;
        if pages.is_multiple_of(10) {
            sender.send(HistoryEvent::FlowStatus {
                message: format!("OKX complete native trade replay: {pages} pages fetched; unproven bars remain incomplete"),
            }).await?;
        }
        if covered_start {
            break;
        }
        let Some((_, id)) = oldest else {
            break;
        };
        if cursor.as_ref() == Some(&id) {
            break;
        }
        cursor = Some(id);
        query[1].1 = "1".into();
        query[2].1 = id.to_string();
        if page_len < 100 {
            break;
        }
    }
    if covered_start {
        coverage.extend(candles.iter().map(|c| c.time));
    }
    Ok(())
}

/// Exact instrument-specific statistics are a cheap recent source for supported swap bars;
/// archives provide the deeper native tape where OKX actually publishes the instrument.
pub async fn flow_history(
    client: &Client,
    instrument: &Instrument,
    market: &Market,
    candles: &[Candle],
    sender: &mpsc::Sender<HistoryEvent>,
    quantity: QuantityMode,
) -> Result<()> {
    let quantities = FlowQuantities::new(instrument, quantity)?;
    sender.send(HistoryEvent::FlowStatus {
        message: format!("Reading OKX trade sources. Recent completed SWAP bars are checked against instrument-specific contract-taker statistics when quantities permit; gaps fall back to raw tape and official archives. {}", quantity_message(instrument, quantity)),
    }).await?;
    let mut stat_sent = HashSet::new();
    let closed: Vec<Candle> = candles
        .iter()
        .copied()
        .filter(|c| bar_end(c, market) * 1000 <= flow::unix_millis())
        .collect();
    if instrument.product == Product::Perpetual
        // No authoritative execution-priced base fields are documented for this
        // contract-statistics endpoint. Inverse base flow requires actual tape.
        && !quantities.inverse
        && let Some(period) = stat_period(market.interval)
        && let (Some(first), Some(last)) = (
            closed.iter().map(|c| c.time).min(),
            closed.iter().map(|c| c.time).max(),
        )
    {
        let mut cursor = last;
        while !sender.is_closed() {
            pace_request(&STAT_REQUESTS, Duration::from_millis(450)).await;
            let raw: Value = get_json(
                client,
                &format!("{REST}/rubik/stat/taker-volume-contract"),
                &[
                    ("instId", instrument.symbol.clone()),
                    ("period", period.to_owned()),
                    ("unit", "1".to_owned()),
                    ("limit", "100".to_owned()),
                    (
                        "end",
                        cursor
                            .saturating_add(market.interval.seconds())
                            .saturating_mul(1000)
                            .to_string(),
                    ),
                ],
            )
            .await?;
            let rows = parse_envelope(raw)?;
            if rows.is_empty() {
                break;
            }
            let mut oldest = cursor;
            for row in rows {
                let (ts, buy, sell) = contract_taker_row(&row)?;
                oldest = oldest.min(ts);
                if stat_sent.contains(&ts) {
                    continue;
                }
                let Some(candle) = closed.iter().find(|c| c.time == ts) else {
                    continue;
                };
                if native_volume_matches(buy + sell, candle) {
                    let flow_value = flow::snapshot(
                        quantities.execution(buy, None)?,
                        quantities.execution(sell, None)?,
                        FlowQuality::Exact,
                        ts,
                        bar_end(candle, market).saturating_mul(1000),
                    )?;
                    sender
                        .send(HistoryEvent::Flow {
                            bar: FlowBar {
                                time: ts,
                                flow: flow_value,
                            },
                        })
                        .await?;
                    stat_sent.insert(ts);
                }
            }
            if oldest >= cursor {
                break;
            }
            cursor = oldest.saturating_sub(1);
            if cursor <= first {
                break;
            }
        }
    }

    // Only bars not already proven by the cheap native statistics source reach
    // potentially large archive or raw-trade downloads.
    let remaining: Vec<Candle> = candles
        .iter()
        .copied()
        .filter(|c| !stat_sent.contains(&c.time))
        .collect();
    if remaining.is_empty() {
        return Ok(());
    }
    let completed: Vec<Candle> = remaining
        .iter()
        .copied()
        .filter(|c| bar_end(c, market) * 1000 <= flow::unix_millis())
        .collect();
    if completed.is_empty() {
        for candle in remaining {
            sender
                .send(HistoryEvent::Flow {
                    bar: FlowBar {
                        time: candle.time,
                        flow: flow::missing(candle.time, bar_end(&candle, market) * 1000),
                    },
                })
                .await?;
        }
        return Ok(());
    }
    let mut bars = BTreeMap::new();
    for candle in &completed {
        bars.insert(candle.time, TradeTotals::default());
    }
    let mut archive_coverage = HashSet::new();
    archive_trades(
        client,
        &completed,
        market.interval,
        &mut bars,
        &mut archive_coverage,
        sender,
        &quantities,
    )
    .await?;
    let cutoff = unix_seconds().saturating_sub(90 * 86_400);
    let recent: Vec<Candle> = completed
        .iter()
        .copied()
        .filter(|c| c.time >= cutoff && !archive_coverage.contains(&c.time))
        .collect();
    let mut rest_coverage = HashSet::new();
    rest_trade_history(
        client,
        &recent,
        market.interval,
        &mut bars,
        &mut rest_coverage,
        sender,
        &quantities,
    )
    .await?;

    for candle in remaining {
        if sender.is_closed() {
            return Ok(());
        }
        let end_ms = bar_end(&candle, market).saturating_mul(1000);
        let result =
            if archive_coverage.contains(&candle.time) || rest_coverage.contains(&candle.time) {
                let totals = bars.get(&candle.time).expect("initialized");
                if native_volume_matches(totals.native_volume, &candle) {
                    flow::snapshot(
                        totals.buy,
                        totals.sell,
                        FlowQuality::Exact,
                        candle.time,
                        end_ms,
                    )?
                } else {
                    flow::missing(candle.time, end_ms)
                }
            } else {
                flow::missing(candle.time, end_ms)
            };
        sender
            .send(HistoryEvent::Flow {
                bar: FlowBar {
                    time: candle.time,
                    flow: result,
                },
            })
            .await?;
    }
    Ok(())
}

pub async fn history(
    client: &Client,
    instrument: &Instrument,
    market: &Market,
    before: Option<i64>,
    _quantity: QuantityMode,
) -> Result<HistoryPage> {
    // Quantity mode affects only flow; OKX's native OHLC and candle volume stay unchanged.
    let end = history_end(before, unix_seconds(), market.interval)?;
    let (path, limit) = if before.is_some() {
        ("history-candles", "100")
    } else {
        ("candles", "300")
    };
    let raw: Value = get_json(
        client,
        &format!("{REST}/market/{path}"),
        &[
            ("instId", instrument.symbol.clone()),
            ("bar", bar(market.interval).to_owned()),
            // OKX's after cursor selects older records; before selects newer.
            (
                "after",
                end.saturating_add(1).saturating_mul(1000).to_string(),
            ),
            ("limit", limit.to_owned()),
        ],
    )
    .await?;
    let candles = parse_envelope(raw)?
        .iter()
        .map(candle_row)
        .collect::<Result<Vec<_>>>()?;
    history_page(candles, end, market.clone())
}

async fn replay_bucket(
    client: &Client,
    instrument: &Instrument,
    candle: &Candle,
    interval: Timeframe,
    quantities: &FlowQuantities<'_>,
) -> Result<Option<TradeTotals>> {
    let start_ms = candle.time.saturating_mul(1000);
    let end_ms = candle
        .time
        .saturating_add(interval.seconds())
        .saturating_mul(1000);
    let mut query = vec![
        ("instId", instrument.symbol.clone()),
        ("type", "2".to_owned()),
        ("after", end_ms.to_string()),
        ("limit", "100".to_owned()),
    ];
    let mut totals = TradeTotals::default();
    let mut covered_start = false;
    loop {
        pace_request(&TAPE_REQUESTS, Duration::from_millis(110)).await;
        let raw: Value = get_json(client, &format!("{REST}/market/history-trades"), &query).await?;
        let rows = parse_envelope(raw)?;
        let page_len = rows.len();
        if rows.is_empty() {
            break;
        }
        let mut oldest_id: Option<u64> = None;
        let mut oldest_ts = i64::MAX;
        for row in rows {
            let id = row
                .get("tradeId")
                .and_then(Value::as_str)
                .context("OKX history trade lacks tradeId")?
                .parse::<u64>()
                .context("invalid OKX history trade ID")?;
            if !totals.ids.insert(id) {
                continue;
            }
            let ts = row
                .get("ts")
                .and_then(Value::as_str)
                .context("OKX history trade lacks ts")?
                .parse::<i64>()?;
            let tie_is_older = ts == oldest_ts && oldest_id.is_some_and(|old| id < old);
            if ts < oldest_ts || tie_is_older {
                oldest_ts = ts;
                oldest_id = Some(id);
            }
            if ts < start_ms {
                covered_start = true;
            }
            if ts < start_ms || ts >= end_ms {
                continue;
            }
            let (native, converted) = execution_size(&row, quantities)?;
            let buy = match row
                .get("side")
                .and_then(Value::as_str)
                .context("OKX history trade lacks side")?
            {
                "buy" => true,
                "sell" => false,
                side => bail!("unknown OKX history trade side {side}"),
            };
            totals.add(native, converted, buy)?;
        }
        if covered_start {
            break;
        }
        let Some(id) = oldest_id else {
            break;
        };
        query[1].1 = "1".into();
        query[2].1 = id.to_string();
        if page_len < 100 {
            break;
        }
    }
    if !covered_start {
        return Ok(None);
    }
    Ok(Some(totals))
}
fn ws_bar(interval: Timeframe) -> &'static str {
    bar(interval)
}

pub async fn stream(
    client: &Client,
    instrument: &Instrument,
    market: &Market,
    cvd: bool,
    sender: &mpsc::Sender<MarketEvent>,
    mut shutdown: watch::Receiver<bool>,
    quantity: QuantityMode,
) -> Result<()> {
    if *shutdown.borrow() {
        return Ok(());
    }
    let quantities = FlowQuantities::new(instrument, quantity)?;
    let (socket, _) = tokio::select! {
        changed = shutdown.changed() => { let _ = changed; return Ok(()); }
        result = connect_async(WS) => result.context("connecting to OKX business WebSocket")?,
    };
    let (mut write, mut read) = socket.split();
    let channel = format!("candle{}", ws_bar(market.interval));
    let mut args = vec![json!({"channel":channel,"instId":instrument.symbol})];
    if cvd {
        args.push(json!({"channel":"trades-all","instId":instrument.symbol}));
    }
    write
        .send(Message::Text(
            json!({"op":"subscribe","args":args}).to_string().into(),
        ))
        .await?;
    let mut subscribed = false;
    let candle_channel = format!("candle{}", ws_bar(market.interval));
    let mut live_buckets = BTreeMap::<i64, LiveBucket>::new();
    let mut flow_ready_at = None;
    let mut finalized = HashSet::new();
    let mut last_live_trade_id = None;
    let mut recent_live_ids = HashSet::new();
    let mut recent_live_order = VecDeque::new();
    loop {
        tokio::select! {
            changed = shutdown.changed() => { if changed.is_err() || *shutdown.borrow() { return Ok(()); } }
            incoming = tokio::time::timeout(Duration::from_secs(25), read.next()) => {
                match incoming {
                    Err(_) => write.send(Message::Text("ping".into())).await.context("sending OKX heartbeat")?,
                    Ok(None) => {
                        if cvd { let _ = sender.send(MarketEvent::FlowStatus { connected:false, message:"OKX trade socket closed; reconnect must recover missed raw IDs before publishing coverage.".into() }).await; }
                        bail!("OKX WebSocket closed")
                    },
                    Ok(Some(Err(error))) => {
                        if cvd { let _ = sender.send(MarketEvent::FlowStatus { connected:false, message:format!("OKX trade socket interrupted; raw trade replay required: {error}") }).await; }
                        return Err(error).context("reading OKX WebSocket")
                    },
                    Ok(Some(Ok(Message::Text(text)))) => {
                        if text == "pong" { continue; }
                        let value: Value = serde_json::from_str(&text).context("invalid OKX WebSocket JSON")?;
                        if value.get("event").and_then(Value::as_str) == Some("error") { bail!("OKX WebSocket error: {}", value.get("msg").and_then(Value::as_str).unwrap_or("unknown")); }
                        if value.get("event").and_then(Value::as_str) == Some("subscribe") {
                            if !subscribed {
                                subscribed = true;
                                sender.send(MarketEvent::Status { connected: true, message: "Connected to OKX candle WebSocket".into() }).await?;
                            }
                            if cvd && value.pointer("/arg/channel").and_then(Value::as_str) == Some("trades-all") {
                                flow_ready_at = Some(flow::unix_millis().saturating_add(1_000));
                                sender.send(MarketEvent::FlowStatus { connected: true, message:format!("OKX trades-all acknowledged; forming-bar flow starts only in buckets opening after subscription is established. Closed bars require REST/raw-ID reconciliation. {}", quantity_message(instrument, quantity)) }).await?;
                            }
                            continue;
                        }
                        let channel = value.pointer("/arg/channel").and_then(Value::as_str).unwrap_or_default();
                        let Some(rows) = value.get("data").and_then(Value::as_array) else { continue; };
                        if !subscribed {
                            subscribed = true;
                            sender.send(MarketEvent::Status { connected: true, message: "Connected to OKX candle WebSocket".into() }).await?;
                        }
                        if channel == "trades-all" {
                            if !cvd { continue; }
                            for row in rows {
                                let id = row.get("tradeId").and_then(Value::as_str).context("OKX live trade lacks tradeId")?.parse::<u64>().context("OKX trade ID is not an unsigned integer")?;
                                let id_result = remember_live_id(id, &mut last_live_trade_id, &mut recent_live_ids, &mut recent_live_order);
                                if id_result == LiveIdResult::Duplicate { continue; }
                                let ts = row.get("ts").and_then(Value::as_str).context("OKX live trade lacks ts")?.parse::<i64>()?;
                                let (native, size) = execution_size(row, &quantities)?;
                                let buy = match row.get("side").and_then(Value::as_str).context("OKX live trade lacks side")? {
                                    "buy" => true,
                                    "sell" => false,
                                    side => bail!("unknown OKX live trade side {side}"),
                                };
                                let bucket = ts.div_euclid(market.interval.seconds().saturating_mul(1000)) * market.interval.seconds();
                                let start_ms = bucket.saturating_mul(1000);
                                let end_ms = bucket.saturating_add(market.interval.seconds()).saturating_mul(1000);
                                if id_result == LiveIdResult::UnseenLower {
                                    let was_finalized = finalized.contains(&bucket);
                                    let state = live_buckets.entry(bucket).or_default();
                                    state.invalid = true;
                                    state.through_ms = state.through_ms.max(if was_finalized { end_ms } else { ts.clamp(start_ms, end_ms) });
                                    if was_finalized || flow_ready_at.is_some_and(|ready| ready <= start_ms) {
                                        sender.send(MarketEvent::Flow { bar: FlowBar { time:bucket, flow:flow::missing(bucket, state.through_ms) } }).await?;
                                    }
                                    sender.send(MarketEvent::FlowStatus { connected:true, message:format!("Unseen lower OKX raw trade ID {id} arrived for bucket {bucket}; numeric live coverage was invalidated pending complete replay.") }).await?;
                                    finalized.remove(&bucket);
                                    continue;
                                }
                                let state = live_buckets.entry(bucket).or_default();
                                state.native_volume = state.native_volume.checked_add(native).context("OKX native live volume overflow")?;
                                if buy { state.buy = state.buy.checked_add(size).context("OKX live buy overflow")?; }
                                else { state.sell = state.sell.checked_add(size).context("OKX live sell overflow")?; }
                                state.through_ms = state.through_ms.max(ts.clamp(start_ms, end_ms));
                                if !state.invalid && !finalized.contains(&bucket) && flow_ready_at.is_some_and(|ready| ready <= start_ms) {
                                    let flow_value = flow::snapshot(state.buy, state.sell, FlowQuality::Exact, bucket, state.through_ms)?;
                                    sender.send(MarketEvent::Flow { bar: FlowBar { time:bucket, flow:flow_value } }).await?;
                                }
                            }
                            continue;
                        }
                        if channel != candle_channel { continue; }
                        for row in rows {
                            let candle = candle_row(row)?;
                            finalized.retain(|time| *time >= candle.time.saturating_sub(market.interval.seconds().saturating_mul(2)));
                            live_buckets.retain(|time, _| *time >= candle.time.saturating_sub(market.interval.seconds().saturating_mul(2)));
                            super::validate(candle, market.interval)?;
                            sender.send(MarketEvent::Candle { candle }).await?;
                            let start_ms = candle.time.saturating_mul(1000);
                            let end_ms = candle.time.saturating_add(market.interval.seconds()).saturating_mul(1000);
                            let closed = row.as_array().and_then(|cols| cols.get(8)).and_then(Value::as_str) == Some("1");
                            if cvd && !closed && !finalized.contains(&candle.time) && flow_ready_at.is_some_and(|ready| ready <= start_ms) {
                                let state = live_buckets.entry(candle.time).or_default();
                                state.through_ms = state.through_ms.max(flow::unix_millis().clamp(start_ms, end_ms));
                                if !state.invalid {
                                    let flow_value = flow::snapshot(state.buy, state.sell, FlowQuality::Exact, candle.time, state.through_ms)?;
                                    sender.send(MarketEvent::Flow { bar: FlowBar { time:candle.time, flow:flow_value } }).await?;
                                }
                            }
                            if cvd && closed && !finalized.contains(&candle.time) {
                                let live_complete = flow_ready_at.is_some_and(|ready| ready <= start_ms);
                                let mut exact = None;
                                if live_complete {
                                    let state = live_buckets.entry(candle.time).or_default();
                                    if !state.invalid && native_volume_matches(state.native_volume, &candle) {
                                        exact = Some(flow::snapshot(state.buy, state.sell, FlowQuality::Exact, candle.time, end_ms)?);
                                    }
                                }
                                let flow_value = if let Some(snapshot) = exact {
                                    finalized.insert(candle.time);
                                    live_buckets.remove(&candle.time);
                                    snapshot
                                } else {
                                    let replay = tokio::select! {
                                        changed = shutdown.changed() => { let _ = changed; return Ok(()); }
                                        result = replay_bucket(client, instrument, &candle, market.interval, &quantities) => result,
                                    };
                                    match replay {
                                        Ok(Some(totals)) => {
                                            if native_volume_matches(totals.native_volume, &candle) {
                                                finalized.insert(candle.time);
                                                live_buckets.remove(&candle.time);
                                                flow::snapshot(totals.buy, totals.sell, FlowQuality::Exact, candle.time, end_ms)?
                                            } else { flow::missing(candle.time, end_ms) }
                                        }
                                        Ok(None) => flow::missing(candle.time, end_ms),
                                        Err(error) => {
                                            sender.send(MarketEvent::FlowStatus { connected:true, message:format!("OKX closed bucket {} awaits complete REST/raw-ID reconciliation: {error}", candle.time) }).await?;
                                            flow::missing(candle.time, end_ms)
                                        }
                                    }
                                };
                                sender.send(MarketEvent::Flow { bar: FlowBar { time:candle.time, flow:flow_value } }).await?;
                            }
                        }
                    }
                    Ok(Some(Ok(Message::Ping(payload)))) => write.send(Message::Pong(payload)).await?,
                    Ok(Some(Ok(Message::Close(frame)))) => bail!("OKX WebSocket closed: {frame:?}"),
                    Ok(Some(Ok(_))) => {}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spot_fixture() -> Instrument {
        Instrument {
            exchange: Exchange::Okx,
            product: Product::Spot,
            symbol: "BTC-USDT".into(),
            base_coin: "BTC".into(),
            quote_coin: Quote::parse("USDT").unwrap(),
            settle_coin: "USDT".into(),
            price_scale: 1,
            contract_size: 1.0,
            inverse: false,
            volume_unit: "BTC".into(),
        }
    }

    fn swap_fixture(inverse: bool) -> Instrument {
        let mut instrument = spot_fixture();
        instrument.product = Product::Perpetual;
        instrument.symbol = if inverse {
            "BTC-USD-SWAP"
        } else {
            "BTC-USDT-SWAP"
        }
        .into();
        instrument.quote_coin = Quote::parse(if inverse { "USD" } else { "USDT" }).unwrap();
        instrument.settle_coin = if inverse { "BTC" } else { "USDT" }.into();
        instrument.contract_size = if inverse { 100.0 } else { 0.01 };
        instrument.inverse = inverse;
        instrument.volume_unit = "contracts".into();
        instrument
    }

    #[test]
    fn spot_modes_preserve_base_sizes_without_execution_prices() {
        let instrument = spot_fixture();
        let native = flow::amount("0.123456789012345678").unwrap();
        for quantity in [QuantityMode::Native, QuantityMode::Base] {
            let quantities = FlowQuantities::new(&instrument, quantity).unwrap();
            assert_eq!(quantities.execution(native, None).unwrap(), native);
        }
    }

    #[test]
    fn linear_base_scales_contract_size_but_reconciles_native_volume() {
        let instrument = swap_fixture(false);
        let candle = candle_row(&json!(["120000", "9", "11", "8", "10", "5"])).unwrap();
        let row = json!({"sz":"5"});
        let native = FlowQuantities::new(&instrument, QuantityMode::Native).unwrap();
        let base = FlowQuantities::new(&instrument, QuantityMode::Base).unwrap();
        assert_eq!(
            execution_size(&row, &native).unwrap(),
            (Decimal::from(5), Decimal::from(5))
        );
        let (size, converted) = execution_size(&row, &base).unwrap();
        assert_eq!(converted, flow::amount("0.05").unwrap());
        assert!(native_volume_matches(size, &candle));
        assert!(!native_volume_matches(converted, &candle));
        assert_eq!(
            (
                candle.open,
                candle.high,
                candle.low,
                candle.close,
                candle.volume
            ),
            (9.0, 11.0, 8.0, 10.0, 5.0)
        );
    }

    #[test]
    fn nonzero_base_execution_underflow_fails_closed_but_zero_is_valid() {
        let smallest = flow::amount("0.0000000000000000000000000001").unwrap();
        let linear = swap_fixture(false);
        let linear_base = FlowQuantities::new(&linear, QuantityMode::Base).unwrap();
        assert!(linear_base.execution(smallest, None).is_err());
        assert_eq!(
            linear_base.execution(Decimal::ZERO, None).unwrap(),
            Decimal::ZERO
        );
        let inverse = swap_fixture(true);
        let inverse_base = FlowQuantities::new(&inverse, QuantityMode::Base).unwrap();
        assert!(inverse_base.execution(smallest, Some("100000")).is_err());
        assert_eq!(
            inverse_base
                .execution(Decimal::ZERO, Some("100000"))
                .unwrap(),
            Decimal::ZERO
        );
        for instrument in [&linear, &inverse] {
            let native = FlowQuantities::new(instrument, QuantityMode::Native).unwrap();
            assert_eq!(native.execution(smallest, None).unwrap(), smallest);
        }
    }

    #[test]
    fn inverse_execution_prices_determine_base_delta_and_volume_not_ohlc() {
        let instrument = swap_fixture(true);
        let candle = candle_row(&json!(["120000", "1", "1", "1", "1", "5"])).unwrap();
        for (quantity, expected_buy, expected_sell) in [
            (QuantityMode::Native, "2", "3"),
            (QuantityMode::Base, "0.02", "0.015"),
        ] {
            let quantities = FlowQuantities::new(&instrument, quantity).unwrap();
            let mut totals = TradeTotals::default();
            for (row, buy) in [
                (json!({"sz":"2", "px":"10000"}), true),
                (json!({"sz":"3", "px":"20000"}), false),
            ] {
                let (native, converted) = execution_size(&row, &quantities).unwrap();
                totals.add(native, converted, buy).unwrap();
            }
            assert_eq!(totals.buy, flow::amount(expected_buy).unwrap());
            assert_eq!(totals.sell, flow::amount(expected_sell).unwrap());
            assert_eq!(totals.native_volume, Decimal::from(5));
            assert!(native_volume_matches(totals.native_volume, &candle));
            assert!(!native_volume_matches(
                totals.native_volume,
                &Candle {
                    volume: 4.0,
                    ..candle
                }
            ));
            let snapshot = flow::snapshot(
                totals.buy,
                totals.sell,
                FlowQuality::Exact,
                candle.time,
                180_000,
            )
            .unwrap();
            let (delta, volume) = if quantity == QuantityMode::Base {
                (0.005, 0.035)
            } else {
                (-1.0, 5.0)
            };
            assert_eq!(snapshot.delta, Some(delta));
            assert_eq!(snapshot.volume, Some(volume));
            assert_eq!(snapshot.quality, FlowQuality::Exact);
            assert_eq!(snapshot.covered_from, 120);
            assert_eq!(snapshot.through_ms, 180_000);
        }
    }

    #[test]
    fn inverse_archive_converts_each_execution_and_deduplicates_native_totals() {
        let instrument = swap_fixture(true);
        let csv = b"instrument_name,trade_id,side,price,size,created_time\n\
                    BTC-USD-SWAP,1,buy,10000,2,120001\n\
                    BTC-USD-SWAP,2,sell,20000,3,120002\n\
                    BTC-USD-SWAP,1,buy,10000,2,120003\n";
        let quantities = FlowQuantities::new(&instrument, QuantityMode::Base).unwrap();
        let mut bars = BTreeMap::from([(120, TradeTotals::default())]);
        add_archive_records(
            csv.as_slice(),
            &instrument,
            &mut bars,
            Timeframe::Min1,
            &quantities,
        )
        .unwrap();
        let totals = &bars[&120];
        assert_eq!(totals.buy, flow::amount("0.02").unwrap());
        assert_eq!(totals.sell, flow::amount("0.015").unwrap());
        assert_eq!(totals.native_volume, Decimal::from(5));
        assert_eq!(totals.ids.len(), 2);
        let candle = candle_row(&json!(["120000", "1", "1", "1", "1", "5"])).unwrap();
        assert!(native_volume_matches(totals.native_volume, &candle));
        assert!(!native_volume_matches(totals.buy + totals.sell, &candle));
    }

    #[test]
    fn inverse_base_requires_positive_actual_price_native_does_not() {
        let instrument = swap_fixture(true);
        let base = FlowQuantities::new(&instrument, QuantityMode::Base).unwrap();
        let native = FlowQuantities::new(&instrument, QuantityMode::Native).unwrap();
        for row in [
            json!({"sz":"2"}),
            json!({"sz":"2", "px":"0"}),
            json!({"sz":"2", "px":"-10000"}),
        ] {
            assert!(execution_size(&row, &base).is_err());
            assert_eq!(execution_size(&row, &native).unwrap().1, Decimal::from(2));
        }
        let csv = b"instrument_name,trade_id,side,size,created_time\nBTC-USD-SWAP,1,buy,2,120001\n";
        let mut bars = BTreeMap::from([(120, TradeTotals::default())]);
        assert!(
            add_archive_records(
                csv.as_slice(),
                &instrument,
                &mut bars,
                Timeframe::Min1,
                &base
            )
            .is_err()
        );
        add_archive_records(
            csv.as_slice(),
            &instrument,
            &mut bars,
            Timeframe::Min1,
            &native,
        )
        .unwrap();
        assert_eq!(bars[&120].buy, Decimal::from(2));
    }

    #[test]
    fn native_archive_rows_use_taker_side_base_size_and_deduplicate_trade_ids() {
        let instrument = spot_fixture();
        let candle_time = 1_700_000_000 - 1_700_000_000 % 60;
        let csv = format!(
            "instrument_name,trade_id,side,price,size,created_time,source\n\
             BTC-USDT,1,buy,1,0.25,{},0\n\
             BTC-USDT,2,sell,1,1.5,{},1\n\
             BTC-USDT,1,buy,1,0.25,{},0\n\
             BTC-USD,3,buy,1,99,{},0\n",
            candle_time * 1000 + 10,
            candle_time * 1000 + 20,
            candle_time * 1000 + 30,
            candle_time * 1000 + 40,
        );
        let mut bars = BTreeMap::from([(candle_time, TradeTotals::default())]);
        let quantities = FlowQuantities::new(&instrument, QuantityMode::Native).unwrap();
        add_archive_records(
            csv.as_bytes(),
            &instrument,
            &mut bars,
            Timeframe::Min1,
            &quantities,
        )
        .unwrap();
        let totals = &bars[&candle_time];
        assert_eq!(totals.buy, flow::amount("0.25").unwrap());
        assert_eq!(totals.sell, flow::amount("1.5").unwrap());
        assert_eq!(totals.ids.len(), 2);
    }

    #[test]
    fn contract_taker_array_is_sell_then_buy_and_unit_is_preserved() {
        let row = json!(["1791375300000", "13882", "8732.41"]);
        let (time, buy, sell) = contract_taker_row(&row).unwrap();
        assert_eq!(time, 1_791_375_300);
        assert_eq!(buy, flow::amount("8732.41").unwrap());
        assert_eq!(sell, flow::amount("13882").unwrap());
    }
    #[test]
    fn bounded_live_id_cache_distinguishes_duplicates_from_unseen_lower_ids() {
        let mut highwater = None;
        let mut recent = HashSet::new();
        let mut order = VecDeque::new();
        assert_eq!(
            remember_live_id(20, &mut highwater, &mut recent, &mut order),
            LiveIdResult::New
        );
        assert_eq!(
            remember_live_id(20, &mut highwater, &mut recent, &mut order),
            LiveIdResult::Duplicate
        );
        assert_eq!(
            remember_live_id(19, &mut highwater, &mut recent, &mut order),
            LiveIdResult::UnseenLower
        );
        assert_eq!(recent.len(), 2);
    }
}
