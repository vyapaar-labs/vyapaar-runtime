# `vyapaar-runtime-testing` & `vyapaar-runtime-test-utils` — Test Harness Reference

Crates `crates/vyapaar-runtime-testing/` (5 src files) and
`crates/vyapaar-runtime-test-utils/` (stub).

## 1. Purpose

`vyapaar-runtime-testing` is the deterministic test harness for the runtime's
actor pipeline, execution paths, and backtests: a mock `BrokerExecutor`, an
empty `MarketDataFeed`, simulation fill helpers, and fixed-time fixtures.
`vyapaar-runtime-test-utils` is an empty stub placeholder (`// Stub crate:
vyapaar-testing`) kept for workspace shape.

## 2. Module Map (`vyapaar-runtime-testing`)

| File | Contents |
|---|---|
| `lib.rs` | module re-exports (`fixtures::*`, `mock_broker::*`, `mocking::*`, `simulation::*`) |
| `mock_broker.rs` | `MockBrokerExecutor` |
| `mocking.rs` | `MockMarketFeed` |
| `simulation.rs` | `simulated_fill` |
| `fixtures.rs` | time/decimal/instrument/bar/settlement fixtures |

## 3. Public API

### `MockBrokerExecutor: BrokerExecutor`

- `new(should_succeed: bool)` — `true`: every order is accepted and an
  `OrderAck` (`MOCK-{uuid}`, `OrderStatus::New`) returned immediately;
  `false`: every `place_order`/`cancel_order` fails with
  `BrokerError::Rejected("mock broker configured to fail")`.
- `placed_orders() -> Vec<OrderRequest>` — recorded for assertions.
- `health_check()` always reports `BrokerHealth::session_ok("mock", "healthy")`;
  `get_order_status` always `OrderStatus::New`.
- Backed by `Arc<MockBrokerInner>` + `tokio::sync::Mutex` so clones share state
  across actor tasks.

### `MockMarketFeed: MarketDataFeed`

The empty feed: `subscribe` yields an empty stream, `fetch_snapshot` returns
`[]`, `feed_health` reports disconnected with no heartbeat. Used where a
strategy's event path must run with no market data driving it.

### `simulated_fill`

```rust
pub fn simulated_fill(
    instrument: &str, side: PositionSide,
    price: Decimal, quantity: Decimal,
    timestamp: DateTime<Utc>,
) -> Fill
```

Builds a `vyapaar_ledger_portfolio::Fill` directly (Delivery product,
`account_id = "default"`, trade id `SIM-{ts_nanos}`) — no broker in the loop.

### Fixtures (`fixtures.rs`)

- `utc_now()` is **frozen** at epoch `1_730_716_800` (2024-11-04 00:00:00 UTC) —
  nothing in the harness reads the wall clock.
- `utc_nanos(nanos: u64)` builds timestamps from epoch nanoseconds.
- `exact_decimal(value: f64) -> Decimal` converts a fixture literal by string
  round-trip (`to_string().parse()`), panicking on non-finite values — tests
  write `exact_decimal(100.5)` and get exactly `100.5`, no float drift.
- `currency_inr()`, `round_lot()` (15 shares), `round_price()` (₹100.00),
  `instrument(symbol)` (NSE), `any_instrument()` (`NSE:X`), `flat_bar`
  (OHLC all = close, volume 1), `settlement_instruction()`, `clearing_calendar()`.

## 4. Data Flow

Tests construct domain events (`Fill`, `OrderRequest`, `Bar`) from fixtures and
feed them to actor pipelines or backtests with `MockBrokerExecutor` /
`MockMarketFeed` in the `Arc<dyn Trait>` seams; the mocks record, never decide.

## 5. Dependencies

`vyapaar-core`, `vyapaar-ledger-portfolio`, `vyapaar-exec-core`
(`BrokerExecutor` port), `vyapaar-market-core` (`MarketDataFeed`, `Tick`),
`tokio`, `async-trait`, `rust_decimal`, `rust_decimal_macros`, `chrono`,
`uuid`, `futures`.

## 6. Tests

The crate is the test support library; it has no own `tests/` directory. Its
consumers include `vyapaar-runtime-actors` (`actor_runtime_tests.rs`,
`e2e_integration_stress.rs`, `observability_audit_tests.rs`) and other runtime
crates' test suites.

## 7. Invariants & Notables

- **Deterministic time**: the fixture clock never moves; any test needing later
  times uses `utc_nanos`.
- **No wall-clock, no network, no I/O**: mocks are in-memory only.
- **Zero floats in assertions**: `exact_decimal` is the only sanctioned f64
  entry point, and it converts exactly via string parsing.
- The sibling placeholder crates `vyapaar-runtime-market` and
  `vyapaar-indicators` are also stubs (`// Stub crate: ...`); real market-data
  and indicator code lives in the `vyapaar-market` repo. See `code-feed.md`
  for the placeholder table.

## Related

- `code-actors.md` — the pipelines these mocks drive
- `code-backtest.md` — the engine `simulated_fill` feeds
- `vyapaar-exec-core` — the `BrokerExecutor` port `MockBrokerExecutor` implements
