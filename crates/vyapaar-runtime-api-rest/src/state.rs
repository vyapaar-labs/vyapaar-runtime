//! Shared state of the REST API: the read ports the handlers depend on.
//!
//! Handlers see only the read ports owned by `vyapaar-ports` ([`InstrumentMaster`],
//! [`BarReader`], [`QuoteReader`], [`DepthReader`]); which adapter answers is decided here, at the composition
//! root. This crate names `vyapaar-feed` only to build those adapters.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError, RwLock};

use vyapaar_core::Currency;
use vyapaar_core::rust_decimal::Decimal;
use vyapaar_risk::RiskEngine;
use vyapaar_runtime_api::{ErrorDetail, StrategyCatalog, TradingState};
use vyapaar_runtime_feed::ports::{BarReader, DepthReader, InstrumentMaster, QuoteReader};
use vyapaar_runtime_feed::wire::UnixNanos;
use vyapaar_runtime_feed::{DatasetReader, LiveQuoteStore, ReaderError};

use crate::{
    ApprovalQueue, BacktestExecutor, OrderLedger, PortfolioStore, ProfileStore, RunService,
    RunServiceConfig, RunStore, StrategyRegistry,
};

/// App state for REST API.
#[derive(Clone)]
pub struct AppState {
    /// Reference data: which instruments exist.
    pub instruments: Arc<dyn InstrumentMaster>,
    /// Stored historical bars.
    pub bars: Arc<dyn BarReader>,
    /// Latest top-of-book quotes.
    pub quotes: Arc<dyn QuoteReader>,
    /// Order-book depth.
    pub depth: Arc<dyn DepthReader>,
    /// Strategies compiled by `POST /strategies` in this process.
    ///
    /// Session-scoped and in memory: it is a plain pure value from `vyapaar-api`, not a port,
    /// because it holds no external resource and nothing else implements it. Clones of the
    /// state share it.
    pub strategies: Arc<Mutex<StrategyCatalog>>,
    /// The run service behind `/backtests` and `/journals`; `None` until a journals root is
    /// configured ([`AppState::with_journals_dir`]).
    pub runs: Option<Arc<RunService>>,
    /// The per-user profile store behind `/profile`; `None` until a profiles directory is
    /// configured ([`AppState::with_profiles_dir`]).
    pub profiles: Option<Arc<ProfileStore>>,
    /// The per-user portfolio store behind `/portfolio`; `None` until a portfolios directory is
    /// configured ([`AppState::with_portfolios_dir`]).
    pub portfolios: Option<Arc<PortfolioStore>>,
    /// NDJSON file `PUT /trading-state` appends one record per change to; `None` until
    /// configured ([`AppState::with_operator_journal`]). The state is never restored from it.
    pub operator_journal: Option<PathBuf>,
    /// Account currency of the runs (journal fills carry no costs; they are zero in this).
    pub account_currency: Currency,
    /// The operator trading state the write routes evaluate against (ADR 0018 decision 7).
    ///
    /// Seeded by the composition root ([`AppState::with_trading_state`]) and changed at runtime
    /// by `PUT /trading-state` ([`AppState::set_trading_state`]). One cell shared by every
    /// clone of the state, so the order gate and `POST /positions/close` see a change on the
    /// next request. `Halted` refuses placements while cancel and close still pass.
    trading_state: Arc<RwLock<TradingState>>,
    /// The pre-trade risk gate the order routes check against (ADR 0018 decision 7). Persistent
    /// across requests (SEBI algo registration, OTR trackers, halts) so a fresh `RiskEngine`
    /// isn't built per request; [`crate::orders::REST_ALGO_ID`] is pre-registered at
    /// construction. Limits default to none; set them with [`AppState::with_risk_limits`].
    pub risk_engine: Arc<Mutex<RiskEngine>>,
    /// Approval queue for order placements (E5-S4).
    ///
    /// When enabled, `POST /orders` is held here until approved by an operator.
    /// Cancel and modify bypass the queue.
    pub approval_queue: Option<Arc<ApprovalQueue>>,
    /// The write ledger every risk-approved placement lands in (E11-S7, ADR 0019
    /// decision 3). `GET /orders` reads it; a restart empties it.
    pub orders: OrderLedger,
    /// The embedded `NextOpenSim` approved orders are submitted to (ADR 0016).
    pub(crate) exec: crate::exec::ExecGateway,
    /// The writable store behind `quotes`, when the state was built by `from_reader`.
    live: Option<Arc<LiveQuoteStore>>,
    /// The quote/trade tick source `fill_model = l2` runs match against (ADR 0030); `None`
    /// refuses such runs with `l2_requires_ticks`.
    ticks: Option<Arc<dyn vyapaar_runtime_feed::ports::TickReader>>,
    /// Execution config the run service's executor is built with (default: bar fills).
    execution: vyapaar_runtime_config::ExecutionConfig,
    /// The sending side of the bar ingest channel behind `POST /bars`; `None` until
    /// [`AppState::start_bar_ingest`] (or [`AppState::attach_bar_ingest`]) and again after
    /// [`AppState::stop_bar_ingest`]. Shared by every clone of the state.
    pub(crate) bar_ingest: Arc<Mutex<Option<tokio::sync::mpsc::Sender<vyapaar_core::Bar>>>>,
    /// Broadcast channel for live market ticks from MarketDataActor.
    /// Zero-polling WebSocket gateway subscribers clone this receiver directly.
    pub market_data_broadcast_tx: Option<tokio::sync::broadcast::Sender<vyapaar_core::Tick>>,
}

