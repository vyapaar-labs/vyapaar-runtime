# Gaps Index — `vyapaar-runtime`

Per workspace rule `.agents/rules/gap-tracking.md`:
- Gaps are individual features (`docs/gaps/XXXX-*.md`).
- Plans are collections of gaps (`docs/plans/XXXX-*.md`).
- When implemented, gap documents are cleaned up from `docs/gaps/` and recorded in this index.

## Open Gaps

| # | Title | Target Scope | Status |
|---|-------|--------------|--------|

## Closed Gaps

| # | Title | Target Scope | Closed In |
|---|-------|--------------|-----------|
| 0001 | Jobs, Runner Lifecycle, and Strategy Dispatch Architecture | `vyapaar-runtime-jobs`, `vyapaar-runtime-strategy`, `vyapaar-runtime-api-rest` | Closed in `60a5bd9` (Stage 1) + `46817a0` (Stages 2–4: pool, registry, budgeting/fencing, cron scheduler, typed manifests, broker dispatch; CLI half in the `vyapaar-ui` repo). Broker wire transports remain open as GAP-0003 |
| 0002 | Strategy Traps, Pitfalls, and Statistical Validation Coverage | `vyapaar-core::analytics`, `vyapaar-runtime-backtest`, `vyapaar-ui-core` | Closed in batch `GAP-0002` (pure-Decimal analytics kernel S1–S6, backtest stop-fill gap-through / same-bar ordering E2/E3, M5 freeze limits via `vyapaar-risk`, and UI six-gate audit card). |
| 0003 | Broker Wire-Transport Scaffolds (Zerodha Kite, Angel One SmartAPI) | `vyapaar-runtime-strategy::dispatch` | Closed — Zerodha `ZerodhaKiteAdapter` wired to the real `vyapaar-exec-zerodha` Kite Connect transport (`POST /orders/regular`, `DELETE /orders/regular/{id}`) when credentials are configured, with `broker_name`/`submit_order`/`cancel_order` delegating to `vyapaar-exec`; statutory `fee_breakdown` computed on every sim fill via `vyapaar-market-finance::india::costs`; `BrokerError` normalized into `DispatchError` via `map_broker_error`; wire delegation, error-mapping (session/rate/network), and cancel tests added in `tests/zerodha_wire_tests.rs`; `test_identical_fills_and_equity_paper_vs_zerodha_vs_angelone` now asserts real (non-zero) fees plus paper/zerodha/angelone parity. Angel One remains a simulation scaffold pending `vyapaar-exec-angelone` (upstream placeholder); golden-vector conformance over recorded broker sessions is follow-up. |
