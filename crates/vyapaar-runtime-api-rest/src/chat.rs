//! Local-LLM-backed chat WebSocket bridge (`GET /ws/chat`).
//!
//! Bridges the `vyapaar-agent-chat` conversational engine (session/turn/event model) to this
//! server over a WebSocket, backed by `vyapaar-agent-llm`'s local Candle-GGUF provider (or an
//! OpenAI/Ollama-compatible fallback) and the read-only tool ports in
//! `vyapaar_runtime_api::chat_adapters`.
//!
//! `GET /ws/chat?workspace=<id>` scopes the session to that workspace ([`ChatQuery`]) — connecting
//! again with the same `workspace` resumes its most recent session rather than starting a new
//! one (`SessionSelector::Continue`). Client -> server messages are `{"text": "..."}`; everything
//! the agent emits for the turn streams back as the chat engine's own `SessionEvent` JSON, one
//! WebSocket text frame per event.
//!
//! v1 scope (see `docs/gaps/0006-chat-agent-session-persistence-and-portfolio-tools.md`):
//! - Sessions are in-memory only (`InMemorySessionStore`); a server restart loses them.
//! - Portfolio-backed tools are stubbed via [`StubPortfolioReader`]: `AppState` holds no
//!   ledger-repository handle to build `LedgerPortfolioReader::from_combined` from, so every
//!   portfolio-backed tool call returns `PortError::unavailable` rather than a real position or
//!   P&L (deliberately an explicit error, not an empty success, so the model can't mistake "not
//!   wired up" for "this account has zero positions").
//!
//! Market/screener tools read through live data via [`crate::chat_market::FeedMarketReader`]/
//! [`crate::chat_market::FeedSnapshotSource`]; a few of their fields still approximate or report
//! unavailable (see `docs/gaps/0007-chat-market-reader-approximations-and-missing-sources.md`).
//! The price-alert tools write the caller's `Profile.alerts` document through the profiles
//! store ([`crate::chat_alerts::ProfileAlertStore`], agent plan 0006 Phase 2). The news/filings
//! content tools (agent plan 0006 Phase 3) answer explicit gap citations through
//! [`crate::chat_content::FeedContentReader`] until their data sources are bridged in.

use std::sync::{Arc, Mutex};

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::response::Response;
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use tracing::{debug, warn};

use vyapaar_agent_chat::{ChatAgent, InMemorySessionStore, SessionSelector, ToolRegistry};
use vyapaar_agent_chat_tools::ports::alert::{AlertReader, AlertWriter};
use vyapaar_agent_chat_tools::ports::error::PortError;
use vyapaar_agent_chat_tools::ports::portfolio::{AccountSummary, DateRange, OrderFilter};
use vyapaar_agent_chat_tools::ports::{market::MarketReader, portfolio::PortfolioReader};
use vyapaar_agent_core::{RuntimeMode, SessionScope, SessionStore};
use vyapaar_agent_llm::LlmProvider;
use vyapaar_agent_llm::candle_gguf::{CandleGgufConfig, CandleGgufProvider, ENV_GGUF_PATH};
use vyapaar_core::{AccountId, InstrumentId, Money};
use vyapaar_ledger::TaxLot;
use vyapaar_ledger::repository::OrderRecord;
use vyapaar_risk::RiskEngine;
use vyapaar_runtime_feed::ports::{BarReader, InstrumentMaster, QuoteReader};

use crate::AppState;
use crate::chat_market::{FeedMarketReader, FeedSnapshotSource};

/// Stand-in portfolio reader used until `AppState` exposes a ledger-repository handle to build
/// `vyapaar_runtime_api::chat_adapters::LedgerPortfolioReader::from_combined` from (see
/// `docs/gaps/0006-chat-agent-session-persistence-and-portfolio-tools.md`): every portfolio tool
/// call answers `PortError::unavailable` rather than fabricating an empty position, order, or
/// P&L that could be mistaken for a real (zero) answer.
struct StubPortfolioReader;

const PORTFOLIO_READER_GAP: &str = "portfolio reader not wired up; see docs/gaps/0006-chat-agent-session-persistence-and-portfolio-tools.md";

#[async_trait::async_trait]
impl PortfolioReader for StubPortfolioReader {
    async fn accounts(&self) -> Result<Vec<AccountSummary>, PortError> {
        Err(PortError::unavailable(PORTFOLIO_READER_GAP))
    }

    async fn positions(
        &self,
        _account: &AccountId,
    ) -> Result<Vec<vyapaar_ledger::PositionRecord>, PortError> {
        Err(PortError::unavailable(PORTFOLIO_READER_GAP))
    }

    async fn tax_lots(
        &self,
        _account: &AccountId,
        _instrument: &InstrumentId,
    ) -> Result<Vec<TaxLot>, PortError> {
        Err(PortError::unavailable(PORTFOLIO_READER_GAP))
    }