impl AppState {
    /// Creates state from any pair of port implementations.
    pub fn new(
        instruments: Arc<dyn InstrumentMaster>,
        bars: Arc<dyn BarReader>,
        quotes: Arc<dyn QuoteReader>,
        depth: Arc<dyn DepthReader>,
    ) -> Self {
        let mut engine = RiskEngine::default();
        engine.register_algo(crate::orders::REST_ALGO_ID);
        Self {
            instruments,
            bars,
            quotes,
            depth,
            strategies: Arc::default(),
            runs: None,
            profiles: None,
            portfolios: None,
            operator_journal: None,
            account_currency: Currency::Inr,
            // `vyapaar_runtime_api::TradingState` has no `Default` (ADR 0018 enumerates three
            // operator states with no "untouched" one); `Active` is the sensible start.
            trading_state: Arc::new(RwLock::new(TradingState::Active)),
            risk_engine: Arc::new(Mutex::new(engine)),
            approval_queue: None,
            orders: OrderLedger::default(),
            exec: crate::exec::ExecGateway::default_sim(),
            live: None,
            ticks: None,
            execution: vyapaar_runtime_config::ExecutionConfig::default(),
            bar_ingest: Arc::default(),
            market_data_broadcast_tx: None,
        }
    }

    /// Attach the live market data broadcast channel from MarketDataActor / Supervisor.
    pub fn with_market_data_broadcast(
        mut self,
        tx: tokio::sync::broadcast::Sender<vyapaar_core::Tick>,
    ) -> Self {
        self.market_data_broadcast_tx = Some(tx);
        self
    }

    /// Subscribe to the live market data broadcast stream if attached.
    pub fn subscribe_market_data(
        &self,
    ) -> Option<tokio::sync::broadcast::Receiver<vyapaar_core::Tick>> {
        self.market_data_broadcast_tx
            .as_ref()
            .map(|tx| tx.subscribe())
    }

    /// The writable live-quote store `quotes` reads, if this state has one.
    ///
    /// `from_reader` and `from_parquet_dir` build it; `AppState::new` with caller-supplied
    /// ports has none. The bar pump writes each bar here before advancing the market.
    pub fn live_quotes(&self) -> Option<&Arc<LiveQuoteStore>> {
        self.live.as_ref()
    }

    /// Replaces the strategy catalog.
    pub fn with_strategies(mut self, strategies: StrategyCatalog) -> Self {
        self.strategies = Arc::new(Mutex::new(strategies));
        self
    }

    /// Enables the approval queue for order placements (E5-S4).
    ///
    /// When enabled, `POST /orders` is held in the queue until approved by an operator.
    /// Cancel and modify bypass the queue. `max_size` bounds the queue (default 100).
    pub fn with_approval_queue(mut self, max_size: usize) -> Self {
        self.approval_queue = Some(Arc::new(ApprovalQueue::new(max_size)));
        self
    }

