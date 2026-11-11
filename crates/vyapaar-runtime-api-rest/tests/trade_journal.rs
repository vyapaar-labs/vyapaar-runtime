//! `/trade-journal/*` route tests.

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde_json::Value;
use tower::util::ServiceExt;
use vyapaar_ledger_performance::model::{
    AccountProvenance, ClosedTrade, JournalAccountId, TradeAnnotation,
};
use vyapaar_ledger_performance::store::{JournalStore, StoreError};
use vyapaar_runtime_api_rest::{AppState, api_router_with};

fn build_router() -> Router {
    let mut store = JournalStore::new();
    let acc_id = JournalAccountId::new("acc-journal-1");
    store
        .create_account(
            acc_id.clone(),
            "Journal Account 1",
            AccountProvenance::Manual,
            vyapaar_core::Currency::Inr,
        )
        .unwrap();

    // A couple of closed trades with annotations.
    let now = NaiveDate::from_ymd_opt(2026, 3, 15).unwrap();
    let entry = vyapaar_core::Price::try_new("2500.00".parse::<Decimal>().unwrap()).unwrap();
    let exit = vyapaar_core::Price::try_new("2600.00".parse::<Decimal>().unwrap()).unwrap();
    let qty = vyapaar_core::Quantity::from_u64(10);
    let trade = ClosedTrade {
        trade_id: "T-1".to_string(),
        instrument: vyapaar_core::InstrumentId::new(vyapaar_core::Exchange::Nse, "RELIANCE"),
        side: vyapaar_core::PositionSide::Long,
        quantity: qty,
        entry_price: entry,
        exit_price: exit,
        realized_pnl: vyapaar_core::Money::inr(Decimal::from(1000)),
        fees: vyapaar_core::Money::inr(Decimal::from(20)),
        holding_period_days: 45,
        is_long_term: false,
        exit_date: now,
        duration_ms: None,
        annotation: TradeAnnotation::new()
            .with_tags(vec!["breakout".into()])
            .with_rating(4)
            .with_planned_risk(vyapaar_core::Money::inr(Decimal::from(500))),
    };
    store.set_closed_trade(trade);

    let state = AppState::default().with_journal_store(store);
    api_router_with(state)
}

async fn call(router: &Router, method: &str, uri: &str, body: Option<&str>) -> (StatusCode, Value) {
    let request = if let Some(b) = body {
        Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(b.to_string()))
            .unwrap()
    } else {
        Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::empty())
            .unwrap()
    };
    let res = router.clone().oneshot(request).await.unwrap();
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn json_body(value: &Value) -> String {
    serde_json::to_string(value).unwrap()
}

