use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
use chrono::DateTime;
use futures_util::{SinkExt, StreamExt};
use mexc_model::{
    Candle, FlowBar, FlowQuality, HistoryEvent, Instrument, Market, MarketEvent, Product,
    VolumeFlow,
};
use reqwest::{Client, header::HeaderMap};
use rust_decimal::{Decimal, prelude::ToPrimitive};
use serde_json::{Value, json};
use tokio::{
    net::TcpStream,
    sync::{mpsc, watch},
    time::{self, Instant},
};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async, tungstenite::Message};

use super::super::flow;

const TRADE_LIMIT: usize = 1_000;
const REST_SPACING: Duration = Duration::from_millis(125);
const LIVE_WS: &str = "wss://ws-feed.exchange.coinbase.com";
const PAGE_BODY_LIMIT: usize = 4 * 1024 * 1024;

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;
type SocketStream = futures_util::stream::SplitStream<Socket>;

#[derive(Default)]
pub(super) struct PriceCoverage {
    candle_times: BTreeSet<i64>,
    observed_from_seconds: Option<i64>,
    observed_through_ms: i64,
}

pub(super) type PriceCoverageHandle = Arc<tokio::sync::Mutex<PriceCoverage>>;

pub(super) fn new_price_coverage() -> PriceCoverageHandle {
    Arc::new(tokio::sync::Mutex::new(PriceCoverage::default()))
}

fn record_price_observation_inner(
    coverage: &mut PriceCoverage,
    candles: &[Candle],
    from_seconds: i64,
    through_ms: i64,
    interval_seconds: i64,
) {
    let previous_through_ms = coverage.observed_through_ms;
    let from_ms = from_seconds.saturating_mul(1_000);
    if previous_through_ms > 0 && from_ms > previous_through_ms {
        coverage.observed_from_seconds = Some(from_seconds);
    } else {
        coverage.observed_from_seconds = Some(
            coverage
                .observed_from_seconds
                .map_or(from_seconds, |old| old.min(from_seconds)),
        );
    }
    coverage.observed_through_ms = previous_through_ms.max(through_ms);
    coverage
        .candle_times
        .extend(candles.iter().map(|candle| candle.time));
    while coverage.candle_times.len() > 20_000 {
        if let Some(oldest) = coverage.candle_times.pop_first() {
            coverage.observed_from_seconds = Some(
                coverage
                    .observed_from_seconds
                    .map_or(oldest.saturating_add(interval_seconds), |old| {
                        old.max(oldest.saturating_add(interval_seconds))
                    }),
            );
        }
    }
}

pub(super) async fn record_price_observation(
    coverage: &PriceCoverageHandle,
    candles: &[Candle],
    from_seconds: i64,
    through_ms: i64,
    interval_seconds: i64,
) {
    let mut coverage = coverage.lock().await;
    record_price_observation_inner(
        &mut coverage,
        candles,
        from_seconds,
        through_ms,
        interval_seconds,
    );
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Trade {
    id: u64,
    time_ms: i64,
    size: Decimal,
    maker_side: MakerSide,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MakerSide {
    Buy,
    Sell,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Totals {
    buy: Decimal,
    sell: Decimal,
}

impl Totals {
    fn add(&mut self, trade: &Trade) -> Result<()> {
        match trade.maker_side {
            MakerSide::Sell => {
                self.buy = self
                    .buy
                    .checked_add(trade.size)
                    .context("Coinbase buy-volume overflow")?;
            }
            MakerSide::Buy => {
                self.sell = self
                    .sell
                    .checked_add(trade.size)
                    .context("Coinbase sell-volume overflow")?;
            }
        }
        Ok(())
    }

    fn volume(self) -> Result<Decimal> {
        self.buy
            .checked_add(self.sell)
            .context("Coinbase total-volume overflow")
    }
}

#[derive(Debug)]
struct TradePage {
    trades: Vec<Trade>,
    after: Option<String>,
    before: Option<String>,
}

#[derive(Default)]
struct Pacer {
    previous: Option<Instant>,
}

impl Pacer {
    async fn wait_history(&mut self, sender: &mpsc::Sender<HistoryEvent>) -> Result<()> {
        if let Some(previous) = self.previous {
            tokio::select! {
                _ = sender.closed() => bail!("Coinbase flow-history receiver closed"),
                _ = time::sleep_until(previous + REST_SPACING) => {},
            }
        }
        self.previous = Some(Instant::now());
        Ok(())
    }

    async fn wait_live(&mut self, shutdown: &mut watch::Receiver<bool>) -> Result<()> {
        if let Some(previous) = self.previous {
            tokio::select! {
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        bail!("Coinbase live flow stopped");
                    }
                }
                _ = time::sleep_until(previous + REST_SPACING) => {},
            }
        }
        self.previous = Some(Instant::now());
        Ok(())
    }
}

fn parse_id(value: &Value) -> Result<u64> {
    match value {
        Value::Number(number) => number
            .as_u64()
            .context("Coinbase trade id is not an unsigned integer"),
        Value::String(text) => text.parse().context("invalid Coinbase trade id"),
        _ => bail!("Coinbase trade id has an invalid type"),
    }
}

fn parse_time(value: &str) -> Result<i64> {
    Ok(DateTime::parse_from_rfc3339(value)
        .context("invalid Coinbase trade timestamp")?
        .timestamp_millis())
}

fn parse_side(value: &str) -> Result<MakerSide> {
    match value {
        "buy" => Ok(MakerSide::Buy),
        "sell" => Ok(MakerSide::Sell),
        _ => bail!("unknown Coinbase maker side {value:?}"),
    }
}

fn parse_trade(value: &Value) -> Result<Trade> {
    let id = parse_id(
        value
            .get("trade_id")
            .context("Coinbase trade has no trade_id")?,
    )?;
    let time_ms = parse_time(
        value
            .get("time")
            .and_then(Value::as_str)
            .context("Coinbase trade has no timestamp")?,
    )?;
    let size = flow::amount(
        value
            .get("size")
            .and_then(Value::as_str)
            .context("Coinbase trade has no native size")?,
    )?;
    let maker_side = parse_side(
        value
            .get("side")
            .and_then(Value::as_str)
            .context("Coinbase trade has no maker side")?,
    )?;
    Ok(Trade {
        id,
        time_ms,
        size,
        maker_side,
    })
}

fn validate_page(trades: &[Trade]) -> Result<()> {
    ensure!(
        trades
            .windows(2)
            .all(|pair| { pair[0].id >= pair[1].id && pair[0].time_ms >= pair[1].time_ms }),
        "Coinbase native trades are not newest-first by id and time"
    );
    Ok(())
}

fn cursor_header(headers: &HeaderMap, name: &'static str) -> Result<Option<String>> {
    headers
        .get(name)
        .map(|value| {
            value
                .to_str()
                .map(str::to_owned)
                .with_context(|| format!("invalid Coinbase {name} cursor header"))
        })
        .transpose()
}

async fn get_trade_page(
    client: &Client,
    symbol: &str,
    cursor_name: Option<&'static str>,
    cursor: Option<&str>,
    limit: usize,
) -> Result<TradePage> {
    let mut query = vec![("limit", limit.to_string())];
    if let (Some(name), Some(value)) = (cursor_name, cursor) {
        query.push((name, value.to_owned()));
    }
    let response = client
        .get(format!("{}/products/{symbol}/trades", super::SPOT_BASE))
        .query(&query)
        .send()
        .await
        .context("GET Coinbase Exchange public trades")?;
    let status = response.status();
    let after = cursor_header(response.headers(), "CB-AFTER")?;
    let before = cursor_header(response.headers(), "CB-BEFORE")?;
    if response
        .content_length()
        .is_some_and(|length| length > PAGE_BODY_LIMIT as u64)
    {
        bail!("Coinbase trade page exceeds {PAGE_BODY_LIMIT} bytes");
    }
    let bytes = response
        .bytes()
        .await
        .context("reading Coinbase trade page")?;
    ensure!(
        bytes.len() <= PAGE_BODY_LIMIT,
        "Coinbase trade page exceeds {PAGE_BODY_LIMIT} bytes"
    );
    if !status.is_success() {
        let excerpt: String = String::from_utf8_lossy(&bytes)
            .chars()
            .take(1_024)
            .collect();
        bail!("Coinbase trades returned HTTP {status}: {excerpt}");
    }
    let values: Vec<Value> =
        serde_json::from_slice(&bytes).context("invalid Coinbase trades JSON")?;
    let trades = values.iter().map(parse_trade).collect::<Result<Vec<_>>>()?;
    validate_page(&trades)?;
    Ok(TradePage {
        trades,
        after,
        before,
    })
}

async fn history_page(
    client: &Client,
    symbol: &str,
    cursor_name: Option<&'static str>,
    cursor: Option<&str>,
    limit: usize,
    pacer: &mut Pacer,
    sender: &mpsc::Sender<HistoryEvent>,
) -> Result<TradePage> {
    pacer.wait_history(sender).await?;
    tokio::select! {
        _ = sender.closed() => bail!("Coinbase flow-history receiver closed"),
        result = get_trade_page(client, symbol, cursor_name, cursor, limit) => result,
    }
}

async fn live_page(
    client: &Client,
    symbol: &str,
    query: (Option<&'static str>, Option<&str>, usize),
    pacer: &mut Pacer,
    inbox: (
        &mut mpsc::Receiver<std::result::Result<Message, String>>,
        &mut Vec<LiveMessage>,
    ),
    shutdown: &mut watch::Receiver<bool>,
    sender: &mpsc::Sender<MarketEvent>,
) -> Result<TradePage> {
    let (cursor_name, cursor, limit) = query;
    let (receiver, buffered) = inbox;
    pacer.wait_live(shutdown).await?;
    let request = get_trade_page(client, symbol, cursor_name, cursor, limit);
    tokio::pin!(request);
    loop {
        tokio::select! {
            _ = sender.closed() => bail!("Coinbase live flow receiver closed"),
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    bail!("Coinbase live flow stopped");
                }
            }
            result = &mut request => return result,
            message = receiver.recv() => {
                let message = message.context("Coinbase WebSocket reader ended during REST reconciliation")?;
                let message = message.map_err(anyhow::Error::msg)?;
                if let Some(message) = parse_live_message(message, symbol)? {
                    buffered.push(message);
                }
            }
        }
    }
}