    /// Replaces the embedded execution simulator with a fresh one (clears whatever it was
    /// tracking as working orders and simulated holdings).
    pub fn with_execution_sim(mut self, sim: crate::NextOpenSim) -> Self {
        self.exec = crate::exec::ExecGateway::new(sim);
        self
    }

    /// Advances the market: feeds `bars` (ascending `ts_event`) to the embedded simulator.
    ///
    /// Market orders submitted earlier fill at the open of their instrument's first bar in a
    /// later session, and each fill, rejection or cancel updates the matching ledger row (so
    /// [`OrderLedger::position`] follows). This is the hook a live bar feed calls; event time is
    /// the bars' `ts_event`, never the wall clock. An out-of-order bar is an error.
    pub fn advance_market(&self, bars: &[vyapaar_core::Bar]) -> Result<(), ErrorDetail> {
        self.exec.advance(&self.orders, bars)
    }

    /// Sets the operator trading state the write routes evaluate against.
    ///
    /// This seeds the state: the cell is replaced, so call it while building the state, before
    /// cloning it. At runtime use [`AppState::set_trading_state`].
    pub fn with_trading_state(mut self, state: TradingState) -> Self {
        self.trading_state = Arc::new(RwLock::new(state));
        self
    }

    /// The operator trading state in force right now.
    pub fn trading_state(&self) -> TradingState {
        *self
            .trading_state
            .read()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Sets the operator trading state for every clone of this state and returns the previous
    /// one. Any transition is allowed; setting the current state again changes nothing.
    pub fn set_trading_state(&self, state: TradingState) -> TradingState {
        let mut guard = self
            .trading_state
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        std::mem::replace(&mut *guard, state)
    }

    /// Sets the risk limits the order routes enforce (ADR 0018), e.g. `price_collar_bps`.
    pub fn with_risk_limits(self, limits: vyapaar_risk::RiskLimits) -> Self {
        self.risk_engine
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .limits = limits;
        self
    }

    /// Sets the tick source L2 backtests read (call before [`AppState::with_journals_dir`]).
    #[must_use]
    pub fn with_tick_reader(
        mut self,
        ticks: Arc<dyn vyapaar_runtime_feed::ports::TickReader>,
    ) -> Self {
        self.ticks = Some(ticks);
        self
    }

    /// Sets the execution config (fill model) the backtest executor runs with (call before
    /// [`AppState::with_journals_dir`]). `fill_model = l2` needs [`AppState::with_tick_reader`].
    #[must_use]
    pub fn with_execution(mut self, execution: vyapaar_runtime_config::ExecutionConfig) -> Self {
        self.execution = execution;
        self
    }

    /// Serves runs from `service` (tests inject a deterministic executor this way).
    pub fn with_run_service(mut self, service: RunService) -> Self {
        self.runs = Some(Arc::new(service));
        self
    }

    /// Starts the run service over the journals root `dir`: recovers and prunes it (blocking
    /// file I/O, once), then runs backtests over this state's bar reader and strategy catalog.
    ///
    /// Call it once at start-up, never from a request handler. `AppState::default()` has no
    /// journals root and so scans nothing.
    pub fn with_journals_dir(
        mut self,
        dir: &Path,
        config: RunServiceConfig,
    ) -> Result<Self, ErrorDetail> {
        let registry = StrategyRegistry::builtin();
        // `BacktestExecutor` is typed over the canonical, zero-I/O `vyapaar_core::BarReader`/
        // `TickReader` (ADR 0018 decision 7's port vocabulary), not the async `vyapaar-feed`
        // ports `self.bars`/`self.ticks` actually are (`market.rs`/`screener.rs` still read
        // bars through those, with `.await`, so those fields cannot change type). The
        // `SyncBarReader`/`SyncTickReader` adapters bridge the two by blocking on the async
        // call: the run workers are plain threads (see `service.rs`) and the reads are
        // in-memory, so blocking is safe.
        let bars: Arc<dyn vyapaar_core::BarReader + Send + Sync> = Arc::new(SyncBarReader {
            inner: self.bars.clone(),
        });
        let mut executor =
            BacktestExecutor::new(bars, registry.clone()).with_execution(self.execution.clone());
        if let Some(ticks) = &self.ticks {
            let ticks: Arc<dyn vyapaar_core::TickReader + Send + Sync> = Arc::new(SyncTickReader {
                inner: ticks.clone(),
            });
            executor = executor.with_tick_reader(ticks);
        }
        self.account_currency = executor.account_currency()?;
        let service = RunService::start(
            Arc::new(RunStore::system(dir.to_path_buf())),
            Arc::new(executor),
            self.strategies.clone(),
            registry,
            config,
        );
        self.runs = Some(Arc::new(service));
        Ok(self)
    }

    /// Serves `/profile` from one JSON file per user under `dir` (created on first write).
    pub fn with_profiles_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.profiles = Some(Arc::new(ProfileStore::new(dir)));
        self
    }