    async fn cash(&self, _account: &AccountId) -> Result<Vec<Money>, PortError> {
        Err(PortError::unavailable(PORTFOLIO_READER_GAP))
    }

    async fn orders(
        &self,
        _account: &AccountId,
        _filter: OrderFilter,
    ) -> Result<Vec<OrderRecord>, PortError> {
        Err(PortError::unavailable(PORTFOLIO_READER_GAP))
    }

    async fn realized_pnl(
        &self,
        _account: &AccountId,
        _range: DateRange,
    ) -> Result<Money, PortError> {
        Err(PortError::unavailable(PORTFOLIO_READER_GAP))
    }
}

/// Whether either LLM backend is configured via environment variables (`VYAPAAR_GGUF_PATH` for
/// the local Candle-GGUF provider, `VYAPAAR_LLM_BASE_URL` for the OpenAI/Ollama-compatible one).
fn llm_configured() -> bool {
    std::env::var(ENV_GGUF_PATH).is_ok() || std::env::var("VYAPAAR_LLM_BASE_URL").is_ok()
}

/// Resolves the `LlmProvider` from environment variables: a local GGUF model
/// (`VYAPAAR_GGUF_PATH`) takes priority; otherwise falls back to an OpenAI/Ollama-compatible
/// endpoint (`VYAPAAR_LLM_BASE_URL`/`VYAPAAR_LLM_MODEL`/`VYAPAAR_LLM_API_KEY`).
fn resolve_llm_provider() -> Result<Arc<dyn LlmProvider>, vyapaar_agent_llm::LlmError> {
    if let Ok(path) = std::env::var(ENV_GGUF_PATH) {
        let config = CandleGgufConfig::new(path);
        let provider = CandleGgufProvider::new(config);
        Ok(Arc::new(provider) as Arc<dyn LlmProvider>)
    } else {
        let provider = vyapaar_agent_llm::config::openai_provider_from_env()?;
        Ok(Arc::new(provider) as Arc<dyn LlmProvider>)
    }
}

/// The chat-agent runtime held by [`AppState`] once configured: the built conversational agent,
/// ready to open one fresh session per WebSocket connection.
pub struct ChatRuntime {
    agent: Arc<ChatAgent>,
}

impl ChatRuntime {
    /// Builds a [`ChatRuntime`] from environment variables if either a local GGUF model or an
    /// OpenAI-compatible endpoint is configured; returns `Ok(None)` when neither is set, so the
    /// caller leaves `AppState::chat` as `None` instead of failing server boot.
    ///
    /// `instruments`/`bars`/`quotes`/`risk_engine` are the same ports `AppState` already serves
    /// `/market-data/*` from — this is how `FeedMarketReader`/`FeedSnapshotSource` answer the
    /// chat tool catalog's market/screener tools with real data (closes the market/screener
    /// stand-in of docs/gaps/0006-chat-agent-session-persistence-and-portfolio-tools.md).
    /// `profiles` is the same store `/profile` routes serve: the price-alert tools read and
    /// write the caller's `Profile.alerts` document through it (see
    /// [`crate::chat_alerts::ProfileAlertStore`]).
    pub fn from_env(
        instruments: Arc<dyn InstrumentMaster>,
        bars: Arc<dyn BarReader>,
        quotes: Arc<dyn QuoteReader>,
        risk_engine: Arc<Mutex<RiskEngine>>,
        profiles: Arc<crate::profile_store::ProfileStore>,
    ) -> anyhow::Result<Option<Self>> {
        if !llm_configured() {
            return Ok(None);
        }
        let provider = resolve_llm_provider()?;

        let mut registry = ToolRegistry::new();
        // stand-in for docs/gaps/0006-chat-agent-session-persistence-and-portfolio-tools.md:
        // AppState has no ledger-repository handle for LedgerPortfolioReader::from_combined.
        let portfolio: Arc<dyn PortfolioReader> = Arc::new(StubPortfolioReader);
        let market: Arc<dyn MarketReader> = Arc::new(FeedMarketReader::new(
            instruments.clone(),
            bars,
            quotes,
            risk_engine,
        ));
        let snapshots: Arc<dyn vyapaar_agent_chat_tools::ports::snapshot::SnapshotSource> =
            Arc::new(FeedSnapshotSource::new(instruments));
        // News/filings content tools (agent plan 0006 Phase 3): no reachable data source
        // yet — `FeedContentReader` answers the explicit gap citations instead of
        // fabricated empty results (see `crate::chat_content`).
        let content: Arc<dyn vyapaar_agent_chat_tools::ports::content::ContentReader> =
            Arc::new(crate::chat_content::FeedContentReader::new());
        vyapaar_agent_chat_tools::catalog::register_read_only_tools(
            &mut registry,
            portfolio,
            market,
            content,
            Some(snapshots),
        );

        // Price alerts write the user's profile document (`ToolRisk::LocalWrite`; agent plan
        // 0006 Phase 2) — the same `Profile.alerts` value vyapaar-ui-web's alerts-store.ts
        // syncs via the /profile routes (docs/gaps/0004's profile-schema precedent).
        let alerts = Arc::new(crate::chat_alerts::ProfileAlertStore::new(profiles));
        let alert_reader: Arc<dyn AlertReader> = alerts.clone();
        let alert_writer: Arc<dyn AlertWriter> = alerts.clone();
        vyapaar_agent_chat_tools::catalog::register_alert_tools(
            &mut registry,
            alert_reader,
            alert_writer,
        );

        // stand-in for docs/gaps/0006-chat-agent-session-persistence-and-portfolio-tools.md:
        // in-memory only; SqlxSessionStore needs a &Database AppState doesn't wire up yet.
        let session_store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::new());

