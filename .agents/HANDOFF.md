# HANDOFF — vyapaar-runtime Tier 3B migration (COMPLETE through Stage 3)

Date: 2026-11-09 · Full context: `/project1/vyapaar-labs/.agents/HANDOFF.md` (superproject).

## Gate status — ✅ GREEN
- `cargo fmt --check`: ✅
- `cargo clippy --workspace --all-targets --profile prod -- -D warnings`: ✅
- `cargo test --workspace --profile prod`: ✅ (17 passed, 0 failed)
- `make prod` (§11 end-of-turn rebuild): ✅ (fresh `target/prod`)
- `grep -rn "#\[test\]\|#\[cfg(test)\]" crates/*/src/`: ✅ 0 matches

## What was implemented (PLAN-0001 Stages 1–3)
- **Stage 1 (RunId & Journal Foundation)**: Crockford Base32 `RunId` + monotonic `RunIdGenerator` (path-traversal rejection), `FileJournalWriter` (`manifest.json`/`events.ndjson`/`trades.json`/`metrics.json`), `recover_interrupted_runs` crash sweeper. Tests: `crates/vyapaar-runtime-api/tests/run_id_tests.rs`, `crates/vyapaar-runtime-api-rest/tests/journal_tests.rs`.
- **Stage 2 (Bounded Worker Pool & Sweep Budgeting)**: `BoundedWorkerPool` (`crates/vyapaar-runtime-jobs/src/pool.rs`) with 429 backpressure (`QueueSaturated`), panic isolation, graceful shutdown; Jesse-style `budget_sweep_cores`/`dispatch_sweep`; `JobBudgets`/`BudgetUsage`, `FencingLease` heartbeat fencing, `FileJobRegistry` (`jobs.json` atomic store, leases, approvals queue). Tests: `pool_tests.rs`, `registry_tests.rs`.
- **Stage 3 (Strategy Dispatch & Broker Boundary)**: `BrokerAdapter` boundary + `Dispatcher` in `crates/vyapaar-runtime-strategy/src/dispatch.rs` — every intent converts to `OrderRequest`, passes the synchronous SEBI `RiskEngine::check_order` (<15 µs SLO test) and only then reaches the adapter; `PaperAdapter`/`ZerodhaAdapter`/`AngelOneAdapter` scaffolds with identical economics; OTR-tracked cancellations; kill-switch refusal protects the broker. Tests: `dispatch_tests.rs` (7 tests pass).

## Stage 3 verification (latest run)
- All dispatch tests pass:
  - `test_sebi_risk_gate_latency_slo`: median <15µs, p95 ≤15µs (debug: ≤25µs) — ✅
  - `test_identical_fills_and_equity_paper_vs_zerodha_vs_angelone`: identical fills, positions, cash, equity — ✅
  - `test_order_cancellation_and_otr_recording`: SEBI OTR audit trail recording — ✅
  - `test_risk_refusal_halts_and_protects_broker`: kill-switch blocks broker dispatch — ✅
  - `test_invalid_intent_rejected_before_risk_and_broker`: intent validation precedes risk gate — ✅
  - `test_fill_timestamp_follows_dispatch_clock`: deterministic timestamps — ✅
  - `test_full_replay_is_byte_identical`: byte-identical journal replay — ✅

## Stage 3 hardening (latest change)
- **Explicit clock at the adapter boundary (temporal determinism)**: `BrokerAdapter::{submit_order, cancel_order}` now receive `now: DateTime<Utc>`; simulated fills are stamped with the dispatch clock — the sim fill path no longer queries `Utc::now()` (last wall-clock call in the crate removed).
- **Deterministic identity end-to-end**: fill IDs are per-venue monotonic `FILL-####` (was `Uuid::new_v4`); `OrderIntent::into_order_with_id` / `Dispatcher::dispatch_intent_with_id` mint caller-owned `client_order_id`s (mirrors `vyapaar_core::OrderRequest::with_id`); `dispatch_intent` stays the interactive convenience path (random ID, documented).
- **Replay proof**: `test_full_replay_is_byte_identical` re-runs a fixed intent sequence through a fresh dispatcher and asserts every fill serializes byte-identically (`events.ndjson` journal line shape) and round-trips; `test_fill_timestamp_follows_dispatch_clock` pins the clock contract.
- **Gap filed**: `docs/gaps/0003-broker-wire-transport-scaffolds.md` (+ README row) — Zerodha/Angel One adapters remain simulation scaffolds (no Kite/SmartAPI HTTP transport, zero-fee sim fills); real transports belong in `vyapaar-exec`/`vyapaar-adapters`.
- Decisions: `BrokerAdapter` trait had no consumers outside this crate (verified), so signature extension was safe; economic-parity test (`test_identical_fills_and_equity_paper_vs_zerodha_vs_angelone`) intentionally stays on the random-ID path per pairwise-adapter design.