    /// Serves `/portfolio` from one JSON file per user under `dir` (created on first write).
    ///
    /// Run one `vyapaar serve` per portfolios directory: the store's lock is in-process only
    /// (ADR 0032 §5).
    pub fn with_portfolios_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.portfolios = Some(Arc::new(PortfolioStore::new(dir)));
        self
    }

    /// Appends one NDJSON record per `PUT /trading-state` change to `path` (created on first
    /// change). Off by default. The journal is a record, not a source of truth: the state is
    /// never restored from it at start-up (ADR 0015, explicit runtime).
    pub fn with_operator_journal(mut self, path: impl Into<PathBuf>) -> Self {
        self.operator_journal = Some(path.into());
        self
    }

    /// Graceful shutdown of the run service (ADR 0017 decision 5): stops admitting, cancels
    /// pending runs, waits out the grace period. Blocking; `None` when there is no service.
    pub fn shutdown_runs(&self) -> Option<crate::ShutdownReport> {
        self.runs.as_ref().map(|runs| runs.shutdown())
    }

    /// Replaces the strategy catalog (for a non-default capacity).
    pub fn with_strategy_catalog(mut self, catalog: StrategyCatalog) -> Self {
        self.strategies = Arc::new(Mutex::new(catalog));
        self
    }

    /// Creates state serving every port from one [`DatasetReader`], except `quotes`, which is
    /// a [`LiveQuoteStore`] over it so a live feed can refresh the latest quote.
    ///
    /// This is the constructor for tests: build a reader over an in-memory
    /// `Dataset` and no file or network is touched.
    pub fn from_reader(reader: DatasetReader) -> Self {
        let reader = Arc::new(reader);
        let live = Arc::new(LiveQuoteStore::new(reader.clone()));
        let mut state = Self::new(reader.clone(), reader.clone(), live.clone(), reader);
        state.live = Some(live);
        state
    }

    /// Creates state serving the `SYMBOL.EXCHANGE.parquet` files in `dir`.
    ///
    /// Blocks on I/O; call it once at start-up. A missing or unreadable
    /// directory is an error rather than an empty catalogue.
    pub fn from_parquet_dir(dir: &Path) -> Result<Self, ReaderError> {
        DatasetReader::from_parquet_dir(dir).map(Self::from_reader)
    }

    /// Creates state serving the `vyapaar-data` bar catalog under `root`
    /// (`{root}/catalog/{TF}/{EXCHANGE}/{SYMBOL}/*.parquet`), every timeframe it holds.
    ///
    /// Bars come from [`CatalogBars`](crate::catalog::CatalogBars); instruments, quotes and
    /// depth from each instrument's most recent slice, with `quotes` a [`LiveQuoteStore`] over
    /// it as in [`AppState::from_reader`]. Blocks on I/O; call it once at start-up.
    pub fn from_catalog_dir(root: &Path) -> Result<Self, crate::CatalogLoadError> {
        let (bars, listing) = crate::catalog::load_catalog(root)?;
        let listing = Arc::new(listing);
        let live = Arc::new(LiveQuoteStore::new(listing.clone()));
        let mut state = Self::new(listing.clone(), Arc::new(bars), live.clone(), listing);
        state.live = Some(live);
        Ok(state)
    }

    /// Whether `root` holds a `vyapaar-data` bar catalog ([`AppState::from_catalog_dir`]).
    pub fn has_catalog(root: &Path) -> bool {
        root.join(crate::catalog::CATALOG_DIR).is_dir()
    }
}

