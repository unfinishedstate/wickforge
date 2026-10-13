use std::{
    collections::{HashSet, VecDeque},
    fmt,
    time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
use futures_util::{SinkExt, StreamExt};
use mexc_model::{
    Candle, Catalog, Exchange, FlowBar, FlowQuality, HistoryEvent, HistoryPage, Instrument, Market,
    MarketEvent, Product, QuantityMode, Quote, Timeframe,
};
use prost::Message as ProstMessage;
use rust_decimal::Decimal as NativeDecimal;
use rust_decimal::prelude::FromPrimitive;
use serde::{
    Deserialize, Deserializer,
    de::{self, Visitor},
};
use serde_json::{Value, json};
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::{connect_async, tungstenite::Message};

use super::{flow, get_json, history_end, history_page, integer, number, unix_seconds, validate};

const SPOT_REST: &str = "https://api.mexc.com/api/v3";
const FUTURES_REST: &str = "https://api.mexc.com/api/v1/contract";
const FUTURES_WS: &str = "wss://contract.mexc.com/edge";
const SPOT_INTERVALS: [Timeframe; 7] = [
    Timeframe::Min1,
    Timeframe::Min5,
    Timeframe::Min15,
    Timeframe::Min30,
    Timeframe::Min60,
    Timeframe::Hour4,
    Timeframe::Day1,
];
const FUTURES_INTERVALS: [Timeframe; 8] = [
    Timeframe::Min1,
    Timeframe::Min5,
    Timeframe::Min15,
    Timeframe::Min30,
    Timeframe::Min60,
    Timeframe::Hour4,
    Timeframe::Hour8,
    Timeframe::Day1,
];

// MEXC returns decimals as either JSON numbers or strings on its futures APIs.
#[derive(Debug, Clone, Copy)]
struct Decimal(f64);
impl<'de> Deserialize<'de> for Decimal {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct NumberVisitor;
        impl<'value> Visitor<'value> for NumberVisitor {
            type Value = Decimal;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a finite decimal number or numeric string")
            }
            fn visit_f64<E: de::Error>(self, value: f64) -> Result<Decimal, E> {
                if value.is_finite() {
                    Ok(Decimal(value))
                } else {
                    Err(E::custom("non-finite decimal"))
                }
            }
            fn visit_i64<E: de::Error>(self, value: i64) -> Result<Decimal, E> {
                self.visit_f64(value as f64)
            }
            fn visit_u64<E: de::Error>(self, value: u64) -> Result<Decimal, E> {
                self.visit_f64(value as f64)
            }
            fn visit_str<E: de::Error>(self, value: &str) -> Result<Decimal, E> {
                self.visit_f64(value.parse().map_err(E::custom)?)
            }
            fn visit_map<A: de::MapAccess<'value>>(self, map: A) -> Result<Decimal, A::Error> {
                let number =
                    serde_json::Number::deserialize(de::value::MapAccessDeserializer::new(map))?;
                self.visit_f64(
                    number
                        .as_f64()
                        .ok_or_else(|| de::Error::custom("unrepresentable decimal"))?,
                )
            }
        }
        deserializer.deserialize_any(NumberVisitor)
    }
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Timestamp {
    Integer(i64),
    String(String),
}
impl Timestamp {
    fn milliseconds(self) -> Result<i64> {
        let value = match self {
            Self::Integer(v) => v,
            Self::String(v) => v.parse().context("invalid timestamp")?,
        };
        Ok(if value >= 100_000_000_000 {
            value
        } else {
            value.saturating_mul(1000)
        })
    }
    fn seconds(self) -> Result<i64> {
        Ok(self.milliseconds()? / 1000)
    }
}

#[derive(Deserialize)]
struct FuturesCatalogResponse {
    success: bool,
    code: Value,
    #[serde(default, alias = "msg")]
    message: Option<String>,
    data: Option<Vec<FuturesInstrument>>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FuturesInstrument {
    symbol: String,
    base_coin: String,
    quote_coin: String,
    settle_coin: String,
    price_scale: u32,
    contract_size: Decimal,
    state: i64,
    #[serde(default)]
    future_type: i64,
}
#[derive(Deserialize)]
struct SpotCatalogResponse {
    symbols: Vec<SpotInstrument>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SpotInstrument {
    symbol: String,
    base_asset: String,
    quote_asset: String,
    #[serde(default)]
    quote_asset_precision: u32,
    #[serde(default)]
    status: String,
    #[serde(default)]
    is_spot_trading_allowed: bool,
}

fn intervals(product: Product) -> Vec<Timeframe> {
    match product {
        Product::Spot => SPOT_INTERVALS.to_vec(),
        Product::Perpetual => FUTURES_INTERVALS.to_vec(),
    }
}

pub async fn catalog(client: &reqwest::Client, product: Product) -> Result<Catalog> {
    let instruments = match product {
        Product::Spot => {
            let response: SpotCatalogResponse = get_json(
                client,
                &format!("{SPOT_REST}/exchangeInfo"),
                &[("permissions", "SPOT".to_owned())],
            )
            .await
            .context("fetching MEXC spot exchange info")?;
            let mut instruments = Vec::new();
            for item in response.symbols {
                let Some(quote_coin) = Quote::parse(&item.quote_asset) else {
                    continue;
                };
                if !item.is_spot_trading_allowed || item.status != "1" {
                    continue;
                }
                ensure!(
                    !item.symbol.is_empty() && !item.base_asset.is_empty(),
                    "invalid MEXC spot symbol metadata"
                );
                instruments.push(Instrument {
                    exchange: Exchange::Mexc,
                    product,
                    symbol: item.symbol,
                    base_coin: item.base_asset.clone(),
                    quote_coin,
                    settle_coin: item.quote_asset.clone(),
                    price_scale: item.quote_asset_precision,
                    contract_size: 1.0,
                    inverse: false,
                    volume_unit: item.base_asset,
                });
            }
            instruments
        }
        Product::Perpetual => {
            let response: FuturesCatalogResponse =
                get_json(client, &format!("{FUTURES_REST}/detail/country"), &[])
                    .await
                    .context("fetching MEXC perpetual contracts")?;
            ensure!(
                response.success && (response.code == 0 || response.code == "0"),
                "MEXC contracts error {}: {}",
                response.code,
                response.message.as_deref().unwrap_or("request rejected")
            );
            let mut instruments = Vec::new();
            for item in response
                .data
                .context("MEXC contracts response has no data")?
            {
                let Some(quote_coin) = Quote::parse(&item.quote_coin) else {
                    continue;
                };
                if item.state != 0 || item.future_type != 1 {
                    continue;
                }
                ensure!(
                    !item.symbol.is_empty()
                        && !item.base_coin.is_empty()
                        && !item.settle_coin.is_empty()
                        && item.contract_size.0 > 0.0,
                    "invalid MEXC contract metadata for {}",
                    item.symbol
                );
                let inverse = item.settle_coin == item.base_coin;
                instruments.push(Instrument {
                    exchange: Exchange::Mexc,
                    product,
                    symbol: item.symbol,
                    base_coin: item.base_coin,
                    quote_coin,
                    settle_coin: item.settle_coin,
                    price_scale: item.price_scale,
                    contract_size: item.contract_size.0,
                    inverse,
                    volume_unit: "contracts".to_owned(),
                });
            }
            instruments
        }
    };
    let mut instruments = instruments;
    instruments.sort_unstable_by(|a, b| a.symbol.cmp(&b.symbol));
    instruments.dedup_by(|a, b| a.symbol == b.symbol);
    Ok(Catalog {
        exchange: Exchange::Mexc,
        product,
        intervals: intervals(product),
        instruments,
        flow_support: super::flow_support(Exchange::Mexc, product),
    })
}

fn spot_interval(interval: Timeframe) -> Result<&'static str> {
    Ok(match interval {
        Timeframe::Min1 => "1m",
        Timeframe::Min5 => "5m",
        Timeframe::Min15 => "15m",
        Timeframe::Min30 => "30m",
        Timeframe::Min60 => "60m",
        Timeframe::Hour4 => "4h",
        Timeframe::Day1 => "1d",
        Timeframe::Hour6 | Timeframe::Hour8 => {
            bail!("MEXC spot REST does not natively support this candle interval")
        }
    })
}
fn futures_interval(interval: Timeframe) -> Result<&'static str> {
    Ok(match interval {
        Timeframe::Min1 => "Min1",
        Timeframe::Min5 => "Min5",
        Timeframe::Min15 => "Min15",
        Timeframe::Min30 => "Min30",
        Timeframe::Min60 => "Min60",
        Timeframe::Hour4 => "Hour4",
        Timeframe::Hour8 => "Hour8",
        Timeframe::Day1 => "Day1",
        Timeframe::Hour6 => bail!("MEXC futures does not natively support 6h candles"),
    })
}
fn check_market(instrument: &Instrument, market: &Market) -> Result<()> {
    ensure!(
        instrument.exchange == Exchange::Mexc
            && market.exchange == Exchange::Mexc
            && instrument.product == market.product
            && instrument.symbol == market.symbol,
        "MEXC market and instrument do not match"
    );
    Ok(())
}

