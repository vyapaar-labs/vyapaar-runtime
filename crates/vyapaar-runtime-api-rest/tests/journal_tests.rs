//! Tests for FileJournalWriter and recover_interrupted_runs (ADR 0017).

use std::fs;
use tempfile::TempDir;
use vyapaar_runtime_api::JournalWriter;
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

/// Generic manifests that do not carry a typed [`RunManifest`] (e.g. CLI job runs,
/// PLAN-0001 Stage 4) are recovered through the raw-JSON fallback path.
#[test]
fn recover_generic_job_manifest_roundtrip() {
    let temp = TempDir::new().unwrap();
    let base_dir = temp.path();

    // A job run manifest: status running, no StrategyIr / ResolvedRequest.
    let run_id = RunId::parse("01JBP8R3D5K87M4Q2N1W6E9TXE").unwrap();
    let job_manifest = serde_json::json!({
        "manifest_version": 1,
        "run_id": run_id.as_str(),
        "kind": "job",
        "job_id": "0f1e2d3c4b5a69788796a5b4c3d2e1f0",
        "job_name": "morning scan",
        "goal": "Scan momentum breaks",
        "trigger": "manual",
        "status": "running",
        "started_at": "2026-11-09T09:30:00Z",
        "created_at": "2026-11-09T09:29:59Z"
    });
    let mut writer =
        FileJournalWriter::create_from_value(base_dir, &run_id, &job_manifest).unwrap();
    writer
        .append_event(&serde_json::json!({"seq": 1, "type": "run.started"}))
        .unwrap();

    // A terminal job manifest that must NOT be touched.
    let done_id = RunId::parse("01JBP8R3D5K87M4Q2N1W6E9TXF").unwrap();
    let done_manifest = serde_json::json!({
        "run_id": done_id.as_str(),
        "kind": "job",
        "status": "completed",
        "created_at": "2026-11-09T09:29:59Z",
        "started_at": "2026-11-09T09:30:00Z",
        "finished_at": "2026-11-09T09:30:02Z"
    });
    FileJournalWriter::create_from_value(base_dir, &done_id, &done_manifest).unwrap();

    let recovered = recover_interrupted_runs(base_dir).unwrap();
    assert_eq!(recovered, 1, "the running job manifest must be recovered");

    let patched: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(base_dir.join(run_id.as_str()).join("manifest.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(patched["status"], "failed");
    assert_eq!(patched["error"]["code"], "internal_error");
    assert!(patched["finished_at"].as_str().is_some());
    // The untouched original job fields survive the patch.
    assert_eq!(patched["job_name"], "morning scan");
    assert_eq!(patched["manifest_version"], 1);

    let done: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(base_dir.join(done_id.as_str()).join("manifest.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        done["status"], "completed",
        "terminal manifests must be untouched"
    );

    // Idempotent second sweep.
    assert_eq!(recover_interrupted_runs(base_dir).unwrap(), 0);
}

/// `create_from_value` writes the manifest verbatim and streams `events.ndjson`.
#[test]
fn create_from_value_writes_generic_manifest() {
    let temp = TempDir::new().unwrap();
    let base_dir = temp.path();
    let run_id = RunId::parse("01JBP8R3D5K87M4Q2N1W6E9TXH").unwrap();
    let manifest = serde_json::json!({
        "run_id": run_id.as_str(),
        "kind": "tool",
        "tool": "run_backtest",
        "status": "pending"
    });
    let mut writer = FileJournalWriter::create_from_value(base_dir, &run_id, &manifest).unwrap();
    writer
        .append_event(&serde_json::json!({"seq": 1, "type": "run.started"}))
        .unwrap();
    writer
        .append_event(&serde_json::json!({"seq": 2, "type": "run.finished"}))
        .unwrap();
    writer.flush().unwrap();

    let saved: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(base_dir.join(run_id.as_str()).join("manifest.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(saved, manifest);
    assert_eq!(writer.dir(), base_dir.join(run_id.as_str()));

    let events = fs::read_to_string(base_dir.join(run_id.as_str()).join("events.ndjson")).unwrap();
    let lines: Vec<&str> = events.lines().collect();
    assert_eq!(lines.len(), 2);
    assert!(lines[0].contains("\"run.started\""));
    assert!(lines[1].contains("\"run.finished\""));
}

/// `write_manifest` swaps the status snapshot without disturbing the event stream.
#[test]
fn write_manifest_overwrites_snapshot() {
    let temp = TempDir::new().unwrap();
    let base_dir = temp.path();
    let run_id = RunId::parse("01JBP8R3D5K87M4Q2N1W6E9TXJ").unwrap();
    let pending = serde_json::json!({
        "run_id": run_id.as_str(),
        "kind": "job",
        "status": "pending"
    });
    let mut writer = FileJournalWriter::create_from_value(base_dir, &run_id, &pending).unwrap();
    writer
        .append_event(&serde_json::json!({"seq": 1, "type": "run.queued"}))
        .unwrap();

    let running = serde_json::json!({
        "run_id": run_id.as_str(),
        "kind": "job",
        "status": "running",
        "started_at": "2026-11-09T09:30:00Z"
    });
    writer.write_manifest(&running).unwrap();

    let saved: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(base_dir.join(run_id.as_str()).join("manifest.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(saved, running);

    // The event stream survived the manifest swap.
    writer
        .append_event(&serde_json::json!({"seq": 2, "type": "run.started"}))
        .unwrap();
    writer.flush().unwrap();
    let events = fs::read_to_string(base_dir.join(run_id.as_str()).join("events.ndjson")).unwrap();
    assert_eq!(events.lines().count(), 2);
}

/// `finalize_value` flushes events and writes manifest/trades/metrics for a job run
/// that does not carry a typed [`RunManifest`].
#[test]
fn finalize_value_writes_terminal_files() {
    let temp = TempDir::new().unwrap();
    let base_dir = temp.path();
    let run_id = RunId::parse("01JBP8R3D5K87M4Q2N1W6E9TXK").unwrap();
    let manifest = serde_json::json!({
        "run_id": run_id.as_str(),
        "kind": "job",
        "status": "completed"
    });
    let mut writer = FileJournalWriter::create_from_value(base_dir, &run_id, &manifest).unwrap();
    writer
        .append_event(&serde_json::json!({"seq": 1, "type": "run.queued"}))
        .unwrap();
    writer
        .append_event(&serde_json::json!({"seq": 3, "type": "run.result"}))
        .unwrap();

    let trades = serde_json::json!({"total_trades": 4});
    let metrics = serde_json::json!({"sharpe_ratio": "1.23", "win_rate_pct": "60"});
    writer.finalize_value(&manifest, &trades, &metrics).unwrap();

    let dir = base_dir.join(run_id.as_str());
    let saved_manifest: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(dir.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(saved_manifest, manifest);
    let saved_trades: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(dir.join("trades.json")).unwrap()).unwrap();
    assert_eq!(saved_trades, trades);
    let saved_metrics: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(dir.join("metrics.json")).unwrap()).unwrap();
    assert_eq!(saved_metrics, metrics);

    let events = fs::read_to_string(dir.join("events.ndjson")).unwrap();
    assert_eq!(
        events.lines().count(),
        2,
        "events must be flushed by finalize_value"
    );
}
