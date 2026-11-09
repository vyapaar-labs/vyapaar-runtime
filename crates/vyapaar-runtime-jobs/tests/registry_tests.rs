//! Integration tests for FileJobRegistry, multi-dimensional budgets, and heartbeat fencing.

use std::time::Duration;

use chrono::Utc;
use rust_decimal_macros::dec;
use tempfile::TempDir;
use uuid::Uuid;
use vyapaar_runtime_jobs::{
    ApprovalPolicy, BudgetExceededError, BudgetUsage, CronSchedule, FencingError, FencingLease,
    FileJobRegistry, JobBudgets, JobSpec, JobStatus, MissedPolicy, RegistryError,
};

#[test]
fn test_job_spec_rich_attributes() {
    let sched = CronSchedule::new("0 9 * * 1-5").unwrap();
    let budgets = JobBudgets::new()
        .with_max_turns(50)
        .with_max_inr(dec!(500.00))
        .with_max_tokens(100_000);

    let spec = JobSpec::new("daily_rebalancer", sched)
        .with_goal("Rebalance multi-asset momentum portfolio at market open")
        .with_budgets(budgets.clone())
        .with_missed_policy(MissedPolicy::CatchUp)
        .with_approval_policy(ApprovalPolicy::Manual)
        .with_read_only(true)
        .with_once(true);

    assert_eq!(spec.name, "daily_rebalancer");
    assert_eq!(
        spec.goal,
        "Rebalance multi-asset momentum portfolio at market open"
    );
    assert_eq!(spec.budgets, budgets);
    assert_eq!(spec.missed_policy, MissedPolicy::CatchUp);
    assert_eq!(spec.approval_policy, ApprovalPolicy::Manual);
    assert!(spec.read_only);
    assert!(spec.once);
    assert_eq!(spec.status, JobStatus::Active);
}

#[test]
fn test_job_budgets_validation() {
    let budgets = JobBudgets::new()
        .with_max_turns(10)
        .with_max_inr(dec!(250.50))
        .with_max_tokens(5000);

    let mut usage = BudgetUsage::new();
    let now = Utc::now();
    usage.record_turn(now);
    usage.record_inr_cost(dec!(100.00), now);
    usage.record_tokens(1500, now);

    assert!(budgets.validate(&usage).is_ok());

    // Exceed turn budget
    let mut turn_exceeded = usage.clone();
    for _ in 0..15 {
        turn_exceeded.record_turn(now);
    }
    assert_eq!(
        budgets.validate(&turn_exceeded),
        Err(BudgetExceededError::TurnsExceeded {
            current: 16,
            limit: 10,
        })
    );

    // Exceed INR budget
    let mut inr_exceeded = usage.clone();
    inr_exceeded.record_inr_cost(dec!(200.00), now); // 300 > 250.50
    assert!(matches!(
        budgets.validate(&inr_exceeded),
        Err(BudgetExceededError::InrCostExceeded { .. })
    ));

    // Exceed token budget
    let mut token_exceeded = usage.clone();
    token_exceeded.record_tokens(10_000, now);
    assert_eq!(
        budgets.validate(&token_exceeded),
        Err(BudgetExceededError::TokensExceeded {
            current: 11500,
            limit: 5000,
        })
    );
}

#[test]
fn test_fencing_lease_heartbeat_and_validation() {
    let job_id = Uuid::new_v4();
    let now = Utc::now();
    let mut lease = FencingLease::grant(job_id, "worker-node-1", 1, Duration::from_secs(5), now);

    assert_eq!(lease.job_id, job_id);
    assert_eq!(lease.worker_id, "worker-node-1");
    assert_eq!(lease.fencing_token, 1);
    assert!(!lease.is_expired(now));

    // Valid check
    assert!(lease.validate("worker-node-1", 1, now).is_ok());

    // Worker mismatch check
    assert_eq!(
        lease.validate("worker-node-2", 1, now),
        Err(FencingError::WorkerMismatch {
            expected: "worker-node-1".into(),
            actual: "worker-node-2".into(),
        })
    );

    // Token mismatch check
    assert_eq!(
        lease.validate("worker-node-1", 2, now),
        Err(FencingError::TokenMismatch {
            expected: 1,
            actual: 2,
        })
    );

    // Heartbeat before expiry
    let halfway = now + chrono::Duration::seconds(2);
    assert!(lease.heartbeat(halfway).is_ok());
    assert!(lease.expires_at > now + chrono::Duration::seconds(5));

    // Expiry check
    let expired_time = lease.expires_at + chrono::Duration::seconds(1);
    assert!(lease.is_expired(expired_time));
    assert!(matches!(
        lease.heartbeat(expired_time),
        Err(FencingError::LeaseExpired { .. })
    ));
}

