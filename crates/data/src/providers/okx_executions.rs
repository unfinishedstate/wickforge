//! Native OKX tape: raw trade IDs, decimal prices and unconverted sz quantities.
//! https://www.okx.com/docs-v5/en/#order-book-trading-market-data-get-trades-history
//! https://www.okx.com/docs-v5/en/#order-book-trading-market-data-ws-all-trades-channel
//! The ordinary `trades` channel aggregates fills; only `trades-all` is used here.
use std::{
    cmp::Ordering,
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering as AtomicOrdering},
    },
};

use anyhow::ensure;
use mexc_model::executions::{Coverage, Execution, ExecutionEvent, ExecutionPage};

use super::*;

const DAY_MS: i64 = 86_400_000;
const TZ8_MS: i64 = 28_800_000;
const OBSERVED_ONLY: &str =
    "OKX trades-all capture is observed-only until complete native replay proves the interval";
const MAX_ARCHIVE_BYTES: usize = 512 * 1024 * 1024;
const MAX_EXECUTIONS: usize = 2_000_000;

fn positive(text: &str) -> Result<Decimal> {
    let value = flow::amount(text)?;
    ensure!(value > Decimal::ZERO, "non-positive OKX execution decimal");
    Ok(value)
}

fn raw_id(text: &str) -> Result<String> {
    ensure!(
        !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()),
        "invalid OKX raw trade ID"
    );
    Ok(format!("raw:{text}"))
}

fn timestamp(text: &str) -> Result<i64> {
    let time_ms = text
        .parse::<i64>()
        .context("invalid OKX execution timestamp")?;
    ensure!(time_ms >= 0, "negative OKX execution timestamp");
    Ok(time_ms)
}

pub(super) fn parse_trade(row: &Value) -> Result<Execution> {
    Ok(Execution {
        time_ms: timestamp(
            row.get("ts")
                .and_then(Value::as_str)
                .context("OKX execution lacks ts")?,
        )?,
        id: raw_id(
            row.get("tradeId")
                .and_then(Value::as_str)
                .context("OKX execution lacks tradeId")?,
        )?,
        price: positive(
            row.get("px")
                .and_then(Value::as_str)
                .context("OKX execution lacks px")?,
        )?,
        // sz is base currency on spot and contracts on ALL swaps, including inverse.
        // Contract face value and execution-price base conversions belong to CVD only.
        quantity: positive(
            row.get("sz")
                .and_then(Value::as_str)
                .context("OKX execution lacks sz")?,
        )?,
    })
}

struct Tape {
    rows: HashMap<String, Option<Execution>>,
    // A malformed or conflicting row prevents claiming completeness, but does not
    // discard unrelated real executions already fetched in the same request.
    problem: Option<String>,
    limit: usize,
}

impl Default for Tape {
    fn default() -> Self {
        Self {
            rows: HashMap::new(),
            problem: None,
            limit: MAX_EXECUTIONS,
        }
    }
}

impl Tape {
    fn insert(&mut self, row: Execution, start_ms: i64, end_ms: i64) -> Result<()> {
        if row.time_ms < start_ms || row.time_ms >= end_ms {
            return Ok(());
        }
        self.admit(&row.id)?;
        use std::collections::hash_map::Entry;
        match self.rows.entry(row.id.clone()) {
            Entry::Vacant(entry) => {
                entry.insert(Some(row));
            }
            Entry::Occupied(mut entry) => {
                if entry.get().as_ref() != Some(&row) {
                    self.problem.get_or_insert_with(|| {
                        format!("Conflicting OKX raw trade identity {}", row.id)
                    });
                    entry.insert(None);
                }
            }
        }
        Ok(())
    }

    async fn merge(&mut self, other: Tape, start_ms: i64, end_ms: i64) -> Result<()> {
        if self.problem.is_none() {
            self.problem = other.problem;
        }
        for (index, (id, row)) in other.rows.into_iter().enumerate() {
            if index % 1024 == 0 {
                // Decode runs off-thread; large map merges must also leave the
                // runtime able to cancel a superseded history request.
                tokio::task::yield_now().await;
            }
            if let Some(row) = row {
                self.insert(row, start_ms, end_ms)?;
            } else {
                self.admit(&id)?;
                self.rows.insert(id, None);
            }
        }
        Ok(())
    }

    fn admit(&mut self, id: &str) -> Result<()> {
        if self.rows.len() >= self.limit && !self.rows.contains_key(id) {
            let error = format!(
                "OKX decoded execution request limit ({}) exceeded",
                self.limit
            );
            self.problem.get_or_insert_with(|| error.clone());
            anyhow::bail!(error);
        }
        Ok(())
    }

