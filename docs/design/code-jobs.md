# `vyapaar-runtime-jobs` — Job Scheduler Reference

Crate `crates/vyapaar-runtime-jobs/` (5 src files, ~1.7k lines).
Crate doc: *"Scheduled cron & background job runner"* — cron expressions, job definitions, and
built-in job implementations for pre-market TOTP refresh, EOD settlement, and snapshot jobs.
Includes a persistent file-backed registry, multi-dimensional budgets, heartbeat lease fencing,
and a bounded worker pool with HTTP-429 backpressure (ADR 0017 / Jesse pattern).

## 1. Purpose

The scheduling layer of the runtime: persistent job specs with 5-field Vixie cron schedules,
claim/heartbeat/release fencing to prevent double execution, per-job budgets across six
dimensions, an approvals queue for manual gates, and a bounded worker pool with panic
isolation. Run lifecycle (RunIds, journals, strategy dispatch) is driven *through* these jobs
by the API layer (`code-api.md`) and the `vyapaar-ui` CLI (`vyapaar jobs ...`).

## 2. Module Map

| File | Contents |
|---|---|
| `lib.rs` | Crate root: inline `job` module (traits), inline `scheduler` module (`CronSchedule`, `JobScheduler` trait, `SchedulerState`, errors), inline `jobs` module (built-ins), `JobError` |
| `budget.rs` | `JobBudgets`, `BudgetUsage`, `BudgetExceededError` |
| `fencing.rs` | `FencingLease`, `FencingError` — heartbeat lease fencing |
| `pool.rs` | `BoundedWorkerPool`, `WorkerPoolConfig`, `SweepContext`, `PoolError` |
| `registry.rs` | `JobSpec`, `JobStatus`, `MissedPolicy`, `ApprovalPolicy`, `RegistryError`, `FileJobRegistry` |

## 3. Job Traits & Scheduler Contract

```rust
#[async_trait] pub trait Job: Send + Sync + Debug {
    fn name(&self) -> &str;
    async fn execute(&self) -> Result<(), JobError>;
    fn spec(&self) -> &JobSpec;
}
#[async_trait] pub trait CancellableJob: Job { async fn cancel(&self); }
#[async_trait] pub trait ProgressJob: Job { async fn progress(&self) -> u8; }  // 0-100
```

`JobScheduler` is a **trait** (`register_job`/`unregister_job[_by_name]`/`get_job`/`list_jobs`/
`start`/`stop`/`state`) — this crate defines the contract only; there is **no concrete
tick-driven loop implementation** inside the workspace (the CLI half lives in `vyapaar-ui`,
`vyapaar-cli`). `SchedulerState { Stopped, Running, Paused }`.

## 4. `CronSchedule` (5-field Vixie)

- Grammar: `minute hour day-of-month month day-of-week`; accepted chars `[0-9*?-/,]` only —
  **letters (`L`/`W`) and month/weekday names are rejected at registration**
  (`CronScheduleError::{InvalidFieldCount, InvalidCharacter}`).
- Semantics: `*`/`?` equivalent; ranges `a-b`; steps `*/n`, `a-b/n`; comma lists; Vixie DOW
  numbering (0/7 = Sunday) translated to the `cron` crate's 1-based scheme; **OR (union)
  semantics when both DOM and DOW are restricted** (Vixie rule); fire times strictly after the
  reference time; never-firing ⇒ `None`.

## 5. `JobSpec` & `FileJobRegistry`

`JobSpec` fields: `id: Uuid`, `name`, `goal`, `schedule: CronSchedule`, `description`,
`metadata: HashMap<String,String>`, `enabled` (default true), `status: JobStatus`
(`Active` default | `Paused` | `PendingApproval` | `Completed` | `Failed`), `budgets:
JobBudgets`, `usage: BudgetUsage`, `missed_policy: MissedPolicy { Skip (default), CatchUp }`,
`approval_policy: ApprovalPolicy { Auto (default), Manual }`, `active_lease:
Option<FencingLease>`, `fencing_counter: u64`, `created_at`, `last_run_at`, `next_run_at`,
`read_only: bool`, `once: bool`.

`FileJobRegistry { path, jobs: HashMap<Uuid, JobSpec> }`:

- `load_or_create(path)` — creates parent dirs + an empty file when missing.
- `save()` — **atomic tmp-rename**: pretty JSON (sorted by `created_at`) →
  `path.with_extension("json.tmp")` → `sync_all()` → `fs::rename`. Default location used by the
  CLI layer: `~/.vyapaar/jobs.json`.
- `add_job` (rejects duplicate names), `get_job[_mut]`, `list_jobs`, `remove_job`,
  `pause_job`/`resume_job`, `disable`/`enable` on specs.
- `claim_job(id, worker_id, ttl, now)` — refuses paused/pending-approval jobs and valid
  existing leases; **increments `fencing_counter` (saturating)** and grants the lease; sets
  `last_run_at`.
- `heartbeat_lease` / `release_lease` — validate `(worker_id, fencing_token, expiry)`;
  `once && Completed` pins `Completed`.
- `record_usage` — merges usage; **budget exceeded OR `ApprovalPolicy::Manual` ⇒ status
  `PendingApproval`**.
- `sweep_expired_leases(now)` — crash recovery: clears expired leases and marks `Active` jobs
  `Failed`.
