# GAP-0001 — Jobs, Runner Lifecycle, and Strategy Dispatch Architecture

> **Status**: Approved / Target Architecture Analysis  
> **Component Scope**: `vyapaar-runtime-jobs`, `vyapaar-runtime-api`, `vyapaar-runtime-api-rest`, `vyapaar-runtime-strategy`, `vyapaar-cli`  
> **Cross-Reference Subsystems**:  
> - Jesse Research Runners (`jesse/services/runner_queue.py`, `runner_worker.py`, `models/RunnerJob.py`)  
> - Honba Server-Side Run Lifecycle & Journaling (`honba/docs/adr/0017-run-lifecycle-and-run-id.md`, `honba-ya/crates/honba-ya-api-rest/src/executor.rs`)  
> - Honba Multi-Broker Strategy Dispatch (`honba-examples/examples/production/06_multi_broker_dispatch.py`)  
> - Honba Async Trader Loop (`honba-nou/crates/honba-nou-trader-loop`)  

---

## 1. Executive Summary

Vyapaar currently possesses high-performance analytical building blocks:
1. **SEBI Pre-Trade Risk Engine** in `vyapaar-core` and `vyapaar-risk` (<15 µs p99 latency).
2. **Strategy & Backtest Event Kernel** in `vyapaar-runtime-backtest` and `vyapaar-runtime-api-rest::BacktestExecutor` driving full statutory tax accounting (STT, GST, SEBI turnover, Stamp Duty).
3. **Parameter Optimization & 2R Calibration Engine** in `vyapaar-ui-core::strategy` with Deflated Sharpe Ratio (DSR) auditing.
4. **Autonomous CLI Interface** in `vyapaar-cli` providing `vyapaar jobs add/list/run/tick/pause/resume/approvals`.

However, an architectural disconnect exists between these subsystems:
- `vyapaar-cli` manages jobs as static JSON entries in `~/.vyapaar/jobs.json`, with `run`, `tick`, and `logs` operating as decoupled stubs.
- `vyapaar-runtime-jobs` defines an in-memory cron scheduler (`JobScheduler`, `JobSpec`, `Job`), but lacks a persistence layer, execution lease fencing, and connection to the backtesting engine.
- Backtest and optimization runs executed via CLI or Agent tool calls lack an immutable, append-only disk journal (`events.ndjson`, `manifest.json`, `trades.json`).
- Parameter sweeps and multi-day simulations run synchronously on calling threads rather than being dispatched to isolated worker threads or distributed compute nodes.

This document identifies the technical gaps by benchmarking against the mature patterns in **Jesse** and **Honba**, establishing the blueprint for the unified runtime job and dispatch system.

---

## 2. Comparative Benchmark: Jesse vs. Honba vs. Vyapaar

