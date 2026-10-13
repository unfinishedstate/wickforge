use std::{sync::Arc, time::Duration};

use arrayvec::ArrayVec;
use futures_util::{SinkExt, Stream};
use mexc_data::DataService;
use mexc_model::{HistoryEvent, Market, MarketEvent, OpenInterestEvent};
use tokio::{
    sync::{mpsc, watch},
    task::{JoinHandle, JoinSet},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Cvd,
    OpenInterest,
}

#[derive(Debug, Clone)]
pub struct Scope {
    pub market: Market,
    pub generation: u64,
    pub kind: Kind,
    pub base_asset: String,
}

#[derive(Debug, Clone)]
pub enum Event {
    Metadata { base_asset: String },
    Market(MarketEvent),
    History(HistoryEvent),
    HistoryProblem { message: String },
    OpenInterest(OpenInterestEvent),
}

#[derive(Debug, Clone)]
pub struct Batch {
    pub scope: Arc<Scope>,
    pub events: Box<[Event]>,
}

struct Producer {
    task: JoinHandle<()>,
    shutdown: watch::Sender<bool>,
}
impl Drop for Producer {
    fn drop(&mut self) {
        let _ = self.shutdown.send(true);
        self.task.abort();
    }
}

pub fn source(
    data: DataService,
    scope: Scope,
    target: watch::Receiver<i64>,
) -> impl Stream<Item = Batch> {
    iced::stream::channel(2, move |mut output| async move {
        let scope = Arc::new(scope);
        let (sender, mut receiver) = mpsc::channel(128);
        let (shutdown, stop) = watch::channel(false);
        let source_scope = Arc::clone(&scope);
        let task = tokio::spawn(async move {
            loop {
                let result =
                    run_source(&data, &source_scope, &sender, target.clone(), stop.clone()).await;
                if result.is_ok() || sender.is_closed() || *stop.borrow() {
                    return;
                }
                let Err(error) = result else {
                    return;
                };
                if sender
                    .send(unavailable(
                        source_scope.kind,
                        format!("Native source unavailable: {error:#}; retrying"),
                    ))
                    .await
                    .is_err()
                {
                    return;
                }
                tokio::select! {
                    _ = sender.closed() => return,
                    _ = tokio::time::sleep(Duration::from_secs(5)) => {},
                }
            }
        });
        let _producer = Producer { task, shutdown };
        while let Some(first) = receiver.recv().await {
            let mut events = ArrayVec::<Event, 64>::new();
            events.push(first);
            let deadline = tokio::time::sleep(Duration::from_millis(16));
            tokio::pin!(deadline);
            while !events.is_full() {
                tokio::select! {
                    biased;
                    _ = &mut deadline => break,
                    event = receiver.recv() => match event {
                        Some(event) => events.push(event),
                        None => break,
                    },
                }
            }
            if output
                .send(Batch {
                    scope: Arc::clone(&scope),
                    events: events.into_iter().collect(),
                })
                .await
                .is_err()
            {
                return;
            }
        }
    })
}

fn unavailable(kind: Kind, message: String) -> Event {
    match kind {
        Kind::Cvd => Event::Market(MarketEvent::FlowStatus {
            connected: false,
            message,
        }),
        Kind::OpenInterest => Event::OpenInterest(OpenInterestEvent::Status {
            connected: false,
            message,
        }),
    }
}