fn seek_is_at_or_after(page: &[Trade], target_ms: i64) -> bool {
    page.first().is_some_and(|trade| trade.time_ms >= target_ms)
}

#[derive(Debug)]
struct Seek {
    cursor: u64,
    observed_ms: i64,
}

async fn seek_cursor(
    client: &Client,
    symbol: &str,
    target_ms: i64,
    pacer: &mut Pacer,
    sender: &mpsc::Sender<HistoryEvent>,
) -> Result<Option<Seek>> {
    let latest_page = history_page(client, symbol, None, None, 1, pacer, sender).await?;
    let Some(latest) = latest_page.trades.first().cloned() else {
        return Ok(None);
    };
    let observed_ms = flow::unix_millis();
    let high = latest
        .id
        .checked_add(1)
        .context("Coinbase latest trade id cannot be paged")?;
    if latest.time_ms < target_ms {
        return Ok(Some(Seek {
            cursor: high,
            observed_ms,
        }));
    }

    // `after` is an older cursor. Binary-search the first cursor whose newest
    // returned record is at/after the requested right edge; paging from it
    // includes that boundary record and every older native record.
    let mut low = 0_u64;
    let mut high = high;
    let mut probes = 0_usize;
    while low + 1 < high {
        let middle = low + (high - low) / 2;
        let page = history_page(
            client,
            symbol,
            Some("after"),
            Some(&middle.to_string()),
            1,
            pacer,
            sender,
        )
        .await?;
        if seek_is_at_or_after(&page.trades, target_ms) {
            high = middle;
        } else {
            low = middle;
        }
        probes += 1;
        if probes.is_multiple_of(8) {
            sender
                .send(HistoryEvent::FlowStatus {
                    message: format!("Coinbase native trade-ID seek: {probes} probes"),
                })
                .await?;
        }
    }
    let boundary = history_page(
        client,
        symbol,
        Some("after"),
        Some(&high.to_string()),
        1,
        pacer,
        sender,
    )
    .await?;
    let Some(anchor) = boundary.trades.first() else {
        bail!("Coinbase trade-ID seek found no right-edge record");
    };
    ensure!(
        anchor.time_ms >= target_ms && anchor.id <= latest.id,
        "Coinbase trade-ID seek did not bracket the requested time"
    );
    let before_anchor = history_page(
        client,
        symbol,
        Some("after"),
        Some(&anchor.id.to_string()),
        1,
        pacer,
        sender,
    )
    .await?;
    ensure!(
        before_anchor
            .trades
            .first()
            .is_none_or(|trade| trade.time_ms < target_ms),
        "Coinbase trade-ID seek skipped records tied at the right-edge timestamp"
    );
    Ok(Some(Seek {
        cursor: high,
        observed_ms,
    }))
}

