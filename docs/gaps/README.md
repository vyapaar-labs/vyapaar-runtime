# Gaps Index — `vyapaar-runtime`

Per workspace rule `.agents/rules/gap-tracking.md`:
- Gaps are individual features (`docs/gaps/XXXX-*.md`).
- Plans are collections of gaps (`docs/plans/XXXX-*.md`).
- When implemented, gap documents are cleaned up from `docs/gaps/` and recorded in this index.

## Open Gaps

| # | Title | Target Scope | Status |
|---|-------|--------------|--------|
| [0002](0002-strategy-traps-pitfalls-and-statistical-validation.md) | Strategy Traps, Pitfalls, and Statistical Validation Coverage | `vyapaar-runtime-backtest` | Approved / Critical Gap Audit |
| [0003](0003-broker-wire-transport-scaffolds.md) | Broker Wire-Transport Scaffolds (Zerodha Kite, Angel One SmartAPI) | `vyapaar-runtime-strategy::dispatch` | Open (stub) |

## Closed Gaps

| # | Title | Target Scope | Closed In |
|---|-------|--------------|-----------|
| 0001 | Jobs, Runner Lifecycle, and Strategy Dispatch Architecture | `vyapaar-runtime-jobs`, `vyapaar-runtime-strategy`, `vyapaar-runtime-api-rest` | Closed in `60a5bd9` (Stage 1) + `46817a0` (Stages 2–4: pool, registry, budgeting/fencing, cron scheduler, typed manifests, broker dispatch; CLI half in the `vyapaar-ui` repo). Broker wire transports remain open as GAP-0003 |