    fn finish(
        self,
        start_ms: i64,
        end_ms: i64,
        mut complete: Vec<(i64, i64)>,
        reason: String,
    ) -> ExecutionPage {
        let reason = if let Some(problem) = self.problem {
            complete.clear();
            format!("{problem}; {reason}")
        } else {
            reason
        };
        let coverage = partition_coverage(start_ms, end_ms, &complete, &reason);
        let mut executions: Vec<_> = self.rows.into_values().flatten().collect();
        executions.sort_unstable_by(|a, b| (a.time_ms, &a.id).cmp(&(b.time_ms, &b.id)));
        let observed_through_ms = coverage
            .iter()
            .filter(|span| span.complete)
            .map(|span| span.end_ms)
            .chain(executions.last().map(|row| row.time_ms.saturating_add(1)))
            .max()
            .unwrap_or(start_ms);
        ExecutionPage {
            executions,
            coverage,
            observed_through_ms,
        }
    }
}

fn partition_coverage(
    start_ms: i64,
    end_ms: i64,
    complete: &[(i64, i64)],
    reason: &str,
) -> Vec<Coverage> {
    let mut spans: Vec<_> = complete
        .iter()
        .map(|&(start, end)| (start.max(start_ms), end.min(end_ms)))
        .filter(|(start, end)| start < end)
        .collect();
    spans.sort_unstable();
    let mut merged: Vec<(i64, i64)> = Vec::new();
    for (start, end) in spans {
        if let Some(last) = merged.last_mut().filter(|last| start <= last.1) {
            last.1 = last.1.max(end);
        } else {
            merged.push((start, end));
        }
    }
    let mut coverage = Vec::new();
    let mut cursor = start_ms;
    for (start, end) in merged {
        if cursor < start {
            coverage.push(Coverage {
                start_ms: cursor,
                end_ms: start,
                complete: false,
                reason: Some(reason.to_owned()),
            });
        }
        coverage.push(Coverage {
            start_ms: start,
            end_ms: end,
            complete: true,
            reason: None,
        });
        cursor = end;
    }
    if cursor < end_ms {
        coverage.push(Coverage {
            start_ms: cursor,
            end_ms,
            complete: false,
            reason: Some(reason.to_owned()),
        });
    }
    coverage
}

fn archive_day(time_ms: i64) -> i64 {
    (time_ms + TZ8_MS).div_euclid(DAY_MS) * DAY_MS - TZ8_MS
}

fn archive_records(
    input: impl Read,
    symbol: &str,
    tape: &mut Tape,
    start_ms: i64,
    end_ms: i64,
    cancelled: &AtomicBool,
) -> Result<()> {
    ensure!(
        !cancelled.load(AtomicOrdering::Relaxed),
        "OKX archive decode cancelled"
    );
    let mut reader = csv::ReaderBuilder::new().flexible(true).from_reader(input);
    let headers = reader.headers()?.clone();
    let instrument_idx = header_index(&headers, &["instrument_name", "instId", "合约名称"])?;
    let id_idx = header_index(&headers, &["trade_id", "tradeId", "成交ID"])?;
    let price_idx = header_index(&headers, &["price", "px", "成交价格"])?;
    let size_idx = header_index(&headers, &["size", "sz", "成交数量"])?;
    let time_idx = header_index(&headers, &["created_time", "ts", "成交时间"])?;
    for row in reader.records() {
        ensure!(
            !cancelled.load(AtomicOrdering::Relaxed),
            "OKX archive decode cancelled"
        );
        let row = row?;
        if row.get(instrument_idx) != Some(symbol) {
            continue;
        }
        let time_ms = timestamp(row.get(time_idx).context("OKX archive lacks timestamp")?)?;
        if time_ms < start_ms || time_ms >= end_ms {
            continue;
        }
        tape.insert(
            Execution {
                time_ms,
                id: raw_id(row.get(id_idx).context("OKX archive lacks trade ID")?)?,
                price: positive(row.get(price_idx).context("OKX archive lacks price")?)?,
                quantity: positive(row.get(size_idx).context("OKX archive lacks size")?)?,
            },
            start_ms,
            end_ms,
        )?;
    }
    Ok(())
}

fn archive_zip(
    bytes: &[u8],
    symbol: &str,
    tape: &mut Tape,
    start_ms: i64,
    end_ms: i64,
    cancelled: &AtomicBool,
) -> Result<()> {
    ensure!(
        !cancelled.load(AtomicOrdering::Relaxed),
        "OKX archive decode cancelled"
    );
    let mut zip = ZipArchive::new(Cursor::new(bytes)).context("opening OKX priced trade ZIP")?;
    let mut csv_count = 0;
    for index in 0..zip.len() {
        ensure!(
            !cancelled.load(AtomicOrdering::Relaxed),
            "OKX archive decode cancelled"
        );
        let mut file = zip.by_index(index)?;
        if file.name().to_ascii_lowercase().ends_with(".csv") {
            csv_count += 1;
            archive_records(&mut file, symbol, tape, start_ms, end_ms, cancelled)?;
        }
    }
    ensure!(csv_count > 0, "OKX trade ZIP contains no CSV");
    Ok(())
}