async fn run_source(
    data: &DataService,
    scope: &Scope,
    sender: &mpsc::Sender<Event>,
    target: watch::Receiver<i64>,
    mut stop: watch::Receiver<bool>,
) -> anyhow::Result<()> {
    let child_stop = stop.clone();
    let session = async {
        let catalog = data
            .catalog(scope.market.exchange, scope.market.product)
            .await?;
        let Some(instrument) = catalog
            .instruments
            .iter()
            .find(|instrument| instrument.symbol == scope.market.symbol)
        else {
            sender
                .send(unavailable(
                    scope.kind,
                    "Selected native instrument is no longer eligible".into(),
                ))
                .await?;
            return Ok(());
        };
        sender
            .send(Event::Metadata {
                base_asset: instrument.base_coin.clone(),
            })
            .await?;
        if instrument.base_coin != scope.base_asset {
            sender.send(unavailable(scope.kind, format!("Source underlying {} does not match chart {}; choose a matching native market", instrument.base_coin, scope.base_asset))).await?;
            return Ok(());
        }
        if scope.kind == Kind::Cvd && !catalog.intervals.contains(&scope.market.interval) {
            sender
                .send(unavailable(
                    scope.kind,
                    format!(
                        "{} does not support native {}; no CVD candle resampling",
                        scope.market.exchange, scope.market.interval
                    ),
                ))
                .await?;
            return Ok(());
        }
        let mut tasks = JoinSet::new();
        match scope.kind {
            Kind::Cvd => {
                let (resync, sync) = watch::channel(0_u64);
                tasks.spawn(cvd_live(
                    data.clone(),
                    scope.market.clone(),
                    sender.clone(),
                    child_stop.clone(),
                    resync,
                ));
                tasks.spawn(cvd_history(
                    data.clone(),
                    scope.market.clone(),
                    sender.clone(),
                    target,
                    sync,
                ));
            }
            Kind::OpenInterest => {
                let (cursor, before) = watch::channel(None);
                tasks.spawn(oi_live(
                    data.clone(),
                    scope.market.clone(),
                    sender.clone(),
                    child_stop.clone(),
                    cursor,
                ));
                tasks.spawn(oi_history(
                    data.clone(),
                    scope.market.clone(),
                    sender.clone(),
                    target,
                    before,
                ));
            }
        }
        while let Some(result) = tasks.join_next().await {
            result??;
        }
        Ok(())
    };
    tokio::select! {
        biased;
        _ = stop.changed() => Ok(()),
        _ = sender.closed() => Ok(()),
        result = session => result,
    }
}

