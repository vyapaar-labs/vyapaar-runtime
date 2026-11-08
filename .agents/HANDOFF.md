# HANDOFF — vyapaar-runtime Tier 3B migration (COMPLETE)

Date: 2026-11-05 · Full context: `/project1/vyapaar-labs/.agents/HANDOFF.md` (superproject).

## Gate status — ✅ GREEN
- `cargo fmt --check`: ✅
- `cargo clippy --workspace --all-targets -- -D warnings`: ✅
- `cargo test --workspace`: ✅
- `grep -rn "#\[cfg(test)\]" crates/*/src/`: ✅ 0 matches

## What was implemented (PLAN-0001 ADR 0017 Run Lifecycle & Journaling)
- `RunId`: Crockford Base32 validation against path traversal and invalid characters (`RunIdError::InvalidLength`, `RunIdError::InvalidCharacter`). Tests in `crates/vyapaar-runtime-api/tests/run_id_tests.rs`.
- `FileJournalWriter`: directory structure management (`manifest.json`, `events.ndjson`, `trades.json`, `metrics.json`) with streaming NDJSON event appends and finalization.
- `recover_interrupted_runs`: scans journals directory on reboot/restart to mark unfinalized pending/running runs as `Failed` with `ErrorDetail` reason `interrupted`. Tests in `crates/vyapaar-runtime-api-rest/tests/journal_tests.rs`.
- Zero in-file tests in production `src/` files per §3.2.

All work committed in this submodule (4 commits): ported crates, workspace wiring + shim crates + configs/schema, ported test suites, SQLx migrations.

## What the finishing session fixed
- Run-route error→status mapping: `RateLimited => 429`, `Unsupported => 422`; journal reader reports foreign `schema_version` (string or numeric) as 422 `unsupported` with `context.found`.
- Journal fill record is the transport-owned `vyapaar_runtime_api::wire::Trade` (`instrument` renamed `instrument_id` on the wire).
- Result cache (ADR 0020, E4-S1) wired into `BacktestExecutor::execute`; `RateLimited` is retryable.
- Deterministic order/fill IDs for byte-identical replay (`uuid v5` order ids, engine `fill_seq` fill ids).
- Stale tests updated: `runs.rs` (fills-only journal + `RateLimited` queue-full), `runs_cache.rs` (fill-count assertions + registered strategy name), `trading_state.rs` (ADR 0018 `Risk*` codes), `version_routing.rs` (`application/vnd.vyapaar.v2+json`), `ws.rs` (ticks published via the broadcast channel, 5 s timeout).

## Do not regress
- ADR 0018 order gate, algo registration in the executor, `TestBarReader` fixture bars, `service.rs` queue-full `RateLimited`, `live_quotes.rs::read_fallback`.
- WebSocket tests must always give the awaited stream a timeout (a hung `ws_stream.next()` blocks the whole workspace run).
