//! The [`RunExecutor`] behind `POST /backtests` and `POST /sweeps`.
//!
//! # How a run executes
//!
//! [`BacktestExecutor::execute`] drives the real, synchronous event kernel in
//! `vyapaar-runtime-backtest` ([`BacktestEngine`]): it reads the run's bars
//! through the canonical [`vyapaar_core::BarReader`] port, builds the strategy
//! from the [`StrategyRegistry`], and runs the engine over the bars. The
//! engine drives the same [`vyapaar_runtime_strategy::StrategyRunner`] the live
//! and paper paths use, so a backtest gets the identical pre-trade risk
//! gating, warm-up-bar suppression, statutory fee computation
//! (`vyapaar_ledger_journal::TaxCalculator`), T+1 settlement timing and
//! position/PnL tracking as every other execution mode.
//!
//! A `fill_model = l2` run additionally reads the configured tick source
//! ([`BacktestExecutor::with_tick_reader`]) and interleaves the ticks with the
//! bars so the engine's L2 order-book simulation can match orders against
//! quote/trade depth. An L2 run without a tick source fails with the stable
//! `l2_requires_ticks` reason; a tick source that holds no tick data fails the
//! same way.
//!
//! Every fill the engine books is appended to the run journal as one
//! [`Event::Fill`] record carrying the wire [`Trade`] (the transport-owned view of
//! `vyapaar_core::FillReport`, which names the instrument `instrument_id`), so
//! `GET /backtests/{id}/journal` and `GET /journals/{id}` aggregate the run's
//! trades from the journal.
//!
//! A sweep ([`ResolvedRequest::Sweep`]) builds the Cartesian parameter grid
//! from the request's `params` ranges, runs one backtest per trial with the
//! strategy constructed from the trial's parameters, ranks the trials
//! best-first by net PnL, and reports the ranking plus the best parameter set.
//!
//! The flat `commission_per_fill` and `slippage_bps` knobs are accepted for
//! API compatibility and reported in the run's assumptions; the engine's
//! statutory cost model (`TaxCalculator`) is the authoritative cost
//! calculation, and both knobs are zero in every current test.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use rust_decimal::prelude::ToPrimitive;
use serde_json::{Value, json};
use vyapaar_core::rust_decimal::Decimal;
use vyapaar_core::{
    BarReader, Currency, InstrumentId, PortError, Quantity, TickReader, Timeframe, TradingCalendar,
};
use vyapaar_ledger_journal::ProductType;
use vyapaar_risk::RiskEngine;
use vyapaar_runtime_api::wire::BarAggregation;
use vyapaar_runtime_api::{
    BacktestMetrics, CacheKey, CachedResult, ErrorCode, ErrorDetail, Event, JournalWriter, Message,
    ResolvedBacktest, ResolvedRequest, ResultCache, RunExecutor, RunJob, RunOutcome,
    SweepReportResponse, parse_instrument_id, parse_timeframe, unknown_strategy,
};
use vyapaar_runtime_backtest::calendar::NseTradingCalendar;
use vyapaar_runtime_backtest::engine::{BacktestConfig, BacktestEngine, BacktestResult};
use vyapaar_runtime_backtest::events::MarketEvent;
use vyapaar_runtime_backtest::sim::latency::NetworkLatency;
use vyapaar_runtime_backtest::sim::mis::MisSquareOffManager;
use vyapaar_runtime_backtest::sim::slippage::SlippageModel;
use vyapaar_runtime_config::{AccountConfig, ExecutionConfig, FillModel};
use vyapaar_runtime_strategy::{DynStrategy, SmaCrossover, Strategy};

use crate::registry::StrategyRegistry;

/// Runs a resolved backtest or sweep job on the real `vyapaar-runtime-backtest` engine.
///
/// The engine is generic over the strategy it drives; the registry hands out
/// type-erased `Box<dyn Strategy>` values, so each run wraps its strategy in a
/// [`DynStrategy`] before constructing the engine.
pub struct BacktestExecutor {
    bars: Arc<dyn BarReader + Send + Sync>,
    registry: StrategyRegistry,
    account: AccountConfig,
    execution: ExecutionConfig,
    #[allow(dead_code)]
    cache: Option<ResultCache>,
    ticks: Option<Arc<dyn TickReader + Send + Sync>>,
}

