//! Priced MEXC executions, retaining spot base quantities and futures contracts.
//! REST spot aggregate IDs and protobuf trade IDs have no documented equivalence:
//! https://www.mexc.com/api-docs/spot-v3/market-data-endpoints/compressedaggregate-trades-list
//! https://github.com/mexcdevelop/websocket-proto/blob/main/PublicAggreDealsV3Api.proto
//! Never infer continuity between those namespaces, or from an absent recent-deals marker.
use std::{collections::HashMap, sync::Arc};

use mexc_model::executions::{Coverage, Execution, ExecutionEvent, ExecutionPage};

use super::*;

const SPOT_WS: &str = "wss://wbs-api.mexc.com/ws";
const SPOT_LIVE_REASON: &str = "MEXC protobuf aggregate capture is observed-only; protobuf tradeId is not proven equivalent to REST aggregate a";
const FUTURES_REASON: &str = "MEXC futures exposes only recent deals and observed capture, not pageable deeper execution history";

pub(super) fn native_tick(value: &Value) -> Option<NativeDecimal> {
    json_decimal(value)
        .ok()
        .filter(|v| *v > NativeDecimal::ZERO)
}

fn positive(value: &Value) -> Result<NativeDecimal> {
    let value = json_decimal(value)?;
    ensure!(
        value > NativeDecimal::ZERO,
        "nonpositive MEXC execution decimal"
    );
    Ok(value)
}

fn milliseconds(value: &Value) -> Result<i64> {
    let value = serde_json::from_value::<Timestamp>(value.clone())?.milliseconds()?;
    ensure!(value > 0, "invalid MEXC execution timestamp");
    Ok(value)
}

fn futures_execution(row: &Value) -> Result<Execution> {
    Ok(Execution {
        time_ms: milliseconds(&row["t"])?,
        id: format!(
            "raw:{}",
            native_id(&row["i"]).context("MEXC futures execution has no native i ID")?
        ),
        price: positive(&row["p"])?,
        // v is the native number of contracts, including on inverse contracts.
        quantity: positive(&row["v"])?,
    })
}

fn rest_execution(row: &Value) -> Result<Execution> {
    Ok(Execution {
        time_ms: milliseconds(&row["T"])?,
        id: format!(
            "agg:rest:{}",
            native_id(&row["a"]).context("MEXC aggregate execution has no native a ID")?
        ),
        price: positive(&row["p"])?,
        quantity: positive(&row["q"])?,
    })
}

fn spot_execution(deal: &PbDeal) -> Result<Execution> {
    ensure!(
        !deal.trade_id.is_empty(),
        "MEXC protobuf execution has no native tradeId"
    );
    Ok(Execution {
        time_ms: milliseconds(&json!(deal.time))?,
        id: format!("agg:pb:{}", deal.trade_id),
        price: positive(&Value::String(deal.price.clone()))?,
        quantity: positive(&Value::String(deal.quantity.clone()))?,
    })
}

fn point_page(execution: Execution, reason: &str) -> ExecutionPage {
    let time_ms = execution.time_ms;
    ExecutionPage {
        executions: vec![execution],
        coverage: vec![Coverage {
            start_ms: time_ms,
            end_ms: time_ms.saturating_add(1),
            complete: false,
            reason: Some(reason.to_owned()),
        }],
        observed_through_ms: time_ms.saturating_add(1),
    }
}

fn observe(instrument: &Instrument, execution: Result<Execution>, time_ms: Option<i64>) {
    match execution {
        Ok(execution) => crate::executions::observe(instrument, execution),
        Err(error) => {
            let time_ms = time_ms.unwrap_or_else(|| unix_seconds().saturating_mul(1000));
            crate::executions::observe_gap(
                instrument,
                time_ms,
                time_ms.saturating_add(1),
                format!("MEXC priced execution unavailable: {error:#}"),
            );
        }
    }
}

pub(super) fn observe_futures(instrument: &Instrument, row: &Value) {
    observe(
        instrument,
        futures_execution(row),
        milliseconds(&row["t"]).ok(),
    );
}

pub(super) fn observe_spot(instrument: &Instrument, deal: &PbDeal) {
    observe(
        instrument,
        spot_execution(deal),
        milliseconds(&json!(deal.time)).ok(),
    );
}

