//! Bounded Worker Pool with core budgeting and queue backpressure (ADR 0017 / Jesse pattern).
//!
//! Controls concurrent job executions with CPU core budgeting and returns 429 backpressure
//! when the waiting queue is saturated. Protects host resources during large sweeps.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use thiserror::Error;
use tokio::sync::Semaphore;

/// Errors returned by the worker pool.
#[derive(Debug, Error, PartialEq, Eq, Clone)]
pub enum PoolError {
    /// The queue is saturated; returns HTTP 429 Backpressure.
    #[error(
        "Worker pool queue is saturated (HTTP 429 Backpressure): maximum {max_queued} queued jobs exceeded"
    )]
    QueueSaturated {
        /// Maximum queued jobs allowed.
        max_queued: usize,
    },

    /// The worker pool is shutting down and no longer accepts work.
    #[error("Worker pool is shutting down")]
    ShuttingDown,

    /// Worker execution panicked.
    #[error("Worker execution panicked: {0}")]
    ExecutionPanic(String),

    /// Core allocation error.
    #[error("Core allocation failed: {0}")]
    CoreAllocation(String),
}

impl PoolError {
    /// Associated HTTP status code (429 for QueueSaturated, 503 for ShuttingDown, 500 for Panic).
    pub fn status_code(&self) -> u16 {
        match self {
            Self::QueueSaturated { .. } => 429,
            Self::ShuttingDown => 503,
            Self::ExecutionPanic(_) => 500,
            Self::CoreAllocation(_) => 422,
        }
    }

    /// Whether this error represents HTTP 429 Backpressure.
    pub fn is_backpressure(&self) -> bool {
        matches!(self, Self::QueueSaturated { .. })
    }
}

/// Configuration for the bounded worker pool.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorkerPoolConfig {
    /// Maximum concurrent jobs allowed simultaneously (e.g. NumCPUs).
    pub max_concurrent: usize,
    /// Maximum queued jobs waiting before returning 429 Backpressure.
    pub max_queued: usize,
    /// Maximum cores assignable to a single optimization sweep.
    pub max_sweep_cores: usize,
}

impl Default for WorkerPoolConfig {
    fn default() -> Self {
        let cpus = num_cpus::get().max(1);
        Self {
            max_concurrent: cpus,
            max_queued: cpus * 2,
            max_sweep_cores: (cpus / 2).max(1),
        }
    }
}

impl WorkerPoolConfig {
    /// Creates a new config with explicit limits.
    pub fn new(max_concurrent: usize, max_queued: usize, max_sweep_cores: usize) -> Self {
        Self {
            max_concurrent: max_concurrent.max(1),
            max_queued,
            max_sweep_cores: max_sweep_cores.max(1),
        }
    }
}

/// Context provided to sweep jobs, encapsulating core budget and a private rayon thread pool.
pub struct SweepContext {
    /// Allocated core count for this sweep.
    pub cores: usize,
    /// Isolated Rayon thread pool for this sweep.
    pub pool: Arc<rayon::ThreadPool>,
}

impl SweepContext {
    /// Executes a closure within this sweep's isolated Rayon thread pool.
    pub fn install<R: Send>(&self, op: impl FnOnce() -> R + Send) -> R {
        self.pool.install(op)
    }
}

/// Bounded worker pool with core allocation and backpressure.
pub struct BoundedWorkerPool {
    semaphore: Arc<Semaphore>,
    queued: Arc<AtomicUsize>,
    shutting_down: Arc<AtomicBool>,
    config: WorkerPoolConfig,
}

