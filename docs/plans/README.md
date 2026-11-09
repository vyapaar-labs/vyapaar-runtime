# Plans Index — `vyapaar-runtime`

Per workspace rule `.agents/rules/gap-tracking.md`:
- Gaps are individual features (`docs/gaps/XXXX-*.md`).
- Plans are collections of gaps (`docs/plans/XXXX-*.md`).
- When implemented, plan documents are cleaned up from `docs/plans/` and recorded in this index.

## Active Plans

| # | Title | Target Scope | Status |
|---|-------|--------------|--------|
| [0002](0002-comprehensive-statistical-gates-and-trap-verification.md) | Comprehensive Statistical Gates, Microstructure Traps, and Overfitting Verification | `vyapaar-runtime-backtest` | Ready for implementation |

## Implemented Plans

| # | Title | Target Scope | Implemented In |
|---|-------|--------------|----------------|
| 0001 | Distributed Jobs Engine, Run Lifecycle, and Strategy Dispatch Runtime | `vyapaar-runtime-jobs`, `vyapaar-runtime-strategy`, `vyapaar-runtime-api-rest` | Stage 1 (RunId, journals, recovery) in `60a5bd9`; Stages 2–4 (pool, registry, budgeting/fencing, cron scheduler, typed manifests, broker dispatch) in `46817a0`; Stage 4 CLI/agent wiring in the `vyapaar-ui` repo (`vyapaar-cli` — `vyapaar jobs run/tick/logs`, tool-call journals, approvals queue) |