fn unavailable(start_ms: i64, end_ms: i64, reason: impl Into<String>) -> ExecutionPage {
    ExecutionPage {
        executions: Vec::new(),
        coverage: vec![Coverage {
            start_ms,
            end_ms,
            complete: false,
            reason: Some(reason.into()),
        }],
        observed_through_ms: start_ms,
    }
}

// Retain an identity once, rejecting both values on a conflict. A conflicting
// price/size must not silently select the first or sum two versions of one fill.
#[derive(Default)]
struct Rows {
    executions: HashMap<String, Option<Execution>>,
    problems: Vec<String>,
}
impl Rows {
    fn ingest(&mut self, execution: Result<Execution>, start_ms: i64, end_ms: i64) {
        match execution {
            Ok(execution) if execution.time_ms >= start_ms && execution.time_ms < end_ms => {
                use std::collections::hash_map::Entry;
                match self.executions.entry(execution.id.clone()) {
                    Entry::Vacant(entry) => {
                        entry.insert(Some(execution));
                    }
                    Entry::Occupied(mut entry) => {
                        if entry.get().as_ref() != Some(&execution) {
                            self.problems.push(format!(
                                "conflicting MEXC native execution {}",
                                execution.id
                            ));
                            entry.insert(None);
                        }
                    }
                }
            }
            Ok(_) => {}
            Err(error) => self
                .problems
                .push(format!("unknown ID or invalid native execution: {error:#}")),
        }
    }
    fn finish(self, start_ms: i64, end_ms: i64, mut reasons: Vec<String>) -> ExecutionPage {
        reasons.extend(self.problems);
        let mut executions: Vec<_> = self.executions.into_values().flatten().collect();
        executions.sort_unstable_by(|a, b| (a.time_ms, &a.id).cmp(&(b.time_ms, &b.id)));
        let observed_through_ms = if reasons.is_empty() {
            end_ms
        } else {
            executions
                .last()
                .map_or(start_ms, |row| row.time_ms.saturating_add(1))
        };
        ExecutionPage {
            executions,
            coverage: vec![Coverage {
                start_ms,
                end_ms,
                complete: reasons.is_empty(),
                reason: if reasons.is_empty() {
                    None
                } else {
                    Some(reasons.join("; "))
                },
            }],
            observed_through_ms,
        }
    }
}

// Half-open millisecond splits avoid inclusive endpoint duplicates. MEXC has no
// documented fromId cursor; a saturated single timestamp is explicitly partial.
fn split_range(start_ms: i64, end_ms: i64) -> Option<((i64, i64), (i64, i64))> {
    let width = end_ms.checked_sub(start_ms)?;
    (width > 1).then(|| {
        let middle = start_ms + width / 2;
        ((start_ms, middle), (middle, end_ms))
    })
}