impl BacktestExecutor {
    /// An executor over `bars` running the strategies in `registry`, with the default
    /// account and execution config.
    pub fn new(bars: Arc<dyn BarReader + Send + Sync>, registry: StrategyRegistry) -> Self {
        Self {
            bars,
            registry,
            account: AccountConfig::default(),
            execution: ExecutionConfig::default(),
            cache: None,
            ticks: None,
        }
    }

    /// Overrides the account config (currency).
    #[must_use]
    pub fn with_account(mut self, account: AccountConfig) -> Self {
        self.account = account;
        self
    }

    /// Overrides the execution config (fill model).
    #[must_use]
    pub fn with_execution(mut self, execution: ExecutionConfig) -> Self {
        self.execution = execution;
        self
    }

    /// Attaches the quote/trade tick source `fill_model = l2` runs would match against.
    #[must_use]
    pub fn with_tick_reader(mut self, ticks: Arc<dyn TickReader + Send + Sync>) -> Self {
        self.ticks = Some(ticks);
        self
    }

    /// Attaches a result cache to this executor (ADR 0020, E4-S1).
    #[must_use]
    pub fn with_cache(mut self, cache: ResultCache) -> Self {
        self.cache = Some(cache);
        self
    }

    pub(crate) fn account_currency(&self) -> Result<Currency, ErrorDetail> {
        match self.account.currency.as_str() {
            "INR" => Ok(Currency::Inr),
            other => Err(ErrorDetail::new(
                ErrorCode::NotImplemented,
                format!("account currency {other:?} is not supported"),
            )),
        }
    }

    /// The fill model a run executes under: the request's `fill_model` when it names one,
    /// otherwise the server's configured model.
    fn effective_fill_model(&self, request: Option<&str>) -> Result<FillModel, ErrorDetail> {
        match request {
            Some(name) => fill_model_from_name(name),
            None => Ok(self.execution.fill_model),
        }
    }

    /// Runs one backtest over `instrument`'s bars in `[start, end)` at `bar_spec` and returns
    /// the engine's full result.
    //
    // Ported from honba-ya; the eight run parameters stay explicit for the two call sites
    // (single run + sweep trial), so allow the argument-count lint rather than hiding the
    // knobs behind a struct.
    #[allow(clippy::too_many_arguments)]
    fn run_engine(
        &self,
        strategy: Box<dyn Strategy>,
        instrument: &InstrumentId,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        bar_spec: &str,
        initial_capital: Decimal,
        seed: u64,
        fill_model: FillModel,
    ) -> Result<BacktestResult, ErrorDetail> {
        let timeframe = timeframe_from_bar_spec(bar_spec)?;
        let bars = self
            .bars
            .read_bars(&vyapaar_core::BarRequest {
                instrument: instrument.clone(),
                timeframe,
                from: start,
                to: end,
            })
            .map_err(port_error)?;
        if bars.is_empty() {
            return Err(
                ErrorDetail::new(ErrorCode::NotFound, "no bars in the requested range")
                    .with_context(json!({"reason": "no_data"})),
            );
        }

        let calendar: Arc<dyn TradingCalendar> = Arc::new(NseTradingCalendar::new());
        let (config, events) = match fill_model {
            FillModel::L2 => {
                let ticks = self.ticks.as_ref().ok_or_else(l2_requires_ticks)?;
                let tick_req = vyapaar_core::TickRequest {
                    instrument: instrument.clone(),
                    from: start,
                    to: end,
                };
                let ticks = match ticks.read_ticks(&tick_req) {
                    Ok(ticks) => ticks,
                    // A tick source that holds no tick data is the same refusal as no source.
                    Err(PortError::Unsupported(_)) => return Err(l2_requires_ticks()),
                    Err(e) => return Err(port_error(e)),
                };
                let mut events: Vec<MarketEvent> = bars.into_iter().map(MarketEvent::Bar).collect();
                events.extend(ticks.into_iter().map(MarketEvent::Tick));
                // Chronological replay; a tick and a bar at the same instant keep the tick
                // first so the book is populated before the bar's synthetic depth is added.
                events.sort_by(|a, b| {
                    a.timestamp()
                        .cmp(&b.timestamp())
                        .then_with(|| match (a, b) {
                            (MarketEvent::Tick(_), MarketEvent::Bar(_)) => Ordering::Less,
                            (MarketEvent::Bar(_), MarketEvent::Tick(_)) => Ordering::Greater,
                            _ => Ordering::Equal,
                        })
                });
                let config = BacktestConfig::with_l2_simulation(
                    initial_capital,
                    ProductType::Delivery,
                    NetworkLatency::new(0, 0, seed),
                    SlippageModel::new(),
                    MisSquareOffManager::new(calendar.clone()),
                );
                (config, events)
            }
            _ => {
                let config = BacktestConfig {
                    starting_cash: initial_capital,
                    ..BacktestConfig::default()
                };
                let events: Vec<MarketEvent> = bars.into_iter().map(MarketEvent::Bar).collect();
                (config, events)
            }
        };

        // The engine runs the strategy under its own SEBI algo id: register it so the
        // pre-trade gate's AlgoNotRegistered check cannot refuse every order.
        let algo_id = strategy.name().to_string();
        let mut risk_engine = RiskEngine::default();
        risk_engine.register_algo(algo_id);
        let mut engine =
            BacktestEngine::new(DynStrategy::new(strategy), risk_engine, config, calendar);
        Ok(engine.run_events(events))
    }

