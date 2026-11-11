//! Read-through `MarketReader`/`SnapshotSource` adapters bridging the feed and risk ports
//! `AppState` already holds to the chat tool catalog's market/screener tools (closes the third
//! stand-in of docs/gaps/0006-chat-agent-session-persistence-and-portfolio-tools.md).
//!
//! Every call reads straight through to the live port each time — no cache, so quotes and bars
//! are never stale and nothing needs a background refresh task. A few fields have no backing
//! data source anywhere in this runtime yet and are reported as explicit gaps rather than
//! fabricated values; see docs/gaps/0007-chat-market-reader-approximations-and-missing-sources.md.

use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use vyapaar_agent_chat_tools::ports::error::PortError;
use vyapaar_agent_chat_tools::ports::market::{
    BlockDeal, InstrumentSearchResult, MacroSeries, MarketBar, MarketHistory, MarketIndicators,
    MarketQuote, MarketReader,
};
use vyapaar_agent_chat_tools::ports::portfolio::DateRange;
use vyapaar_agent_chat_tools::ports::snapshot::{SnapshotData, SnapshotNeeds, SnapshotSource};
use vyapaar_core::{InstrumentId, Price, PriceBand, Quantity};
use vyapaar_risk::RiskEngine;
use vyapaar_runtime_feed::ports::{BarReader, BarRequest, InstrumentMaster, QuoteReader};
use vyapaar_runtime_feed::wire::{BarAggregation, BarSpecification, PriceType, UnixNanos};

/// Bar-count cap shared with `vyapaar_runtime_api::chat_adapters::RuntimeMarketReader`.
const MAX_BAR_CAP: usize = 500;

fn to_decimal(x: f64) -> Decimal {
    Decimal::from_f64_retain(x).unwrap_or_default()
}

/// Maps the feed's async port error onto the chat tool catalog's own `PortError`.
fn feed_port_error(e: vyapaar_runtime_feed::ports::PortError) -> PortError {
    match e {
        vyapaar_runtime_feed::ports::PortError::Unsupported(msg) => PortError::unavailable(msg),
        vyapaar_runtime_feed::ports::PortError::Internal(msg) => PortError::internal(msg),
        vyapaar_runtime_feed::ports::PortError::Terminal(msg) => PortError::internal(msg),
    }
}

/// Read-through `MarketReader` over the same feed ports `AppState` already serves
/// `/market-data/*` routes from, plus the shared risk engine for price bands.
pub struct FeedMarketReader {
    instruments: Arc<dyn InstrumentMaster>,
    bars: Arc<dyn BarReader>,
    quotes: Arc<dyn QuoteReader>,
    risk_engine: Arc<Mutex<RiskEngine>>,
}

impl FeedMarketReader {
    /// Builds a [`FeedMarketReader`] over the given feed ports and shared risk engine.
    pub fn new(
        instruments: Arc<dyn InstrumentMaster>,
        bars: Arc<dyn BarReader>,
        quotes: Arc<dyn QuoteReader>,
        risk_engine: Arc<Mutex<RiskEngine>>,
    ) -> Self {
        Self {
            instruments,
            bars,
            quotes,
            risk_engine,
        }
    }
}

#[async_trait]
impl MarketReader for FeedMarketReader {
    async fn quote(&self, instrument: &InstrumentId) -> Result<MarketQuote, PortError> {
        let tick = self
            .quotes
            .read_quote(instrument, None)
            .await
            .map_err(feed_port_error)?
            .ok_or_else(|| PortError::not_found(format!("Quote not found for {instrument}")))?;
        let bid = to_decimal(tick.bid_price());
        let ask = to_decimal(tick.ask_price());
        // No last-trade print or traded-volume field is wired into `QuoteReader` (top-of-book
        // only) — see docs/gaps/0007-chat-market-reader-approximations-and-missing-sources.md.
        let mid = (bid + ask) / Decimal::from(2);
        Ok(MarketQuote {
            instrument: instrument.clone(),
            last_price: Price::new(mid),
            bid: Some(Price::new(bid)),
            ask: Some(Price::new(ask)),
            volume: Quantity::new(Decimal::ZERO),
            as_of: tick.ts_event().to_datetime(),
        })
    }

    async fn history(
        &self,
        instrument: &InstrumentId,
        range: DateRange,
        limit: Option<usize>,
    ) -> Result<MarketHistory, PortError> {
        let spec = BarSpecification::new(1, BarAggregation::Day, PriceType::Last);
        let from = range.start.map(UnixNanos::from_datetime);
        let to = range.end.map(UnixNanos::from_datetime);
        let request =
            BarRequest::new(instrument.clone(), spec, from, to).map_err(feed_port_error)?;
        let bars = self
            .bars
            .read_bars(&request)
            .await
            .map_err(feed_port_error)?;

        let effective_limit = limit.map(|l| l.min(MAX_BAR_CAP)).unwrap_or(MAX_BAR_CAP);
        let downsampled = bars.len() > effective_limit;
        let bars: Vec<MarketBar> = bars
            .into_iter()
            .take(effective_limit)
            .map(|b| MarketBar {
                timestamp: b.ts_event().to_datetime(),
                open: Price::new(to_decimal(b.open())),
                high: Price::new(to_decimal(b.high())),
                low: Price::new(to_decimal(b.low())),
                close: Price::new(to_decimal(b.close())),
                volume: Quantity::new(to_decimal(b.volume())),
            })
            .collect();

        Ok(MarketHistory {
            instrument: instrument.clone(),
            bars,
            downsampled,
        })
    }

