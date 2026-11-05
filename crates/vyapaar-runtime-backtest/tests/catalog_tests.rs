use arrow::array::{Float64Array, Int64Array};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use parquet::arrow::ArrowWriter;
use std::fs;
use std::path::Path;
use std::sync::Arc;

use vyapaar_core::{Exchange, InstrumentId};
use vyapaar_runtime_backtest::CatalogLoader;

fn write_partition(path: &Path, opens: &[f64], highs: &[f64], lows: &[f64], closes: &[f64]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let schema = Arc::new(Schema::new(vec![
        Field::new("ts", DataType::Int64, false),
        Field::new("open", DataType::Float64, false),
        Field::new("high", DataType::Float64, false),
        Field::new("low", DataType::Float64, false),
        Field::new("close", DataType::Float64, false),
        Field::new("volume", DataType::Float64, false),
    ]));
    let ts: Vec<i64> = (0..opens.len())
        .map(|i| ((i as i64) + 1) * 60_000_000_000)
        .collect();
    let vols = vec![100.0; opens.len()];
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(Int64Array::from(ts)),
            Arc::new(Float64Array::from(opens.to_vec())),
            Arc::new(Float64Array::from(highs.to_vec())),
            Arc::new(Float64Array::from(lows.to_vec())),
            Arc::new(Float64Array::from(closes.to_vec())),
            Arc::new(Float64Array::from(vols)),
        ],
    )
    .unwrap();
    let mut writer = ArrowWriter::try_new(fs::File::create(path).unwrap(), schema, None).unwrap();
    writer.write(&batch).unwrap();
    writer.close().unwrap();
}

#[test]
fn inspect_and_verify_catalog_detect_valid_and_anomalous_ohlc() {
    let tmp = tempfile::tempdir().unwrap();
    let ok_file = tmp.path().join("catalog/1d/NSE/TCS/2025.parquet");
    write_partition(
        &ok_file,
        &[10.0, 11.0],
        &[12.0, 13.0],
        &[9.0, 10.0],
        &[11.0, 12.0],
    );

    let loader = CatalogLoader::new(tmp.path());
    let summary = loader.inspect_summary(None).unwrap();
    assert_eq!(summary.len(), 1);
    assert_eq!(summary[0].bar_count, 2);
    assert_eq!(
        summary[0].instrument,
        InstrumentId::new(Exchange::Nse, "TCS")
    );

    let report = loader.verify_integrity(None).unwrap();
    assert!(report.is_clean(), "{report:?}");
    assert_eq!(report.total_bars, 2);

    // Add an anomalous partition (low > high)
    let bad_file = tmp.path().join("catalog/1d/NSE/INFY/2025.parquet");
    write_partition(&bad_file, &[10.0], &[9.0], &[12.0], &[10.0]);
    let bad_report = loader.verify_integrity(Some("1d")).unwrap();
    assert!(!bad_report.is_clean());
    assert!(!bad_report.anomalies.is_empty());
}
