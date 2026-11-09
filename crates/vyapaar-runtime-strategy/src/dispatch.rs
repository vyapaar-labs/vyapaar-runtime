//! Strategy intent to multi-broker dispatch boundary.
//!
//! Provides the [`BrokerAdapter`] boundary, [`Dispatcher`] coordinator, and
//! paper/Zerodha/AngelOne execution adapters and scaffolds.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use tokio::runtime::Runtime;
use vyapaar_core::uuid::Uuid;
use vyapaar_core::{FillReport, InstrumentId, Money, OrderRequest, OrderSide, Price};
use vyapaar_exec_core::{BrokerError, BrokerExecutor};
use vyapaar_exec_zerodha::ZerodhaExecutor;
use vyapaar_market_finance::india::costs::NseCashEquitySchedule;
use vyapaar_market_finance::{CostSchedule, MarketSegment};
use vyapaar_risk::{RiskEngine, RiskRefusal};

use crate::intent::{IntentError, OrderIntent};

/// Abstraction for a broker execution venue.
///
/// Every operation receives the dispatch timestamp explicitly so that
/// simulated venues are temporally deterministic: replaying the same
/// timestamped sequence produces byte-identical fills.
pub trait BrokerAdapter: Send + Sync {
    /// Return the canonical name of the broker (e.g., `"paper"`, `"zerodha"`).
    fn broker_name(&self) -> &'static str;

    /// Submit a concrete [`OrderRequest`] to the broker venue at instant `now`.
    fn submit_order(&self, order: &OrderRequest, now: DateTime<Utc>) -> Result<String, String>;

    /// Request cancellation of an open order by broker order ID at instant `now`.
    fn cancel_order(&self, broker_order_id: &str, now: DateTime<Utc>) -> Result<(), String>;
}

impl<T: BrokerAdapter + ?Sized> BrokerAdapter for Arc<T> {
    fn broker_name(&self) -> &'static str {
        (**self).broker_name()
    }

    fn submit_order(&self, order: &OrderRequest, now: DateTime<Utc>) -> Result<String, String> {
        (**self).submit_order(order, now)
    }

    fn cancel_order(&self, broker_order_id: &str, now: DateTime<Utc>) -> Result<(), String> {
        (**self).cancel_order(broker_order_id, now)
    }
}

/// Errors arising during intent dispatch.
#[derive(Debug, thiserror::Error)]
pub enum DispatchError {
    /// Intent validation failure (e.g. non-positive quantity).
    #[error("intent error: {0}")]
    Intent(#[from] IntentError),

    /// Pre-trade SEBI / exchange risk check refusal.
    #[error("SEBI risk gate refusal: {0}")]
    RiskRefusal(Box<RiskRefusal>),

    /// Broker venue rejected or failed order execution.
    #[error("broker rejection: {0}")]
    BrokerRejection(String),

    /// Broker session expired or authentication failure.
    #[error("session expired: {0}")]
    SessionExpired(String),

    /// Market is closed or instrument not traded.
    #[error("market closed: {0}")]
    MarketClosed(String),

    /// Rate limit exceeded on broker API.
    #[error("rate limited: {0}")]
    RateLimited(String),

    /// Network connectivity or transport error.
    #[error("network error: {0}")]
    Network(String),
}

impl From<RiskRefusal> for DispatchError {
    fn from(err: RiskRefusal) -> Self {
        Self::RiskRefusal(Box::new(err))
    }
}

/// Map a [`vyapaar_exec_core::BrokerError`] produced by a real wire transport
/// into a human-readable string carried by [`DispatchError::BrokerRejection`].
///
/// The [`BrokerAdapter`] trait returns `Result<String, String>` to keep the
/// boundary sync and temporally deterministic, so the broker error taxonomy is
/// surfaced in the message text. A future trait refactor can promote these to
/// fully typed [`DispatchError`] variants (`SessionExpired`, `RateLimited`,
/// `Network`, `MarketClosed`).
fn map_broker_error(err: BrokerError) -> String {
    match err {
        BrokerError::Auth(msg) => format!("session expired: {msg}"),
        BrokerError::RateLimited(msg) => format!("rate limited: {msg}"),
        BrokerError::Transport(msg) => format!("network error: {msg}"),
        BrokerError::Timeout => "network error: request timed out".to_string(),
        BrokerError::OrderNotFound(msg)
        | BrokerError::Rejected(msg)
        | BrokerError::InvalidRequest(msg) => {
            format!("broker rejection: {msg}")
        }
        BrokerError::RiskRefusal(_) => "broker rejection: pre-trade risk refusal".to_string(),
    }
}

/// High-throughput synchronous dispatcher connecting strategy intents to broker execution venues
/// via the synchronous SEBI pre-trade risk gate.
pub struct Dispatcher {
    risk_engine: RiskEngine,
    broker: Box<dyn BrokerAdapter>,
    algo_id: String,
}

impl Dispatcher {
    /// Creates a new [`Dispatcher`] with the given risk engine and broker adapter.
    ///
    /// Automatically registers the default `algo_id` (`"STRATEGY_DISPATCH"`) in the
    /// risk engine's SEBI algorithm registry.
    pub fn new(mut risk_engine: RiskEngine, broker: Box<dyn BrokerAdapter>) -> Self {
        let algo_id = "STRATEGY_DISPATCH".to_string();
        risk_engine.register_algo(&algo_id);
        Self {
            risk_engine,
            broker,
            algo_id,
        }
    }

