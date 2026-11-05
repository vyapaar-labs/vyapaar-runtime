use arrow::array::{ArrayRef, Float64Array, Int64Array};
use arrow::record_batch::RecordBatch;
use chrono::{DateTime, Utc};
use parquet::arrow::ArrowWriter;
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use uuid::Uuid;

use vyapaar_core::{Exchange, FillReport, InstrumentId, OrderSide, Price, Quantity};
use vyapaar_runtime_api_rest::ticks_file::{
    equity_curve, load_parquet_bars_summary, pair_fills, parse_ticks_ndjson,
};
use vyapaar_runtime_feed::import::parquet_source::bar_schema;
use vyapaar_runtime_feed::ports::{TickReader, TickRequest};
use vyapaar_runtime_feed::wire::{AggressorSide, UnixNanos};

const HEAD: &str = "{\"honba_ticks\":1}\n";
const QUOTE: &str = "{\"kind\":\"quote\",\"symbol\":\"TCS\",\"exchange\":\"NSE\",\"ts_event\":200,\"bid_price\":99.9,\"ask_price\":100.1,\"bid_size\":500,\"ask_size\":400}\n";
const TRADE: &str = "{\"kind\":\"trade\",\"symbol\":\"TCS\",\"exchange\":\"NSE\",\"ts_event\":100,\"ts_init\":150,\"price\":100.0,\"size\":50,\"aggressor\":\"buyer\",\"trade_id\":\"T-1\"}\n";

fn tcs() -> InstrumentId {
    InstrumentId::new(Exchange::Nse, "TCS")
}

fn err(text: &str) -> String {
    format!(
        "{:#}",
        parse_ticks_ndjson(text).expect_err("expected an error")
    )
}

#[tokio::test]
async fn parses_quotes_and_trades_and_defaults_ts_init() {
    let reader = parse_ticks_ndjson(&format!("{HEAD}\n{QUOTE}{TRADE}")).unwrap();
    let req = TickRequest::new(tcs(), None, None).unwrap();
    let quotes = reader.read_quotes(&req).await.unwrap();
    let trades = reader.read_trades(&req).await.unwrap();
    assert_eq!(quotes.len(), 1);
    assert_eq!(quotes[0].bid_price(), 99.9);
    assert_eq!(quotes[0].ts_init(), UnixNanos::from_u64(200));
    assert_eq!(trades.len(), 1);
    assert_eq!(trades[0].ts_init(), UnixNanos::from_u64(150));
    assert_eq!(trades[0].aggressor_side(), AggressorSide::Buyer);
}

#[test]
fn rejects_a_missing_or_wrong_header_with_the_line() {
    assert!(err("").contains("empty"));
    assert!(err(QUOTE).contains("line 1"));
    let e = err("{\"honba_ticks\":2}\n");
    assert!(e.contains("line 1") && e.contains("version 2"), "{e}");
    assert!(err(HEAD).contains("no ticks"));
}

#[test]
fn a_bad_line_names_its_line_number() {
    let e = err(&format!("{HEAD}{QUOTE}not json\n"));
    assert!(e.contains("line 3"), "{e}");
    let e = err(&format!("{HEAD}\n{QUOTE}{{\"kind\":\"depth\"}}\n"));
    assert!(e.contains("line 4"), "{e}");
}

#[test]
fn unknown_fields_and_invariant_violations_are_refused() {
    let extra = QUOTE.replace("\"ts_event\":200", "\"ts_event\":200,\"bogus\":1");
    assert!(err(&format!("{HEAD}{extra}")).contains("line 2"));
    let crossed = QUOTE.replace("\"bid_price\":99.9", "\"bid_price\":101.0");
    let e = err(&format!("{HEAD}{crossed}"));
    assert!(e.contains("line 2") && e.contains("invalid quote"), "{e}");
    let neg = TRADE.replace("\"size\":50", "\"size\":-1");
    assert!(err(&format!("{HEAD}{neg}")).contains("invalid trade"));
}

#[test]
fn load_rejects_non_parquet_sources_before_touching_the_file_system() {
    let err = load_parquet_bars_summary("does/not/exist.csv", "TCS", None).unwrap_err();
    assert!(
        err.to_string().contains("unsupported source extension"),
        "{err}"
    );
}

#[test]
fn load_names_the_file_when_it_cannot_be_read() {
    let err = load_parquet_bars_summary("does/not/exist.parquet", "TCS", None).unwrap_err();
    assert!(err.to_string().contains("does/not/exist.parquet"), "{err}");
}

