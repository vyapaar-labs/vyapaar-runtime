//! Persistent Job Registry and State Machine (Honba ADR 0017 / Jesse pattern).
//!
//! Manages job specifications, execution states, budget usage, and lease fencing.
//! Safely persists to `~/.vyapaar/jobs.json` or custom path via atomic tmp-rename writes.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::budget::{BudgetExceededError, BudgetUsage, JobBudgets};
use crate::fencing::{FencingError, FencingLease};
use crate::scheduler::CronSchedule;

fn default_true() -> bool {
    true
}

/// Operational status of a scheduled job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub enum JobStatus {
    /// Active and eligible for scheduled or manual execution.
    #[default]
    Active,
    /// Temporarily paused by operator or policy.
    Paused,
    /// Blocked awaiting human approval (e.g. manual threshold policy or budget breach).
    PendingApproval,
    /// Successfully ran to completion (e.g. for `once = true` jobs).
    Completed,
    /// Terminal failure or unrecoverable error.
    Failed,
}

impl std::fmt::Display for JobStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Active => write!(f, "Active"),
            Self::Paused => write!(f, "Paused"),
            Self::PendingApproval => write!(f, "PendingApproval"),
            Self::Completed => write!(f, "Completed"),
            Self::Failed => write!(f, "Failed"),
        }
    }
}

/// Policy for handling missed scheduled ticks (e.g. during daemon downtime).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum MissedPolicy {
    /// Skip missed ticks and advance to the next future schedule.
    #[default]
    Skip,
    /// Immediately catch up missed executions sequentially.
    CatchUp,
}

/// Approval policy for job executions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ApprovalPolicy {
    /// Automatically approve executions within budget.
    #[default]
    Auto,
    /// Always hold executions in `PendingApproval` until explicit human signoff.
    Manual,
}

/// Specification and runtime state of a background job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobSpec {
    /// Unique job ID.
    pub id: Uuid,
    /// Human-readable job name.
    pub name: String,
    /// High-level goal or strategy intent.
    #[serde(default)]
    pub goal: String,
    /// Cron schedule for the job.
    pub schedule: CronSchedule,
    /// Optional job description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Optional metadata key-value pairs.
    #[serde(default)]
    pub metadata: HashMap<String, String>,
    /// Whether the job is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Current job status.
    #[serde(default)]
    pub status: JobStatus,
    /// Multi-dimensional budget constraints.
    #[serde(default)]
    pub budgets: JobBudgets,
    /// Resource usage tracker.
    #[serde(default)]
    pub usage: BudgetUsage,
    /// Policy for missed scheduled ticks.
    #[serde(default)]
    pub missed_policy: MissedPolicy,
    /// Approval policy for executions.
    #[serde(default)]
    pub approval_policy: ApprovalPolicy,
    /// Currently active worker fencing lease.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_lease: Option<FencingLease>,
    /// Monotonic fencing counter.
    #[serde(default)]
    pub fencing_counter: u64,
    /// Creation timestamp.
    #[serde(default = "Utc::now")]
    pub created_at: DateTime<Utc>,
    /// Last execution timestamp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run_at: Option<DateTime<Utc>>,
    /// Next scheduled execution timestamp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_run_at: Option<DateTime<Utc>>,
    /// Whether execution is strictly read-only.
    #[serde(default)]
    pub read_only: bool,
    /// Whether to run only once then mark completed.
    #[serde(default)]
    pub once: bool,
}

impl JobSpec {
    /// Creates a new job specification.
    pub fn new(name: impl Into<String>, schedule: CronSchedule) -> Self {
        let name_str = name.into();
        Self {
            id: Uuid::new_v4(),
            name: name_str.clone(),
            goal: name_str,
            schedule,
            description: None,
            metadata: HashMap::new(),
            enabled: true,
            status: JobStatus::Active,
            budgets: JobBudgets::default(),
            usage: BudgetUsage::default(),
            missed_policy: MissedPolicy::Skip,
            approval_policy: ApprovalPolicy::Auto,
            active_lease: None,
            fencing_counter: 0,
            created_at: Utc::now(),
            last_run_at: None,
            next_run_at: None,
            read_only: false,
            once: false,
        }
    }

    /// Sets the high-level goal.
    pub fn with_goal(mut self, goal: impl Into<String>) -> Self {
        self.goal = goal.into();
        self
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

    /// Sets the multi-dimensional budget constraints.
    pub fn with_budgets(mut self, budgets: JobBudgets) -> Self {
        self.budgets = budgets;
        self
    }

    /// Sets the policy for missed executions.
    pub fn with_missed_policy(mut self, policy: MissedPolicy) -> Self {
        self.missed_policy = policy;
        self
    }

    /// Sets the approval policy.
    pub fn with_approval_policy(mut self, policy: ApprovalPolicy) -> Self {
        self.approval_policy = policy;
        self
    }

    /// Sets whether execution is read-only.
    pub fn with_read_only(mut self, read_only: bool) -> Self {
        self.read_only = read_only;
        self
    }

    /// Sets whether the job runs only once.
    pub fn with_once(mut self, once: bool) -> Self {
        self.once = once;
        self
    }

    /// Disables the job (sets `enabled = false` and `status = Paused`).
    pub fn disable(mut self) -> Self {
        self.enabled = false;
        self.status = JobStatus::Paused;
        self
    }

    /// Enables the job (sets `enabled = true` and `status = Active`).
    pub fn enable(mut self) -> Self {
        self.enabled = true;
        self.status = JobStatus::Active;
        self
    }
}

/// Errors returned by the persistent job registry.
#[derive(Debug, Error)]
pub enum RegistryError {
    #[error("Job with ID {0} not found")]
    JobNotFound(Uuid),

