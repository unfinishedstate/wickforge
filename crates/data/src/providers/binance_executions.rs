//! Native Binance market executions, independent of the cheap kline CVD feed.
//! All three products use the same aggregate identity across REST, daily archive
//! and WebSocket. Raw trade IDs (f/l) are deliberately NOT aggregate identities.
//! Sources: https://github.com/binance/binance-public-data#data-information
//! https://developers.binance.com/legacy-docs/derivatives/usds-margined-futures/market-data/rest-api/Compressed-Aggregate-Trades-List
//! Futures market aggregates exclude insurance-fund/ADL trades; q includes RPI.
use std::{
    io::Cursor,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use chrono::{DateTime, Utc};
use mexc_model::executions::{Coverage, Execution, ExecutionEvent, ExecutionPage};
use rust_decimal::Decimal;

use super::*;

const DAY_MS: i64 = 86_400_000;
const HOUR_MS: i64 = 3_600_000;
const FUTURES_RETENTION_MS: i64 = 48 * HOUR_MS;
const REST_LIMIT: usize = 1000;
const ARCHIVE_ROOT: &str = "https://data.binance.vision/data";
const MAX_ARCHIVE_BYTES: usize = 512 * 1024 * 1024;
// Futures sends server Ping every three minutes; lack of trades is not a gap.
const EXECUTION_IDLE_TIMEOUT: Duration = Duration::from_secs(4 * 60);

fn native_id(value: &Value) -> Result<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str()?.parse().ok())
        .context("Binance execution missing unsigned native ID")
}

fn milliseconds(value: i64) -> Result<i64> {
    ensure!(value >= 0, "negative Binance execution timestamp");
    // Spot archives changed to microseconds on 2025-01-01. REST/socket default
    // to ms; normalize each source before half-open filtering and deduplication.
    Ok(if value >= 100_000_000_000_000 {
        value / 1000
    } else {
        value
    })
}

fn positive(text: &str) -> Result<Decimal> {
    let value = flow::amount(text)?;
    ensure!(
        value > Decimal::ZERO,
        "nonpositive Binance execution price/quantity"
    );
    Ok(value)
}

fn aggregate(row: &Value) -> Result<Execution> {
    Ok(Execution {
        id: format!("agg:{}", native_id(&row["a"])?),
        time_ms: milliseconds(integer(&row["T"])?)?,
        price: positive(
            row["p"]
                .as_str()
                .context("Binance aggregate missing price")?,
        )?,
        // Native base units for spot/USD-M; native contracts for COIN-M.
        // Never use quoteQty/baseQty, nq, or catalog contract_size here.
        quantity: positive(
            row["q"]
                .as_str()
                .context("Binance aggregate missing quantity")?,
        )?,
    })
}

fn coverage(start_ms: i64, end_ms: i64, reason: Option<String>) -> Coverage {
    Coverage {
        start_ms,
        end_ms,
        complete: reason.is_none(),
        reason,
    }
}

fn archive_url(instrument: &Instrument, day_ms: i64) -> Result<String> {
    endpoint(instrument)?;
    let date = DateTime::<Utc>::from_timestamp_millis(day_ms)
        .context("Binance archive date outside supported calendar")?
        .format("%Y-%m-%d");
    let path = match (instrument.product, instrument.inverse) {
        (Product::Spot, false) => "spot",
        (Product::Perpetual, false) => "futures/um",
        (Product::Perpetual, true) => "futures/cm",
        _ => bail!("invalid Binance archive product"),
    };
    Ok(format!(
        "{ARCHIVE_ROOT}/{path}/daily/aggTrades/{0}/{0}-aggTrades-{date}.zip",
        instrument.symbol
    ))
}

