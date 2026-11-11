# 0006 — Chat agent session persistence and portfolio-backed tools are stubbed

**Status**: open

## What's stubbed or hardcoded

`crates/vyapaar-runtime-api-rest/src/chat.rs` wires the `vyapaar-agent-chat` conversational
engine to `GET /ws/chat`, backed by `vyapaar-agent-llm`'s local Candle-GGUF provider (or an
OpenAI/Ollama-compatible fallback) and the read-only tool catalog in
`vyapaar-agent-chat-tools::catalog::register_read_only_tools`. Two pieces of that wiring are
still stand-ins for v1 (the third — market/screener tools — closed via
`crates/vyapaar-runtime-api-rest/src/chat_market.rs`'s `FeedMarketReader`/`FeedSnapshotSource`;
see docs/gaps/0007-chat-market-reader-approximations-and-missing-sources.md for what those still
approximate):

1. **Session store is in-memory only.** `ChatRuntime::from_env` builds the `ChatAgent` with
   `vyapaar_agent_chat::InMemorySessionStore`, not `vyapaar-runtime-persistence`'s
   `SqlxSessionStore`. Every chat session is lost on server restart. `SqlxSessionStore` needs a
   `&Database` handle, and `AppState` (`crates/vyapaar-runtime-api-rest/src/state.rs`) has no such
   handle wired up anywhere today — adding one is out of scope for this pass.
2. **Portfolio-backed tools always answer `PortError::unavailable`.** `register_read_only_tools`
   requires an `Arc<dyn PortfolioReader>` (from
   `vyapaar-agent-chat-tools::ports::portfolio::PortfolioReader`), and the real implementation
   (`vyapaar_runtime_api::chat_adapters::LedgerPortfolioReader::from_combined`) needs an
   `Arc<R>` where `R: AccountRepository + PositionRepository + OrderRepository + LedgerRepository`.
   `AppState` holds no ledger-repository handle of any kind (checked: its fields are
   `instruments`, `bars`, `quotes`, `depth`, `strategies`, `runs`, `profiles`, `portfolios`
   (a flat JSON `PortfolioStore`, not a ledger repository), `orders` (`OrderLedger`, the REST
   write-ledger, also not a `vyapaar-ledger` repository trait object), `risk_engine`, `exec`,
   etc. — none implement the four ledger-repository traits `from_combined` needs). `chat.rs`
   instead defines `StubPortfolioReader`, which implements `PortfolioReader` by returning
   `Err(PortError::unavailable(...))` from every method (`accounts`, `positions`, `tax_lots`,
   `cash`, `orders`, `realized_pnl`) rather than fabricating empty data that an LLM could mistake
   for "this account really has zero positions."

## Why

The goal of this pass was to stand up the WebSocket bridge and wire the already-built chat engine
and tool catalog end to end, using what `AppState` already exposes. Neither of `AppState`'s
remaining gaps here is a ledger-repository handle, and wiring one up is a separate, larger piece
of work (a `Database` connection pool for session persistence; a decision on which
`vyapaar-ledger` repository implementation `AppState` should hold and how it gets populated from
the run/order flow for the portfolio tools).

## How to close it

- Session persistence: give `AppState` a `Database` handle (or an `Option<Arc<Database>>` set at
  boot the way `journals`/`profiles`/`portfolios` dirs are), and have `ChatRuntime::from_env` (or
  a new `ChatRuntime::with_persistence`) use `vyapaar-runtime-persistence::SqlxSessionStore`
  instead of `InMemorySessionStore` when it's present.
- Portfolio tools: decide which `vyapaar-ledger` repository implementation (or a thin
  `AppState`-side adapter over `OrderLedger`/`PortfolioStore`) should back
  `AccountRepository + PositionRepository + OrderRepository + LedgerRepository`, construct it in
  `AppState`, and replace `StubPortfolioReader` with
  `vyapaar_runtime_api::chat_adapters::LedgerPortfolioReader::from_combined(..)`.