struct CancelDecode(Arc<AtomicBool>);

impl Drop for CancelDecode {
    fn drop(&mut self) {
        self.0.store(true, AtomicOrdering::Relaxed);
    }
}

async fn priced_archive(
    client: &Client,
    url: &str,
    symbol: &str,
    start_ms: i64,
    end_ms: i64,
) -> Result<Tape> {
    let download = async {
        let mut response = client.get(url).send().await?.error_for_status()?;
        ensure!(
            response
                .content_length()
                .is_none_or(|length| length <= MAX_ARCHIVE_BYTES as u64),
            "OKX archive exceeds download limit"
        );
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .context("reading OKX priced archive")?
        {
            ensure!(
                chunk.len() <= MAX_ARCHIVE_BYTES.saturating_sub(bytes.len()),
                "OKX archive exceeds download limit"
            );
            bytes.extend_from_slice(&chunk);
        }
        Ok::<_, anyhow::Error>(bytes)
    };
    let bytes = tokio::time::timeout(Duration::from_secs(120), download)
        .await
        .context("OKX archive download timed out")??;
    let cancelled = Arc::new(AtomicBool::new(false));
    let _cancel_on_drop = CancelDecode(cancelled.clone());
    let symbol = symbol.to_owned();
    tokio::task::spawn_blocking(move || {
        let mut tape = Tape::default();
        archive_zip(&bytes, &symbol, &mut tape, start_ms, end_ms, &cancelled)?;
        Ok(tape)
    })
    .await
    .context("OKX priced archive decoder interrupted")?
}

async fn archive_history(
    client: &Client,
    instrument: &Instrument,
    start_ms: i64,
    end_ms: i64,
    tape: &mut Tape,
    complete: &mut Vec<(i64, i64)>,
) -> Result<()> {
    if start_ms >= end_ms {
        return Ok(());
    }
    let (inst_type, key, value) = if instrument.product == Product::Spot {
        ("SPOT", "instIdList", instrument.symbol.as_str())
    } else {
        (
            "SWAP",
            "instFamilyList",
            instrument
                .symbol
                .strip_suffix("-SWAP")
                .context("OKX swap symbol lacks -SWAP")?,
        )
    };
    let mut day = archive_day(start_ms);
    let finish = archive_day(end_ms - 1);
    let expected_filename = format!("{}-trades-", instrument.symbol);
    let mut downloaded = HashSet::new();
    while day <= finish {
        let last_day = day.saturating_add(9 * DAY_MS).min(finish);
        pace_request(&TAPE_REQUESTS, Duration::from_millis(110)).await;
        let raw: Value = get_json(
            client,
            &format!("{REST}/public/market-data-history"),
            &[
                ("module", "1".into()),
                ("instType", inst_type.into()),
                (key, value.to_owned()),
                ("dateAggrType", "daily".into()),
                ("begin", day.to_string()),
                ("end", last_day.to_string()),
            ],
        )
        .await?;
        for group in parse_envelope(raw)? {
            let details = group
                .get("details")
                .and_then(Value::as_array)
                .context("OKX archive index lacks details")?;
            for detail in details {
                let files = detail
                    .get("groupDetails")
                    .and_then(Value::as_array)
                    .context("OKX archive index lacks groupDetails")?;
                for archive in files {
                    // SWAP listings are family scoped. Do not let a sibling instrument's
                    // archive (or some other module's format) certify this instrument.
                    let filename = archive
                        .get("filename")
                        .and_then(Value::as_str)
                        .context("OKX archive lacks filename")?;
                    if !filename.starts_with(&expected_filename) || !filename.ends_with(".zip") {
                        continue;
                    }
                    let date_ms =
                        integer(archive.get("dateTs").context("OKX archive lacks dateTs")?)?;
                    ensure!(
                        archive_day(date_ms) == date_ms,
                        "OKX archive day is not UTC+8 midnight"
                    );
                    if date_ms < day
                        || date_ms > last_day
                        || !downloaded.insert(filename.to_owned())
                    {
                        continue;
                    }
                    let from = start_ms.max(date_ms);
                    let through = end_ms.min(date_ms.saturating_add(DAY_MS));
                    if from >= through {
                        continue;
                    }
                    let url = archive
                        .get("url")
                        .and_then(Value::as_str)
                        .context("OKX archive lacks URL")?;
                    let decoded = match priced_archive(
                        client,
                        url,
                        &instrument.symbol,
                        from,
                        through,
                    )
                    .await
                    {
                        Ok(decoded) => decoded,
                        Err(error) => return Err(error),
                    };
                    // No partially decoded worker state is visible to the caller.
                    tape.merge(decoded, from, through).await?;
                    // Availability alone is insufficient: every CSV must first decode.
                    complete.push((from, through));
                }
            }
        }
        day = last_day.saturating_add(DAY_MS);
    }
    Ok(())
}

