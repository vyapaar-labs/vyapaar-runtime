# Gap 0008 — Dual Execution-Port Boundary (`BrokerAdapter` vs `vyapaar_exec_core::BrokerExecutor`)

**Repo**: `vyapaar-runtime` · **Scope**: `vyapaar-runtime-strategy::dispatch`
**Status**: Open (documented decision) · **Source of truth**: `src/dispatch.rs`.

## Problem

`vyapaar-runtime` owns a **second** broker abstraction alongside
`vyapaar_exec_core::BrokerExecutor`:

- `BrokerAdapter` (`dispatch.rs`) — **synchronous**, temporally deterministic
  (`submit_order(&order, now: DateTime<Utc>)`), returns `Result<String, String>`.
- `vyapaar_exec_core::BrokerExecutor` — **asynchronous**, canonical exec port.

`BrokerAdapter` is the *intentional* dual-execution-target port (AGENTS §4.3.3):
the sync/`now`-stamped surface lets the backtest and the paper broker replay
byte-identical fills, while real wire transports delegate to `vyapaar-exec`
(Zerodha Kite via `vyapaar_exec_zerodha`, closed gap 0003). The cost is a second
"execution" vocabulary in the runtime and duplicated broker semantics
(`DispatchError` vs `BrokerError` string-mapped in `map_broker_error`).

## Options

1. **Keep `BrokerAdapter` as the runtime's deterministic sim port** (status
   quo, documented here): `BrokerAdapter` = simulation/backtest boundary;
   `BrokerExecutor` = live wire boundary; the `ExecutionActor` composes exec's
   `RiskGatedExecutor`/`RateLimitedBroker` wrappers.
2. **Lift `BrokerAdapter` into `vyapaar-exec-core`** so both execution ports
   live in the Hand bounded context; the runtime only orchestrates. Larger
   refactor — exec gains the sync port; `dispatch.rs` becomes a thin re-export.
3. **Merge onto `BrokerExecutor`**: lose the deterministic sync surface —
   rejected (breaks research-live parity).

## Acceptance Criteria

- The boundary is documented in `docs/design/code-strategy.md` / this gap so
  new contributors don't fork a third abstraction.
- The runtime's `ExecutionActor` composes exec's wrappers (done — see
  `execution.rs::with_exec_stack`) rather than re-implementing risk/rate logic.
- This gap is removed and recorded as Closed when option 2 lands.
