# HANDOFF — vyapaar-runtime Tier 3B migration (in progress)

Date: 2026-11-05 · Full context: `/project1/vyapaar-labs/.agents/HANDOFF.md` (superproject).

## Gate status
- `cargo check --workspace --all-targets`, `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`: ✅ green.
- `grep -rn "#\[cfg(test)\]" crates/*/src/`: ✅ 0 matches.
- `cargo test --workspace`: ⚠️ 3 failures, all in `crates/vyapaar-runtime-api-rest/tests/rest_backtest.rs`:
  1. `a_full_queue_is_429_with_a_reason` — body correct (`rate_limited`/`run_queue_full`) but HTTP status 500, expected 429. Add `ErrorCode::RateLimited => 429` (and check `Risk*` codes) to the router's error→status mapping.
  2. `a_journal_at_another_schema_version_is_422_unsupported` — journal reader returns 500 `journal_corrupt` for numeric `schema_version`; must report 422 `unsupported` with `context.found`.
  3. `rest_backtest_roundtrip` — journal `trades[0]["instrument_id"]` is `Null`; fill's instrument serializes under a different key; make the journal trade emit `instrument_id`.

## Already fixed this session (do not regress)
- ADR 0018 order gate in `api-rest` (`orders.rs`/`approval.rs`/`exec.rs`): new `Risk*`/`RateLimited` ErrorCodes, state→instrument→lot→tick→collar→engine check order, lowercase side + numeric qty wire shape, dot-form queue instrument ids, close-to-flat sell fix, collar via `LiveQuoteStore::read_fallback`.
- Backtest algo registration in `executor.rs` (strategy's algo id registered on the `RiskEngine` before `run_events`).
- `tests/support/mod.rs::TestBarReader` now returns the 80 daily TCS fixture bars.
- `service.rs` run queue-full → `RateLimited`.
- `vyapaar-market-store/src/live_quotes.rs` gained `LiveQuoteStore::read_fallback`.

## Not committed yet
Nothing in this submodule has been committed (all tier-3B work is staged/unstaged in the working tree). Commit plan: (1) ported crates src, (2) workspace wiring + new shim crates + configs/schema, (3) tests. Then update `docs/roadmap/roadmap.md` milestones.

## Sibling enablers to commit in their own submodules
- `vyapaar-core`: `MarketEvent` re-export + `FillReport.account_id`.
- `vyapaar-ledger`: `match_lots(&mut self, &FillReport)` signature.
- `vyapaar-market`: untracked `crates/vyapaar-market-store/` + `read_fallback`.
