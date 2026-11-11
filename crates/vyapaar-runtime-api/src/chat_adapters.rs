//! Port adapters bridging runtime and ledger state to conversational chat tools (§6.2, §6.4, §16.3).
//!
//! Provides:
//! - [`LedgerPortfolioReader`]: Bridges `vyapaar-ledger` domain repositories to `PortfolioReader`.
//! - [`RuntimeMarketReader`]: Bridges market data, history, and indicators to `MarketReader`.
//! - [`RuntimeSnapshotSource`]: Bridges live runtime context into `SnapshotSource` for pure tools.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use vyapaar_agent_chat_tools::ports::error::PortError;
use vyapaar_agent_chat_tools::ports::market::{
    BlockDeal, InstrumentSearchResult, MacroSeries, MarketBar, MarketHistory, MarketIndicators,
    MarketQuote, MarketReader,
};
use vyapaar_agent_chat_tools::ports::portfolio::{
    AccountSummary, DateRange, OrderFilter, PortfolioReader,
};
use vyapaar_agent_chat_tools::ports::snapshot::{SnapshotData, SnapshotNeeds, SnapshotSource};
use vyapaar_core::{AccountId, Currency, InstrumentId, Money, PriceBand};
use vyapaar_ledger::TaxLot;
use vyapaar_ledger::repository::{
    AccountCode, AccountRepository, LedgerRepository, OrderRecord, OrderRepository, PositionRecord,
    PositionRepository,
};

// ——————————————————————————————————————————————————————————————————————————
// 1. LedgerPortfolioReader (§6.2)
// ——————————————————————————————————————————————————————————————————————————

/// Read-only portfolio reader adapter bridging `vyapaar-ledger` domain repositories.
///
/// Implements `PortfolioReader` with compile-time zero-I/O read constraints (§4.3, §6.2).
pub struct LedgerPortfolioReader {
    accounts: Arc<dyn AccountRepository>,
    positions: Arc<dyn PositionRepository>,
    orders: Arc<dyn OrderRepository>,
    ledger: Arc<dyn LedgerRepository>,
}

impl LedgerPortfolioReader {
    /// Construct from individual repository instances.
    pub fn new(
        accounts: Arc<dyn AccountRepository>,
        positions: Arc<dyn PositionRepository>,
        orders: Arc<dyn OrderRepository>,
        ledger: Arc<dyn LedgerRepository>,
    ) -> Self {
        Self {
            accounts,
            positions,
            orders,
            ledger,
        }
    }

    /// Construct from a unified repository instance implementing all four traits.
    pub fn from_combined<R>(repos: Arc<R>) -> Self
    where
        R: AccountRepository + PositionRepository + OrderRepository + LedgerRepository + 'static,
    {
        Self {
            accounts: repos.clone(),
            positions: repos.clone(),
            orders: repos.clone(),
            ledger: repos,
        }
    }
}

#[async_trait]
impl PortfolioReader for LedgerPortfolioReader {
    async fn accounts(&self) -> Result<Vec<AccountSummary>, PortError> {
        let accounts = self
            .accounts
            .list_accounts()
            .map_err(|e| PortError::internal(e.to_string()))?;
        Ok(accounts
            .into_iter()
            .map(|rec| AccountSummary {
                account_id: rec.account_id,
                algo_id: rec.algo_id,
                cash: rec.cash,
                status: rec.status,
                updated_at: rec.updated_at,
            })
            .collect())
    }

    async fn positions(&self, account: &AccountId) -> Result<Vec<PositionRecord>, PortError> {
        self.positions
            .get_positions(account)
            .map_err(|e| PortError::internal(e.to_string()))
    }

    async fn tax_lots(
        &self,
        account: &AccountId,
        instrument: &InstrumentId,
    ) -> Result<Vec<TaxLot>, PortError> {
        let positions = self
            .positions
            .get_positions(account)
            .map_err(|e| PortError::internal(e.to_string()))?;
        let lots = positions
            .into_iter()
            .find(|p| p.instrument == *instrument)
            .map(|p| p.lots)
            .unwrap_or_default();
        Ok(lots)
    }

    async fn cash(&self, account: &AccountId) -> Result<Vec<Money>, PortError> {
        if let Some(acct) = self
            .accounts
            .get_account(account)
            .map_err(|e| PortError::internal(e.to_string()))?
        {
            return Ok(vec![acct.cash]);
        }
        let code = AccountCode::new(AccountCode::CASH_AT_BROKER);
        match self.ledger.get_balance(&code) {
            Ok(balance) => Ok(vec![balance]),
            Err(_) => Ok(vec![]),
        }
    }

