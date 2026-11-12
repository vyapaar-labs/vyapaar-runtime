# `vyapaar-runtime-actors` — Actor System Reference

Crate `crates/vyapaar-runtime-actors/` (13 src files, ~3.2k lines).
Crate doc: *"Asynchronous actor runtime and supervision tree for market data, strategies,
risk gate, and execution"*.

## 1. Purpose

The live/paper Tokio actor pipeline: `MarketData → Strategy → (PositionSizing) → Risk →
Execution`, plus a standalone `PortfolioActor` and a Python-isolated `PythonStrategyActor`.
Includes Prometheus metrics, `tracing` spans with W3C trace-context propagation, formal SLO
verify functions, and deterministic backpressure.

## 2. Module Map

| File | Contents |
|---|---|
| `lib.rs` | Crate docs + `pub mod` + re-exports |
| `messages.rs` | Message enums (`MarketDataMsg`, `StrategyMsg`, `PositionSizingMsg`, `RiskMsg`, `ExecutionMsg`, `PortfolioMsg`), `ActorStatus`, `RiskBreachEvent`, portfolio DTOs, `PortfolioError` |
| `market_data.rs` | `MarketDataActor` — tick/bar ingestion, subscription filtering, coalescing, backpressure, broadcast |
| `strategy.rs` | `Strategy` trait (actor-side async trait) + `StrategyActor<S>` |
| `sizing.rs` | `PositionSizingActor` + `SimplePositionSizer` (hexagonal `PositionSizer` port) |
| `risk.rs` | `RiskActor` — synchronous `vyapaar_risk::RiskEngine` wrapper, breach events, audit persistence |
| `execution.rs` | `ExecutionActor` — broker dispatch via `BrokerExecutor`, OMS idempotency, audit persistence |
| `portfolio.rs` | `PortfolioActor` — FIFO tax-lot accounting, statutory fee calc, ledger posting; `SymbolMapper` |
| `supervisor.rs` | `Supervisor` — supervision-tree construction, restart strategies, staged graceful shutdown |
| `metrics.rs` | `MetricsCollector` — 11 Prometheus metric families + Axum `/metrics` router |
| `tracing_spans.rs` | Lifecycle spans (`OrderGenerated`→`RiskEvaluated`→`BrokerSubmitted`→`BrokerAcked`→`Filled`/`Cancelled`) + W3C TraceContext helpers |
| `python.rs` | `PythonStrategyActor` + `PythonStrategyEngine` — dedicated OS-thread isolation, timeouts, CancellationToken, fail-safe halts (ADR 0032 Phase 8.4) |
| `slo.rs` | SLO constants + `SloViolation` + 4 verify functions |

## 3. Message Model

Every actor owns its state and communicates over typed bounded `tokio::sync::mpsc` channels;
request/response pairs carry a `oneshot::Sender` reply channel inside the message:

| Message | Variants (essentials) |
|---|---|
| `MarketDataMsg` | `IngestTick(Tick)`, `IngestBar(Bar)`, `Subscribe/Unsubscribe(InstrumentId)`, `Shutdown` |
| `StrategyMsg` | `OnTick(Tick)`, `OnBar(Bar)`, `OnFill(FillReport)`, `Shutdown` |
| `PositionSizingMsg` | `SizeOrder { request, now, reply_to }`, `UpdatePortfolio`, `SetSizingMethod`, `SetRiskPct(Decimal)`, `Shutdown` |
| `RiskMsg` | `EvaluateOrder { request, now, reply_to }`, `RecordCancellation { algo_id }`, `RecordFill(FillReport)`, `HaltAlgo/ResumeAlgo`, `HaltGlobal/ResumeGlobal`, `Shutdown` |
| `ExecutionMsg` | `ExecuteOrder { request, reply_to }`, `CancelOrder { order_id, algo_id, reply_to }`, `QueryStatus { order_id, reply_to }`, `RiskBreachEvent(RiskBreachEvent)`, `Shutdown` |
| `PortfolioMsg` | `ProcessFill { fill, reply_to }`, `GetSnapshot { reply_to }`, `GetTaxSummary { period, reply_to }`, `ApplyCorporateActions`, `SettleTrades`, `Shutdown` |

