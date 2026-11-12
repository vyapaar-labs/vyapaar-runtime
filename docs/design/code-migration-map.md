# `vyapaar-runtime` — Historical Migration Map (honba-ya → vyapaar-runtime)

> **Tier**: Tier 3B (Depends on `vyapaar-core`, `vyapaar-risk`, `vyapaar-ledger`, `vyapaar-market`, `vyapaar-exec`)
> **Source**: `honba-workspace/honba-ya`
> **Target**: `vyapaar-runtime`
> **Status**: Migration complete (all 10 sub-crates ported).

## Sub-Crate Mapping Table (Migrated in Listed Order)

| Order | Source Crate (`honba-ya/crates/`) | Target Crate (`vyapaar-runtime/crates/`) | Key Refactoring & Renames |
| :---: | :--- | :--- | :--- |
| 1 | `honba-ya-config` | `vyapaar-runtime-config` | Port config loaders and TOML schemas. |
| 2 | `honba-ya-testing` | `vyapaar-runtime-testing` | Move `src/tests/*.rs` to `tests/`. Remove `#[cfg(test)] mod tests;` from `src/lib.rs`. |
| 3 | `honba-ya-strategy` | `vyapaar-runtime-strategy` | Move `src/tests/*.rs` to `tests/`. Remove `#[cfg(test)] mod tests;` from `src/lib.rs`. Wire to `vyapaar_risk`. |
| 4 | `honba-ya-persistence` | `vyapaar-runtime-persistence` | Copy `migrations/` and `src/`. Implement `vyapaar_ledger` repository traits. |
| 5 | `honba-ya-backtest` | `vyapaar-runtime-backtest` | Wire to `vyapaar_ledger_journal`, `vyapaar_ledger_portfolio`, `vyapaar_market_finance`, `vyapaar_runtime_strategy`. |
| 6 | `honba-ya-enhancer` | `vyapaar-runtime-enhancer` | Port GA, Bayesian TPE, WFO, DSR, and Odysseus optimizers. |
| 7 | `honba-ya-actors` | `vyapaar-runtime-actors` | Port actor pipeline and supervision tree. |
| 8 | *(New Crate)* | `vyapaar-runtime-jobs` | Implement deterministic `JobScheduler`, `CronSchedule`, `JobSpec` + `tests/unit_tests.rs`. |
| 9 | `honba-ya-api` | `vyapaar-runtime-api` | Port API service traits and models. |
| 10 | `honba-ya-api-rest` | `vyapaar-runtime-api-rest` | Port Axum REST routes, `/ws/live` WebSocket broadcaster, screener endpoints, and OpenAPI generator. |
| *(new)* | *(new)* | `vyapaar-runtime-feed` | Market data feed adapter crate. |
| *(new)* | *(new)* | `vyapaar-runtime-market` | Market data normalization crate. |
| *(new)* | *(new)* | `vyapaar-indicators` | Technical indicators crate. |
| *(new)* | *(new)* | `vyapaar-runtime-test-utils` | Shared test utilities crate. |

## Mandatory DDD Fixes During Migration

1. **Strict Parallel Test Directory Compliance**:
   - In `honba-ya-strategy`, `src/tests/` (`context.rs`, `dyn_strategy.rs`, `intent.rs`, `ir.rs`, `ledger.rs`, `look_ahead.rs`, `manifest.rs`, `mtf.rs`, `runner_*.rs`) lived inside `src/`. Move them into `crates/vyapaar-runtime-strategy/tests/`.
   - In `honba-ya-testing`, `src/tests/` (`assert.rs`, `feed.rs`, `fixtures.rs`, `ports.rs`, `recorder.rs`, `scripted_gateway.rs`) lived inside `src/`. Move them into `crates/vyapaar-runtime-testing/tests/`.
2. **Import Replacements**:
   - `honba_core` → `vyapaar_core`
   - `honba_mon` → `vyapaar_risk`
   - `honba_kokoro` → `vyapaar_ledger`
   - `honba_kokoro_oms` → `vyapaar_ledger_oms`
   - `honba_kokoro_portfolio` / `honba_portfolio` → `vyapaar_ledger_portfolio`
   - `honba_kokoro_ledger` → `vyapaar_ledger_journal`
   - `honba_me_*` → `vyapaar_market_*` (`honba_me_market` → `vyapaar_market_finance`)
   - `honba_te_*` → `vyapaar_exec_*`
   - `honba_ya_*` → `vyapaar_runtime_*`
3. Copy `honba-ya/configs/` into `vyapaar-runtime/configs/`.

## Verification Commands

```bash
# Run from the repository root
grep -rn "#\[cfg(test)\]" crates/*/src/ && exit 1 || true
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --workspace
```

## Migration Milestone Status

| Milestone | Status |
|-----------|--------|
| Milestone 1: Config, Testing Harness & Pure Strategy Runtime | Complete |
| Milestone 2: Persistence, Backtester & Strategy Enhancer | Complete |
| Milestone 3: Supervised Actors, Cron Jobs & Axum REST/WS Gateway | Complete |
| Zero inline `#[cfg(test)]` in `src/` | Complete |
| `cargo fmt --check` | Clean |
| `cargo clippy --all-targets -- -D warnings` | Clean |
| `cargo test --workspace` | Passing |