        let agent = ChatAgent::builder(provider, registry, session_store).build()?;
        Ok(Some(Self {
            agent: Arc::new(agent),
        }))
    }
}

/// Incoming client message shape: `{"text": "..."}`.
#[derive(Debug, Clone, Deserialize)]
struct ChatClientMessage {
    text: String,
}

/// Query params accepted on `GET /ws/chat`.
#[derive(Debug, Clone, Deserialize)]
pub struct ChatQuery {
    /// Workspace identifier the session is scoped to. Each distinct workspace gets its own
    /// session (and, once `SqlxSessionStore` lands per `docs/gaps/0006`, its own persisted
    /// history) — connections with the same `workspace` resume the most recent session for it
    /// rather than starting a new one every time (`SessionSelector::Continue`, not `New`).
    /// Omitted/blank falls back to a fixed `"default"` scope, matching pre-workspace-scoping
    /// behavior for any caller that doesn't pass one yet.
    workspace: Option<String>,
}

/// Upgrades an HTTP connection to a WebSocket bound to one chat-agent session, scoped to the
/// requested workspace (`?workspace=<id>`).
pub async fn chat_ws_handler(
    ws: WebSocketUpgrade,
    State(app_state): State<Arc<AppState>>,
    Query(query): Query<ChatQuery>,
) -> Response {
    let scope_name = query
        .workspace
        .filter(|w| !w.trim().is_empty())
        .unwrap_or_else(|| "default".to_string());
    ws.on_upgrade(move |socket| handle_chat_socket(socket, app_state, scope_name))
}

/// Handles one chat WebSocket connection: client text frames are submitted as turns, and every
/// session event the agent emits streams back as a JSON text frame.
async fn handle_chat_socket(socket: WebSocket, state: Arc<AppState>, scope_name: String) {
    let (mut sender, mut receiver) = socket.split();

    let Some(chat) = state.chat.clone() else {
        let err = serde_json::json!({
            "type": "error",
            "message": "chat not configured — set VYAPAAR_GGUF_PATH or VYAPAAR_LLM_BASE_URL",
        });
        let _ = sender.send(Message::Text(err.to_string())).await;
        return;
    };

    let (handle, mut events) = match chat
        .agent
        .open(SessionSelector::Continue(SessionScope::new(
            &scope_name,
            RuntimeMode::Live,
        )))
        .await
    {
        Ok(pair) => pair,
        Err(e) => {
            warn!("chat session open failed: {e}");
            let err = serde_json::json!({"type": "error", "message": e.to_string()});
            let _ = sender.send(Message::Text(err.to_string())).await;
            return;
        }
    };

    loop {
        tokio::select! {
            biased;

            client_msg = receiver.next() => {
                let Some(msg) = client_msg else {
                    debug!("chat WebSocket client disconnected");
                    break;
                };
                match msg {
                    Ok(Message::Text(text)) => {
                        match serde_json::from_str::<ChatClientMessage>(&text) {
                            Ok(parsed) => {
                                if let Err(e) = handle.submit(parsed.text).await {
                                    warn!("chat submit failed: {e}");
                                }
                            }
                            Err(e) => debug!("ignoring malformed chat client message: {e}"),
                        }
                    }
                    Ok(Message::Close(_)) | Err(_) => {
                        debug!("chat WebSocket connection closed");
                        break;
                    }
                    _ => {}
                }
            }

            event = events.next() => {
                let Some(event) = event else {
                    debug!("chat session event stream closed");
                    break;
                };
                match serde_json::to_string(&event) {
                    Ok(json) => {
                        if sender.send(Message::Text(json)).await.is_err() {
                            break;
                        }
                    }
                    Err(e) => warn!("failed to serialize chat session event: {e}"),
                }
            }
        }
    }
}
