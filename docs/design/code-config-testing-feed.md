# `vyapaar-runtime` — Config, Testing, Feed & Market Reference

Documents the remaining runtime sub-crates: config, testing, feed, market, and indicators.

## `vyapaar-runtime-config`

Typed TOML/env configuration profiles:
- `LiveConfig` — Production trading configuration
- `PaperConfig` — Paper trading configuration  
- `BacktestConfig` — Historical backtest configuration

## `vyapaar-runtime-testing`

Deterministic test harness:
- `MockBroker` — Simulated broker for testing
- `ScriptedGateway` — Scripted execution gateway
- Synthetic feeds and contract assertions

## `vyapaar-runtime-feed`

Market data feed adapter crate — normalizes broker tick formats into canonical
`vyapaar_core::Tick` and `vyapaar_core::Bar`.

## `vyapaar-runtime-market`

Market data normalization crate — tick coalescing, L2/L3 depth maintenance.

## `vyapaar-indicators`

Technical indicators crate — streaming calculation of moving averages,
oscillators, volatility bands, and volume profiles with zero heap allocations
in hot paths. WASM-portable.

## `vyapaar-runtime-test-utils`

Shared test utilities crate used across runtime sub-crate test suites.

## Related

- `vyapaar-market` — Rust market data subsystem (broker feeds, indicators)
- `vyapaar-core` — Canonical `Tick`, `Bar`, `PriceBand` types
- `vyapaar-risk` — Pre-trade risk gate for validated market data