#[derive(Deserialize)]
struct FuturesHistoryResponse {
    success: bool,
    code: Value,
    #[serde(default, alias = "msg")]
    message: Option<String>,
    data: Option<FuturesColumns>,
}
#[derive(Deserialize, Default)]
#[serde(default)]
struct FuturesColumns {
    time: Vec<Timestamp>,
    open: Vec<Decimal>,
    high: Vec<Decimal>,
    low: Vec<Decimal>,
    close: Vec<Decimal>,
    vol: Vec<Decimal>,
}

pub async fn history(
    client: &reqwest::Client,
    instrument: &Instrument,
    market: &Market,
    before: Option<i64>,
) -> Result<HistoryPage> {
    check_market(instrument, market)?;
    let end = history_end(before, unix_seconds(), market.interval)?;
    let candles = match market.product {
        Product::Perpetual => {
            let interval = futures_interval(market.interval)?;
            let response: FuturesHistoryResponse = get_json(
                client,
                &format!("{FUTURES_REST}/kline/{}", instrument.symbol),
                &[
                    ("interval", interval.to_owned()),
                    (
                        "start",
                        (end - market.interval.seconds() * 2000).max(0).to_string(),
                    ),
                    ("end", end.to_string()),
                ],
            )
            .await
            .context("fetching MEXC perpetual history")?;
            ensure!(
                response.success && (response.code == 0 || response.code == "0"),
                "MEXC history error {}: {}",
                response.code,
                response.message.as_deref().unwrap_or("request rejected")
            );
            let cols = response.data.context("MEXC history response has no data")?;
            let n = cols.time.len();
            ensure!(
                [
                    cols.open.len(),
                    cols.high.len(),
                    cols.low.len(),
                    cols.close.len(),
                    cols.vol.len()
                ]
                .into_iter()
                .all(|x| x == n),
                "MEXC history column lengths differ"
            );
            let mut candles = Vec::with_capacity(n);
            for (i, time) in cols.time.into_iter().enumerate() {
                candles.push(Candle {
                    time: time.seconds()?,
                    open: cols.open[i].0,
                    high: cols.high[i].0,
                    low: cols.low[i].0,
                    close: cols.close[i].0,
                    volume: cols.vol[i].0,
                    flow: None,
                });
            }
            candles
        }
        Product::Spot => {
            let interval = spot_interval(market.interval)?;
            let rows: Vec<Vec<Value>> = get_json(
                client,
                &format!("{SPOT_REST}/klines"),
                &[
                    ("symbol", instrument.symbol.clone()),
                    ("interval", interval.to_owned()),
                    (
                        "startTime",
                        (end.saturating_sub(market.interval.seconds() * 500).max(0) * 1000)
                            .to_string(),
                    ),
                    ("endTime", (end * 1000).to_string()),
                    ("limit", "500".to_owned()),
                ],
            )
            .await
            .context("fetching MEXC spot history")?;
            rows.into_iter()
                .map(|row| {
                    ensure!(row.len() >= 6, "short MEXC spot kline row");
                    Ok(Candle {
                        time: integer(&row[0])? / 1000,
                        open: number(&row[1])?,
                        high: number(&row[2])?,
                        low: number(&row[3])?,
                        close: number(&row[4])?,
                        volume: number(&row[5])?,
                        flow: None,
                    })
                })
                .collect::<Result<Vec<_>>>()?
        }
    };
    history_page(candles, end, market.clone())
}

fn add_spot_maker_side(
    buy: &mut NativeDecimal,
    sell: &mut NativeDecimal,
    buyer_maker: bool,
    qty: NativeDecimal,
) -> Result<()> {
    if buyer_maker {
        *sell = sell
            .checked_add(qty)
            .context("MEXC spot sell volume overflow")?;
    } else {
        *buy = buy
            .checked_add(qty)
            .context("MEXC spot buy volume overflow")?;
    }
    Ok(())
}

fn add_reported_side(
    buy: &mut NativeDecimal,
    sell: &mut NativeDecimal,
    side: i64,
    qty: NativeDecimal,
) -> Result<bool> {
    match side {
        1 => {
            *buy = buy
                .checked_add(qty)
                .context("MEXC reported buy volume overflow")?
        }
        2 => {
            *sell = sell
                .checked_add(qty)
                .context("MEXC reported sell volume overflow")?
        }
        _ => return Ok(false),
    }
    Ok(true)
}

#[derive(Deserialize)]
struct SpotAggTrade {
    #[serde(default)]
    q: Value,
    #[serde(default, alias = "qty")]
    quantity: Value,
    #[serde(default)]
    m: Option<bool>,
    #[serde(default, alias = "T")]
    time: Option<i64>,
}

fn split_agg_range(start: i64, end: i64) -> Option<((i64, i64), (i64, i64))> {
    if end.saturating_sub(start) < 1000 {
        return None;
    }
    let mid = start + (end - start) / 2;
    Some(((start, mid), (mid + 1, end)))
}

fn accumulate_agg_trades(
    rows: impl IntoIterator<Item = Value>,
    start: i64,
    end: i64,
    buy: &mut NativeDecimal,
    sell: &mut NativeDecimal,
) -> Result<()> {
    for row in rows {
        let trade: SpotAggTrade =
            serde_json::from_value(row).context("invalid MEXC aggregate-trade record")?;
        let ts = trade.time.context("MEXC aggregate trade has no time")?;
        if ts < start || ts > end {
            continue;
        }
        let qty = json_decimal(if trade.q.is_null() {
            &trade.quantity
        } else {
            &trade.q
        })?;
        add_spot_maker_side(
            buy,
            sell,
            trade
                .m
                .context("MEXC aggregate trade has no buyer-maker flag")?,
            qty,
        )?;
    }
    Ok(())
}

fn json_decimal(value: &Value) -> Result<NativeDecimal> {
    match value {
        Value::String(text) => flow::amount(text),
        Value::Number(number) => flow::amount(&number.to_string()),
        _ => bail!("invalid MEXC native decimal"),
    }
}

async fn spot_agg_range(
    client: &reqwest::Client,
    symbol: &str,
    start_ms: i64,
    end_ms: i64,
    sender: &mpsc::Sender<HistoryEvent>,
) -> Result<(NativeDecimal, NativeDecimal, bool)> {
    let mut pending = vec![(start_ms, end_ms)];
    let mut buy = NativeDecimal::ZERO;
    let mut sell = NativeDecimal::ZERO;
    let mut complete = true;
    while let Some((start, end)) = pending.pop() {
        if sender.is_closed() {
            bail!("history consumer closed");
        }
        let rows: Vec<Value> = get_json(
            client,
            &format!("{SPOT_REST}/aggTrades"),
            &[
                ("symbol", symbol.to_owned()),
                ("startTime", start.to_string()),
                ("endTime", end.to_string()),
                ("limit", "1000".to_owned()),
            ],
        )
        .await
        .context("fetching MEXC spot aggregate trades")?;
        if rows.len() >= 1000 {
            // No usable trade cursor exists once a single second-resolution
            // timestamp bucket is saturated.
            let Some((left, right)) = split_agg_range(start, end) else {
                complete = false;
                continue;
            };
            pending.push(right);
            pending.push(left);
            continue;
        }
        accumulate_agg_trades(rows, start, end, &mut buy, &mut sell)?;
    }
    Ok((buy, sell, complete))
}

