//! Pure-Rust server bootstrap configuration and lifecycle runner for `vyapaar serve`.
//!
//! Market data is loaded once at start-up (a missing or unreadable directory fails before any
//! socket binds), then `vyapaar-ya-api-rest` serves it until graceful shutdown (`SIGINT` /
//! `SIGTERM` or an explicit shutdown future).
//!
//! With `bar_ingest = true`, `POST /bars` feeds a bounded queue drained by the bar pump (fills and
//! quotes follow). Shutdown drops the sender, waits for the pump to drain, and reports counts.
//!
//! With `fill_model = FillModel::L2` and `ticks = Some(...)`, REST backtests match against quote
//! and trade ticks from an ADR 0030 NDJSON file or Parquet tick store directory.

use std::future::Future;
use std::io::Write;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use vyapaar_core::TradingState;
use vyapaar_runtime_config::{ExecutionConfig, FillModel};
use vyapaar_runtime_feed::{InMemoryTickReader, ParquetTickStore};

use crate::{
    ApiConfig, AppState, BarPumpReport, JwtConfig, RetentionPolicy, RunServiceConfig,
    ShutdownReport,
};

/// Parses a `--trading-state` wire spelling (`active`, `reducing`, or `halted`) into
/// [`vyapaar_core::TradingState`] (ADR 0018 decision 2).
pub fn parse_trading_state(raw: &str) -> Result<TradingState, String> {
    match raw {
        "active" => Ok(TradingState::Active),
        "reducing" => Ok(TradingState::Paused { resume_at: None }),
        "halted" => Ok(TradingState::Halted {
            reason: "operator halt via `--trading-state halted`".to_string(),
        }),
        other => Err(format!(
            "unknown trading state {other:?}; expected active, reducing or halted"
        )),
    }
}

/// Parses an operator `trading-state set` spelling (case-insensitive) into
/// [`vyapaar_core::TradingState`].
pub fn parse_trading_state_command(raw: &str) -> Result<TradingState> {
    let new_state = match raw.to_lowercase().as_str() {
        "active" => TradingState::Active,
        "reducing" => TradingState::Paused { resume_at: None },
        "halted" => TradingState::Halted {
            reason: "operator halt via `vyapaar trading-state set`".to_string(),
        },
        other => {
            bail!("unknown trading state {other:?}; expected active, reducing or halted")
        }
    };
    Ok(new_state)
}

/// Maps [`vyapaar_core::TradingState`] onto the flat [`vyapaar_runtime_api::TradingState`] wire enum.
pub fn to_api_trading_state(state: TradingState) -> vyapaar_runtime_api::TradingState {
    match state {
        TradingState::Active => vyapaar_runtime_api::TradingState::Active,
        TradingState::Paused { .. } => vyapaar_runtime_api::TradingState::Reducing,
        TradingState::Halted { .. } => vyapaar_runtime_api::TradingState::Halted,
    }
}

/// Returns the stderr security warning for a non-loopback listen address, or `None` for loopback.
pub fn non_loopback_warning(addr: SocketAddr) -> Option<String> {
    if addr.ip().is_loopback() {
        return None;
    }
    Some(format!(
        "warning: listening on {addr}, which is not loopback; without --tokens-file or \
         --jwt-secret the API has no auth or TLS, so anyone who can reach it can read the data"
    ))
}

/// Parses a `--fill-model` wire spelling (`bar_fill`, `bar_fill_through`, or `l2`).
pub fn parse_fill_model(raw: &str) -> Result<FillModel, String> {
    match raw {
        "bar_fill" => Ok(FillModel::BarFill),
        "bar_fill_through" => Ok(FillModel::BarFillThrough),
        "l2" => Ok(FillModel::L2),
        other => Err(format!(
            "unknown fill model {other:?}; expected bar_fill, bar_fill_through or l2"
        )),
    }
}

