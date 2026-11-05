//! Integration tests for the /ws and /ws/market WebSocket endpoints.

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use vyapaar_runtime_api_rest::{AppState, serve};

async fn ws_handshake(addr: std::net::SocketAddr, path: &str) -> u16 {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let request = format!(
        "GET {path} HTTP/1.1\r\n\
         Host: {addr}\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
         Sec-WebSocket-Version: 13\r\n\r\n"
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut buf = [0u8; 1024];
    let n = stream.read(&mut buf).await.unwrap();
    let response = String::from_utf8_lossy(&buf[..n]);
    let status_line = response.lines().next().expect("response status line");
    status_line
        .split_whitespace()
        .nth(1)
        .expect("status code")
        .parse()
        .expect("numeric status")
}

#[tokio::test]
async fn ws_endpoint_responds_with_101_switching_protocols() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (stop, stopped) = oneshot::channel::<()>();
    let server = tokio::spawn(serve(listener, AppState::default(), async move {
        let _ = stopped.await;
    }));

    let status = ws_handshake(addr, "/ws").await;
    assert_eq!(status, 101);

    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn ws_market_endpoint_responds_with_101_switching_protocols() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (stop, stopped) = oneshot::channel::<()>();
    let server = tokio::spawn(serve(listener, AppState::default(), async move {
        let _ = stopped.await;
    }));

    let status = ws_handshake(addr, "/ws/market").await;
    assert_eq!(status, 101);

    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn ws_streams_ticks_to_client() {
    use futures::StreamExt;
    use std::time::Duration;
    use tokio::sync::broadcast;
    use vyapaar_core::{Currency, Exchange, InstrumentId, InstrumentKind, Price, Quantity, Tick};
    use vyapaar_runtime_api_rest::ws::WsTick;
    use vyapaar_runtime_feed::wire::{
        BarAggregation, BarSpecification, Instrument, PriceType, UnixNanos,
    };
    use vyapaar_runtime_feed::{ColumnarSliceBuilder, Dataset, DatasetReader};

    let id = InstrumentId::new(Exchange::Nse, "RELIANCE");
    let spec = BarSpecification::new(1, BarAggregation::Minute, PriceType::Last);
    let mut builder = ColumnarSliceBuilder::new(id.clone(), spec);
    builder
        .push(
            UnixNanos::from_u64(1),
            2500.0,
            2500.0,
            2500.0,
            2500.0,
            100.0,
        )
        .unwrap();
    let dataset = Dataset::from_slices(vec![builder.finish().unwrap()]).unwrap();
    let instrument = Instrument::new(id.clone(), InstrumentKind::Equity, Currency::Inr, 1.0, 0.05);
    // The gateway streams live ticks from the market-data broadcast channel, so the test
    // attaches one and publishes through it after connecting.
    let (ticks, _) = broadcast::channel::<Tick>(16);
    let state = AppState::from_reader(DatasetReader::new(dataset, vec![instrument]))
        .with_market_data_broadcast(ticks.clone());

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (stop, stopped) = oneshot::channel::<()>();
    let server = tokio::spawn(serve(listener, state, async move {
        let _ = stopped.await;
    }));

    let ws_url = format!("ws://{addr}/ws/market");
    let (mut ws_stream, _) = tokio_tungstenite::connect_async(&ws_url).await.unwrap();

    // An empty subscription set means "all instruments" in the handler, so publishing one
    // tick is enough for the client to receive it.
    let now = chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap();
    ticks
        .send(Tick::Ltp {
            instrument: id,
            price: Price::new(vyapaar_core::rust_decimal::Decimal::from(2500)),
            volume: Quantity::new(vyapaar_core::rust_decimal::Decimal::from(100)),
            exchange_ts: now,
            receive_ts: now,
        })
        .unwrap();

    let msg = tokio::time::timeout(Duration::from_secs(5), ws_stream.next())
        .await
        .expect("the server should stream a published tick")
        .unwrap()
        .unwrap();
    let text = msg.to_text().unwrap();
    let tick: WsTick = serde_json::from_str(text).unwrap();

    assert_eq!(tick.symbol, "RELIANCE");
    assert!((tick.price - 2500.0).abs() < f64::EPSILON);

    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
}