pub async fn execution_history(
    client: &reqwest::Client,
    instrument: &Instrument,
    start_ms: i64,
    end_ms: i64,
) -> Result<ExecutionPage> {
    ensure!(
        start_ms >= 0 && end_ms > start_ms,
        "invalid MEXC execution history range"
    );
    ensure!(
        instrument.exchange == Exchange::Mexc,
        "not a MEXC instrument"
    );
    let mut rows = Rows::default();
    let observed_at = flow::unix_millis();
    if instrument.product == Product::Perpetual {
        let response: Result<Value> = get_json(
            client,
            &format!("{FUTURES_REST}/deals/{}", instrument.symbol),
            &[("limit", "100".to_owned())],
        )
        .await;
        return Ok(match response {
            Ok(response) if response["success"].as_bool() == Some(true) => {
                let Some(deals) = response["data"].as_array() else {
                    return Ok(unavailable(start_ms, end_ms, FUTURES_REASON));
                };
                for row in deals {
                    rows.ingest(futures_execution(row), start_ms, end_ms);
                }
                rows.finish(start_ms, end_ms, vec![FUTURES_REASON.to_owned()])
            }
            Ok(_) => unavailable(
                start_ms,
                end_ms,
                format!("{FUTURES_REASON}; recent-deals endpoint rejected request"),
            ),
            Err(error) => unavailable(
                start_ms,
                end_ms,
                format!("{FUTURES_REASON}; recent-deals unavailable: {error:#}"),
            ),
        });
    }
    let mut pending = vec![(start_ms, end_ms)];
    let mut reasons = if end_ms > observed_at {
        vec!["Requested MEXC execution range extends beyond capture time".to_owned()]
    } else {
        Vec::new()
    };
    while let Some((start, end)) = pending.pop() {
        let response: Result<Vec<Value>> = get_json(
            client,
            &format!("{SPOT_REST}/aggTrades"),
            &[
                ("symbol", instrument.symbol.clone()),
                ("startTime", start.to_string()),
                ("endTime", (end - 1).to_string()),
                ("limit", "1000".to_owned()),
            ],
        )
        .await;
        let page = match response {
            Ok(page) => page,
            Err(error) => {
                reasons.push(format!(
                    "MEXC spot aggregate replay unavailable/disabled for [{start},{end}): {error:#}"
                ));
                // Do not hammer a disabled endpoint for the remaining subranges.
                break;
            }
        };
        if page
            .iter()
            .any(|row| milliseconds(&row["T"]).is_ok_and(|time| time < start || time >= end))
        {
            reasons.push(format!(
                "MEXC aggregate endpoint did not respect requested bounds [{start},{end})"
            ));
            break;
        }
        if page.len() >= 1000 {
            if let Some((left, right)) = split_range(start, end) {
                pending.push(right);
                pending.push(left);
                continue;
            }
            reasons.push(format!(
                "MEXC aggregate timestamp saturated at {start}; no documented ID paging cursor"
            ));
        }
        for row in page {
            rows.ingest(rest_execution(&row), start, end);
        }
    }
    // Conditional REST replay can cover a historical window; it cannot certify
    // a splice to protobuf capture. The namespace distinction is retained in IDs.
    Ok(rows.finish(start_ms, end_ms, reasons))
}

async fn emit(sender: &mpsc::Sender<ExecutionEvent>, event: ExecutionEvent) -> Result<()> {
    sender
        .send(event)
        .await
        .context("execution consumer closed")
}

async fn emit_execution(
    sender: &mpsc::Sender<ExecutionEvent>,
    execution: Result<Execution>,
    time_ms: Option<i64>,
    reason: &str,
) -> Result<()> {
    match execution {
        Ok(execution) => {
            emit(
                sender,
                ExecutionEvent::Page(Arc::new(point_page(execution, reason))),
            )
            .await
        }
        Err(error) => {
            let time_ms = time_ms.unwrap_or_else(|| unix_seconds().saturating_mul(1000));
            emit(
                sender,
                ExecutionEvent::Invalidate {
                    start_ms: time_ms,
                    end_ms: time_ms.saturating_add(1),
                    reason: format!("MEXC unknown ID or invalid priced execution: {error:#}"),
                },
            )
            .await
        }
    }
}