fn consume_page(
    trades: &[Trade],
    oldest_id: &mut Option<u64>,
    oldest_time_ms: &mut Option<i64>,
    start_ms: i64,
    end_ms: i64,
    width_seconds: i64,
    totals: &mut BTreeMap<i64, Totals>,
) -> Result<usize> {
    validate_page(trades)?;
    let mut fresh = 0;
    for trade in trades {
        if oldest_id.is_some_and(|previous| trade.id >= previous) {
            continue;
        }
        *oldest_id = Some(trade.id);
        *oldest_time_ms = Some(oldest_time_ms.map_or(trade.time_ms, |old| old.min(trade.time_ms)));
        fresh += 1;
        if trade.time_ms >= start_ms && trade.time_ms < end_ms {
            let bucket = trade.time_ms.div_euclid(width_seconds * 1_000) * width_seconds;
            totals.entry(bucket).or_default().add(trade)?;
        }
    }
    Ok(fresh)
}

fn sum_volume_matches(traded: Decimal, candle_volume: f64) -> bool {
    if !candle_volume.is_finite() || candle_volume < 0.0 {
        return false;
    }
    let Some(traded) = traded.to_f64() else {
        return false;
    };
    if !traded.is_finite() {
        return false;
    }
    let tolerance = f64::EPSILON * traded.abs().max(candle_volume.abs()).max(1.0) * 8.0;
    (traded - candle_volume).abs() <= tolerance
}

fn bucket_complete(totals: Totals, candle_volume: f64, left_edge_proven: bool) -> Result<bool> {
    let traded_volume = totals.volume()?;
    if candle_volume == 0.0 && traded_volume.is_zero() {
        return Ok(true);
    }
    Ok(left_edge_proven && sum_volume_matches(traded_volume, candle_volume))
}

