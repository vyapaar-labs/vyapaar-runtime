# PLAN-0001 — Distributed Jobs Engine, Run Lifecycle, and Strategy Dispatch Runtime

> **Status**: Ready for Implementation  
> **Target Crates**: `vyapaar-runtime-jobs`, `vyapaar-runtime-api`, `vyapaar-runtime-api-rest`, `vyapaar-runtime-strategy`, `vyapaar-cli`  
> **Pre-requisites**: `vyapaar-runtime/docs/gaps/0001-jobs-and-strategy-dispatch-framework.md`  
> **Architectural Provenance**:  
> - Honba ADR 0017 (Server-Side Run Lifecycle, Monotonic ULID, Immutable Journaling)  
> - Jesse Research Runners (`RunnerJob`, Worker Process Isolation, Fencing Tokens, Core Budgeting)  
> - Honba Strategy Dispatch & Trader Loop (Decoupled `OrderIntent`, SEBI `<15µs` Pre-Trade Gate, Paper/Live Parity)  

---

## 1. Executive Summary & Objective

This document defines the production plan to upgrade Vyapaar's runtime execution pipeline from static, unjournaled CLI stubs to an industrial-grade, deterministic, and verifiable **Jobs and Strategy Dispatch Runtime**.

### Primary Objectives:
1. **Durable Autonomous Job Engine**: Equip `vyapaar-runtime-jobs` with persistent storage, robust cron scheduling, execution budget guards (turns, INR friction, tokens), and a human-in-the-loop approval gate.
2. **Honba ADR 0017 Run Lifecycle & Journaling**: Establish a standardized 5-state run machine (`pending -> running -> completed | failed | cancelled`), monotonic Crockford Base32 `RunId`s, and append-only filesystem journals (`manifest.json`, `events.ndjson`, `trades.json`, `metrics.json`).
3. **Jesse-Style Worker Isolation & Core Budgeting**: Implement bounded concurrent worker pools with CPU core budgeting and heartbeat lease fencing, allowing large-scale parameter sweeps (e.g. 10,000 trials) to run efficiently without blocking user interactions.
4. **Adapter-Agnostic Order Intent Dispatch**: Route strategy decisions as pure `OrderIntent`s through Vyapaar’s sub-15 microsecond SEBI pre-trade risk engine before dispatching to Indian broker adapters (Zerodha Kite, Angel One SmartAPI).
5. **Seamless CLI & Agent Tool Unification**: Connect `vyapaar jobs run/tick`, `vyapaar ask`, and agent tool calling directly into this runtime pipeline.

---

## 2. System Architecture & Component Interaction

