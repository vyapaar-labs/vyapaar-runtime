# `vyapaar-runtime-strategy` — Strategy Runtime Reference

Crate `crates/vyapaar-runtime-strategy/` — lib name `vyapaar_runtime_strategy`.
Description: *"Strategy trait, runner state machine, and manifest contracts for Vyapaar"*.
Crate lints: `#![deny(missing_docs)]`, `#![deny(rustdoc::broken_intra_doc_links)]`,
`#![deny(unsafe_code)]`.

## 1. Purpose

The strategy-authoring contract (ADR 008), mirroring the Python `vyapaar.strategies` package:

- A [`Strategy`] reacts to typed events (`Bar`, `Tick::Quote`, `Tick::Ltp`, `FillReport`) and
  acts **only** through its [`StrategyContext`] — reading the clock, positions, cash, instrument
  metadata — and emits [`OrderIntent`]s. It never touches the execution engine, I/O, or the wall
  clock, so the same strategy runs unchanged in backtest, paper, and live.
- [`StrategyRunner`] pairs a strategy + [`LedgerContext`] with an optional
  `vyapaar_risk::RiskEngine` pre-trade gate, converts intents into `vyapaar_core::OrderRequest`s
  with deterministic ids, and hands them to the host (actor runtime, backtester, anything
  implementing `vyapaar_core::ports::ExecutionGateway`). This crate owns no execution gateway
  and stays zero-I/O.
- [`StrategyManifest`] / [`StrategyIr`] / `step.toml` (`StepManifest`) are the data-only,
  hashable run contracts: a strategy is described *once* by a verify step and everything
  downstream (backtest, sweep, live, MCP, frontend) consumes the manifest rather than the source.
- [`dispatch`] is the strategy → multi-broker boundary: the `BrokerAdapter` trait, the
  `Dispatcher` coordinator, and the paper / Zerodha / Angel One adapters.

## 2. Module Map

| File | Contents |
|---|---|
| `lib.rs` | Module declarations + the crate's full `pub use` surface |
| `strategy.rs` | `Strategy` trait (8 methods), `StrategyAdapter<S>` |
| `context.rs` | `StrategyContext` trait, `InstrumentInfo`, `LedgerContext` (the one reference impl) |
| `intent.rs` | `OrderIntent`, `IntentError`, serde wire form `OrderIntentRepr` |
| `runner.rs` | `StrategyRunner<S>`, `SubmittedIntent`, `SuppressedIntent`, `IntentRejection`, `OrderRejection` |
| `dispatch.rs` | `BrokerAdapter`, `DispatchError`, `Dispatcher`, `AdapterState`, `PaperAdapter`, `ZerodhaKiteAdapter`, `AngelOneSmartApiAdapter` (+ aliases `ZerodhaAdapter`/`AngelOneAdapter`), `map_broker_error` |
| `manifest.rs` | `StrategyManifest`, `ParameterSpec`, `Universe`, `Subscriptions`, `TimeframeSpec`, `BarAggregation`, `ManifestError`, `STRATEGY_API_VERSION`, `param_range_to_schema`, `sweep_space_from_manifest`, `parameter_grid_from_manifest` |
| `step.rs` | `step.toml` grammar: `StepManifest`, `StepKind`, `StepParam`, `HistoryBars`, `StepRequires`, `StepConsumes`, `StepEmits`, `StepChain`, `StepPipeline`, `StepShapeOrList` |
| `ir.rs` | `StrategyIr`, `IrUniverse`, `IrSubscriptions`, `IrError` (+ stable `code()`), `StrategyIr::compile` |
| `factory.rs` | `StrategyFactory` trait, `StrategyFactoryRegistry`, `BuiltinStrategyFactory`, per-strategy factories, `manifest_to_parameter_grid`, `filter_sweepable_params` |
| `dyn_strategy.rs` | `DynStrategy(Box<dyn Strategy>)` — full-delegation type erasure for sweeps |
| `contract_probe.rs` | `ContractProbe` reference strategy + `Observation`; the conformance-fixture main strategy |
| `sma_crossover.rs` | `SmaCrossover` (+ `MAX_SMA_PERIOD = 10_000`) |
| `rsi_reversal.rs` | `RsiReversal` (internal `Side::{Flat,Long,Short}`) |
| `buy_and_hold.rs` | `BuyAndHold` |
| `error.rs` | `StrategyError::{Intent, Other}`, `Result<T>` |
| `order/`, `pnl/`, `position/`, `risk/` | Empty placeholder `mod.rs` stubs |