fn gap_has_native_trades(
    totals: &BTreeMap<i64, Totals>,
    after_seconds: i64,
    before_seconds: i64,
) -> Result<bool> {
    for (_, value) in totals.range(after_seconds..before_seconds) {
        if !value.volume()?.is_zero() {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(super) async fn flow_history(
    client: &Client,
    instrument: &Instrument,
    market: &Market,
    candles: &[Candle],
    sender: &mpsc::Sender<HistoryEvent>,
) -> Result<()> {
    if market.product == Product::Perpetual {
        sender
            .send(HistoryEvent::FlowStatus {
                message: "Coinbase International has no public signed market-trade history; portfolio fills and candle-volume proxies are unavailable".into(),
            })
            .await?;
        for candle in candles {
            let through_ms = candle.time.saturating_mul(1_000);
            sender
                .send(HistoryEvent::Flow {
                    bar: FlowBar {
                        time: candle.time,
                        flow: flow::missing(candle.time, through_ms),
                    },
                })
                .await?;
        }
        return Ok(());
    }
    super::spot_symbol(&instrument.symbol)?;
    if candles.is_empty() {
        sender
            .send(HistoryEvent::FlowStatus {
                message: "Coinbase trade replay has no requested native candles".into(),
            })
            .await?;
        return Ok(());
    }
    let width = market.interval.seconds();
    ensure!(width > 0, "invalid Coinbase candle interval");
    ensure!(
        candles.windows(2).all(|pair| pair[0].time < pair[1].time),
        "Coinbase flow-history candles are not sorted"
    );
    let range_start_ms = candles[0].time.saturating_mul(1_000);
    let requested_end_ms = candles[candles.len() - 1]
        .time
        .saturating_add(width)
        .saturating_mul(1_000);
    sender
        .send(HistoryEvent::FlowStatus {
            message:
                "Coinbase Exchange: seeking native trade IDs before complete maker-side replay"
                    .into(),
        })
        .await?;
    let mut pacer = Pacer::default();
    let Some(seek) = seek_cursor(
        client,
        &instrument.symbol,
        requested_end_ms,
        &mut pacer,
        sender,
    )
    .await?
    else {
        emit_missing_history(candles, width, sender).await?;
        sender
            .send(HistoryEvent::FlowStatus {
                message: "Coinbase public trade history returned no native records; requested buckets remain missing".into(),
            })
            .await?;
        return Ok(());
    };
    let observed_ms = seek.observed_ms;
    let through_end_ms = requested_end_ms.min(observed_ms);
    if through_end_ms <= range_start_ms {
        emit_missing_history(candles, width, sender).await?;
        sender
            .send(HistoryEvent::FlowStatus {
                message: "Coinbase trade replay has not reached the requested bucket range".into(),
            })
            .await?;
        return Ok(());
    }

    let mut cursor = seek.cursor.to_string();
    let mut seen_cursors = HashSet::new();
    let mut totals = BTreeMap::<i64, Totals>::new();
    let mut oldest_id = None;
    let mut oldest_time_ms = None;
    let mut pages = 0_usize;
    loop {
        let page = history_page(
            client,
            &instrument.symbol,
            Some("after"),
            Some(&cursor),
            TRADE_LIMIT,
            &mut pacer,
            sender,
        )
        .await?;
        if page.trades.is_empty() {
            break;
        }
        consume_page(
            &page.trades,
            &mut oldest_id,
            &mut oldest_time_ms,
            range_start_ms,
            through_end_ms,
            width,
            &mut totals,
        )?;
        pages += 1;
        let reached_left_edge = oldest_time_ms.is_some_and(|time| time < range_start_ms);
        if pages.is_multiple_of(10) {
            sender
                .send(HistoryEvent::FlowStatus {
                    message: format!("Coinbase native trade replay: {pages} pages fetched"),
                })
                .await?;
        }
        if reached_left_edge || page.trades.len() < TRADE_LIMIT {
            break;
        }
        let Some(next) = page.after else {
            break;
        };
        if !seen_cursors.insert(next.clone()) || next == cursor {
            break;
        }
        cursor = next;
    }

    let mut first_complete = None;
    let mut started = false;
    let mut chain_broken = false;
    let mut previous_complete_time: Option<i64> = None;
    let mut missing = 0_usize;
    for candle in candles {
        if sender.is_closed() {
            bail!("Coinbase flow-history receiver closed");
        }
        let open_ms = candle.time.saturating_mul(1_000);
        let end_ms = candle.time.saturating_add(width).saturating_mul(1_000);
        let through_ms = end_ms.min(observed_ms);
        let own = totals.get(&candle.time).copied().unwrap_or_default();
        let left_edge_proven = oldest_time_ms.is_some_and(|time| time < open_ms);
        let native_zero = candle.volume == 0.0 && own.volume()?.is_zero();
        let complete = through_ms >= open_ms
            && (native_zero
                || if end_ms > observed_ms {
                    left_edge_proven
                } else {
                    bucket_complete(own, candle.volume, left_edge_proven)?
                });
        let flow_value = if !complete {
            missing += 1;
            if started {
                chain_broken = true;
            }
            flow::missing(candle.time, through_ms.max(open_ms))
        } else {
            if !started {
                started = true;
                first_complete = Some(candle.time);
            }
            if let Some(previous) = previous_complete_time
                && candle.time > previous.saturating_add(width)
                && gap_has_native_trades(&totals, previous.saturating_add(width), candle.time)?
            {
                chain_broken = true;
            }
            previous_complete_time = Some(candle.time);
            let covered_from = if chain_broken {
                candle.time
            } else {
                first_complete.unwrap_or(candle.time)
            };
            flow::snapshot(
                own.buy,
                own.sell,
                FlowQuality::Exact,
                covered_from,
                through_ms,
            )?
        };
        sender
            .send(HistoryEvent::Flow {
                bar: FlowBar {
                    time: candle.time,
                    flow: flow_value,
                },
            })
            .await?;
    }
    let status = if missing == 0 {
        format!(
            "Coinbase replay complete: {pages} native trade pages, maker-side inverted to aggressor flow"
        )
    } else {
        format!(
            "Coinbase replay covered {} of {} requested buckets; {missing} are missing or failed native-volume/edge validation",
            candles.len() - missing,
            candles.len()
        )
    };
    sender
        .send(HistoryEvent::FlowStatus { message: status })
        .await?;
    Ok(())
}

async fn emit_missing_history(
    candles: &[Candle],
    width: i64,
    sender: &mpsc::Sender<HistoryEvent>,
) -> Result<()> {
    for candle in candles {
        if sender.is_closed() {
            bail!("Coinbase flow-history receiver closed");
        }
        let open_ms = candle.time.saturating_mul(1_000);
        let through_ms = candle
            .time
            .saturating_add(width)
            .saturating_mul(1_000)
            .min(flow::unix_millis())
            .max(open_ms);
        let value = if candle.volume == 0.0 {
            flow::snapshot(
                Decimal::ZERO,
                Decimal::ZERO,
                FlowQuality::Exact,
                candle.time,
                through_ms,
            )?
        } else {
            flow::missing(candle.time, through_ms)
        };
        sender
            .send(HistoryEvent::Flow {
                bar: FlowBar {
                    time: candle.time,
                    flow: value,
                },
            })
            .await?;
    }
    Ok(())
}

#[derive(Clone, Debug)]
enum LiveMessage {
    Trade(Trade),
    Heartbeat { last_trade_id: u64, time_ms: i64 },
}

fn parse_live_message(message: Message, symbol: &str) -> Result<Option<LiveMessage>> {
    let text = match message {
        Message::Text(text) => text.to_string(),
        Message::Binary(bytes) => {
            String::from_utf8(bytes.to_vec()).context("non-UTF8 Coinbase WebSocket payload")?
        }
        Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => return Ok(None),
        Message::Close(frame) => bail!("Coinbase WebSocket closed: {frame:?}"),
    };
    let value: Value = serde_json::from_str(&text).context("invalid Coinbase WebSocket JSON")?;
    let kind = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match kind {
        "match" | "last_match" => {
            if value.get("product_id").and_then(Value::as_str) != Some(symbol) {
                return Ok(None);
            }
            Ok(Some(LiveMessage::Trade(parse_trade(&value)?)))
        }
        "heartbeat" => {
            if value.get("product_id").and_then(Value::as_str) != Some(symbol) {
                return Ok(None);
            }
            let last_trade_id = parse_id(
                value
                    .get("last_trade_id")
                    .context("Coinbase heartbeat has no last_trade_id")?,
            )?;
            let time_ms = parse_time(
                value
                    .get("time")
                    .and_then(Value::as_str)
                    .context("Coinbase heartbeat has no timestamp")?,
            )?;
            Ok(Some(LiveMessage::Heartbeat {
                last_trade_id,
                time_ms,
            }))
        }
        "error" => bail!("Coinbase WebSocket error: {value}"),
        _ => Ok(None),
    }
}

fn merge_live_trades(after_id: Option<u64>, mut trades: Vec<Trade>) -> Result<Vec<Trade>> {
    trades.retain(|trade| after_id.is_none_or(|last| trade.id > last));
    trades.sort_by_key(|trade| trade.id);
    let mut merged = Vec::<Trade>::with_capacity(trades.len());
    for trade in trades {
        if let Some(previous) = merged.last() {
            if previous.id == trade.id {
                ensure!(
                    previous == &trade,
                    "Coinbase REST/WS trade ID has conflicting native values"
                );
                continue;
            }
            ensure!(
                previous.id < trade.id,
                "Coinbase live trade IDs are not increasing"
            );
            ensure!(
                previous.time_ms <= trade.time_ms,
                "Coinbase live trade time regressed by ID"
            );
        }
        merged.push(trade);
    }
    Ok(merged)
}

#[derive(Default)]
struct LiveState {
    buckets: BTreeMap<i64, Totals>,
    coverage: BTreeMap<i64, bool>,
    last_seen_id: Option<u64>,
    last_seen_ms: Option<i64>,
    verified_ms: Option<i64>,
    last_price_checked_through_ms: i64,
    current_bucket: Option<i64>,
    last_emitted_bucket: Option<i64>,
    coverage_origin: Option<i64>,
    started: bool,
    chain_broken: bool,
    connection_gap_broke_chain: bool,
    price_gap_broke_chain: bool,
    capture_contiguous: bool,
    price_gaps: HashMap<i64, bool>,
}

impl LiveState {
    fn add_trades(&mut self, trades: &[Trade], width: i64) -> Result<Vec<i64>> {
        let mut changed = Vec::new();
        for trade in trades {
            let bucket = trade.time_ms.div_euclid(width * 1_000) * width;
            self.buckets.entry(bucket).or_default().add(trade)?;
            self.coverage.entry(bucket).or_insert_with(|| {
                self.capture_contiguous
                    && self.current_bucket.is_some_and(|current| bucket >= current)
            });
            if changed.last().copied() != Some(bucket) {
                changed.push(bucket);
            }
            self.last_seen_id = Some(self.last_seen_id.map_or(trade.id, |id| id.max(trade.id)));
            self.last_seen_ms = Some(
                self.last_seen_ms
                    .map_or(trade.time_ms, |ms| ms.max(trade.time_ms)),
            );
        }
        Ok(changed)
    }
}

fn missing_flow(time: i64, through_ms: i64, width: i64) -> VolumeFlow {
    let bucket_open = time.saturating_mul(1_000);
    let bucket_end = time.saturating_add(width).saturating_mul(1_000);
    flow::missing(time, through_ms.clamp(bucket_open, bucket_end))
}

async fn send_live_flow(
    sender: &mpsc::Sender<MarketEvent>,
    state: &mut LiveState,
    bucket: i64,
    through_ms: i64,
    width: i64,
) -> Result<()> {
    let bucket_end = bucket.saturating_add(width).saturating_mul(1_000);
    let observed = through_ms.min(bucket_end);
    let complete = state.coverage.get(&bucket).copied().unwrap_or(false);
    let totals = state.buckets.get(&bucket).copied().unwrap_or_default();
    let value = if !complete {
        if state.started {
            state.chain_broken = true;
        }
        missing_flow(bucket, observed.max(bucket.saturating_mul(1_000)), width)
    } else {
        if !state.started {
            state.started = true;
            state.coverage_origin = Some(bucket);
        }
        let covered_from = if state.chain_broken {
            bucket
        } else {
            state.coverage_origin.unwrap_or(bucket)
        };
        flow::snapshot(
            totals.buy,
            totals.sell,
            FlowQuality::Exact,
            covered_from,
            observed.max(bucket.saturating_mul(1_000)),
        )?
    };
    sender
        .send(MarketEvent::Flow {
            bar: FlowBar {
                time: bucket,
                flow: value,
            },
        })
        .await?;
    Ok(())
}

async fn emit_verified(
    sender: &mpsc::Sender<MarketEvent>,
    state: &mut LiveState,
    price_coverage: &PriceCoverageHandle,
    through_ms: i64,
    width: i64,
) -> Result<()> {
    let current = through_ms.div_euclid(width * 1_000) * width;
    let price_through_ms = price_coverage.lock().await.observed_through_ms;
    let price_advanced = price_through_ms > state.last_price_checked_through_ms;
    let mut start = state
        .last_emitted_bucket
        .or(state.current_bucket)
        .unwrap_or(current);
    if price_advanced && let Some(oldest_gap) = state.price_gaps.keys().copied().min() {
        start = start.min(oldest_gap);
    }
    state.last_price_checked_through_ms = price_through_ms;
    let mut bucket = start;
    while bucket <= current {
        let end_ms = bucket.saturating_add(width).saturating_mul(1_000);
        let through = through_ms.min(end_ms);
        let follows_observed_bar = state
            .current_bucket
            .is_some_and(|previous| bucket > previous);
        state
            .coverage
            .entry(bucket)
            .or_insert(state.capture_contiguous && follows_observed_bar);
        if through >= bucket.saturating_mul(1_000) {
            let price_observation = price_coverage.lock().await;
            let has_price_candle = price_observation.candle_times.contains(&bucket);
            let price_from = price_observation.observed_from_seconds;
            let price_through = price_observation.observed_through_ms;
            drop(price_observation);
            let absent_native_bucket_is_observed = !has_price_candle
                && price_from.is_some_and(|from| from <= bucket)
                && price_through >= end_ms
                && through >= end_ms;
            if has_price_candle {
                if let Some(was_complete) = state.price_gaps.remove(&bucket) {
                    state.coverage.insert(bucket, was_complete);
                    if was_complete
                        && state.price_gap_broke_chain
                        && state.price_gaps.is_empty()
                        && !state.connection_gap_broke_chain
                    {
                        state.chain_broken = false;
                        state.price_gap_broke_chain = false;
                    }
                }
                if bucket >= start || state.buckets.contains_key(&bucket) {
                    send_live_flow(sender, state, bucket, through, width).await?;
                }
            } else if absent_native_bucket_is_observed {
                let totals = state.buckets.get(&bucket).copied().unwrap_or_default();
                if !totals.volume()?.is_zero() {
                    let was_complete = state.coverage.get(&bucket).copied().unwrap_or(false);
                    let first_gap = !state.price_gaps.contains_key(&bucket);
                    state.price_gaps.entry(bucket).or_insert(was_complete);
                    state.coverage.insert(bucket, false);
                    if was_complete && state.started && !state.chain_broken {
                        state.chain_broken = true;
                        state.price_gap_broke_chain = true;
                    }
                    sender
                        .send(MarketEvent::Flow {
                            bar: FlowBar {
                                time: bucket,
                                flow: missing_flow(bucket, end_ms, width),
                            },
                        })
                        .await?;
                    if first_gap {
                        sender
                            .send(MarketEvent::FlowStatus {
                                connected: true,
                                message: format!(
                                    "Coinbase trades were recovered in {bucket}, but the native candle is absent; CVD continuity is not bridged"
                                ),
                            })
                            .await?;
                    }
                } else if state.coverage.get(&bucket).copied().unwrap_or(false) {
                    state.price_gaps.remove(&bucket);
                }
            } else if bucket >= start || state.buckets.contains_key(&bucket) {
                send_live_flow(sender, state, bucket, through, width).await?;
            }
        }
        bucket = bucket.saturating_add(width);
    }
    state.current_bucket = Some(current);
    state.last_emitted_bucket = Some(current);
    state.verified_ms = Some(through_ms);
    let retain_from = current.saturating_sub(width.saturating_mul(20_000));
    state.buckets.retain(|bucket, _| *bucket >= retain_from);
    state.coverage.retain(|bucket, _| *bucket >= retain_from);
    state.price_gaps.retain(|bucket, _| *bucket >= retain_from);
    Ok(())
}

async fn emit_invalidated(
    sender: &mpsc::Sender<MarketEvent>,
    state: &mut LiveState,
    through_ms: i64,
    width: i64,
) -> Result<()> {
    let current = through_ms.div_euclid(width * 1_000) * width;
    let start_ms = state
        .verified_ms
        .or(state.last_seen_ms)
        .unwrap_or(through_ms);
    let start = start_ms.div_euclid(width * 1_000) * width;
    let mut affected = Vec::new();
    let mut bucket = start;
    while bucket <= current {
        affected.push(bucket);
        bucket = bucket.saturating_add(width);
    }
    for bucket in affected {
        let end_ms = bucket.saturating_add(width).saturating_mul(1_000);
        let observed = through_ms.min(end_ms).max(bucket.saturating_mul(1_000));
        sender
            .send(MarketEvent::Flow {
                bar: FlowBar {
                    time: bucket,
                    flow: missing_flow(bucket, observed, width),
                },
            })
            .await?;
    }
    if state.started && current >= start && !state.chain_broken {
        state.connection_gap_broke_chain = true;
        state.chain_broken = true;
    }
    state.capture_contiguous = false;
    state.current_bucket = Some(current);
    state.last_emitted_bucket = Some(start);
    Ok(())
}

async fn reader_task(
    stream: SocketStream,
    sender: mpsc::Sender<std::result::Result<Message, String>>,
) {
    futures_util::pin_mut!(stream);
    while let Some(message) = stream.next().await {
        if sender
            .send(message.map_err(|error| error.to_string()))
            .await
            .is_err()
        {
            break;
        }
    }
}

async fn live_session(
    client: &Client,
    instrument: &Instrument,
    market: &Market,
    sender: &mpsc::Sender<MarketEvent>,
    price_coverage: &PriceCoverageHandle,
    shutdown: &mut watch::Receiver<bool>,
    session_state: (&mut Pacer, &mut LiveState),
) -> Result<()> {
    let (pacer, state) = session_state;
    let width = market.interval.seconds();
    let now_ms = flow::unix_millis();
    let active_bucket = now_ms.div_euclid(width * 1_000) * width;
    let prior_seen_id = state.last_seen_id;
    let prior_seen_ms = state.last_seen_ms;
    sender
        .send(MarketEvent::FlowStatus {
            connected: false,
            message: "Connecting to Coinbase public matches and heartbeat channels".into(),
        })
        .await?;
    let socket = loop {
        tokio::select! {
            _ = sender.closed() => return Ok(()),
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return Ok(());
                }
            }
            result = connect_async(LIVE_WS) => {
                break result.context("connecting Coinbase public feed")?.0;
            }
        }
    };
    let (mut sink, stream) = socket.split();
    let (reader_tx, mut reader_rx) = mpsc::channel(4_096);
    let _reader = super::super::AbortOnDrop(tokio::spawn(reader_task(stream, reader_tx)));
    async {
        sink.send(Message::Text(
            json!({
                "type": "subscribe",
                "product_ids": [instrument.symbol],
                "channels": ["matches", "heartbeat"]
            })
            .to_string()
            .into(),
        ))
        .await
        .context("subscribing to Coinbase matches and heartbeat")?;
        sender
            .send(MarketEvent::FlowStatus {
                connected: false,
                message: "Coinbase feed connected; recovering the forming bucket from REST before publishing flow".into(),
            })
            .await?;

        let mut buffered = Vec::new();
        let mut totals = BTreeMap::<i64, Totals>::new();
        let mut oldest_id = None;
        let mut oldest_time = None;
        let mut cursor: Option<String> = None;
        let mut first = true;
        let mut newest: Option<Trade> = None;
        let mut rest_through_ms = now_ms;
        let mut edge_complete = false;
        loop {
            let page = live_page(
                client,
                &instrument.symbol,
                (if first { None } else { Some("after") }, cursor.as_deref(), TRADE_LIMIT),
                pacer,
                (&mut reader_rx, &mut buffered),
                shutdown,
                sender,
            )
            .await?;
            first = false;
            if page.trades.is_empty() {
                break;
            }
            if newest.is_none() {
                newest = page.trades.first().cloned();
                rest_through_ms = newest.as_ref().map_or(now_ms, |trade| trade.time_ms);
            }
            consume_page(
                &page.trades,
                &mut oldest_id,
                &mut oldest_time,
                active_bucket.saturating_mul(1_000),
                i64::MAX,
                width,
                &mut totals,
            )?;
            edge_complete = oldest_time.is_some_and(|time| time < active_bucket.saturating_mul(1_000));
            if edge_complete || page.trades.len() < TRADE_LIMIT {
                break;
            }
            let Some(next) = page.after else { break };
            if cursor.as_deref() == Some(next.as_str()) {
                break;
            }
            cursor = Some(next);
        }

        let heartbeat = buffered
            .iter()
            .filter_map(|message| match message {
                LiveMessage::Heartbeat { last_trade_id, time_ms } => {
                    Some((*last_trade_id, *time_ms))
                }
                LiveMessage::Trade(_) => None,
            })
            .max_by_key(|(id, _)| *id);
        let rest_id = newest.as_ref().map(|trade| trade.id);
        let target_id = [prior_seen_id, rest_id, heartbeat.map(|(id, _)| id)]
            .into_iter()
            .flatten()
            .max()
            .context("Coinbase has no REST or heartbeat trade cursor to reconcile")?;
        let from_id = prior_seen_id.or(rest_id).context("Coinbase has no initial REST trade cursor")?;
        let mut catchup_trades = if target_id > from_id {
            catch_up_to_heartbeat(
                client,
                &instrument.symbol,
                (from_id, target_id),
                pacer,
                (&mut reader_rx, &mut buffered),
                shutdown,
                sender,
            )
            .await?
        } else {
            Vec::new()
        };
        for message in &buffered {
            if let LiveMessage::Trade(trade) = message
                && trade.id > from_id && trade.id <= target_id
            {
                catchup_trades.push(trade.clone());
            }
        }
        let merged = merge_live_trades(Some(from_id), catchup_trades)?;
        let old_current_complete = state.coverage.get(&active_bucket).copied().unwrap_or(false);
        let use_rest_base = edge_complete || !old_current_complete;
        if use_rest_base {
            state.buckets.retain(|bucket, _| *bucket < active_bucket);
            for (bucket, value) in &totals {
                state.buckets.insert(*bucket, *value);
            }
            state.coverage.retain(|bucket, _| *bucket < active_bucket);
            for bucket in totals.keys() {
                let left_proven = oldest_time.is_some_and(|time| time < bucket.saturating_mul(1_000));
                state.coverage.insert(*bucket, left_proven);
            }
        }
        let prior_bucket = prior_seen_ms.map(|ms| ms.div_euclid(width * 1_000) * width);
        state.coverage.insert(
            active_bucket,
            edge_complete
                || old_current_complete
                || prior_bucket.is_some_and(|bucket| bucket < active_bucket),
        );
        for trade in &merged {
            let bucket = trade.time_ms.div_euclid(width * 1_000) * width;
            if use_rest_base && bucket >= active_bucket && rest_id.is_some_and(|id| trade.id <= id) {
                continue;
            }
            state.buckets.entry(bucket).or_default().add(trade)?;
            let left_proven = oldest_time.is_some_and(|time| time < bucket.saturating_mul(1_000));
            let recovered_after_open = prior_bucket.is_some_and(|previous| bucket > previous);
            state.coverage
                .entry(bucket)
                .or_insert(left_proven || recovered_after_open);
        }
        let target_trade = if rest_id == Some(target_id) {
            newest.as_ref()
        } else {
            merged.iter().find(|trade| trade.id == target_id)
        };
        if target_id > from_id {
            ensure!(
                target_trade.is_some(),
                "Coinbase heartbeat trade {target_id} was not recovered by REST"
            );
        }
        state.last_seen_id = Some(target_id);
        if let Some(trade) = target_trade {
            state.last_seen_ms = Some(trade.time_ms);
        }
        let verified_ms = heartbeat
            .filter(|(id, _)| *id >= target_id)
            .map_or(rest_through_ms, |(_, time_ms)| time_ms);
        state.verified_ms = Some(verified_ms.max(state.last_seen_ms.unwrap_or(verified_ms)));
        state.current_bucket = Some(active_bucket);
        state.last_emitted_bucket.get_or_insert(active_bucket);
        if state.connection_gap_broke_chain {
            state.chain_broken = false;
            state.connection_gap_broke_chain = false;
        }
        state.capture_contiguous = true;
        if let (Some(previous_ms), Some(verified_ms)) = (prior_seen_ms, state.verified_ms) {
            let mut bucket = previous_ms.div_euclid(width * 1_000) * width + width;
            let through_bucket = verified_ms.div_euclid(width * 1_000) * width;
            while bucket <= through_bucket {
                state.coverage.insert(bucket, true);
                bucket = bucket.saturating_add(width);
            }
        }
        buffered.clear();
        let through = state.verified_ms.unwrap_or(rest_through_ms);
        emit_verified(sender, state, price_coverage, through, width).await?;

        sender
            .send(MarketEvent::FlowStatus {
                connected: true,
                message: if state.coverage.get(&active_bucket).copied().unwrap_or(false) {
                    "Coinbase matches/heartbeat reconciled with complete forming-bucket REST history; maker side is inverted".into()
                } else {
                    "Coinbase matches/heartbeat connected, but the forming-bucket REST left edge was not proven; flow remains missing until complete coverage".into()
                },
            })
            .await?;

        let mut ping = time::interval(Duration::from_secs(20));
        ping.tick().await;
        loop {
            tokio::select! {
                _ = sender.closed() => return Ok(()),
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() { return Ok(()); }
                }
                _ = ping.tick() => {
                    sink.send(Message::Ping(Vec::new().into()))
                        .await.context("sending Coinbase WebSocket ping")?;
                }
                item = reader_rx.recv() => {
                    let item = item.context("Coinbase WebSocket reader ended")?;
                    let item = item.map_err(anyhow::Error::msg)?;
                    let Some(message) = parse_live_message(item, &instrument.symbol)? else { continue; };
                    match message {
                        LiveMessage::Trade(trade) => buffered.push(LiveMessage::Trade(trade)),
                        LiveMessage::Heartbeat { last_trade_id, time_ms } => {
                            let previous_seen = state.last_seen_id;
                            let previous_seen_ms = state.last_seen_ms;
                            let mut recovered = if previous_seen.is_some_and(|from| last_trade_id > from) {
                                catch_up_to_heartbeat(
                                    client,
                                    &instrument.symbol,
                                    (previous_seen.unwrap(), last_trade_id),
                                    pacer,
                                    (&mut reader_rx, &mut buffered),
                                    shutdown,
                                    sender,
                                )
                                .await?
                            } else {
                                Vec::new()
                            };
                            let mut retain = Vec::new();
                            for pending in buffered.drain(..) {
                                match pending {
                                    LiveMessage::Trade(trade) if trade.id <= last_trade_id => recovered.push(trade),
                                    LiveMessage::Trade(trade) if trade.id > last_trade_id => {
                                        retain.push(LiveMessage::Trade(trade));
                                    }
                                    LiveMessage::Heartbeat { last_trade_id: id, time_ms: at }
                                        if id > last_trade_id =>
                                    {
                                        retain.push(LiveMessage::Heartbeat { last_trade_id: id, time_ms: at });
                                    }
                                    _ => {}
                                }
                            }
                            buffered = retain;
                            let merged = merge_live_trades(previous_seen, recovered)?;
                            if previous_seen.is_none_or(|id| last_trade_id > id) {
                                ensure!(
                                    merged.iter().any(|trade| trade.id == last_trade_id),
                                    "Coinbase heartbeat trade {} was not recovered by REST",
                                    last_trade_id
                                );
                            }
                            let touched = state.add_trades(&merged, width)?;
                            if last_trade_id > previous_seen.unwrap_or(0) {
                                state.last_seen_id = Some(last_trade_id);
                                if let Some(trade) = merged.iter().find(|trade| trade.id == last_trade_id) {
                                    state.last_seen_ms = Some(trade.time_ms);
                                }
                            }
                            let through = time_ms
                                .max(state.last_seen_ms.unwrap_or(time_ms))
                                .max(state.verified_ms.unwrap_or(time_ms));
                            let previous_bucket = previous_seen_ms
                                .map(|ms| ms.div_euclid(width * 1_000) * width);
                            for bucket in touched {
                                let complete_from_cursor = previous_bucket
                                    .is_some_and(|previous| bucket > previous);
                                state.coverage.entry(bucket).or_insert(complete_from_cursor);
                            }
                            state.capture_contiguous = true;
                            state.verified_ms = Some(through);
                            emit_verified(sender, state, price_coverage, through, width).await?;
                            sender.send(MarketEvent::FlowStatus {
                                connected: true,
                                message: "Coinbase heartbeat REST reconciliation complete; absolute flow snapshots are current".into(),
                            }).await?;
                        }
                    }
                }
            }
        }
    }
    .await
}