    /// Sets a custom SEBI algorithm identifier and registers it with the risk engine.
    pub fn with_algo_id(mut self, algo_id: impl Into<String>) -> Self {
        self.algo_id = algo_id.into();
        self.risk_engine.register_algo(&self.algo_id);
        self
    }

    /// Returns the active SEBI algo ID for this dispatcher.
    pub fn algo_id(&self) -> &str {
        &self.algo_id
    }

    /// Returns an immutable reference to the underlying broker adapter.
    pub fn broker(&self) -> &dyn BrokerAdapter {
        &*self.broker
    }

    /// Returns an immutable reference to the SEBI risk engine.
    pub fn risk_engine(&self) -> &RiskEngine {
        &self.risk_engine
    }

    /// Returns a mutable reference to the SEBI risk engine.
    pub fn risk_engine_mut(&mut self) -> &mut RiskEngine {
        &mut self.risk_engine
    }

    /// Validates an [`OrderIntent`], passes it through the synchronous SEBI pre-trade
    /// risk engine (<15 µs evaluation SLO), and dispatches it to the broker adapter.
    ///
    /// Mints a fresh random [`OrderRequest::client_order_id`]; this is the
    /// interactive convenience path. Journal replays, seeded backtests, and
    /// restored sessions must use [`Dispatcher::dispatch_intent_with_id`] so
    /// the run is byte-for-byte reproducible.
    pub fn dispatch_intent(
        &mut self,
        intent: &OrderIntent,
        now: DateTime<Utc>,
    ) -> Result<String, DispatchError> {
        let order = intent.to_order_request_with_algo(&self.algo_id)?;
        self.dispatch_order(&order, now)
    }

