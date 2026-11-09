# `vyapaar-runtime` — Strategy Runtime Reference

Documents `crates/vyapaar-runtime-strategy/src/`: the `Strategy` trait, context,
order intents, manifests, and zero-I/O strategy runner.

## Modules

| Module | Purpose |
|--------|---------|
| `strategy.rs` | `Strategy` trait, `StrategyContext`, `OrderIntent` |
| `manifest.rs` | `StrategyManifest`, manifest loading |
| `step.rs` | `StepPipeline` (`step.toml` IR) |
| `context.rs` | Strategy execution context |
| `intent.rs` | Order intent types |
| `runner.rs` | Zero-I/O `StrategyRunner` |
| `factory.rs` | Strategy factory |
| `ir.rs` | Intermediate representation |
| `contract_probe.rs` | Strategy contract validation |
| `buy_and_hold.rs` | Reference strategy: buy and hold |
| `sma_crossover.rs` | Reference strategy: SMA crossover |
| `rsi_reversal.rs` | Reference strategy: RSI mean reversion |

## `Strategy` Trait

The core trait all strategies implement. Strategies are pure, zero-I/O, and
deterministic — the same instance runs in backtest and live.

## Zero-I/O & Temporal Determinism

- Simulation clock stepped deterministically
- All timestamps injected explicitly
- No wall-clock queries in strategy logic

## Related

- `vyapaar-runtime-backtest` — Backtest engine executes strategies
- `vyapaar-runtime-enhancer` — Optimizer tunes strategy parameters
- `vyapaar-risk` — Pre-trade risk gate validates strategy orders
