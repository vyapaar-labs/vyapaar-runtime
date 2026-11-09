# `vyapaar-runtime` — Job Scheduler Reference

Documents `crates/vyapaar-runtime-jobs/src/`: deterministic cron and background
task scheduler.

## Components

| Component | File | Purpose |
|-----------|------|---------|
| `JobScheduler` | `lib.rs` | Main scheduler, tick-driven cron evaluation |
| `JobSpec` | `lib.rs` | Job specification (schedule, task, budget) |
| `ScheduledJob` | `lib.rs` | Scheduled job with status tracking |
| `CronSchedule` | `lib.rs` | Cron expression parsing and evaluation |
| `JobPool` | `pool.rs` | Worker pool for concurrent job execution |
| `JobRegistry` | `registry.rs` | Registry of known job types |
| `Budget` | `budget.rs` | Per-job resource budgets |
| `Fencing` | `fencing.rs` | Distributed fencing for job execution |

## Scheduled Jobs

| Job | Schedule | Purpose |
|-----|----------|---------|
| TOTP auth refresh | 08:45 IST daily | Pre-market broker session renewal |
| EOD bhavcopy sync | End of day | Download and import NSE bhavcopy data |
| T+1 settlement sweep | T+1 | Demat settlement processing |
| State snapshotter | Configurable | Periodic state snapshots for recovery |

## Determinism

- Tick-driven cron evaluation (no wall-clock dependency)
- Deterministic job ordering within a tick
- Explicit `DateTime<Utc>` for all timestamps

## Related

- `vyapaar-runtime-api-rest` — REST endpoints for job management (`/jobs`)
- `vyapaar-exec-auth` — TOTP auth used by pre-market refresh job
