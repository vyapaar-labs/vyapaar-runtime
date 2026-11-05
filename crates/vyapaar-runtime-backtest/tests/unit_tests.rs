use chrono::NaiveDate;

use vyapaar_runtime_backtest::NseTradingCalendar;

#[test]
fn nse_calendar_marks_weekdays_and_holidays() {
    let cal = NseTradingCalendar::new();
    // Wednesday, Jan 15, 2025 is a regular trading day.
    let weekday = NaiveDate::from_ymd_opt(2025, 1, 15).unwrap();
    assert!(cal.is_trading_day(weekday));
    // Saturday, Jan 18, 2025 is not.
    let saturday = NaiveDate::from_ymd_opt(2025, 1, 18).unwrap();
    assert!(!cal.is_trading_day(saturday));
}
