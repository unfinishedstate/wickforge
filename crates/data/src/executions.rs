//! Owned subscription fanout. Existing native CVD producers have priority; profiles
//! attach to that producer rather than opening another trade socket. With no CVD
//! producer the first subscriber drives one cancellable provider session.
use anyhow::{Context, Result, ensure};
use mexc_model::{
    Instrument,
    executions::{Execution, ExecutionEvent, ExecutionPage, NativeInstrument},
};
use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc, Mutex, OnceLock, Weak,
        atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::{broadcast, mpsc, watch};

const RECENT_IDENTITIES: usize = 100_000;
struct Seen {
    rows: HashMap<String, Execution>,
    order: VecDeque<String>,
}
struct Hub {
    events: broadcast::Sender<ExecutionEvent>,
    changed: watch::Sender<u64>,
    external: AtomicUsize,
    owned: AtomicBool,
    last_ms: AtomicI64,
    seen: Mutex<Seen>,
}
static HUBS: OnceLock<Mutex<HashMap<NativeInstrument, Weak<Hub>>>> = OnceLock::new();
fn key(instrument: &Instrument) -> NativeInstrument {
    NativeInstrument {
        exchange: instrument.exchange,
        product: instrument.product,
        symbol: instrument.symbol.clone(),
    }
}
fn hub(instrument: &Instrument) -> Arc<Hub> {
    let mut hubs = HUBS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    hubs.retain(|_, value| value.strong_count() > 0);
    let key = key(instrument);
    if let Some(found) = hubs.get(&key).and_then(Weak::upgrade) {
        return found;
    }
    let (events, _) = broadcast::channel(1024);
    let (changed, _) = watch::channel(0);
    let found = Arc::new(Hub {
        events,
        changed,
        external: AtomicUsize::new(0),
        owned: AtomicBool::new(false),
        last_ms: AtomicI64::new(0),
        seen: Mutex::new(Seen {
            rows: HashMap::new(),
            order: VecDeque::new(),
        }),
    });
    hubs.insert(key, Arc::downgrade(&found));
    found
}
fn notify(hub: &Hub) {
    hub.changed
        .send_modify(|revision| *revision = revision.wrapping_add(1));
}

pub(crate) struct NativeProducer {
    hub: Arc<Hub>,
}
pub(crate) fn native_producer(instrument: &Instrument) -> NativeProducer {
    let hub = hub(instrument);
    hub.external.fetch_add(1, Ordering::AcqRel);
    notify(&hub);
    NativeProducer { hub }
}
impl Drop for NativeProducer {
    fn drop(&mut self) {
        self.hub.external.fetch_sub(1, Ordering::AcqRel);
        let now = crate::providers::flow::unix_millis();
        let start = self.hub.last_ms.load(Ordering::Acquire);
        if start > 0 {
            let _ = self.hub.events.send(ExecutionEvent::Invalidate {
                start_ms: start,
                end_ms: now.max(start.saturating_add(1)),
                reason:
                    "Shared native trade producer interrupted; continuity requires native replay"
                        .into(),
            });
        }
        notify(&self.hub);
    }
}

pub(crate) fn observe(instrument: &Instrument, execution: Execution) {
    let through = execution.time_ms.saturating_add(1);
    observe_page(instrument, ExecutionPage { coverage: vec![mexc_model::executions::Coverage { start_ms: execution.time_ms, end_ms: through, complete: false, reason: Some("Observed native execution; interval completeness requires replay or sequence proof".into()) }], executions: vec![execution], observed_through_ms: through });
}
pub(crate) fn observe_page(instrument: &Instrument, page: ExecutionPage) {
    publish(&hub(instrument), ExecutionEvent::Page(Arc::new(page)));
}
pub(crate) fn observe_gap(
    instrument: &Instrument,
    start_ms: i64,
    end_ms: i64,
    reason: impl Into<String>,
) {
    publish(
        &hub(instrument),
        ExecutionEvent::Invalidate {
            start_ms,
            end_ms,
            reason: reason.into(),
        },
    );
}

