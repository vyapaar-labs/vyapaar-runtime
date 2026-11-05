//! Simulation helpers for backtesting.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use vyapaar_core::{Exchange, InstrumentId, PositionSide, Price, Quantity};
use vyapaar_ledger_portfolio::{Fill, Product};

/// Create a simulated fill for testing.
pub fn simulated_fill(
    instrument: &str,
    side: PositionSide,
    price: Decimal,
    quantity: Decimal,
    timestamp: DateTime<Utc>,
) -> Fill {
    Fill {
        trade_id: format!("SIM-{}", timestamp.timestamp_nanos_opt().unwrap_or(0)),
        instrument: InstrumentId::new(Exchange::Nse, instrument),
        side,
        price: Price::try_new(price).unwrap(),
        quantity: Quantity::try_new(quantity).unwrap(),
        product: Product::Delivery,
        timestamp,
        account_id: "default".into(),
    }
}