/// One native session. The shared execution service owns reconnect and switches
/// this socket off when an existing candle/CVD producer supplies the native tape.
pub async fn execution_session(
    _client: &reqwest::Client,
    instrument: &Instrument,
    sender: &mpsc::Sender<ExecutionEvent>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    ensure!(
        instrument.exchange == Exchange::Mexc,
        "not a MEXC instrument"
    );
    if *shutdown.borrow() {
        return Ok(());
    }
    let spot = instrument.product == Product::Spot;
    let (socket, _) = tokio::select! {
        result = connect_async(if spot { SPOT_WS } else { FUTURES_WS }) => result.context("connecting MEXC execution websocket")?,
        _ = shutdown.changed() => return Ok(()),
    };
    let (mut write, mut read) = socket.split();
    let subscription = if spot {
        json!({"method":"SUBSCRIPTION","params":[format!("spot@public.aggre.deals.v3.api.pb@100ms@{}", instrument.symbol)]})
    } else {
        json!({"method":"sub.deal","param":{"symbol":instrument.symbol},"gzip":false,"compress":false})
    };
    write
        .send(Message::Text(subscription.to_string().into()))
        .await
        .context("subscribing to MEXC priced executions")?;
    let reason = if spot {
        SPOT_LIVE_REASON
    } else {
        FUTURES_REASON
    };
    let mut confirmed = false;
    let mut heartbeat = tokio::time::interval(Duration::from_secs(15));
    loop {
        tokio::select! {
            _ = shutdown.changed() => { let _ = write.send(Message::Close(None)).await; return Ok(()); },
            _ = heartbeat.tick() => write.send(Message::Text(if spot { json!({"method":"PING"}) } else { json!({"method":"ping"}) }.to_string().into())).await.context("sending MEXC execution heartbeat")?,
            frame = read.next() => {
                match frame.context("MEXC execution websocket disconnected")?? {
                    Message::Binary(bytes) if spot => {
                        let wrapper = PbWrapper::decode(bytes.as_ref()).context("decoding MEXC priced protobuf executions")?;
                        if wrapper.symbol.as_deref().is_some_and(|symbol| symbol != instrument.symbol) { continue; }
                        if let Some(pb_wrapper::Body::AggDeals(deals)) = wrapper.body {
                            if !confirmed { emit(sender, ExecutionEvent::Status { connected: true, message: reason.to_owned() }).await?; confirmed = true; }
                            for deal in deals.deals {
                                emit_execution(sender, spot_execution(&deal), milliseconds(&json!(deal.time)).ok(), reason).await?;
                            }
                        }
                    }
                    Message::Text(text) => {
                        let value: Value = serde_json::from_str(&text).context("invalid MEXC execution JSON")?;
                        if !spot && value["channel"].as_str() == Some("push.deal") && value["symbol"].as_str() == Some(instrument.symbol.as_str()) {
                            if let Some(deals) = value["data"].as_array() {
                                if !confirmed { emit(sender, ExecutionEvent::Status { connected: true, message: reason.to_owned() }).await?; confirmed = true; }
                                for deal in deals { emit_execution(sender, futures_execution(deal), milliseconds(&deal["t"]).ok(), reason).await?; }
                            }
                        } else if spot && text.contains("PING") {
                            write.send(Message::Text(json!({"method":"PONG"}).to_string().into())).await?;
                        } else if value.get("code").and_then(Value::as_i64).is_some_and(|code| code != 0) || value["channel"].as_str() == Some("rs.error") {
                            bail!("MEXC execution subscription rejected: {text}");
                        }
                    }
                    Message::Ping(data) => write.send(Message::Pong(data)).await?,
                    Message::Close(frame) => bail!("MEXC execution websocket closed: {frame:?}"),
                    _ => {},
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_native_price_and_contract_size_do_not_require_a_side() {
        let row: Value = serde_json::from_str(r#"{"p":12345.1234567890123456789,"v":987654321.000000000001,"i":"7","t":1700000000000,"T":0}"#).unwrap();
        let execution = futures_execution(&row).unwrap();
        assert_eq!(
            execution.price,
            flow::amount("12345.1234567890123456789").unwrap()
        );
        assert_eq!(
            execution.quantity,
            flow::amount("987654321.000000000001").unwrap()
        );
        assert_eq!(execution.id, "raw:7");
    }

    #[test]
    fn protobuf_price_and_base_size_survive_unknown_side_and_namespace_is_distinct() {
        let deal = PbDeal {
            price: "0.000000123456789".into(),
            quantity: "12.34567890123456789".into(),
            trade_type: 0,
            time: 1700000000000,
            trade_id: "42".into(),
        };
        let live = spot_execution(&deal).unwrap();
        let replay =
            rest_execution(&json!({"p":deal.price,"q":deal.quantity,"a":"42","T":deal.time}))
                .unwrap();
        assert_eq!(live.price, replay.price);
        assert_eq!(live.quantity, replay.quantity);
        assert_ne!(live.id, replay.id);
        assert!(!point_page(live, SPOT_LIVE_REASON).coverage[0].complete);
    }

    #[test]
    fn unknown_id_is_an_explicit_hole_not_a_synthetic_execution() {
        let mut rows = Rows::default();
        rows.ingest(
            rest_execution(&json!({"p":"1.1","q":"2","a":null,"T":1700000000000_i64})),
            1700000000000,
            1700000000001,
        );
        let page = rows.finish(1700000000000, 1700000000001, Vec::new());
        assert!(page.executions.is_empty());
        assert!(!page.coverage[0].complete);
        assert!(
            page.coverage[0]
                .reason
                .as_ref()
                .unwrap()
                .contains("unknown ID")
        );
    }

    #[test]
    fn duplicates_are_idempotent_but_conflicts_remove_both_versions() {
        let first =
            futures_execution(&json!({"p":"10","v":"2","i":"7","t":1700000000000_i64})).unwrap();
        let mut rows = Rows::default();
        rows.ingest(Ok(first.clone()), first.time_ms, first.time_ms + 1);
        rows.ingest(Ok(first.clone()), first.time_ms, first.time_ms + 1);
        let mut conflict = first.clone();
        conflict.price += NativeDecimal::ONE;
        rows.ingest(Ok(conflict), first.time_ms, first.time_ms + 1);
        let page = rows.finish(first.time_ms, first.time_ms + 1, Vec::new());
        assert!(page.executions.is_empty());
        assert!(!page.coverage[0].complete);
    }

    #[test]
    fn replay_paging_is_half_open_and_saturated_millisecond_cannot_be_split() {
        assert_eq!(split_range(1000, 1003), Some(((1000, 1001), (1001, 1003))));
        assert_eq!(split_range(1000, 1001), None);
        let execution =
            futures_execution(&json!({"p":"1","v":"1","i":"7","t":1700000000000_i64})).unwrap();
        let mut rows = Rows::default();
        rows.ingest(
            Ok(execution.clone()),
            execution.time_ms - 1,
            execution.time_ms,
        );
        assert!(
            rows.finish(execution.time_ms - 1, execution.time_ms, Vec::new())
                .executions
                .is_empty()
        );
    }

    #[test]
    fn native_tick_uses_only_positive_authoritative_price_unit() {
        assert_eq!(
            native_tick(&json!("0.00000000000025")),
            Some(flow::amount("0.00000000000025").unwrap())
        );
        assert_eq!(native_tick(&Value::Null), None);
        assert_eq!(native_tick(&json!("0")), None);
        assert_eq!(native_tick(&json!("-1")), None);
    }

    #[test]
    fn recent_marker_is_required_and_ordering_must_be_proven() {
        let response = json!({"success":true,"data":[
            {"i":"9","t":1700000000002_i64,"p":"12","v":"2"},
            {"i":"8","t":1700000000001_i64,"p":"11","v":"3"},
            {"i":"7","t":1700000000000_i64,"p":"10","v":"4"}
        ]});
        assert!(futures_recovery_rows(&response, "6").is_none());
        let recovered = futures_recovery_rows(&response, "7").unwrap();
        assert_eq!(recovered.len(), 2);
        assert_eq!(futures_execution(&recovered[0]).unwrap().id, "raw:8");
        assert_eq!(
            futures_execution(&recovered[1]).unwrap().quantity,
            NativeDecimal::from(2)
        );
        let mut unordered = response.clone();
        unordered["data"][1]["t"] = json!(1700000000003_i64);
        assert!(futures_recovery_rows(&unordered, "7").is_none());
        assert!(!unavailable(1, 2, FUTURES_REASON).coverage[0].complete);
    }

    #[test]
    fn proved_replay_and_observed_only_pages_keep_distinct_exclusive_boundaries() {
        let complete = Rows::default().finish(100, 200, Vec::new());
        assert!(complete.coverage[0].complete);
        assert_eq!(complete.observed_through_ms, 200);
        let row = Execution {
            time_ms: 150,
            id: "agg:rest:7".into(),
            price: NativeDecimal::ONE,
            quantity: NativeDecimal::ONE,
        };
        let observed = point_page(row.clone(), SPOT_LIVE_REASON);
        assert_eq!(observed.observed_through_ms, 151);
        assert!(!observed.coverage[0].complete);
        let mut rows = Rows::default();
        rows.ingest(Ok(row), 100, 200);
        let partial = rows.finish(100, 200, vec![FUTURES_REASON.into()]);
        assert!(!partial.coverage[0].complete);
        assert_eq!(partial.observed_through_ms, 151);
    }
}
