use std::sync::Arc;

use mexc_model::HistoryPage;
use tokio::sync::oneshot;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    Recent,
    Repair { before: i64, target: i64 },
    Older { before: i64 },
    FlowBackfill { before: i64, target: i64 },
}

impl Request {
    pub fn before(self) -> Option<i64> {
        match self {
            Self::Recent => None,
            Self::Repair { before, .. }
            | Self::Older { before }
            | Self::FlowBackfill { before, .. } => Some(before),
        }
    }
}

#[derive(Default)]
pub struct HistoryState {
    pub generation: u64,
    pub error: Option<String>,
    pub flow_error: Option<String>,
    pub progress: Option<String>,
    pub page: Option<Arc<HistoryPage>>,
    pub cancel: Option<oneshot::Sender<()>>,
    pub flow_target: Option<i64>,
    pub in_flight: bool,
    pub pending_sync: bool,
    pub anchor: Option<i64>,
    pub failed: Option<Request>,
    pub retry_ticks: u8,
}

impl HistoryState {
    pub fn cancel(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            let _ = cancel.send(());
        }
    }
}

/// Independent request lifetime and cursor for native OI observations.
#[derive(Default)]
pub struct OpenInterestHistoryState {
    pub generation: u64,
    pub next_before_ms: Option<i64>,
    pub initialized: bool,
    pub in_flight: bool,
    pub error: Option<String>,
    pub retry_ticks: u8,
    pub cancel: Option<oneshot::Sender<()>>,
}

impl OpenInterestHistoryState {
    pub fn reset(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            let _ = cancel.send(());
        }
        self.generation = self.generation.wrapping_add(1);
        self.next_before_ms = None;
        self.initialized = false;
        self.in_flight = false;
        self.error = None;
        self.retry_ticks = 0;
    }
}

pub fn repair_request(
    request: Request,
    page: &HistoryPage,
    target: Option<i64>,
) -> Option<Request> {
    let target = target?;
    let oldest = page.candles.first()?.time;
    let before = page.next_before?;
    if oldest > target && request.before().is_none_or(|previous| before < previous) {
        Some(match request {
            Request::FlowBackfill { .. } => Request::FlowBackfill { before, target },
            _ => Request::Repair { before, target },
        })
    } else {
        None
    }
}
