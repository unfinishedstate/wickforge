//! Native priced tape. Ranges are half-open UTC milliseconds; quantities retain
//! catalog native units (including contracts on inverse instruments).
use crate::{Exchange, Market, Product, Timeframe};
use rust_decimal::Decimal;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NativeInstrument {
    pub exchange: Exchange,
    pub product: Product,
    pub symbol: String,
}
impl NativeInstrument {
    pub fn from_market(market: &Market) -> Self {
        Self {
            exchange: market.exchange,
            product: market.product,
            symbol: market.symbol.clone(),
        }
    }
    pub fn market(&self, interval: Timeframe) -> Market {
        Market {
            exchange: self.exchange,
            product: self.product,
            symbol: self.symbol.clone(),
            interval,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Execution {
    pub time_ms: i64,
    /// Venue-native identity with raw/aggregate namespace. Never a timestamp hash.
    pub id: String,
    pub price: Decimal,
    pub quantity: Decimal,
}
#[derive(Debug, Clone, PartialEq)]
/// Native source integrity over a half-open UTC-millisecond range. `complete`
/// requires replay/sequence proof; merely receiving priced trades is not proof.
pub struct Coverage {
    pub start_ms: i64,
    pub end_ms: i64,
    pub complete: bool,
    pub reason: Option<String>,
}
#[derive(Debug, Clone)]
pub struct ExecutionPage {
    pub executions: Vec<Execution>,
    pub coverage: Vec<Coverage>,
    /// Exclusive observed boundary, not a substitute for `coverage` proof.
    pub observed_through_ms: i64,
}
#[derive(Debug, Clone)]
pub enum ExecutionEvent {
    Page(Arc<ExecutionPage>),
    Status {
        connected: bool,
        message: String,
    },
    Invalidate {
        start_ms: i64,
        end_ms: i64,
        reason: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_identity_excludes_display_timeframe() {
        let a = Market::default();
        let mut b = a.clone();
        b.interval = Timeframe::Min1;
        assert_eq!(
            NativeInstrument::from_market(&a),
            NativeInstrument::from_market(&b)
        );
        assert_eq!(NativeInstrument::from_market(&a).market(a.interval), a);
    }
}