pub async fn flow_history(
    client: &reqwest::Client,
    instrument: &Instrument,
    market: &Market,
    candles: &[Candle],
    sender: &mpsc::Sender<HistoryEvent>,
) -> Result<()> {
    check_market(instrument, market)?;
    if market.product == Product::Perpetual {
        for candle in candles {
            sender
                .send(HistoryEvent::Flow {
                    bar: FlowBar {
                        time: candle.time,
                        flow: flow::missing(
                            candle.time,
                            candle
                                .time
                                .saturating_add(market.interval.seconds())
                                .saturating_mul(1000),
                        ),
                    },
                })
                .await
                .context("history consumer closed")?;
        }
        sender.send(HistoryEvent::FlowStatus { message: "MEXC perpetual trade history is unavailable; no historical CVD is inferred from candles".into() }).await.context("history consumer closed")?;
        return Ok(());
    }
    sender.send(HistoryEvent::FlowStatus { message: "MEXC spot aggTrades has no usable ID cursor and shallow retention; saturated timestamp buckets or candle-volume mismatches are reported missing".into() }).await.context("history consumer closed")?;
    for candle in candles {
        if sender.is_closed() {
            bail!("history consumer closed");
        }
        let open_ms = candle.time.saturating_mul(1000);
        let end_ms = open_ms.saturating_add(market.interval.seconds().saturating_mul(1000));
        if end_ms > flow::unix_millis() {
            sender
                .send(HistoryEvent::Flow {
                    bar: FlowBar {
                        time: candle.time,
                        flow: flow::missing(candle.time, end_ms),
                    },
                })
                .await
                .context("history consumer closed")?;
            continue;
        }
        let mut buy = NativeDecimal::ZERO;
        let mut sell = NativeDecimal::ZERO;
        let mut complete = true;
        let mut start = open_ms;
        while start < end_ms {
            let end = (start + 3_600_000 - 1).min(end_ms - 1);
            let (b, s, c) = spot_agg_range(client, &instrument.symbol, start, end, sender).await?;
            buy = buy.checked_add(b).context("MEXC buy volume overflow")?;
            sell = sell.checked_add(s).context("MEXC sell volume overflow")?;
            complete &= c;
            start = end + 1;
        }
        let candle_volume =
            NativeDecimal::from_f64(candle.volume).context("unrepresentable MEXC candle volume")?;
        let observed = buy.checked_add(sell).context("MEXC volume overflow")?;
        complete &= observed == candle_volume;
        let snapshot = if complete {
            flow::snapshot(buy, sell, FlowQuality::Exact, candle.time, end_ms)?
        } else {
            flow::missing(candle.time, end_ms)
        };
        sender
            .send(HistoryEvent::Flow {
                bar: FlowBar {
                    time: candle.time,
                    flow: snapshot,
                },
            })
            .await
            .context("history consumer closed")?;
    }
    Ok(())
}

#[derive(Deserialize)]
struct StreamCandle {
    t: Timestamp,
    o: Decimal,
    h: Decimal,
    l: Decimal,
    c: Decimal,
    q: Decimal,
    #[serde(default)]
    symbol: Option<String>,
    interval: String,
}
#[derive(Deserialize)]
#[serde(tag = "channel")]
enum WireEvent {
    #[serde(rename = "push.kline")]
    Candle {
        data: StreamCandle,
        #[serde(default)]
        symbol: Option<String>,
    },
    #[serde(rename = "rs.sub.kline")]
    Subscription { data: Value },
    #[serde(rename = "pong")]
    Pong,
    #[serde(rename = "rs.error")]
    Error { data: Value },
    #[serde(other)]
    Other,
}
enum Update {
    Candle(Candle),
    Subscribed,
    Pong,
    Ignore,
}
fn parse_frame(bytes: &[u8], market: &Market) -> Result<Update> {
    match serde_json::from_slice(bytes).context("invalid MEXC websocket JSON")? {
        WireEvent::Candle { data, symbol } => {
            if data.symbol.as_deref().or(symbol.as_deref()) != Some(market.symbol.as_str())
                || data.symbol.as_ref().is_some_and(|s| s != &market.symbol)
                || symbol.as_ref().is_some_and(|s| s != &market.symbol)
                || data.interval != futures_interval(market.interval)?
            {
                return Ok(Update::Ignore);
            }
            let candle = Candle {
                time: data.t.seconds()?,
                open: data.o.0,
                high: data.h.0,
                low: data.l.0,
                close: data.c.0,
                volume: data.q.0,
                flow: None,
            };
            validate(candle, market.interval)?;
            Ok(Update::Candle(candle))
        }
        WireEvent::Subscription { data } => {
            ensure!(data == "success", "MEXC subscription rejected: {data}");
            Ok(Update::Subscribed)
        }
        WireEvent::Pong => Ok(Update::Pong),
        WireEvent::Error { data } => bail!("MEXC websocket error: {data}"),
        WireEvent::Other => Ok(Update::Ignore),
    }
}
async fn send(sender: &mpsc::Sender<MarketEvent>, event: MarketEvent) -> Result<()> {
    sender.send(event).await.context("stream consumer closed")
}

#[derive(Clone, PartialEq, ProstMessage)]
struct PbWrapper {
    #[prost(string, tag = "1")]
    channel: String,
    #[prost(
        oneof = "pb_wrapper::Body",
        tags = "301, 302, 303, 304, 305, 306, 307, 308, 309, 310, 311, 312, 313, 314, 315"
    )]
    body: Option<pb_wrapper::Body>,
    #[prost(string, optional, tag = "3")]
    symbol: Option<String>,
}
mod pb_wrapper {
    use super::PbDeals;
    use prost::Oneof;
    #[derive(Clone, PartialEq, Oneof)]
    pub enum Body {
        #[prost(message, tag = "314")]
        AggDeals(PbDeals),
    }
}
#[derive(Clone, PartialEq, ProstMessage)]
pub struct PbDeals {
    #[prost(message, repeated, tag = "1")]
    deals: Vec<PbDeal>,
}
#[derive(Clone, PartialEq, ProstMessage)]
struct PbDeal {
    #[prost(string, tag = "1")]
    price: String,
    #[prost(string, tag = "2")]
    quantity: String,
    #[prost(int32, tag = "3")]
    trade_type: i32,
    #[prost(int64, tag = "4")]
    time: i64,
    #[prost(string, tag = "5")]
    trade_id: String,
}

pub async fn stream(
    client: &reqwest::Client,
    instrument: &Instrument,
    market: &Market,
    cvd: bool,
    sender: &mpsc::Sender<MarketEvent>,
    mut shutdown: watch::Receiver<bool>,
    quantity: QuantityMode,
) -> Result<()> {
    check_market(instrument, market)?;
    match market.product {
        Product::Perpetual => {
            stream_futures(
                client,
                instrument,
                market,
                cvd,
                sender,
                &mut shutdown,
                quantity,
            )
            .await
        }
        Product::Spot => stream_spot(client, instrument, market, cvd, sender, &mut shutdown).await,
    }
}
const RECENT_TRADE_IDS: usize = 100;

#[derive(Default)]
struct RecentTradeIds {
    set: HashSet<String>,
    order: VecDeque<String>,
    last: Option<String>,
}