/// Reject same-ID corrections instead of counting both. This also canonicalizes
/// REST/archive overlap without mistaking a timestamp for an execution identity.
pub(crate) fn canonicalize(
    rows: Vec<Execution>,
    start_ms: i64,
    end_ms: i64,
) -> Result<Vec<Execution>> {
    let mut ids = HashMap::<String, Execution>::new();
    for row in rows {
        ensure!(
            !row.id.is_empty()
                && row.price > rust_decimal::Decimal::ZERO
                && row.quantity >= rust_decimal::Decimal::ZERO,
            "invalid native priced execution"
        );
        if let Some(old) = ids.get(&row.id) {
            ensure!(
                old == &row,
                "conflicting native execution identity {}",
                row.id
            );
        } else {
            ids.insert(row.id.clone(), row);
        }
    }
    let mut rows: Vec<_> = ids
        .into_values()
        .filter(|row| row.time_ms >= start_ms && row.time_ms < end_ms)
        .collect();
    rows.sort_unstable_by(|a, b| (a.time_ms, &a.id).cmp(&(b.time_ms, &b.id)));
    Ok(rows)
}
fn publish(hub: &Hub, event: ExecutionEvent) {
    let event = match event {
        ExecutionEvent::Page(page) => {
            let mut seen = hub.seen.lock().unwrap_or_else(|e| e.into_inner());
            let mut invalid = false;
            let mut accepted = Vec::with_capacity(page.executions.len());
            for execution in &page.executions {
                if execution.id.is_empty()
                    || execution.price <= rust_decimal::Decimal::ZERO
                    || execution.quantity < rust_decimal::Decimal::ZERO
                {
                    invalid = true;
                    let _ = hub.events.send(ExecutionEvent::Invalidate {
                        start_ms: execution.time_ms,
                        end_ms: execution.time_ms.saturating_add(1),
                        reason: "Invalid native priced execution".into(),
                    });
                    continue;
                }
                if let Some(old) = seen.rows.get(&execution.id) {
                    if old != execution {
                        invalid = true;
                        let _ = hub.events.send(ExecutionEvent::Invalidate {
                            start_ms: old.time_ms.min(execution.time_ms),
                            end_ms: old.time_ms.max(execution.time_ms).saturating_add(1),
                            reason: format!(
                                "Conflicting native execution identity {}",
                                execution.id
                            ),
                        });
                    } else {
                        // Identity memory detects corrections, not subscriber delivery.
                        accepted.push(execution.clone());
                    }
                    continue;
                }
                seen.order.push_back(execution.id.clone());
                seen.rows.insert(execution.id.clone(), execution.clone());
                accepted.push(execution.clone());
                while seen.order.len() > RECENT_IDENTITIES {
                    if let Some(id) = seen.order.pop_front() {
                        seen.rows.remove(&id);
                    }
                }
            }
            hub.last_ms
                .fetch_max(page.observed_through_ms, Ordering::AcqRel);
            let mut coverage = page.coverage.clone();
            if invalid {
                for range in &mut coverage {
                    range.complete = false;
                    range.reason = Some(
                        "Invalid or conflicting native execution identity; replay required".into(),
                    );
                }
            }
            ExecutionEvent::Page(Arc::new(ExecutionPage {
                executions: accepted,
                coverage,
                observed_through_ms: page.observed_through_ms,
            }))
        }
        other => other,
    };
    let _ = hub.events.send(event);
}
struct Owner(Arc<Hub>);
impl Drop for Owner {
    fn drop(&mut self) {
        self.0.owned.store(false, Ordering::Release);
        notify(&self.0);
    }
}

pub(crate) async fn cancelled(mut shutdown: watch::Receiver<bool>) {
    loop {
        if *shutdown.borrow() {
            return;
        }
        if shutdown.changed().await.is_err() {
            return;
        }
    }
}

pub(crate) async fn stream(
    client: &reqwest::Client,
    instrument: &Instrument,
    sender: mpsc::Sender<ExecutionEvent>,
    shutdown: watch::Receiver<bool>,
) -> Result<()> {
    // Cancellation must also interrupt backpressure in relay/send, not merely
    // socket reads. Dropping inner releases Owner and the provider future.
    tokio::select! {
        _ = cancelled(shutdown.clone()) => Ok(()),
        _ = sender.closed() => Ok(()),
        result = stream_inner(client, instrument, sender.clone(), shutdown) => result,
    }
}