#[test]
fn test_file_job_registry_persistence_and_claim_lifecycle() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("jobs.json");

    // 1. Create registry and add jobs
    let mut registry = FileJobRegistry::load_or_create(&db_path).unwrap();
    let sched = CronSchedule::new("0 8 * * 1-5").unwrap();
    let spec1 = JobSpec::new("morning_scanner", sched.clone())
        .with_goal("Scan high momentum Nifty 50 constituents");
    let id1 = registry.add_job(spec1).unwrap();

    let spec2 =
        JobSpec::new("eod_reconcile", sched).with_goal("Audit positions against broker ledger");
    let id2 = registry.add_job(spec2).unwrap();

    assert_eq!(registry.list_jobs().len(), 2);

    // 2. Reload registry from disk to verify atomic persistence
    let mut reloaded = FileJobRegistry::load_or_create(&db_path).unwrap();
    assert_eq!(reloaded.list_jobs().len(), 2);
    assert_eq!(reloaded.get_job(&id1).unwrap().name, "morning_scanner");

    // 3. Worker claims job 1
    let now = Utc::now();
    let lease = reloaded
        .claim_job(&id1, "worker-node-A", Duration::from_secs(10), now)
        .unwrap();
    assert_eq!(lease.fencing_token, 1);
    assert_eq!(lease.worker_id, "worker-node-A");

    // Attempting concurrent claim before expiry is rejected
    let concurrent_err = reloaded
        .claim_job(&id1, "worker-node-B", Duration::from_secs(10), now)
        .unwrap_err();
    assert!(matches!(concurrent_err, RegistryError::InvalidState(_)));

    // 4. Worker heartbeats
    let heartbeat_time = now + chrono::Duration::seconds(3);
    assert!(
        reloaded
            .heartbeat_lease(&id1, "worker-node-A", 1, heartbeat_time)
            .is_ok()
    );

    // 5. Worker completes and releases lease
    assert!(
        reloaded
            .release_lease(&id1, "worker-node-A", 1, JobStatus::Active, heartbeat_time)
            .is_ok()
    );
    assert!(reloaded.get_job(&id1).unwrap().active_lease.is_none());

    // 6. Pause and Resume
    assert!(reloaded.pause_job(&id2).is_ok());
    assert_eq!(reloaded.get_job(&id2).unwrap().status, JobStatus::Paused);
    assert!(
        reloaded
            .claim_job(&id2, "worker-node-A", Duration::from_secs(10), now)
            .is_err()
    );

    assert!(reloaded.resume_job(&id2).is_ok());
    assert_eq!(reloaded.get_job(&id2).unwrap().status, JobStatus::Active);
}

#[test]
fn test_budget_exceeded_triggers_pending_approval() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("jobs.json");
    let mut registry = FileJobRegistry::load_or_create(&db_path).unwrap();

    let sched = CronSchedule::new("0 * * * *").unwrap();
    let spec = JobSpec::new("agent_researcher", sched)
        .with_budgets(JobBudgets::new().with_max_inr(dec!(100.00)));
    let id = registry.add_job(spec).unwrap();

    let now = Utc::now();
    let mut usage = BudgetUsage::new();
    usage.record_inr_cost(dec!(50.00), now);

    // Within budget -> remains Active
    let status = registry.record_usage(&id, &usage, now).unwrap();
    assert_eq!(status, JobStatus::Active);

    // Cost exceeds ₹100 threshold -> transitions to PendingApproval
    let mut more_usage = BudgetUsage::new();
    more_usage.record_inr_cost(dec!(75.00), now);
    let status2 = registry.record_usage(&id, &more_usage, now).unwrap();
    assert_eq!(status2, JobStatus::PendingApproval);

    // Appears in approvals queue
    let queue = registry.approvals_queue();
    assert_eq!(queue.len(), 1);
    assert_eq!(queue[0].id, id);

    // Cannot claim a job pending approval
    assert!(
        registry
            .claim_job(&id, "worker-1", Duration::from_secs(10), now)
            .is_err()
    );

    // Human approves job
    assert!(registry.approve_job(&id).is_ok());
    assert_eq!(registry.get_job(&id).unwrap().status, JobStatus::Active);
}

#[test]
fn test_crash_recovery_sweeper_cleans_expired_leases() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("jobs.json");
    let mut registry = FileJobRegistry::load_or_create(&db_path).unwrap();

    let sched = CronSchedule::new("0 8 * * 1-5").unwrap();
    let spec = JobSpec::new("orphaned_job", sched);
    let id = registry.add_job(spec).unwrap();

    let now = Utc::now();
    // Claim lease with 2-second TTL
    registry
        .claim_job(&id, "crashed-worker", Duration::from_secs(2), now)
        .unwrap();

    // Advance 5 seconds (worker died and stopped heartbeating)
    let future = now + chrono::Duration::seconds(5);
    let swept = registry.sweep_expired_leases(future);

    assert_eq!(swept, vec![id]);
    let recovered_job = registry.get_job(&id).unwrap();
    assert!(recovered_job.active_lease.is_none());
    assert_eq!(recovered_job.status, JobStatus::Failed);
}