impl RecentTradeIds {
    fn insert(&mut self, id: &str) -> bool {
        if !self.set.insert(id.to_owned()) {
            return false;
        }
        self.last = Some(id.to_owned());
        self.order.push_back(id.to_owned());
        if self.order.len() > RECENT_TRADE_IDS
            && let Some(expired) = self.order.pop_front()
        {
            self.set.remove(&expired);
        }
        true
    }
    fn clear(&mut self) {
        self.set.clear();
        self.order.clear();
        self.last = None;
    }
}

#[derive(Default)]
struct FuturesCapture {
    eligible_from: Option<i64>,
    active_bar: Option<i64>,
    buy: NativeDecimal,
    sell: NativeDecimal,
    observed_from: Option<i64>,
    last_trade_ms: Option<i64>,
    valid: bool,
    base_factor: Option<NativeDecimal>,
}

impl FuturesCapture {
    fn snapshot(&self, from: i64, through_ms: i64) -> Result<mexc_model::VolumeFlow> {
        let (buy, sell) = if let Some(factor) = self.base_factor {
            let buy = self
                .buy
                .checked_mul(factor)
                .context("MEXC base buy quantity overflow")?;
            let sell = self
                .sell
                .checked_mul(factor)
                .context("MEXC base sell quantity overflow")?;
            ensure!(
                (self.buy == NativeDecimal::ZERO || buy > NativeDecimal::ZERO)
                    && (self.sell == NativeDecimal::ZERO || sell > NativeDecimal::ZERO),
                "MEXC base quantity underflow"
            );
            (buy, sell)
        } else {
            (self.buy, self.sell)
        };
        flow::snapshot(buy, sell, FlowQuality::Reported, from, through_ms)
    }

    async fn invalidate(&mut self, step: i64, sender: &mpsc::Sender<MarketEvent>) -> Result<()> {
        if let Some(bar) = self.active_bar {
            sender
                .send(MarketEvent::Flow {
                    bar: FlowBar {
                        time: bar,
                        flow: flow::missing(bar, bar.saturating_add(step).saturating_mul(1000)),
                    },
                })
                .await
                .context("stream consumer closed")?;
        }
        self.eligible_from = None;
        self.active_bar = None;
        self.buy = NativeDecimal::ZERO;
        self.sell = NativeDecimal::ZERO;
        self.observed_from = None;
        self.last_trade_ms = None;
        self.valid = false;
        Ok(())
    }
    async fn mark_active_missing(
        &mut self,
        step: i64,
        sender: &mpsc::Sender<MarketEvent>,
    ) -> Result<()> {
        if let Some(bar) = self.active_bar {
            self.valid = false;
            self.observed_from = None;
            sender
                .send(MarketEvent::Flow {
                    bar: FlowBar {
                        time: bar,
                        flow: flow::missing(bar, bar.saturating_add(step).saturating_mul(1000)),
                    },
                })
                .await
                .context("stream consumer closed")?;
        }
        Ok(())
    }

    async fn ingest(
        &mut self,
        deal: &Value,
        has_id: bool,
        step: i64,
        sender: &mpsc::Sender<MarketEvent>,
    ) -> Result<()> {
        let Some(raw_timestamp) = deal.get("t") else {
            self.mark_active_missing(step, sender).await?;
            return Ok(());
        };
        let timestamp: Timestamp = match serde_json::from_value(raw_timestamp.clone()) {
            Ok(timestamp) => timestamp,
            Err(_) => {
                self.mark_active_missing(step, sender).await?;
                return Ok(());
            }
        };
        let event_ms = match timestamp.milliseconds() {
            Ok(timestamp) => timestamp,
            Err(_) => {
                self.mark_active_missing(step, sender).await?;
                return Ok(());
            }
        };
        let bucket = (event_ms / 1000).div_euclid(step) * step;
        if self.eligible_from.is_none() {
            self.eligible_from = Some(bucket.saturating_add(step));
            sender
                .send(MarketEvent::Flow {
                    bar: FlowBar {
                        time: bucket,
                        flow: flow::missing(
                            bucket,
                            bucket.saturating_add(step).saturating_mul(1000),
                        ),
                    },
                })
                .await
                .context("stream consumer closed")?;
            return Ok(());
        }
        let eligible = self.eligible_from.expect("capture boundary initialized");
        if bucket < eligible {
            return Ok(());
        }
        if self.active_bar.is_some_and(|active| bucket < active) {
            sender
                .send(MarketEvent::Flow {
                    bar: FlowBar {
                        time: bucket,
                        flow: flow::missing(
                            bucket,
                            bucket.saturating_add(step).saturating_mul(1000),
                        ),
                    },
                })
                .await
                .context("stream consumer closed")?;
            return Ok(());
        }
        if self.active_bar != Some(bucket) {
            if let (Some(previous), Some(from)) = (self.active_bar, self.observed_from) {
                let end = previous.saturating_add(step);
                let flow = if self.valid {
                    self.snapshot(from, end.saturating_mul(1000))?
                } else {
                    flow::missing(previous, end.saturating_mul(1000))
                };
                sender
                    .send(MarketEvent::Flow {
                        bar: FlowBar {
                            time: previous,
                            flow,
                        },
                    })
                    .await
                    .context("stream consumer closed")?;
                let mut gap = end;
                while gap < bucket {
                    sender
                        .send(MarketEvent::Flow {
                            bar: FlowBar {
                                time: gap,
                                flow: flow::missing(
                                    gap,
                                    gap.saturating_add(step).saturating_mul(1000),
                                ),
                            },
                        })
                        .await
                        .context("stream consumer closed")?;
                    gap = gap.saturating_add(step);
                }
            } else {
                let mut gap = self
                    .active_bar
                    .map(|bar| bar.saturating_add(step))
                    .unwrap_or(eligible);
                while gap < bucket {
                    sender
                        .send(MarketEvent::Flow {
                            bar: FlowBar {
                                time: gap,
                                flow: flow::missing(
                                    gap,
                                    gap.saturating_add(step).saturating_mul(1000),
                                ),
                            },
                        })
                        .await
                        .context("stream consumer closed")?;
                    gap = gap.saturating_add(step);
                }
            }
            self.active_bar = Some(bucket);
            self.buy = NativeDecimal::ZERO;
            self.sell = NativeDecimal::ZERO;
            self.observed_from = Some(bucket);
            self.last_trade_ms = None;
            self.valid = true;
        }
        if !has_id {
            self.valid = false;
            self.observed_from = None;
            sender
                .send(MarketEvent::Flow {
                    bar: FlowBar {
                        time: bucket,
                        flow: flow::missing(
                            bucket,
                            bucket.saturating_add(step).saturating_mul(1000),
                        ),
                    },
                })
                .await
                .context("stream consumer closed")?;
            return Ok(());
        }
        let qty = match deal.get("v").and_then(|value| json_decimal(value).ok()) {
            Some(qty) => qty,
            None => {
                self.valid = false;
                self.observed_from = None;
                sender
                    .send(MarketEvent::Flow {
                        bar: FlowBar {
                            time: bucket,
                            flow: flow::missing(
                                bucket,
                                bucket.saturating_add(step).saturating_mul(1000),
                            ),
                        },
                    })
                    .await
                    .context("stream consumer closed")?;
                return Ok(());
            }
        };
        let Some(side) = deal.get("T").and_then(Value::as_i64) else {
            self.valid = false;
            self.observed_from = None;
            sender
                .send(MarketEvent::Flow {
                    bar: FlowBar {
                        time: bucket,
                        flow: flow::missing(
                            bucket,
                            bucket.saturating_add(step).saturating_mul(1000),
                        ),
                    },
                })
                .await
                .context("stream consumer closed")?;
            return Ok(());
        };
        if matches!(
            add_reported_side(&mut self.buy, &mut self.sell, side, qty),
            Ok(true)
        ) {
            self.last_trade_ms = Some(
                self.last_trade_ms
                    .map_or(event_ms, |last| last.max(event_ms)),
            );
        } else {
            self.valid = false;
            self.observed_from = None;
            sender
                .send(MarketEvent::Flow {
                    bar: FlowBar {
                        time: bucket,
                        flow: flow::missing(
                            bucket,
                            bucket.saturating_add(step).saturating_mul(1000),
                        ),
                    },
                })
                .await
                .context("stream consumer closed")?;
        }
        Ok(())
    }