async fn catch_up_to_heartbeat(
    client: &Client,
    symbol: &str,
    ids: (u64, u64),
    pacer: &mut Pacer,
    inbox: (
        &mut mpsc::Receiver<std::result::Result<Message, String>>,
        &mut Vec<LiveMessage>,
    ),
    shutdown: &mut watch::Receiver<bool>,
    sender: &mpsc::Sender<MarketEvent>,
) -> Result<Vec<Trade>> {
    let (from_id, target_id) = ids;
    let (receiver, buffered) = inbox;
    let mut cursor = from_id.to_string();
    let mut seen = HashSet::new();
    let mut trades = Vec::new();
    loop {
        let page = live_page(
            client,
            symbol,
            (Some("before"), Some(&cursor), TRADE_LIMIT),
            pacer,
            (&mut *receiver, &mut *buffered),
            shutdown,
            sender,
        )
        .await?;
        if page.trades.is_empty() {
            bail!("Coinbase REST returned no trades while reconciling heartbeat {target_id}");
        }
        let mut reached = false;
        for trade in &page.trades {
            if trade.id <= from_id {
                continue;
            }
            if trade.id == target_id {
                reached = true;
            }
            if trade.id <= target_id {
                trades.push(trade.clone());
            }
        }
        if reached {
            return Ok(trades);
        }
        if page.trades.len() < TRADE_LIMIT {
            bail!("Coinbase REST ended before heartbeat trade {target_id}");
        }
        let next = page
            .before
            .context("Coinbase REST omitted CB-BEFORE during heartbeat pagination")?;
        ensure!(
            seen.insert(next.clone()) && next != cursor,
            "Coinbase CB-BEFORE cursor did not advance"
        );
        cursor = next;
    }
}

