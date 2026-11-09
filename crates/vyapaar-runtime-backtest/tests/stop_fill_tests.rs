//! GAP-0002 step 3: overnight gap-through-stop penalties (E2) and conservative
//! same-bar stop ordering (E3) in the backtest fill engine.
//!
//! Spec: vyapaar-runtime/docs/gaps/0002-strategy-traps-pitfalls-and-statistical-validation.md

use chrono::{DateTime, NaiveDate, NaiveTime, TimeZone, Utc};
use rust_decimal_macros::dec;
use std::sync::Arc;
use vyapaar_core::{Bar, Exchange, InstrumentId, OrderSide, Price, Quantity};
use vyapaar_ledger_journal::ProductType;
use vyapaar_risk::{OtrConfig, RiskEngine, RiskLimits};
use vyapaar_runtime_backtest::{BacktestConfig, BacktestEngine, MarketEvent, NseTradingCalendar};
use vyapaar_runtime_strategy::{OrderIntent, Strategy, StrategyContext};

/// Orders the scripted strategy submits on a given bar.
enum Script {
    MarketBuy(u64),
    LimitSell(u64, &'static str),
    StopSell(u64, &'static str),
    StopBuy(u64, &'static str),
    StopLimitSell(u64, &'static str, &'static str),
}

/// Strategy that submits a fixed script of orders on the first two bars it sees.
struct ScriptedStrategy {
    instrument: InstrumentId,
    bar_scripts: Vec<Vec<Script>>,
    bar: usize,
}

impl ScriptedStrategy {
    fn new(instrument: InstrumentId, bar_scripts: Vec<Vec<Script>>) -> Self {
        Self {
            instrument,
            bar_scripts,
            bar: 0,
        }
    }
}

impl Strategy for ScriptedStrategy {
    fn name(&self) -> &str {
        "scripted-stop"
    }

    fn on_bar(
        &mut self,
        ctx: &mut dyn StrategyContext,
        _bar: &Bar,
    ) -> vyapaar_runtime_strategy::Result<()> {
        self.bar += 1;
        let script = self.bar_scripts.get(self.bar - 1);
        let empty: Vec<Script> = Vec::new();
        for step in script.unwrap_or(&empty) {
            let intent = match step {
                Script::MarketBuy(qty) => {
                    OrderIntent::market_buy(self.instrument.clone(), Quantity::from_u64(*qty))
                }
                Script::LimitSell(qty, limit) => OrderIntent::limit_sell(
                    self.instrument.clone(),
                    Quantity::from_u64(*qty),
                    Price::from_rupees(limit).unwrap(),
                ),
                Script::StopSell(qty, trigger) => OrderIntent::stop_sell(
                    self.instrument.clone(),
                    Quantity::from_u64(*qty),
                    Price::from_rupees(trigger).unwrap(),
                ),
                Script::StopBuy(qty, trigger) => OrderIntent::stop_buy(
                    self.instrument.clone(),
                    Quantity::from_u64(*qty),
                    Price::from_rupees(trigger).unwrap(),
                ),
                Script::StopLimitSell(qty, trigger, limit) => OrderIntent::stop_limit_sell(
                    self.instrument.clone(),
                    Quantity::from_u64(*qty),
                    Price::from_rupees(trigger).unwrap(),
                    Price::from_rupees(limit).unwrap(),
                ),
            };
            ctx.submit(intent);
        }
        Ok(())
    }
}

fn trading_day() -> NaiveDate {
    NaiveDate::from_ymd_opt(2025, 1, 15).unwrap()
}

fn ts(day: NaiveDate, h: u32, m: u32) -> DateTime<Utc> {
    Utc.from_utc_datetime(&day.and_time(NaiveTime::from_hms_opt(h, m, 0).unwrap()))
}

fn instrument() -> InstrumentId {
    InstrumentId::new(Exchange::Nse, "INFY")
}

/// Bar with high/low kept one rupee outside the open/close range so that
/// low <= open,close <= high always holds.
fn bar_with(inst: &InstrumentId, open: &str, close: &str, ts_: DateTime<Utc>) -> Bar {
    let open_p = Price::from_rupees(open).unwrap();
    let close_p = Price::from_rupees(close).unwrap();
    let (high_dec, low_dec) = if open_p.0 > close_p.0 {
        (open_p.0 + dec!(1.00), close_p.0 - dec!(1.00))
    } else {
        (close_p.0 + dec!(1.00), open_p.0 - dec!(1.00))
    };
    Bar {
        instrument: inst.clone(),
        open: open_p,
        high: Price::from_rupees(high_dec.to_string().as_str()).unwrap(),
        low: Price::from_rupees(low_dec.to_string().as_str()).unwrap(),
        close: close_p,
        volume: Quantity::from_u64(1_000),
        timestamp: ts_,
    }
}

fn backtest_engine(strategy: ScriptedStrategy) -> BacktestEngine<ScriptedStrategy> {
    let mut risk_engine = RiskEngine::new(RiskLimits::default(), OtrConfig::default());
    // Runner-stamped orders carry the strategy's name as their algo id, so the
    // risk engine must register that exact id (mirrors engine_strategy_runner_tests).
    risk_engine.register_algo(strategy.name());
    BacktestEngine::new(
        strategy,
        risk_engine,
        BacktestConfig {
            starting_cash: dec!(1_000_000.00),
            product_type: ProductType::Delivery,
            ..Default::default()
        },
        Arc::new(NseTradingCalendar::new()),
    )
}

fn sell_fill_prices(result: &vyapaar_runtime_backtest::BacktestResult) -> Vec<Price> {
    result
        .fills
        .iter()
        .filter(|fill| fill.side == OrderSide::Sell)
        .map(|fill| fill.price)
        .collect()
}

fn buy_fill_prices(result: &vyapaar_runtime_backtest::BacktestResult) -> Vec<Price> {
    result
        .fills
        .iter()
        .filter(|fill| fill.side == OrderSide::Buy)
        .map(|fill| fill.price)
        .collect()
}

/// E2: bar opened below the sell-stop trigger (overnight gap) — the fill is
/// penalized to the open price, not the trigger and not the close.
#[test]
fn test_gap_below_sell_stop_fills_at_bar_open() {
    let inst = instrument();
    let strategy = ScriptedStrategy::new(
        inst.clone(),
        vec![
            vec![Script::MarketBuy(10)],
            vec![Script::StopSell(10, "96.00")],
        ],
    );
    let mut engine = backtest_engine(strategy);
    let b1 = bar_with(&inst, "100.00", "100.00", ts(trading_day(), 9, 20));
    let b2 = bar_with(&inst, "94.00", "95.00", ts(trading_day(), 9, 25));

    let result = engine.run_events(vec![MarketEvent::Bar(b1), MarketEvent::Bar(b2)]);

    assert_eq!(result.fills.len(), 2);
    let sells = sell_fill_prices(&result);
    assert_eq!(sells.len(), 1);
    assert_eq!(sells[0], Price::from_rupees("94.00").unwrap());
}

/// E2: stop triggered intra-bar with no gap through the open — the fill is the
/// trigger price (never better than the trigger).
#[test]
fn test_sell_stop_without_gap_fills_at_trigger() {
    let inst = instrument();
    let strategy = ScriptedStrategy::new(
        inst.clone(),
        vec![
            vec![Script::MarketBuy(10)],
            vec![Script::StopSell(10, "96.00")],
        ],
    );
    let mut engine = backtest_engine(strategy);
    let b1 = bar_with(&inst, "100.00", "100.00", ts(trading_day(), 9, 20));
    let b2 = bar_with(&inst, "98.00", "95.00", ts(trading_day(), 9, 25));

    let result = engine.run_events(vec![MarketEvent::Bar(b1), MarketEvent::Bar(b2)]);

    assert_eq!(result.fills.len(), 2);
    let sells = sell_fill_prices(&result);
    assert_eq!(sells.len(), 1);
    assert_eq!(sells[0], Price::from_rupees("96.00").unwrap());
}

/// E2: buy stop triggered while the bar opened above the trigger — the fill is
/// the (worse) open price.
#[test]
fn test_gap_above_buy_stop_fills_at_bar_open() {
    let inst = instrument();
    let strategy = ScriptedStrategy::new(
        inst.clone(),
        vec![vec![], vec![Script::StopBuy(10, "104.00")]],
    );
    let mut engine = backtest_engine(strategy);
    let b1 = bar_with(&inst, "100.00", "100.00", ts(trading_day(), 9, 20));
    let b2 = bar_with(&inst, "106.00", "105.00", ts(trading_day(), 9, 25));

    let result = engine.run_events(vec![MarketEvent::Bar(b1), MarketEvent::Bar(b2)]);

    assert_eq!(result.fills.len(), 1);
    let buys = buy_fill_prices(&result);
    assert_eq!(buys.len(), 1);
    assert_eq!(buys[0], Price::from_rupees("106.00").unwrap());
}

/// A stop that never triggered keeps the legacy market-at-close behavior.
#[test]
fn test_untriggered_stop_keeps_market_fill() {
    let inst = instrument();
    let strategy = ScriptedStrategy::new(
        inst.clone(),
        vec![
            vec![Script::MarketBuy(10)],
            vec![Script::StopSell(10, "90.00")],
        ],
    );
    let mut engine = backtest_engine(strategy);
    let b1 = bar_with(&inst, "100.00", "100.00", ts(trading_day(), 9, 20));
    let b2 = bar_with(&inst, "98.00", "96.00", ts(trading_day(), 9, 25));

    let result = engine.run_events(vec![MarketEvent::Bar(b1), MarketEvent::Bar(b2)]);

    let sells = sell_fill_prices(&result);
    assert_eq!(sells.len(), 1);
    assert_eq!(sells[0], Price::from_rupees("96.00").unwrap());
}

/// E3: when a target-exit limit and a stop for the same instrument are
/// dispatched in one bar, the stop fills first, then the limit.
#[test]
fn test_same_bar_stop_executes_before_target() {
    let inst = instrument();
    let strategy = ScriptedStrategy::new(
        inst.clone(),
        vec![
            vec![Script::MarketBuy(20)],
            vec![
                Script::LimitSell(10, "105.50"),
                Script::StopSell(10, "96.00"),
            ],
        ],
    );
    let mut engine = backtest_engine(strategy);
    let b1 = bar_with(&inst, "100.00", "100.00", ts(trading_day(), 9, 20));
    let b2 = bar_with(&inst, "98.00", "95.00", ts(trading_day(), 9, 25));

    let result = engine.run_events(vec![MarketEvent::Bar(b1), MarketEvent::Bar(b2)]);

    let sells = sell_fill_prices(&result);
    assert_eq!(sells.len(), 2);
    // Stop executes first, filling at its trigger (no gap: open 98.00 > 96.00).
    assert_eq!(sells[0], Price::from_rupees("96.00").unwrap());
    // Target limit executes after the stop, at the current price as before.
    assert_eq!(sells[1], Price::from_rupees("95.00").unwrap());
}

/// E2, stop-limit: a gap that opens through the trigger and below the limit is
/// not marketable — no sell fill is produced. The marketable variant fills at
/// the limit price.
#[test]
fn test_stop_limit_sell_gap_not_marketable_then_marketable_variant() {
    let inst = instrument();

    // Variant A: bar opened at 93.00, below both trigger 96.00 and limit 94.00 —
    // the limit is never marketable, so there must be NO sell fill.
    let strategy_a = ScriptedStrategy::new(
        inst.clone(),
        vec![
            vec![Script::MarketBuy(10)],
            vec![Script::StopLimitSell(10, "96.00", "94.00")],
        ],
    );
    let mut engine_a = backtest_engine(strategy_a);
    let a1 = bar_with(&inst, "100.00", "100.00", ts(trading_day(), 9, 20));
    let a2 = bar_with(&inst, "93.00", "93.50", ts(trading_day(), 9, 25));
    let result_a = engine_a.run_events(vec![MarketEvent::Bar(a1), MarketEvent::Bar(a2)]);
    assert_eq!(result_a.fills.len(), 1);
    let sells_a = sell_fill_prices(&result_a);
    assert!(
        sells_a.is_empty(),
        "gap-down through a stop-limit with limit 94.00 above the 93.00 open must not fill"
    );

    // Variant B: crossed via the close 95.00 with no gap below the limit —
    // marketable, fills (conservatively) at the limit price 94.00.
    let strategy_b = ScriptedStrategy::new(
        inst.clone(),
        vec![
            vec![Script::MarketBuy(10)],
            vec![Script::StopLimitSell(10, "96.00", "94.00")],
        ],
    );
    let mut engine_b = backtest_engine(strategy_b);
    let b1 = bar_with(&inst, "100.00", "100.00", ts(trading_day(), 9, 20));
    let b2 = bar_with(&inst, "95.50", "95.00", ts(trading_day(), 9, 25));
    let result_b = engine_b.run_events(vec![MarketEvent::Bar(b1), MarketEvent::Bar(b2)]);
    assert_eq!(result_b.fills.len(), 2);
    let sells_b = sell_fill_prices(&result_b);
    assert_eq!(sells_b.len(), 1);
    assert_eq!(sells_b[0], Price::from_rupees("94.00").unwrap());
}
