# Plans Index — `vyapaar-runtime`

Per workspace rule `.agents/rules/gap-tracking.md`:
- Gaps are individual features (`docs/gaps/XXXX-*.md`).
- Plans are collections of gaps (`docs/plans/XXXX-*.md`).
- When implemented, plan documents are cleaned up from `docs/plans/` and recorded in this index.

## Implemented Plans

| # | Title | Target Scope | Implemented In |
|---|-------|--------------|----------------|
| 0001 | Distributed Jobs Engine, Run Lifecycle, and Strategy Dispatch Runtime | `vyapaar-runtime-jobs`, `vyapaar-runtime-strategy`, `vyapaar-runtime-api-rest` | Stage 1 (RunId, journals, recovery) in `60a5bd9`; Stages 2–4 (pool, registry, budgeting/fencing, cron scheduler, typed manifests, broker dispatch) in `46817a0`; Stage 4 CLI/agent wiring in the `vyapaar-ui` repo (`vyapaar-cli` — `vyapaar jobs run/tick/logs`, tool-call journals, approvals queue) |
| 0002 | Comprehensive Statistical Gates, Microstructure Traps, and Overfitting Verification | `vyapaar-core::analytics`, `vyapaar-runtime-backtest` | `vyapaar-core::analytics` six-gate kernel (DSR, PBO/CSCV, walk-forward, holdout, block-bootstrap Monte Carlo, cost stress) + `ValidationBlock` (30 tests in `tests/analytics_gates_tests.rs`); backtest E2 gap-through-stop penalties + E3 conservative same-bar stop ordering (6 tests in `tests/stop_fill_tests.rs`); M5 freeze limits via `vyapaar-risk`. Closed with GAP-0002. |