/// Run storage, concurrency, and retention tunables (ADR 0017 decisions 3–5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServeRunsConfig {
    /// Directory holding one sub-directory per run (`manifest.json` + `events.ndjson`).
    pub journals_dir: PathBuf,
    /// Directory holding one JSON profile per user (`/profile` routes); created on first write.
    pub profiles_dir: PathBuf,
    /// Directory holding one JSON portfolio per user (`/portfolio` routes); created on first write.
    pub portfolios_dir: PathBuf,
    /// Optional NDJSON file each `PUT /trading-state` change is appended to (ADR 0018 decision 7).
    pub operator_journal: Option<PathBuf>,
    /// Worker threads, one backtest each (`None` = available parallelism; must be `>= 1` if `Some`).
    pub max_concurrent_runs: Option<u64>,
    /// Maximum pending runs in the queue before `POST /backtests` answers 429 (must be `>= 1`).
    pub max_queued_runs: u64,
    /// Seconds a stopping server waits for running runs before cancelling them.
    pub shutdown_grace_secs: u64,
    /// Finished runs that always survive start-up retention.
    pub keep_runs: u64,
    /// Finished runs younger than this many days survive start-up retention.
    pub keep_days: u64,
}

impl Default for ServeRunsConfig {
    fn default() -> Self {
        Self {
            journals_dir: PathBuf::from("data/journals"),
            profiles_dir: PathBuf::from("data/profiles"),
            portfolios_dir: PathBuf::from("data/portfolios"),
            operator_journal: None,
            max_concurrent_runs: None,
            max_queued_runs: 64,
            shutdown_grace_secs: 10,
            keep_runs: 1_000,
            keep_days: 30,
        }
    }
}

impl ServeRunsConfig {
    /// Validates non-zero pool and queue bounds.
    pub fn validate(&self) -> Result<()> {
        if self.max_concurrent_runs == Some(0) {
            bail!("--max-concurrent-runs must be >= 1");
        }
        if self.max_queued_runs == 0 {
            bail!("--max-queued-runs must be >= 1");
        }
        Ok(())
    }

    /// Converts these flags into a [`RunServiceConfig`].
    pub fn service_config(&self) -> RunServiceConfig {
        let defaults = RunServiceConfig::default();
        RunServiceConfig {
            max_concurrent: self
                .max_concurrent_runs
                .map_or(defaults.max_concurrent, clamp_usize),
            max_queued: clamp_usize(self.max_queued_runs),
            shutdown_grace: Duration::from_secs(self.shutdown_grace_secs),
            retention: RetentionPolicy {
                keep_runs: clamp_usize(self.keep_runs),
                keep_days: self.keep_days,
            },
        }
    }
}

/// Authentication configuration for `vyapaar serve` (ADR 0023, E11-S8, Phase 6.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServeAuthConfig {
    /// Path to `tokens.toml`. When set, Bearer token auth is enabled and hot-reloaded.
    pub tokens_file: Option<PathBuf>,
    /// Path to NDJSON auth audit log file (defaults to `{journals_dir}/audit.log` when `tokens_file` is set).
    pub audit_log_file: Option<PathBuf>,
    /// HMAC secret for HS256 JWT bearer token verification.
    pub jwt_secret: Option<String>,
    /// Path to PEM-encoded RSA public key for RS256 JWT verification.
    pub jwt_rsa_key: Option<PathBuf>,
    /// Expected JWT `iss` claim (default `"honba"`).
    pub jwt_issuer: String,
    /// Expected JWT `aud` claim (default `"vyapaar-api"`).
    pub jwt_audience: String,
}

impl Default for ServeAuthConfig {
    fn default() -> Self {
        Self {
            tokens_file: None,
            audit_log_file: None,
            jwt_secret: None,
            jwt_rsa_key: None,
            jwt_issuer: "honba".to_string(),
            jwt_audience: "vyapaar-api".to_string(),
        }
    }
}

impl ServeAuthConfig {
    /// Returns `true` when either token-file or JWT bearer auth is configured.
    pub fn is_enabled(&self) -> bool {
        self.tokens_file.is_some() || self.jwt_secret.is_some() || self.jwt_rsa_key.is_some()
    }
}

/// Live bar feed configuration behind `POST /bars`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServeIngestConfig {
    /// Accept bars on `POST /bars` and feed them to the simulated market.
    pub bar_ingest: bool,
    /// Maximum queued bars before `POST /bars` answers 429 (must be `>= 1`).
    pub bar_ingest_capacity: u64,
}

impl Default for ServeIngestConfig {
    fn default() -> Self {
        Self {
            bar_ingest: false,
            bar_ingest_capacity: 1_024,
        }
    }
}

