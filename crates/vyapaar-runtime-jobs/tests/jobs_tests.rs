//! Integration tests for the jobs crate: cron validation and job specs.

use vyapaar_runtime_jobs::{CronSchedule, JobSpec};

#[test]
fn test_cron_schedule_validation() {
    // Valid expressions
    assert!(CronSchedule::new("* * * * *").is_ok());
    assert!(CronSchedule::new("0 8 * * 1-5").is_ok());
    assert!(CronSchedule::new("0 0 * * *").is_ok());
    assert!(CronSchedule::new("*/5 * * * *").is_ok());

    // Invalid expressions
    assert!(CronSchedule::new("* * * *").is_err()); // Too few fields
    assert!(CronSchedule::new("* * * * * *").is_err()); // Too many fields
}

#[test]
fn test_job_spec_creation() {
    let schedule = CronSchedule::new("0 * * * *").unwrap();
    let spec = JobSpec::new("test_job", schedule)
        .with_description("A test job")
        .with_metadata("key1", "value1");

    assert_eq!(spec.name, "test_job");
    assert_eq!(spec.description, Some("A test job".to_string()));
    assert_eq!(spec.metadata.get("key1"), Some(&"value1".to_string()));
    assert!(spec.enabled);
}

#[test]
fn test_job_spec_disable_enable() {
    let schedule = CronSchedule::new("0 * * * *").unwrap();
    let mut spec = JobSpec::new("test_job", schedule).disable();

    assert!(!spec.enabled);

    spec = spec.enable();
    assert!(spec.enabled);
}
