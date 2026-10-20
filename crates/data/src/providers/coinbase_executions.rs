use std::{collections::HashSet, sync::Arc, time::Duration};

use anyhow::{Context, Result, bail, ensure};
use futures_util::{SinkExt, StreamExt};
use mexc_model::{
    Instrument, Product,
    executions::{Coverage, Execution, ExecutionEvent, ExecutionPage},
};
use reqwest::Client;
use serde_json::json;
use tokio::{
    sync::{mpsc, watch},
    time::{self, Instant},
};
use tokio_tungstenite::{connect_async, tungstenite::Message};

use super::cvd::{self, LiveMessage, TradePage};

const PAGE_SIZE: usize = 1_000;
const MAX_HISTORY_RECORDS: usize = 250_000;
const MAX_REPLAY_PAGES: usize = 250;
const REST_SPACING: Duration = Duration::from_millis(125);
const LIVE_WS: &str = "wss://ws-feed.exchange.coinbase.com";
const INTX_UNAVAILABLE: &str = "Coinbase International has no supported public priced execution tape; Exchange spot, portfolio fills, and candle-volume proxies are not substitutes";

#[derive(Default)]
struct Pacer(Option<Instant>);

impl Pacer {
    async fn page(
        &mut self,
        client: &Client,
        symbol: &str,
        direction: Option<&'static str>,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<TradePage> {
        if let Some(previous) = self.0 {
            time::sleep_until(previous + REST_SPACING).await;
        }
        self.0 = Some(Instant::now());
        cvd::get_trade_page(client, symbol, direction, cursor, limit).await
    }
}

fn replay_complete(from_id: u64, through_id: u64, unique_records: usize) -> bool {
    through_id.checked_sub(from_id) == u64::try_from(unique_records).ok()
}

fn adjacent_ids(newer: u64, older: u64) -> bool {
    older.checked_add(1) == Some(newer)
}

struct HistorySequence {
    expected_newest: u64,
    previous: Option<cvd::Trade>,
    complete: bool,
}

impl HistorySequence {
    fn observe(&mut self, trade: &cvd::Trade) -> Result<bool> {
        if let Some(previous) = &self.previous {
            if previous.id == trade.id {
                ensure!(
                    previous == trade,
                    "Coinbase repeated history trade ID has conflicting native values"
                );
                return Ok(false);
            }
            ensure!(
                trade.id < previous.id,
                "Coinbase history trade IDs moved forward during older pagination"
            );
            self.complete &= adjacent_ids(previous.id, trade.id);
        } else {
            self.complete &= trade.id == self.expected_newest;
        }
        self.previous = Some(trade.clone());
        Ok(true)
    }
}

struct Seek {
    cursor: String,
    expected_newest: u64,
    boundary_complete: bool,
}

fn page(
    executions: Vec<Execution>,
    start_ms: i64,
    end_ms: i64,
    observed_through_ms: i64,
    complete: bool,
    reason: Option<String>,
) -> ExecutionPage {
    ExecutionPage {
        executions,
        coverage: if start_ms < end_ms {
            vec![Coverage {
                start_ms,
                end_ms,
                complete,
                reason,
            }]
        } else {
            Vec::new()
        },
        observed_through_ms,
    }
}

// Coinbase documents CB-AFTER as the last returned record's older cursor.
// Probe IDs only to locate a right-edge anchor, then use the returned native
// pagination cursors for every replay page. A predecessor probe includes all
// records tied at the right-edge millisecond, not just one timestamp match.
async fn seek(
    client: &Client,
    symbol: &str,
    target_ms: i64,
    pacer: &mut Pacer,
) -> Result<Option<Seek>> {
    let latest = pacer.page(client, symbol, None, None, 1).await?;
    let Some(latest) = latest.trades.first() else {
        return Ok(None);
    };
    let mut high = latest
        .id
        .checked_add(1)
        .context("Coinbase latest trade ID cannot be paged")?;
    if latest.time_ms < target_ms {
        return Ok(Some(Seek {
            cursor: high.to_string(),
            expected_newest: latest.id,
            boundary_complete: true,
        }));
    }
    let latest_id = latest.id;
    let mut low = 0_u64;
    while low + 1 < high {
        let middle = low + (high - low) / 2;
        let probe = pacer
            .page(client, symbol, Some("after"), Some(&middle.to_string()), 1)
            .await?;
        if probe
            .trades
            .first()
            .is_some_and(|trade| trade.time_ms >= target_ms)
        {
            high = middle;
        } else {
            low = middle;
        }
    }
    let boundary = pacer
        .page(client, symbol, Some("after"), Some(&high.to_string()), 1)
        .await?;
    let anchor = boundary
        .trades
        .first()
        .context("Coinbase trade-ID seek returned no right-edge anchor")?;
    ensure!(
        anchor.time_ms >= target_ms && anchor.id <= latest_id,
        "Coinbase execution seek did not bracket requested end"
    );
    let predecessor = pacer
        .page(
            client,
            symbol,
            Some("after"),
            Some(&anchor.id.to_string()),
            1,
        )
        .await?;
    ensure!(
        predecessor
            .trades
            .first()
            .is_none_or(|trade| trade.time_ms < target_ms),
        "Coinbase execution seek skipped tied right-edge records"
    );
    let boundary_complete = predecessor
        .trades
        .first()
        .is_some_and(|previous| adjacent_ids(anchor.id, previous.id));
    Ok(Some(Seek {
        cursor: high.to_string(),
        expected_newest: anchor.id,
        boundary_complete,
    }))
}

