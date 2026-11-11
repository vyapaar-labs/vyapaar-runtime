//! `/market-data/*` reference-data route tests (Phase 3).

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::Value;
use tower::util::ServiceExt;
use vyapaar_runtime_api_rest::api_router;

async fn call(method: &str, uri: &str) -> (StatusCode, Value) {
    let res = api_router()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn symbol_profile() {
    let (status, body) = call("GET", "/api/v1/market-data/symbol-profile/RELIANCE.NSE").await;
    assert_eq!(status, StatusCode::OK);
    let p = &body["data"];
    assert_eq!(p["symbol"], "NSE:RELIANCE");
    assert_eq!(p["name"], "Reliance Industries Ltd.");
    assert_eq!(p["sector"], "Energy");
    assert_eq!(p["market_cap"], "1800000000000.00");
    assert_eq!(p["is_actively_trading"], true);
}

#[tokio::test]
async fn symbol_profile_unknown() {
    let (status, _body) = call("GET", "/api/v1/market-data/symbol-profile/NOPE.NSE").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn calendar() {
    let (status, body) = call("GET", "/api/v1/market-data/calendar").await;
    assert_eq!(status, StatusCode::OK);
    let events = &body["data"]["events"];
    assert_eq!(events.as_array().unwrap().len(), 2);
    assert_eq!(events[0]["category"], "Earnings");
    assert_eq!(
        events[0]["event_name"],
        "Reliance Industries Q3 FY27 Earnings"
    );
    assert_eq!(events[0]["impact_level"], "High");
}

#[tokio::test]
async fn calendar_category_filter() {
    let (status, body) = call("GET", "/api/v1/market-data/calendar?category=Economic").await;
    assert_eq!(status, StatusCode::OK);
    let events = &body["data"]["events"];
    assert_eq!(events.as_array().unwrap().len(), 1);
    assert_eq!(events[0]["category"], "Economic");
}

#[tokio::test]
async fn routes_are_not_flagged_not_implemented() {
    let (_status, body) = call("GET", "/api/v1/capabilities").await;
    let flagged: Vec<String> = body["data"]["capabilities"]["not_implemented"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    for route in [
        "GET /market-data/symbol-profile/:id",
        "GET /market-data/calendar",
    ] {
        assert!(
            !flagged.contains(&route.to_string()),
            "{route} is flagged not-implemented"
        );
    }
}