#[test]
fn load_accepts_an_upper_case_parquet_extension() {
    let err = load_parquet_bars_summary("does/not/exist.PARQUET", "TCS", None).unwrap_err();
    assert!(
        !err.to_string().contains("unsupported source extension"),
        "{err}"
    );
}

#[test]
fn load_rejects_a_conflicting_qualifier_before_touching_the_file_system() {
    let err =
        load_parquet_bars_summary("does/not/exist.parquet", "NSE:TCS", Some("BSE")).unwrap_err();
    assert!(err.to_string().contains("conflicts"), "{err}");
}

#[test]
fn load_rejects_an_unknown_exchange() {
    let err = load_parquet_bars_summary("does/not/exist.parquet", "TCS", Some("NYSE")).unwrap_err();
    assert!(err.to_string().contains("NSE, BSE"), "{err}");
}

fn write_bars_parquet(path: &Path) {
    let schema = Arc::new(bar_schema());
    let col = |v: f64| -> ArrayRef { Arc::new(Float64Array::from(vec![v; 2])) };
    let columns: Vec<ArrayRef> = vec![
        Arc::new(Int64Array::from(vec![60_000_000_000_i64, 120_000_000_000])),
        col(10.0),
        col(12.0),
        col(9.0),
        col(11.0),
        col(100.0),
    ];
    let batch = RecordBatch::try_new(schema.clone(), columns).unwrap();
    let mut writer = ArrowWriter::try_new(fs::File::create(path).unwrap(), schema, None).unwrap();
    writer.write(&batch).unwrap();
    writer.close().unwrap();
}

#[test]
fn load_summarises_parquet_file_and_honours_exchange_qualifier() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("TCS.NSE.parquet");
    write_bars_parquet(&file);

    let summary = load_parquet_bars_summary(file.to_str().unwrap(), "TCS", None).unwrap();
    assert_eq!(summary.qualified_instrument, "NSE:TCS");
    assert_eq!(summary.bar_count, 2);
    assert_eq!(summary.first.unwrap().1, 11.0);

    let bse = load_parquet_bars_summary(file.to_str().unwrap(), "TCS", Some("bse")).unwrap();
    assert_eq!(bse.qualified_instrument, "BSE:TCS");

    let qualified = load_parquet_bars_summary(file.to_str().unwrap(), "BSE:TCS", None).unwrap();
    assert_eq!(qualified.qualified_instrument, "BSE:TCS");
}

fn any_instrument() -> InstrumentId {
    InstrumentId::new(Exchange::Nse, "X")
}

fn ts(n: u64) -> DateTime<Utc> {
    DateTime::from_timestamp(0, n as u32).expect("fixture timestamp in range")
}

fn fill(side: OrderSide, price: f64, n: u64) -> FillReport {
    FillReport::new(
        format!("F-{n}"),
        format!("O-{n}"),
        Uuid::new_v4(),
        any_instrument(),
        side,
        Price::new(Decimal::from_f64_retain(price).expect("fixture price is finite")),
        Quantity::from_u64(1),
        "test-algo",
        ts(n),
        "default",
    )
}

#[test]
fn pair_fills_pairs_consecutive_entry_exit_fills() {
    let fills = [
        fill(OrderSide::Buy, 100.0, 1),
        fill(OrderSide::Sell, 105.0, 2),
        fill(OrderSide::Sell, 110.0, 3),
        fill(OrderSide::Buy, 104.0, 4),
    ];
    let trips = pair_fills(&fills);
    let pnl: Vec<f64> = trips.iter().map(|t| t.net_pnl.to_f64().unwrap()).collect();
    assert_eq!(pnl, [5.0, 6.0]);
}

#[test]
fn pair_fills_skips_same_side_pairs_and_a_trailing_fill() {
    let fills = [
        fill(OrderSide::Buy, 100.0, 1),
        fill(OrderSide::Buy, 101.0, 2),
        fill(OrderSide::Buy, 102.0, 3),
    ];
    assert!(pair_fills(&fills).is_empty());
    assert!(pair_fills(&[]).is_empty());
}

#[test]
fn equity_curve_accumulates_net_pnl_from_the_start() {
    let trips = pair_fills(&[
        fill(OrderSide::Buy, 100.0, 1),
        fill(OrderSide::Sell, 90.0, 2),
        fill(OrderSide::Buy, 100.0, 3),
        fill(OrderSide::Sell, 130.0, 4),
    ]);
    assert_eq!(equity_curve(1_000.0, &trips), [1_000.0, 990.0, 1_020.0]);
    assert_eq!(equity_curve(1_000.0, &[]), [1_000.0]);
}