## Do not regress
- Zero in-file `#[test]` / `#[cfg(test)]` in `crates/*/src/`.
- Strict canonical `prod` profile (`target/prod`); `make prod` before saying done.
- Zero floats in all financial accounting (`rust_decimal::Decimal`); no wall-clock/random inside sim state machines — state transitions take explicit `DateTime<Utc>`/IDs from callers.
- ADR 0018 order gate, algo registration in the executor, `TestBarReader` fixture bars, `service.rs` queue-full `RateLimited`, `live_quotes.rs::read_fallback`.
- WebSocket tests must always give the awaited stream a timeout (a hung `ws_stream.next()` blocks the whole workspace run).

## Stage 4 runtime-side (latest change, PLAN-0001)
- **Full Vixie cron grammar in `JobScheduler::next_execution`**
  (`crates/vyapaar-runtime-jobs/src/lib.rs`): evaluation delegates to the
  `cron` crate (0.17) over a 6-field form, with `translate_dow_field` mapping
  Vixie 0/7-Sunday dow into the crate's 1=Monday scheme and OR-combining
  dom/dow exactly like cron (`* * * 13 * 5` = 13th OR Friday). Validation is
  tightened up front: alphabetic/`@daily`-style tokens and out-of-range fields
  are rejected at ingestion; `?` is accepted as a synonym of `*`. 12 tests in
  `crates/vyapaar-runtime-jobs/tests/scheduler_cron_tests.rs`.
- **Typed-free job/tool manifests**: `FileJournalWriter::write_manifest(&Value)`
  flips an in-flight run's `manifest.json` (Queued -> Running) and
  `finalize_value(&Value, &Value, &Value)` writes manifest/trades/metrics for
  manifests that are *not* backtest runs — `kind: "job"` scheduled-run
  manifests and `kind: "tool"` agent-tool manifests (CLI-side shapes:
  `vyapaar-ui/.agents/HANDOFF.md`). `journal_tests.rs` extended to 7 tests.
- CLI/agent half of Stage 4 landed in `vyapaar-ui` (`vyapaar-cli`): `vyapaar
  jobs run/tick/logs` on `BoundedWorkerPool` + `FileJobRegistry`, agent
  tool-call journals, approval-queue gating — see that repo's HANDOFF.

## Gate status
- `cargo clippy --workspace --all-targets --profile prod -- -D warnings`: ✅
- `cargo test --workspace --profile prod`: ✅ 117 targets, 445 passed, 0 failed
- `make prod` (§11 end-of-turn rebuild): ✅ fresh `target/prod`

## GAP-0002 remediation complete
- **Statistical validation kernel & backtest stop-fill engine**:
  - `vyapaar-runtime-backtest`: implemented overnight gap-through-stop fill penalties (E2) and conservative same-bar stop ordering (E3) in `crates/vyapaar-runtime-backtest/src/engine.rs` + 6 tests in `tests/stop_fill_tests.rs`.
  - M5 freeze limits already fully enforced upstream via `vyapaar-risk` (`limits.rs`, `freeze_limits: HashMap<InstrumentId, Quantity>`) and `vyapaar-exec`.
  - Retired GAP-0002 (removed doc, moved to Closed Gaps table in `docs/gaps/README.md`).

## Gate status
- `cargo clippy --workspace --all-targets --profile prod -- -D warnings`: ✅
- `cargo test --workspace --profile prod`: ✅ all targets passed, 0 failed
- `make prod` (§11 end-of-turn rebuild): ✅ fresh `target/prod`

## Immediate next step
GAP-0002 closed successfully across `vyapaar-core`, `vyapaar-runtime`, and `vyapaar-ui`. Open follow-up is GAP-0003 (broker wire transports).
