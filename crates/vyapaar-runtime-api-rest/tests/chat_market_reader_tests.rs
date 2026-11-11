//! Unit tests for `FeedMarketReader`/`FeedSnapshotSource` — the read-through adapters that close
//! the market/screener stand-in of
//! docs/gaps/0006-chat-agent-session-persistence-and-portfolio-tools.md.

use std::sync::{Arc, Mutex};

use chrono::{TimeZone, Utc};
use rust_decimal::Decimal;
use vyapaar_agent_chat_tools::ports::error::PortErrorKind;
use vyapaar_agent_chat_tools::ports::market::MarketReader;
use vyapaar_agent_chat_tools::ports::portfolio::DateRange;
use vyapaar_agent_chat_tools::ports::snapshot::{SnapshotNeeds, SnapshotSource};
use vyapaar_core::{Currency, Exchange, InstrumentId, InstrumentKind, Price, PriceBand};
use vyapaar_risk::RiskEngine;
use vyapaar_runtime_api_rest::chat_market::{FeedMarketReader, FeedSnapshotSource};
use vyapaar_runtime_feed::ports::{
    BarReader, BarRequest, InstrumentMaster, PortError as FeedPortError, QuoteReader,
};
use vyapaar_runtime_feed::wire::{
    Bar, BarAggregation, BarSpecification, BarType, Instrument, PriceType, QuoteTick, UnixNanos,
};

fn ts(hour: u32) -> UnixNanos {
    UnixNanos::from_datetime(Utc.with_ymd_and_hms(2026, 1, 5, hour, 0, 0).unwrap())
}

fn reliance() -> InstrumentId {
    InstrumentId::new(Exchange::Nse, "reliance")
}

/// Fake feed implementing all three read ports over a fixed, in-memory fixture — a standard
/// trait-level test double; no network or filesystem I/O.
#[derive(Clone, Default)]
struct FakeFeed {
    instruments: Vec<Instrument>,
    quote: Option<QuoteTick>,
    bars: Vec<Bar>,
}

#[async_trait::async_trait]
impl InstrumentMaster for FakeFeed {
    async fn get_instrument(&self, id: &InstrumentId) -> Result<Option<Instrument>, FeedPortError> {
        Ok(self.instruments.iter().find(|i| i.id() == id).cloned())
    }

    async fn list_instruments(&self) -> Result<Vec<Instrument>, FeedPortError> {
        Ok(self.instruments.clone())
    }
}

#[async_trait::async_trait]
impl QuoteReader for FakeFeed {
    async fn read_quote(
        &self,
        _id: &InstrumentId,
        _as_of: Option<UnixNanos>,
    ) -> Result<Option<QuoteTick>, FeedPortError> {
        Ok(self.quote.clone())
    }
}

#[async_trait::async_trait]
impl BarReader for FakeFeed {
    async fn read_bars(&self, _request: &BarRequest) -> Result<Vec<Bar>, FeedPortError> {
        Ok(self.bars.clone())
    }
}

fn reader_over(feed: FakeFeed, risk_engine: RiskEngine) -> FeedMarketReader {
    let feed = Arc::new(feed);
    FeedMarketReader::new(
        feed.clone(),
        feed.clone(),
        feed,
        Arc::new(Mutex::new(risk_engine)),
    )
}

fn daily_bar(hour: u32, close: f64, volume: f64) -> Bar {
    let spec = BarSpecification::new(1, BarAggregation::Day, PriceType::Last);
    let bar_type = BarType::new(reliance(), spec);
    Bar::new(
        bar_type,
        close,
        close,
        close,
        close,
        volume,
        ts(hour),
        ts(hour),
    )
}

#[tokio::test]
async fn quote_reports_bid_ask_and_midpoint_last_price() {
    let instrument = reliance();
    let quote = QuoteTick::new(
        instrument.clone(),
        2_500.0,
        2_502.0,
        100.0,
        150.0,
        ts(9),
        ts(9),
    );
    let reader = reader_over(
        FakeFeed {
            quote: Some(quote),
            ..Default::default()
        },
        RiskEngine::default(),
    );

    let result = reader.quote(&instrument).await.unwrap();
    assert_eq!(result.bid.unwrap().value(), Decimal::from(2500));
    assert_eq!(result.ask.unwrap().value(), Decimal::from(2502));
    assert_eq!(result.last_price.value(), Decimal::from(2501));
}