    /// Runs the parameter sweep: one backtest per grid point, ranked best-first by net PnL.
    fn run_sweep(
        &self,
        spec: &vyapaar_runtime_api::ResolvedSweep,
        instrument: &InstrumentId,
        strategy_name: &str,
    ) -> Result<SweepReportResponse, ErrorDetail> {
        let mut grid = parameter_grid_from_value(&spec.params)
            .map_err(|e| invalid("params", "invalid_range", e))?;
        if grid.is_empty() {
            return Err(invalid(
                "params",
                "empty_grid",
                "the parameter ranges produced no trials",
            ));
        }
        grid.truncate(spec.trials as usize);

        let mut ranked: Vec<(f64, Value)> = Vec::with_capacity(grid.len());
        for trial in &grid {
            let strategy = self.trial_strategy(strategy_name, instrument, spec.seed, trial)?;
            let result = self.run_engine(
                strategy,
                instrument,
                spec.start,
                spec.end,
                &spec.bar_spec,
                spec.initial_capital,
                spec.seed,
                FillModel::BarFill,
            )?;
            let fitness = result.net_pnl.to_f64().unwrap_or(0.0);
            let mut entry: serde_json::Map<String, Value> = trial.clone().into_iter().collect();
            entry.insert("fitness".to_string(), json!(fitness));
            ranked.push((fitness, Value::Object(entry)));
        }
        ranked.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(Ordering::Equal));
        let best = ranked.first().map(|(_, entry)| entry.clone());
        Ok(SweepReportResponse {
            ranked: ranked.into_iter().map(|(_, entry)| entry).collect(),
            best,
            validation: None,
        })
    }

    /// Builds the strategy a sweep trial runs: `sma_crossover` reads its `fast`/`slow` periods
    /// from the trial's parameters; every other registered strategy is built by the registry.
    fn trial_strategy(
        &self,
        name: &str,
        instrument: &InstrumentId,
        seed: u64,
        trial: &BTreeMap<String, Value>,
    ) -> Result<Box<dyn Strategy>, ErrorDetail> {
        if name == "sma_crossover" || name == "sma_cross" {
            let fast = trial.get("fast").and_then(Value::as_u64).ok_or_else(|| {
                invalid(
                    "params",
                    "missing_fast",
                    "sma_crossover needs a fast period",
                )
            })?;
            let slow = trial.get("slow").and_then(Value::as_u64).ok_or_else(|| {
                invalid(
                    "params",
                    "missing_slow",
                    "sma_crossover needs a slow period",
                )
            })?;
            if fast == 0 || slow == 0 || fast >= slow {
                return Err(invalid(
                    "params",
                    "invalid_sma_periods",
                    "fast must be positive and less than slow",
                ));
            }
            return Ok(Box::new(SmaCrossover::new(
                instrument.clone(),
                fast as usize,
                slow as usize,
                Quantity::from_u64(10),
            )));
        }
        self.registry
            .build(name, instrument, seed)
            .ok_or_else(|| unknown_strategy(name))
    }
}