impl ServeIngestConfig {
    /// Validates queue capacity bounds.
    pub fn validate(&self) -> Result<()> {
        if self.bar_ingest_capacity == 0 {
            bail!("--bar-ingest-capacity must be >= 1");
        }
        Ok(())
    }
}

/// Pre-trade risk limit configuration for write routes (ADR 0018 decision 8).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServeRiskConfig {
    /// Optional TOML file containing a `[risk]` table.
    pub risk_config: Option<PathBuf>,
}

/// Fill model and tick source configuration for REST backtests (ADR 0030).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServeFillConfig {
    /// Fill model (`BarFill`, `BarFillThrough`, or `L2`). Defaults to `BarFill`.
    pub fill_model: Option<FillModel>,
    /// NDJSON ticks file or Parquet tick store directory required when `fill_model == Some(FillModel::L2)`.
    pub ticks: Option<PathBuf>,
}

/// Loaded tick source behind [`ServeFillConfig::ticks`].
pub enum TickSource {
    /// Versioned NDJSON ticks file loaded into memory.
    Ndjson(InMemoryTickReader),
    /// Persistent Parquet tick store directory.
    Parquet(ParquetTickStore),
}

impl ServeFillConfig {
    /// Resolves execution config and validates/opens the tick source.
    pub fn resolve(&self) -> Result<(ExecutionConfig, Option<TickSource>)> {
        let fill_model = self.fill_model.unwrap_or_default();
        if fill_model == FillModel::L2 && self.ticks.is_none() {
            bail!(
                "--fill-model l2 needs --ticks <FILE|DIR> (NDJSON or a Parquet tick store, see ADR 0030)"
            );
        }
        let ticks = match self.ticks.as_deref() {
            Some(path) if path.is_dir() => Some(TickSource::Parquet(
                ParquetTickStore::open(path)
                    .with_context(|| format!("reading --ticks store {}", path.display()))?,
            )),
            Some(path) => Some(TickSource::Ndjson(crate::ticks_file::load_ticks_ndjson(
                path,
            )?)),
            None => None,
        };
        let execution = ExecutionConfig {
            fill_model,
            ..ExecutionConfig::default()
        };
        Ok((execution, ticks))
    }
}

/// Top-level pure-Rust configuration for launching the `vyapaar-ya-api-rest` server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServeConfig {
    /// Market data directory (either a `catalog/` tree or flat `SYMBOL.EXCHANGE.parquet` files).
    pub data_dir: PathBuf,
    /// Socket address to bind (default `127.0.0.1:8000`; `127.0.0.1:0` binds an ephemeral port).
    pub addr: SocketAddr,
    /// Explicit CORS origins allowed to read the API (empty = CORS disabled).
    pub cors_origins: Vec<String>,
    /// Initial operator trading state enforced by write routes.
    pub trading_state: TradingState,
    /// Run store, worker pool, and retention settings.
    pub runs: ServeRunsConfig,
    /// Authentication settings.
    pub auth: ServeAuthConfig,
    /// Live bar ingest settings.
    pub ingest: ServeIngestConfig,
    /// Pre-trade risk limit file settings.
    pub risk: ServeRiskConfig,
    /// Backtest fill model and tick store settings.
    pub fill: ServeFillConfig,
}

impl ServeConfig {
    /// Creates a [`ServeConfig`] rooted at `data_dir` with canonical defaults (`127.0.0.1:8000`,
    /// `TradingState::Active`, CORS off).
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
            addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8000),
            cors_origins: Vec::new(),
            trading_state: TradingState::Active,
            runs: ServeRunsConfig::default(),
            auth: ServeAuthConfig::default(),
            ingest: ServeIngestConfig::default(),
            risk: ServeRiskConfig::default(),
            fill: ServeFillConfig::default(),
        }
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RiskFile {
    #[serde(default)]
    risk: vyapaar_risk::RiskLimits,
}

/// Reads and validates a `--risk-config` TOML file. Errors carry the path and name the field.
pub fn load_risk_limits(path: &Path) -> Result<vyapaar_risk::RiskLimits> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading --risk-config {}", path.display()))?;
    let file: RiskFile = toml::from_str(&text)
        .with_context(|| format!("parsing --risk-config {}", path.display()))?;
    anyhow::ensure!(
        file.risk.is_valid(),
        "invalid [risk] in --risk-config {}: check max_notional_per_order, order_rate_limit, \
         price_collar_bps, and freeze_limits",
        path.display()
    );
    Ok(file.risk)
}