```
┌─────────────────────────────────────────────────────────────────────────────────────────┐
│                                 ENTRYPOINT INVOCATIONS                                  │
│   • vyapaar jobs {add, run, tick, list, logs}   • vyapaar ask "..." / Agent Chat Tool   │
│   • REST API: POST /api/v1/backtests            • Systemd / Cron daemon tick            │
└───────────────────────────────────────────┬─────────────────────────────────────────────┘
                                            │
┌───────────────────────────────────────────▼─────────────────────────────────────────────┐
│                          vyapaar-runtime-jobs (Orchestration)                           │
│  ┌───────────────────────────────────────────────────────────────────────────────────┐  │
│  │ Persistent Job Registry (~/.vyapaar/jobs.db / jobs.json)                          │  │
│  │ • JobSpec: ID, Goal, CronSchedule, Budgets (INR, Turns, Tokens), MissedPolicy    │  │
│  │ • State Machine: Active | Paused | PendingApproval                                │  │
│  └────────────────────────────────────────┬──────────────────────────────────────────┘  │
│                                           │                                             │
│  ┌────────────────────────────────────────▼──────────────────────────────────────────┐  │
│  │ Dispatcher & Bounded Worker Pool                                                  │  │
│  │ • RunId Generator: 26-char monotonic Crockford Base32 ULID                        │  │
│  │ • Concurrency: max_concurrent workers, max_queued queue (HTTP 429 backpressure)   │  │
│  │ • Fencing Leases & Crash Recovery Sweeper (marks orphaned running jobs as failed) │  │
│  └────────────────────────────────────────┬──────────────────────────────────────────┘  │
└───────────────────────────────────────────┼─────────────────────────────────────────────┘
                                            │
┌───────────────────────────────────────────▼─────────────────────────────────────────────┐
│                      vyapaar-runtime-api-rest (Execution Layer)                         │
│  ┌───────────────────────────────────────────────────────────────────────────────────┐  │
│  │ RunExecutor Trait (Synchronous Kernel Host)                                       │  │
│  │ • Input: RunJob (RunId, Kind, StrategyIr, ResolvedRequest, Seed)                  │  │
│  │ • Output: RunOutcome (Backtest | Sweep | ScannerReport)                           │  │
│  │ • Hardware Budgeting: CPU affinity & core allocation per trial                    │  │
│  └────────────────────────────────────────┬──────────────────────────────────────────┘  │
│                                           │                                             │
│  ┌────────────────────────────────────────▼──────────────────────────────────────────┐  │
│  │ Immutable JournalWriter (~/.vyapaar/journals/<run_id>/)                           │  │
│  │ • manifest.json  • events.ndjson  • trades.json  • metrics.json                   │  │
│  └───────────────────────────────────────────────────────────────────────────────────┘  │
└───────────────────────────────────────────┬─────────────────────────────────────────────┘
                                            │
                     ┌──────────────────────┴──────────────────────┐
                     ▼                                             ▼
┌──────────────────────────────────────────┐  ┌───────────────────────────────────────────┐
│  Historical Engine: Backtest & Sweeps    │  │   Live & Paper Dispatch: TraderLoop       │
│  • vyapaar-runtime-backtest              │  │   • vyapaar-runtime-strategy              │
│  • BarReader / TickReader (L2 Depth Sim) │  │   • OrderIntent Generator                 │
│  • Next-Bar Open Fill Engine (No Peek)   │  │   • SEBI Pre-Trade Risk Gate (<15 µs)     │
│  • TaxCalculator (STT, GST, Stamp, Turn) │  │   • Broker Adapters (Kite, SmartAPI)      │
└──────────────────────────────────────────┘  └───────────────────────────────────────────┘
```

---

## 3. Data Structures and Rust Component Design

### 3.1 Run Identifier (`RunId`) & Input Sanitization
Adopt Honba ADR 0017's 26-character Crockford Base32 ULID:

```rust
// vyapaar-runtime-api/src/run_id.rs

use std::fmt;
use std::str::FromStr;

/// 26-character Crockford Base32 monotonic Run Identifier.
/// High 48 bits: Unix millisecond timestamp.
/// Low 80 bits: Monotonic counter / cryptographic entropy.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RunId([u8; 16]);

impl RunId {
    /// Validates and parses a string slice. Rejects paths with path traversal (`../`, `%2e`).
    pub fn parse(s: &str) -> Result<Self, RunIdError> {
        if s.len() != 26 {
            return Err(RunIdError::InvalidLength(s.len()));
        }
        // Strict Crockford Base32 check: [0-9A-HJKMNP-TV-Z] (excludes I, L, O, U)
        for c in s.chars() {
            if !matches!(c, '0'..='9' | 'A'..='H' | 'J'..='K' | 'M'..='N' | 'P'..='T' | 'V'..='Z') {
                return Err(RunIdError::InvalidCharacter(c));
            }
        }
        // Decode bytes...
        Ok(Self([0u8; 16]))
    }
}

impl fmt::Display for RunId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Formats as 26-character uppercase string
        write!(f, "01JBP8R3D5K87M4Q2N1W6E9TXV")
    }
}
```

### 3.2 The 5-State Run Machine & Journal Structure
Adopt Honba's state machine in `vyapaar-runtime-api::runs`:

```rust
// vyapaar-runtime-api/src/runs.rs

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use crate::run_id::RunId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunStatus {
    /// Accepted and queued in worker pool, not yet started.
    Pending,
    /// Currently executing on a worker thread.
    Running,
    /// Finished successfully with metrics and fills recorded.
    Completed,
    /// Terminated with error or crash.
    Failed,
    /// Terminated via user or system cancellation before completion.
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunManifest {
    pub run_id: RunId,
    pub kind: RunKind, // Backtest | Sweep | PaperTrade | LiveTrade
    pub strategy_id: String,
    pub symbol: String,
    pub parameters: serde_json::Value,
    pub capital: rust_decimal::Decimal,
    pub seed: u64,
    pub status: RunStatus,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub error: Option<String>,
}
```

### 3.3 Append-Only Journal Writer (`JournalWriter`)
Synchronous file writer managing the per-run directory layout:

