//! Wire-transport tests for `ZerodhaKiteAdapter` (GAP-0003).
//!
//! Verifies that when Kite Connect credentials are configured the adapter
//! delegates `submit_order` / `cancel_order` to the real `vyapaar-exec-zerodha`
//! wire transport, and that broker errors normalize into `DispatchError`.
//!
//! The mock Kite Connect server is a blocking std HTTP/1.1 server so these
//! tests stay synchronous: the adapter bridges the async executor via
//! `tokio::runtime::Runtime::block_on`, which must not run inside a tokio
//! context.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::thread;

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use vyapaar_core::{Exchange, InstrumentId, Quantity};
use vyapaar_risk::RiskEngine;
use vyapaar_runtime_strategy::{
    BrokerAdapter, DispatchError, Dispatcher, OrderIntent, ZerodhaAdapter,
};

fn fixed_now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-01-15T09:15:00Z")
        .expect("valid fixed timestamp")
        .with_timezone(&Utc)
}

#[derive(Clone)]
struct MockResponse {
    status: u16,
    body: String,
}

struct MockState {
    routes: HashMap<String, MockResponse>,
    default: MockResponse,
}

/// Spawn a blocking mock Kite Connect HTTP/1.1 server on a random port and
/// return that port. Each connection is read once, routed by request path,
/// answered, and closed.
fn spawn_mock(state: Arc<Mutex<MockState>>) -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("mock server binds");
    let port = listener.local_addr().expect("mock local addr").port();
    let state = state.clone();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.expect("mock accepts connection");
            let state = state.clone();
            thread::spawn(move || {
                let mut buf = [0u8; 8192];
                let _ = stream.read(&mut buf);
                let req = String::from_utf8_lossy(&buf[..]);
                let MockResponse { status, body } = {
                    let st = state.lock().expect("mock state not poisoned");
                    let first_line = req.lines().next().unwrap_or("");
                    let mut parts = first_line.split_whitespace();
                    let _method = parts.next();
                    let path = parts.next().unwrap_or("/");
                    st.routes
                        .get(path)
                        .cloned()
                        .unwrap_or_else(|| st.default.clone())
                };
                let resp = format!(
                    "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(resp.as_bytes());
            });
        }
    });
    port
}

fn success_state() -> Arc<Mutex<MockState>> {
    Arc::new(Mutex::new(MockState {
        routes: HashMap::from([(
            "/orders/regular".to_string(),
            MockResponse {
                status: 200,
                body: r#"{"status":"success","data":{"order_id":"ZERODHA_ORD_1001"}}"#.to_string(),
            },
        )]),
        default: MockResponse {
            status: 200,
            body: r#"{"status":"success","data":{"order_id":"ZERODHA_ORD_DEFAULT"}}"#.to_string(),
        },
    }))
}

#[test]
fn test_zerodha_wire_submit_order_delegates_to_kite_transport() {
    let state = success_state();
    let port = spawn_mock(state.clone());

    let adapter = ZerodhaAdapter::with_base_url(
        "test_api_key",
        "test_access_token",
        format!("http://127.0.0.1:{port}"),
        Decimal::from(1_000_000),
    );
    assert_eq!(adapter.broker_name(), "zerodha");

    let nse = Exchange::Nse;
    let infy = InstrumentId::new(nse, "INFY");
    let intent = OrderIntent::market_buy(infy, Quantity::from_u64(10));

    let mut dispatcher = Dispatcher::new(RiskEngine::default(), Box::new(adapter));
    let order_id = dispatcher
        .dispatch_intent_with_id(
            &intent,
            vyapaar_core::uuid::Uuid::from_u128(0x000C_10EF_0001),
            fixed_now(),
        )
        .expect("wire submit_order should succeed");

    assert_eq!(order_id, "ZERODHA_ORD_1001");
}

#[test]
fn test_zerodha_wire_auth_error_maps_to_session_expired() {
    let state = Arc::new(Mutex::new(MockState {
        routes: HashMap::from([(
            "/orders/regular".to_string(),
            MockResponse {
                status: 401,
                body: r#"{"status":"error","error_type":"TokenException","message":"invalid api key"}"#
                    .to_string(),
            },
        )]),
        default: MockResponse {
            status: 500,
            body: r#"{"status":"error","message":"internal"}"#.to_string(),
        },
    }));
    let port = spawn_mock(state.clone());

    let adapter = ZerodhaAdapter::with_base_url(
        "test_api_key",
        "test_access_token",
        format!("http://127.0.0.1:{port}"),
        Decimal::from(1_000_000),
    );
    let nse = Exchange::Nse;
    let infy = InstrumentId::new(nse, "INFY");
    let intent = OrderIntent::market_buy(infy, Quantity::from_u64(10));

    let mut dispatcher = Dispatcher::new(RiskEngine::default(), Box::new(adapter));
    let err = dispatcher
        .dispatch_intent_with_id(
            &intent,
            vyapaar_core::uuid::Uuid::from_u128(0x000C_10EF_0002),
            fixed_now(),
        )
        .expect_err("wire auth failure must be rejected");

    match err {
        DispatchError::BrokerRejection(msg) => {
            assert!(msg.contains("session expired"), "unexpected message: {msg}");
        }
        other => panic!("expected BrokerRejection(session expired), got {other:?}"),
    }
}