fn clamp_usize(n: u64) -> usize {
    usize::try_from(n).unwrap_or(usize::MAX)
}

/// Summary returned when a bootstrapped server finishes shutting down.
#[derive(Debug, Clone)]
pub struct ServeOutcome {
    /// Actual bound socket address.
    pub bound_addr: SocketAddr,
    /// Non-loopback warning emitted when unauthenticated on a non-loopback address.
    pub non_loopback_warning: Option<String>,
    /// Final bar-ingest drain report when `bar_ingest` was enabled.
    pub bar_ingest: Option<BarPumpReport>,
    /// Final run-service graceful shutdown report.
    pub runs_shutdown: Option<ShutdownReport>,
}

/// Validated, pre-loaded server state ready to bind and serve.
pub struct PreparedServer {
    addr: SocketAddr,
    auth_enabled: bool,
    bar_ingest: bool,
    bar_ingest_capacity: usize,
    api_config: ApiConfig,
    state: AppState,
}

/// Validates `config` and loads market data, risk limits, ticks, and run stores before any
/// network socket is bound.
pub fn prepare_server(config: &ServeConfig) -> Result<PreparedServer> {
    config.runs.validate()?;
    config.ingest.validate()?;
    let (execution, ticks) = config.fill.resolve()?;
    let risk_limits = match &config.risk.risk_config {
        Some(path) => load_risk_limits(path)?,
        None => vyapaar_risk::RiskLimits::default(),
    };
    let mut api_config = ApiConfig::default()
        .with_cors_origins(&config.cors_origins)
        .map_err(|e| anyhow::anyhow!(e))
        .context("invalid --cors-origin (cors)")?;

    if let Some(tokens_file) = &config.auth.tokens_file {
        api_config.auth.tokens_file = Some(tokens_file.clone());
        if let Some(audit_log_file) = &config.auth.audit_log_file {
            api_config.auth.audit_log_path = Some(audit_log_file.clone());
        } else {
            api_config.auth.audit_log_path = Some(config.runs.journals_dir.join("audit.log"));
        }
    }

    if config.auth.jwt_secret.is_some() || config.auth.jwt_rsa_key.is_some() {
        let rsa_pem = if let Some(path) = &config.auth.jwt_rsa_key {
            Some(
                std::fs::read_to_string(path)
                    .with_context(|| format!("reading RSA key {}", path.display()))?,
            )
        } else {
            None
        };
        api_config.auth.jwt_config = Some(JwtConfig {
            hmac_secret: config
                .auth
                .jwt_secret
                .as_ref()
                .map(|s| s.as_bytes().to_vec()),
            rsa_public_key: rsa_pem,
            issuer: config.auth.jwt_issuer.clone(),
            audience: config.auth.jwt_audience.clone(),
        });
    }

    let state = if AppState::has_catalog(&config.data_dir) {
        AppState::from_catalog_dir(&config.data_dir).map_err(anyhow::Error::from)
    } else {
        AppState::from_parquet_dir(&config.data_dir).map_err(anyhow::Error::from)
    };
    let mut state = state
        .with_context(|| format!("loading data directory {}", config.data_dir.display()))?
        .with_trading_state(to_api_trading_state(config.trading_state.clone()))
        .with_risk_limits(risk_limits)
        .with_execution(execution);

    if let Some(ticks) = ticks {
        let reader: Arc<dyn vyapaar_runtime_feed::ports::TickReader> = match ticks {
            TickSource::Ndjson(reader) => Arc::new(reader),
            TickSource::Parquet(store) => Arc::new(store),
        };
        state = state.with_tick_reader(reader);
    }

    let mut state = state
        .with_profiles_dir(&config.runs.profiles_dir)
        .with_portfolios_dir(&config.runs.portfolios_dir);
    if let Some(journal) = &config.runs.operator_journal {
        state = state.with_operator_journal(journal);
    }
    let mut state = state
        .with_journals_dir(&config.runs.journals_dir, config.runs.service_config())
        .map_err(|e| anyhow::anyhow!("{}", e.message))
        .with_context(|| {
            format!(
                "opening journals directory {}",
                config.runs.journals_dir.display()
            )
        })?;

    // `GET /ws/chat` only comes up when a local GGUF model (`VYAPAAR_GGUF_PATH`) or an
    // OpenAI-compatible endpoint (`VYAPAAR_LLM_BASE_URL`) is configured; otherwise `AppState.chat`
    // stays `None` and the route answers a clear "not configured" error instead of panicking.
    if let Some(chat) = crate::chat::ChatRuntime::from_env(
        state.instruments.clone(),
        state.bars.clone(),
        state.quotes.clone(),
        state.risk_engine.clone(),
    )
    .context("configuring chat agent")?
    {
        state = state.with_chat_agent(chat);
    }

    Ok(PreparedServer {
        addr: config.addr,
        auth_enabled: config.auth.is_enabled(),
        bar_ingest: config.ingest.bar_ingest,
        bar_ingest_capacity: clamp_usize(config.ingest.bar_ingest_capacity),
        api_config,
        state,
    })
}