```
┌──────────────────────────────────────────────────────────────────────────────────────────────────┐
│                                 ARCHITECTURAL BENCHMARK                                         │
├──────────────────────┬──────────────────────────┬─────────────────────────┬──────────────────────┤
│ Feature              │ Jesse Research Runners   │ Honba ADR 0017 & Loop   │ Vyapaar Current      │
├──────────────────────┼──────────────────────────┼─────────────────────────┼──────────────────────┤
│ Job Queuing          │ PostgreSQL SKIP LOCKED   │ Bounded in-memory pool  │ Flat JSON file       │
│                      │ with LISTEN/NOTIFY       │ with 429 backpressure   │ (~/.vyapaar/jobs)    │
├──────────────────────┼──────────────────────────┼─────────────────────────┼──────────────────────┤
│ Identity Scheme      │ UUID v4 (job & session)  │ Crockford Base32 ULID   │ 8-char hex string    │
│                      │                          │ (26 chars, monotonic)   │ (random u32)         │
├──────────────────────┼──────────────────────────┼─────────────────────────┼──────────────────────┤
│ State Machine        │ queued -> running ->     │ pending -> running ->   │ Active | Paused      │
│                      │ done | failed | cancelled│ completed|failed|cancel │ (untracked execution)│
├──────────────────────┼──────────────────────────┼─────────────────────────┼──────────────────────┤
│ Fencing & Liveness   │ Claim IDs + leases       │ Graceful shutdown sweep;│ None (process death  │
│                      │ (session_liveness)       │ startup crash detection │ leaves corrupted state)│
├──────────────────────┼──────────────────────────┼─────────────────────────┼──────────────────────┤
│ Code Isolation       │ SHA-256 code manifests   │ Compiled StrategyIr     │ Direct in-tree FFI / │
│                      │ & isolated temp folders  │ registered in catalog   │ Python script writes │
├──────────────────────┼──────────────────────────┼─────────────────────────┼──────────────────────┤
│ Audit & Journaling   │ Database rows + tarball  │ Append-only directory   │ Standard out /       │
│                      │ artifacts synced to hub  │ with events.ndjson      │ transient terminal   │
├──────────────────────┼──────────────────────────┼─────────────────────────┼──────────────────────┤
│ Strategy Dispatch    │ Process manager / Ray    │ OrderIntent -> Adapters │ In-process sync or   │
│                      │ workers per function     │ (Zerodha, Angel, Upstox)│ external python call │
└──────────────────────┴──────────────────────────┴─────────────────────────┴──────────────────────┘
```

---

## 3. Detailed Catalog of Technical Gaps

### Gap 1: Disconnected Job Scheduling & Execution Pipeline
- **Problem**: `vyapaar jobs add` stores specifications in `~/.vyapaar/jobs.json`. When `vyapaar jobs tick` or `vyapaar jobs run <id>` is called, it outputs informational mock strings rather than dispatching to `vyapaar-runtime-jobs::JobScheduler` or `BacktestExecutor`.
- **Impact**: Autonomous multi-day rebalancing, scheduled morning scanner runs, and quantitative report jobs cannot execute unattended.
- **Reference**:
  - Honba’s `honba_examples.ewr.schedule` parses calendar durations and schedules automated rebalancing intervals cleanly against trading days.
  - Jesse’s `runner_queue.py` automatically claims queued research jobs and executes them via dedicated runners.

### Gap 2: Absence of Structured Append-Only Execution Journals
- **Problem**: When a backtest or parameter tuning job finishes, only the final scalar metrics (PnL, Sharpe, Drawdown) are displayed or returned in memory. There is no durable record of bar-by-bar portfolio equity, individual order events, fills, or regulatory charges.
- **Impact**:
  - Impossible to perform post-trade forensic audits or recreate historical equity curves.
  - TUI / Web dashboards cannot stream live progress or query historical runs (`GET /backtests/{id}/journal`).
- **Reference**: Honba ADR 0017 defines an append-only filesystem layout:
  ```text
  ~/.vyapaar/journals/<run_id>/
  ├── manifest.json   (Strategy spec, parameters, git commit, dataset ID, seed, timestamps)
  ├── events.ndjson   (Bar ticks, indicator calculations, order intents, fills, cancellations)
  ├── trades.json     (Aggregated round-trip trades with STT, GST, and turnover costs)
  └── metrics.json    (Sharpe, Sortino, Calmar, Max Drawdown, Win Rate, DSR confidence)
  ```

### Gap 3: Missing Run Identity, Fencing Leases, and Crash Recovery
- **Problem**:
  - Run IDs in `vyapaar-cli` are 8-character ad-hoc hex numbers (`rand_u32()`). They are not time-sortable, monotonic, or universally unique.
  - If a background job or worker crashes midway through execution, the job remains forever in an undefined state.
  - Concurrent runners risk race conditions when claiming or completing jobs.
- **Impact**: Risk of state corruption, duplicated executions, or orphaned lock files.
- **Reference**:
  - Honba ADR 0017 specifies 26-character Crockford Base32 ULIDs generated from a 48-bit millisecond timestamp and 80 bits of monotonic randomness, with startup sweep marking orphaned runs as `failed` (`interrupted`).
  - Jesse’s `session_liveness` implements fencing tokens (`claim_id`) with periodic heartbeats; if a worker loses network connectivity or crashes, its lease lapses and another worker safely resumes.