async fn cvd_live(
    data: DataService,
    market: Market,
    sender: mpsc::Sender<Event>,
    stop: watch::Receiver<bool>,
    resync: watch::Sender<u64>,
) -> anyhow::Result<()> {
    let mut attempt = 0_u64;
    loop {
        let (native, mut receiver) = mpsc::channel(128);
        let request = data.stream_in_base(market.clone(), true, native, stop.clone());
        tokio::pin!(request);
        let result = loop {
            tokio::select! {
                result = &mut request => {
                    while let Ok(event) = receiver.try_recv() {
                        if matches!(event, MarketEvent::Resync) { resync.send_modify(|value| *value = value.wrapping_add(1)); }
                        sender.send(Event::Market(event)).await?;
                    }
                    break result;
                },
                event = receiver.recv() => if let Some(event) = event {
                    if matches!(event, MarketEvent::Resync) { resync.send_modify(|value| *value = value.wrapping_add(1)); }
                    sender.send(Event::Market(event)).await?;
                },
            }
        };
        if sender.is_closed() || *stop.borrow() {
            return Ok(());
        }
        let message = result.err().map_or_else(
            || "Native source stream ended; repairing coverage".into(),
            |error| format!("Native source disconnected: {error:#}"),
        );
        sender.send(unavailable(Kind::Cvd, message)).await?;
        attempt = attempt.wrapping_add(1);
        resync.send_replace(attempt);
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

async fn cvd_history(
    data: DataService,
    market: Market,
    sender: mpsc::Sender<Event>,
    mut target: watch::Receiver<i64>,
    mut resync: watch::Receiver<u64>,
) -> anyhow::Result<()> {
    let mut before = None;
    let mut recent = true;
    loop {
        if recent || before.is_some_and(|cursor| cursor > *target.borrow()) {
            let query = if recent { None } else { before };
            let (native, mut receiver) = mpsc::channel(128);
            let request = data.history_with_flow_in_base(market.clone(), query, native);
            tokio::pin!(request);
            let mut next = None;
            let mut terminal = false;
            let result = loop {
                tokio::select! {
                    result = &mut request => {
                        while let Ok(event) = receiver.try_recv() {
                            if let HistoryEvent::History { page } = &event { next = page.next_before; }
                            terminal |= matches!(event, HistoryEvent::Finished | HistoryEvent::Error { .. });
                            sender.send(Event::History(event)).await?;
                        }
                        break result;
                    },
                    event = receiver.recv() => if let Some(event) = event {
                        if let HistoryEvent::History { page } = &event { next = page.next_before; }
                        terminal |= matches!(event, HistoryEvent::Finished | HistoryEvent::Error { .. });
                        sender.send(Event::History(event)).await?;
                    },
                }
            };
            if result.is_err() || !terminal {
                if !terminal {
                    sender
                        .send(Event::History(HistoryEvent::Error {
                            message: "Native source history ended before completion".into(),
                        }))
                        .await?;
                }
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            }
            before = next.filter(|next| query.is_none_or(|previous| *next < previous));
            recent = false;
        } else {
            tokio::select! {
                changed = target.changed() => if changed.is_err() { return Ok(()); },
                changed = resync.changed() => {
                    if changed.is_err() { return Ok(()); }
                    recent = true;
                },
                _ = sender.closed() => return Ok(()),
            }
        }
    }
}

async fn oi_live(
    data: DataService,
    market: Market,
    sender: mpsc::Sender<Event>,
    stop: watch::Receiver<bool>,
    cursor: watch::Sender<Option<i64>>,
) -> anyhow::Result<()> {
    loop {
        let (native, mut receiver) = mpsc::channel(128);
        let request = data.open_interest_stream_in_base(market.clone(), native, stop.clone());
        tokio::pin!(request);
        let mut initialized = false;
        let result = loop {
            tokio::select! {
                result = &mut request => {
                    while let Ok(event) = receiver.try_recv() {
                        if let OpenInterestEvent::History(page) = &event && !initialized {
                            cursor.send_replace(page.next_before_ms);
                            initialized = true;
                        }
                        sender.send(Event::OpenInterest(event)).await?;
                    }
                    break result;
                },
                event = receiver.recv() => if let Some(event) = event {
                    if let OpenInterestEvent::History(page) = &event && !initialized {
                        cursor.send_replace(page.next_before_ms);
                        initialized = true;
                    }
                    sender.send(Event::OpenInterest(event)).await?;
                },
            }
        };
        if sender.is_closed() || *stop.borrow() {
            return Ok(());
        }
        match result {
            Ok(()) => return Ok(()),
            Err(error) => {
                sender
                    .send(unavailable(
                        Kind::OpenInterest,
                        format!("Native OI source disconnected: {error:#}"),
                    ))
                    .await?;
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        }
    }
}

async fn oi_history(
    data: DataService,
    market: Market,
    sender: mpsc::Sender<Event>,
    mut target: watch::Receiver<i64>,
    mut cursor: watch::Receiver<Option<i64>>,
) -> anyhow::Result<()> {
    let mut before = None;
    let mut initialized = false;
    loop {
        if !initialized && let Some(first) = *cursor.borrow_and_update() {
            before = Some(first);
            initialized = true;
        }
        if let Some(query) = before.filter(|before| *before > target.borrow().saturating_mul(1000))
        {
            match data
                .open_interest_history_in_base(&market, Some(query))
                .await
            {
                Ok(page) => {
                    before = page.next_before_ms.filter(|next| *next < query);
                    sender
                        .send(Event::OpenInterest(OpenInterestEvent::History(Arc::new(
                            page,
                        ))))
                        .await?;
                }
                Err(error) => {
                    sender
                        .send(Event::HistoryProblem {
                            message: format!("Native OI source history failed: {error:#}"),
                        })
                        .await?;
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
            }
        } else {
            tokio::select! {
                changed = target.changed() => if changed.is_err() { return Ok(()); },
                changed = cursor.changed() => if changed.is_err() { return Ok(()); },
                _ = sender.closed() => return Ok(()),
            }
        }
    }
}