```rust
// vyapaar-runtime-api-rest/src/journal.rs

use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use vyapaar_runtime_api::run_id::RunId;
use vyapaar_runtime_api::runs::RunManifest;

pub struct FileJournalWriter {
    dir: PathBuf,
    events_writer: BufWriter<File>,
}

impl FileJournalWriter {
    pub fn create(base_dir: &Path, manifest: &RunManifest) -> std::io::Result<Self> {
        let dir = base_dir.join(manifest.run_id.to_string());
        fs::create_dir_all(&dir)?;

        // Write initial manifest.json
        let manifest_path = dir.join("manifest.json");
        fs::write(&manifest_path, serde_json::to_string_pretty(manifest)?)?;

        // Open events.ndjson for streaming
        let events_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("events.ndjson"))?;

        Ok(Self {
            dir,
            events_writer: BufWriter::new(events_file),
        })
    }

    pub fn append_event(&mut self, event: &serde_json::Value) -> std::io::Result<()> {
        serde_json::to_writer(&mut self.events_writer, event)?;
        self.events_writer.write_all(b"\n")?;
        Ok(())
    }

    pub fn finalize(&mut self, manifest: &RunManifest, trades: &serde_json::Value, metrics: &serde_json::Value) -> std::io::Result<()> {
        self.events_writer.flush()?;
        fs::write(self.dir.join("manifest.json"), serde_json::to_string_pretty(manifest)?)?;
        fs::write(self.dir.join("trades.json"), serde_json::to_string_pretty(trades)?)?;
        fs::write(self.dir.join("metrics.json"), serde_json::to_string_pretty(metrics)?)?;
        Ok(())
    }
}
```

### 3.4 Bounded Worker Pool with Jesse-Style Core Allocation
Jesse’s core budgeting adapted to Rust using `tokio::sync::Semaphore` and rayon/threadpool affinity:

```rust
// vyapaar-runtime-jobs/src/pool.rs

use std::sync::Arc;
use tokio::sync::Semaphore;

pub struct WorkerPoolConfig {
    /// Maximum concurrent jobs allowed simultaneously (e.g. NumCPUs).
    pub max_concurrent: usize,
    /// Maximum queued jobs waiting before returning 429 Backpressure.
    pub max_queued: usize,
    /// Maximum cores assignable to a single optimization sweep.
    pub max_sweep_cores: usize,
}

pub struct BoundedWorkerPool {
    semaphore: Arc<Semaphore>,
    config: WorkerPoolConfig,
}

impl BoundedWorkerPool {
    pub fn new(config: WorkerPoolConfig) -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(config.max_concurrent)),
            config,
        }
    }

    pub async fn dispatch<F, T>(&self, task: F) -> Result<T, PoolError>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        let permit = self.semaphore.clone().acquire_owned().await
            .map_err(|_| PoolError::ShuttingDown)?;

        tokio::task::spawn_blocking(move || {
            let result = task();
            drop(permit);
            result
        })
        .await
        .map_err(|e| PoolError::ExecutionPanic(e.to_string()))
    }
}
```

### 3.5 Strategy Intent to Multi-Broker Dispatch Boundary
Adopt Honba's decoupled `OrderIntent` and SEBI risk gate:

```rust
// vyapaar-runtime-strategy/src/dispatch.rs

use rust_decimal::Decimal;
use vyapaar_core::order::OrderRequest;
use vyapaar_risk::{RiskEngine, RiskRefusal};

pub trait BrokerAdapter: Send + Sync {
    fn broker_name(&self) -> &'static str;
    fn submit_order(&self, order: &OrderRequest) -> Result<String, String>;
    fn cancel_order(&self, broker_order_id: &str) -> Result<(), String>;
}

pub struct Dispatcher {
    risk_engine: RiskEngine,
    broker: Box<dyn BrokerAdapter>,
}

impl Dispatcher {
    pub fn dispatch_intent(&mut self, intent: &OrderIntent, now: chrono::DateTime<chrono::Utc>) -> Result<String, DispatchError> {
        // 1. Convert intent to concrete OrderRequest
        let order = intent.to_order_request();

        // 2. Pass through SEBI pre-trade risk gate (<15us latency)
        self.risk_engine.check_order(&order, now)
            .map_err(DispatchError::RiskRefusal)?;

        // 3. Dispatch to paper simulator or wire adapter
        self.broker.submit_order(&order)
            .map_err(DispatchError::BrokerRejection)
    }
}
```

