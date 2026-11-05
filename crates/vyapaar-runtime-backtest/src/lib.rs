//! # 本場・検 vyapaar-runtime-backtest
//!
//! Research-live parity event-driven backtesting engine. Indian statutory
//! cost modeling, T+1 settlement timing, and position/PnL tracking are all
//! delegated to `vyapaar_ledger`/`vyapaar_ledger_portfolio` — the same canonical
//! implementations the live-trading actor path uses — so a backtest and a live run
//! agree byte-for-byte on cost and settlement math.
//! This crate owns the real NSE trading/holiday calendar
//! (`NseTradingCalendar`), zero-copy Arrow/Parquet catalog ingestion
//! (`CatalogLoader`), and the event-driven simulation loop itself.
//!
//! Event loop:
//! $$\text{MarketEvent} \longrightarrow \text{SignalEvent} \longrightarrow \text{OrderEvent} \longrightarrow \text{FillEvent}$$
//!
//! Uses the identical [`Strategy`] and [`RiskEngine`] traits as live execution.

pub mod calendar;
pub mod catalog;
pub mod engine;
pub mod events;
pub mod sim;

pub use calendar::{NseTradingCalendar, SessionWindow};
pub use catalog::{
    CatalogError, CatalogInstrumentSummary, CatalogLoader, CatalogVerificationReport,
    decimal_from_f64_lossless,
};
pub use engine::{BacktestConfig, BacktestEngine, BacktestResult, decimal_qty_to_u64};
pub use events::{FillEvent, MarketEvent, OrderEvent, SignalEvent};
pub use vyapaar_core::TradingCalendar;
pub use vyapaar_ledger_journal::Transaction as LedgerTransaction;
pub use vyapaar_ledger_portfolio::{
    ClearingCalendar, DematHoldings, FillSettlementResult, SettlementEngine, SettlementInstruction,
};