impl Default for AppState {
    /// An empty catalogue: no instruments and no bars.
    fn default() -> Self {
        Self::from_reader(DatasetReader::from_dataset(Default::default()))
    }
}

/// Bridges the async `vyapaar-feed` bar port to the sync [`vyapaar_core::BarReader`] the
/// backtest executor reads through, by blocking on the async call.
///
/// The run workers are plain threads (see `service.rs`), so there is no tokio runtime to
/// borrow; `futures::executor::block_on` drives the in-memory `DatasetReader` read to
/// completion on the calling thread. The feed's `f64`-valued bars are converted to the
/// canonical `Decimal`-valued [`vyapaar_core::Bar`] at this boundary, exactly as
/// `market.rs`'s `GET /bars/{id}` does.
struct SyncBarReader {
    inner: Arc<dyn vyapaar_runtime_feed::ports::BarReader>,
}

impl vyapaar_core::BarReader for SyncBarReader {
    fn read_bars(
        &self,
        req: &vyapaar_core::BarRequest,
    ) -> vyapaar_core::PortResult<Vec<vyapaar_core::Bar>> {
        let feed_req = vyapaar_runtime_feed::ports::BarRequest::new(
            req.instrument.clone(),
            to_feed_spec(req.timeframe),
            Some(UnixNanos::from_datetime(req.from)),
            Some(UnixNanos::from_datetime(req.to)),
        )
        .map_err(feed_port_error)?;
        let bars = futures::executor::block_on(self.inner.read_bars(&feed_req))
            .map_err(feed_port_error)?;
        Ok(bars.into_iter().map(feed_bar_to_core).collect())
    }
}

/// Bridges the async `vyapaar-feed` tick port to the sync [`vyapaar_core::TickReader`] the
/// backtest executor reads through; see [`SyncBarReader`]. Quotes and trades are read
/// separately and merged into one chronological [`vyapaar_core::Tick`] stream.
struct SyncTickReader {
    inner: Arc<dyn vyapaar_runtime_feed::ports::TickReader>,
}

impl vyapaar_core::TickReader for SyncTickReader {
    fn read_ticks(
        &self,
        req: &vyapaar_core::TickRequest,
    ) -> vyapaar_core::PortResult<Vec<vyapaar_core::Tick>> {
        let feed_req = vyapaar_runtime_feed::ports::TickRequest::new(
            req.instrument.clone(),
            Some(UnixNanos::from_datetime(req.from)),
            Some(UnixNanos::from_datetime(req.to)),
        )
        .map_err(feed_port_error)?;
        let quotes = futures::executor::block_on(self.inner.read_quotes(&feed_req))
            .map_err(feed_port_error)?;
        let trades = futures::executor::block_on(self.inner.read_trades(&feed_req))
            .map_err(feed_port_error)?;
        let mut ticks: Vec<vyapaar_core::Tick> = Vec::with_capacity(quotes.len() + trades.len());
        ticks.extend(quotes.into_iter().map(feed_quote_to_core));
        ticks.extend(trades.into_iter().map(feed_trade_to_core));
        ticks.sort_by_key(vyapaar_core::Tick::receive_ts);
        Ok(ticks)
    }
}

/// Maps a canonical [`vyapaar_core::Timeframe`] to the feed's bar specification, the inverse
/// of the executor's `bar_spec` -> `Timeframe` mapping.
fn to_feed_spec(
    timeframe: vyapaar_core::Timeframe,
) -> vyapaar_runtime_feed::wire::BarSpecification {
    use vyapaar_runtime_feed::wire::{BarAggregation, PriceType};
    let (step, aggregation) = match timeframe {
        vyapaar_core::Timeframe::M1 => (1, BarAggregation::Minute),
        vyapaar_core::Timeframe::M5 => (5, BarAggregation::Minute),
        vyapaar_core::Timeframe::M15 => (15, BarAggregation::Minute),
        vyapaar_core::Timeframe::M30 => (30, BarAggregation::Minute),
        vyapaar_core::Timeframe::H1 => (1, BarAggregation::Hour),
        vyapaar_core::Timeframe::D1 => (1, BarAggregation::Day),
        vyapaar_core::Timeframe::W1 => (1, BarAggregation::Week),
    };
    vyapaar_runtime_feed::wire::BarSpecification::new(step, aggregation, PriceType::Last)
}