pub async fn execution_history(
    client: &Client,
    instrument: &Instrument,
    start_ms: i64,
    end_ms: i64,
) -> Result<ExecutionPage> {
    ensure!(
        start_ms >= 0 && end_ms >= start_ms,
        "invalid Coinbase execution millisecond range"
    );
    let observed = super::super::flow::unix_millis();
    if instrument.product != Product::Spot {
        return Ok(page(
            Vec::new(),
            start_ms,
            end_ms,
            0,
            false,
            Some(INTX_UNAVAILABLE.into()),
        ));
    }
    super::spot_symbol(&instrument.symbol)?;
    if start_ms == end_ms {
        return Ok(page(
            Vec::new(),
            start_ms,
            end_ms,
            end_ms.min(observed),
            true,
            None,
        ));
    }
    let through = end_ms.min(observed);
    if through <= start_ms {
        return Ok(page(
            Vec::new(),
            start_ms,
            end_ms,
            observed,
            false,
            Some("Requested Coinbase execution range is not yet observable".into()),
        ));
    }
    let mut pacer = Pacer::default();
    let Some(seek) = seek(client, &instrument.symbol, through, &mut pacer).await? else {
        return Ok(page(Vec::new(), start_ms, end_ms, 0, false, Some("Coinbase public trades returned no records; empty tape does not prove range coverage".into())));
    };
    let mut cursor = seek.cursor;
    let mut sequence = HistorySequence {
        expected_newest: seek.expected_newest,
        previous: None,
        complete: seek.boundary_complete,
    };
    let mut seen = HashSet::new();
    let mut executions = Vec::new();
    let mut fetched = 0_usize;
    let mut oldest_id = None;
    let mut oldest_time_ms = None;
    let mut complete = false;
    let reason = loop {
        let native = match pacer
            .page(
                client,
                &instrument.symbol,
                Some("after"),
                Some(&cursor),
                PAGE_SIZE,
            )
            .await
        {
            Ok(native) => native,
            Err(error) if !executions.is_empty() => {
                break Some(format!("Coinbase execution replay interrupted: {error:#}"));
            }
            Err(error) => return Err(error),
        };
        if native.trades.is_empty() {
            break Some("Coinbase native pagination ended before a trade preceding the requested start; retention/empty-range coverage is unproved".into());
        }
        let cursor_id: u64 = cursor
            .parse()
            .context("invalid Coinbase execution cursor")?;
        ensure!(
            native.trades.iter().all(|trade| trade.id <= cursor_id),
            "Coinbase older trade page crossed its cursor"
        );
        ensure!(
            oldest_time_ms.is_none_or(|old| native.trades[0].time_ms <= old),
            "Coinbase execution timestamps regressed across older pages"
        );
        let mut fresh = 0_usize;
        for trade in &native.trades {
            if sequence.observe(trade)? {
                oldest_id = Some(trade.id);
                fresh += 1;
            }
            if trade.time_ms >= start_ms && trade.time_ms < through {
                executions.push(trade.execution());
            }
        }
        oldest_time_ms = native.trades.last().map(|trade| trade.time_ms);
        fetched += native.trades.len();
        if native
            .trades
            .last()
            .is_some_and(|trade| trade.time_ms < start_ms)
        {
            complete = sequence.complete;
            break (!complete).then(|| "Coinbase native history contains trade-ID holes within/across pages or at the sought right boundary; completeness is unproved".into());
        }
        if fetched >= MAX_HISTORY_RECORDS {
            break Some(format!(
                "Coinbase native replay reached its {MAX_HISTORY_RECORDS}-record request limit before proving the left edge; request a smaller range"
            ));
        }
        if fresh == 0 {
            break Some("Coinbase native trade pagination made no progress".into());
        }
        let Some(next) = native.after else {
            break Some(
                "Coinbase omitted CB-AFTER before the requested left edge was proven".into(),
            );
        };
        ensure!(
            next.parse::<u64>().ok() == oldest_id,
            "Coinbase CB-AFTER does not identify the oldest native trade"
        );
        if next == cursor || !seen.insert(next.clone()) {
            break Some(
                "Coinbase native trade cursor repeated before the requested left edge was proven"
                    .into(),
            );
        }
        cursor = next;
    };
    let executions = crate::executions::canonicalize(executions, start_ms, through)?;
    let mut result = page(executions, start_ms, through, through, complete, reason);
    if through < end_ms {
        result.coverage.push(Coverage {
            start_ms: through,
            end_ms,
            complete: false,
            reason: Some(
                "Coinbase execution range extends past the observed native snapshot".into(),
            ),
        });
    }
    Ok(result)
}