    /// Validates an [`OrderIntent`], passes it through the synchronous SEBI pre-trade
    /// risk engine (<15 µs evaluation SLO), and dispatches it to the broker adapter,
    /// stamping the order with the caller-supplied deterministic
    /// `client_order_id`.
    ///
    /// This is the deterministic-replay entry point: given the same intent
    /// sequence, IDs, and clock, every fill produced by a simulated adapter is
    /// byte-identical.
    pub fn dispatch_intent_with_id(
        &mut self,
        intent: &OrderIntent,
        client_order_id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<String, DispatchError> {
        let order = intent.to_order_request_with_id(client_order_id, &self.algo_id)?;
        self.dispatch_order(&order, now)
    }

    /// Evaluates an existing [`OrderRequest`] against the SEBI pre-trade risk engine
    /// and submits it to the broker adapter upon approval.
    pub fn dispatch_order(
        &mut self,
        order: &OrderRequest,
        now: DateTime<Utc>,
    ) -> Result<String, DispatchError> {
        self.risk_engine
            .check_order(order, now)
            .map_err(DispatchError::from)?;

        self.broker
            .submit_order(order, now)
            .map_err(DispatchError::BrokerRejection)
    }

    /// Requests cancellation of an order through the broker adapter and records
    /// the event in the SEBI risk engine for OTR tracking.
    pub fn cancel_order(
        &mut self,
        broker_order_id: &str,
        now: DateTime<Utc>,
    ) -> Result<(), DispatchError> {
        self.broker
            .cancel_order(broker_order_id, now)
            .map_err(DispatchError::BrokerRejection)?;

        self.risk_engine.record_cancellation(&self.algo_id, now);
        Ok(())
    }
}

/// Internal ledger state for simulated broker adapters.
#[derive(Debug, Clone)]
struct AdapterState {
    cash: Decimal,
    positions: HashMap<InstrumentId, Decimal>,
    orders: HashMap<String, OrderRequest>,
    fills: Vec<FillReport>,
    market_prices: HashMap<InstrumentId, Decimal>,
    cancelled_orders: Vec<String>,
}

impl AdapterState {
    fn new(initial_cash: Decimal) -> Self {
        Self {
            cash: initial_cash,
            positions: HashMap::new(),
            orders: HashMap::new(),
            fills: Vec::new(),
            market_prices: HashMap::new(),
            cancelled_orders: Vec::new(),
        }
    }

    fn set_market_price(&mut self, instrument: InstrumentId, price: Decimal) {
        self.market_prices.insert(instrument, price);
    }

    fn execute_order(
        &mut self,
        prefix: &str,
        order: &OrderRequest,
        now: DateTime<Utc>,
    ) -> Result<String, String> {
        let fill_price = match order.order_type.limit_price() {
            Some(price) => price.0,
            None => match self.market_prices.get(&order.instrument) {
                Some(p) => *p,
                None => {
                    return Err(format!(
                        "no market price available for market order on instrument {}",
                        order.instrument
                    ));
                }
            },
        };

        let qty = order.quantity.0;
        let notional = fill_price * qty;

        match order.side {
            OrderSide::Buy => {
                self.cash -= notional;
                *self
                    .positions
                    .entry(order.instrument.clone())
                    .or_insert(Decimal::ZERO) += qty;
            }
            OrderSide::Sell => {
                self.cash += notional;
                *self
                    .positions
                    .entry(order.instrument.clone())
                    .or_insert(Decimal::ZERO) -= qty;
            }
        }

        // Compute statutory transaction costs using vyapaar-market-finance
        let notional_f64 = notional.to_f64().unwrap_or(0.0);
        let schedule = NseCashEquitySchedule::delivery();
        let seg = MarketSegment::from("equity");
        let raw_fees = schedule.compute_costs(&seg, order.side, notional_f64);

        let brokerage = Money::inr(
            Decimal::from_f64_retain(raw_fees.get("brokerage").unwrap_or(0.0)).unwrap_or_default(),
        );
        let stt = Money::inr(
            Decimal::from_f64_retain(raw_fees.get("stt").unwrap_or(0.0)).unwrap_or_default(),
        );
        let exchange_charges = Money::inr(
            Decimal::from_f64_retain(raw_fees.get("exchange_fee").unwrap_or(0.0))
                .unwrap_or_default(),
        );
        let gst = Money::inr(
            Decimal::from_f64_retain(raw_fees.get("gst").unwrap_or(0.0)).unwrap_or_default(),
        );
        let stamp_duty = Money::inr(
            Decimal::from_f64_retain(raw_fees.get("stamp_duty").unwrap_or(0.0)).unwrap_or_default(),
        );
        let sebi_charges = Money::inr(
            Decimal::from_f64_retain(raw_fees.get("sebi_fee").unwrap_or(0.0)).unwrap_or_default(),
        );

        let fee_breakdown = vyapaar_core::FeeBreakdown {
            brokerage,
            stt,
            exchange_charges,
            gst,
            stamp_duty,
            sebi_charges,
        };
        let total_fee = fee_breakdown.total();

        // Deduct statutory fees from cash
        self.cash -= total_fee.amount;

        // Deterministic fill identity: a per-venue monotonic sequence (same
        // convention as exchange-provided fill numbers). No random UUIDs and
        // no wall-clock queries here — replaying the same timestamped order
        // sequence must reproduce this fill byte-for-byte.
        let order_id = format!("{prefix}-{}", order.client_order_id);
        let fill = FillReport {
            fill_id: format!("FILL-{:04}", self.fills.len() + 1),
            order_id: order_id.clone(),
            client_order_id: order.client_order_id,
            instrument: order.instrument.clone(),
            side: order.side,
            price: Price(fill_price),
            quantity: order.quantity,
            algo_id: order.algo_id.clone(),
            timestamp: now,
            correlation_id: order.correlation_id,
            client_id: order.client_id.clone(),
            account_id: None,
            fee: total_fee,
            fee_breakdown: Some(fee_breakdown),
            trace_context: order.trace_context,
        };

        self.fills.push(fill);
        self.orders.insert(order_id.clone(), order.clone());
        Ok(order_id)
    }