fn csv_aggregate(record: &csv::StringRecord) -> Result<Execution> {
    ensure!(record.len() >= 7, "short Binance aggregate archive record");
    let id: u64 = record[0]
        .parse()
        .context("invalid Binance archive aggregate ID")?;
    let first: u64 = record[3]
        .parse()
        .context("invalid Binance archive first raw trade ID")?;
    let last: u64 = record[4]
        .parse()
        .context("invalid Binance archive last raw trade ID")?;
    ensure!(last >= first, "reversed Binance archive raw trade IDs");
    Ok(Execution {
        id: format!("agg:{id}"),
        price: positive(&record[1])?,
        quantity: positive(&record[2])?,
        time_ms: milliseconds(
            record[5]
                .parse()
                .context("invalid Binance archive timestamp")?,
        )?,
    })
}

fn decode_archive(
    bytes: &[u8],
    day_ms: i64,
    start_ms: i64,
    end_ms: i64,
    cancelled: &AtomicBool,
) -> Result<Vec<Execution>> {
    ensure!(
        !cancelled.load(Ordering::Relaxed),
        "Binance archive decode cancelled"
    );
    let mut zip =
        zip::ZipArchive::new(Cursor::new(bytes)).context("invalid Binance daily archive ZIP")?;
    ensure!(
        zip.len() == 1,
        "Binance daily archive must contain exactly one CSV"
    );
    let file = zip.by_index(0).context("opening Binance archive CSV")?;
    ensure!(
        file.name().ends_with(".csv"),
        "Binance archive member is not CSV"
    );
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .from_reader(file);
    let mut rows = Vec::new();
    let mut previous: Option<(u64, i64)> = None;
    for (index, record) in reader.records().enumerate() {
        ensure!(
            !cancelled.load(Ordering::Relaxed),
            "Binance archive decode cancelled"
        );
        let record = record.context("invalid Binance archive CSV")?;
        // Futures archives have a header; spot archives generally do not.
        if index == 0 && matches!(record.get(0), Some("agg_trade_id" | "aggregate_trade_id")) {
            ensure!(
                record.get(1) == Some("price") && record.get(2) == Some("quantity"),
                "invalid Binance archive header"
            );
            continue;
        }
        let row = csv_aggregate(&record)?;
        ensure!(
            row.time_ms >= day_ms && row.time_ms < day_ms.saturating_add(DAY_MS),
            "Binance archive timestamp outside named UTC day"
        );
        let id: u64 = record[0].parse()?;
        if let Some((old_id, old_time)) = previous {
            ensure!(
                id > old_id && row.time_ms >= old_time,
                "unordered/duplicate Binance archive aggregate ID"
            );
            ensure!(
                old_id.checked_add(1) == Some(id),
                "Binance archive skipped an aggregate ID"
            );
        }
        previous = Some((id, row.time_ms));
        if row.time_ms >= start_ms && row.time_ms < end_ms {
            rows.push(row);
        }
    }
    // Reading every CSV record also checks the ZIP member's CRC. A truncated
    // or malformed archive never grants completeness, even outside the range.
    Ok(rows)
}

struct CancelDecode(Arc<AtomicBool>);

