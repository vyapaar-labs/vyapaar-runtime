use chrono::{NaiveDate, NaiveTime};

use vyapaar_runtime_backtest::{NseTradingCalendar, SessionWindow};

#[test]
fn test_regular_weekday_is_trading_day() {
    let cal = NseTradingCalendar::new();
    // Wednesday, Jan 15, 2025
    let d = NaiveDate::from_ymd_opt(2025, 1, 15).unwrap();
    assert!(cal.is_trading_day(d));
    let session = cal.session_for_date(d).unwrap();
    assert_eq!(session, SessionWindow::regular());
}

#[test]
fn test_weekend_and_holidays_excluded() {
    let cal = NseTradingCalendar::new();
    // Saturday, Jan 18, 2025
    let sat = NaiveDate::from_ymd_opt(2025, 1, 18).unwrap();
    assert!(!cal.is_trading_day(sat));

    // Independence Day: Aug 15, 2025 (Friday)
    let holiday = NaiveDate::from_ymd_opt(2025, 8, 15).unwrap();
    assert!(cal.is_holiday(holiday));
    assert!(!cal.is_trading_day(holiday));

    // Next trading day after Thursday Aug 14 is Monday Aug 18
    let aug14 = NaiveDate::from_ymd_opt(2025, 8, 14).unwrap();
    let next = cal.next_trading_day(aug14);
    assert_eq!(next, NaiveDate::from_ymd_opt(2025, 8, 18).unwrap());
}

#[test]
fn test_muhurat_trading_session() {
    let cal = NseTradingCalendar::new();
    let diwali = NaiveDate::from_ymd_opt(2025, 10, 21).unwrap();
    assert!(cal.is_muhurat_session(diwali));
    assert!(cal.is_trading_day(diwali));

    let session = cal.session_for_date(diwali).unwrap();
    assert_eq!(session, SessionWindow::muhurat());
    assert_eq!(session.open, NaiveTime::from_hms_opt(18, 15, 0).unwrap());
    assert_eq!(session.close, NaiveTime::from_hms_opt(19, 15, 0).unwrap());
}