impl PreparedServer {
    /// Binds the configured address inside an active Tokio runtime, invokes `on_bound` with the
    /// bound [`SocketAddr`] and optional security warning, serves until `shutdown` resolves, and
    /// drains both the bar pump and run service cleanly.
    pub async fn serve_with_shutdown<F, S>(self, on_bound: F, shutdown: S) -> Result<ServeOutcome>
    where
        F: FnOnce(SocketAddr, Option<&str>) + Send,
        S: Future<Output = ()> + Send + 'static,
    {
        let Self {
            addr,
            auth_enabled,
            bar_ingest,
            bar_ingest_capacity,
            api_config,
            state,
        } = self;

        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .with_context(|| format!("binding {addr}"))?;
        let bound_addr = listener.local_addr()?;
        let warning = if !auth_enabled {
            non_loopback_warning(addr)
        } else {
            None
        };
        let pump = bar_ingest.then(|| state.start_bar_ingest(bar_ingest_capacity));
        on_bound(bound_addr, warning.as_deref());

        let served = crate::serve_with_config(listener, state.clone(), &api_config, shutdown)
            .await
            .context("serving");

        state.stop_bar_ingest();
        let bar_report = match pump {
            Some(pump) => Some(pump.await.context("bar ingest pump task panicked")?),
            None => None,
        };

        let runs_shutdown = tokio::task::spawn_blocking(move || state.shutdown_runs())
            .await
            .context("joining run-service shutdown")?;

        served?;
        Ok(ServeOutcome {
            bound_addr,
            non_loopback_warning: warning,
            bar_ingest: bar_report,
            runs_shutdown,
        })
    }
}

/// Runs `vyapaar serve` synchronously to completion: validates `config`, starts a multi-threaded
/// Tokio runtime, prints `listening on http://<addr>` to stdout, and shuts down cleanly on
/// `SIGINT` (`Ctrl-C`) or `SIGTERM`.
pub fn run_server_blocking(config: &ServeConfig) -> Result<()> {
    let prepared = prepare_server(config)?;
    let PreparedServer {
        addr,
        auth_enabled,
        bar_ingest,
        bar_ingest_capacity,
        api_config,
        state,
    } = prepared;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("starting the async runtime")?;

    let served = runtime.block_on(async {
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .with_context(|| format!("binding {addr}"))?;
        if !auth_enabled && let Some(warning) = non_loopback_warning(addr) {
            eprintln!("{warning}");
        }
        let pump = bar_ingest.then(|| state.start_bar_ingest(bar_ingest_capacity));
        println!("listening on http://{}", listener.local_addr()?);
        std::io::stdout().flush()?;
        let served = crate::serve_with_config(listener, state.clone(), &api_config, stop_signal())
            .await
            .context("serving");
        state.stop_bar_ingest();
        if let Some(pump) = pump {
            match pump.await {
                Ok(report) => {
                    eprintln!("bar ingest: {} bars, {} errors", report.bars, report.errors)
                }
                Err(e) => eprintln!("bar ingest: pump task failed: {e}"),
            }
        }
        served
    });

    if let Some(report) = state.shutdown_runs()
        && report.cancelled_pending + report.cancelled_running > 0
    {
        eprintln!(
            "cancelled {} pending and {} running runs",
            report.cancelled_pending, report.cancelled_running
        );
    }
    served
}

/// Resolves on `Ctrl-C` (`SIGINT`) or, on Unix, `SIGTERM`.
async fn stop_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = term.recv() => {}
            },
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