    async fn emit_current(&self, step: i64, sender: &mpsc::Sender<MarketEvent>) -> Result<()> {
        if let (Some(bar), Some(from), Some(through)) =
            (self.active_bar, self.observed_from, self.last_trade_ms)
        {
            let through = through
                .max(bar.saturating_mul(1000))
                .min(bar.saturating_add(step).saturating_mul(1000));
            let flow = if self.valid {
                self.snapshot(from, through)?
            } else {
                flow::missing(bar, through)
            };
            sender
                .send(MarketEvent::Flow {
                    bar: FlowBar { time: bar, flow },
                })
                .await
                .context("stream consumer closed")?;
        }
        Ok(())
    }
}
fn native_id(value: &Value) -> Option<String> {
    match value {
        Value::String(id) if !id.is_empty() => Some(id.clone()),
        Value::Number(id) => Some(id.to_string()),
        _ => None,
    }
}

async fn fetch_futures_recovery(
    client: &reqwest::Client,
    symbol: &str,
    recent_ids: &RecentTradeIds,
) -> Result<Option<Vec<Value>>> {
    let Some(marker) = recent_ids.last.as_deref() else {
        return Ok(None);
    };
    let response: Value = get_json(
        client,
        &format!("{FUTURES_REST}/deals/{symbol}"),
        &[("limit", "100".to_owned())],
    )
    .await
    .context("fetching MEXC futures recent trades for reconnect recovery")?;
    Ok(futures_recovery_rows(&response, marker))
}

fn futures_recovery_rows(response: &Value, marker: &str) -> Option<Vec<Value>> {
    if response.get("success").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    let rows = response.get("data").and_then(Value::as_array)?;
    let marker_at = rows
        .iter()
        .position(|row| row.get("i").and_then(native_id).as_deref() == Some(marker))?;
    let window = rows.get(..=marker_at)?;
    let mut previous_ms = None;
    for row in window {
        let raw_time = row.get("t")?;
        let timestamp = serde_json::from_value::<Timestamp>(raw_time.clone()).ok()?;
        let time_ms = timestamp.milliseconds().ok()?;
        if previous_ms.is_some_and(|newer| time_ms > newer) {
            return None;
        }
        previous_ms = Some(time_ms);
        row.get("i").and_then(native_id)?;
    }
    Some(window[..marker_at].iter().rev().cloned().collect())
}

async fn stream_futures(
    client: &reqwest::Client,
    instrument: &Instrument,
    market: &Market,
    cvd: bool,
    sender: &mpsc::Sender<MarketEvent>,
    shutdown: &mut watch::Receiver<bool>,
    quantity: QuantityMode,
) -> Result<()> {
    let mut recent_ids = RecentTradeIds::default();
    let mut capture = FuturesCapture {
        base_factor: if quantity == QuantityMode::Base {
            let factor = NativeDecimal::from_f64(crate::base_quantity_factor(instrument)?)
                .context("unrepresentable MEXC base contract size")?
                .normalize();
            ensure!(
                factor > NativeDecimal::ZERO,
                "MEXC base contract size underflow"
            );
            Some(factor)
        } else {
            None
        },
        ..FuturesCapture::default()
    };
    loop {
        if *shutdown.borrow() {
            return Ok(());
        }
        let mut connection_shutdown = shutdown.clone();
        let result = stream_futures_connection(
            client,
            market,
            cvd,
            sender,
            &mut connection_shutdown,
            &mut recent_ids,
            &mut capture,
        )
        .await;
        if *shutdown.borrow() {
            return Ok(());
        }
        let message = match result {
            Ok(()) => "MEXC perpetual stream ended; starting a fresh observed session".to_owned(),
            Err(error) => format!(
                "MEXC perpetual stream disconnected; starting a fresh observed session: {error:#}"
            ),
        };
        if cvd {
            send(
                sender,
                MarketEvent::FlowStatus {
                    connected: false,
                    message,
                },
            )
            .await?;
        } else {
            send(
                sender,
                MarketEvent::Status {
                    connected: false,
                    message,
                },
            )
            .await?;
        }
        tokio::select! {
            changed = shutdown.changed() => if changed.is_err() || *shutdown.borrow() { return Ok(()); },
            _ = tokio::time::sleep(Duration::from_secs(1)) => {}
        }
    }
}

async fn stream_futures_connection(
    client: &reqwest::Client,
    market: &Market,
    cvd: bool,
    sender: &mpsc::Sender<MarketEvent>,
    shutdown: &mut watch::Receiver<bool>,
    recent_ids: &mut RecentTradeIds,
    capture: &mut FuturesCapture,
) -> Result<()> {
    if *shutdown.borrow() {
        return Ok(());
    }
    let (socket, _) = connect_async(FUTURES_WS)
        .await
        .context("connecting MEXC futures websocket")?;
    let (mut write, mut read) = socket.split();
    if cvd {
        write.send(Message::Text(json!({"method":"sub.deal","param":{"symbol":market.symbol},"gzip":false,"compress":false}).to_string().into()))
            .await.context("subscribing to MEXC perpetual raw deals")?;
    }
    write.send(Message::Text(json!({"method":"sub.kline","param":{"symbol":market.symbol,"interval":futures_interval(market.interval)?}}).to_string().into())).await.context("subscribing to MEXC futures candles")?;
    let step = market.interval.seconds();
    let mut buffered = VecDeque::<Message>::new();
    if cvd && recent_ids.last.is_some() {
        let rows = {
            let recovery = fetch_futures_recovery(client, &market.symbol, recent_ids);
            tokio::pin!(recovery);
            loop {
                tokio::select! {
                    result = &mut recovery => break result?,
                    frame = read.next() => match frame {
                        Some(Ok(Message::Ping(data))) => write.send(Message::Pong(data)).await.context("replying to MEXC futures websocket ping while recovering")?,
                        Some(Ok(Message::Close(frame))) => bail!("MEXC futures websocket closed during recovery: {frame:?}"),
                        Some(Ok(Message::Text(text))) => {
                            if buffered.len() == RECENT_TRADE_IDS * 10 { break None; }
                            buffered.push_back(Message::Text(text));
                        }
                        Some(Ok(_)) => {},
                        Some(Err(error)) => return Err(error).context("reading MEXC futures websocket during recovery"),
                        None => bail!("MEXC futures websocket disconnected during recovery"),
                    },
                    changed = shutdown.changed() => if changed.is_err() || *shutdown.borrow() { return Ok(()); },
                }
            }
        };
        if let Some(rows) = rows {
            for deal in rows {
                let Some(id) = deal.get("i").and_then(native_id) else {
                    capture.invalidate(step, sender).await?;
                    recent_ids.clear();
                    break;
                };
                if recent_ids.insert(&id) {
                    capture.ingest(&deal, true, step, sender).await?;
                }
            }
            capture.emit_current(step, sender).await?;
        } else {
            capture.invalidate(step, sender).await?;
            recent_ids.clear();
        }
    }
    let buffered_stream = futures_util::stream::iter(
        buffered
            .into_iter()
            .map(Ok::<Message, tokio_tungstenite::tungstenite::Error>),
    );
    let mut incoming = buffered_stream.chain(read);
    let mut subscribed = false;
    let mut heartbeat = tokio::time::interval(Duration::from_secs(15));
    let mut flow_connection_confirmed = false;
    loop {
        tokio::select! {
            result = shutdown.changed() => {
                if result.is_err() || *shutdown.borrow() {
                    if cvd {
                        capture.invalidate(step, sender).await?;
                        send(sender, MarketEvent::FlowStatus { connected: false, message: "MEXC perpetual CVD capture stopped".into() }).await?;
                    }
                    let _ = write.send(Message::Close(None)).await;
                    return Ok(());
                }
            }
            _ = heartbeat.tick() => write.send(Message::Text(json!({"method":"ping"}).to_string().into())).await.context("sending MEXC futures heartbeat")?,
            frame = incoming.next() => {
                let message = match frame {
                    Some(Ok(message)) => message,
                    Some(Err(error)) => return Err(error).context("reading MEXC futures websocket"),
                    None => bail!("MEXC futures websocket disconnected"),
                };
                match message {
                Message::Text(text) => {
                    let val: Value = serde_json::from_str(&text).context("invalid MEXC futures JSON")?;
                    if cvd && val.get("channel").and_then(Value::as_str) == Some("push.deal") {
                        if val.get("symbol").and_then(Value::as_str) != Some(market.symbol.as_str()) { continue; }
                        if let Some(deals) = val.get("data").and_then(Value::as_array) {
                            for deal in deals {
                                let id = deal.get("i").and_then(native_id);
                                if id.as_ref().is_some_and(|id| !recent_ids.insert(id)) { continue; }
                                if !flow_connection_confirmed {
                                    send(sender, MarketEvent::FlowStatus { connected: true, message: "MEXC perpetual sides are venue-reported (unconfirmed aggressor); capture is session-only".into() }).await?;
                                    flow_connection_confirmed = true;
                                }
                                capture.ingest(deal, id.is_some(), step, sender).await?;
                            }
                            capture.emit_current(step, sender).await?;
                        }
                    } else {
                        match parse_frame(text.as_bytes(), market)? {
                            Update::Subscribed => { if !subscribed { send(sender, MarketEvent::Status { connected:true, message:"Connected to MEXC perpetual candle stream".into() }).await?; subscribed=true; } },
                            Update::Candle(candle) => send(sender, MarketEvent::Candle { candle }).await?,
                            Update::Pong | Update::Ignore => {},
                        }
                    }
                },
                Message::Ping(data) => write.send(Message::Pong(data)).await.context("replying to MEXC websocket ping")?,
                Message::Close(frame) => bail!("MEXC futures websocket closed: {frame:?}"),
                Message::Binary(_) => bail!("unexpected binary MEXC futures websocket frame"),
                Message::Pong(_) | Message::Frame(_) => {},
            }
                }
        }
    }
}