---

## 4. Integration with CLI & Autonomous Agent

### 4.1 CLI Command Wiring
Upgrade `vyapaar-cli/src/runner.rs` to replace static placeholders with runtime calls:

1. **`vyapaar jobs run <id>`**:
   - Fetches `JobSpec` from persistent store.
   - Assigns a new `RunId`.
   - Submits to `BoundedWorkerPool`.
   - Streams live progress to `~/.vyapaar/journals/<run_id>/events.ndjson`.
2. **`vyapaar jobs tick`**:
   - Scans scheduled jobs due for execution.
   - Validates multi-dimensional budgets:
     - `turns <= max_turns`
     - `accrued_inr_cost <= max_inr`
     - `token_count <= max_tokens`
   - If human confirmation is required (`ApprovalPolicy::Manual`), places job in `Approvals` queue rather than firing.
3. **`vyapaar jobs logs <id>`**:
   - Reads the latest run from `~/.vyapaar/journals/<id>/` and prints the structured execution trail.

### 4.2 Crash Recovery on Engine Boot
On startup of `vyapaar`, `vyapaar-cli`, or `vyapaar serve`:
```rust
pub fn recover_interrupted_runs(journals_dir: &Path) -> std::io::Result<usize> {
    let mut recovered = 0;
    for entry in std::fs::read_dir(journals_dir)? {
        let entry = entry?;
        let manifest_file = entry.path().join("manifest.json");
        if manifest_file.exists() {
            let mut manifest: RunManifest = serde_json::from_reader(File::open(&manifest_file)?)?;
            if manifest.status == RunStatus::Running || manifest.status == RunStatus::Pending {
                manifest.status = RunStatus::Failed;
                manifest.error = Some("Interrupted: process terminated unexpectedly".to_string());
                manifest.finished_at = Some(Utc::now());
                serde_json::to_writer_pretty(File::create(&manifest_file)?, &manifest)?;
                recovered += 1;
            }
        }
    }
    Ok(recovered)
}
```

---

## 5. Phased Implementation Plan

| Phase | Milestone | Deliverables | Verification Criteria |
|---|---|---|---|
| **Phase 1** | **RunId & Journal Foundation** | • Add `RunId` (Crockford Base32) to `vyapaar-runtime-api`.<br>• Implement `FileJournalWriter` in `vyapaar-runtime-api-rest`.<br>• Write crash recovery sweeper. | `cargo test -p vyapaar-runtime-api`<br>Assert path traversal rejection (`../`, `%2e`).<br>Assert orphaned run recovery on reboot. |
| **Phase 2** | **Bounded Worker Pool & Sweep Budgeting** | • Implement `BoundedWorkerPool` in `vyapaar-runtime-jobs`.<br>• Integrate Jesse-style core allocations for sweeps.<br>• Implement 429 queue backpressure. | `cargo test -p vyapaar-runtime-jobs`<br>Assert `max_concurrent` limits and 429 refusal on queue saturation. |
| **Phase 3** | **Strategy Dispatch & Broker Boundary** | • Decouple `OrderIntent` from broker wire adapters.<br>• Wire `<15 µs` SEBI `RiskEngine` gate before order dispatch.<br>• Add `PaperAdapter` and `ZerodhaAdapter` scaffolds. | Integration test verifying identical fills and equity between Paper and Live adapters. |
| **Phase 4** | **CLI & Agent Jobs Unification** | • Wire `vyapaar jobs run/tick/logs` to runtime pool.<br>• Connect agent tool calling to append run journals.<br>• Connect approval queue for budget threshold breaches. | End-to-end CLI roundtrip: `vyapaar jobs add ... -> vyapaar jobs tick -> journal verified`. |

---

## 6. Acceptance Invariants

1. **Zero Floats in Financial Ledgers**: All prices, margin requirements, friction fees, and PnL values use `rust_decimal::Decimal`.
2. **Determinism**: Given an identical dataset ID, strategy manifest, and seed, the resulting `events.ndjson` and `metrics.json` must be byte-identical.
3. **No Phantom State**: No run shall remain in `running` or `pending` across process restarts.
4. **Strict Isolation**: A crashed or terminated worker thread must release its semaphore permits and never contaminate adjacent runner memory.
