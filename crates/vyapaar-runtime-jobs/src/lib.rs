//! vyapaar-runtime-jobs: Scheduled cron & background job runner.
//!
//! Provides a job scheduling framework with cron expressions, job definitions,
//! and built-in job implementations for pre-market TOTP refresh, EOD settlement,
//! and snapshot jobs.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use thiserror::Error;
use tracing::{debug, info};
use uuid::Uuid;

pub use job::Job;
pub use scheduler::{CronSchedule, JobScheduler, JobSpec};

/// Job execution module
pub mod job {
    use super::*;

    /// Trait for scheduled background jobs.
    ///
    /// Implement this trait to create custom jobs that can be scheduled
    /// and executed by the `JobScheduler`.
    #[async_trait::async_trait]
    pub trait Job: Send + Sync + std::fmt::Debug {
        /// Returns the job name.
        fn name(&self) -> &str;

        /// Executes the job.
        async fn execute(&self) -> Result<(), JobError>;

        /// Returns the job specification.
        fn spec(&self) -> &JobSpec;
    }

    /// Trait for jobs that can be cancelled.
    #[async_trait::async_trait]
    pub trait CancellableJob: Job {
        /// Cancels the job execution.
        async fn cancel(&self);
    }

    /// Trait for jobs that provide progress updates.
    #[async_trait::async_trait]
    pub trait ProgressJob: Job {
        /// Returns the current progress as a percentage (0-100).
        async fn progress(&self) -> u8;
    }
}

/// Scheduler module
pub mod scheduler {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Arc;

    /// Cron schedule expression (5-field format: minute hour day month weekday).
    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
    pub struct CronSchedule(String);

    impl CronSchedule {
        /// Creates a new cron schedule from a string.
        pub fn new(expr: impl Into<String>) -> Result<Self, CronScheduleError> {
            let expr = expr.into();
            Self::validate(&expr)?;
            Ok(Self(expr))
        }

        /// Validates a cron expression.
        fn validate(expr: &str) -> Result<(), CronScheduleError> {
            let fields: Vec<&str> = expr.split_whitespace().collect();
            if fields.len() != 5 {
                return Err(CronScheduleError::InvalidFieldCount(fields.len()));
            }
            // Basic validation - each field should contain valid cron syntax
            for (i, field) in fields.iter().enumerate() {
                let valid = field.chars().all(|c| {
                    c.is_ascii_alphanumeric()
                        || matches!(c, '*' | '?' | '-' | '/' | ',' | 'L' | 'W')
                });
                if !valid {
                    return Err(CronScheduleError::InvalidCharacter {
                        field_index: i,
                        field: field.to_string(),
                    });
                }
            }
            Ok(())
        }

        /// Returns the cron expression string.
        pub fn expression(&self) -> &str {
            &self.0
        }

        /// Returns the next execution time after the given reference time.
        pub fn next_execution(&self, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
            // This is a simplified implementation. A full implementation would
            // parse and calculate the next execution time based on cron fields.
            // For production, consider using the `cron` crate's `Schedule` type.
            debug!("Cron schedule: {}, next after: {}", self.0, after);
            None
        }
    }