impl Drop for CancelDecode {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

async fn daily_archive(
    client: &Client,
    instrument: &Instrument,
    day_ms: i64,
    start_ms: i64,
    end_ms: i64,
) -> Result<Vec<Execution>> {
    let url = archive_url(instrument, day_ms)?;
    let download = async {
        let mut response = client
            .get(&url)
            .send()
            .await
            .context("downloading Binance daily executions")?
            .error_for_status()
            .context("Binance daily archive unavailable")?;
        ensure!(
            response
                .content_length()
                .is_none_or(|length| length <= MAX_ARCHIVE_BYTES as u64),
            "Binance daily archive exceeds download limit"
        );
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .context("reading Binance daily archive")?
        {
            ensure!(
                chunk.len() <= MAX_ARCHIVE_BYTES.saturating_sub(bytes.len()),
                "Binance daily archive exceeds download limit"
            );
            bytes.extend_from_slice(&chunk);
        }
        Ok::<_, anyhow::Error>(bytes)
    };
    let bytes = tokio::time::timeout(Duration::from_secs(120), download)
        .await
        .context("Binance archive download timed out")??;
    let cancelled = Arc::new(AtomicBool::new(false));
    let _cancel_on_drop = CancelDecode(cancelled.clone());
    tokio::task::spawn_blocking(move || {
        decode_archive(&bytes, day_ms, start_ms, end_ms, &cancelled)
    })
    .await
    .context("Binance archive decoder interrupted")?
}

// Binance's initial time query is inclusive at both ends and limited to 1000
// records. A full page MUST continue by native ID, never last timestamp + 1:
// thousands of aggregates may share one millisecond. fromId is sent alone, as
// required by futures docs, then rows are filtered to the original window.
async fn rest_window(
    client: &Client,
    instrument: &Instrument,
    start_ms: i64,
    end_ms: i64,
    out: &mut Vec<Execution>,
) -> Result<()> {
    let url = format!("{}/aggTrades", endpoint(instrument)?);
    rest_window_at(client, &url, &instrument.symbol, start_ms, end_ms, out).await
}

async fn rest_window_at(
    client: &Client,
    url: &str,
    symbol: &str,
    start_ms: i64,
    end_ms: i64,
    out: &mut Vec<Execution>,
) -> Result<()> {
    ensure!(
        end_ms > start_ms && end_ms - start_ms <= HOUR_MS,
        "invalid Binance execution REST window"
    );
    let mut from_id: Option<u64> = None;
    let mut previous: Option<(u64, i64)> = None;
    loop {
        let mut query = vec![
            ("symbol", symbol.to_owned()),
            ("limit", REST_LIMIT.to_string()),
        ];
        if let Some(id) = from_id {
            query.push(("fromId", id.to_string()));
        } else {
            query.push(("startTime", start_ms.to_string()));
            query.push(("endTime", (end_ms - 1).to_string()));
        }
        let page: Vec<Value> = get_json(client, url, &query).await?;
        ensure!(
            page.len() <= REST_LIMIT,
            "oversized Binance aggregate REST page"
        );
        let length = page.len();
        let mut reached_end = false;
        for value in page {
            let id = native_id(&value["a"])?;
            if let Some(expected) = from_id {
                ensure!(
                    id >= expected,
                    "Binance aggregate ID cursor did not advance"
                );
            }
            let row = aggregate(&value)?;
            ensure!(
                row.time_ms >= start_ms,
                "Binance aggregate query returned execution before start"
            );
            if let Some((old_id, old_time)) = previous {
                ensure!(
                    id > old_id && row.time_ms >= old_time,
                    "unordered Binance aggregate REST page"
                );
                ensure!(
                    id == old_id
                        .checked_add(1)
                        .context("Binance aggregate ID overflow")?,
                    "Binance aggregate replay skipped an ID"
                );
            }
            previous = Some((id, row.time_ms));
            if row.time_ms >= end_ms {
                reached_end = true;
                break;
            }
            out.push(row);
        }
        if reached_end || length < REST_LIMIT {
            return Ok(());
        }
        from_id = Some(
            previous
                .context("full Binance aggregate page has no cursor")?
                .0
                .checked_add(1)
                .context("Binance aggregate ID overflow")?,
        );
    }
}

pub async fn execution_history(
    client: &Client,
    instrument: &Instrument,
    start_ms: i64,
    end_ms: i64,
) -> Result<ExecutionPage> {
    endpoint(instrument)?;
    ensure!(
        start_ms >= 0 && end_ms > start_ms,
        "invalid Binance execution history range"
    );
    let now_ms = flow::unix_millis();
    let historical_end = end_ms.min(now_ms);
    let retention_start = now_ms.saturating_sub(FUTURES_RETENTION_MS);
    let today = now_ms.div_euclid(DAY_MS) * DAY_MS;
    let mut rows = Vec::new();
    let mut ranges = Vec::new();
    let mut cursor = start_ms;
    while cursor < historical_end {
        let day = cursor.div_euclid(DAY_MS) * DAY_MS;
        let end = day.saturating_add(DAY_MS).min(historical_end);
        let archive_error = if day < today {
            match daily_archive(client, instrument, day, cursor, end).await {
                Ok(archive_rows) => {
                    rows.extend(archive_rows);
                    ranges.push(coverage(cursor, end, None));
                    cursor = end;
                    continue;
                }
                Err(error) => Some(format!("daily archive unavailable/invalid: {error:#}")),
            }
        } else {
            None
        };
        // Missing archives are not empty days. Futures REST cannot prove a day
        // older than its documented rolling 48-hour retention boundary.
        if instrument.product == Product::Perpetual && cursor < retention_start {
            let unavailable_end = end.min(retention_start);
            ranges.push(coverage(
                cursor,
                unavailable_end,
                Some(format!(
                    "Binance futures REST retains only 48 hours; {}",
                    archive_error
                        .as_deref()
                        .unwrap_or("daily archive not yet published")
                )),
            ));
            cursor = unavailable_end;
        }
        while cursor < end {
            let window_end = cursor.saturating_add(HOUR_MS).min(end);
            let reason = rest_window(client, instrument, cursor, window_end, &mut rows)
                .await
                .err()
                .map(|error| {
                    format!(
                        "Binance aggregate REST replay incomplete: {error:#}{}",
                        archive_error
                            .as_ref()
                            .map_or(String::new(), |archive| format!("; {archive}"))
                    )
                });
            ranges.push(coverage(cursor, window_end, reason));
            cursor = window_end;
        }
    }
    if historical_end < end_ms {
        ranges.push(coverage(
            start_ms.max(historical_end),
            end_ms,
            Some("Binance future/current capture is not a completed historical interval".into()),
        ));
    }
    let executions = crate::executions::canonicalize(rows, start_ms, end_ms)?;
    let observed_through_ms = ranges
        .iter()
        .filter(|range| range.complete)
        .map(|range| range.end_ms)
        .chain(executions.last().map(|row| row.time_ms.saturating_add(1)))
        .max()
        .unwrap_or(start_ms);
    Ok(ExecutionPage {
        executions,
        coverage: ranges,
        observed_through_ms,
    })
}

fn live_page(execution: Execution, continuous_from: Option<i64>) -> ExecutionPage {
    let end_ms = execution.time_ms.saturating_add(1);
    // The first captured millisecond may already have unseen earlier trades.
    // Once anchored just AFTER it, consecutive aggregate IDs close subsequent
    // milliseconds only when a trade from a later millisecond is observed.
    let mut ranges = Vec::with_capacity(2);
    if let Some(start_ms) = continuous_from
        && execution.time_ms > start_ms
    {
        ranges.push(coverage(start_ms, execution.time_ms, None));
    }
    ranges.push(coverage(
        execution.time_ms,
        end_ms,
        Some(if continuous_from.is_some() {
            "Latest Binance aggregate millisecond remains open".into()
        } else {
            "First Binance aggregate observed; earlier continuity requires native replay".into()
        }),
    ));
    ExecutionPage {
        executions: vec![execution],
        coverage: ranges,
        observed_through_ms: end_ms,
    }
}

async fn emit(
    sender: &mpsc::Sender<ExecutionEvent>,
    shutdown: &mut watch::Receiver<bool>,
    event: ExecutionEvent,
) -> Result<bool> {
    tokio::select! {
        _ = shutdown.changed() => Ok(false),
        result = sender.send(event) => { result.context("Binance execution consumer closed")?; Ok(true) }
    }
}

/// One standalone native aggregate socket. Binance candle/CVD sockets carry
/// only kline totals and therefore MUST NOT register a native_producer guard.
/// The broker owns this future and drops it when its last subscriber leaves.
pub async fn execution_session(
    _client: &Client,
    instrument: &Instrument,
    sender: &mpsc::Sender<ExecutionEvent>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    let base = match endpoint(instrument)? {
        SPOT_REST => SPOT_WS,
        UM_REST => UM_WS,
        CM_REST => CM_WS,
        _ => unreachable!(),
    };
    if *shutdown.borrow() || sender.is_closed() {
        return Ok(());
    }
    let url = format!("{base}/{}@aggTrade", instrument.symbol.to_lowercase());
    let (mut socket, _) = tokio::select! {
        _ = shutdown.changed() => return Ok(()),
        _ = sender.closed() => return Ok(()),
        result = tokio::time::timeout(IO_TIMEOUT, connect_async(&url)) => result.context("Binance execution WebSocket connect timed out")??,
    };
    let message = if instrument.product == Product::Perpetual {
        "Connected to Binance native market aggregates; native quantity; excludes insurance-fund/ADL trades; q includes RPI"
    } else {
        "Connected to Binance native spot aggregates; native base quantity; no kline proxy"
    };
    if !emit(
        sender,
        &mut shutdown,
        ExecutionEvent::Status {
            connected: true,
            message: message.into(),
        },
    )
    .await?
    {
        return Ok(());
    }
    let mut previous: Option<(u64, Execution)> = None;
    let mut continuous_from: Option<i64> = None;
    loop {
        let message = tokio::select! {
            _ = shutdown.changed() => return Ok(()),
            _ = sender.closed() => return Ok(()),
            result = tokio::time::timeout(EXECUTION_IDLE_TIMEOUT, socket.next()) => result.context("Binance execution stream heartbeat timed out")?.context("Binance execution WebSocket disconnected")??,
        };
        match message {
            Message::Text(text) => {
                let value: Value = serde_json::from_str(&text)
                    .context("invalid Binance execution WebSocket JSON")?;
                match value["e"].as_str() {
                    Some("aggTrade") => {
                        ensure!(
                            value["s"].as_str() == Some(instrument.symbol.as_str()),
                            "Binance execution socket returned wrong symbol"
                        );
                        let row = aggregate(&value)?;
                        let id = native_id(&value["a"])?;
                        let mut contiguous = false;
                        if let Some((old_id, old)) = &previous {
                            if id == *old_id {
                                ensure!(old == &row, "conflicting Binance live aggregate identity");
                                continue;
                            }
                            contiguous =
                                old_id.checked_add(1) == Some(id) && row.time_ms >= old.time_ms;
                            if !contiguous {
                                let start_ms = old.time_ms.min(row.time_ms);
                                let end_ms = old.time_ms.max(row.time_ms).saturating_add(1);
                                if !emit(sender, &mut shutdown, ExecutionEvent::Invalidate { start_ms, end_ms, reason: format!("Binance aggregate sequence discontinuity {old_id} -> {id}; native replay required") }).await? { return Ok(()); }
                            }
                        }
                        if !contiguous {
                            continuous_from = Some(row.time_ms.saturating_add(1));
                        }
                        let page =
                            live_page(row.clone(), if contiguous { continuous_from } else { None });
                        if contiguous {
                            continuous_from =
                                Some(continuous_from.unwrap_or(row.time_ms).max(row.time_ms));
                        }
                        if !emit(sender, &mut shutdown, ExecutionEvent::Page(Arc::new(page)))
                            .await?
                        {
                            return Ok(());
                        }
                        previous = Some((id, row));
                    }
                    Some("serverShutdown") => bail!("Binance execution server shutting down"),
                    _ if value.get("code").is_some() || value.get("msg").is_some() => {
                        bail!("Binance execution WebSocket error: {value}")
                    }
                    _ => {}
                }
            }
            Message::Ping(payload) => {
                tokio::select! {
                    _ = shutdown.changed() => return Ok(()),
                    _ = sender.closed() => return Ok(()),
                    result = tokio::time::timeout(IO_TIMEOUT, socket.send(Message::Pong(payload))) => result.context("Binance execution pong timed out")??,
                }
            }
            Message::Pong(_) => {}
            Message::Close(frame) => bail!("Binance execution WebSocket closed: {frame:?}"),
            Message::Binary(_) => bail!("unexpected binary Binance execution WebSocket message"),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;

    fn parse_archive(
        bytes: &[u8],
        day_ms: i64,
        start_ms: i64,
        end_ms: i64,
    ) -> Result<Vec<Execution>> {
        decode_archive(bytes, day_ms, start_ms, end_ms, &AtomicBool::new(false))
    }

    #[test]
    fn dropped_archive_request_cancels_decoder() {
        let cancelled = Arc::new(AtomicBool::new(false));
        let guard = CancelDecode(cancelled.clone());
        drop(guard);
        let zip = archive("77,1,1,1,1,1735689600000,false\n");
        assert!(
            decode_archive(
                &zip,
                1735689600000,
                1735689600000,
                1735689600001,
                &cancelled
            )
            .is_err()
        );
    }

    #[tokio::test]
    async fn saturated_millisecond_pages_by_id_without_skipping_or_crossing_end() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/aggTrades", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let mut requests = Vec::new();
            for page in 0..2 {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                loop {
                    let mut buffer = [0u8; 1024];
                    let count = socket.read(&mut buffer).await.unwrap();
                    assert!(count > 0);
                    request.extend_from_slice(&buffer[..count]);
                    if request.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                requests.push(String::from_utf8(request).unwrap());
                let rows: Vec<Value> = if page == 0 {
                    (0..1000)
                        .map(|id| json!({"a":id,"T":1000,"p":"123.00000001","q":"0.00000001"}))
                        .collect()
                } else {
                    vec![
                        json!({"a":1000,"T":1000,"p":"123.00000001","q":"0.00000001"}),
                        json!({"a":1001,"T":1001,"p":"123","q":"99"}),
                    ]
                };
                let body = serde_json::to_string(&rows).unwrap();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            }
            requests
        });
        let mut rows = Vec::new();
        rest_window_at(&Client::new(), &url, "BTCUSDT", 1000, 1001, &mut rows)
            .await
            .unwrap();
        let requests = server.await.unwrap();
        assert!(requests[0].contains("startTime=1000"));
        assert!(requests[0].contains("endTime=1000"));
        assert!(!requests[0].contains("fromId"));
        assert!(requests[1].contains("fromId=1000"));
        assert!(!requests[1].contains("startTime"));
        assert!(!requests[1].contains("endTime"));
        assert_eq!(rows.len(), 1001);
        assert!(rows.iter().all(|row| row.time_ms == 1000));
        assert_eq!(
            rows.iter().map(|row| row.quantity).sum::<Decimal>(),
            Decimal::new(1001, 8)
        );
        assert_eq!(rows.last().unwrap().id, "agg:1000");
    }

    fn instrument(product: Product, inverse: bool) -> Instrument {
        Instrument {
            exchange: Exchange::Binance,
            product,
            inverse,
            symbol: if inverse { "BTCUSD_PERP" } else { "BTCUSDT" }.into(),
            base_coin: "BTC".into(),
            quote_coin: if inverse { Quote::USD } else { Quote::USDT },
            settle_coin: if inverse { "BTC" } else { "USDT" }.into(),
            price_scale: 2,
            tick_size: Some(Decimal::new(1, 2)),
            contract_size: if inverse { 100.0 } else { 1.0 },
            volume_unit: if inverse { "contracts" } else { "BTC" }.into(),
        }
    }

    fn archive(csv: &str) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        zip.start_file("trades.csv", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(csv.as_bytes()).unwrap();
        zip.finish().unwrap().into_inner()
    }

    #[test]
    fn decimal_price_and_native_quantity_survive_all_products() {
        for (product, inverse, quantity) in [
            (Product::Spot, false, "0.00000001"),
            (Product::Perpetual, false, "1000000000000000.00000001"),
            (Product::Perpetual, true, "206"),
        ] {
            let _instrument = instrument(product, inverse);
            let row = aggregate(&json!({"a":9007199254740993u64,"T":1735689600123i64,"p":"9642.40000001","q":quantity,"nq":"1","f":42,"l":44})).unwrap();
            assert_eq!(row.id, "agg:9007199254740993");
            assert_eq!(row.price.to_string(), "9642.40000001");
            assert_eq!(row.quantity, Decimal::from_str_exact(quantity).unwrap());
        }
    }

    #[test]
    fn archive_microseconds_and_rest_overlap_are_one_native_aggregate() {
        let day = 1735689600000;
        let zip = archive(
            "77,123.45000001,0.00000001,900,901,1735689600123999,false,true\n78,124,2,902,902,1735689600124000,false,true\n",
        );
        let mut rows = parse_archive(&zip, day, day + 123, day + 124).unwrap();
        assert_eq!(rows.len(), 1);
        let rest =
            aggregate(&json!({"a":77,"T":day+123,"p":"123.45000001","q":"0.00000001"})).unwrap();
        assert_eq!(rows[0], rest);
        rows.push(rest);
        let canonical = crate::executions::canonicalize(rows, day + 123, day + 124).unwrap();
        assert_eq!(canonical.len(), 1);
        assert_eq!(canonical[0].quantity, Decimal::new(1, 8));
    }

    #[test]
    fn inverse_archive_header_keeps_contracts_not_base_or_notional() {
        let day = 1591228800000;
        let zip = archive(
            "agg_trade_id,price,quantity,first_trade_id,last_trade_id,transact_time,is_buyer_maker\n416690,9642.4,3,595259,595259,1591250548649,false\n",
        );
        let rows = parse_archive(&zip, day, day, day + DAY_MS).unwrap();
        assert_eq!(rows[0].quantity, Decimal::new(3, 0));
        assert_eq!(rows[0].price, Decimal::new(96424, 1));
        assert_eq!(rows[0].id, "agg:416690");
        assert_ne!(rows[0].id, "raw:595259");
    }

    #[test]
    fn malformed_archive_never_proves_empty_coverage() {
        let day = 1735689600000;
        for csv in [
            "77,1,1,1,1,1735689600000,false\n78,NaN,1,2,2,1735689601000,false\n",
            "77,1,1,2,1,1735689600000,false\n",
            "77,1,1,1,1,1735776000000000,false\n",
            "77,1,1,1,1,1735689600000,false\n77,1,1,1,1,1735689600000,false\n",
            "77,1,1,1,1,1735689600000,false\n79,1,1,3,3,1735689600001,false\n",
            "not_a_valid_header,price,quantity\n",
        ] {
            assert!(parse_archive(&archive(csv), day, day, day + 1).is_err());
        }
    }

    #[test]
    fn daily_archive_routes_native_products() {
        for (product, inverse, expected) in [
            (Product::Spot, false, "spot"),
            (Product::Perpetual, false, "futures/um"),
            (Product::Perpetual, true, "futures/cm"),
        ] {
            let instrument = instrument(product, inverse);
            assert_eq!(
                archive_url(&instrument, 1735689600000).unwrap(),
                format!(
                    "{ARCHIVE_ROOT}/{expected}/daily/aggTrades/{0}/{0}-aggTrades-2025-01-01.zip",
                    instrument.symbol
                )
            );
        }
    }

    #[test]
    fn live_continuity_closes_only_previous_milliseconds() {
        let a = aggregate(&json!({"a":7,"T":1000,"p":"1","q":"2"})).unwrap();
        let b = aggregate(&json!({"a":8,"T":1002,"p":"1","q":"3"})).unwrap();
        let page = live_page(b, Some(a.time_ms + 1));
        assert_eq!(page.coverage[0], coverage(1001, 1002, None));
        assert!(!page.coverage[1].complete);
        assert_eq!(page.coverage[1].start_ms, 1002);
        assert_eq!(page.coverage[1].end_ms, 1003);
        assert!(!live_page(a, None).coverage[0].complete);
    }

    #[test]
    fn invalid_native_numbers_and_ids_are_not_synthesized() {
        for row in [
            json!({"T":1,"p":"1","q":"1"}),
            json!({"a":-1,"T":1,"p":"1","q":"1"}),
            json!({"a":1,"T":-1,"p":"1","q":"1"}),
            json!({"a":1,"T":1,"p":"0","q":"1"}),
            json!({"a":1,"T":1,"p":"1","q":"-1"}),
            json!({"a":1,"T":1,"p":"1","q":"0.00000000000000000000000000001"}),
        ] {
            assert!(aggregate(&row).is_err());
        }
        assert_eq!(milliseconds(1735689600123999).unwrap(), 1735689600123);
        assert_eq!(milliseconds(1735689600123).unwrap(), 1735689600123);
    }
}
