# Gaps Index — `vyapaar-runtime`

Per workspace rule `.agents/rules/gap-tracking.md`:
- Gaps are individual features (`docs/gaps/XXXX-*.md`).
- Plans are collections of gaps (`docs/plans/XXXX-*.md`).
- When implemented, gap documents are cleaned up from `docs/gaps/` and recorded in this index.

## Open Gaps

| # | Title | Target Scope | Status |
|---|-------|--------------|--------|
| [0001](0001-jobs-and-strategy-dispatch-framework.md) | Jobs, Runner Lifecycle, and Strategy Dispatch Architecture | `vyapaar-runtime-jobs`, `vyapaar-runtime-strategy` | Approved / In progress |
| [0002](0002-strategy-traps-pitfalls-and-statistical-validation.md) | Strategy Traps, Pitfalls, and Statistical Validation Coverage | `vyapaar-runtime-backtest` | Approved / Critical Gap Audit |
| [0003](0003-broker-wire-transport-scaffolds.md) | Broker Wire-Transport Scaffolds (Zerodha Kite, Angel One SmartAPI) | `vyapaar-runtime-strategy::dispatch` | Open (stub) |

## Closed Gaps

| # | Title | Target Scope | Closed In |
|---|-------|--------------|-----------|