/// Converts a feed bar (`f64`-valued, dual `UnixNanos` timestamps) to the canonical
/// `Decimal`-valued [`vyapaar_core::Bar`], at the sync-port boundary.
fn feed_bar_to_core(bar: vyapaar_runtime_feed::wire::Bar) -> vyapaar_core::Bar {
    vyapaar_core::Bar {
        instrument: bar.bar_type().instrument_id().clone(),
        open: vyapaar_core::Price::new(Decimal::from_f64_retain(bar.open()).unwrap_or_default()),
        high: vyapaar_core::Price::new(Decimal::from_f64_retain(bar.high()).unwrap_or_default()),
        low: vyapaar_core::Price::new(Decimal::from_f64_retain(bar.low()).unwrap_or_default()),
        close: vyapaar_core::Price::new(Decimal::from_f64_retain(bar.close()).unwrap_or_default()),
        volume: vyapaar_core::Quantity::new(
            Decimal::from_f64_retain(bar.volume()).unwrap_or_default(),
        ),
        timestamp: bar.ts_event().to_datetime(),
    }
}

/// Converts a feed quote tick to the canonical [`vyapaar_core::Tick::Quote`].
fn feed_quote_to_core(quote: vyapaar_runtime_feed::wire::QuoteTick) -> vyapaar_core::Tick {
    vyapaar_core::Tick::Quote {
        instrument: quote.instrument_id().clone(),
        bid: vyapaar_core::Price::new(
            Decimal::from_f64_retain(quote.bid_price()).unwrap_or_default(),
        ),
        ask: vyapaar_core::Price::new(
            Decimal::from_f64_retain(quote.ask_price()).unwrap_or_default(),
        ),
        bid_qty: vyapaar_core::Quantity::new(
            Decimal::from_f64_retain(quote.bid_size()).unwrap_or_default(),
        ),
        ask_qty: vyapaar_core::Quantity::new(
            Decimal::from_f64_retain(quote.ask_size()).unwrap_or_default(),
        ),
        last_price: None,
        exchange_ts: quote.ts_event().to_datetime(),
        receive_ts: quote.ts_init().to_datetime(),
        band: None,
    }
}

/// Converts a feed trade tick to the canonical [`vyapaar_core::Tick::Ltp`].
fn feed_trade_to_core(trade: vyapaar_runtime_feed::wire::TradeTick) -> vyapaar_core::Tick {
    vyapaar_core::Tick::Ltp {
        instrument: trade.instrument_id().clone(),
        price: vyapaar_core::Price::new(
            Decimal::from_f64_retain(trade.price()).unwrap_or_default(),
        ),
        volume: vyapaar_core::Quantity::new(
            Decimal::from_f64_retain(trade.size()).unwrap_or_default(),
        ),
        exchange_ts: trade.ts_event().to_datetime(),
        receive_ts: trade.ts_init().to_datetime(),
    }
}

/// Maps the feed's async port error onto the canonical [`vyapaar_core::PortError`] the sync
/// executor's error mapping understands.
fn feed_port_error(e: vyapaar_runtime_feed::ports::PortError) -> vyapaar_core::PortError {
    match e {
        vyapaar_runtime_feed::ports::PortError::Unsupported(msg) => {
            vyapaar_core::PortError::Unsupported(msg)
        }
        vyapaar_runtime_feed::ports::PortError::Internal(msg) => {
            vyapaar_core::PortError::CalculationFailed(msg)
        }
        vyapaar_runtime_feed::ports::PortError::Terminal(msg) => {
            vyapaar_core::PortError::ConnectionFailed(msg)
        }
    }
}

impl fmt::Debug for AppState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AppState").finish_non_exhaustive()
    }
}