### Gap 4: Synchronous Execution & Lack of Core Budgeting for Sweeps
- **Problem**: Parameter sweeps (e.g. 100-trial tuning in `StrategyBridge::tune_strategy`) run synchronously on the main thread, blocking the CLI or API server.
- **Impact**: Scaling parameter sweeps to 10,000 trials or across multiple symbols exhausts thread pools or freezes interactive UIs.
- **Reference**: Jesse’s `runner_worker.py` and `RunnerJob` allocate explicit `cores_budget` per machine, spawning worker processes bounded to assigned CPU affinities and running trials in parallel across cores without thread contention.

### Gap 5: Strategy Signal to Multi-Broker Order Dispatch Boundary
- **Problem**: Strategies in `vyapaar-ui-core` produce `TradeSignal` objects directly with static price targets. There is no decoupled broker dispatch pipeline that validates orders against the SEBI pre-trade risk engine before translating them to Indian broker protocols (Zerodha Kite Connect, Angel One SmartAPI).
- **Impact**: Strategy logic remains tied to backtesting data types and cannot be deployed in paper or live trading without rewriting the order loop.
- **Reference**:
  - Honba’s `06_multi_broker_dispatch.py` and `StrategyRunner` accept `OrderIntent` objects from strategies and route them through uniform broker adapters with verified paper/live parity.
  - `honba-nou-trader-loop` validates every emitted intent against `RiskEngine::check_decision` in under 15 microseconds before sending to wire adapters.

---

## 4. Architectural Target State

Closing these gaps requires uniting the existing platform crates into an integrated pipeline:

```
┌────────────────────────────────────────────────────────────────────────┐
│                        User & Autonomous Trigger                       │
│    • vyapaar ask "..." / Agent Chat Tool / vyapaar jobs tick / cron    │
└───────────────────────────────────┬────────────────────────────────────┘
                                    │
┌───────────────────────────────────▼────────────────────────────────────┐
│                  vyapaar-runtime-jobs (Queue & Scheduler)              │
│  • Persistent SQLite / File Job Store (~/.vyapaar/jobs.db)             │
│  • Bounded Worker Pool (max_concurrent, max_queued with backpressure)  │
│  • Monotonic Crockford Base32 RunId Generator                          │
│  • Heartbeat Leases & Crash Recovery Sweeper                           │
└───────────────────────────────────┬────────────────────────────────────┘
                                    │
┌───────────────────────────────────▼────────────────────────────────────┐
│              vyapaar-runtime-api-rest::BacktestExecutor                │
│  • Job Dispatcher: Backtest / Sweep / Scanner / Trader Loop            │
│  • Isolated Worker Execution (Multi-core budgeting)                    │
│  • Pure Append-Only JournalWriter (~/.vyapaar/journals/<run_id>/)      │
└───────────────────┬────────────────────────────────────┬───────────────┘
                    │                                    │
┌───────────────────▼──────────────┐   ┌─────────────────▼───────────────┐
│     Historical Engine & Sim      │   │    Live & Paper Trader Loop     │
│  • vyapaar-runtime-backtest      │   │  • vyapaar-runtime-strategy     │
│  • BarReader / TickReader        │   │  • OrderIntent Generator        │
│  • Zero-Lookahead Fill Engine    │   │  • SEBI Pre-Trade Risk Gate     │
│  • Statutory Tax Model (STT/GST) │   │  • Broker Adapters (Kite/Angel) │
└──────────────────────────────────┘   └─────────────────────────────────┘
```

---

## 5. Next Steps

Refer to `vyapaar-runtime/docs/plans/0001-distributed-jobs-and-strategy-dispatch-runtime.md` for the technical implementation specification, data structures, and phased rollout plan.