// Native IDs are decimal strings, not floating-point values. Comparing lengths
// avoids imposing u64's size limit or assuming adjacent numeric IDs are consecutive.
fn compare_ids(a: &str, b: &str) -> Ordering {
    let a = a.trim_start_matches('0');
    let b = b.trim_start_matches('0');
    a.len().cmp(&b.len()).then_with(|| a.cmp(b))
}

#[derive(Default)]
struct RestCursor {
    oldest_id: Option<String>,
    oldest_ms: Option<i64>,
    crossed_start: bool,
}

impl RestCursor {
    fn consume(
        &mut self,
        rows: &[Value],
        tape: &mut Tape,
        start_ms: i64,
        end_ms: i64,
    ) -> Result<bool> {
        let mut oldest_id: Option<&str> = None;
        let mut oldest_ms = None;
        for row in rows {
            let execution = match parse_trade(row) {
                Ok(execution) => execution,
                Err(error) => {
                    tape.problem
                        .get_or_insert_with(|| format!("Invalid OKX history execution: {error:#}"));
                    return Err(error);
                }
            };
            // Keep the output half-open even if an endpoint includes its anchor.
            ensure!(
                execution.time_ms <= end_ms,
                "OKX history returned a trade newer than its upper bound"
            );
            let id = row
                .get("tradeId")
                .and_then(Value::as_str)
                .context("OKX history lacks tradeId")?;
            if let Some(previous) = &self.oldest_id {
                ensure!(
                    compare_ids(id, previous) != Ordering::Greater,
                    "OKX history moved forward across its raw ID cursor"
                );
            }
            if oldest_id.is_none_or(|old| compare_ids(id, old) == Ordering::Less) {
                oldest_id = Some(id);
            }
            oldest_ms =
                Some(oldest_ms.map_or(execution.time_ms, |old: i64| old.min(execution.time_ms)));
            self.crossed_start |= execution.time_ms < start_ms;
            tape.insert(execution, start_ms, end_ms)?;
        }
        let Some(id) = oldest_id else {
            return Ok(false);
        };
        let advanced = self
            .oldest_id
            .as_ref()
            .is_none_or(|previous| compare_ids(id, previous) == Ordering::Less);
        if advanced {
            self.oldest_id = Some(id.to_owned());
            self.oldest_ms = Some(
                self.oldest_ms
                    .map_or(oldest_ms.unwrap(), |old| old.min(oldest_ms.unwrap())),
            );
        }
        Ok(advanced)
    }

    fn proven_start(&self, start_ms: i64, end_ms: i64) -> i64 {
        if self.crossed_start {
            start_ms
        } else {
            // Do not certify the last timestamp: a page limit may split its fills.
            self.oldest_ms
                .map_or(end_ms, |old| old.saturating_add(1).clamp(start_ms, end_ms))
        }
    }
}

async fn rest_history(
    client: &Client,
    instrument: &Instrument,
    start_ms: i64,
    end_ms: i64,
    tape: &mut Tape,
    complete: &mut Vec<(i64, i64)>,
) -> Result<()> {
    let mut query = vec![
        ("instId", instrument.symbol.clone()),
        ("type", "2".into()),
        ("after", end_ms.to_string()),
        ("limit", "100".into()),
    ];
    let mut cursor = RestCursor::default();
    let result = loop {
        pace_request(&TAPE_REQUESTS, Duration::from_millis(110)).await;
        let response = async {
            let raw: Value =
                get_json(client, &format!("{REST}/market/history-trades"), &query).await?;
            parse_envelope(raw)
        }
        .await;
        let rows = match response {
            Ok(rows) => rows,
            Err(error) => break Err(error),
        };
        let advanced = match cursor.consume(&rows, tape, start_ms, end_ms) {
            Ok(advanced) => advanced,
            Err(error) => {
                tape.problem
                    .get_or_insert_with(|| format!("Invalid OKX raw replay page: {error:#}"));
                break Err(error);
            }
        };
        if cursor.crossed_start || !advanced {
            break Ok(());
        }
        // Timestamp pagination is only used once. ID cursors retain ALL fills at a
        // saturated millisecond; subtracting a millisecond would silently lose them.
        query[1].1 = "1".into();
        query[2].1 = cursor.oldest_id.as_ref().unwrap().clone();
    };
    let proven_start = cursor.proven_start(start_ms, end_ms);
    if proven_start < end_ms {
        complete.push((proven_start, end_ms));
    }
    result
}