Removed history: the `CacheContext` backed by `vyapaar_engine::StateCache` and the runner's old
`ExecutionEngine`/`Handler` base were deleted in the `vyapaar-core`/`vyapaar-risk` migration;
`vyapaar-engine` is fully out of the dependency graph.

## 3. `Strategy` Trait

`pub trait Strategy: Send + 'static` — the hook set matches Python
`vyapaar.strategies.base.Strategy`; all hooks except `name` default to no-ops:

| Method | Semantics |
|---|---|
| `name() -> &str` | Stable name; order ids are `"{name}-{n}"` |
| `on_start(&mut self, ctx) -> Result<()>` | Once before the first event; intents submitted here are processed **with the first event** (same timestamp, same risk check) |
| `on_bar(&mut self, ctx, bar: &Bar)` | Every `MarketEvent::Bar` |
| `on_quote(&mut self, ctx, quote: &Tick)` | Every `Tick::Quote` |
| `on_trade(&mut self, ctx, trade: &Tick)` | Every `Tick::Ltp` (a market print; own executions arrive in `on_fill`) |
| `on_fill(&mut self, ctx, fill: &FillReport)` | Each fill, **after** it is booked in the context; intents queued here are processed with the *next* event |
| `on_intent_rejected(&mut self, ctx, intent, error: &IntentError)` | Runner rejected an intent violating invariants; no order was submitted, it no longer counts as busy |
| `on_stop(&mut self, ctx)` | Once after the last event; intents submitted here are **never executed** |

`StrategyAdapter<S>` owns `(S, LedgerContext)` and routes `MarketEvent`s: sets
`ctx.set_now(ts_init)`, then `Bar → on_bar`, `Tick::Quote → on_quote`, `Tick::Ltp → on_trade`.
`Tick::Depth` and `MarketEvent::ConnectorStateChanged` reach **no hook**.

## 4. `StrategyContext` & `LedgerContext`

`pub trait StrategyContext` (every hook receives `&mut dyn StrategyContext`):

| Method | Meaning |
|---|---|
| `now() -> DateTime<Utc>` | `ts_init` of the event being processed; `DateTime::MIN_UTC` before the first event |
| `position(&InstrumentId) -> Decimal` | Net signed qty (long + / short −), updated from fills |
| `positions() -> Vec<(InstrumentId, Decimal)>` | Every non-flat position, BTreeMap order |
| `cash() -> Money` | Initial cash + net cash flow of all fills |
| `busy(&InstrumentId) -> bool` | True while an intent for the instrument is unfilled/unrejected |
| `instrument(&InstrumentId) -> Option<&InstrumentInfo>` | Lot size + tick size metadata |
| `submit(OrderIntent)` | Queue an intent; the runner converts it after the hook returns |

`LedgerContext` is the deterministic in-memory ledger: `BTreeMap` positions/pending/instruments,
an outbox of intents, `MIN_UTC` clock, `Money::zero_inr()` default cash, `Currency::Inr`.
Runner/ops-facing extras: `with_cash`, `add_instrument`, `seed_position` (reduce-only runs),
`drain_intents`, `apply_fill` (all-or-nothing: refuses a fill whose `fee.currency` differs — a
failed booking leaves the ledger untouched), `release` (intent stops counting toward `busy`),
`release_remainder` (unfilled remainder from the execution port).