    impl std::fmt::Display for CronSchedule {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{}", self.0)
        }
    }

    /// Specification for a scheduled job.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct JobSpec {
        /// Unique job ID.
        pub id: Uuid,
        /// Human-readable job name.
        pub name: String,
        /// Cron schedule for the job.
        pub schedule: CronSchedule,
        /// Optional job description.
        pub description: Option<String>,
        /// Optional metadata.
        #[serde(default)]
        pub metadata: HashMap<String, String>,
        /// Whether the job is enabled.
        pub enabled: bool,
    }

    impl JobSpec {
        /// Creates a new job specification.
        pub fn new(name: impl Into<String>, schedule: CronSchedule) -> Self {
            Self {
                id: Uuid::new_v4(),
                name: name.into(),
                schedule,
                description: None,
                metadata: HashMap::new(),
                enabled: true,
            }
        }

        /// Sets the job description.
        pub fn with_description(mut self, desc: impl Into<String>) -> Self {
            self.description = Some(desc.into());
            self
        }

        /// Adds metadata key-value pair.
        pub fn with_metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
            self.metadata.insert(key.into(), value.into());
            self
        }

        /// Disables the job.
        pub fn disable(mut self) -> Self {
            self.enabled = false;
            self
        }

        /// Enables the job.
        pub fn enable(mut self) -> Self {
            self.enabled = true;
            self
        }
    }

    /// Error types for job scheduler operations.
    #[derive(Error, Debug)]
    pub enum SchedulerError {
        #[error("Job with ID {0} not found")]
        JobNotFound(Uuid),

        #[error("Job with name '{0}' already exists")]
        JobAlreadyExists(String),

        #[error("Invalid cron schedule: {0}")]
        InvalidCronSchedule(#[from] CronScheduleError),

        #[error("Job execution failed: {0}")]
        ExecutionFailed(String),

        #[error("Scheduler is not running")]
        NotRunning,

        #[error("Failed to store job state: {0}")]
        StorageError(String),
    }

    /// Error types for cron schedule validation.
    #[derive(Error, Debug)]
    pub enum CronScheduleError {
        #[error("Invalid cron expression: expected 5 fields, got {0}")]
        InvalidFieldCount(usize),

        #[error("Invalid character in field {field_index}: '{field}'")]
        InvalidCharacter { field_index: usize, field: String },
    }

    /// Trait for job scheduler implementations.
    #[async_trait::async_trait]
    pub trait JobScheduler: Send + Sync + std::fmt::Debug {
        /// Registers a job with the scheduler.
        async fn register_job(
            &mut self,
            spec: JobSpec,
            job: Arc<dyn Job>,
        ) -> Result<(), SchedulerError>;

        /// Unregisters a job by ID.
        async fn unregister_job(&mut self, id: Uuid) -> Result<(), SchedulerError>;

        /// Unregisters a job by name.
        async fn unregister_job_by_name(&mut self, name: &str) -> Result<(), SchedulerError>;

        /// Gets a job specification by ID.
        async fn get_job(&self, id: Uuid) -> Result<JobSpec, SchedulerError>;

        /// Gets all registered job specifications.
        async fn list_jobs(&self) -> Vec<JobSpec>;

        /// Starts the scheduler.
        async fn start(&mut self) -> Result<(), SchedulerError>;

        /// Stops the scheduler.
        async fn stop(&mut self) -> Result<(), SchedulerError>;

        /// Returns the scheduler state.
        fn state(&self) -> SchedulerState;
    }

    /// Scheduler state.
    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
    pub enum SchedulerState {
        /// Scheduler is stopped.
        Stopped,
        /// Scheduler is running.
        Running,
        /// Scheduler is paused.
        Paused,
    }

    impl std::fmt::Display for SchedulerState {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                SchedulerState::Stopped => write!(f, "stopped"),
                SchedulerState::Running => write!(f, "running"),
                SchedulerState::Paused => write!(f, "paused"),
            }
        }
    }
}

/// Built-in job implementations.
pub mod jobs {
    use super::*;
    use std::time::Duration;

    /// Pre-market TOTP refresh job.
    ///
    /// Refreshes TOTP secrets before market open for authenticated sessions.
    #[derive(Debug)]
    pub struct PreMarketTOTPJob {
        spec: JobSpec,
    }

    impl PreMarketTOTPJob {
        /// Creates a new pre-market TOTP refresh job.
        pub fn new() -> Self {
            let spec = JobSpec::new(
                "pre_market_totp_refresh",
                CronSchedule::new("0 8 * * 1-5").unwrap(), // 8:00 AM weekdays
            )
            .with_description("Refresh TOTP secrets before market open");
            Self { spec }
        }

        /// Creates a job with a custom schedule.
        pub fn with_schedule(schedule: CronSchedule) -> Self {
            let spec = JobSpec::new("pre_market_totp_refresh", schedule)
                .with_description("Refresh TOTP secrets before market open");
            Self { spec }
        }
    }

    impl Default for PreMarketTOTPJob {
        fn default() -> Self {
            Self::new()
        }
    }

    #[async_trait::async_trait]
    impl Job for PreMarketTOTPJob {
        fn name(&self) -> &str {
            &self.spec.name
        }

        async fn execute(&self) -> Result<(), JobError> {
            info!("Executing pre-market TOTP refresh");
            // Simulated TOTP refresh logic
            tokio::time::sleep(Duration::from_millis(100)).await;
            info!("Pre-market TOTP refresh completed");
            Ok(())
        }