    #[error("Job with name '{0}' already exists")]
    JobAlreadyExists(String),

    #[error("Lease error: {0}")]
    Lease(#[from] FencingError),

    #[error("Budget exceeded: {0}")]
    Budget(#[from] BudgetExceededError),

    #[error("Invalid job state transition: {0}")]
    InvalidState(String),

    #[error("File I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}

/// Persistent file-based registry storing job specifications in JSON format.
///
/// Updates are atomic: serialized to a temporary sibling file (`jobs.json.tmp`)
/// and renamed over the destination to prevent corruption during crashes.
pub struct FileJobRegistry {
    path: PathBuf,
    jobs: HashMap<Uuid, JobSpec>,
}

impl FileJobRegistry {
    /// Loads the registry from `path`, creating parent directories and an empty registry if missing.
    pub fn load_or_create(path: impl Into<PathBuf>) -> Result<Self, RegistryError> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        if !path.exists() {
            let registry = Self {
                path,
                jobs: HashMap::new(),
            };
            registry.save()?;
            return Ok(registry);
        }

        let content = fs::read_to_string(&path)?;
        let items: Vec<JobSpec> = if content.trim().is_empty() {
            Vec::new()
        } else {
            serde_json::from_str(&content)?
        };

        let mut jobs = HashMap::new();
        for item in items {
            jobs.insert(item.id, item);
        }

        Ok(Self { path, jobs })
    }

    /// Path to the backing storage file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Atomically writes the entire registry to disk.
    pub fn save(&self) -> Result<(), RegistryError> {
        let mut list: Vec<&JobSpec> = self.jobs.values().collect();
        list.sort_by_key(|a| a.created_at);

        let json = serde_json::to_string_pretty(&list)?;
        let tmp_path = self.path.with_extension("json.tmp");

        let mut file = File::create(&tmp_path)?;
        file.write_all(json.as_bytes())?;
        file.flush()?;
        file.sync_all()?;

        fs::rename(&tmp_path, &self.path)?;
        Ok(())
    }

    /// Registers a new job specification and flushes to disk.
    pub fn add_job(&mut self, spec: JobSpec) -> Result<Uuid, RegistryError> {
        if self.jobs.values().any(|j| j.name == spec.name) {
            return Err(RegistryError::JobAlreadyExists(spec.name));
        }
        let id = spec.id;
        self.jobs.insert(id, spec);
        self.save()?;
        Ok(id)
    }

    /// Retrieves an immutable reference to a job specification by ID.
    pub fn get_job(&self, id: &Uuid) -> Option<&JobSpec> {
        self.jobs.get(id)
    }

    /// Retrieves a mutable reference to a job specification by ID.
    pub fn get_job_mut(&mut self, id: &Uuid) -> Option<&mut JobSpec> {
        self.jobs.get_mut(id)
    }

    /// Returns a list of all registered jobs, sorted by creation timestamp.
    pub fn list_jobs(&self) -> Vec<JobSpec> {
        let mut all: Vec<JobSpec> = self.jobs.values().cloned().collect();
        all.sort_by_key(|a| a.created_at);
        all
    }

    /// Removes a job specification by ID.
    pub fn remove_job(&mut self, id: &Uuid) -> Result<JobSpec, RegistryError> {
        let removed = self
            .jobs
            .remove(id)
            .ok_or(RegistryError::JobNotFound(*id))?;
        self.save()?;
        Ok(removed)
    }

    /// Pauses an active job.
    pub fn pause_job(&mut self, id: &Uuid) -> Result<(), RegistryError> {
        let job = self
            .jobs
            .get_mut(id)
            .ok_or(RegistryError::JobNotFound(*id))?;
        job.status = JobStatus::Paused;
        job.enabled = false;
        self.save()
    }

    /// Resumes a paused job.
    pub fn resume_job(&mut self, id: &Uuid) -> Result<(), RegistryError> {
        let job = self
            .jobs
            .get_mut(id)
            .ok_or(RegistryError::JobNotFound(*id))?;
        job.status = JobStatus::Active;
        job.enabled = true;
        self.save()
    }

    /// Claims execution of a job by granting a new worker lease with an incremented fencing token.
    pub fn claim_job(
        &mut self,
        id: &Uuid,
        worker_id: &str,
        ttl: std::time::Duration,
        now: DateTime<Utc>,
    ) -> Result<FencingLease, RegistryError> {
        let job = self
            .jobs
            .get_mut(id)
            .ok_or(RegistryError::JobNotFound(*id))?;

        if job.status == JobStatus::Paused {
            return Err(RegistryError::InvalidState(format!(
                "Cannot claim paused job {id}"
            )));
        }
        if job.status == JobStatus::PendingApproval {
            return Err(RegistryError::InvalidState(format!(
                "Job {id} requires human approval before execution"
            )));
        }

        // Check if existing lease is still valid
        if let Some(existing) = &job.active_lease {
            if !existing.is_expired(now) {
                return Err(RegistryError::InvalidState(format!(
                    "Job {id} is currently claimed by worker '{}' until {}",
                    existing.worker_id, existing.expires_at
                )));
            }
        }

        job.fencing_counter = job.fencing_counter.saturating_add(1);
        let lease = FencingLease::grant(*id, worker_id, job.fencing_counter, ttl, now);
        job.active_lease = Some(lease.clone());
        job.last_run_at = Some(now);

        self.save()?;
        Ok(lease)
    }

    /// Heartbeats an active lease to extend its expiration.
    pub fn heartbeat_lease(
        &mut self,
        id: &Uuid,
        worker_id: &str,
        token: u64,
        now: DateTime<Utc>,
    ) -> Result<(), RegistryError> {
        let job = self
            .jobs
            .get_mut(id)
            .ok_or(RegistryError::JobNotFound(*id))?;

        let lease = job
            .active_lease
            .as_mut()
            .ok_or(FencingError::NoActiveLease(*id))?;

        lease.validate(worker_id, token, now)?;
        lease.heartbeat(now)?;

        self.save()
    }

    /// Releases an active worker lease and updates the job's terminal or resting status.
    pub fn release_lease(
        &mut self,
        id: &Uuid,
        worker_id: &str,
        token: u64,
        next_status: JobStatus,
        now: DateTime<Utc>,
    ) -> Result<(), RegistryError> {
        let job = self
            .jobs
            .get_mut(id)
            .ok_or(RegistryError::JobNotFound(*id))?;

        if let Some(lease) = &job.active_lease {
            lease.validate(worker_id, token, now)?;
        } else {
            return Err(RegistryError::Lease(FencingError::NoActiveLease(*id)));
        }

        job.active_lease = None;
        job.status = if job.once && next_status == JobStatus::Completed {
            JobStatus::Completed
        } else {
            next_status
        };

        self.save()
    }

    /// Records resource usage and checks if the job's budget is exceeded.
    ///
    /// If budget is exceeded or approval policy is manual, the job status transitions to `PendingApproval`.
    pub fn record_usage(
        &mut self,
        id: &Uuid,
        additional_usage: &BudgetUsage,
        now: DateTime<Utc>,
    ) -> Result<JobStatus, RegistryError> {
        let job = self
            .jobs
            .get_mut(id)
            .ok_or(RegistryError::JobNotFound(*id))?;

        job.usage.merge(additional_usage, now);

        if let Err(_exceeded) = job.budgets.validate(&job.usage) {
            job.status = JobStatus::PendingApproval;
        } else if job.approval_policy == ApprovalPolicy::Manual {
            job.status = JobStatus::PendingApproval;
        }

        let status = job.status;
        self.save()?;
        Ok(status)
    }

    /// Sweeps all jobs with expired leases and marks them failed or cleans the stale lease.
    /// Returns the list of recovered job IDs.
    pub fn sweep_expired_leases(&mut self, now: DateTime<Utc>) -> Vec<Uuid> {
        let mut swept = Vec::new();
        for job in self.jobs.values_mut() {
            if let Some(lease) = &job.active_lease {
                if lease.is_expired(now) {
                    swept.push(job.id);
                    job.active_lease = None;
                    if job.status == JobStatus::Active {
                        job.status = JobStatus::Failed;
                    }
                }
            }
        }
        if !swept.is_empty() {
            let _ = self.save();
        }
        swept
    }

    /// Returns all jobs currently in the `PendingApproval` queue.
    pub fn approvals_queue(&self) -> Vec<JobSpec> {
        self.jobs
            .values()
            .filter(|j| j.status == JobStatus::PendingApproval)
            .cloned()
            .collect()
    }

    /// Approves a job in `PendingApproval`, returning it to `Active`.
    pub fn approve_job(&mut self, id: &Uuid) -> Result<(), RegistryError> {
        let job = self
            .jobs
            .get_mut(id)
            .ok_or(RegistryError::JobNotFound(*id))?;
        if job.status != JobStatus::PendingApproval {
            return Err(RegistryError::InvalidState(format!(
                "Job {id} is not in PendingApproval status (current: {})",
                job.status
            )));
        }
        job.status = JobStatus::Active;
        self.save()
    }
}