async fn live_flow_loop(
    client: &Client,
    instrument: &Instrument,
    market: &Market,
    sender: &mpsc::Sender<MarketEvent>,
    price_coverage: PriceCoverageHandle,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    let mut state = LiveState::default();
    let mut pacer = Pacer::default();
    loop {
        if *shutdown.borrow() || sender.is_closed() {
            return Ok(());
        }
        match live_session(
            client,
            instrument,
            market,
            sender,
            &price_coverage,
            &mut shutdown,
            (&mut pacer, &mut state),
        )
        .await
        {
            Ok(()) => return Ok(()),
            Err(error) => {
                if *shutdown.borrow() || sender.is_closed() {
                    return Ok(());
                }
                let now = flow::unix_millis();
                emit_invalidated(sender, &mut state, now, market.interval.seconds()).await?;
                sender
                    .send(MarketEvent::FlowStatus {
                        connected: false,
                        message: format!("Coinbase matches/heartbeat interrupted; flow coverage invalidated while reconnecting: {error:#}"),
                    })
                    .await?;
                tokio::select! {
                    _ = sender.closed() => return Ok(()),
                    changed = shutdown.changed() => {
                        if changed.is_err() || *shutdown.borrow() { return Ok(()); }
                    }
                    _ = time::sleep(Duration::from_secs(1)) => {}
                }
            }
        }
    }
}

