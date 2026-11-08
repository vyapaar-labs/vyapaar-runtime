//! The NDJSON implementation of the run journal port (`vyapaar_runtime_api::JournalWriter`) (ADR 0017 decisions 3 and 8).

use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use vyapaar_runtime_api::JournalWriter;
use vyapaar_runtime_api::responses::RunStatus;
use vyapaar_runtime_api::runs::RunManifest;
use vyapaar_runtime_api::{ErrorCode, ErrorDetail, Message};

/// `events.ndjson`: one wire [`Message`] per line, in append order.
pub struct NdjsonJournal<W: Write + Send = File> {
    writer: BufWriter<W>,
    failure: Option<ErrorDetail>,
}

impl<W: Write + Send> NdjsonJournal<W> {
    /// Wraps any writer (a file in production, memory in unit tests).
    pub fn new(writer: W) -> Self {
        Self {
            writer: BufWriter::new(writer),
            failure: None,
        }
    }
}

impl NdjsonJournal<File> {
    /// Opens `path` for appending, creating it when missing.
    pub fn open(path: &Path) -> Result<Self, ErrorDetail> {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|e| write_failure(&e))?;
        Ok(Self::new(file))
    }
}

impl<W: Write + Send> NdjsonJournal<W> {
    fn check(&self) -> Result<(), ErrorDetail> {
        self.failure.clone().map_or(Ok(()), Err)
    }

    /// Records the first I/O failure so every later call reports it.
    fn keep(&mut self, result: std::io::Result<()>) -> Result<(), ErrorDetail> {
        result.map_err(|e| {
            let detail = write_failure(&e);
            self.failure = Some(detail.clone());
            detail
        })
    }
}

/// The error for a failed journal write. Names the I/O kind, never a path.
fn write_failure(e: &std::io::Error) -> ErrorDetail {
    ErrorDetail::new(
        ErrorCode::InternalError,
        format!("journal write failed: {}", e.kind()),
    )
    .with_context(serde_json::json!({"reason": "journal_write"}))
}

impl<W: Write + Send> JournalWriter for NdjsonJournal<W> {
    fn append(&mut self, msg: &Message) -> Result<(), ErrorDetail> {
        self.check()?;
        // Serialise first so a failure cannot leave half a record in the buffer.
        let mut line = serde_json::to_vec(msg).map_err(|e| {
            ErrorDetail::new(
                ErrorCode::InternalError,
                format!("journal encode failed: {e}"),
            )
            .with_context(serde_json::json!({"reason": "journal_write"}))
        })?;
        line.push(b'\n');
        let result = self.writer.write_all(&line);
        self.keep(result)
    }

    fn flush(&mut self) -> Result<(), ErrorDetail> {
        self.check()?;
        let result = self.writer.flush();
        self.keep(result)
    }
}

/// Synchronous filesystem journal writer managing the per-run directory layout
/// (`manifest.json`, `events.ndjson`, `trades.json`, `metrics.json`).
pub struct FileJournalWriter {
    dir: PathBuf,
    events_writer: BufWriter<File>,
}

impl FileJournalWriter {
    /// Creates a new run directory under `base_dir` using `manifest.run_id`, writes the
    /// initial `manifest.json`, and opens `events.ndjson` for append streaming.
    pub fn create(base_dir: &Path, manifest: &RunManifest) -> std::io::Result<Self> {
        let dir = base_dir.join(manifest.run_id.to_string());
        fs::create_dir_all(&dir)?;

        // Write initial manifest.json
        let manifest_path = dir.join("manifest.json");
        let manifest_json = serde_json::to_string_pretty(manifest)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        fs::write(&manifest_path, manifest_json)?;

        // Open events.ndjson for streaming
        let events_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("events.ndjson"))?;

