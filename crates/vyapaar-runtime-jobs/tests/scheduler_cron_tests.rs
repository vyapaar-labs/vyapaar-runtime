//! Scheduling tests: `CronSchedule::next_execution` against concrete instants (PLAN-0001 Stage 4).

use chrono::{DateTime, Utc};
use vyapaar_runtime_jobs::CronSchedule;

fn ts(text: &str) -> DateTime<Utc> {
    chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S")
        .unwrap()
        .and_utc()
}

#[test]
fn test_next_execution_simple_daily_expression() {
    let schedule = CronSchedule::new("0 9 * * 1-5").unwrap();
    // 2025-01-06 is a Monday; the following 08:59:30 instant schedules next 09:00:00 same day.
    let after = ts("2025-01-06 08:59:30");
    let next = schedule.next_execution(after).unwrap();
    assert_eq!(next, ts("2025-01-06 09:00:00"));
    // Iteration is monotonic: the next-next firing skips the weekend.
    let following = schedule.next_execution(next).unwrap();
    assert_eq!(following, ts("2025-01-07 09:00:00"));
}

#[test]
fn test_next_execution_skips_weekend() {
    let schedule = CronSchedule::new("0 9 * * 1-5").unwrap();
    // 2025-01-10 (Friday) 09:00 -> next firing is Monday 2025-01-13.
    let after = ts("2025-01-10 09:00:00");
    let next = schedule.next_execution(after).unwrap();
    assert_eq!(next, ts("2025-01-13 09:00:00"));
}

#[test]
fn test_next_execution_every_n_minutes() {
    let schedule = CronSchedule::new("*/15 * * * *").unwrap();
    let after = ts("2025-01-06 08:07:03");
    // :00, :15, :30, :45 -> 08:15.
    assert_eq!(
        schedule.next_execution(after).unwrap(),
        ts("2025-01-06 08:15:00")
    );
}

#[test]
fn test_next_execution_list_fields() {
    let schedule = CronSchedule::new("0 8,12,18 * * 1-5").unwrap();
    let after = ts("2025-01-06 12:00:01");
    assert_eq!(
        schedule.next_execution(after).unwrap(),
        ts("2025-01-06 18:00:00")
    );
}

#[test]
fn test_next_execution_range_step() {
    let schedule = CronSchedule::new("0 0 1-7/2 * *").unwrap();
    // Any 1..=7 odd day of the month at midnight.
    let after = ts("2025-01-06 00:00:01");
    assert_eq!(
        schedule.next_execution(after).unwrap(),
        ts("2025-01-07 00:00:00")
    );
}

#[test]
fn test_next_execution_dom_dow_or_semantics() {
    // Both day fields restricted: Vixie cron fires when EITHER matches.
    let schedule = CronSchedule::new("0 0 1 * 1").unwrap();
    // 2025-01-06 is Monday AND not the 1st; Monday matches -> fires.
    let after = ts("2025-01-05 12:00:00");
    assert_eq!(
        schedule.next_execution(after).unwrap(),
        ts("2025-01-06 00:00:00")
    );
}

#[test]
fn test_next_execution_question_mark() {
    let schedule = CronSchedule::new("0 9 ? * 1-5").unwrap();
    assert_eq!(
        schedule.next_execution(ts("2025-01-06 08:59:30")).unwrap(),
        ts("2025-01-06 09:00:00")
    );
}

#[test]
fn test_next_execution_vixie_dow_numbering() {
    // Vixie numbering: 0 = Sunday, 6 = Saturday, 7 = Sunday.
    let sunday = CronSchedule::new("0 9 * * 0").unwrap();
    assert_eq!(
        sunday.next_execution(ts("2025-01-05 09:00:00")).unwrap(), // Sunday 09:00
        ts("2025-01-12 09:00:00")
    );
    let saturday = CronSchedule::new("0 9 * * 6").unwrap();
    assert_eq!(
        saturday.next_execution(ts("2025-01-05 09:00:00")).unwrap(),
        ts("2025-01-11 09:00:00") // Saturday 2025-01-11
    );
    let seven_is_sunday = CronSchedule::new("0 9 * * 7").unwrap();
    assert_eq!(
        seven_is_sunday
            .next_execution(ts("2025-01-05 09:00:00"))
            .unwrap(),
        ts("2025-01-12 09:00:00")
    );
    let every_other_day = CronSchedule::new("0 9 * * 0-6/2").unwrap();
    // Fires Sun, Tue, Thu, Sat.
    assert_eq!(
        every_other_day
            .next_execution(ts("2025-01-05 09:00:00"))
            .unwrap(),
        ts("2025-01-07 09:00:00") // Tuesday
    );
}

#[test]
fn test_next_execution_strictly_after_reference() {
    let schedule = CronSchedule::new("0 * * * *").unwrap();
    let at = ts("2025-01-06 09:00:00");
    // Next hourly firing after exactly 09:00 is 10:00, never 09:00 itself.
    assert_eq!(
        schedule.next_execution(at).unwrap(),
        ts("2025-01-06 10:00:00")
    );
}

#[test]
fn test_next_execution_never_firing_expression() {
    // February 31st never exists: the iterator terminates.
    let schedule = CronSchedule::new("0 0 31 2 *").unwrap();
    assert!(schedule.next_execution(ts("2025-01-06 09:00:00")).is_none());
}

#[test]
fn test_validation_rejects_letters_and_junk() {
    // Letters / arbitrary tokens cannot be evaluated and must not register.
    assert!(CronSchedule::new("0 8 * * L").is_err());
    assert!(CronSchedule::new("abc * * * *").is_err());
    assert!(CronSchedule::new("@daily").is_err());
}

#[test]
fn test_validation_accepts_standard_grammar() {
    assert!(CronSchedule::new("* * * * *").is_ok());
    assert!(CronSchedule::new("0 8 * * 1-5").is_ok());
    assert!(CronSchedule::new("0 8,12,18 * * 1-5").is_ok());
    assert!(CronSchedule::new("*/5 * * * *").is_ok());
    assert!(CronSchedule::new("0 0 1-7/2 * *").is_ok());
    assert!(CronSchedule::new("30 7 ? * Mon-Fri").is_err()); // weekday names unsupported
}