    fn cancel_order(&mut self, broker_order_id: &str) -> Result<(), String> {
        if self.orders.contains_key(broker_order_id) {
            self.cancelled_orders.push(broker_order_id.to_string());
            Ok(())
        } else {
            Err(format!("order '{broker_order_id}' not found"))
        }
    }

    fn equity(&self) -> Decimal {
        let mut total = self.cash;
        for (inst, qty) in &self.positions {
            if let Some(price) = self.market_prices.get(inst) {
                total += *qty * *price;
            }
        }
        total
    }

    fn equity_with_prices(&self, prices: &HashMap<InstrumentId, Decimal>) -> Decimal {
        let mut total = self.cash;
        for (inst, qty) in &self.positions {
            let price = prices
                .get(inst)
                .or_else(|| self.market_prices.get(inst))
                .copied()
                .unwrap_or(Decimal::ZERO);
            total += *qty * price;
        }
        total
    }
}

/// In-memory paper trading broker adapter.
#[derive(Debug, Clone)]
pub struct PaperAdapter {
    state: Arc<Mutex<AdapterState>>,
}

impl PaperAdapter {
    /// Creates a new [`PaperAdapter`] with the specified initial cash balance in INR.
    pub fn new(initial_cash: Decimal) -> Self {
        Self {
            state: Arc::new(Mutex::new(AdapterState::new(initial_cash))),
        }
    }

    /// Sets or updates the market price for an instrument.
    pub fn set_market_price(&self, instrument: InstrumentId, price: Decimal) {
        self.state
            .lock()
            .expect("poisoned lock")
            .set_market_price(instrument, price);
    }

    /// Returns the current cash balance in INR.
    pub fn cash(&self) -> Decimal {
        self.state.lock().expect("poisoned lock").cash
    }

    /// Returns the current open positions.
    pub fn positions(&self) -> HashMap<InstrumentId, Decimal> {
        self.state.lock().expect("poisoned lock").positions.clone()
    }

    /// Returns all execution fills recorded by this adapter.
    pub fn fills(&self) -> Vec<FillReport> {
        self.state.lock().expect("poisoned lock").fills.clone()
    }

    /// Returns all orders submitted to this adapter.
    pub fn orders(&self) -> HashMap<String, OrderRequest> {
        self.state.lock().expect("poisoned lock").orders.clone()
    }

    /// Computes current total portfolio equity (cash + mark-to-market positions).
    pub fn equity(&self) -> Decimal {
        self.state.lock().expect("poisoned lock").equity()
    }

    /// Computes portfolio equity using an explicit set of market prices.
    pub fn equity_with_prices(&self, prices: &HashMap<InstrumentId, Decimal>) -> Decimal {
        self.state
            .lock()
            .expect("poisoned lock")
            .equity_with_prices(prices)
    }
}

impl Default for PaperAdapter {
    fn default() -> Self {
        Self::new(Decimal::new(1_000_000, 0))
    }
}

impl BrokerAdapter for PaperAdapter {
    fn broker_name(&self) -> &'static str {
        "paper"
    }