impl RunExecutor for BacktestExecutor {
    fn execute(
        &self,
        job: RunJob,
        journal: &mut dyn JournalWriter,
    ) -> Result<RunOutcome, ErrorDetail> {
        // The manifest pins the strategy's registered name (a compiled strategy runs only
        // when a Rust strategy carries its name, so this is always a registry name).
        let strategy_name = job.strategy.manifest.name.clone();
        match &job.request {
            ResolvedRequest::Backtest(spec) => {
                let instrument = parse_universe(&spec.universe)?;
                // A cache hit serves the exact metrics and assumptions without simulating or
                // journaling (ADR 0020, E4-S1): the key pins every input that shapes a run.
                let key = backtest_cache_key(&job, &instrument);
                if let Some(cached) = self.cache.as_ref().and_then(|c| c.get(&key)) {
                    return Ok(RunOutcome::Backtest {
                        metrics: cached.metrics,
                        assumptions: cached.assumptions,
                    });
                }
                let strategy = self
                    .registry
                    .build(&strategy_name, &instrument, spec.seed)
                    .ok_or_else(|| unknown_strategy(&strategy_name))?;
                let fill_model = self.effective_fill_model(spec.fill_model.as_deref())?;
                let result = self.run_engine(
                    strategy,
                    &instrument,
                    spec.start,
                    spec.end,
                    &spec.bar_spec,
                    spec.initial_capital,
                    spec.seed,
                    fill_model,
                )?;
                write_fills(journal, &result)?;
                let outcome = RunOutcome::Backtest {
                    metrics: metrics_from_result(&result),
                    assumptions: assumptions(spec, fill_model),
                };
                if let (
                    RunOutcome::Backtest {
                        metrics,
                        assumptions,
                    },
                    Some(cache),
                ) = (&outcome, &self.cache)
                {
                    cache.insert(
                        key,
                        CachedResult {
                            metrics: metrics.clone(),
                            assumptions: assumptions.clone(),
                            journal_path: None,
                        },
                    );
                }
                Ok(outcome)
            }
            ResolvedRequest::Sweep(spec) => {
                let instrument = parse_universe(&spec.universe)?;
                let report = self.run_sweep(spec, &instrument, &strategy_name)?;
                Ok(RunOutcome::Sweep { report })
            }
        }
    }
}

fn invalid(field: &str, reason: &str, message: impl Into<String>) -> ErrorDetail {
    ErrorDetail::new(ErrorCode::ValidationInvalidRequest, message)
        .with_context(json!({"field": field, "reason": reason}))
}

/// Version of the statutory cost model the backtest engine applies, part of every result-cache
/// key (ADR 0020): a cost-model change must invalidate previously cached runs. Bump when the
/// cost calculation changes.
const COST_MODEL_VERSION: &str = "1";

/// The result-cache key of a backtest job (ADR 0020, E4-S1): every input that shapes the
/// outcome — strategy id, compiled manifest, universe, seed, capital, window, bar spec, fill
/// model and the cost knobs — so identical inputs hit the cache and any change misses it. The
/// run id is deliberately absent: the identical rerun must map to the same key.
fn backtest_cache_key(job: &RunJob, instrument: &InstrumentId) -> CacheKey {
    let params = match &job.request {
        ResolvedRequest::Backtest(spec) => json!({
            "strategy": job.strategy_id,
            "manifest_name": job.strategy.manifest.name,
            "source_hash": job.strategy.manifest.source_hash,
            "seed": spec.seed,
            "initial_capital": spec.initial_capital,
            "bar_spec": spec.bar_spec,
            "start": spec.start.to_rfc3339(),
            "end": spec.end.to_rfc3339(),
            "fill_model": spec.fill_model,
            "commission_per_fill": spec.commission_per_fill,
            "slippage_bps": spec.slippage_bps,
        }),
        ResolvedRequest::Sweep(_) => json!({"kind": "sweep"}),
    };
    CacheKey::compute(
        &job.strategy_id,
        &params,
        &instrument.to_string(),
        COST_MODEL_VERSION,
    )
}

/// The stable refusal of an L2 run whose tick source is missing or holds no tick data.
fn l2_requires_ticks() -> ErrorDetail {
    ErrorDetail::new(
        ErrorCode::ValidationInvalidRequest,
        "fill_model l2 requires a tick source",
    )
    .with_context(json!({"reason": "l2_requires_ticks"}))
}