pub async fn execution_history(
    client: &Client,
    instrument: &Instrument,
    start_ms: i64,
    end_ms: i64,
) -> Result<ExecutionPage> {
    ensure!(
        instrument.exchange == Exchange::Okx,
        "not an OKX instrument"
    );
    ensure!(
        start_ms >= 0 && start_ms < end_ms && end_ms <= i64::MAX - TZ8_MS,
        "invalid OKX execution history range"
    );
    let mut tape = Tape::default();
    let mut complete = Vec::new();
    let mut reasons = vec!["OKX REST exposes only the last three months; missing/unpublished archives, unproved boundary timestamps and future time remain incomplete".to_owned()];
    let now = flow::unix_millis();
    // Official module=1 daily archives are UTC+8 days, and current-day files are
    // not yet final. Never certify an unfinished day from an index entry alone.
    let archive_end = end_ms.min(archive_day(now));
    if let Err(error) = archive_history(
        client,
        instrument,
        start_ms,
        archive_end,
        &mut tape,
        &mut complete,
    )
    .await
    {
        reasons.push(format!("Official archive retrieval: {error:#}"));
    }
    let rest_end = end_ms.min(now);
    if start_ms < rest_end {
        let gaps = partition_coverage(start_ms, rest_end, &complete, "");
        for gap in gaps.into_iter().filter(|span| !span.complete) {
            if let Err(error) = rest_history(
                client,
                instrument,
                gap.start_ms,
                gap.end_ms,
                &mut tape,
                &mut complete,
            )
            .await
            {
                reasons.push(format!("REST raw replay: {error:#}"));
            }
        }
    }
    Ok(tape.finish(start_ms, end_ms, complete, reasons.join("; ")))
}

fn observed_page(executions: Vec<Execution>) -> Option<ExecutionPage> {
    let start_ms = executions.iter().map(|row| row.time_ms).min()?;
    let end_ms = executions
        .iter()
        .map(|row| row.time_ms.saturating_add(1))
        .max()?;
    Some(ExecutionPage {
        executions,
        coverage: vec![Coverage {
            start_ms,
            end_ms,
            complete: false,
            reason: Some(OBSERVED_ONLY.to_owned()),
        }],
        observed_through_ms: end_ms,
    })
}

