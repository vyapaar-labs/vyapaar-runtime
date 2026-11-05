//! Mock implementations for backtesting and simulation.

use async_trait::async_trait;
use futures::stream::BoxStream;
use vyapaar_core::InstrumentId;
use vyapaar_market_core::{FeedHealth, MarketDataFeed, MdError, Subscription, Tick};

/// Mock market data feed that yields no data.
#[derive(Debug, Clone, Default)]
pub struct MockMarketFeed;

impl MockMarketFeed {
    /// Create a new mock feed.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl MarketDataFeed for MockMarketFeed {
    fn name(&self) -> &str {
        "mock"
    }

    async fn subscribe(
        &self,
        _subs: Vec<Subscription>,
    ) -> Result<BoxStream<'static, Tick>, MdError> {
        Ok(Box::pin(futures::stream::empty()))
    }

    async fn unsubscribe(&self, _instruments: &[InstrumentId]) -> Result<(), MdError> {
        Ok(())
    }

    async fn fetch_snapshot(&self, _instruments: &[InstrumentId]) -> Result<Vec<Tick>, MdError> {
        Ok(Vec::new())
    }

    fn feed_health(&self) -> FeedHealth {
        FeedHealth {
            name: self.name().to_string(),
            connected: false,
            last_heartbeat: None,
            rtt_ms: None,
            dropped_count: 0,
            reconnect_count: 0,
        }
    }
}
