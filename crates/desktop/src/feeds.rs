use std::{sync::Arc, time::Duration};

use arrayvec::ArrayVec;
use futures_util::{SinkExt, Stream};
use mexc_data::DataService;
use mexc_model::{HistoryEvent, Market, MarketEvent, OpenInterestEvent};
use tokio::{
    sync::{mpsc, oneshot, watch},
    task::JoinHandle,
};

const CHANNEL_CAPACITY: usize = 128;
const BATCH_CAPACITY: usize = 64;
const FRAME_INTERVAL: Duration = Duration::from_millis(16);

#[derive(Debug, Clone)]
pub struct Scope {
    pub market: Market,
    pub generation: u64,
    pub flow_generation: u64,
    pub cvd: bool,
}

#[derive(Debug, Clone)]
pub struct Event {
    pub scope: Arc<Scope>,
    pub events: Box<[MarketEvent]>,
}

#[derive(Debug, Clone)]
pub struct OpenInterestScope {
    pub market: Market,
    pub generation: u64,
    pub open_interest_generation: u64,
}

#[derive(Debug, Clone)]
pub struct OpenInterestBatch {
    pub scope: Arc<OpenInterestScope>,
    pub events: Box<[OpenInterestEvent]>,
}

// Dropping an Iced subscription must stop native I/O, including provider children.
struct Producer {
    task: JoinHandle<()>,
    shutdown: Option<watch::Sender<bool>>,
}

impl Drop for Producer {
    fn drop(&mut self) {
        if let Some(shutdown) = &self.shutdown {
            let _ = shutdown.send(true);
        }
        self.task.abort();
    }
}

async fn next_batch<T>(receiver: &mut mpsc::Receiver<T>) -> Option<Box<[T]>> {
    let first = receiver.recv().await?;
    let mut batch = ArrayVec::<T, BATCH_CAPACITY>::new();
    batch.push(first);
    let deadline = tokio::time::sleep(FRAME_INTERVAL);
    tokio::pin!(deadline);
    while !batch.is_full() {
        tokio::select! {
            biased;
            _ = &mut deadline => break,
            event = receiver.recv() => match event {
                Some(event) => batch.push(event),
                None => break,
            },
        }
    }
    Some(batch.into_iter().collect())
}

pub fn live(data: DataService, scope: Scope) -> impl Stream<Item = Event> {
    iced::stream::channel(2, move |mut output| async move {
        let scope = Arc::new(scope);
        let (sender, mut receiver) = mpsc::channel(CHANNEL_CAPACITY);
        let (shutdown, stop) = watch::channel(false);
        let source_scope = Arc::clone(&scope);
        let source_sender = sender.clone();
        let task = tokio::spawn(async move {
            let mut retry = false;
            loop {
                if retry && source_sender.send(MarketEvent::Resync).await.is_err() {
                    return;
                }
                let result = data
                    .stream(
                        source_scope.market.clone(),
                        source_scope.cvd,
                        source_sender.clone(),
                        stop.clone(),
                    )
                    .await;
                if source_sender.is_closed() || *stop.borrow() {
                    return;
                }
                let message = match result {
                    Ok(()) => "Native data stream ended; reconnecting".to_owned(),
                    Err(error) => format!("Native data unavailable: {error:#}; reconnecting"),
                };
                if source_sender
                    .send(MarketEvent::Status {
                        connected: false,
                        message: message.clone(),
                    })
                    .await
                    .is_err()
                {
                    return;
                }
                if source_scope.cvd
                    && source_sender
                        .send(MarketEvent::FlowStatus {
                            connected: false,
                            message,
                        })
                        .await
                        .is_err()
                {
                    return;
                }
                tokio::select! {
                    _ = source_sender.closed() => return,
                    _ = tokio::time::sleep(Duration::from_secs(2)) => {},
                }
                retry = true;
            }
        });
        let _producer = Producer {
            task,
            shutdown: Some(shutdown),
        };
        drop(sender);
        while let Some(events) = next_batch(&mut receiver).await {
            if output
                .send(Event {
                    scope: Arc::clone(&scope),
                    events,
                })
                .await
                .is_err()
            {
                return;
            }
        }
    })
}

pub fn open_interest(
    data: DataService,
    scope: OpenInterestScope,
) -> impl Stream<Item = OpenInterestBatch> {
    iced::stream::channel(2, move |mut output| async move {
        let scope = Arc::new(scope);
        let (sender, mut receiver) = mpsc::channel(CHANNEL_CAPACITY);
        let (shutdown, stop) = watch::channel(false);
        let source_scope = Arc::clone(&scope);
        let task = tokio::spawn(async move {
            loop {
                let result = data
                    .open_interest_stream(source_scope.market.clone(), sender.clone(), stop.clone())
                    .await;
                if result.is_ok() || sender.is_closed() || *stop.borrow() {
                    return;
                }
                let error = result.expect_err("checked native OI error");
                if sender
                    .send(OpenInterestEvent::Status {
                        connected: false,
                        message: format!("Native OI unavailable: {error:#}; reconnecting"),
                    })
                    .await
                    .is_err()
                {
                    return;
                }
                tokio::select! {
                    _ = sender.closed() => return,
                    _ = tokio::time::sleep(Duration::from_secs(2)) => {},
                }
            }
        });
        let _producer = Producer {
            task,
            shutdown: Some(shutdown),
        };
        while let Some(events) = next_batch(&mut receiver).await {
            if output
                .send(OpenInterestBatch {
                    scope: Arc::clone(&scope),
                    events,
                })
                .await
                .is_err()
            {
                return;
            }
        }
    })
}

pub fn enriched_history(
    data: DataService,
    market: Market,
    before: Option<i64>,
    mut cancel: oneshot::Receiver<()>,
) -> impl Stream<Item = Result<Box<[HistoryEvent]>, String>> {
    iced::stream::channel(2, move |mut output| async move {
        let (sender, mut receiver) = mpsc::channel(CHANNEL_CAPACITY);
        let task = tokio::spawn(async move {
            // Terminal errors are already delivered through the typed event channel.
            let _ = data.history_with_flow(market, before, sender).await;
        });
        let _producer = Producer {
            task,
            shutdown: None,
        };
        loop {
            let batch = tokio::select! {
                _ = &mut cancel => {
                    let _ = output.send(Err("History replay cancelled".into())).await;
                    return;
                },
                batch = next_batch(&mut receiver) => batch,
            };
            let Some(batch) = batch else {
                let _ = output
                    .send(Err("Native history ended before completion".into()))
                    .await;
                return;
            };
            let terminal = batch
                .iter()
                .any(|event| matches!(event, HistoryEvent::Finished | HistoryEvent::Error { .. }));
            if output.send(Ok(batch)).await.is_err() || terminal {
                return;
            }
        }
    })
}
