//! Heartbeat lease fencing and worker tokens (Jesse pattern).
//!
//! Protects against split-brain execution across distributed workers.
//! Every active claim holds a monotonic fencing token and lease expiration timestamp.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

/// Fencing and lease validation errors.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum FencingError {
    /// The lease has expired.
    #[error("Lease has expired at {expired_at} (current time {current_time})")]
    LeaseExpired {
        /// Time when the lease expired.
        expired_at: DateTime<Utc>,
        /// Current validation time.
        current_time: DateTime<Utc>,
    },

    /// The supplied fencing token is older than the current lease token.
    #[error("Fencing token mismatch: expected {expected}, got {actual}")]
    TokenMismatch {
        /// Expected token.
        expected: u64,
        /// Actual supplied token.
        actual: u64,
    },

    /// The worker attempting an action does not hold the lease.
    #[error("Worker mismatch: lease held by '{expected}', attempted by '{actual}'")]
    WorkerMismatch {
        /// Worker holding the lease.
        expected: String,
        /// Worker attempting operation.
        actual: String,
    },

    /// No active lease exists for the job.
    #[error("No active lease for job {0}")]
    NoActiveLease(Uuid),
}

/// A fencing lease held by a worker process or thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FencingLease {
    /// Target job ID.
    pub job_id: Uuid,
    /// Unique lease identifier.
    pub lease_id: Uuid,
    /// Identifier of claiming worker (e.g. host:pid, worker-UUID).
    pub worker_id: String,
    /// Monotonically increasing fencing counter.
    pub fencing_token: u64,
    /// When the lease was granted.
    pub granted_at: DateTime<Utc>,
    /// When the lease expires if not refreshed by heartbeat.
    pub expires_at: DateTime<Utc>,
    /// Time-to-live duration in seconds.
    pub ttl_secs: u64,
}

impl FencingLease {
    /// Grants a new lease with an incremented fencing token.
    pub fn grant(
        job_id: Uuid,
        worker_id: impl Into<String>,
        fencing_token: u64,
        ttl: std::time::Duration,
        now: DateTime<Utc>,
    ) -> Self {
        let ttl_secs = ttl.as_secs().max(1);
        let expires_at = now + Duration::seconds(ttl_secs as i64);
        Self {
            job_id,
            lease_id: Uuid::new_v4(),
            worker_id: worker_id.into(),
            fencing_token,
            granted_at: now,
            expires_at,
            ttl_secs,
        }
    }

    /// Extends the lease via worker heartbeat.
    pub fn heartbeat(&mut self, now: DateTime<Utc>) -> Result<(), FencingError> {
        if self.is_expired(now) {
            return Err(FencingError::LeaseExpired {
                expired_at: self.expires_at,
                current_time: now,
            });
        }
        self.expires_at = now + Duration::seconds(self.ttl_secs as i64);
        Ok(())
    }

    /// Checks if the lease has expired relative to `now`.
    pub fn is_expired(&self, now: DateTime<Utc>) -> bool {
        now >= self.expires_at
    }

    /// Validates whether a token and worker match this active lease.
    pub fn validate(
        &self,
        worker_id: &str,
        token: u64,
        now: DateTime<Utc>,
    ) -> Result<(), FencingError> {
        if self.is_expired(now) {
            return Err(FencingError::LeaseExpired {
                expired_at: self.expires_at,
                current_time: now,
            });
        }
        if self.worker_id != worker_id {
            return Err(FencingError::WorkerMismatch {
                expected: self.worker_id.clone(),
                actual: worker_id.to_string(),
            });
        }
        if token != self.fencing_token {
            return Err(FencingError::TokenMismatch {
                expected: self.fencing_token,
                actual: token,
            });
        }
        Ok(())
    }
}
