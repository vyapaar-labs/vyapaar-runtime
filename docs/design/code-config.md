# `vyapaar-runtime-config` — Run Configuration Reference

Crate `crates/vyapaar-runtime-config/` (1 src file, 3 test files).

## 1. Purpose

The single declared type a caller submits to run a backtest. Config is the input
to every surface — a backtest, a sweep, a live session, a CI job — so it is one
schema the code generator, the REST layer, and the CLI all walk, instead of a
TOML parsed ad hoc in four places. It lives in its own crate (not `vyapaar-cli`)
so codegen and REST depend on it without the CLI.

## 2. Public API

### `BacktestRunConfig` (serde `deny_unknown_fields`)

```rust
pub struct BacktestRunConfig {
    pub strategy: StrategyManifest,   // the verified strategy — by manifest, not source
    pub data: DataSourceConfig,       // default: Local { root: "data/catalog" }
    pub execution: ExecutionConfig,   // default fill_model = BarFill
    pub account: AccountConfig,       // default starting_cash = 1_000_000.0, currency = "INR"
    pub seed: u64,                    // required; same seed + data ⇒ identical journal
    pub risk: RiskLimits,             // re-exported from vyapaar_risk (ADR 0018); absent = no limits
}
```

- `DataSourceConfig` — `Local { root }` (Parquet catalog read via `vyapaar-feed`)
  or `Adapter { name }` (broker adapter resolved at run time, e.g. `dhan`).
- `ExecutionConfig { fill_model, cost_model_version = "latest", slippage_multiplier = 1.0, apply_market_calendar = true }`.
  `FillModel` (non-exhaustive, snake_case on the wire): `BarFill` (default: fill at
  bar open / next open / fraction of range), `BarFillThrough` (only when the bar
  trades through the order price), `L2` (top-of-book matching vs quote/trade ticks,
  ADR 0030 — a bars-only data source is refused with `l2_requires_ticks`), `None`
  (no fills; tests the signal path alone).
  Note the deliberate asymmetry: `apply_market_calendar` has `#[serde(default)]`
  (a missing field ⇒ `false`) while the struct's `Default` impl sets `true` —
  explicit TOML and programmatic defaults differ.
- `AccountConfig { starting_cash: f64, currency = "INR" }`.
- `[risk]` is the `vyapaar_risk::RiskLimits` / `OrderRateLimit` pair re-exported
  here so run configs, the CLI, and the REST server share one definition.

### `validate()` — checked at the door

A run that starts inconsistent produces a journal nobody can interpret, so
validation is front-loaded into `RunConfigError` variants:

| Variant | Condition |
|---|---|
| `Strategy(ManifestError)` | embedded manifest invalid |
| `ZeroSeed` | `seed == 0` (every run identical, defeats reproducibility) |
| `NegativeSlippage` | `slippage_multiplier < 0` would reward worse fills |
| `InvalidStartingCash` | not finite or not `> 0` |
| `EmptyDataRoot` | blank local data root |
| `Risk(InvalidRiskLimits)` | `[risk]` structurally invalid (max notional, rate limit, price collar, freeze limits) |

### `ResolvedUniverse`

A run config cannot hold a bare `Universe::Named` — a run needs to know which
instruments to load. `ResolvedUniverse { name, instruments }` sorts + dedups on
construction (determinism) and `apply(&mut StrategyManifest)` overwrites both
`manifest.universe = Explicit(...)` and `subscriptions.instruments` so the
manifest and the data loader agree on the instrument set.

## 3. Data Flow

TOML file → `BacktestRunConfig` (deny unknown fields) → `validate()` →
named universe resolved → `ResolvedUniverse::apply` → backtest/sweep/live surface
consumes it; the same bytes are echoed into the run's assumptions block.

## 4. Config Itself & Real Examples

`configs/` in the repo root:

| File | Notes |
|---|---|
| `configs/backtest/alpha30_factor.toml` | seed 30, ₹5,000,000, `bar_fill`, monthly rebalance, 10 NIFTY names, `warmup_bars = 252` |
| `configs/backtest/banknifty_options.toml` | seed 7, `bar_fill_through`, `slippage_multiplier = 1.5` |
| `configs/backtest/nifty50_momentum.toml` | seed 42 |
| `configs/live/{dhan,zerodha}_{live,paper}.toml` | broker sessions with `${...}` env placeholders for secrets |
| `configs/ai/{research_loop,rl_training}.toml` | agent-loop settings |

The historical header comment in `alpha30_factor.toml` still says
`honba_config::BacktestRunConfig` and `crates/honba-config/tests/configs.rs` —
a rename-era leftover; the type is `vyapaar_runtime_config::BacktestRunConfig`.

## 5. Dependencies

`schemars` (JsonSchema for codegen), `serde`, `vyapaar-core` (`InstrumentId`),
`vyapaar-risk` (`RiskLimits`, `OrderRateLimit`), `vyapaar-runtime-strategy`
(`StrategyManifest`, `Universe`).

## 6. Tests (`tests/`, 3 files)

| File | Coverage |
|---|---|
| `configs.rs` | the shipped example configs parse and validate |
| `risk_toml.rs` | `[risk]` section round-trips into `RiskLimits` and rejects bad values |
| `unit_tests.rs` | defaults, `ResolvedUniverse::apply`, error variants |

## 7. Invariants & Notables

- **No env overrides in config**: env interpolation (`${...}`) appears only in the
  live broker TOMLs consumed at a different layer; `BacktestRunConfig` itself is
  pure serde — reproducibility over convenience.
- `deny_unknown_fields` everywhere: a typo'd key is a hard error, not a silent default.
- `seed` is mandatory and non-zero; it feeds every stochastic component.
- `FillModel` is `#[non_exhaustive]`: adding a model is semver-compatible for
  producers, breaking for exhaustive matchers — intended, so results can state
  exactly how they were filled.
- Zero-floats caveat: `starting_cash` and `slippage_multiplier` are `f64` by
  schema decision (schemars-friendly config input); the engine converts to
  `Decimal` on entry.

## Related

- `code-strategy.md` — the `StrategyManifest` embedded in `strategy`
- `code-backtest.md` — the engine that consumes this config
- `code-feed.md` — what `DataSourceConfig::Local` reads
- `../architecture.md` — where config sits in the 14-crate map