fn observed_trade(trade: &cvd::Trade) -> ExecutionEvent {
    ExecutionEvent::Page(Arc::new(page(vec![trade.execution()], trade.time_ms, trade.time_ms.saturating_add(1), trade.time_ms.saturating_add(1), false, Some("Native Coinbase match observed; matches can be dropped until heartbeat REST reconciliation".into()))))
}

// Forward cursor replay follows the same native before/after contract as CVD.
// The reader remains owned by this session while REST awaits; dropping the
// session aborts it, including broker handover to an existing CVD producer.
async fn reconcile(
    client: &Client,
    instrument: &Instrument,
    from_id: u64,
    target_id: u64,
    pacer: &mut Pacer,
) -> Result<Vec<Execution>> {
    let mut cursor = from_id.to_string();
    let mut seen = HashSet::new();
    let mut executions = Vec::new();
    for _ in 0..MAX_REPLAY_PAGES {
        let native = pacer
            .page(
                client,
                &instrument.symbol,
                Some("before"),
                Some(&cursor),
                PAGE_SIZE,
            )
            .await?;
        ensure!(
            !native.trades.is_empty(),
            "Coinbase heartbeat replay returned no native executions"
        );
        let reached = native.trades.iter().any(|trade| trade.id == target_id);
        for trade in &native.trades {
            if trade.id > from_id && trade.id <= target_id {
                executions.push(trade.execution());
            }
        }
        if reached {
            return crate::executions::canonicalize(executions, 0, i64::MAX);
        }
        let next = native
            .before
            .context("Coinbase heartbeat replay omitted CB-BEFORE")?;
        ensure!(
            next != cursor && seen.insert(next.clone()),
            "Coinbase heartbeat replay cursor did not advance"
        );
        cursor = next;
    }
    bail!(
        "Coinbase heartbeat replay exceeded {MAX_REPLAY_PAGES} native pages; continuity remains unproved"
    )
}

fn heartbeat_page(
    executions: Vec<Execution>,
    from_id: u64,
    target_id: u64,
    from_ms: i64,
    through_ms: i64,
    first_proven_ms: i64,
) -> ExecutionPage {
    let complete = replay_complete(from_id, target_id, executions.len());
    page(
        executions,
        from_ms.max(first_proven_ms),
        through_ms,
        through_ms,
        complete,
        (!complete).then(|| {
            "Coinbase heartbeat replay contains native trade-ID holes; completeness is unproved"
                .into()
        }),
    )
}