#[tokio::test]
async fn quote_errors_when_feed_has_no_quote() {
    let reader = reader_over(FakeFeed::default(), RiskEngine::default());
    assert!(reader.quote(&reliance()).await.is_err());
}

#[tokio::test]
async fn history_caps_at_limit_and_flags_downsampled() {
    let bars = vec![daily_bar(9, 100.0, 1_000.0), daily_bar(10, 101.0, 2_000.0)];
    let reader = reader_over(
        FakeFeed {
            bars,
            ..Default::default()
        },
        RiskEngine::default(),
    );

    let history = reader
        .history(
            &reliance(),
            DateRange {
                start: None,
                end: None,
            },
            Some(1),
        )
        .await
        .unwrap();
    assert_eq!(history.bars.len(), 1);
    assert!(history.downsampled);
    assert_eq!(history.bars[0].close.value(), Decimal::from(100));
}

#[tokio::test]
async fn search_instruments_matches_symbol_case_insensitively() {
    let instrument = Instrument::new(reliance(), InstrumentKind::Equity, Currency::Inr, 1.0, 0.05);
    let reader = reader_over(
        FakeFeed {
            instruments: vec![instrument],
            ..Default::default()
        },
        RiskEngine::default(),
    );

    let hits = reader.search_instruments("rel").await.unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].exchange, "NSE");
    assert_eq!(hits[0].instrument, reliance());
}

#[tokio::test]
async fn price_band_reads_from_risk_engine() {
    let instrument = reliance();
    let band = PriceBand::new(
        Price::new(Decimal::from(2000)),
        Price::new(Decimal::from(3000)),
        Price::new(Decimal::from(2500)),
    );
    let mut engine = RiskEngine::default();
    engine.set_price_band(instrument.clone(), band);
    let reader = reader_over(FakeFeed::default(), engine);

    let result = reader.price_band(&instrument).await.unwrap();
    assert_eq!(result.lower.value(), Decimal::from(2000));
}

#[tokio::test]
async fn price_band_not_found_when_unregistered() {
    let reader = reader_over(FakeFeed::default(), RiskEngine::default());
    assert!(reader.price_band(&reliance()).await.is_err());
}

#[tokio::test]
async fn indicators_and_macro_series_are_explicitly_unavailable() {
    let reader = reader_over(FakeFeed::default(), RiskEngine::default());
    assert!(reader.indicators(&reliance(), &[]).await.is_err());
    assert!(reader.macro_series("nifty_pe").await.is_err());
}

#[tokio::test]
async fn block_deals_are_explicitly_unavailable() {
    let reader = reader_over(FakeFeed::default(), RiskEngine::default());
    let err = reader
        .block_deals(Some(&reliance()), None)
        .await
        .unwrap_err();
    assert_eq!(err.kind, PortErrorKind::Unavailable);
    assert!(
        err.message
            .contains("0005-block-bulk-deal-disclosure-ingestion.md"),
        "stub must cite the upstream gap, got: {err}"
    );
}

#[tokio::test]
async fn snapshot_source_reports_universe_from_instrument_catalog() {
    let instrument = Instrument::new(reliance(), InstrumentKind::Equity, Currency::Inr, 1.0, 0.05);
    let source = FeedSnapshotSource::new(Arc::new(FakeFeed {
        instruments: vec![instrument],
        ..Default::default()
    }));

    let snapshot = source
        .snapshot(&SnapshotNeeds::for_tool("screener"))
        .await
        .unwrap();
    assert_eq!(snapshot.universe, Some(vec!["RELIANCE".to_string()]));
}

#[tokio::test]
async fn snapshot_source_reports_no_universe_when_catalog_empty() {
    let source = FeedSnapshotSource::new(Arc::new(FakeFeed::default()));
    let snapshot = source
        .snapshot(&SnapshotNeeds::for_tool("screener"))
        .await
        .unwrap();
    assert_eq!(snapshot.universe, None);
}
