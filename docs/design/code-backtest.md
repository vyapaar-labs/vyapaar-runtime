# `vyapaar-runtime` — Event-Driven Backtester Reference

Documents `crates/vyapaar-runtime-backtest/src/`: the event-driven backtester with
exact research-live parity.

## Event Pipeline

```
MarketEvent → SignalEvent → OrderEvent → FillEvent
```

## Key Components

| Component | File | Purpose |
|-----------|------|---------|
| `Engine` | `engine.rs` | Core backtest loop, event processing |
| `CatalogLoader` | `catalog.rs` | Arrow Parquet catalog data loading |
| `Calendar` | `calendar.rs` | Trading calendar (NSE/BSE sessions) |
| `Events` | `events.rs` | `MarketEvent`, `SignalEvent`, `OrderEvent`, `FillEvent` |
| `L2 Book` | `sim/l2_book.rs` | L2 depth fill simulator |
| `Latency` | `sim/latency.rs` | Simulated latency modeling |
| `MIS` | `sim/mis.rs` | Intraday square-off logic |
| `Slippage` | `sim/slippage.rs` | Slippage models |

## Research-Live Parity

The exact same code path executes in backtesting and live trading — strategies
are unaware of whether they're running against historical Parquet bars or live
WebSocket feeds.

## Indian Market Features

- T+1 Demat settlement
- Indian statutory costs (STT, stamp duty, exchange charges, SEBI turnover fees, GST, DP charges)
- MIS square-off at end of day
- Freeze limit compliance via `vyapaar-risk`

## Related

- `vyapaar-runtime-persistence` — Stores backtest results via `vyapaar-ledger` repository traits
- `vyapaar-runtime-enhancer` — Optimizer runs on backtest results
- `vyapaar-market-finance` — India F&O contract grammar and cost schedules
- `vyapaar-risk` — Pre-trade risk gate applied during backtest
