//! Unit tests for vyapaar-runtime-jobs crate.

use vyapaar_runtime_jobs::{CronSchedule, Job, JobError, JobSpec, jobs};

#[test]
fn test_cron_schedule_valid_expressions() {
    // Valid cron expressions should succeed
    assert!(CronSchedule::new("* * * * *").is_ok());
    assert!(CronSchedule::new("0 8 * * 1-5").is_ok());
    assert!(CronSchedule::new("0 0 * * *").is_ok());
    assert!(CronSchedule::new("*/5 * * * *").is_ok());
    assert!(CronSchedule::new("0 8,12,18 * * 1-5").is_ok());
}

#[test]
fn test_cron_schedule_invalid_expressions() {
    // Invalid cron expressions should fail
    assert!(CronSchedule::new("* * * *").is_err()); // Too few fields
    assert!(CronSchedule::new("* * * * * *").is_err()); // Too many fields
    assert!(CronSchedule::new("invalid").is_err()); // Invalid syntax
    assert!(CronSchedule::new("").is_err()); // Empty
}

#[test]
fn test_cron_schedule_expression() {
    let expr = "0 8 * * 1-5";
    let schedule = CronSchedule::new(expr).unwrap();
    assert_eq!(schedule.expression(), expr);
}

#[test]
fn test_job_spec_creation() {
    let schedule = CronSchedule::new("0 * * * *").unwrap();
    let spec = JobSpec::new("test_job", schedule)
        .with_description("A test job")
        .with_metadata("key1", "value1");

    assert_eq!(spec.name, "test_job");
    assert_eq!(spec.id.get_version_num(), 4); // UUID v4
    assert_eq!(spec.description, Some("A test job".to_string()));
    assert_eq!(spec.metadata.get("key1"), Some(&"value1".to_string()));
    assert!(spec.enabled);
}

#[test]
fn test_job_spec_modifiers() {
    let schedule = CronSchedule::new("0 * * * *").unwrap();
    let mut spec = JobSpec::new("test_job", schedule);

    // Test disable
    spec = spec.disable();
    assert!(!spec.enabled);

    // Test enable
    spec = spec.enable();
    assert!(spec.enabled);

    // Test description
    spec = spec.with_description("Updated description");
    assert_eq!(spec.description, Some("Updated description".to_string()));

    // Test metadata
    spec = spec.with_metadata("new_key", "new_value");
    assert_eq!(spec.metadata.get("new_key"), Some(&"new_value".to_string()));
}

#[tokio::test]
async fn test_pre_market_totp_job_creation() {
    let job = jobs::PreMarketTOTPJob::new();
    assert_eq!(job.name(), "pre_market_totp_refresh");
    assert_eq!(job.spec().schedule.expression(), "0 8 * * 1-5");
    assert_eq!(
        job.spec().description,
        Some("Refresh TOTP secrets before market open".to_string())
    );
}

#[tokio::test]
async fn test_pre_market_totp_job_execution() {
    let job = jobs::PreMarketTOTPJob::new();
    assert!(job.execute().await.is_ok());
}

#[tokio::test]
async fn test_pre_market_totp_job_with_custom_schedule() {
    let schedule = CronSchedule::new("0 7 * * 1-5").unwrap();
    let job = jobs::PreMarketTOTPJob::with_schedule(schedule);
    assert_eq!(job.spec().schedule.expression(), "0 7 * * 1-5");
    assert!(job.execute().await.is_ok());
}

#[tokio::test]
async fn test_eod_settlement_job_creation() {
    let job = jobs::EODSettlementJob::new();
    assert_eq!(job.name(), "eod_settlement");
    assert_eq!(job.spec().schedule.expression(), "0 15 * * 1-5");
    assert_eq!(
        job.spec().description,
        Some("End-of-day settlement and position reconciliation".to_string())
    );
}

#[tokio::test]
async fn test_eod_settlement_job_execution() {
    let job = jobs::EODSettlementJob::new();
    assert!(job.execute().await.is_ok());
}

#[tokio::test]
async fn test_eod_settlement_job_with_custom_schedule() {
    let schedule = CronSchedule::new("0 16 * * 1-5").unwrap();
    let job = jobs::EODSettlementJob::with_schedule(schedule);
    assert_eq!(job.spec().schedule.expression(), "0 16 * * 1-5");
    assert!(job.execute().await.is_ok());
}

#[tokio::test]
async fn test_snapshot_job_creation() {
    let job = jobs::SnapshotJob::new(jobs::SnapshotType::Positions);
    assert!(job.execute().await.is_ok());
}

#[tokio::test]
async fn test_snapshot_job_with_type() {
    let jobs_to_test = [
        (jobs::SnapshotType::Positions, "positions_snapshot"),
        (jobs::SnapshotType::Portfolio, "portfolio_snapshot"),
        (jobs::SnapshotType::System, "system_snapshot"),
    ];

    for (snapshot_type, expected_name) in jobs_to_test {
        let job = jobs::SnapshotJob::new(snapshot_type);
        assert_eq!(job.name(), expected_name);
        assert!(job.execute().await.is_ok());
    }
}

#[tokio::test]
async fn test_snapshot_job_with_custom_schedule() {
    let schedule = CronSchedule::new("0 6 * * *").unwrap();
    let job = jobs::SnapshotJob::with_schedule(jobs::SnapshotType::Portfolio, schedule);
    assert_eq!(job.spec().schedule.expression(), "0 6 * * *");
    assert!(job.execute().await.is_ok());
}

#[test]
fn test_job_error_display() {
    let err = JobError::ExecutionFailed("test error".to_string());
    assert_eq!(format!("{}", err), "Job execution failed: test error");

    let err = JobError::Cancelled;
    assert_eq!(format!("{}", err), "Job cancelled");
}

#[test]
fn test_scheduler_state_display() {
    use vyapaar_runtime_jobs::scheduler::SchedulerState;

    assert_eq!(format!("{}", SchedulerState::Stopped), "stopped");
    assert_eq!(format!("{}", SchedulerState::Running), "running");
    assert_eq!(format!("{}", SchedulerState::Paused), "paused");
}

#[test]
fn test_cron_schedule_format() {
    let schedule = CronSchedule::new("0 8 * * 1-5").unwrap();
    assert_eq!(format!("{}", schedule), "0 8 * * 1-5");
}

#[tokio::test]
async fn test_job_spec_serialization() {
    use serde_json;

    let schedule = CronSchedule::new("0 * * * *").unwrap();
    let spec = JobSpec::new("test_job", schedule)
        .with_description("A test job")
        .with_metadata("key1", "value1");

    let json = serde_json::to_string(&spec).unwrap();
    let deserialized: JobSpec = serde_json::from_str(&json).unwrap();

    assert_eq!(spec.name, deserialized.name);
    assert_eq!(spec.description, deserialized.description);
    assert_eq!(spec.metadata, deserialized.metadata);
}