`ActorStatus { Starting, Running, Degraded, Draining, Stopped, Errored(String) }` — every
actor `run()` returns one. All prices/quantities/PnL are `Decimal`; f64 conversion happens only
at the Prometheus boundary.

## 4. `Supervisor` — Wiring the Tree

| Constructor | Tree |
|---|---|
| `spawn_with_strategy(strategy, risk_engine, broker, channel_capacity, restart_strategy)` | 4 actors: `MarketDataActor` → `StrategyActor` → `RiskActor` → `ExecutionActor` |
| `spawn_with_position_sizing(strategy, risk_engine, portfolio, broker, capacity, restart)` | 5 actors: sizing sits between strategy and risk (`StrategyActor::with_sizing_tx`) |
| `spawn_with_observability(strategy, risk_engine, broker, capacity, metrics, audit_ledger)` | 4 actors + `.with_metrics(...)` everywhere + `.with_audit_ledger(...)` on Risk/Execution (`OneForOne`) |

All `mpsc` channels use the caller-supplied `channel_capacity` as mailbox size. Accessors:
`market_data_sender()`, `strategy_sender()`, `sizing_sender() -> Option<_>`, `risk_sender()`,
`execution_sender()`, `resubscribe_ticks() -> broadcast::Receiver<Tick>`.

- **Graceful shutdown**: `shutdown_gracefully(timeout)` sends `Shutdown` down the chain
  (MarketData → Strategy → Sizing → Risk → Execution) and joins all handles under a timeout.
- **Restart**: `RestartStrategy::{OneForOne, AllForOne}` is tracked, handles can be swapped
  (`restart_strategy()`, `restart_count(...)`); there is **no automatic restart-on-failure
  monitor** — crash detection happens via `JoinHandle` errors at shutdown.

## 5. Actor Semantics

### `MarketDataActor`
Consumes `MarketDataMsg`; forwards ticks for subscribed instruments to every strategy sender
(`try_send`, warn+drop on full mailbox) and broadcasts `Tick` over `tokio::sync::broadcast`.
Builder chain: `with_backpressure_policy(BackpressurePolicy)`,
`with_max_buffer_size(usize)` (default **100**), `with_coalesce_interval_ms(50)`,
`with_metrics`. Event loop is `tokio::select! { biased }` between the receiver and a coalesce
flusher interval; status flows `Starting → Running → Stopped` (buffer flushed first on
shutdown).

`BackpressurePolicy { Passthrough, CoalesceLatest (default), DropNewest, DropOldest }`:
buffered modes keep only the latest tick per instrument on flush; full-buffer behaviour is
policy-driven (coalesce-then-push / discard newest / evict oldest) with per-drop metrics.

### `StrategyActor<S: Strategy>`
Consumes `StrategyMsg`; produces orders into `RiskMsg::EvaluateOrder` (or
`PositionSizingMsg::SizeOrder` when sizing is wired) and **awaits the oneshot reply**. Stamps
the request's trace context (`derive_or_create_trace_context`) and emits the `OrderGenerated`
span + metrics.

### `PositionSizingActor`
Default sizer `SimplePositionSizer` (delegates to pure
`vyapaar_core::sizing::calculate_position_size` via the `PositionSizer` port); defaults
`SizingMethod::FixedFractional { risk_pct: 0.01 }`. Entry price = limit price else ₹100.00
placeholder; zero-size results keep quantity 1; sizer errors forward the unadjusted order.
Portfolio updates arrive only via the explicit `UpdatePortfolio` message (no auto-fill sync).