    async fn orders(
        &self,
        account: &AccountId,
        filter: OrderFilter,
    ) -> Result<Vec<OrderRecord>, PortError> {
        let acct = self
            .accounts
            .get_account(account)
            .map_err(|e| PortError::internal(e.to_string()))?;

        let mut results = Vec::new();
        if let Some(acct) = acct {
            if let Some(algo_id) = acct.algo_id {
                let active = self
                    .orders
                    .list_active_orders(&algo_id)
                    .map_err(|e| PortError::internal(e.to_string()))?;
                results = active;
            }
        }

        if let Some(status) = filter.status {
            results.retain(|o| o.status == status);
        }
        if let Some(ref inst) = filter.instrument {
            results.retain(|o| o.instrument == *inst);
        }
        if filter.active_only {
            results.retain(|o| o.is_active());
        }
        if let Some(limit) = filter.limit {
            results.truncate(limit);
        }
        Ok(results)
    }

    async fn realized_pnl(
        &self,
        account: &AccountId,
        _range: DateRange,
    ) -> Result<Money, PortError> {
        let positions = self
            .positions
            .get_positions(account)
            .map_err(|e| PortError::internal(e.to_string()))?;
        let currency = positions
            .first()
            .map(|p| p.realized_pnl.currency)
            .unwrap_or(Currency::Inr);
        let total_amount = positions
            .into_iter()
            .fold(Decimal::ZERO, |acc, p| acc + p.realized_pnl.amount);
        Ok(Money::new(total_amount, currency))
    }
}

// ——————————————————————————————————————————————————————————————————————————
// 2. RuntimeMarketReader (§6.4)
// ——————————————————————————————————————————————————————————————————————————

/// Thread-safe in-memory market reader adapter (§6.4).
///
/// Implements `MarketReader` for conversational tools, adhering to 500-bar caps
/// and downsampling markers (§6.5).
#[derive(Default)]
pub struct RuntimeMarketReader {
    quotes: RwLock<HashMap<InstrumentId, MarketQuote>>,
    history: RwLock<HashMap<InstrumentId, Vec<MarketBar>>>,
    indicators: RwLock<HashMap<(InstrumentId, String), Decimal>>,
    search_index: RwLock<Vec<InstrumentSearchResult>>,
    macro_data: RwLock<HashMap<String, MacroSeries>>,
    price_bands: RwLock<HashMap<InstrumentId, PriceBand>>,
    block_deals: RwLock<Vec<BlockDeal>>,
}

impl RuntimeMarketReader {
    /// Create a new empty market reader.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set or update the latest quote for an instrument.
    pub fn set_quote(&self, quote: MarketQuote) {
        let mut map = self.quotes.write().unwrap();
        map.insert(quote.instrument.clone(), quote);
    }

    /// Set or update the bar history for an instrument.
    pub fn set_history(&self, instrument: InstrumentId, bars: Vec<MarketBar>) {
        let mut map = self.history.write().unwrap();
        map.insert(instrument, bars);
    }

    /// Set or update a technical indicator value.
    pub fn set_indicator(&self, instrument: InstrumentId, name: impl Into<String>, value: Decimal) {
        let mut map = self.indicators.write().unwrap();
        map.insert((instrument, name.into()), value);
    }

    /// Add an instrument to the search discovery index.
    pub fn add_instrument(&self, result: InstrumentSearchResult) {
        let mut index = self.search_index.write().unwrap();
        index.push(result);
    }

    /// Set or update a macro-economic series.
    pub fn set_macro_series(&self, series: MacroSeries) {
        let mut map = self.macro_data.write().unwrap();
        map.insert(series.series.clone(), series);
    }

    /// Set or replace the disclosed block/bulk deal list.
    pub fn set_block_deals(&self, deals: Vec<BlockDeal>) {
        *self.block_deals.write().unwrap() = deals;
    }

    /// Set or update price bands for an instrument.
    pub fn set_price_band(&self, instrument: InstrumentId, band: PriceBand) {
        let mut map = self.price_bands.write().unwrap();
        map.insert(instrument, band);
    }
}

#[async_trait]
impl MarketReader for RuntimeMarketReader {
    async fn quote(&self, instrument: &InstrumentId) -> Result<MarketQuote, PortError> {
        let map = self.quotes.read().unwrap();
        map.get(instrument)
            .cloned()
            .ok_or_else(|| PortError::not_found(format!("Quote not found for {instrument}")))
    }

    async fn history(
        &self,
        instrument: &InstrumentId,
        range: DateRange,
        limit: Option<usize>,
    ) -> Result<MarketHistory, PortError> {
        let map = self.history.read().unwrap();
        let empty = Vec::new();
        let all_bars = map.get(instrument).unwrap_or(&empty);

        let mut filtered: Vec<MarketBar> = all_bars
            .iter()
            .filter(|b| {
                if let Some(start) = range.start {
                    if b.timestamp < start {
                        return false;
                    }
                }
                if let Some(end) = range.end {
                    if b.timestamp >= end {
                        return false;
                    }
                }
                true
            })
            .cloned()
            .collect();

        const MAX_BAR_CAP: usize = 500;
        let effective_limit = limit.map(|l| l.min(MAX_BAR_CAP)).unwrap_or(MAX_BAR_CAP);

        let downsampled = filtered.len() > effective_limit;
        if downsampled {
            filtered.truncate(effective_limit);
        }

        Ok(MarketHistory {
            instrument: instrument.clone(),
            bars: filtered,
            downsampled,
        })
    }