`InstrumentInfo { id, lot_size: Quantity, tick_size: Price }` — `Clone, Debug, PartialEq, Eq`,
deliberately no `Default`.

## 5. `OrderIntent`

```rust
pub struct OrderIntent {
    pub instrument_id: InstrumentId,
    pub side: OrderSide,
    pub quantity: Quantity,
    pub order_type: OrderType,   // prices live INSIDE OrderType
    pub time_in_force: TimeInForce,
}
```

- Price/type coherence is enforced by the type system via `vyapaar_core::OrderType`:
  `Market`, `Limit(Price)`, `StopLoss(Price)` (trigger), `StopLimit { trigger, limit }`.
  The old runtime errors (`MissingPrice`, `UnexpectedPrice`, …) are no longer representable.
  **No trailing-stop variant exists** — strategies recompute stop triggers per event.
- The only runtime invariant is positive quantity: `IntentError::NonPositiveQuantity`.
- Constructors (all default `TimeInForce::Day`): `market_buy/_sell`, `limit_buy/_sell`,
  `stop_buy/_sell` (→ `StopLoss`), `stop_limit_buy/_sell`.
- Wire form `OrderIntentRepr` is `#[serde(deny_unknown_fields)]`; `TryFrom` runs `validate()`.
- Conversions to `vyapaar_core::OrderRequest`: `into_order(algo_id)`,
  `into_order_with_id(client_order_id, algo_id)` (deterministic-replay path),
  `to_order_request[_with_algo|_with_id]`.

## 6. `StrategyRunner<S>`

State: `StrategyAdapter<S>`, monotonic `order_seq`, `submitted`/`fills`/`rejections`/
`order_rejections` records, `warmup_bars`/`bars_seen`, `open_orders` (remaining qty per order
id), `released` totals, optional `RiskEngine`, `TradingState`, outbox.