pub(super) async fn stream(
    client: &Client,
    instrument: &Instrument,
    market: &Market,
    sender: &mpsc::Sender<MarketEvent>,
    shutdown: watch::Receiver<bool>,
) -> Result<()> {
    if market.product != Product::Spot {
        bail!("Coinbase live flow is unavailable for International perpetuals");
    }
    super::spot_symbol(&instrument.symbol)?;
    let price_coverage = new_price_coverage();
    let price_shutdown = shutdown.clone();
    let price_sender = sender.clone();
    let price = super::price_stream(
        client,
        instrument,
        market,
        false,
        Some(price_coverage.clone()),
        &price_sender,
        price_shutdown,
    );
    let flow = live_flow_loop(client, instrument, market, sender, price_coverage, shutdown);
    tokio::select! {
        result = price => result,
        result = flow => result,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trade(id: u64, time_ms: i64, side: MakerSide, size: &str) -> Trade {
        Trade {
            id,
            time_ms,
            size: flow::amount(size).unwrap(),
            maker_side: side,
        }
    }

    #[test]
    fn maker_sell_is_positive_and_maker_buy_is_negative() {
        let mut totals = Totals::default();
        totals
            .add(&trade(12, 1_000, MakerSide::Sell, "0.125"))
            .unwrap();
        totals
            .add(&trade(11, 1_000, MakerSide::Buy, "0.025"))
            .unwrap();
        let result = flow::snapshot(totals.buy, totals.sell, FlowQuality::Exact, 1, 2_000).unwrap();
        assert_eq!(result.delta, Some(0.1));
        assert_eq!(result.volume, Some(0.15));
    }

    #[test]
    fn overlapping_native_pages_dedup_ids_but_keep_same_time_records() {
        let mut totals = BTreeMap::new();
        let mut oldest_id = None;
        let mut oldest_time = None;
        let page1 = [
            trade(30, 2_000, MakerSide::Sell, "0.1"),
            trade(29, 2_000, MakerSide::Buy, "0.2"),
        ];
        let page2 = [
            trade(29, 2_000, MakerSide::Buy, "0.2"),
            trade(28, 1_999, MakerSide::Sell, "0.3"),
        ];
        consume_page(
            &page1,
            &mut oldest_id,
            &mut oldest_time,
            0,
            60_000,
            60,
            &mut totals,
        )
        .unwrap();
        consume_page(
            &page2,
            &mut oldest_id,
            &mut oldest_time,
            0,
            60_000,
            60,
            &mut totals,
        )
        .unwrap();
        let result = totals[&0];
        assert_eq!(result.buy, flow::amount("0.4").unwrap());
        assert_eq!(result.sell, flow::amount("0.2").unwrap());
        assert_eq!(oldest_id, Some(28));
    }

    #[test]
    fn seek_boundary_includes_exact_target_time_and_tied_records() {
        let target = 60_000;
        let at_boundary = [
            trade(101, target, MakerSide::Sell, "1"),
            trade(100, target, MakerSide::Buy, "1"),
            trade(99, target - 1, MakerSide::Sell, "1"),
        ];
        assert!(seek_is_at_or_after(&at_boundary[..2], target));
        assert!(!seek_is_at_or_after(&at_boundary[2..], target));
        let excluded = at_boundary
            .iter()
            .filter(|trade| trade.time_ms < target)
            .count();
        assert_eq!(excluded, 1);
    }

    #[test]
    fn rest_reconciliation_recovers_dropped_matches_and_deduplicates_ws_overlap() {
        let rest = trade(52, 3_000, MakerSide::Sell, "0.4");
        let dropped = trade(51, 2_000, MakerSide::Buy, "0.2");
        let overlap = rest.clone();
        let merged = merge_live_trades(Some(50), vec![rest, dropped.clone(), overlap]).unwrap();
        assert_eq!(
            merged,
            vec![dropped, trade(52, 3_000, MakerSide::Sell, "0.4")]
        );
    }

    #[test]
    fn exact_coverage_requires_bucket_edges_and_native_volume_agreement() {
        let totals = Totals {
            buy: flow::amount("0.75").unwrap(),
            sell: flow::amount("0.25").unwrap(),
        };
        assert!(bucket_complete(totals, 1.0, true).unwrap());
        assert!(!bucket_complete(totals, 1.0, false).unwrap());
        assert!(!bucket_complete(totals, 1.1, true).unwrap());
        assert!(bucket_complete(Totals::default(), 0.0, false).unwrap());
    }

    #[test]
    fn a_trade_in_an_absent_price_bucket_breaks_continuous_coverage() {
        let mut totals = BTreeMap::new();
        totals.insert(
            60,
            Totals {
                buy: flow::amount("0.1").unwrap(),
                sell: Decimal::ZERO,
            },
        );
        assert!(gap_has_native_trades(&totals, 60, 180).unwrap());
        totals.insert(60, Totals::default());
        assert!(!gap_has_native_trades(&totals, 60, 180).unwrap());
    }

    #[test]
    fn price_observation_does_not_bridge_an_unqueried_poll_gap() {
        let mut coverage = PriceCoverage::default();
        record_price_observation_inner(&mut coverage, &[], 0, 59_000, 60);
        record_price_observation_inner(&mut coverage, &[], 0, 61_000, 60);
        assert_eq!(coverage.observed_from_seconds, Some(0));

        record_price_observation_inner(&mut coverage, &[], 120, 180_000, 60);
        assert_eq!(coverage.observed_from_seconds, Some(120));
        assert_eq!(coverage.observed_through_ms, 180_000);
    }
}