pub async fn execution_session(
    client: &Client,
    instrument: &Instrument,
    sender: &mpsc::Sender<ExecutionEvent>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    if *shutdown.borrow() || sender.is_closed() {
        return Ok(());
    }
    if instrument.product != Product::Spot {
        sender
            .send(ExecutionEvent::Page(Arc::new(page(
                Vec::new(),
                0,
                super::super::flow::unix_millis(),
                0,
                false,
                Some(INTX_UNAVAILABLE.into()),
            ))))
            .await?;
        sender
            .send(ExecutionEvent::Status {
                connected: false,
                message: INTX_UNAVAILABLE.into(),
            })
            .await?;
        tokio::select! { _ = sender.closed() => {}, _ = shutdown.changed() => {} }
        return Ok(());
    }
    super::spot_symbol(&instrument.symbol)?;
    sender
        .send(ExecutionEvent::Status {
            connected: false,
            message:
                "Connecting Coinbase Exchange native matches/heartbeat; no candle-volume proxy"
                    .into(),
        })
        .await?;
    let socket = tokio::select! {
        _ = sender.closed() => return Ok(()),
        _ = shutdown.changed() => return Ok(()),
        result = connect_async(LIVE_WS) => result.context("connecting Coinbase native executions")?.0,
    };
    let (mut sink, stream) = socket.split();
    let (reader_tx, mut reader_rx) = mpsc::channel(4_096);
    let _reader = super::super::AbortOnDrop(tokio::spawn(cvd::reader_task(stream, reader_tx)));
    sink.send(Message::Text(json!({"type":"subscribe", "product_ids":[instrument.symbol], "channels":["matches", "heartbeat"]}).to_string().into())).await.context("subscribing Coinbase native executions")?;
    sender.send(ExecutionEvent::Status { connected: true, message: "Coinbase native executions connected; heartbeat anchors and REST prove only observed intervals".into() }).await?;
    let mut pacer = Pacer::default();
    let mut anchor: Option<(u64, i64)> = None;
    // The first anchor excludes IDs at/before it, including earlier fractional
    // timestamps in its floored millisecond. Only independent history can fill it.
    let mut first_proven_ms = None;
    let mut ping = time::interval(Duration::from_secs(20));
    ping.tick().await;
    loop {
        tokio::select! {
            _ = sender.closed() => return Ok(()),
            _ = shutdown.changed() => return Ok(()),
            _ = ping.tick() => sink.send(Message::Ping(Vec::new().into())).await.context("pinging Coinbase execution feed")?,
            item = reader_rx.recv() => {
                let item = item.context("Coinbase execution reader ended")?.map_err(anyhow::Error::msg)?;
                let Some(message) = cvd::parse_live_message(item, &instrument.symbol)? else { continue; };
                match message {
                    LiveMessage::Trade(trade) => sender.send(observed_trade(&trade)).await?,
                    LiveMessage::Heartbeat { last_trade_id, time_ms } => {
                        if let Some((from_id, from_ms)) = anchor {
                            ensure!(last_trade_id >= from_id && time_ms >= from_ms, "Coinbase execution heartbeat regressed");
                            let executions = if last_trade_id > from_id {
                                tokio::select! {
                                    _ = sender.closed() => return Ok(()),
                                    _ = shutdown.changed() => return Ok(()),
                                    result = reconcile(client, instrument, from_id, last_trade_id, &mut pacer) => result?,
                                }
                            } else { Vec::new() };
                            let event = heartbeat_page(executions, from_id, last_trade_id,
                                from_ms, time_ms, first_proven_ms.unwrap_or(i64::MAX));
                            sender.send(ExecutionEvent::Page(Arc::new(event))).await?;
                        }
                        anchor = Some((last_trade_id, time_ms));
                        first_proven_ms.get_or_insert(time_ms.saturating_add(1));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mexc_model::{Exchange, Quote};
    use rust_decimal::Decimal;

    fn instrument(product: Product) -> Instrument {
        Instrument {
            exchange: Exchange::Coinbase,
            product,
            symbol: "BTC-USD".into(),
            base_coin: "BTC".into(),
            quote_coin: Quote::USD,
            settle_coin: "USD".into(),
            price_scale: 2,
            tick_size: Some(Decimal::new(1, 2)),
            contract_size: 1.0,
            inverse: false,
            volume_unit: "BTC".into(),
        }
    }

    #[test]
    fn half_open_native_ranges_keep_all_same_millisecond_ids_and_exact_quantities() {
        let row = |id: &str, time_ms| Execution {
            id: id.into(),
            time_ms,
            price: Decimal::from_str_exact("1000000000000000.00000001").unwrap(),
            quantity: Decimal::from_str_exact("0.000000000123456789").unwrap(),
        };
        let a = row("raw:9007199254740993", 1_000);
        let rows = crate::executions::canonicalize(
            vec![
                a.clone(),
                a.clone(),
                row("raw:9007199254740994", 1_000),
                row("raw:9007199254740995", 2_000),
            ],
            1_000,
            2_000,
        )
        .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].price, a.price);
        assert_eq!(rows[0].quantity, a.quantity);
    }

    #[tokio::test]
    async fn international_is_explicitly_unsupported_without_requesting_spot() {
        let result = execution_history(
            &Client::new(),
            &instrument(Product::Perpetual),
            1_000,
            2_000,
        )
        .await
        .unwrap();
        assert!(result.executions.is_empty());
        assert_eq!(result.observed_through_ms, 0);
        assert!(!result.coverage[0].complete);
        assert!(
            result.coverage[0]
                .reason
                .as_ref()
                .unwrap()
                .contains("International")
        );
    }

    #[tokio::test]
    async fn empty_range_never_requests_native_trades() {
        let result = execution_history(&Client::new(), &instrument(Product::Spot), 1_000, 1_000)
            .await
            .unwrap();
        assert!(result.executions.is_empty());
        assert!(result.coverage.is_empty());
    }

    #[test]
    fn observed_match_is_not_a_complete_interval_claim() {
        let event = page(
            Vec::new(),
            1_000,
            1_001,
            1_001,
            false,
            Some("heartbeat replay pending".into()),
        );
        assert!(!event.coverage[0].complete);
        assert_eq!(event.coverage[0].end_ms, 1_001);
    }

    #[test]
    fn reaching_a_heartbeat_id_does_not_prove_recovery_of_missing_ids() {
        assert!(replay_complete(100, 103, 3));
        assert!(!replay_complete(100, 103, 2));
        assert!(replay_complete(100, 100, 0));
        assert!(!replay_complete(103, 100, 0));
    }

    #[test]
    fn fractional_anchor_hole_requires_independent_history_recovery() {
        let value = json!({"type": "heartbeat", "product_id": "BTC-USD",
            "last_trade_id": 100, "time": "2026-01-01T00:00:00.123999Z"});
        let Some(LiveMessage::Heartbeat { time_ms, .. }) =
            cvd::parse_live_message(Message::Text(value.to_string().into()), "BTC-USD").unwrap()
        else {
            panic!("expected heartbeat");
        };
        let mut missing = native(100);
        // This match occurred at .123100, before the .123999 anchor.
        assert_eq!(missing.time_ms, time_ms);
        let mut recovered = native(101);
        recovered.time_ms += 1;
        let live = heartbeat_page(
            vec![recovered.execution()],
            100,
            101,
            time_ms,
            time_ms + 2,
            time_ms + 1,
        );
        assert!(live.coverage[0].complete);
        assert_eq!(live.coverage[0].start_ms, time_ms + 1);
        assert!(missing.time_ms < live.coverage[0].start_ms);
        let mut sequence = HistorySequence {
            expected_newest: 101,
            previous: None,
            complete: true,
        };
        let mut history = Vec::new();
        for trade in [&recovered, &missing] {
            if sequence.observe(trade).unwrap() {
                history.push(trade.execution());
            }
        }
        missing.id = 99;
        missing.time_ms -= 1;
        assert!(sequence.observe(&missing).unwrap());
        assert!(sequence.complete);
        let rows = crate::executions::canonicalize(
            history.into_iter().chain(live.executions).collect(),
            time_ms,
            time_ms + 2,
        )
        .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows.iter().map(|row| row.quantity).sum::<Decimal>(),
            recovered.size * Decimal::from(2)
        );
    }

    fn native(id: u64) -> cvd::Trade {
        let value = json!({
            "type": "match", "product_id": "BTC-USD", "trade_id": id,
            "time": "2026-01-01T00:00:00.123Z", "price": "12345.00000001",
            "size": "0.000000001", "side": "sell"
        });
        match cvd::parse_live_message(Message::Text(value.to_string().into()), "BTC-USD").unwrap() {
            Some(LiveMessage::Trade(trade)) => trade,
            _ => panic!("expected native trade"),
        }
    }

    #[test]
    fn history_id_holes_within_and_across_pages_never_prove_complete() {
        let mut sequence = HistorySequence {
            expected_newest: 105,
            previous: None,
            complete: true,
        };
        assert!(sequence.observe(&native(105)).unwrap());
        assert!(sequence.observe(&native(104)).unwrap());
        assert!(sequence.complete);
        assert!(!sequence.observe(&native(104)).unwrap());
        // An older page starts at 102: missing 103 breaks the sequence proof.
        assert!(sequence.observe(&native(102)).unwrap());
        assert!(!sequence.complete);
        assert!(sequence.observe(&native(100)).unwrap());
        assert!(!sequence.complete);
        // A conflicting repeated ID must not be silently canonicalized.
        let mut conflict = native(100);
        conflict.price += Decimal::new(1, 8);
        assert!(sequence.observe(&conflict).is_err());
    }

    #[test]
    fn right_boundary_id_gap_and_skipped_first_record_are_unproved() {
        assert!(adjacent_ids(105, 104));
        assert!(!adjacent_ids(105, 103));
        let mut boundary = HistorySequence {
            expected_newest: 105,
            previous: None,
            complete: false,
        };
        boundary.observe(&native(105)).unwrap();
        boundary.observe(&native(104)).unwrap();
        assert!(!boundary.complete);
        let mut skipped = HistorySequence {
            expected_newest: 105,
            previous: None,
            complete: true,
        };
        skipped.observe(&native(104)).unwrap();
        assert!(!skipped.complete);
    }
}