#[test]
fn test_zerodha_wire_rate_limit_error_maps_to_rate_limited() {
    let state = Arc::new(Mutex::new(MockState {
        routes: HashMap::from([(
            "/orders/regular".to_string(),
            MockResponse {
                status: 429,
                body: r#"{"status":"error","error_type":"TooManyRequests","message":"rate limit exceeded"}"#
                    .to_string(),
            },
        )]),
        default: MockResponse {
            status: 500,
            body: r#"{"status":"error","message":"internal"}"#.to_string(),
        },
    }));
    let port = spawn_mock(state.clone());

    let adapter = ZerodhaAdapter::with_base_url(
        "test_api_key",
        "test_access_token",
        format!("http://127.0.0.1:{port}"),
        Decimal::from(1_000_000),
    );
    let nse = Exchange::Nse;
    let infy = InstrumentId::new(nse, "INFY");
    let intent = OrderIntent::market_buy(infy, Quantity::from_u64(10));

    let mut dispatcher = Dispatcher::new(RiskEngine::default(), Box::new(adapter));
    let err = dispatcher
        .dispatch_intent_with_id(
            &intent,
            vyapaar_core::uuid::Uuid::from_u128(0x000C_10EF_0003),
            fixed_now(),
        )
        .expect_err("wire rate limit must be rejected");

    match err {
        DispatchError::BrokerRejection(msg) => {
            assert!(msg.contains("rate limited"), "unexpected message: {msg}");
        }
        other => panic!("expected BrokerRejection(rate limited), got {other:?}"),
    }
}

#[test]
fn test_zerodha_wire_network_error_maps_to_network() {
    // Point at a port with no listener: connection refused -> transport error.
    let adapter = ZerodhaAdapter::with_base_url(
        "test_api_key",
        "test_access_token",
        "http://127.0.0.1:19999",
        Decimal::from(1_000_000),
    );
    let nse = Exchange::Nse;
    let infy = InstrumentId::new(nse, "INFY");
    let intent = OrderIntent::market_buy(infy, Quantity::from_u64(10));

    let mut dispatcher = Dispatcher::new(RiskEngine::default(), Box::new(adapter));
    let err = dispatcher
        .dispatch_intent_with_id(
            &intent,
            vyapaar_core::uuid::Uuid::from_u128(0x000C_10EF_0004),
            fixed_now(),
        )
        .expect_err("wire network failure must be rejected");

    match err {
        DispatchError::BrokerRejection(msg) => {
            assert!(msg.contains("network error"), "unexpected message: {msg}");
        }
        other => panic!("expected BrokerRejection(network), got {other:?}"),
    }
}

#[test]
fn test_zerodha_wire_cancel_order_delegates_to_kite_transport() {
    let state = Arc::new(Mutex::new(MockState {
        routes: HashMap::from([
            (
                "/orders/regular".to_string(),
                MockResponse {
                    status: 200,
                    body: r#"{"status":"success","data":{"order_id":"ZERODHA_ORD_2001"}}"#
                        .to_string(),
                },
            ),
            (
                "/orders/regular/ZERODHA_ORD_2001".to_string(),
                MockResponse {
                    status: 200,
                    body: "{}".to_string(),
                },
            ),
        ]),
        default: MockResponse {
            status: 500,
            body: r#"{"status":"error","message":"internal"}"#.to_string(),
        },
    }));
    let port = spawn_mock(state.clone());

    let adapter = ZerodhaAdapter::with_base_url(
        "test_api_key",
        "test_access_token",
        format!("http://127.0.0.1:{port}"),
        Decimal::from(1_000_000),
    );
    let nse = Exchange::Nse;
    let infy = InstrumentId::new(nse, "INFY");
    let intent = OrderIntent::market_buy(infy, Quantity::from_u64(10));

    let mut dispatcher = Dispatcher::new(RiskEngine::default(), Box::new(adapter));
    let order_id = dispatcher
        .dispatch_intent_with_id(
            &intent,
            vyapaar_core::uuid::Uuid::from_u128(0x000C_10EF_0005),
            fixed_now(),
        )
        .expect("place order");

    dispatcher
        .cancel_order(&order_id, fixed_now())
        .expect("wire cancel_order should succeed");
    assert_eq!(order_id, "ZERODHA_ORD_2001");
}