impl BoundedWorkerPool {
    /// Creates a new worker pool with `config`.
    pub fn new(config: WorkerPoolConfig) -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(config.max_concurrent)),
            queued: Arc::new(AtomicUsize::new(0)),
            shutting_down: Arc::new(AtomicBool::new(false)),
            config,
        }
    }

    /// The pool's configuration.
    pub fn config(&self) -> &WorkerPoolConfig {
        &self.config
    }

    /// Number of active workers currently holding permits.
    pub fn active_count(&self) -> usize {
        self.config
            .max_concurrent
            .saturating_sub(self.semaphore.available_permits())
    }

    /// Number of jobs currently queued waiting for permits.
    pub fn queued_count(&self) -> usize {
        self.queued.load(Ordering::SeqCst)
    }

    /// Available semaphore permits.
    pub fn available_permits(&self) -> usize {
        self.semaphore.available_permits()
    }

    /// Whether the pool is in the process of shutting down.
    pub fn is_shutting_down(&self) -> bool {
        self.shutting_down.load(Ordering::SeqCst)
    }

    /// Shuts down the worker pool, waking any queued waiters with `PoolError::ShuttingDown`.
    pub fn shutdown(&self) {
        self.shutting_down.store(true, Ordering::SeqCst);
        self.semaphore.close();
    }

    /// Calculates budgeted cores for a sweep job based on requested cores and `max_sweep_cores`.
    pub fn budget_sweep_cores(&self, requested: Option<usize>) -> usize {
        match requested {
            Some(r) => r.clamp(1, self.config.max_sweep_cores),
            None => self.config.max_sweep_cores,
        }
    }

    /// Dispatches a synchronous task onto the worker pool.
    ///
    /// If permits are available, acquires immediately.
    /// If all workers are busy, enters the queue unless `queued >= max_queued`,
    /// in which case it returns `Err(PoolError::QueueSaturated)`.
    ///
    /// Releases the permit even if `task` panics.
    pub async fn dispatch<F, T>(&self, task: F) -> Result<T, PoolError>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        let permit = self.acquire_permit().await?;

        tokio::task::spawn_blocking(move || {
            let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(task));
            drop(permit);
            match res {
                Ok(val) => Ok(val),
                Err(panic_err) => {
                    let msg = if let Some(s) = panic_err.downcast_ref::<&str>() {
                        s.to_string()
                    } else if let Some(s) = panic_err.downcast_ref::<String>() {
                        s.clone()
                    } else {
                        "unknown worker panic".to_string()
                    };
                    Err(PoolError::ExecutionPanic(msg))
                }
            }
        })
        .await
        .map_err(|e| PoolError::ExecutionPanic(e.to_string()))?
    }

    /// Dispatches an optimization sweep with Jesse-style core budgeting and worker threadpool isolation.
    ///
    /// Allocates `min(requested_cores, max_sweep_cores)` cores and spins up a dedicated
    /// Rayon threadpool for the sweep task.
    pub async fn dispatch_sweep<F, T>(
        &self,
        requested_cores: Option<usize>,
        task: F,
    ) -> Result<T, PoolError>
    where
        F: FnOnce(SweepContext) -> T + Send + 'static,
        T: Send + 'static,
    {
        let cores = self.budget_sweep_cores(requested_cores);
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(cores)
            .build()
            .map_err(|e| PoolError::CoreAllocation(e.to_string()))?;

        let ctx = SweepContext {
            cores,
            pool: Arc::new(pool),
        };

        self.dispatch(move || task(ctx)).await
    }

    async fn acquire_permit(&self) -> Result<tokio::sync::OwnedSemaphorePermit, PoolError> {
        if self.shutting_down.load(Ordering::SeqCst) {
            return Err(PoolError::ShuttingDown);
        }

        // Try non-blocking acquisition first
        if let Ok(permit) = self.semaphore.clone().try_acquire_owned() {
            return Ok(permit);
        }

        // Must queue: check backpressure limit
        loop {
            if self.shutting_down.load(Ordering::SeqCst) {
                return Err(PoolError::ShuttingDown);
            }
            let q = self.queued.load(Ordering::SeqCst);
            if q >= self.config.max_queued {
                return Err(PoolError::QueueSaturated {
                    max_queued: self.config.max_queued,
                });
            }
            if self
                .queued
                .compare_exchange(q, q + 1, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                break;
            }
        }

        struct QueueGuard(Arc<AtomicUsize>);
        impl Drop for QueueGuard {
            fn drop(&mut self) {
                self.0.fetch_sub(1, Ordering::SeqCst);
            }
        }
        let guard = QueueGuard(self.queued.clone());

        match self.semaphore.clone().acquire_owned().await {
            Ok(permit) => {
                drop(guard);
                Ok(permit)
            }
            Err(_) => {
                drop(guard);
                Err(PoolError::ShuttingDown)
            }
        }
    }
}