    async fn indicators(
        &self,
        instrument: &InstrumentId,
        names: &[String],
    ) -> Result<MarketIndicators, PortError> {
        let map = self.indicators.read().unwrap();
        let mut values = HashMap::new();
        for name in names {
            if let Some(val) = map.get(&(instrument.clone(), name.clone())) {
                values.insert(name.clone(), *val);
            }
        }
        Ok(MarketIndicators {
            instrument: instrument.clone(),
            values,
            as_of: Utc::now(),
        })
    }

    async fn search_instruments(
        &self,
        query: &str,
    ) -> Result<Vec<InstrumentSearchResult>, PortError> {
        let index = self.search_index.read().unwrap();
        let q = query.trim().to_lowercase();
        let hits = index
            .iter()
            .filter(|r| {
                r.instrument.symbol.to_lowercase().contains(&q)
                    || r.name.to_lowercase().contains(&q)
            })
            .cloned()
            .collect();
        Ok(hits)
    }

    async fn macro_series(&self, series: &str) -> Result<MacroSeries, PortError> {
        let map = self.macro_data.read().unwrap();
        map.get(series)
            .cloned()
            .ok_or_else(|| PortError::not_found(format!("Macro series '{series}' not found")))
    }

    async fn block_deals(
        &self,
        instrument: Option<&InstrumentId>,
        date: Option<DateTime<Utc>>,
    ) -> Result<Vec<BlockDeal>, PortError> {
        let deals = self.block_deals.read().unwrap();
        if deals.is_empty() {
            // Nothing seeded and no real disclosure feed exists anywhere yet — see
            // vyapaar-data/docs/gaps/0005-block-bulk-deal-disclosure-ingestion.md. An
            // explicit error, not an empty success, so callers can't mistake "not wired
            // up" for "no deals happened today."
            return Err(PortError::unavailable(
                "no block/bulk deal disclosures available; see vyapaar-data/docs/gaps/0005-block-bulk-deal-disclosure-ingestion.md",
            ));
        }
        Ok(deals
            .iter()
            .filter(|d| match instrument {
                Some(i) => d.instrument == *i,
                None => true,
            })
            .filter(|d| match date {
                // Calendar-day match, not exact-instant equality: callers pass a trading
                // day, but disclosures carry a real intraday timestamp (exchange
                // publish time), so `==` would only ever match a midnight-UTC seed.
                Some(ts) => d.trade_date.date_naive() == ts.date_naive(),
                None => true,
            })
            .cloned()
            .collect())
    }

    async fn price_band(&self, instrument: &InstrumentId) -> Result<PriceBand, PortError> {
        let map = self.price_bands.read().unwrap();
        map.get(instrument)
            .copied()
            .ok_or_else(|| PortError::not_found(format!("Price band not found for {instrument}")))
    }
}

// ——————————————————————————————————————————————————————————————————————————
// 3. RuntimeSnapshotSource (§4.1, §4.4)
// ——————————————————————————————————————————————————————————————————————————

/// Live runtime snapshot source adapter grounding pure tools (§4.1, §4.4).
#[derive(Default)]
pub struct RuntimeSnapshotSource {
    universe: RwLock<Vec<String>>,
    regime: RwLock<Option<String>>,
    risk_metrics: RwLock<HashMap<String, serde_json::Value>>,
}

impl RuntimeSnapshotSource {
    /// Create a new empty snapshot source.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the screener universe symbols.
    pub fn set_universe(&self, universe: Vec<String>) {
        let mut u = self.universe.write().unwrap();
        *u = universe;
    }

    /// Set the market regime classification label.
    pub fn set_regime(&self, regime: impl Into<String>) {
        let mut r = self.regime.write().unwrap();
        *r = Some(regime.into());
    }

    /// Set a pre-trade risk metric entry.
    pub fn set_risk_metric(&self, key: impl Into<String>, value: serde_json::Value) {
        let mut m = self.risk_metrics.write().unwrap();
        m.insert(key.into(), value);
    }
}

#[async_trait]
impl SnapshotSource for RuntimeSnapshotSource {
    async fn snapshot(&self, _needs: &SnapshotNeeds) -> Result<SnapshotData, PortError> {
        let universe = {
            let u = self.universe.read().unwrap();
            if u.is_empty() { None } else { Some(u.clone()) }
        };
        let regime = self.regime.read().unwrap().clone();
        let risk_metrics = self.risk_metrics.read().unwrap().clone();

        Ok(SnapshotData {
            as_of: Utc::now(),
            universe,
            regime,
            risk_metrics,
        })
    }
}
