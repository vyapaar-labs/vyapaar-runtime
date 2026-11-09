# GAP-0003 — Broker Wire-Transport Scaffolds (Zerodha Kite Connect, Angel One SmartAPI)

> **Status**: Open (stub tracked per `.agents/rules/gap-tracking.md`)
> **Component Scope**: `vyapaar-runtime-strategy::dispatch`
> **Introduced In**: PLAN-0001 Stage 3 — Strategy Dispatch & Broker Boundary

---

## 1. What Is Stubbed

`ZerodhaAdapter` and `AngelOneAdapter`
(`crates/vyapaar-runtime-strategy/src/dispatch.rs`):
- Are **simulation-mode scaffolds**. They hold Kite Connect v3 (`api_key`,
  `access_token`) and SmartAPI (`api_key`, `client_code`, `jwt_token`)
  credential fields, but every `submit_order` / `cancel_order` is executed
  against the in-process shared `AdapterState` — economically identical to
  `PaperAdapter` (next-bar-instant synthetic fills, mark-to-market equity).
- Produce simulated fills with `fee: Money::zero_inr()` and no
  `fee_breakdown`; the statutory transaction-cost engine (STT, GST, SEBI
  turnover, Stamp Duty, brokerage) is never consulted at this boundary.
- Keep credentials as plain `Option<String>` holders: no TOTP/session-refresh
  lifecycle, no wire-transport failure taxonomy, no market-hour or
  authenticity checks on the venue side.

The `BrokerAdapter` trait itself, the `Dispatcher` risk-gate ordering, and the
`dispatch_intent_with_id` deterministic-replay seam are **fully implemented**
and are the stable contract the eventual wire adapters will plug into.

## 2. Why (Root Cause)

- PLAN-0001 Stage 3 explicitly scopes `PaperAdapter` and `ZerodhaAdapter` as
  *scaffolds* and gates the stage on paper/live **economic parity** (identical
  fills and equity between adapters), not on live exchange integration.
- The real transports belong to other bounded contexts:
  `vyapaar-adapters` (Python broker connectors: Zerodha, Angel One, …) and
  `vyapaar-exec` (Rust: headless TOTP auth, token-bucket rate limiter, Smart
  Order Router, SQLite WAL audit journal). Implementing wire protocols inside
  the strategy crate would leak broker-specific payloads across a domain
  boundary in violation of the Anti-Corruption-Layer rule (workspace
  `AGENTS.md` §3.1).

## 3. How To Close

1. **Wire adapters behind the existing boundary**: implement
   `ZerodhaKiteAdapter` / `AngelOneSmartApiAdapter` conforming to
   `BrokerAdapter`, with real HTTP transports (Kite Connect v3
   `POST /orders/regular`; SmartAPI `placeOrder` / `cancelOrder`), delegating
   auth/session refresh and rate limiting to `vyapaar-exec` — or bridging the
   `vyapaar-adapters` Python connectors through `vyapaar-ffi`.
2. **Consult the statutory cost model at fill time** so live and paper fills
   both carry an itemized `fee_breakdown` (cost engine lives in the
   `vyapaar-market` repo, `vyapaar-market-finance::india::costs`, per
   `vyapaar-core::FillReport` documentation); the paper path should reuse the
   same engine so parity shows real (not zero) fees.
3. **Normalize failures into `DispatchError`**: broker rejections, session
   expiry, and market closures must map to typed variants; broker wire
   payloads stay inside the adapter.
4. **Conformance**: golden-vector parity replay of paper vs. wire adapter over
   recorded broker sessions (`tests/golden/`), plus the existing
   `test_identical_fills_and_equity_paper_vs_zerodha_vs_angelone` extended to
   the live adapters.