### `RiskActor`
Synchronous `risk_engine.check_order(&request, now)` with timing; on refusal: warn,
`RiskBreachEvent` (`rule_code` = the refusal's rule), metrics, **audit row**
(`audit.record_risk_refusal`), `Err(BrokerError::RiskRefusal(...))` reply, and an
`ExecutionMsg::RiskBreachEvent` emission. On approval: forwards `ExecuteOrder` with a fresh
oneshot and relays the broker result. Also handles `RecordCancellation`/`RecordFill`
(OTR + audit) and `HaltAlgo`/`ResumeAlgo`/`HaltGlobal`/`ResumeGlobal`.

### `ExecutionActor`
`new(broker: Arc<dyn BrokerExecutor>, rx, risk_tx, strategy_tx)` with optional
`.with_oms(OrderStateMachine)` and `.with_audit_ledger`. `ExecuteOrder`: OMS idempotency check
on `client_order_id` (duplicate ⇒ `InvalidRequest`), `register_order` at `PendingNew`, then
timed `broker.place_order` — ack ⇒ `New` + audit + metrics; error ⇒ `Rejected`. `CancelOrder`
on success emits the `Cancelled` span (fresh correlation `Uuid::new_v4()`), an audit row, and
forwards `RiskMsg::RecordCancellation`. `RiskBreachEvent` is mirrored into
`vyapaar_runtime_persistence::RiskBreachEvent` for the audit ledger. Shutdown drains remaining
queued messages (`try_recv` loop) before stopping.

### `PortfolioActor` (standalone, not wired by `Supervisor`)
Consumes `PortfolioMsg`; owns a single default account. `ProcessFill`:
`vyapaar_ledger_portfolio::apply_fill_to_account` (FIFO tax-lot matching) + exact-`Decimal`
statutory fees (STT 0.1% delivery / 0.025% intraday sell-only; NSE turnover ₹2.97/crore; SEBI
₹10/crore; stamp duty 0.015% buy-side; GST 18%; DP ₹13.50+GST on delivery sell — all
`.floor()`; brokerage placeholder ₹0) + a `vyapaar_ledger_journal::Transaction` posting.
`GetSnapshot` computes MTM unrealized PnL from marks; `GetTaxSummary` delegates to
`TaxSummary::from_fills`; `SettleTrades` is a **stub** (zeroed `PortfolioSettleResult`).
`SymbolMapper` maps broker exchange tokens → canonical `InstrumentId`.

### `PythonStrategyActor` (ADR 0032 Phase 8.4)
`PythonStrategyActor::spawn(engine, rx, risk_tx, capacity)` runs the sync
`PythonStrategyEngine` on a **dedicated named OS thread** (`"honba-py-{algo_id}"`), bridged by
an mpsc task channel; every engine call is wrapped in `std::panic::catch_unwind`. Each
on-tick/bar gets a fresh `CancellationToken` and a `timeout(...)` wait (default **50 ms**); a
timeout or crash ⇒ status `Degraded`, an error metric, and a protective
`RiskMsg::HaltAlgo { algo_id, reason }`. Final status `Degraded` if degraded else `Stopped`.
`PythonExecutionError { Timeout, WorkerCrashed, GilContention, ChannelDisconnected,
StrategyError }`.

## 6. Observability

### Metrics (`MetricsCollector`, Prometheus)
- `actor_mailbox_queue_depth`, `actor_processing_latency_seconds`,
  `actor_backpressure_drops_total`, `actor_errors_total` (Phase-8 families) plus legacy-prefixed
  families: `honba_otr_ratio`, `honba_risk_evaluation_duration_seconds`,
  `honba_order_dispatch_duration_seconds`, `honba_notional_turnover_inr_total`,
  `honba_daily_pnl_inr`, `honba_risk_refusals_total`, `honba_order_operations_total`
  (historical `honba_*` prefix retained).
- Axum `/metrics` router exposed by the collector.

### Tracing (`tracing_spans.rs`)
Lifecycle spans as `tracing::info_span!` with W3C `traceparent` fields carried in
`TraceContext` (no `opentelemetry` crate dependency):
`actor.process_message`, `market_data.coalesce`, `risk.evaluate_order`,
`execution.dispatch_wire`, `OrderGenerated`, `RiskEvaluated`, `BrokerSubmitted`,
`BrokerAcked`, `Filled`, `Cancelled`. Helpers: `inject_trace_context` / `extract_trace_context`
/ `derive_or_create_trace_context`.

### SLOs (`slo.rs`)
| Constant | Bound |
|---|---|
| `SLO_TICK_TO_QUOTE_WS_BROADCAST_MAX` | p99 < 2 ms |
| `SLO_STRATEGY_SIGNAL_TO_ORDER_MAX` | p99 < 1 ms |
| `SLO_END_TO_END_TICK_TO_WIRE_MAX` | p99 < 5 ms |
| `SLO_PRE_TRADE_RISK_GATE_MAX` | p99 < 15 µs |

Verify functions `verify_ws_broadcast_slo`, `verify_strategy_signal_slo`,
`verify_wire_dispatch_slo`, `verify_risk_gate_slo` return `SloViolation` with
actual-vs-threshold.

## 7. Dependencies

`vyapaar-core`, `vyapaar-risk`, `vyapaar-ledger` (+ `-portfolio`/`-oms`/`-journal`),
`vyapaar-runtime-persistence` (`SqliteAuditLedger` + the `RiskBreachEvent` mirror),
`vyapaar-exec-core` (`BrokerExecutor`/`BrokerError`); `tokio` (full), `tokio-util 0.7`
(CancellationToken), `async-trait`, `futures`, `chrono`, `rust_decimal`, `thiserror`,
`tracing`, `serde`/`serde_json`, `uuid`, `prometheus 0.13`, `axum 0.7`. Dev:
`tracing-subscriber`, `vyapaar-runtime-testing`.

## 8. Tests (`tests/`, 5 files)

| File | Coverage |
|---|---|
| `unit_tests.rs` | Metric records + Prometheus text export contains the `actor_*` families |
| `actor_runtime_tests.rs` | End-to-end 4-actor pipeline (subscribe→tick→order→risk→mock broker→shutdown); risk refusal on empty algo id + global kill switch; `DropNewest` backpressure; restart-strategy counts; execution drain loop |
| `e2e_integration_stress.rs` | Multi-thread full-pipeline integration; **50k ticks/sec zero-data-loss stress** |
| `observability_audit_tests.rs` | Tracing + Prometheus export; `spawn_with_observability` with in-memory audit assertions |
| `phase8_tests.rs` | All 4 SLO verify functions; W3C trace-context propagation; sizing chaining; Python actor OS-thread isolation, timeout → Degraded + halt |

## 9. Invariants & Notables

- **Zero-I/O actors**: pure except explicitly injected ports (broker executor, audit ledger).
- **Explicit timestamps** on `EvaluateOrder`/`SizeOrder`; wall-clock uses are confined to
  audit rows, lifecycle logging, and metric instrumentation.
- **No automatic supervisor restart monitor** — `RestartStrategy` is tracked and handles are
  swappable; recovery is caller-driven.
- **`PortfolioActor` is standalone** (not part of the `Supervisor` trees) and `SettleTrades`
  is a stub.
- Metric names retain the legacy `honba_*` prefix alongside the `actor_*` families.

## Related

- `code-persistence.md` — `SqliteAuditLedger` used by Risk/Execution audit
- `code-api-rest.md` — the HTTP/WS surface that feeds the actor system
- `vyapaar-risk` — `RiskActor` delegates to `RiskEngine`
- `vyapaar-exec` — `ExecutionActor` dispatches to `BrokerExecutor`
- `vyapaar-ledger` — `PortfolioActor` uses portfolio/OMS/journal crates