/// Maps a request's `fill_model` name to the config enum, through serde so the accepted
/// spellings are the enum's own (pinned to `vyapaar_runtime_api::FILL_MODELS` by a test).
fn fill_model_from_name(name: &str) -> Result<FillModel, ErrorDetail> {
    serde_json::from_value::<FillModel>(serde_json::Value::String(name.to_string())).map_err(|_| {
        invalid(
            "fill_model",
            "unknown_fill_model",
            format!("unknown fill_model {name:?}"),
        )
    })
}

fn port_error(e: PortError) -> ErrorDetail {
    let code = match e {
        PortError::Unsupported(_) => ErrorCode::NotImplemented,
        PortError::NotFound(_) => ErrorCode::NotFound,
        PortError::Timeout => ErrorCode::Timeout,
        PortError::ConnectionFailed(_) | PortError::CalculationFailed(_) => {
            ErrorCode::InternalError
        }
    };
    ErrorDetail::new(code, format!("bar read failed: {e}"))
        .with_context(json!({"reason": "bar_read"}))
}

/// The one instrument of a universe. A list or a bare symbol is refused.
pub(crate) fn parse_universe(universe: &str) -> Result<InstrumentId, ErrorDetail> {
    let bad = |why: &str| {
        invalid(
            "universe",
            "unsupported_universe",
            format!("universe must be one SYMBOL.EXCHANGE instrument: {why}"),
        )
    };
    let text = universe.trim();
    if text.is_empty() || text.contains(',') || text.contains(char::is_whitespace) {
        return Err(bad("lists are not supported"));
    }
    parse_instrument_id(text).map_err(|_| bad("not SYMBOL.EXCHANGE"))
}

/// Annualisation used for the Sharpe ratio.
const PERIODS_PER_YEAR: f64 = 252.0;

fn assumptions(spec: &ResolvedBacktest, fill_model: FillModel) -> Value {
    let mut not_modelled: Vec<&str> = Vec::new();
    if spec.commission_per_fill == Decimal::ZERO {
        not_modelled.push("transaction_costs");
    }
    if spec.slippage_bps == Decimal::ZERO {
        not_modelled.push("slippage");
    }
    not_modelled.extend([
        "taxes_and_levies",
        "market_impact",
        "partial_fills",
        "order_acknowledgement_latency",
        "mark_to_market_of_open_positions",
        "corporate_actions",
        "market_calendar",
    ]);
    json!({
        "not_modelled": not_modelled,
        "applied": {
            "commission_per_fill": spec.commission_per_fill,
            "slippage_bps": spec.slippage_bps,
        },
        "timing": "market orders fill at the close of the bar on which the strategy decided",
        "fill_model": serde_json::to_value(fill_model).unwrap_or_else(|_| Value::String("bar_fill".into())),
        "metrics_basis": "closed_round_trips",
        "periods_per_year": PERIODS_PER_YEAR
    })
}

/// Converts a JSON parameter object (name -> [low, high, step]) into a Cartesian grid of trial params.
fn parameter_grid_from_value(params_obj: &Value) -> Result<Vec<BTreeMap<String, Value>>, String> {
    let obj = params_obj
        .as_object()
        .ok_or("params must be a JSON object")?;
    let mut space: BTreeMap<String, Vec<Value>> = BTreeMap::new();

    for (name, range_val) in obj {
        let range = range_val
            .as_array()
            .ok_or(format!("{name}: range must be an array"))?;
        if range.len() != 3 {
            return Err(format!("{name}: range must be [low, high, step]"));
        }
        let low = range[0]
            .as_f64()
            .ok_or(format!("{name}: low must be a number"))?;
        let high = range[1]
            .as_f64()
            .ok_or(format!("{name}: high must be a number"))?;
        let step = range[2]
            .as_f64()
            .ok_or(format!("{name}: step must be a number"))?;
        if low > high {
            return Err(format!("{name}: low cannot exceed high"));
        }
        if step <= 0.0 {
            return Err(format!("{name}: step must be positive"));
        }

        let is_int = low.fract() == 0.0 && high.fract() == 0.0 && step.fract() == 0.0;
        let mut values = Vec::new();
        let mut curr = low;
        while curr <= high + step * 1e-6 {
            if is_int {
                values.push(Value::from(curr.round() as i64));
            } else {
                values.push(Value::from((curr * 1e6).round() / 1e6));
            }
            curr += step;
        }
        if values.is_empty() {
            return Err(format!("{name}: range produced no values"));
        }
        space.insert(name.clone(), values);
    }

    if space.is_empty() {
        return Ok(Vec::new());
    }

    let mut grid: Vec<BTreeMap<String, Value>> = vec![BTreeMap::new()];
    for (name, values) in space {
        let mut next = Vec::new();
        for entry in &grid {
            for v in &values {
                let mut updated = entry.clone();
                updated.insert(name.clone(), v.clone());
                next.push(updated);
            }
        }
        grid = next;
    }
    Ok(grid)
}