- `approvals_queue()` / `approve_job(id)` (`PendingApproval → Active` only).

## 6. Budgets (`JobBudgets` / `BudgetUsage`)

Six dimensions, each optional: `max_turns`, `max_inr` (Decimal), `max_usd` (Decimal),
`max_tokens`, `max_tool_calls`, `max_data_calls`. `BudgetUsage` records each dimension with an
explicit `now: DateTime<Utc>` and merges. `BudgetExceededError` variants name
current-vs-limit per dimension. Money budgets are `Decimal` — no floats in budget accounting.

## 7. Fencing (`FencingLease`)

`FencingLease { job_id, lease_id, worker_id, fencing_token: u64, granted_at, expires_at,
ttl_secs }`:

- `grant(job_id, worker_id, fencing_token, ttl, now)` — ttl ≥ 1 s.
- `heartbeat(now)` — rejects expired leases, then extends.
- `validate(worker_id, token, now)` — expiry + worker match + token match.
- Split-brain protection: stale workers holding an older fencing token fail validation with
  `TokenMismatch`; expired leases are swept and their jobs marked `Failed`.

## 8. Bounded Worker Pool (`BoundedWorkerPool`)

`WorkerPoolConfig` defaults: `max_concurrent = num_cpus`, `max_queued = cpus × 2`,
`max_sweep_cores = cpus / 2 (min 1)`.

- `dispatch(task)` — try-acquire a permit; else CAS-increment the queued counter
  (`QueueSaturated` when full ⇒ HTTP **429**, `is_backpressure()`), then blocking acquire and
  run on `tokio::task::spawn_blocking` with panic catch (permit always released) ⇒
  `ExecutionPanic`. `ShuttingDown` (semaphore closed) ⇒ HTTP **503**; `CoreAllocation` ⇒ 422;
  `ExecutionPanic` ⇒ 500 (`PoolError::status_code()`).
- `dispatch_sweep(requested_cores, task)` — builds a dedicated `rayon::ThreadPool` of the
  clamped core budget wrapped in `SweepContext` (Jesse-style compute sweeps), routed through
  `dispatch`.
- `shutdown()` closes the semaphore; queued waiters get `ShuttingDown`.

## 9. Built-in Jobs (inline `jobs` module)

| Job | Default schedule | Name | Body |
|---|---|---|---|
| `PreMarketTOTPJob` | `0 8 * * 1-5` (08:00 weekdays) | `pre_market_totp_refresh` | Simulated: `sleep` + log, always `Ok(())` |
| `EODSettlementJob` | `0 15 * * 1-5` (15:00 weekdays) | `eod_settlement` | Simulated |
| `SnapshotJob` | `0 0 * * *` (midnight daily) | `positions_snapshot` / `portfolio_snapshot` / `system_snapshot` (by `SnapshotType`) | Simulated |

All built-in `execute()` bodies are **simulated stubs** (`tokio::time::sleep` + log) — real
TOTP refresh integration lives in `vyapaar-exec-auth`; the schedules and registry lifecycle are
the production surface. Each job has a `with_schedule(...)` constructor variant.

## 10. Dependencies

`vyapaar-core` (declared); externals: `tokio` (full), `chrono`, `serde`/`serde_json`,
`thiserror`, `tracing`, `async-trait`, `sqlx 0.8` (only for the `JobError::Storage` variant —
no SQL storage in this crate), `uuid`, `rayon 1.9` (sweep pools), `num_cpus`, `rust_decimal`,
`cron 0.17` (fire-time evaluation). Dev: `futures`, `tempfile`, `rust_decimal_macros`.

## 11. Tests (`tests/`, 5 files)

| File | Coverage |
|---|---|
| `unit_tests.rs` | Cron valid/invalid expressions; `JobSpec` creation/modifiers/serialization; built-in job creation + execution; error Display |
| `jobs_tests.rs` | Cron validation; spec disable/enable |
| `scheduler_cron_tests.rs` | `next_execution`: daily, weekend skip, every-N-minutes, comma lists, range+step, DOM/DOW OR semantics, `?`, Vixie DOW, strictly-after, never-firing → None, letter rejection |
| `registry_tests.rs` | Rich spec attributes; budget validation; lease heartbeat/validation; registry persistence + claim lifecycle; budget-exceeded → PendingApproval; crash-recovery sweeper |
| `pool_tests.rs` | Dispatch success; concurrency limits + 429 backpressure; worker panic isolation + permit release; sweep core budgeting; shutdown |

## 12. Invariants & Notables

- **No SQL storage**: persistence is the atomic JSON file; `sqlx` exists solely for the error
  variant.
- **Explicit time everywhere**: registry/fencing/budget calls take `now: DateTime<Utc>`.
- **Approval queue is the human gate**; `read_only` and `once` flags pin job semantics.
- Pool errors map to HTTP statuses (429/503/500/422) for the API layer to surface verbatim.

## Related

- `code-api.md` — `RunId` / run manifests / journals consumed by job runs
- `code-api-rest.md` — `vyapaar serve` integration
- `vyapaar-exec-auth` — the real TOTP refresh capability the pre-market job stubs for
- `vyapaar-ui` (`vyapaar-cli`) — `vyapaar jobs add/list/run/tick/logs/pause/resume/approvals/...`