// MEXC spot candle subscriptions use the protobuf topic
// `spot@public.kline.v3.api.pb`, not JSON kline frames. Poll the native REST
// candle instead; this is native candle data, not trade aggregation.
async fn stream_spot(
    client: &reqwest::Client,
    instrument: &Instrument,
    market: &Market,
    cvd: bool,
    sender: &mpsc::Sender<MarketEvent>,
    shutdown: &mut watch::Receiver<bool>,
) -> Result<()> {
    let interval = spot_interval(market.interval)?;
    let mut timer = tokio::time::interval(Duration::from_secs(5));
    let mut last: Option<Candle> = None;
    if *shutdown.borrow() {
        return Ok(());
    }
    let _flow_task = if cvd {
        let flow_client = client.clone();
        let flow_instrument = instrument.clone();
        let flow_market = market.clone();
        let flow_sender = sender.clone();
        let flow_shutdown = shutdown.clone();
        Some(super::AbortOnDrop(tokio::spawn(async move {
            if let Err(error) = stream_spot_flow(
                &flow_client,
                &flow_instrument,
                &flow_market,
                &flow_sender,
                flow_shutdown,
            )
            .await
            {
                let _ = send(&flow_sender, MarketEvent::FlowStatus { connected: false, message: format!("MEXC spot reported-side trade stream ended; coverage after last complete bar is missing: {error:#}") }).await;
            }
        })))
    } else {
        None
    };
    loop {
        tokio::select! {
            result = shutdown.changed() => { if result.is_err() || *shutdown.borrow() { return Ok(()); } }
            _ = timer.tick() => {
                let rows: Vec<Vec<Value>> = get_json(client, &format!("{SPOT_REST}/klines"), &[("symbol", instrument.symbol.clone()), ("interval", interval.to_owned()), ("limit", "1".to_owned())]).await.context("polling MEXC spot latest candle (REST polling)")?;
                let row = rows.last().context("MEXC spot latest-candle response was empty")?;
                ensure!(row.len() >= 6, "short MEXC spot latest kline row");
                let candle = Candle { time: integer(&row[0])? / 1000, open:number(&row[1])?, high:number(&row[2])?, low:number(&row[3])?, close:number(&row[4])?, volume:number(&row[5])?, flow: None };
                validate(candle, market.interval)?;
                if !last.is_some_and(|previous| previous == candle) {
                    if last.is_none() { send(sender, MarketEvent::Status { connected:true, message:"Connected to MEXC spot candles (REST polling)".into() }).await?; }
                    send(sender, MarketEvent::Candle { candle }).await?;
                    last = Some(candle);
                }
            }
        }
    }
}

async fn stream_spot_flow(
    client: &reqwest::Client,
    instrument: &Instrument,
    market: &Market,
    sender: &mpsc::Sender<MarketEvent>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    let mut recent_ids = RecentTradeIds::default();
    loop {
        if *shutdown.borrow() {
            return Ok(());
        }
        let result = stream_spot_flow_connection(
            client,
            instrument,
            market,
            sender,
            shutdown.clone(),
            &mut recent_ids,
        )
        .await;
        if *shutdown.borrow() {
            return Ok(());
        }
        let message = match result {
            Ok(()) => "MEXC spot trade stream ended; starting a fresh observed session".to_owned(),
            Err(error) => format!(
                "MEXC spot trade stream disconnected; starting a fresh observed session: {error:#}"
            ),
        };
        send(
            sender,
            MarketEvent::FlowStatus {
                connected: false,
                message,
            },
        )
        .await?;
        tokio::select! {
            changed = shutdown.changed() => if changed.is_err() || *shutdown.borrow() { return Ok(()); },
            _ = tokio::time::sleep(Duration::from_secs(1)) => {}
        }
    }
}

