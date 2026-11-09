# `vyapaar-runtime` — Migration Verification Milestone

Verification commands for the `honba-ya → vyapaar-runtime` migration:

```bash
cd /home/pmallapp/Devel/Finance/vyapaar-labs/vyapaar-runtime
grep -rn "#\[cfg(test)\]" crates/*/src/ && exit 1 || true
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --workspace
```

## Migration Milestone Status

| Milestone | Status |
|-----------|--------|
| `vyapaar-runtime-config` — Typed TOML/env configuration | Complete |
| `vyapaar-runtime-testing` — Deterministic `MockBroker`, `ScriptedGateway` | Complete |
| `vyapaar-runtime-strategy` — `Strategy` trait, `StepPipeline` | Complete |
| `vyapaar-runtime-persistence` — SQLite/PostgreSQL ledger repositories | Complete |
| `vyapaar-runtime-backtest` — Event-driven backtester | Complete |
| `vyapaar-runtime-enhancer` — GA, Bayesian TPE, WFO, DSR optimizers | Complete |
| `vyapaar-runtime-actors` — Supervised Tokio actor pipeline | Complete |
| `vyapaar-runtime-jobs` — `JobScheduler`, `CronSchedule`, `JobSpec` | Complete |
| `vyapaar-runtime-api` — API service traits and DTOs | Complete |
| `vyapaar-runtime-api-rest` — Axum REST + `/ws/live` WebSocket | Complete |
| `vyapaar-runtime-feed`, `vyapaar-runtime-market`, `vyapaar-indicators` | Complete |
| `vyapaar-runtime-test-utils` — Shared test utilities | Complete |
| `cargo fmt --check` | Clean |
| `cargo clippy --all-targets -- -D warnings` | Clean |
| `cargo test --workspace` | Passing |