/// Maps a `bar_spec` text (e.g. `1d`) to the canonical [`Timeframe`] a [`vyapaar_core::BarRequest`]
/// carries. Only the timeframes `vyapaar_core` models are accepted.
fn timeframe_from_bar_spec(bar_spec: &str) -> Result<Timeframe, ErrorDetail> {
    let spec = parse_timeframe(bar_spec)
        .map_err(|e| invalid("bar_spec", "invalid_bar_spec", e.message))?;
    let timeframe = match (spec.step(), spec.aggregation()) {
        (1, BarAggregation::Minute) => Timeframe::M1,
        (5, BarAggregation::Minute) => Timeframe::M5,
        (15, BarAggregation::Minute) => Timeframe::M15,
        (30, BarAggregation::Minute) => Timeframe::M30,
        (1, BarAggregation::Hour) => Timeframe::H1,
        (1, BarAggregation::Day) => Timeframe::D1,
        (1, BarAggregation::Week) => Timeframe::W1,
        _ => {
            return Err(invalid(
                "bar_spec",
                "unsupported_timeframe",
                format!("bar_spec {bar_spec:?} has no core timeframe equivalent"),
            ));
        }
    };
    Ok(timeframe)
}

/// Appends one `Event::Fill` journal record per fill the engine booked, in execution order.
///
/// The wire `Trade` is a distinct transport type (not a bare `FillReport`) so its journal shape
/// names the instrument `instrument_id`; the engine's `FillReport` converts losslessly.
fn write_fills(
    journal: &mut dyn JournalWriter,
    result: &BacktestResult,
) -> Result<(), ErrorDetail> {
    for fill in &result.fills {
        journal.append(&Message::new(
            Event::Fill(fill.clone().into()),
            fill.timestamp,
        ))?;
    }
    Ok(())
}

/// Headline metrics from an engine result. `trades` counts closed round trips (two fills
/// each), matching the journal's one fill record per fill.
fn metrics_from_result(result: &BacktestResult) -> BacktestMetrics {
    let total_return = if result.initial_cash > Decimal::ZERO {
        result.net_pnl / result.initial_cash
    } else {
        Decimal::ZERO
    };
    BacktestMetrics {
        trades: (result.total_trades / 2) as u64,
        net_pnl: result.net_pnl,
        sharpe: sharpe_from_equity_curve(&result.equity_curve),
        max_drawdown: result.max_drawdown_pct / Decimal::from(100),
        total_return,
    }
}

/// Annualized Sharpe ratio from the engine's per-event equity marks: the mean of the
/// per-period returns over their sample standard deviation, annualized by
/// [`PERIODS_PER_YEAR`]. Zero when there are not enough marks to estimate a deviation.
fn sharpe_from_equity_curve(curve: &[(DateTime<Utc>, Decimal)]) -> Decimal {
    if curve.len() < 2 {
        return Decimal::ZERO;
    }
    let mut returns: Vec<f64> = Vec::with_capacity(curve.len() - 1);
    for pair in curve.windows(2) {
        let prev = pair[0].1;
        if prev > Decimal::ZERO {
            returns.push(((pair[1].1 - prev) / prev).to_f64().unwrap_or(0.0));
        }
    }
    if returns.len() < 2 {
        return Decimal::ZERO;
    }
    let n = returns.len() as f64;
    let mean = returns.iter().sum::<f64>() / n;
    let variance = returns.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / (n - 1.0);
    let std = variance.sqrt();
    if std <= 1e-12 {
        return Decimal::ZERO;
    }
    let sharpe = mean / std * PERIODS_PER_YEAR.sqrt();
    Decimal::from_f64_retain(sharpe).unwrap_or_default()
}