    fn submit_order(&self, order: &OrderRequest, now: DateTime<Utc>) -> Result<String, String> {
        self.state
            .lock()
            .expect("poisoned lock")
            .execute_order("PAPER", order, now)
    }

    fn cancel_order(&self, broker_order_id: &str, _now: DateTime<Utc>) -> Result<(), String> {
        self.state
            .lock()
            .expect("poisoned lock")
            .cancel_order(broker_order_id)
    }
}

/// Concrete implementation for Zerodha (Kite Connect v3) execution adapter.
#[derive(Clone)]
pub struct ZerodhaKiteAdapter {
    api_key: Option<String>,
    access_token: Option<String>,
    state: Arc<Mutex<AdapterState>>,
    /// Live Kite Connect executor, only present when credentials are supplied.
    /// When `None` the adapter runs in simulation/scaffold mode.
    executor: Option<Arc<ZerodhaExecutor>>,
    /// Tokio runtime used to drive the async `ZerodhaExecutor` from the sync
    /// [`BrokerAdapter`] trait methods. Only `Some` alongside `executor`.
    runtime: Option<Arc<Runtime>>,
}

impl std::fmt::Debug for ZerodhaKiteAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ZerodhaKiteAdapter")
            .field("api_key", &self.api_key)
            .field("access_token", &self.access_token)
            .field("state", &self.state)
            .field(
                "executor",
                &self.executor.as_ref().map(|_| "ZerodhaExecutor"),
            )
            .field(
                "runtime",
                &self.runtime.as_ref().map(|_| "tokio::runtime::Runtime"),
            )
            .finish()
    }
}

impl ZerodhaKiteAdapter {
    /// Creates a new [`ZerodhaKiteAdapter`] in simulation/scaffold mode with the given initial cash.
    pub fn new(initial_cash: Decimal) -> Self {
        Self {
            api_key: None,
            access_token: None,
            state: Arc::new(Mutex::new(AdapterState::new(initial_cash))),
            executor: None,
            runtime: None,
        }
    }

    /// Creates a new [`ZerodhaKiteAdapter`] configured with Kite Connect credentials
    /// and the default Kite Connect base URL (`https://api.kite.trade`).
    ///
    /// When credentials are supplied the adapter holds a live
    /// [`ZerodhaExecutor`] (backed by [`vyapaar-exec`]) plus a tokio runtime so
    /// the sync [`BrokerAdapter`] trait can drive the async wire transport.
    /// [`submit_order`](Self::submit_order) / [`cancel_order`](Self::cancel_order)
    /// then hit the real Kite Connect `POST /orders/regular` / `DELETE
    /// /orders/regular/{id}` endpoints instead of the in-process simulation.
    pub fn with_credentials(
        api_key: impl Into<String>,
        access_token: impl Into<String>,
        initial_cash: Decimal,
    ) -> Self {
        Self::with_base_url(
            api_key,
            access_token,
            ZerodhaExecutor::DEFAULT_BASE_URL,
            initial_cash,
        )
    }

    /// Creates a new [`ZerodhaKiteAdapter`] configured with Kite Connect
    /// credentials and a custom base URL (useful for testing against a local
    /// mock or point-of-presence).
    pub fn with_base_url(
        api_key: impl Into<String>,
        access_token: impl Into<String>,
        base_url: impl Into<String>,
        initial_cash: Decimal,
    ) -> Self {
        let key = api_key.into();
        let token = access_token.into();
        let exec = ZerodhaExecutor::with_base_url(key.clone(), token.clone(), base_url);
        let runtime = Runtime::new().expect("tokio runtime for zerodha wire transport");
        Self {
            api_key: Some(key),
            access_token: Some(token),
            state: Arc::new(Mutex::new(AdapterState::new(initial_cash))),
            executor: Some(Arc::new(exec)),
            runtime: Some(Arc::new(runtime)),
        }
    }

    /// Returns the configured Kite API key if set.
    pub fn api_key(&self) -> Option<&str> {
        self.api_key.as_deref()
    }