        Ok(Self {
            dir,
            events_writer: BufWriter::new(events_file),
        })
    }

    /// Appends one event to `events.ndjson` as a single newline-terminated JSON line.
    pub fn append_event(&mut self, event: &serde_json::Value) -> std::io::Result<()> {
        serde_json::to_writer(&mut self.events_writer, event)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        self.events_writer.write_all(b"\n")?;
        Ok(())
    }

    /// Flushes buffered events and writes final `manifest.json`, `trades.json`, and `metrics.json`.
    pub fn finalize(
        &mut self,
        manifest: &RunManifest,
        trades: &serde_json::Value,
        metrics: &serde_json::Value,
    ) -> std::io::Result<()> {
        self.events_writer.flush()?;
        let manifest_json = serde_json::to_string_pretty(manifest)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let trades_json = serde_json::to_string_pretty(trades)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let metrics_json = serde_json::to_string_pretty(metrics)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

        fs::write(self.dir.join("manifest.json"), manifest_json)?;
        fs::write(self.dir.join("trades.json"), trades_json)?;
        fs::write(self.dir.join("metrics.json"), metrics_json)?;
        Ok(())
    }

    /// Returns the directory path for this run.
    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

impl JournalWriter for FileJournalWriter {
    fn append(&mut self, msg: &Message) -> Result<(), ErrorDetail> {
        let val = serde_json::to_value(msg).map_err(|e| {
            ErrorDetail::new(
                ErrorCode::InternalError,
                format!("journal encode failed: {e}"),
            )
            .with_context(serde_json::json!({"reason": "journal_write"}))
        })?;
        self.append_event(&val).map_err(|e| {
            ErrorDetail::new(
                ErrorCode::InternalError,
                format!("journal write failed: {}", e.kind()),
            )
            .with_context(serde_json::json!({"reason": "journal_write"}))
        })
    }

    fn flush(&mut self) -> Result<(), ErrorDetail> {
        self.events_writer.flush().map_err(|e| {
            ErrorDetail::new(
                ErrorCode::InternalError,
                format!("journal flush failed: {}", e.kind()),
            )
            .with_context(serde_json::json!({"reason": "journal_write"}))
        })
    }
}

/// Scans `journals_dir` for interrupted (Pending or Running) runs and transitions them to Failed.
/// Sets `finished_at` and an `ErrorDetail` noting the unexpected interruption.
/// Returns the number of recovered runs.
pub fn recover_interrupted_runs(journals_dir: &Path) -> std::io::Result<usize> {
    let mut recovered = 0;
    if !journals_dir.exists() {
        return Ok(0);
    }
    for entry in std::fs::read_dir(journals_dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let manifest_file = entry.path().join("manifest.json");
        if manifest_file.exists() {
            let file = match File::open(&manifest_file) {
                Ok(f) => f,
                Err(_) => continue,
            };

            let updated: Option<serde_json::Value> =
                match serde_json::from_reader::<_, RunManifest>(file) {
                    Ok(mut manifest) => {
                        if manifest.status == RunStatus::Running
                            || manifest.status == RunStatus::Pending
                        {
                            manifest.status = RunStatus::Failed;
                            manifest.error = Some(
                                ErrorDetail::new(
                                    ErrorCode::InternalError,
                                    "Interrupted: process terminated unexpectedly",
                                )
                                .with_context(serde_json::json!({"reason": "interrupted"})),
                            );
                            manifest.finished_at = Some(chrono::Utc::now().to_rfc3339());
                            serde_json::to_value(&manifest).ok()
                        } else {
                            None
                        }
                    }
                    Err(_) => {
                        if let Ok(bytes) = std::fs::read(&manifest_file) {
                            if let Ok(mut value) =
                                serde_json::from_slice::<serde_json::Value>(&bytes)
                            {
                                let status = value.get("status").and_then(|s| s.as_str());
                                if status == Some("running") || status == Some("pending") {
                                    value["status"] = serde_json::json!("failed");
                                    value["error"] = serde_json::json!({
                                        "code": "internal_error",
                                        "message": "Interrupted: process terminated unexpectedly",
                                        "retryable": false,
                                        "context": {"reason": "interrupted"}
                                    });
                                    value["finished_at"] =
                                        serde_json::json!(chrono::Utc::now().to_rfc3339());
                                    Some(value)
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    }
                };

            if let Some(val) = updated {
                let tmp = entry.path().join("manifest.json.tmp");
                let mut out_file = File::create(&tmp)?;
                let serialized = serde_json::to_string_pretty(&val)
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
                out_file.write_all(serialized.as_bytes())?;
                out_file.flush()?;
                out_file.sync_all()?;
                std::fs::rename(&tmp, &manifest_file)?;
                recovered += 1;
            }
        }
    }
    Ok(recovered)
}
