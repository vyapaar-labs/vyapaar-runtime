//! Local-LLM-backed chat WebSocket bridge (`GET /ws/chat`).
//!
//! Bridges the `vyapaar-agent-chat` conversational engine (session/turn/event model) to this
//! server over a WebSocket, backed by `vyapaar-agent-llm`'s local Candle-GGUF provider (or an
//! OpenAI/Ollama-compatible fallback) and the read-only tool ports in
//! `vyapaar_runtime_api::chat_adapters`.
//!
//! Client -> server messages are `{"text": "..."}`; everything the agent emits for the turn
//! streams back as the chat engine's own `SessionEvent` JSON, one WebSocket text frame per event.
//!
//! v1 scope (see `docs/gaps/0006-chat-agent-session-persistence-and-portfolio-tools.md`):
//! - Sessions are in-memory only (`InMemorySessionStore`); a server restart loses them.
//! - Portfolio-backed tools are stubbed via [`StubPortfolioReader`]: `AppState` holds no
//!   ledger-repository handle to build `LedgerPortfolioReader::from_combined` from, so every
//!   portfolio-backed tool call answers empty rather than a real position or P&L.
//! - `RuntimeMarketReader`/`RuntimeSnapshotSource` are constructed empty; nothing calls their
//!   setters, so market/screener tool calls also answer empty until that is wired up.

use std::sync::Arc;

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use tracing::{debug, warn};

use vyapaar_agent_chat::{ChatAgent, InMemorySessionStore, SessionSelector, ToolRegistry};
use vyapaar_agent_chat_tools::ports::error::PortError;
use vyapaar_agent_chat_tools::ports::portfolio::{AccountSummary, DateRange, OrderFilter};
use vyapaar_agent_chat_tools::ports::{market::MarketReader, portfolio::PortfolioReader};
use vyapaar_agent_core::{RuntimeMode, SessionScope, SessionStore};
use vyapaar_agent_llm::LlmProvider;
use vyapaar_agent_llm::candle_gguf::{CandleGgufConfig, CandleGgufProvider, ENV_GGUF_PATH};
use vyapaar_core::{AccountId, InstrumentId, Money};
use vyapaar_ledger::TaxLot;
use vyapaar_ledger::repository::OrderRecord;
use vyapaar_runtime_api::chat_adapters::{RuntimeMarketReader, RuntimeSnapshotSource};

use crate::AppState;

/// Stand-in portfolio reader used until `AppState` exposes a ledger-repository handle to build
/// `vyapaar_runtime_api::chat_adapters::LedgerPortfolioReader::from_combined` from (see
/// `docs/gaps/0006-chat-agent-session-persistence-and-portfolio-tools.md`): every portfolio tool
/// call answers `PortError::unavailable` rather than fabricating an empty position, order, or
/// P&L that could be mistaken for a real (zero) answer.
struct StubPortfolioReader;

const PORTFOLIO_READER_GAP: &str =
    "portfolio reader not wired up; see docs/gaps/0006-chat-agent-session-persistence-and-portfolio-tools.md";

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
    pub fn from_env() -> anyhow::Result<Option<Self>> {
        if !llm_configured() {
            return Ok(None);
        }
        let provider = resolve_llm_provider()?;

        let mut registry = ToolRegistry::new();
        // stand-in for docs/gaps/0006-chat-agent-session-persistence-and-portfolio-tools.md:
        // AppState has no ledger-repository handle for LedgerPortfolioReader::from_combined.
        let portfolio: Arc<dyn PortfolioReader> = Arc::new(StubPortfolioReader);
        // stand-in for docs/gaps/0006-chat-agent-session-persistence-and-portfolio-tools.md:
        // constructed empty, nothing calls these readers' setters yet.
        let market: Arc<dyn MarketReader> = Arc::new(RuntimeMarketReader::new());
        let snapshots: Arc<dyn vyapaar_agent_chat_tools::ports::snapshot::SnapshotSource> =
            Arc::new(RuntimeSnapshotSource::new());
        vyapaar_agent_chat_tools::catalog::register_read_only_tools(
            &mut registry,
            portfolio,
            market,
            Some(snapshots),
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

/// Upgrades an HTTP connection to a WebSocket bound to one fresh chat-agent session.
pub async fn chat_ws_handler(
    ws: WebSocketUpgrade,
    State(app_state): State<Arc<AppState>>,
) -> Response {
    ws.on_upgrade(move |socket| handle_chat_socket(socket, app_state))
}

/// Handles one chat WebSocket connection: client text frames are submitted as turns, and every
/// session event the agent emits streams back as a JSON text frame.
async fn handle_chat_socket(socket: WebSocket, state: Arc<AppState>) {
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
        .open(SessionSelector::New(SessionScope::new(
            "web-chat",
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