async fn stream_inner(
    client: &reqwest::Client,
    instrument: &Instrument,
    sender: mpsc::Sender<ExecutionEvent>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    let hub = hub(instrument);
    let mut events = hub.events.subscribe();
    let mut changed = hub.changed.subscribe();
    let sharing = hub.external.load(Ordering::Acquire) > 0;
    sender
        .send(ExecutionEvent::Status {
            connected: sharing,
            message: if sharing {
                "Sharing existing native CVD execution producer; unproved gaps remain incomplete"
            } else {
                "Waiting for native execution source; no candle-volume proxy"
            }
            .into(),
        })
        .await?;
    loop {
        if *shutdown.borrow() || sender.is_closed() {
            return Ok(());
        }
        if hub.external.load(Ordering::Acquire) == 0
            && hub
                .owned
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
        {
            let _owner = Owner(hub.clone());
            let (provider_sender, mut provider_events) = mpsc::channel(256);
            let session = crate::providers::execution_session(
                client,
                instrument,
                &provider_sender,
                shutdown.clone(),
            );
            tokio::pin!(session);
            let failure = loop {
                tokio::select! {
                    biased;
                    _ = shutdown.changed() => return Ok(()),
                    _ = sender.closed() => return Ok(()),
                    _ = changed.changed() => { if hub.external.load(Ordering::Acquire) > 0 { break None; } },
                    result = &mut session => { break Some(result); },
                    event = provider_events.recv() => { if let Some(event) = event { publish(&hub, event); } },
                    event = events.recv() => relay(event, &sender, &hub).await?,
                }
            };
            // Drain final buffered messages before the provider future is dropped.
            while let Ok(event) = provider_events.try_recv() {
                publish(&hub, event);
            }
            if let Some(result) = failure {
                let now = crate::providers::flow::unix_millis();
                let last = hub.last_ms.load(Ordering::Acquire);
                if last > 0 {
                    publish(
                        &hub,
                        ExecutionEvent::Invalidate {
                            start_ms: last,
                            end_ms: now.max(last.saturating_add(1)),
                            reason:
                                "Native execution connection interrupted; gap not proven recovered"
                                    .into(),
                        },
                    );
                }
                publish(
                    &hub,
                    ExecutionEvent::Status {
                        connected: false,
                        message: format!(
                            "Native execution source reconnecting: {}",
                            result
                                .err()
                                .map_or_else(|| "session ended".into(), |e| format!("{e:#}"))
                        ),
                    },
                );
                // Keep forwarding queued observations while avoiding hot reconnects.
                let delay = tokio::time::sleep(Duration::from_secs(1));
                tokio::pin!(delay);
                loop {
                    tokio::select! { _ = &mut delay => break, _ = shutdown.changed() => return Ok(()), _ = sender.closed() => return Ok(()), event = events.recv() => relay(event, &sender, &hub).await? }
                }
            } else {
                sender.send(ExecutionEvent::Status { connected: true, message: "Sharing existing native CVD execution producer; unproved gaps remain incomplete".into() }).await?;
            }
        } else {
            tokio::select! {
                _ = shutdown.changed() => return Ok(()),
                _ = sender.closed() => return Ok(()),
                _ = changed.changed() => {
                    if hub.external.load(Ordering::Acquire) > 0 {
                        sender.send(ExecutionEvent::Status { connected: true, message: "Sharing existing native CVD execution producer; unproved gaps remain incomplete".into() }).await?;
                    }
                },
                event = events.recv() => relay(event, &sender, &hub).await?,
            }
        }
    }
}
async fn relay(
    event: std::result::Result<ExecutionEvent, broadcast::error::RecvError>,
    sender: &mpsc::Sender<ExecutionEvent>,
    hub: &Hub,
) -> Result<()> {
    match event {
        Ok(event) => sender
            .send(event)
            .await
            .context("execution consumer closed")?,
        Err(broadcast::error::RecvError::Lagged(count)) => {
            let now = crate::providers::flow::unix_millis();
            sender
                .send(ExecutionEvent::Invalidate {
                    start_ms: 0,
                    end_ms: now
                        .max(hub.last_ms.load(Ordering::Acquire))
                        .saturating_add(1),
                    reason: format!(
                        "Execution consumer missed {count} native events; replay required"
                    ),
                })
                .await?;
        }
        Err(broadcast::error::RecvError::Closed) => anyhow::bail!("native execution broker closed"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::Decimal;
    fn row(id: &str, time_ms: i64) -> Execution {
        Execution {
            id: id.into(),
            time_ms,
            price: Decimal::new(12345, 3),
            quantity: Decimal::new(123456789, 8),
        }
    }
    fn instrument(symbol: &str) -> Instrument {
        Instrument {
            exchange: mexc_model::Exchange::Mexc,
            product: mexc_model::Product::Spot,
            symbol: symbol.into(),
            base_coin: "TEST".into(),
            quote_coin: mexc_model::Quote::USDT,
            settle_coin: "USDT".into(),
            price_scale: 8,
            tick_size: None,
            contract_size: 1.0,
            inverse: false,
            volume_unit: "TEST".into(),
        }
    }
    #[test]
    fn duplicate_identity_is_one_but_conflict_is_error() {
        let a = row("raw:1", 100);
        assert_eq!(
            canonicalize(vec![a.clone(), a.clone()], 0, 200).unwrap(),
            vec![a.clone()]
        );
        let mut b = a.clone();
        b.quantity += Decimal::ONE;
        assert!(canonicalize(vec![a, b], 0, 200).is_err());
    }
    #[test]
    fn range_is_half_open_and_raw_aggregate_ids_are_distinct() {
        let rows = canonicalize(
            vec![row("raw:1", 100), row("agg:1", 100), row("raw:2", 200)],
            100,
            200,
        )
        .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].quantity.to_string(), "1.23456789");
    }
    #[tokio::test]
    async fn shutdown_interrupts_a_backpressured_broker() {
        let instrument = instrument("BROKER-CANCEL-TEST");
        let producer = native_producer(&instrument);
        let (sender, _receiver) = mpsc::channel(1);
        sender
            .send(ExecutionEvent::Status {
                connected: false,
                message: "full".into(),
            })
            .await
            .unwrap();
        let (shutdown, receiver) = watch::channel(false);
        let client = reqwest::Client::new();
        let operation = stream(&client, &instrument, sender, receiver);
        tokio::pin!(operation);
        tokio::select! {
            result = &mut operation => panic!("broker ended before cancellation: {result:?}"),
            _ = tokio::task::yield_now() => {},
        }
        shutdown.send(true).unwrap();
        tokio::time::timeout(Duration::from_secs(1), operation)
            .await
            .unwrap()
            .unwrap();
        drop(producer);
    }

    #[tokio::test]
    async fn lag_is_an_explicit_replay_invalidation() {
        let hub = hub(&instrument("BROKER-LAG-TEST"));
        let (sender, mut receiver) = mpsc::channel(1);
        relay(Err(broadcast::error::RecvError::Lagged(9)), &sender, &hub)
            .await
            .unwrap();
        match receiver.recv().await.unwrap() {
            ExecutionEvent::Invalidate {
                start_ms,
                end_ms,
                reason,
            } => {
                assert_eq!(start_ms, 0);
                assert!(end_ms > 0);
                assert!(reason.contains("9"));
            }
            other => panic!("expected replay invalidation, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn lagged_consumer_recovers_quantity_before_replay_certifies_coverage() {
        let hub = hub(&instrument("BROKER-REPLAY-QUANTITY"));
        let mut events = hub.events.subscribe();
        let execution = row("raw:lost", 100);
        let replay = ExecutionEvent::Page(Arc::new(ExecutionPage {
            executions: vec![execution.clone()],
            coverage: vec![mexc_model::executions::Coverage {
                start_ms: 100,
                end_ms: 101,
                complete: true,
                reason: None,
            }],
            observed_through_ms: 101,
        }));
        publish(&hub, replay.clone());
        for _ in 0..2048 {
            publish(
                &hub,
                ExecutionEvent::Status {
                    connected: true,
                    message: String::new(),
                },
            );
        }
        let lost = events.recv().await;
        assert!(matches!(lost, Err(broadcast::error::RecvError::Lagged(_))));
        let (sender, mut receiver) = mpsc::channel(1);
        relay(lost, &sender, &hub).await.unwrap();
        assert!(matches!(
            receiver.recv().await.unwrap(),
            ExecutionEvent::Invalidate { .. }
        ));
        while events.try_recv().is_ok() {}
        let mut consumer_rows = HashMap::new();
        for _ in 0..2 {
            publish(&hub, replay.clone());
            relay(events.recv().await, &sender, &hub).await.unwrap();
            let ExecutionEvent::Page(page) = receiver.recv().await.unwrap() else {
                panic!("expected replay page");
            };
            for row in &page.executions {
                consumer_rows.entry(row.id.clone()).or_insert(row.quantity);
            }
            assert_eq!(
                consumer_rows.values().copied().sum::<Decimal>(),
                execution.quantity
            );
            assert!(page.coverage[0].complete);
        }
    }

    #[test]
    fn ownership_and_external_guards_release_on_drop() {
        let instrument = instrument("BROKER-GUARD-TEST");
        let hub = hub(&instrument);
        let producer = native_producer(&instrument);
        assert_eq!(hub.external.load(Ordering::Acquire), 1);
        drop(producer);
        assert_eq!(hub.external.load(Ordering::Acquire), 0);
        hub.owned.store(true, Ordering::Release);
        drop(Owner(hub.clone()));
        assert!(!hub.owned.load(Ordering::Acquire));
    }

    #[test]
    fn a_conflict_cannot_publish_complete_coverage_after_invalidation() {
        let hub = hub(&instrument("BROKER-CONFLICT-TEST"));
        let mut events = hub.events.subscribe();
        let first = row("raw:9", 100);
        let mut changed = first.clone();
        changed.price += Decimal::ONE;
        publish(
            &hub,
            ExecutionEvent::Page(Arc::new(ExecutionPage {
                executions: vec![first, changed],
                coverage: vec![mexc_model::executions::Coverage {
                    start_ms: 100,
                    end_ms: 101,
                    complete: true,
                    reason: None,
                }],
                observed_through_ms: 101,
            })),
        );
        assert!(matches!(
            events.try_recv().unwrap(),
            ExecutionEvent::Invalidate { .. }
        ));
        let ExecutionEvent::Page(page) = events.try_recv().unwrap() else {
            panic!("expected page");
        };
        assert!(!page.coverage[0].complete);
        assert_eq!(page.executions.len(), 1);
    }

    #[tokio::test]
    async fn subscribers_receive_identical_cvd_replays_for_local_deduplication() {
        let instrument = instrument("BROKER-SHARE-TEST");
        let producer = native_producer(&instrument);
        let (sender, mut receiver) = mpsc::channel(8);
        let (shutdown, shutdown_receiver) = watch::channel(false);
        let source = instrument.clone();
        let task = tokio::spawn(async move {
            stream(&reqwest::Client::new(), &source, sender, shutdown_receiver).await
        });
        let event = tokio::time::timeout(Duration::from_secs(1), receiver.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            event,
            ExecutionEvent::Status {
                connected: true,
                ..
            }
        ));
        let execution = row("agg:pb:7", 100);
        observe(&instrument, execution.clone());
        observe(&instrument, execution.clone());
        let first = tokio::time::timeout(Duration::from_secs(1), receiver.recv())
            .await
            .unwrap()
            .unwrap();
        let ExecutionEvent::Page(first) = first else {
            panic!("expected execution page");
        };
        assert_eq!(first.executions, vec![execution.clone()]);
        let second = tokio::time::timeout(Duration::from_secs(1), receiver.recv())
            .await
            .unwrap()
            .unwrap();
        let ExecutionEvent::Page(second) = second else {
            panic!("expected duplicate page");
        };
        assert_eq!(second.executions, vec![execution]);
        shutdown.send(true).unwrap();
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        drop(producer);
    }
}