pub async fn execution_session(
    _client: &Client,
    instrument: &Instrument,
    sender: &mpsc::Sender<ExecutionEvent>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    ensure!(
        instrument.exchange == Exchange::Okx,
        "not an OKX instrument"
    );
    if *shutdown.borrow() || sender.is_closed() {
        return Ok(());
    }
    let (socket, _) = tokio::select! {
        _ = shutdown.changed() => return Ok(()),
        _ = sender.closed() => return Ok(()),
        result = connect_async(WS) => result.context("connecting OKX raw executions")?,
    };
    let (mut write, mut read) = socket.split();
    write
        .send(Message::Text(
            json!({"op":"subscribe","args":[{"channel":"trades-all","instId":instrument.symbol}]})
                .to_string()
                .into(),
        ))
        .await?;
    // Do not acquire native_producer here: this session is already owned by the
    // execution broker; pretending to be an external CVD producer cancels itself.
    let mut ping_pending = false;
    loop {
        tokio::select! {
            _ = shutdown.changed() => return Ok(()),
            _ = sender.closed() => return Ok(()),
            incoming = tokio::time::timeout(Duration::from_secs(20), read.next()) => {
                match incoming {
                    Err(_) if ping_pending => bail!("OKX raw execution heartbeat timed out"),
                    Err(_) => {
                        write.send(Message::Text("ping".into())).await?;
                        ping_pending = true;
                    }
                    Ok(None) => bail!("OKX raw execution socket closed"),
                    Ok(Some(Err(error))) => return Err(error).context("reading OKX raw executions"),
                    Ok(Some(Ok(Message::Text(text)))) => {
                        ping_pending = false;
                        if text == "pong" { continue; }
                        let value: Value = serde_json::from_str(&text).context("invalid OKX raw execution JSON")?;
                        if value.get("event").and_then(Value::as_str) == Some("error") {
                            bail!("OKX raw execution subscription error: {}", value.get("msg").and_then(Value::as_str).unwrap_or("unknown"));
                        }
                        if value.pointer("/arg/channel").and_then(Value::as_str) != Some("trades-all") {
                            continue;
                        }
                        ensure!(value.pointer("/arg/instId").and_then(Value::as_str) == Some(instrument.symbol.as_str()), "OKX raw execution instrument mismatch");
                        if value.get("event").and_then(Value::as_str) == Some("subscribe") {
                            sender.send(ExecutionEvent::Status { connected: true, message: format!("OKX trades-all acknowledged; {OBSERVED_ONLY}") }).await?;
                            continue;
                        }
                        let rows = value.get("data").and_then(Value::as_array).context("OKX raw execution message lacks data")?;
                        let mut executions = Vec::with_capacity(rows.len());
                        for row in rows {
                            ensure!(row.get("instId").and_then(Value::as_str).is_none_or(|symbol| symbol == instrument.symbol.as_str()), "OKX raw execution row instrument mismatch");
                            match parse_trade(row) {
                                Ok(execution) => executions.push(execution),
                                Err(error) => {
                                    let time_ms = row.get("ts").and_then(Value::as_str).and_then(|text| timestamp(text).ok()).unwrap_or_else(flow::unix_millis);
                                    sender.send(ExecutionEvent::Invalidate { start_ms: time_ms, end_ms: time_ms.saturating_add(1), reason: format!("Invalid OKX raw execution: {error:#}") }).await?;
                                }
                            }
                        }
                        if let Some(page) = observed_page(executions) {
                            sender.send(ExecutionEvent::Page(Arc::new(page))).await?;
                        }
                    }
                    Ok(Some(Ok(Message::Ping(payload)))) => write.send(Message::Pong(payload)).await?,
                    Ok(Some(Ok(Message::Close(frame)))) => bail!("OKX raw execution socket closed: {frame:?}"),
                    Ok(Some(Ok(_))) => {}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trade(id: &str, time_ms: i64, price: &str, size: &str) -> Value {
        json!({"tradeId":id,"ts":time_ms.to_string(),"px":price,"sz":size,"side":"unknown"})
    }

    #[test]
    fn raw_trade_retains_decimal_price_native_size_and_identity_without_side() {
        let execution = parse_trade(&trade(
            "184467440737095516160",
            1_727_712_000_750,
            "63558.80000000000000000001",
            "3.146e-05",
        ))
        .unwrap();
        assert_eq!(execution.id, "raw:184467440737095516160");
        assert_eq!(
            execution.price,
            "63558.80000000000000000001".parse::<Decimal>().unwrap()
        );
        assert_eq!(execution.quantity, "0.00003146".parse::<Decimal>().unwrap());
        assert_eq!(execution.time_ms, 1_727_712_000_750);
    }

    #[test]
    fn inverse_swap_keeps_contracts_not_price_converted_base() {
        let execution = parse_trade(&json!({"instId":"BTC-USD-SWAP","tradeId":"13","ts":"1727712000750","px":"63558.8","sz":"2.5"})).unwrap();
        assert_eq!(execution.quantity, Decimal::new(25, 1));
        assert_eq!(execution.price, Decimal::new(635588, 1));
    }

    #[test]
    fn missing_ids_prices_and_nonpositive_sizes_are_rejected() {
        assert!(parse_trade(&trade("", 1000, "1", "1")).is_err());
        assert!(parse_trade(&trade("not-native", 1000, "1", "1")).is_err());
        assert!(parse_trade(&trade("1", 1000, "0", "1")).is_err());
        assert!(parse_trade(&trade("1", 1000, "1", "-1")).is_err());
        assert!(parse_trade(&trade("1", 1000, "1", "0")).is_err());
        assert!(parse_trade(&json!({"tradeId":"1","ts":"1000","sz":"1"})).is_err());
    }

    #[test]
    fn archive_retains_prices_units_filters_symbol_and_half_open_range() {
        let csv = b"instrument_name,trade_id,side,price,size,created_time\nBTC-USD-SWAP,11,sell,63558.8,2.5,1000\nBTC-USD-SWAP,12,buy,63559.1,3,1001\nBTC-USDT,13,buy,999,99,1000\nBTC-USD-SWAP,14,buy,63560,4,1002\n";
        let mut tape = Tape::default();
        archive_records(
            &csv[..],
            "BTC-USD-SWAP",
            &mut tape,
            1000,
            1002,
            &AtomicBool::new(false),
        )
        .unwrap();
        tape.insert(
            parse_trade(&trade("11", 1000, "63558.8", "2.5")).unwrap(),
            1000,
            1002,
        )
        .unwrap();
        let page = tape.finish(1000, 1002, vec![(1000, 1002)], "missing".into());
        assert_eq!(page.executions.len(), 2);
        assert_eq!(page.executions[0].id, "raw:11");
        assert_eq!(page.executions[0].price, Decimal::new(635588, 1));
        assert_eq!(
            page.executions
                .iter()
                .map(|row| row.quantity)
                .sum::<Decimal>(),
            Decimal::new(55, 1)
        );
        assert!(page.coverage[0].complete);
    }

    #[test]
    fn archive_boundaries_are_utc_plus_eight_not_utc_days() {
        assert_eq!(archive_day(1_727_712_000_750), 1_727_712_000_000);
        assert_eq!(
            archive_day(1_727_712_000_000 - 1),
            1_727_712_000_000 - DAY_MS
        );
        assert_eq!(
            archive_day(1_727_712_000_000 + DAY_MS - 1),
            1_727_712_000_000
        );
    }

    #[test]
    fn rest_id_cursor_preserves_fills_sharing_one_millisecond() {
        let mut tape = Tape::default();
        let mut cursor = RestCursor::default();
        assert!(
            cursor
                .consume(
                    &[
                        trade("104", 1000, "1.1", "0.1"),
                        trade("103", 1000, "1.2", "0.2")
                    ],
                    &mut tape,
                    1000,
                    1002
                )
                .unwrap()
        );
        assert_eq!(cursor.oldest_id.as_deref(), Some("103"));
        assert_eq!(cursor.proven_start(1000, 1002), 1001);
        assert!(
            cursor
                .consume(
                    &[
                        trade("102", 1000, "1.3", "0.3"),
                        trade("101", 999, "1.4", "0.4")
                    ],
                    &mut tape,
                    1000,
                    1002
                )
                .unwrap()
        );
        assert_eq!(cursor.proven_start(1000, 1002), 1000);
        let page = tape.finish(1000, 1002, vec![(1000, 1002)], "missing".into());
        assert_eq!(page.executions.len(), 3);
        assert_eq!(
            page.executions
                .iter()
                .map(|row| row.quantity)
                .sum::<Decimal>(),
            Decimal::new(6, 1)
        );
        assert!(page.coverage.iter().all(|span| span.complete));
    }

    #[test]
    fn empty_short_and_repeating_pages_never_prove_missing_prefix() {
        let mut tape = Tape::default();
        let mut cursor = RestCursor::default();
        assert!(!cursor.consume(&[], &mut tape, 1000, 1010).unwrap());
        assert_eq!(cursor.proven_start(1000, 1010), 1010);
        let rows = [trade("10", 1005, "1", "1")];
        assert!(cursor.consume(&rows, &mut tape, 1000, 1010).unwrap());
        assert!(!cursor.consume(&rows, &mut tape, 1000, 1010).unwrap());
        let page = tape.finish(
            1000,
            1010,
            vec![(cursor.proven_start(1000, 1010), 1010)],
            "retention or cursor exhaustion".into(),
        );
        assert_eq!(
            (
                page.coverage[0].start_ms,
                page.coverage[0].end_ms,
                page.coverage[0].complete
            ),
            (1000, 1006, false)
        );
        assert_eq!(
            (
                page.coverage[1].start_ms,
                page.coverage[1].end_ms,
                page.coverage[1].complete
            ),
            (1006, 1010, true)
        );
    }

    #[test]
    fn conflicting_overlap_removes_identity_and_invalidates_completeness() {
        let mut tape = Tape::default();
        tape.insert(
            parse_trade(&trade("1", 1000, "1.1", "2")).unwrap(),
            1000,
            1002,
        )
        .unwrap();
        tape.insert(
            parse_trade(&trade("1", 1000, "1.2", "2")).unwrap(),
            1000,
            1002,
        )
        .unwrap();
        tape.insert(
            parse_trade(&trade("1", 1000, "1.1", "2")).unwrap(),
            1000,
            1002,
        )
        .unwrap();
        let page = tape.finish(1000, 1002, vec![(1000, 1002)], "missing".into());
        assert!(page.executions.is_empty());
        assert!(!page.coverage[0].complete);
        assert!(
            page.coverage[0]
                .reason
                .as_ref()
                .unwrap()
                .contains("Conflicting")
        );
    }

    #[test]
    fn coverage_merges_archive_rest_overlap_and_retains_unavailable_gaps() {
        let coverage = partition_coverage(
            1000,
            1100,
            &[(1005, 1030), (1020, 1040), (1040, 1050), (1080, 1200)],
            "unavailable",
        );
        assert_eq!(
            coverage
                .iter()
                .map(|span| (span.start_ms, span.end_ms, span.complete))
                .collect::<Vec<_>>(),
            vec![
                (1000, 1005, false),
                (1005, 1050, true),
                (1050, 1080, false),
                (1080, 1100, true)
            ]
        );
    }

    #[test]
    fn rest_filters_inclusive_upper_anchor_and_compares_large_raw_ids() {
        let mut tape = Tape::default();
        let mut cursor = RestCursor::default();
        assert!(
            cursor
                .consume(
                    &[
                        trade("184467440737095516162", 1002, "1", "100"),
                        trade("184467440737095516161", 1001, "1.1", "0.3"),
                        trade("184467440737095516160", 999, "1.2", "100"),
                    ],
                    &mut tape,
                    1000,
                    1002
                )
                .unwrap()
        );
        assert_eq!(cursor.oldest_id.as_deref(), Some("184467440737095516160"));
        assert_eq!(cursor.proven_start(1000, 1002), 1000);
        let page = tape.finish(1000, 1002, vec![(1000, 1002)], "missing".into());
        assert_eq!(page.executions.len(), 1);
        assert_eq!(page.executions[0].quantity, Decimal::new(3, 1));
        assert!(page.coverage[0].complete);
    }

    #[test]
    fn missing_archive_price_is_not_replaced_by_candle_price() {
        let csv = b"instrument_name,trade_id,side,size,created_time\nBTC-USDT,1,buy,1,1000\n";
        assert!(
            archive_records(
                &csv[..],
                "BTC-USDT",
                &mut Tape::default(),
                1000,
                1002,
                &AtomicBool::new(false)
            )
            .is_err()
        );
    }

    #[test]
    fn malformed_rest_execution_invalidates_previously_proven_spans() {
        let mut tape = Tape::default();
        let mut cursor = RestCursor::default();
        assert!(
            cursor
                .consume(&[trade("10", 1005, "1", "1")], &mut tape, 1000, 1010)
                .unwrap()
        );
        assert!(
            cursor
                .consume(&[trade("9", 1004, "0", "1")], &mut tape, 1000, 1010)
                .is_err()
        );
        let page = tape.finish(1000, 1010, vec![(1006, 1010)], "missing".into());
        assert_eq!(page.executions.len(), 1);
        assert!(page.coverage.iter().all(|span| !span.complete));
    }

    #[test]
    fn dropping_archive_decode_guard_cancels_worker_before_parsing() {
        let cancelled = Arc::new(AtomicBool::new(false));
        let guard = CancelDecode(cancelled.clone());
        drop(guard);
        assert!(cancelled.load(AtomicOrdering::Relaxed));
        assert!(
            archive_zip(
                &[],
                "BTC-USDT",
                &mut Tape::default(),
                1000,
                1002,
                &cancelled
            )
            .unwrap_err()
            .to_string()
            .contains("cancelled")
        );
    }

    #[test]
    fn archive_decode_checks_cancellation_between_records() {
        struct CancelOnRead<'a> {
            input: &'a [u8],
            cancelled: &'a AtomicBool,
        }
        impl Read for CancelOnRead<'_> {
            fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
                let count = self.input.read(bytes)?;
                self.cancelled.store(true, AtomicOrdering::Relaxed);
                Ok(count)
            }
        }
        let cancelled = AtomicBool::new(false);
        let csv = b"instrument_name,trade_id,price,size,created_time\nBTC-USDT,1,1.1,2.5,1000\n";
        let mut tape = Tape::default();
        let reader = CancelOnRead {
            input: &csv[..],
            cancelled: &cancelled,
        };
        assert!(
            archive_records(reader, "BTC-USDT", &mut tape, 1000, 1002, &cancelled)
                .unwrap_err()
                .to_string()
                .contains("cancelled")
        );
        assert!(tape.rows.is_empty());
    }

    #[test]
    fn live_observation_does_not_fabricate_continuity() {
        let page = observed_page(vec![
            parse_trade(&trade("1", 1000, "1", "2")).unwrap(),
            parse_trade(&trade("9", 1003, "2", "3")).unwrap(),
        ])
        .unwrap();
        assert!(!page.coverage[0].complete);
        assert_eq!(page.coverage[0].start_ms, 1000);
        assert_eq!(page.coverage[0].end_ms, 1004);
        assert_eq!(page.observed_through_ms, 1004);
    }
}

#[cfg(test)]
mod budget_tests {
    use super::*;

    fn row(id: &str) -> Execution {
        Execution {
            id: format!("raw:{id}"),
            time_ms: 1000,
            price: Decimal::ONE,
            quantity: Decimal::ONE,
        }
    }

    #[tokio::test]
    async fn cumulative_archive_merge_stops_before_overflow_and_cannot_certify_omitted_rows() {
        let mut request = Tape {
            limit: 3,
            ..Default::default()
        };
        let mut first = Tape::default();
        first.insert(row("1"), 1000, 1001).unwrap();
        first.insert(row("2"), 1000, 1001).unwrap();
        request.merge(first, 1000, 1001).await.unwrap();
        let mut second = Tape::default();
        second.insert(row("3"), 1000, 1001).unwrap();
        second.insert(row("4"), 1000, 1001).unwrap();
        assert!(request.merge(second, 1000, 1001).await.is_err());
        assert_eq!(request.rows.len(), 3);
        let page = request.finish(1000, 1001, vec![(1000, 1001)], "unavailable".into());
        assert_eq!(
            page.executions
                .iter()
                .map(|row| row.quantity)
                .sum::<Decimal>(),
            Decimal::from(3)
        );
        assert!(page.coverage.iter().all(|span| !span.complete));
    }

    #[test]
    fn archive_decoder_enforces_limit_without_rejecting_duplicate_overlap() {
        let mut tape = Tape {
            limit: 1,
            ..Default::default()
        };
        tape.insert(row("1"), 1000, 1001).unwrap();
        tape.insert(row("1"), 1000, 1001).unwrap();
        let input = b"instrument_name,trade_id,price,size,created_time\nBTC-USDT,2,1,1,1000\n";
        assert!(
            archive_records(
                &input[..],
                "BTC-USDT",
                &mut tape,
                1000,
                1001,
                &AtomicBool::new(false)
            )
            .is_err()
        );
        assert_eq!(tape.rows.len(), 1);
    }
}