    /// Returns the configured Kite access token if set.
    pub fn access_token(&self) -> Option<&str> {
        self.access_token.as_deref()
    }

    /// Sets or updates the market price for an instrument.
    pub fn set_market_price(&self, instrument: InstrumentId, price: Decimal) {
        self.state
            .lock()
            .expect("poisoned lock")
            .set_market_price(instrument, price);
    }

    /// Returns the current cash balance in INR.
    pub fn cash(&self) -> Decimal {
        self.state.lock().expect("poisoned lock").cash
    }

    /// Returns the current open positions.
    pub fn positions(&self) -> HashMap<InstrumentId, Decimal> {
        self.state.lock().expect("poisoned lock").positions.clone()
    }

    /// Returns all execution fills recorded by this adapter.
    pub fn fills(&self) -> Vec<FillReport> {
        self.state.lock().expect("poisoned lock").fills.clone()
    }

    /// Returns all orders submitted to this adapter.
    pub fn orders(&self) -> HashMap<String, OrderRequest> {
        self.state.lock().expect("poisoned lock").orders.clone()
    }

    /// Computes current total portfolio equity (cash + mark-to-market positions).
    pub fn equity(&self) -> Decimal {
        self.state.lock().expect("poisoned lock").equity()
    }

    /// Computes portfolio equity using an explicit set of market prices.
    pub fn equity_with_prices(&self, prices: &HashMap<InstrumentId, Decimal>) -> Decimal {
        self.state
            .lock()
            .expect("poisoned lock")
            .equity_with_prices(prices)
    }
}

impl Default for ZerodhaKiteAdapter {
    fn default() -> Self {
        Self::new(Decimal::new(1_000_000, 0))
    }
}

impl BrokerAdapter for ZerodhaKiteAdapter {
    fn broker_name(&self) -> &'static str {
        "zerodha"
    }

    fn submit_order(&self, order: &OrderRequest, now: DateTime<Utc>) -> Result<String, String> {
        // When a live ZerodhaExecutor is configured (credentials supplied),
        // place the order on the real Kite Connect wire transport, delegating
        // auth/session/rate-limiting to `vyapaar-exec`. Otherwise fall back to
        // the in-process simulation scaffold (preserving determinism and
        // paper/live economic parity for the sim path).
        if let Some(exec) = &self.executor {
            let runtime = self
                .runtime
                .as_ref()
                .expect("zerodha executor configured without tokio runtime");
            // NOTE: `block_on` must not be called from within a tokio runtime
            // worker thread (it panics). The sync `BrokerAdapter` boundary is
            // currently only invoked from sync code; if the `Dispatcher` is
            // later driven from an async actor context, this seam must become
            // async or marshal onto a dedicated thread.
            let ack = runtime
                .block_on(exec.place_order(order.clone()))
                .map_err(map_broker_error)?;
            return Ok(ack.order_id);
        }
        self.state
            .lock()
            .expect("poisoned lock")
            .execute_order("KITE", order, now)
    }

    fn cancel_order(&self, broker_order_id: &str, _now: DateTime<Utc>) -> Result<(), String> {
        if let Some(exec) = &self.executor {
            let runtime = self
                .runtime
                .as_ref()
                .expect("zerodha executor configured without tokio runtime");
            return runtime
                .block_on(exec.cancel_order(broker_order_id))
                .map_err(map_broker_error);
        }
        self.state
            .lock()
            .expect("poisoned lock")
            .cancel_order(broker_order_id)
    }
}

/// Concrete implementation for Angel One (SmartAPI) execution adapter.
///
/// NOTE (GAP-0003): `vyapaar-exec-angelone` is a placeholder crate
/// (`vyapaar-exec/crates/vyapaar-exec-angelone`), so no real SmartAPI
/// `placeOrder`/`cancelOrder` transport exists yet. This adapter therefore
/// remains a simulation/scaffold identical in economics to [`PaperAdapter`].
/// The real SmartAPI wire transport belongs in `vyapaar-exec`/`vyapaar-adapters`
/// and will plug into this same [`BrokerAdapter`] boundary once available.
#[derive(Debug, Clone)]
pub struct AngelOneSmartApiAdapter {
    api_key: Option<String>,
    client_code: Option<String>,
    jwt_token: Option<String>,
    state: Arc<Mutex<AdapterState>>,
}