#[tokio::test]
async fn trade_journal_requires_configured_store() {
    let router = api_router_with(AppState::default());
    let res = router
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/v1/trade-journal/accounts")
                .header("content-type", "application/json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn list_accounts() {
    let router = build_router();
    let (status, body) = call(&router, "GET", "/api/v1/trade-journal/accounts", None).await;
    assert_eq!(status, StatusCode::OK);
    let accounts = &body["data"]["accounts"];
    assert_eq!(accounts.as_array().unwrap().len(), 1);
    assert_eq!(accounts[0]["id"], "acc-journal-1");
    assert_eq!(accounts[0]["display_name"], "Journal Account 1");
}

#[tokio::test]
async fn create_account() {
    let router = build_router();
    let (status, body) = call(
        &router,
        "POST",
        "/api/v1/trade-journal/accounts",
        Some(&json_body(&serde_json::json!({
            "display_name": "New Account",
            "provenance": "Imported",
            "currency": "INR",
            "starting_balance": "50000.00"
        }))),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["display_name"], "New Account");
    assert_eq!(body["provenance"], "Imported");
    assert_eq!(body["starting_balance"], "50000.00");

    // Listing now shows two accounts.
    let (status, body) = call(&router, "GET", "/api/v1/trade-journal/accounts", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["accounts"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn create_account_bad_provenance() {
    let router = build_router();
    let (status, _body) = call(
        &router,
        "POST",
        "/api/v1/trade-journal/accounts",
        Some(&json_body(&serde_json::json!({
            "display_name": "X",
            "provenance": "NotAProvenance",
            "currency": "INR"
        }))),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn list_trades() {
    let router = build_router();
    let (status, body) = call(&router, "GET", "/api/v1/trade-journal/trades", None).await;
    assert_eq!(status, StatusCode::OK);
    let trades = &body["data"]["trades"];
    assert_eq!(trades.as_array().unwrap().len(), 1);
    assert_eq!(trades[0]["trade_id"], "T-1");
    assert_eq!(trades[0]["instrument"], "NSE:RELIANCE");
    assert_eq!(trades[0]["annotation"]["rating"], 4);
    assert_eq!(trades[0]["annotation"]["tags"][0], "breakout");
    assert_eq!(trades[0]["annotation"]["planned_risk"]["amount"], "500");
    // net_pnl = 1000 realized - 20 fees = 980 (Decimal trims trailing zeros)
    assert_eq!(trades[0]["net_pnl"], "980");
}

#[tokio::test]
async fn patch_trade_annotation() {
    let router = build_router();
    let (status, body) = call(
        &router,
        "PATCH",
        "/api/v1/trade-journal/trades/T-1",
        Some(&json_body(&serde_json::json!({
            "rating": 5,
            "review": "great setup",
            "planned_risk": "600.00"
        }))),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["rating"], 5);
    assert_eq!(body["data"]["review"], "great setup");
    assert_eq!(body["data"]["planned_risk"]["amount"], "600.00");
}

#[tokio::test]
async fn overview() {
    let router = build_router();
    let (status, body) = call(&router, "GET", "/api/v1/trade-journal/overview", None).await;
    assert_eq!(status, StatusCode::OK);
    let metrics = &body["data"]["metrics"];
    assert_eq!(metrics["total_trades"], 1);
    assert_eq!(metrics["wins"], 1);
    assert_eq!(metrics["net_pnl"], "980");
    assert_eq!(metrics["trades_with_risk"], 1);
    let edge = &body["data"]["edge_score"];
    assert_eq!(edge["version"], 1);
    // With a single trade the score is None (needs >= 5).
    assert!(edge["score"].is_null());
}

#[tokio::test]
async fn calendar() {
    let router = build_router();
    let (status, body) = call(
        &router,
        "GET",
        "/api/v1/trade-journal/calendar?month=2026-03",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let cal = &body["data"]["calendar"];
    assert_eq!(cal["year"], 2026);
    assert_eq!(cal["month"], 3);
    let days: Vec<_> = cal["weeks"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|w| w["days"].as_array().unwrap().iter())
        .collect();
    // March 15 should have a trade.
    let mar15 = days.iter().find(|d| d["date"] == "2026-03-15").unwrap();
    assert_eq!(mar15["trades"], 1);
    assert_eq!(mar15["net_pnl"], "980");
}

#[tokio::test]
async fn notes() {
    let router = build_router();
    // POST a note.
    let (status, _body) = call(
        &router,
        "POST",
        "/api/v1/trade-journal/notes",
        Some(&json_body(&serde_json::json!({
            "account_id": "acc-journal-1",
            "day": "2026-03-15",
            "text": "Nice setup"
        }))),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    // GET the notes.
    let (status, body) = call(
        &router,
        "GET",
        "/api/v1/trade-journal/notes?account=acc-journal-1",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let notes = &body["data"]["notes"];
    assert_eq!(notes.as_array().unwrap().len(), 1);
    assert_eq!(notes[0]["text"], "Nice setup");
    assert_eq!(notes[0]["day"], "2026-03-15");
}

#[tokio::test]
async fn compare() {
    let router = build_router();
    let (status, body) = call(
        &router,
        "GET",
        "/api/v1/trade-journal/compare?accountA=acc-journal-1&accountB=acc-journal-1",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(!body["data"]["is_prior_period"].as_bool().unwrap());
    assert_eq!(body["data"]["account_a"]["account_id"], "acc-journal-1");
    assert_eq!(body["data"]["account_b"]["account_id"], "acc-journal-1");
}

#[tokio::test]
async fn routes_are_not_flagged_not_implemented() {
    let router = api_router_with(AppState::default());
    let (_status, body) = call(&router, "GET", "/api/v1/capabilities", None).await;
    let flagged: Vec<String> = body["data"]["capabilities"]["not_implemented"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    for route in [
        "GET /trade-journal/accounts",
        "POST /trade-journal/accounts",
        "GET /trade-journal/overview",
        "GET /trade-journal/trades",
        "PATCH /trade-journal/trades/:id",
        "GET /trade-journal/calendar",
        "GET /trade-journal/notes",
        "POST /trade-journal/notes",
        "GET /trade-journal/compare",
    ] {
        assert!(
            !flagged.contains(&route.to_string()),
            "{route} is flagged not-implemented"
        );
    }
}

#[test]
fn store_test_helpers() {
    // Exercise the store helpers used by the handlers directly.
    let mut store = JournalStore::new();
    assert_eq!(store.closed_trade_count(), 0);
    assert!(
        store
            .create_account(
                JournalAccountId::new("a"),
                "A",
                AccountProvenance::Manual,
                vyapaar_core::Currency::Inr,
            )
            .is_ok()
    );
    assert_eq!(
        store
            .create_account(
                JournalAccountId::new("a"),
                "B",
                AccountProvenance::Manual,
                vyapaar_core::Currency::Inr,
            )
            .unwrap_err(),
        StoreError::AccountAlreadyExists(JournalAccountId::new("a"))
    );
    assert!(
        store
            .set_note(&JournalAccountId::new("ghost"), now(), "x")
            .is_err()
    );
    assert_eq!(store.account_count(), 1);
}

fn now() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 3, 15).unwrap()
}