Builders: `with_context(strategy, ctx)`, `with_risk(RiskEngine)`, `with_warmup_bars(u32)`.
Introspection: `context()`, `submitted()`, `suppressed()`, `fills()`, `rejections()`,
`order_rejections()`, `warming_up()`, `trading_state()`, `released_quantity()`,
`drain_orders()` (the host's submission queue), `into_parts()`.

### 6.1 Event path

`on_event(event, ts_init)`:

1. Driving-bar counting — `MarketEvent::Bar` with a new `ts_init` increments `bars_seen`
   (multi-instrument bars at one timestamp count once).
2. `adapter.on_event` — hook dispatch; hooks queue intents in `LedgerContext`.
3. `drain_and_submit(ts_init)` per intent:
   1. `validate()` fails → `ctx.release`, `strategy.on_intent_rejected`, record `IntentRejection`.
   2. `warming_up()` → `ctx.release`, record `SuppressedIntent` (suppressed, not dropped).
   3. `order_id = "{name}-{n}"`; `OrderRequest.algo_id` = the strategy name;
      `client_order_id = Uuid::new_v5(&Uuid::NAMESPACE_OID, order_id.as_bytes())`;
      `correlation_id = Some(client_order_id)` — deterministic UUID v5 (ADR 0017 decision 6).
   4. Optional risk gate: `risk.check_order(&request, ts_init)` refusal → release + record
      `OrderRejection` (refusal text). Deliberately **not** routed to `on_intent_rejected`
      (different error shape).
   5. Insert `OpenOrder`, push to `outbox` + `SubmittedIntent`.

`on_fill(fill)`: decrement the matching `OpenOrder.remaining` (remove at zero),
`risk.record_fill`, `ctx.apply_fill(fill)?` (all-or-nothing), `strategy.on_fill`.
`on_order_event(OrderEvent)`: `Rejected`/`Cancelled` release the remaining quantity and record
an `OrderRejection`; `Acked`/`Filled` are no-ops (fills arrive via `on_fill`).

### 6.2 Semantic notes

- `TradingState` is **recorded, not enforced** — halting behaviour is not implemented here.
- Warm-up gate: `warming_up() = warmup_bars > 0 && bars_seen <= warmup_bars`.
- `on_start` intents ride the first event; `on_stop` intents are drained and **discarded**.
- Identical runs replay byte-identical orders/journals; randomness survives only in
  `Dispatcher::dispatch_intent` (see §8).

## 7. Manifests & IR

### 7.1 `StrategyManifest` (serde `deny_unknown_fields`)

Fields: `api_version` (must equal `STRATEGY_API_VERSION = "1.0.0"`), `name`, `source_hash`
(byte-identical journals keyed to source), `universe: Universe` (`Named(String)` |
`Explicit(Vec<InstrumentId>)`), `subscriptions: Subscriptions { instruments, quotes, trades }`,
`driving_timeframe: TimeframeSpec { interval, aggregation }`
(`BarAggregation::{Second,Minute,Hour,Day,Week}`), `warmup_bars: u32`,
`schedules: BTreeMap<String, String>`, `parameters: BTreeMap<String, ParameterSpec>`.

`ParameterSpec { default, range ("min:max:step"), min/max/step, description }` validates
consistency and that a numeric default lies within bounds; `discrete_values()` enumerates the
sweep grid (integral when the default is `i64`/`u64` or min/max/step are whole). Free
functions: `parameter_grid_from_manifest` (Cartesian product), `sweep_space_from_manifest`,
`param_range_to_schema`. `StrategyManifest::from_step_toml_str` SHA-256-hashes the TOML bytes
as `source_hash`.

### 7.2 `step.toml` IR grammar (`StepManifest`)

Matches `vyapaar-strategies/schema/step.schema.json`:

- `StepKind` (snake_case): `Screen | Filter | Signal | Size | Risk | Execute`.
- `StepParam { type, default, optional, min/max/step, choices, pattern, description, sweepable }`.
- `StepRequires { bars, fundamentals, indicators, history_bars: Count(u32) | Param("$slow"), pit }`.
- `StepConsumes { shape: Single|Multiple, columns }`; `StepEmits { shape, adds, report, reportable }`.
- `StepPipeline { steps: Vec<String> }` — chain DSL `parse("screen.universe -> signal.sma -> size")`
  validates stage ordering by prefix rank: screen 1 < filter 2 < signal 3 < size 4 < risk 5 <
  execute 6 (suffixed ids inherit their stage rank; unknown prefixes skipped).
- `to_strategy_manifest_with_hash`: strategy name = the part of `id` after the first `.`;
  `warmup_bars` from `requires.history_bars`; conversion defaults `universe = Named("NSE_500")`,
  `driving_timeframe = 1 Day`, subscription `NSE:NIFTY50` (quotes/trades off).

### 7.3 `StrategyIr::compile(manifest)`

Validates the manifest, rejects an empty subscription list (`IrError::NoSubscriptions`), and for
`Universe::Explicit` rejects any subscribed instrument outside the universe
(`IrError::SubscriptionOutsideUniverse`). Output: `schema_version` (must equal
`vyapaar_core::SCHEMA_VERSION`), `strategy_api_version`, embedded manifest, resolved
`IrUniverse` (sorted + deduped), per-channel `IrSubscriptions { bars, quotes, trades }`,
`timeframes` (driving first), `warmup_bars`. `IrError::code()` exposes stable snake_case codes
(`empty_name`, `no_subscriptions`, `subscription_outside_universe`, …) for agents/CLI.

## 8. Factories & Dispatch

### 8.1 `StrategyFactoryRegistry`

`StrategyFactory` produces strategies from `(params, instrument, quantity)`; the registry
auto-registers the builtins `buy_and_hold`, `sma_crossover` (requires `fast`/`slow` as `u64`,
`fast < slow`), `rsi_reversal` (requires `period` > 0 and `oversold < overbought`), and returns
`DynStrategy` (full-delegation `Box<dyn Strategy>`) for sweeps. `filter_sweepable_params` keeps
only `sweepable == true` manifest params.

### 8.2 `BrokerAdapter` + `Dispatcher`

```rust
pub trait BrokerAdapter: Send + Sync {
    fn broker_name(&self) -> &'static str;
    fn submit_order(&self, order: &OrderRequest, now: DateTime<Utc>) -> Result<String, String>;
    fn cancel_order(&self, broker_order_id: &str, now: DateTime<Utc>) -> Result<(), String>;
}
```

Every op receives `now` explicitly (temporal determinism: the same timestamped sequence ⇒
byte-identical sim fills). `Dispatcher { risk_engine, broker, algo_id }`:

- `new` registers `"STRATEGY_DISPATCH"` in the risk engine's SEBI algo registry;
  `with_algo_id` registers a custom id.
- `dispatch_intent[_with_id]` → `to_order_request[_with_id](_with_algo)` →
  `dispatch_order` → `risk_engine.check_order` (<15 µs SLO) → `broker.submit_order(order, now)`.
- `cancel_order` → broker cancel → `risk_engine.record_cancellation` (OTR tracking).
- `DispatchError`: `Intent`, `RiskRefusal(Box<RiskRefusal>)`, `BrokerRejection`,
  `SessionExpired`, `MarketClosed`, `RateLimited`, `Network` — `map_broker_error` maps the
  `vyapaar-exec` wire taxonomy onto these messages.

### 8.3 Adapters

| Adapter | `broker_name()` | Behaviour |
|---|---|---|
| `PaperAdapter` | `"paper"` (prefix `PAPER`) | In-process sim over shared `AdapterState`; default cash ₹1,000,000. |
| `ZerodhaKiteAdapter` (`ZerodhaAdapter`) | `"zerodha"` (prefix `KITE`) | With credentials: dedicated tokio `Runtime` + `block_on` over the real `vyapaar-exec-zerodha` `ZerodhaExecutor` (Kite Connect `POST /orders/regular`, `DELETE /orders/regular/{id}`); errors mapped via `map_broker_error` (auth → session expired, rate-limit → rate limited, transport → network). Without credentials: deterministic sim scaffold with the same economics. The `block_on` seam must not run on a tokio worker thread (documented in code). |
| `AngelOneSmartApiAdapter` (`AngelOneAdapter`) | `"angelone"` (prefix `SMARTAPI`) | Simulation scaffold identical in economics to paper — pending a real `vyapaar-exec-angelone` transport (GAP-0003 note); credential accessors exist but are unused. |

`AdapterState::execute_order` fills at `limit_price()` else the recorded market price, updates
cash/positions, computes the statutory `FeeBreakdown` via
`vyapaar_market_finance::india::costs::NseCashEquitySchedule::delivery().compute_costs`
(brokerage, STT, exchange charges, GST, stamp duty, SEBI charges — the DP charge is folded into
`exchange_charges`), and emits a `FillReport` with `fill_id = "FILL-{seq:04}"` and
`order_id = "{prefix}-{client_order_id}"`. Cancels append to `cancelled_orders`; `equity()` =
cash + Σ qty·market price.

## 9. Reference Strategies

| Strategy | Behaviour |
|---|---|
| `BuyAndHold` | One `market_buy` on the first bar, then nothing. |
| `SmaCrossover` | `vyapaar_market_indicators::Sma` over `bar.close`; market buy on cross-up, sell on cross-down; panics unless `0 < fast < slow`. `MAX_SMA_PERIOD` is declared but unenforced. |
| `RsiReversal` | `Rsi` with internal `Side` state to avoid repeat signals; panics if `period == 0` or `oversold >= overbought`. |
| `ContractProbe` | Conformance-fixture strategy recording `Observation { hook, now, position, cash, busy, open_positions }` in every hook; exercises market/limit/stop/stop-limit intents in lot/tick quantities. |

## 10. Dependencies

| Dependency | Use |
|---|---|
| `vyapaar-core` | `Bar`, `Tick`, `MarketEvent`, `FillReport`, `OrderRequest`, `OrderEvent`, `OrderType`, `Price/Quantity/Money`, `FeeBreakdown`, `SCHEMA_VERSION` |
| `vyapaar-risk` | `RiskEngine`, `RiskRefusal` |
| `vyapaar-market-indicators` | `Sma`, `Rsi` for the built-ins |
| `vyapaar-market-finance` | statutory cost schedule |
| `vyapaar-exec-core` / `vyapaar-exec-zerodha` | broker wire transport |
| `tokio` | dedicated runtime for the sync→async Zerodha seam |
| `serde`/`serde_json`/`schemars` | manifest/intent/IR wire + JSON Schema |
| `toml`, `sha2`, `chrono`, `rust_decimal`, `thiserror` | step parsing, source hashes, event clock, exact math, errors |

## 11. Tests (`tests/`, 17 files — 8 active, 9 gated behind the empty `legacy_engine` feature)

Active:

| File | Coverage |
|---|---|
| `dispatch_tests.rs` | Risk-gate latency SLO; identical fills/equity across paper vs Zerodha vs Angel One with real non-zero fees; cancel + OTR recording; risk refusal protects the broker; invalid intent rejected before risk/broker; fill timestamp follows the dispatch clock; full replay byte-identical |
| `zerodha_wire_tests.rs` | Local HTTP mock: submit/cancel delegate to Kite transport; auth/rate-limit/network error mapping |
| `strategies.rs` | Buy-and-hold, SMA cross-up/down, bad periods, RSI zones, runner round-trip, intent field preservation |
| `warmup.rs` | Warm-up gate vectors + manifest error-code vectors |
| `step_manifest.rs` | Parses every catalog `step.toml` (kama_crossover, turtle_20, equal_weight, …) |
| `ir.rs` | Shared manifest vectors compile or fail with their stable code; compiled IR crosses JSON and configures the runner |
| `sweep_space.rs` | Grid generation; empty-parameter manifest → empty grid |
| `pipeline_tests.rs` | Chain DSL parse + stage ordering validation |

Gated-out (dead by default): `conformance.rs`, `runner_semantics.rs`, `runner_event_stream.rs`,
`runner_risk.rs`, `order_rejections.rs`, `order_rejection_edges.rs`, `intent_validation.rs`,
`e2e.rs`, `golden.rs` — they reference the removed legacy crates.

## 12. Invariants & Notables

- **Execution-free and I/O-free strategies**: the world is seen only via `StrategyContext`;
  time is always the explicit event `ts_init` — never the wall clock.
- **All-or-nothing fill booking**: foreign-currency fees are refused; a mid-drain conversion
  error releases all remaining drained intents so the context never stays busy for an unqueued
  order.
- **Deterministic identity**: `"{name}-{n}"` tags + UUID v5 client order ids; sim adapters mint
  `FILL-{seq:04}` fill ids with no UUIDs or clock queries.
- **Risk-gate placement**: optional in the runner (refusal = `OrderRejection`), mandatory in the
  `Dispatcher` (<15 µs SLO) with `record_cancellation` feeding OTR.
- **Float seams (documented)**: indicator math (`f64` closes, RSI thresholds) and the paper
  adapter's cost-schedule notional round-trip; all accounting stays `Decimal`.
- **Serialization strictness**: `StrategyManifest` and `OrderIntentRepr` reject unknown fields;
  `StrategyIr` ignores unknown fields but rejects a non-current `schema_version`.

## Related

- `code-backtest.md` — the engine that drives this runner
- `code-enhancer.md` — optimizer consumes the manifest parameter grid + factory registry
- `code-config.md` — run configs embed a `StrategyManifest`
- `vyapaar-risk` — the pre-trade gate