async fn stream_spot_flow_connection(
    _client: &reqwest::Client,
    instrument: &Instrument,
    market: &Market,
    sender: &mpsc::Sender<MarketEvent>,
    mut shutdown: watch::Receiver<bool>,
    recent_ids: &mut RecentTradeIds,
) -> Result<()> {
    let (socket, _) = connect_async("wss://wbs-api.mexc.com/ws")
        .await
        .context("connecting MEXC spot trade websocket")?;
    let (mut write, mut read) = socket.split();
    let topic = format!(
        "spot@public.aggre.deals.v3.api.pb@100ms@{}",
        instrument.symbol
    );
    write
        .send(Message::Text(
            json!({"method":"SUBSCRIPTION","params":[topic]})
                .to_string()
                .into(),
        ))
        .await
        .context("subscribing to MEXC spot raw trade stream")?;
    let step = market.interval.seconds();
    let mut eligible_from: Option<i64> = None;
    let mut heartbeat = tokio::time::interval(Duration::from_secs(20));
    let mut active: Option<i64> = None;
    let mut buy = NativeDecimal::ZERO;
    let mut sell = NativeDecimal::ZERO;
    let mut valid = true;
    let mut last_trade_ms: Option<i64> = None;
    let mut reported_connected = false;
    loop {
        tokio::select! {
            result = shutdown.changed() => {
                if result.is_err() || *shutdown.borrow() {
                    if let Some(bar) = active {
                        send(sender, MarketEvent::Flow { bar: FlowBar { time: bar, flow: flow::missing(bar, bar.saturating_add(step).saturating_mul(1000)) } }).await?;
                    }
                    send(sender, MarketEvent::FlowStatus { connected: false, message: "MEXC spot CVD capture stopped; the active interval is incomplete".into() }).await?;
                    let _ = write.send(Message::Close(None)).await;
                    return Ok(());
                }
            }
            _ = heartbeat.tick() => {
                write.send(Message::Text(json!({"method":"PING"}).to_string().into())).await.context("sending MEXC spot websocket heartbeat")?;
            }
            frame = read.next() => {
                let message = match frame {
                    Some(Ok(message)) => message,
                    Some(Err(error)) => {
                        if let Some(bar) = active {
                            send(sender, MarketEvent::Flow { bar: FlowBar { time: bar, flow: flow::missing(bar, bar.saturating_add(step).saturating_mul(1000)) } }).await?;
                        }
                        return Err(error).context("reading MEXC spot trade websocket");
                    }
                    None => {
                        if let Some(bar) = active {
                            send(sender, MarketEvent::Flow { bar: FlowBar { time: bar, flow: flow::missing(bar, bar.saturating_add(step).saturating_mul(1000)) } }).await?;
                        }
                        bail!("MEXC spot trade websocket disconnected")
                    }
                };
                match message {
                Message::Binary(bytes) => {
                    let wrapper = PbWrapper::decode(bytes.as_ref()).context("decoding official MEXC spot protobuf envelope")?;
                    if wrapper.symbol.as_deref().is_some_and(|s| s != instrument.symbol) { continue; }
                    let Some(pb_wrapper::Body::AggDeals(deals)) = wrapper.body else { continue; };
                    for deal in deals.deals {
                        let has_id = !deal.trade_id.is_empty();
                        if has_id && !recent_ids.insert(&deal.trade_id) { continue; }
                        if !reported_connected {
                            send(sender, MarketEvent::FlowStatus { connected: true, message: "MEXC tradeType is venue-reported, not confirmed aggressor; the first observed interval is incomplete and later data is session-only".into() }).await?;
                            reported_connected = true;
                        }
                        if deal.time <= 0 {
                            if let Some(bar) = active {
                                valid = false;
                                send(sender, MarketEvent::Flow { bar: FlowBar { time: bar, flow: flow::missing(bar, bar.saturating_add(step).saturating_mul(1000)) } }).await?;
                            }
                            continue;
                        }
                        let event_ms = if deal.time >= 100_000_000_000 { deal.time } else { deal.time.saturating_mul(1000) };
                        let bucket = (event_ms / 1000).div_euclid(step) * step;
                        if eligible_from.is_none() {
                            eligible_from = Some(bucket.saturating_add(step));
                            send(sender, MarketEvent::Flow { bar: FlowBar { time: bucket, flow: flow::missing(bucket, bucket.saturating_add(step).saturating_mul(1000)) } }).await?;
                            continue;
                        }
                        let eligible = eligible_from.expect("capture boundary initialized");
                        if bucket < eligible { continue; }
                        if active.is_some_and(|current| bucket < current) {
                            send(sender, MarketEvent::Flow { bar: FlowBar { time: bucket, flow: flow::missing(bucket, bucket.saturating_add(step).saturating_mul(1000)) } }).await?;
                            continue;
                        }
                        if active != Some(bucket) {
                            if let Some(previous) = active {
                                let end = previous.saturating_add(step);
                                let flow = if valid { flow::snapshot(buy, sell, FlowQuality::Reported, previous, end.saturating_mul(1000))? } else { flow::missing(previous, end.saturating_mul(1000)) };
                                send(sender, MarketEvent::Flow { bar: FlowBar { time: previous, flow } }).await?;
                                let mut gap = end;
                                while gap < bucket {
                                    send(sender, MarketEvent::Flow { bar: FlowBar { time: gap, flow: flow::missing(gap, gap.saturating_add(step).saturating_mul(1000)) } }).await?;
                                    gap = gap.saturating_add(step);
                                }
                            } else {
                                let mut gap = eligible;
                                while gap < bucket {
                                    send(sender, MarketEvent::Flow { bar: FlowBar { time: gap, flow: flow::missing(gap, gap.saturating_add(step).saturating_mul(1000)) } }).await?;
                                    gap = gap.saturating_add(step);
                                }
                            }
                            active = Some(bucket);
                            buy = NativeDecimal::ZERO;
                            sell = NativeDecimal::ZERO;
                            last_trade_ms = None;
                            valid = true;
                        }
                        if !has_id {
                            valid = false;
                            send(sender, MarketEvent::Flow { bar: FlowBar { time: bucket, flow: flow::missing(bucket, bucket.saturating_add(step).saturating_mul(1000)) } }).await?;
                            continue;
                        }
                        let quantity = match flow::amount(&deal.quantity) {
                            Ok(quantity) => quantity,
                            Err(_) => {
                                valid = false;
                                send(sender, MarketEvent::Flow { bar: FlowBar { time: bucket, flow: flow::missing(bucket, bucket.saturating_add(step).saturating_mul(1000)) } }).await?;
                                continue;
                            }
                        };
                        if !matches!(add_reported_side(&mut buy, &mut sell, i64::from(deal.trade_type), quantity), Ok(true)) {
                            valid = false;
                            send(sender, MarketEvent::Flow { bar: FlowBar { time: bucket, flow: flow::missing(bucket, bucket.saturating_add(step).saturating_mul(1000)) } }).await?;
                        } else {
                            last_trade_ms = Some(last_trade_ms.map_or(event_ms, |last| last.max(event_ms)));
                        }
                    }
                    if let (Some(bar), Some(through)) = (active, last_trade_ms) {
                        let through = through.max(bar.saturating_mul(1000)).min(bar.saturating_add(step).saturating_mul(1000));
                        let snapshot = if valid { flow::snapshot(buy, sell, FlowQuality::Reported, bar, through)? } else { flow::missing(bar, through) };
                        send(sender, MarketEvent::Flow { bar: FlowBar { time: bar, flow: snapshot } }).await?;
                    }
                }
                Message::Ping(data) => write.send(Message::Pong(data)).await.context("replying to MEXC spot websocket ping")?,
                Message::Close(frame) => {
                    if let Some(bar) = active {
                        send(sender, MarketEvent::Flow { bar: FlowBar { time: bar, flow: flow::missing(bar, bar.saturating_add(step).saturating_mul(1000)) } }).await?;
                    }
                    bail!("MEXC spot trade websocket closed: {frame:?}")
                }
                Message::Text(text) if text.contains("PING") => {
                    write.send(Message::Text(json!({"method":"PONG"}).to_string().into())).await.context("replying to MEXC spot heartbeat")?;
                }
                Message::Text(_) | Message::Pong(_) | Message::Frame(_) => {},
            }
                }
        }
    }
}

#[cfg(test)]
mod flow_tests {
    use super::*;

    #[test]
    fn base_contract_snapshot_scales_decimal_delta_before_float_rounding() {
        let mut capture = FuturesCapture {
            buy: flow::amount("1000000000000000.00000001").unwrap(),
            sell: flow::amount("1000000000000000").unwrap(),
            base_factor: Some(flow::amount("0.0001").unwrap()),
            ..FuturesCapture::default()
        };
        let normalized = capture.snapshot(300, 600_000).unwrap();
        assert_eq!(normalized.delta, Some(0.000000000001));
        assert_eq!(normalized.volume, Some(200_000_000_000.0));
        assert_eq!(normalized.quality, FlowQuality::Reported);
        assert_eq!(normalized.covered_from, 300);
        assert_eq!(normalized.through_ms, 600_000);
        capture.base_factor = None;
        assert_eq!(
            capture.snapshot(300, 600_000).unwrap().delta,
            Some(0.00000001)
        );
    }

    #[test]
    fn numeric_contract_quantities_preserve_small_signed_difference_before_float_conversion() {
        let buy = serde_json::from_str::<Value>("1000000000000000.00000001").unwrap();
        let sell = serde_json::from_str::<Value>("1000000000000000").unwrap();
        let snapshot = flow::snapshot(
            json_decimal(&buy).unwrap(),
            json_decimal(&sell).unwrap(),
            FlowQuality::Reported,
            300,
            600_000,
        )
        .unwrap();
        assert_eq!(snapshot.delta, Some(1e-8));
    }

