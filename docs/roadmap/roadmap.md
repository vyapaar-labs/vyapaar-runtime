# `vyapaar-runtime` — TDD & Engineering Roadmap

## Milestone 1: Config, Testing Harness & Pure Strategy Runtime (P0) — [COMPLETED]
- [x] Port `vyapaar-runtime-config`, `vyapaar-runtime-testing`, and `vyapaar-runtime-strategy`.
- [x] Relocate `src/tests/` out of `vyapaar-runtime-strategy/src/` and `vyapaar-runtime-testing/src/` into `tests/`.

## Milestone 2: Persistence, Backtester & Strategy Enhancer (P0) — [COMPLETED]
- [x] Port `vyapaar-runtime-persistence` implementing `vyapaar-ledger` repository traits.
- [x] Port `vyapaar-runtime-backtest` and `vyapaar-runtime-enhancer`.

## Milestone 3: Supervised Actors, Cron Jobs & Axum REST/WS Gateway (P0) — [COMPLETED]
- [x] Port `vyapaar-runtime-actors`, `vyapaar-runtime-api`, and `vyapaar-runtime-api-rest`.
- [x] Implement `vyapaar-runtime-jobs` (`JobScheduler`, `ScheduledJob`, `JobStatus`, deterministic tick-driven cron evaluator).

## Migration Verification — [COMPLETED]
- [x] `cargo fmt --check` — clean
- [x] `cargo clippy --all-targets -- -D warnings` — clean
- [x] `cargo test --workspace` — passing
- [x] Zero inline `#[cfg(test)]` blocks in `src/`
- [x] Zero-I/O domain logic — simulation clock stepped deterministically
- [x] Zero floats — all accounting and prices use `Decimal`

See `docs/roadmap/migration-verification.md` for full migration milestone status.