impl AngelOneSmartApiAdapter {
    /// Creates a new [`AngelOneSmartApiAdapter`] in simulation/scaffold mode with the given initial cash.
    pub fn new(initial_cash: Decimal) -> Self {
        Self {
            api_key: None,
            client_code: None,
            jwt_token: None,
            state: Arc::new(Mutex::new(AdapterState::new(initial_cash))),
        }
    }

    /// Creates a new [`AngelOneSmartApiAdapter`] configured with SmartAPI credentials.
    pub fn with_credentials(
        api_key: impl Into<String>,
        client_code: impl Into<String>,
        jwt_token: impl Into<String>,
        initial_cash: Decimal,
    ) -> Self {
        Self {
            api_key: Some(api_key.into()),
            client_code: Some(client_code.into()),
            jwt_token: Some(jwt_token.into()),
            state: Arc::new(Mutex::new(AdapterState::new(initial_cash))),
        }
    }

    /// Returns the configured SmartAPI key if set.
    pub fn api_key(&self) -> Option<&str> {
        self.api_key.as_deref()
    }

    /// Returns the configured SmartAPI client code if set.
    pub fn client_code(&self) -> Option<&str> {
        self.client_code.as_deref()
    }

    /// Returns the configured SmartAPI JWT session token if set.
    pub fn jwt_token(&self) -> Option<&str> {
        self.jwt_token.as_deref()
    }

    /// Sets or updates the market price for an instrument.
    pub fn set_market_price(&self, instrument: InstrumentId, price: Decimal) {
        self.state
            .lock()
            .expect("poisoned lock")
            .set_market_price(instrument, price);
    }

    /// Returns the current cash balance in INR.
    pub fn cash(&self) -> Decimal {
        self.state.lock().expect("poisoned lock").cash
    }

    /// Returns the current open positions.
    pub fn positions(&self) -> HashMap<InstrumentId, Decimal> {
        self.state.lock().expect("poisoned lock").positions.clone()
    }

    /// Returns all execution fills recorded by this adapter.
    pub fn fills(&self) -> Vec<FillReport> {
        self.state.lock().expect("poisoned lock").fills.clone()
    }

    /// Returns all orders submitted to this adapter.
    pub fn orders(&self) -> HashMap<String, OrderRequest> {
        self.state.lock().expect("poisoned lock").orders.clone()
    }

    /// Computes current total portfolio equity (cash + mark-to-market positions).
    pub fn equity(&self) -> Decimal {
        self.state.lock().expect("poisoned lock").equity()
    }

    /// Computes portfolio equity using an explicit set of market prices.
    pub fn equity_with_prices(&self, prices: &HashMap<InstrumentId, Decimal>) -> Decimal {
        self.state
            .lock()
            .expect("poisoned lock")
            .equity_with_prices(prices)
    }
}

impl Default for AngelOneSmartApiAdapter {
    fn default() -> Self {
        Self::new(Decimal::new(1_000_000, 0))
    }
}

impl BrokerAdapter for AngelOneSmartApiAdapter {
    fn broker_name(&self) -> &'static str {
        "angelone"
    }

    fn submit_order(&self, order: &OrderRequest, now: DateTime<Utc>) -> Result<String, String> {
        self.state
            .lock()
            .expect("poisoned lock")
            .execute_order("SMARTAPI", order, now)
    }

    fn cancel_order(&self, broker_order_id: &str, _now: DateTime<Utc>) -> Result<(), String> {
        self.state
            .lock()
            .expect("poisoned lock")
            .cancel_order(broker_order_id)
    }
}

/// Type alias for [`ZerodhaKiteAdapter`].
pub type ZerodhaAdapter = ZerodhaKiteAdapter;

/// Type alias for [`AngelOneSmartApiAdapter`].
pub type AngelOneAdapter = AngelOneSmartApiAdapter;