    #[test]
    fn aggregate_time_splits_are_disjoint_and_stop_at_a_saturated_second() {
        let Some((left, right)) = split_agg_range(10_000, 12_000) else {
            panic!("two-second range should split");
        };
        assert_eq!(left, (10_000, 11_000));
        assert_eq!(right, (11_001, 12_000));
        assert_eq!(left.1 + 1, right.0);
        assert_eq!(split_agg_range(10_000, 10_999), None);
    }

    #[test]
    fn null_id_identical_aggregate_fills_are_both_counted_inclusive_once() {
        let row = json!({"q":"0.4","m":true,"T":12_000,"a":null});
        let mut buy = NativeDecimal::ZERO;
        let mut sell = NativeDecimal::ZERO;
        accumulate_agg_trades([row.clone(), row], 12_000, 12_000, &mut buy, &mut sell).unwrap();
        assert_eq!(buy, NativeDecimal::ZERO);
        assert_eq!(sell, flow::amount("0.8").unwrap());
    }

    #[test]
    fn official_spot_protobuf_deal_decodes_native_quantity_and_reported_direction() {
        // PublicAggreDealsV3Api.deals field 1 containing an item with the
        // official price/quantity/tradeType/tradeId field numbers.
        let bytes = [
            0x0a, 0x20, 0x0a, 0x06, b'1', b'0', b'0', b'.', b'2', b'5', 0x12, 0x05, b'0', b'.',
            b'1', b'2', b'5', 0x18, 0x02, 0x2a, 0x0d, b'n', b'a', b't', b'i', b'v', b'e', b'-',
            b'f', b'i', b'l', b'l', b'-', b'7',
        ];
        let deals = PbDeals::decode(bytes.as_slice()).unwrap();
        let deal = &deals.deals[0];

        let mut buy = NativeDecimal::ZERO;
        let mut sell = NativeDecimal::ZERO;
        assert!(
            add_reported_side(
                &mut buy,
                &mut sell,
                i64::from(deal.trade_type),
                flow::amount(&deal.quantity).unwrap(),
            )
            .unwrap()
        );
        assert_eq!(
            buy - sell,
            NativeDecimal::ZERO - flow::amount("0.125").unwrap()
        );
    }

    #[test]
    fn mexc_spot_maker_flag_and_reported_buy_sell_have_explicit_opposite_signs() {
        let qty = flow::amount("2.5").unwrap();
        let mut buy = NativeDecimal::ZERO;
        let mut sell = NativeDecimal::ZERO;
        add_spot_maker_side(&mut buy, &mut sell, false, qty).unwrap();
        assert_eq!(buy - sell, qty);
        add_spot_maker_side(&mut buy, &mut sell, true, qty).unwrap();
        assert_eq!(buy - sell, NativeDecimal::ZERO);

        let mut buy = NativeDecimal::ZERO;
        let mut sell = NativeDecimal::ZERO;
        assert!(add_reported_side(&mut buy, &mut sell, 1, qty).unwrap());
        assert_eq!(buy - sell, qty);
        assert!(add_reported_side(&mut buy, &mut sell, 2, qty).unwrap());
        assert_eq!(buy - sell, NativeDecimal::ZERO);
        assert!(!add_reported_side(&mut buy, &mut sell, 3, qty).unwrap());
    }
    #[test]
    fn futures_recovery_uses_opaque_marker_and_requires_ordered_recent_window() {
        let response = json!({
            "success": true,
            "data": [
                {"i":"opaque-next","t":300,"v":"1","T":1},
                {"i":"opaque-middle","t":200,"v":"2","T":2},
                {"i":"opaque-marker","t":100,"v":"3","T":1}
            ]
        });
        let recovered = futures_recovery_rows(&response, "opaque-marker").unwrap();
        assert_eq!(
            recovered
                .iter()
                .filter_map(|row| row.get("i").and_then(native_id))
                .collect::<Vec<_>>(),
            vec!["opaque-middle".to_owned(), "opaque-next".to_owned()]
        );
        assert!(futures_recovery_rows(&response, "expired-marker").is_none());
        let out_of_order = json!({
            "success": true,
            "data": [
                {"i":"newer","t":200},
                {"i":"older","t":300},
                {"i":"opaque-marker","t":100}
            ]
        });
        assert!(futures_recovery_rows(&out_of_order, "opaque-marker").is_none());
    }

    #[test]
    fn reconnect_id_deduplication_is_bounded() {
        let mut ids = RecentTradeIds::default();
        for index in 0..RECENT_TRADE_IDS {
            assert!(ids.insert(&format!("id-{index}")));
        }
        assert_eq!(ids.order.len(), RECENT_TRADE_IDS);
        assert!(!ids.insert("id-99"));
        assert!(ids.insert("newest"));
        assert_eq!(ids.order.len(), RECENT_TRADE_IDS);
        assert!(ids.insert("id-0"));
    }

    #[tokio::test]
    async fn futures_capture_resets_per_bucket_and_finalizes_only_on_native_progression() {
        let base = 1_800_000_000_i64;
        let (sender, mut receiver) = mpsc::channel(16);
        let mut capture = FuturesCapture::default();
        capture
            .ingest(
                &json!({"i":"first","t":(base + 120) * 1000,"v":"8","T":1}),
                true,
                60,
                &sender,
            )
            .await
            .unwrap();
        let first = receiver.try_recv().unwrap();
        assert!(
            matches!(first, MarketEvent::Flow { bar } if bar.time == base + 120 && bar.flow.quality == FlowQuality::Missing)
        );

        capture
            .ingest(
                &json!({"i":"buy-2","t":(base + 180) * 1000 + 500,"v":"2","T":1}),
                true,
                60,
                &sender,
            )
            .await
            .unwrap();
        capture.emit_current(60, &sender).await.unwrap();
        let interim = receiver.try_recv().unwrap();
        assert!(
            matches!(interim, MarketEvent::Flow { bar } if bar.time == base + 180 && bar.flow.delta == Some(2.0))
        );
        capture
            .ingest(
                &json!({"i":"sell-1","t":(base + 240) * 1000 + 10,"v":"1","T":2}),
                true,
                60,
                &sender,
            )
            .await
            .unwrap();
        let finalized = receiver.try_recv().unwrap();
        assert!(matches!(finalized, MarketEvent::Flow { bar } if
            bar.time == base + 180
                && bar.flow.quality == FlowQuality::Reported
                && bar.flow.delta == Some(2.0)
                && bar.flow.volume == Some(2.0)
                && bar.flow.covered_from == base + 180
                && bar.flow.through_ms == (base + 240) * 1000
        ));
        assert_eq!(capture.buy, NativeDecimal::ZERO);
        assert_eq!(capture.sell, flow::amount("1").unwrap());
        assert_eq!(capture.active_bar, Some(base + 240));
    }
    #[tokio::test]
    async fn futures_trade_without_native_id_revokes_the_active_numeric_snapshot() {
        let base = 1_800_000_000_i64;
        let (sender, mut receiver) = mpsc::channel(16);
        let mut capture = FuturesCapture::default();
        capture
            .ingest(
                &json!({"i":"baseline","t":(base + 120) * 1000,"v":"8","T":1}),
                true,
                60,
                &sender,
            )
            .await
            .unwrap();
        let _ = receiver.try_recv().unwrap();
        capture
            .ingest(
                &json!({"i":"known","t":(base + 180) * 1000 + 100,"v":"2","T":1}),
                true,
                60,
                &sender,
            )
            .await
            .unwrap();
        capture.emit_current(60, &sender).await.unwrap();
        let _ = receiver.try_recv().unwrap();
        capture
            .ingest(
                &json!({"t":(base + 180) * 1000 + 200,"v":"5","T":1}),
                false,
                60,
                &sender,
            )
            .await
            .unwrap();
        assert!(!capture.valid);
        let revoked = receiver.try_recv().unwrap();
        assert!(matches!(revoked, MarketEvent::Flow { bar } if
            bar.time == base + 180
                && bar.flow.quality == FlowQuality::Missing
                && bar.flow.delta.is_none()
        ));
        capture.emit_current(60, &sender).await.unwrap();
        assert!(receiver.try_recv().is_err());
    }
}