        fn spec(&self) -> &JobSpec {
            &self.spec
        }
    }

    /// EOD settlement job.
    ///
    /// Performs end-of-day settlement operations including position reconciliation
    /// and P&L calculations.
    #[derive(Debug)]
    pub struct EODSettlementJob {
        spec: JobSpec,
    }

    impl EODSettlementJob {
        /// Creates a new EOD settlement job.
        pub fn new() -> Self {
            let spec = JobSpec::new(
                "eod_settlement",
                CronSchedule::new("0 15 * * 1-5").unwrap(), // 3:00 PM weekdays
            )
            .with_description("End-of-day settlement and position reconciliation");
            Self { spec }
        }

        /// Creates a job with a custom schedule.
        pub fn with_schedule(schedule: CronSchedule) -> Self {
            let spec = JobSpec::new("eod_settlement", schedule)
                .with_description("End-of-day settlement and position reconciliation");
            Self { spec }
        }
    }

    impl Default for EODSettlementJob {
        fn default() -> Self {
            Self::new()
        }
    }

    #[async_trait::async_trait]
    impl Job for EODSettlementJob {
        fn name(&self) -> &str {
            &self.spec.name
        }

        async fn execute(&self) -> Result<(), JobError> {
            info!("Executing EOD settlement");
            // Simulated EOD settlement logic
            tokio::time::sleep(Duration::from_millis(200)).await;
            info!("EOD settlement completed");
            Ok(())
        }

        fn spec(&self) -> &JobSpec {
            &self.spec
        }
    }

    /// Snapshot job.
    ///
    /// Generates and stores system snapshots at scheduled intervals.
    #[derive(Debug)]
    pub struct SnapshotJob {
        spec: JobSpec,
        snapshot_type: SnapshotType,
    }

    /// Type of snapshot to generate.
    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
    #[serde(rename_all = "snake_case")]
    pub enum SnapshotType {
        /// Position snapshot.
        Positions,
        /// Portfolio snapshot.
        Portfolio,
        /// System state snapshot.
        System,
    }

    impl std::fmt::Display for SnapshotType {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                SnapshotType::Positions => write!(f, "positions"),
                SnapshotType::Portfolio => write!(f, "portfolio"),
                SnapshotType::System => write!(f, "system"),
            }
        }
    }

    impl SnapshotJob {
        /// Creates a new snapshot job.
        pub fn new(snapshot_type: SnapshotType) -> Self {
            let name = match snapshot_type {
                SnapshotType::Positions => "positions_snapshot",
                SnapshotType::Portfolio => "portfolio_snapshot",
                SnapshotType::System => "system_snapshot",
            };
            let spec = JobSpec::new(
                name,
                CronSchedule::new("0 0 * * *").unwrap(), // Midnight daily
            )
            .with_description(format!("Generate {} snapshot", snapshot_type));
            Self {
                spec,
                snapshot_type,
            }
        }

        /// Creates a job with a custom schedule.
        pub fn with_schedule(snapshot_type: SnapshotType, schedule: CronSchedule) -> Self {
            let name = match snapshot_type {
                SnapshotType::Positions => "positions_snapshot",
                SnapshotType::Portfolio => "portfolio_snapshot",
                SnapshotType::System => "system_snapshot",
            };
            let spec = JobSpec::new(name, schedule)
                .with_description(format!("Generate {} snapshot", snapshot_type));
            Self {
                spec,
                snapshot_type,
            }
        }
    }

    #[async_trait::async_trait]
    impl Job for SnapshotJob {
        fn name(&self) -> &str {
            &self.spec.name
        }

        async fn execute(&self) -> Result<(), JobError> {
            info!("Executing {:?} snapshot", self.snapshot_type);
            // Simulated snapshot generation
            tokio::time::sleep(Duration::from_millis(150)).await;
            info!("{:?} snapshot completed", self.snapshot_type);
            Ok(())
        }

        fn spec(&self) -> &JobSpec {
            &self.spec
        }
    }
}

/// General job execution errors.
#[derive(Error, Debug)]
pub enum JobError {
    #[error("Job execution failed: {0}")]
    ExecutionFailed(String),

    #[error("Job timeout after {0:?}")]
    Timeout(Duration),

    #[error("Job cancelled")]
    Cancelled,

    #[error("Storage error: {0}")]
    Storage(#[from] sqlx::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}
