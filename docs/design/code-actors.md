# `vyapaar-runtime` — Actor System Reference

Documents `crates/vyapaar-runtime-actors/src/`: the supervised Tokio actor pipeline
for the central runtime.

## Actor Types

| Actor | File | Responsibility |
|-------|------|----------------|
| `MarketDataActor` | `market_data.rs` | Ingest and distribute market ticks/bars |
| `StrategyActor` | `strategy.rs` | Execute strategy logic, produce order intents |
| `RiskActor` | `risk.rs` | Pre-trade risk gate evaluation |
| `ExecutionActor` | `execution.rs` | Route orders to brokers via `vyapaar-exec` |
| `PositionSizingActor` | `sizing.rs` | Calculate position sizes |
| `PortfolioActor` | `portfolio.rs` | Track portfolio state via `vyapaar-ledger` |

## Actor Communication

- Concurrency via `tokio::sync::mpsc` channels
- Actors communicate through typed `SessionEvent` streams
- OpenTelemetry trace propagation across actor boundaries
- Mailbox metrics for observability

## Supervision Tree

```
Supervisor
├── MarketDataActor
├── StrategyActor
├── RiskActor
├── ExecutionActor
├── PositionSizingActor
└── PortfolioActor
```

## Zero-I/O in Domain Logic

Actor message handling is deterministic — simulation clock is stepped
deterministically for backtesting parity with live trading.

## Related

- `vyapaar-runtime-api-rest` — Axum REST API feeds into actor system
- `vyapaar-risk` — `RiskActor` delegates to `RiskEngine`
- `vyapaar-exec` — `ExecutionActor` routes to broker executors
- `vyapaar-ledger` — `PortfolioActor` uses repository traits
