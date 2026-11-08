//! Tests for FileJournalWriter and recover_interrupted_runs (ADR 0017).

use std::fs;
use tempfile::TempDir;
use vyapaar_runtime_api::RunId;
use vyapaar_runtime_api::responses::RunStatus;
use vyapaar_runtime_api::run_resolve::ResolvedBacktest;
use vyapaar_runtime_api::runs::RunManifest;
use vyapaar_runtime_api_rest::{FileJournalWriter, recover_interrupted_runs};

fn sample_manifest(id_str: &str, status: RunStatus) -> RunManifest {
    let run_id = RunId::parse(id_str).unwrap();
    let tcs = serde_json::json!({"symbol": "TCS", "exchange": "NSE"});
    let s_manifest = serde_json::json!({
        "api_version": "1.0.0",
        "name": "sma_crossover",
        "source_hash": "abc",
        "universe": {"explicit": [tcs]},
        "subscriptions": {"instruments": [tcs]},
        "driving_timeframe": {"interval": 1, "aggregation": "day"},
        "warmup_bars": 20
    });
    let compiled = vyapaar_runtime_api::compile_strategy(
        &mut vyapaar_runtime_api::StrategyCatalog::default(),
        serde_json::from_value(s_manifest).unwrap(),
    )
    .unwrap();

    let req = vyapaar_runtime_api::ResolvedRequest::Backtest(ResolvedBacktest {
        strategy: "sma".into(),
        universe: "nifty50".into(),
        start: chrono::DateTime::from_timestamp_nanos(1_000_000_000),
        end: chrono::DateTime::from_timestamp_nanos(2_000_000_000),
        bar_spec: "1d".into(),
        initial_capital: rust_decimal_macros::dec!(1_000_000.0),
        seed: 42,
        fill_model: None,
        commission_per_fill: rust_decimal_macros::dec!(0.0),
        slippage_bps: rust_decimal_macros::dec!(0.0),
    });

    let mut manifest = RunManifest::new_pending(
        run_id,
        compiled.id,
        compiled.ir,
        req,
        "2026-10-08T00:00:00.000Z",
    );
    manifest.status = status;
    manifest
}

#[test]
fn file_journal_writer_lifecycle() {
    let temp = TempDir::new().unwrap();
    let base_dir = temp.path();
    let manifest = sample_manifest("01JBP8R3D5K87M4Q2N1W6E9TXV", RunStatus::Pending);

    let mut writer = FileJournalWriter::create(base_dir, &manifest).unwrap();
    assert!(writer.dir().exists());
    assert!(writer.dir().join("manifest.json").exists());
    assert!(writer.dir().join("events.ndjson").exists());

    let event1 = serde_json::json!({"seq": 1, "kind": "bar"});
    let event2 = serde_json::json!({"seq": 2, "kind": "signal"});
    writer.append_event(&event1).unwrap();
    writer.append_event(&event2).unwrap();

    let mut final_manifest = manifest.clone();
    final_manifest.status = RunStatus::Completed;
    let trades = serde_json::json!([{"id": "t1", "pnl": 500}]);
    let metrics = serde_json::json!({"net_pnl": 500, "sharpe": 1.5});

    writer.finalize(&final_manifest, &trades, &metrics).unwrap();

    assert!(writer.dir().join("trades.json").exists());
    assert!(writer.dir().join("metrics.json").exists());

    // Verify events.ndjson content
    let events_str = fs::read_to_string(writer.dir().join("events.ndjson")).unwrap();
    let lines: Vec<&str> = events_str.trim().split('\n').collect();
    assert_eq!(lines.len(), 2);

    // Verify manifest updated
    let loaded_manifest: RunManifest =
        serde_json::from_str(&fs::read_to_string(writer.dir().join("manifest.json")).unwrap())
            .unwrap();
    assert_eq!(loaded_manifest.status, RunStatus::Completed);
}

#[test]
fn recover_orphaned_runs_on_reboot() {
    let temp = TempDir::new().unwrap();
    let base_dir = temp.path();

    // 1. Pending run
    let m_pending = sample_manifest("01JBP8R3D5K87M4Q2N1W6E9TXA", RunStatus::Pending);
    FileJournalWriter::create(base_dir, &m_pending).unwrap();

    // 2. Running run
    let mut m_running = sample_manifest("01JBP8R3D5K87M4Q2N1W6E9TXB", RunStatus::Running);
    m_running.started_at = Some("2026-10-08T00:00:01.000Z".to_string());
    FileJournalWriter::create(base_dir, &m_running).unwrap();

    // 3. Completed run
    let mut m_completed = sample_manifest("01JBP8R3D5K87M4Q2N1W6E9TXC", RunStatus::Completed);
    m_completed.finished_at = Some("2026-10-08T00:00:02.000Z".to_string());
    FileJournalWriter::create(base_dir, &m_completed).unwrap();

    // 4. Failed run (already terminal)
    let mut m_failed = sample_manifest("01JBP8R3D5K87M4Q2N1W6E9TXD", RunStatus::Failed);
    m_failed.finished_at = Some("2026-10-08T00:00:02.000Z".to_string());
    FileJournalWriter::create(base_dir, &m_failed).unwrap();

    // Execute recovery
    let recovered = recover_interrupted_runs(base_dir).unwrap();
    assert_eq!(recovered, 2, "pending and running runs must be recovered");

    // Verify pending run was marked Failed
    let rec_pending: RunManifest = serde_json::from_str(
        &fs::read_to_string(
            base_dir
                .join(m_pending.run_id.to_string())
                .join("manifest.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(rec_pending.status, RunStatus::Failed);
    assert!(rec_pending.finished_at.is_some());
    assert_eq!(
        rec_pending.error.unwrap().context.unwrap(),
        serde_json::json!({"reason": "interrupted"})
    );

    // Verify running run was marked Failed
    let rec_running: RunManifest = serde_json::from_str(
        &fs::read_to_string(
            base_dir
                .join(m_running.run_id.to_string())
                .join("manifest.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(rec_running.status, RunStatus::Failed);
    assert!(rec_running.finished_at.is_some());

    // Verify completed run is untouched
    let rec_completed: RunManifest = serde_json::from_str(
        &fs::read_to_string(
            base_dir
                .join(m_completed.run_id.to_string())
                .join("manifest.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(rec_completed.status, RunStatus::Completed);

    // Verify idempotent second run
    let recovered_again = recover_interrupted_runs(base_dir).unwrap();
    assert_eq!(
        recovered_again, 0,
        "subsequent recovery must find 0 orphaned runs"
    );
}

#[test]
fn recover_missing_dir_returns_zero() {
    let temp = TempDir::new().unwrap();
    let nonexistent = temp.path().join("does_not_exist");
    assert_eq!(recover_interrupted_runs(&nonexistent).unwrap(), 0);
}
