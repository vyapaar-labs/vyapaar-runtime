//! Read-through `ContentReader` adapter for the chat tool catalog's news/filings tools
//! (agent plan 0006 Phase 3).
//!
//! Neither content category has a data source reachable from this Rust runtime yet, so
//! every method answers an explicit [`PortError::unavailable`] with the gap citation the
//! chat agent surfaces to the user — never a fabricated empty success (same honest-stub
//! shape as [`crate::chat_market::FeedMarketReader`] per
//! `docs/gaps/0007-chat-market-reader-approximations-and-missing-sources.md`):
//! - News: no news ingestion exists anywhere yet — see
//!   `vyapaar-data/docs/gaps/0006-news-ingestion.md`.
//! - Filings: filings ARE ingested in `vyapaar-data`'s Python service (closed gap
//!   `vyapaar-data/docs/gaps/0003`, `vyapaar_data.service.filings_reader`), but nothing
//!   bridges that lakehouse into Rust consumers yet — see
//!   `vyapaar-data/docs/gaps/0007-filings-rust-bridge.md`.

use async_trait::async_trait;
use vyapaar_agent_chat_tools::ports::content::{ContentReader, FilingRecord, NewsArticle};
use vyapaar_agent_chat_tools::ports::error::PortError;
use vyapaar_core::InstrumentId;

/// Content adapter answering news/filings chat tools with explicit gap citations
/// until their data sources are bridged into this runtime.
pub struct FeedContentReader(());

impl FeedContentReader {
    /// Builds the content adapter. Takes no ports today: there is nothing in
    /// `AppState` to read news or filings through yet.
    pub fn new() -> Self {
        Self(())
    }
}

impl Default for FeedContentReader {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ContentReader for FeedContentReader {
    async fn news_search(
        &self,
        _query: Option<&str>,
        _instrument: Option<&str>,
        _limit: Option<usize>,
    ) -> Result<Vec<NewsArticle>, PortError> {
        // No news ingestion exists anywhere yet — see
        // vyapaar-data/docs/gaps/0006-news-ingestion.md. An explicit error, not an
        // empty success, so the model can't mistake "not wired up" for "nothing
        // happened in the news today."
        Err(PortError::unavailable(
            "news ingestion is not wired up; see vyapaar-data/docs/gaps/0006-news-ingestion.md",
        ))
    }

    async fn filings(
        &self,
        _instrument: Option<&InstrumentId>,
        _filing_type: Option<&str>,
        _limit: Option<usize>,
    ) -> Result<Vec<FilingRecord>, PortError> {
        // Filings are ingested in vyapaar-data's Python service (closed gap 0003,
        // `vyapaar_data.service.filings_reader.LakehouseFilingsReader`) but not yet
        // bridged to Rust consumers — see
        // vyapaar-data/docs/gaps/0007-filings-rust-bridge.md. Same explicit error
        // shape as `FeedMarketReader::block_deals` / `macro_series` / `indicators`.
        Err(PortError::unavailable(
            "filings are ingested in vyapaar-data but not yet bridged to Rust consumers; see vyapaar-data/docs/gaps/0007-filings-rust-bridge.md",
        ))
    }
}
