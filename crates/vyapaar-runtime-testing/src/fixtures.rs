//! Fixtures for backtesting and simulation.

use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use vyapaar_core::{Bar, Currency, Exchange, InstrumentId, Money, PositionSide, Price, Quantity};
use vyapaar_ledger_portfolio::{ClearingCalendar, Product, SettlementInstruction};

/// Current UTC time fixture.
pub fn utc_now() -> DateTime<Utc> {
    DateTime::from_timestamp(1730716800, 0).unwrap() // 2024-11-04 00:00:00 UTC
}

/// Builds a UTC timestamp from `u64` nanoseconds since the Unix epoch.
pub fn utc_nanos(nanos: u64) -> DateTime<Utc> {
    let secs = (nanos / 1_000_000_000) as i64;
    let sub = (nanos % 1_000_000_000) as u32;
    DateTime::from_timestamp(secs, sub).expect("fixture timestamp out of range")
}

/// Converts an `f64` fixture literal into an exact [`Decimal`].
///
/// # Panics
///
/// Panics if the value is not a finite decimal literal.
pub fn exact_decimal(value: f64) -> Decimal {
    value
        .to_string()
        .parse::<Decimal>()
        .unwrap_or_else(|_| panic!("fixture f64 must be a finite decimal, got {value}"))
}

/// Common INR currency fixture.
pub fn currency_inr() -> Currency {
    Currency::Inr
}

/// Round-lot quantity fixture (15 shares for Indian equities).
pub fn round_lot() -> Quantity {
    Quantity::try_new(dec!(15)).unwrap()
}

/// Round-lot price fixture (100.00 INR).
pub fn round_price() -> Price {
    Price::try_new(dec!(100.00)).unwrap()
}

/// Instrument `symbol` on the NSE exchange.
pub fn instrument(symbol: &str) -> InstrumentId {
    InstrumentId::new(Exchange::Nse, symbol)
}

/// A placeholder instrument (`X` on NSE) for tests where the instrument does
/// not matter.
pub fn any_instrument() -> InstrumentId {
    instrument("X")
}

/// A flat bar: open, high, low and close all equal `close`, volume 1, and the
/// timestamp at `ts` nanoseconds since the Unix epoch.
pub fn flat_bar(symbol: &str, close: f64, ts: u64) -> Bar {
    let price = Price::new(exact_decimal(close));
    Bar {
        instrument: instrument(symbol),
        open: price,
        high: price,
        low: price,
        close: price,
        volume: Quantity::from_u64(1),
        timestamp: utc_nanos(ts),
    }
}

/// Default settlement instruction fixture.
pub fn settlement_instruction() -> SettlementInstruction {
    SettlementInstruction {
        trade_id: "default".into(),
        instrument: instrument("RELIANCE"),
        side: PositionSide::Long,
        quantity: round_lot(),
        price: round_price(),
        product: Product::Delivery,
        trade_date: NaiveDate::from_ymd_opt(2024, 11, 4).unwrap(),
        settle_date: NaiveDate::from_ymd_opt(2024, 11, 5).unwrap(),
        account_id: "default".into(),
        cash_flow: Money::zero_inr(),
    }
}

/// Default clearing calendar fixture.
pub fn clearing_calendar() -> ClearingCalendar {
    ClearingCalendar::new()
}
