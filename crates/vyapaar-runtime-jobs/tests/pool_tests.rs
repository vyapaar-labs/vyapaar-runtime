//! Integration tests for BoundedWorkerPool (ADR 0017 / Jesse pattern).

use std::sync::Arc;
use std::time::Duration;

use rayon::prelude::*;
use vyapaar_runtime_jobs::{BoundedWorkerPool, PoolError, WorkerPoolConfig};

#[tokio::test]
async fn test_pool_dispatch_success() {
    let config = WorkerPoolConfig::new(2, 2, 2);
    let pool = BoundedWorkerPool::new(config);

    let result = pool.dispatch(|| 42).await.unwrap();
    assert_eq!(result, 42);
}

#[tokio::test]
async fn test_max_concurrent_limits_and_backpressure_429() {
    // max_concurrent: 2, max_queued: 2
    let config = WorkerPoolConfig::new(2, 2, 2);
    let pool = Arc::new(BoundedWorkerPool::new(config));

    let barrier_start = Arc::new(tokio::sync::Barrier::new(3));
    let barrier_release = Arc::new(tokio::sync::Barrier::new(3));

    // Spawn Job 1 (occupies permit 1)
    let p1 = pool.clone();
    let bs1 = barrier_start.clone();
    let br1 = barrier_release.clone();
    let h1 = tokio::spawn(async move {
        p1.dispatch(move || {
            futures::executor::block_on(bs1.wait());
            futures::executor::block_on(br1.wait());
            1
        })
        .await
    });

    // Spawn Job 2 (occupies permit 2)
    let p2 = pool.clone();
    let bs2 = barrier_start.clone();
    let br2 = barrier_release.clone();
    let h2 = tokio::spawn(async move {
        p2.dispatch(move || {
            futures::executor::block_on(bs2.wait());
            futures::executor::block_on(br2.wait());
            2
        })
        .await
    });

    // Wait until both jobs are running inside their threads
    barrier_start.wait().await;
    assert_eq!(pool.active_count(), 2);
    assert_eq!(pool.available_permits(), 0);

    // Spawn Job 3 (queued 1)
    let p3 = pool.clone();
    let h3 = tokio::spawn(async move { p3.dispatch(|| 3).await });

    // Spawn Job 4 (queued 2)
    let p4 = pool.clone();
    let h4 = tokio::spawn(async move { p4.dispatch(|| 4).await });

    // Give tasks a brief moment to register in queue
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(pool.queued_count(), 2);

    // Job 5: queue is saturated (max_queued == 2), must return 429 Backpressure immediately!
    let err = pool.dispatch(|| 5).await.unwrap_err();
    assert!(err.is_backpressure());
    assert_eq!(err.status_code(), 429);
    assert_eq!(err, PoolError::QueueSaturated { max_queued: 2 });

    // Release Job 1 and Job 2
    barrier_release.wait().await;

    assert_eq!(h1.await.unwrap().unwrap(), 1);
    assert_eq!(h2.await.unwrap().unwrap(), 2);
    assert_eq!(h3.await.unwrap().unwrap(), 3);
    assert_eq!(h4.await.unwrap().unwrap(), 4);

    assert_eq!(pool.queued_count(), 0);
    assert_eq!(pool.active_count(), 0);
}

#[tokio::test]
async fn test_worker_panic_isolation_and_permit_release() {
    let config = WorkerPoolConfig::new(1, 1, 1);
    let pool = Arc::new(BoundedWorkerPool::new(config));

    // Task panics
    let panic_err = pool
        .dispatch(|| {
            panic!("intentional test panic");
        })
        .await
        .unwrap_err();

    match panic_err {
        PoolError::ExecutionPanic(msg) => {
            assert!(msg.contains("intentional test panic"));
        }
        other => panic!("expected ExecutionPanic, got {other:?}"),
    }

    // Verify permit was released and pool is healthy
    assert_eq!(pool.available_permits(), 1);
    let next_result = pool.dispatch(|| "healthy").await.unwrap();
    assert_eq!(next_result, "healthy");
}

#[tokio::test]
async fn test_jesse_style_sweep_core_budgeting() {
    // Core budget max_sweep_cores = 3
    let config = WorkerPoolConfig::new(4, 4, 3);
    let pool = BoundedWorkerPool::new(config);

    // Requesting 8 cores should be clamped to max_sweep_cores (3)
    assert_eq!(pool.budget_sweep_cores(Some(8)), 3);
    // Requesting None should default to max_sweep_cores (3)
    assert_eq!(pool.budget_sweep_cores(None), 3);
    // Requesting 2 should stay 2
    assert_eq!(pool.budget_sweep_cores(Some(2)), 2);

    // Run sweep using rayon parallel iterator inside SweepContext
    let sweep_result = pool
        .dispatch_sweep(Some(3), |ctx| {
            assert_eq!(ctx.cores, 3);
            ctx.install(|| {
                let items = vec![1, 2, 3, 4, 5, 6, 7, 8];
                items.par_iter().map(|&x| x * 10).collect::<Vec<_>>()
            })
        })
        .await
        .unwrap();

    assert_eq!(sweep_result, vec![10, 20, 30, 40, 50, 60, 70, 80]);
}

#[tokio::test]
async fn test_pool_shutdown() {
    let config = WorkerPoolConfig::new(1, 2, 1);
    let pool = Arc::new(BoundedWorkerPool::new(config));

    let barrier_start = Arc::new(tokio::sync::Barrier::new(2));
    let barrier_release = Arc::new(tokio::sync::Barrier::new(2));

    let p1 = pool.clone();
    let bs = barrier_start.clone();
    let br = barrier_release.clone();
    let _h1 = tokio::spawn(async move {
        p1.dispatch(move || {
            futures::executor::block_on(bs.wait());
            futures::executor::block_on(br.wait());
        })
        .await
    });

    barrier_start.wait().await;

    // Queue a job
    let p2 = pool.clone();
    let h2 = tokio::spawn(async move { p2.dispatch(|| 100).await });

    tokio::time::sleep(Duration::from_millis(50)).await;

    // Shut down pool
    pool.shutdown();
    assert!(pool.is_shutting_down());

    // Any new dispatch fails with ShuttingDown
    let new_err = pool.dispatch(|| 200).await.unwrap_err();
    assert_eq!(new_err, PoolError::ShuttingDown);
    assert_eq!(new_err.status_code(), 503);

    barrier_release.wait().await;

    // Queued job also fails with ShuttingDown
    let queued_res = h2.await.unwrap();
    assert_eq!(queued_res.unwrap_err(), PoolError::ShuttingDown);
}