    async fn indicators(
        &self,
        _instrument: &InstrumentId,
        _names: &[String],
    ) -> Result<MarketIndicators, PortError> {
        // No indicator engine is wired into this runtime's chat tools yet — see
        // docs/gaps/0007-chat-market-reader-approximations-and-missing-sources.md. An explicit
        // error, not an empty success, so the model can't mistake "not wired up" for "this
        // instrument really has no indicator values."
        Err(PortError::unavailable(
            "indicator engine not wired up; see docs/gaps/0007-chat-market-reader-approximations-and-missing-sources.md",
        ))
    }

    async fn search_instruments(
        &self,
        query: &str,
    ) -> Result<Vec<InstrumentSearchResult>, PortError> {
        let instruments = self
            .instruments
            .list_instruments()
            .await
            .map_err(feed_port_error)?;
        let q = query.trim().to_lowercase();
        let hits = instruments
            .into_iter()
            .filter(|inst| inst.id().symbol.to_lowercase().contains(&q))
            .map(|inst| InstrumentSearchResult {
                instrument: inst.id().clone(),
                // No instrument display-name master-data field exists yet — see
                // docs/gaps/0007-chat-market-reader-approximations-and-missing-sources.md.
                name: inst.id().symbol.clone(),
                exchange: inst.id().exchange.as_str().to_string(),
                lot_size: Some(inst.lot_size().round() as u64),
            })
            .collect();
        Ok(hits)
    }

    async fn macro_series(&self, _series: &str) -> Result<MacroSeries, PortError> {
        // No macro-economic data source is wired into this runtime yet — see
        // docs/gaps/0007-chat-market-reader-approximations-and-missing-sources.md.
        Err(PortError::unavailable(
            "macro data source not wired up; see docs/gaps/0007-chat-market-reader-approximations-and-missing-sources.md",
        ))
    }

    async fn block_deals(
        &self,
        _instrument: Option<&InstrumentId>,
        _date: Option<DateTime<Utc>>,
    ) -> Result<Vec<BlockDeal>, PortError> {
        // No block/bulk deal disclosure data source is ingested anywhere yet — see
        // vyapaar-data/docs/gaps/0005-block-bulk-deal-disclosure-ingestion.md. An explicit
        // error, not an empty success, so the model can't mistake "not wired up" for "no
        // deals happened today" (same pattern as `indicators`/`macro_series` above, per
        // docs/gaps/0007-chat-market-reader-approximations-and-missing-sources.md).
        Err(PortError::unavailable(
            "block/bulk deal disclosures are not yet ingested; see vyapaar-data/docs/gaps/0005-block-bulk-deal-disclosure-ingestion.md",
        ))
    }

    async fn price_band(&self, instrument: &InstrumentId) -> Result<PriceBand, PortError> {
        let engine = self
            .risk_engine
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        engine
            .price_bands
            .get(instrument)
            .copied()
            .ok_or_else(|| PortError::not_found(format!("Price band not found for {instrument}")))
    }
}

/// Read-through `SnapshotSource` grounding screener/universe tools in the live instrument
/// catalog `AppState` already holds.
pub struct FeedSnapshotSource {
    instruments: Arc<dyn InstrumentMaster>,
}

impl FeedSnapshotSource {
    /// Builds a [`FeedSnapshotSource`] over the given instrument catalog port.
    pub fn new(instruments: Arc<dyn InstrumentMaster>) -> Self {
        Self { instruments }
    }
}

#[async_trait]
impl SnapshotSource for FeedSnapshotSource {
    async fn snapshot(&self, _needs: &SnapshotNeeds) -> Result<SnapshotData, PortError> {
        let instruments = self
            .instruments
            .list_instruments()
            .await
            .map_err(feed_port_error)?;
        let universe: Vec<String> = instruments
            .into_iter()
            .map(|i| i.id().symbol.clone())
            .collect();
        Ok(SnapshotData {
            as_of: Utc::now(),
            universe: if universe.is_empty() {
                None
            } else {
                Some(universe)
            },
            // No regime classifier or risk-metrics feed is wired into chat tools yet — see
            // docs/gaps/0007-chat-market-reader-approximations-and-missing-sources.md.
            regime: None,
            risk_metrics: std::collections::HashMap::new(),
        })
    }
}
