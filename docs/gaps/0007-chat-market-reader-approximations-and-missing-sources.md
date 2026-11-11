# 0007 — Chat market/content-reader adapters have approximated fields and missing data sources

**Status**: open

## What's stubbed or hardcoded

`crates/vyapaar-runtime-api-rest/src/chat_market.rs`'s `FeedMarketReader`/`FeedSnapshotSource`
close the market/screener stand-in of
`docs/gaps/0006-chat-agent-session-persistence-and-portfolio-tools.md` by reading through to the
same `InstrumentMaster`/`BarReader`/`QuoteReader`/`RiskEngine` ports `AppState` already serves
`/market-data/*` and `/risk` from. A few fields still have no real backing source in this runtime
and are either approximated or answered with an explicit error rather than fabricated data:

1. **`MarketQuote::last_price` is the bid/ask midpoint, not a real last-trade print.**
   `QuoteReader::read_quote` returns only a top-of-book `QuoteTick` (bid/ask price and size); there
   is no last-traded-price feed wired into it.
2. **`MarketQuote::volume` is always `Decimal::ZERO`.** `QuoteTick` carries no traded-volume field
   either, so there is nothing real to report; zero is a placeholder, not an observed value.
3. **`MarketReader::indicators` and `MarketReader::macro_series` always answer
   `PortError::unavailable`.** No indicator engine or macro-economic data source exists anywhere
   in `vyapaar-runtime` yet for the chat tools to read from.
4. **`InstrumentSearchResult::name` falls back to the raw symbol.** `vyapaar_market_store::wire::Instrument`
   has no human-readable display-name field, only `id()` (the `InstrumentId`), `kind()`,
   `currency()`, `lot_size()`, `tick_size()`.
5. **`SnapshotData::regime` and `SnapshotData::risk_metrics` are always `None`/empty.**
   `FeedSnapshotSource` only populates `universe` (from `InstrumentMaster::list_instruments`); no
   market-regime classifier or curated risk-metrics feed exists yet to back the other two fields.
6. **`MarketReader::block_deals` always answers `PortError::unavailable`.** Added with chat tool
   `block_deals` (plan 0004 Phases 1–3): no block/bulk deal disclosure data source is ingested
   anywhere yet — the upstream ingestion decision lives in
   `vyapaar-data/docs/gaps/0005-block-bulk-deal-disclosure-ingestion.md`. The tool reports the gap
   explicitly instead of the model fabricating deals.
7. **`ContentReader::news_search` and `ContentReader::filings_search` always answer
   `PortError::unavailable`.** Added with chat tools `news_search`/`filings_search`
   (`vyapaar-agent` plan 0006 Phase 3) via the new
   `crates/vyapaar-runtime-api-rest/src/chat_content.rs::FeedContentReader`: news has no data
   source anywhere yet (`vyapaar-data/docs/gaps/0006-news-ingestion.md`), and filings are ingested
   in `vyapaar-data`'s Python service (closed gap 0003) but nothing bridges that lakehouse into
   Rust consumers yet (`vyapaar-data/docs/gaps/0007-filings-rust-bridge.md`). Both report the gap
   explicitly instead of the model fabricating headlines or filings.

## Why

Closing docs/gaps/0006's market/screener stand-in only required wiring the tool catalog to data
AppState already has live access to (instrument reference data, quotes, bars, risk-engine price
bands). The fields above need either a new data source this runtime doesn't have yet (a
last-trade/volume feed, an indicator engine, a macro-economic data integration, a regime
classifier) or a product decision (which risk metrics to surface to the chat agent) — both out of
scope for a read-through wiring pass.

## How to close it

- Items 1–2: wire a last-trade/volume feed into `QuoteReader` (or a sibling port) and read it from
  `FeedMarketReader::quote`.
- Item 3: stand up an indicator engine (or reuse one from `vyapaar-market`'s 100+ indicator
  library) and a macro-economic data source, and implement `FeedMarketReader::indicators`/
  `macro_series` against them instead of returning `PortError::unavailable`.
- Item 4: add a display-name field to `Instrument` (`vyapaar-market-store`'s `wire.rs`) sourced
  from instrument reference data, and read it in `FeedMarketReader::search_instruments`.
- Item 5: decide which risk metrics (OTR counters, halt status, kill-switch state) the chat agent
  should see, and either build or find a market-regime classifier; wire both into
  `FeedSnapshotSource::snapshot`.
- Item 6: close `vyapaar-data` gap 0005 (disclosure ingestion), then replace
  `FeedMarketReader::block_deals`'s stub with a real read (plan 0004 Phase 4).
- Item 7: close `vyapaar-data` gaps 0006 (news ingestion) and 0007 (filings → Rust bridge),
  then replace `FeedContentReader`'s stubs with real reads through the bridge.
