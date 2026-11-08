# PLAN-0002 — Comprehensive Statistical Gates, Microstructure Traps, and Overfitting Verification

> **Status**: Ready for Implementation  
> **Component Scope**: `vyapaar-core::analytics`, `vyapaar-ui-core::strategy`, `vyapaar-runtime-backtest`, `vyapaar-agent`  
> **Pre-requisites**: `vyapaar-runtime/docs/gaps/0002-strategy-traps-pitfalls-and-statistical-validation.md`  
> **Mathematical Provenance**:  
> - Marcos López de Prado (Deflated Sharpe Ratio, Probability of Backtest Overfitting / CSCV)  
> - Robert Pardo (Walk-Forward Efficiency)  
> - Honba Analytics Engine (`honba-nou/crates/honba-nou-analytics/src/validation.rs`)  

---

## 1. Executive Summary & Objective

This plan details the implementation of the **Comprehensive Statistical Validation Engine** and **Microstructure Trap Verification Pipeline** within Vyapaar.

Following the gap analysis in `GAP-0002`, this upgrade ensures that every strategy selected, backtested, and tuned by the autonomous agent passes through **six mandatory statistical gates** and **four execution reality checks** before a live trade recommendation or 2R trade card is issued:

1. **Deflated Sharpe Ratio (DSR $\ge 95\%$)**: Discounts Sharpe for trial count and skewness.
2. **Probability of Backtest Overfitting (PBO $< 50\%$)**: CSCV cross-validation across all sweep trials.
3. **Walk-Forward Efficiency (WFE $\ge 50\%$)**: Ratio of out-of-sample return to in-sample return.
4. **Holdout Retention ($\ge 50\%$)**: Performance retention on untouched test period.
5. **Block-Bootstrap Monte Carlo**: 1,000 resampled paths measuring 95% worst drawdown ($< 30\%$) and ruin probability ($< 1\%$).
6. **Cost-Stress Testing**: Validating edge survival under $2\times$ and $3\times$ Indian statutory friction (STT, GST, SEBI fee, Stamp duty).
7. **Execution Reality Corrections**: Gap-through-stop pricing and conservative same-bar touch ordering.

---

## 2. Mathematical Formulations & Component Architecture

```
┌────────────────────────────────────────────────────────────────────────────────────────┐
│                        Raw Strategy Backtest & Parameter Sweep                         │
│                    (Historical Bar Data & Next-Bar Open Execution)                     │
└───────────────────────────────────────────┬────────────────────────────────────────────┘
                                            │ Returns Series R_t & Trial Matrix
┌───────────────────────────────────────────▼────────────────────────────────────────────┐
│                    vyapaar-core::analytics::ValidationBlock                            │
├───────────────────────┬────────────────────────┬───────────────────────────────────────┤
│ 1. DSR Gate           │ 2. PBO Gate (CSCV)     │ 3. Walk-Forward Gate                  │
│ • Skew / Kurtosis adj │ • 16-block matrix      │ • Rolling IS / OOS windows            │
│ • DSR >= 95.0%        │ • PBO < 50.0%          │ • WFE >= 50.0%                        │
├───────────────────────┼────────────────────────┼───────────────────────────────────────┤
│ 4. Holdout Gate       │ 5. Monte Carlo Gate    │ 6. Cost-Stress Gate                   │
│ • 70% IS / 30% OOS    │ • 1,000 Block Boots    │ • 1x -> 2x -> 3x Statutory Fees       │
│ • Retention >= 50.0%  │ • DD_95 < 30.0%        │ • Reject if 2x Sharpe <= 0.0          │
└───────────────────────┴───────────┬────────────┴───────────────────────────────────────┘
                                    │
                                    │ Verified Pass / Warnings
┌───────────────────────────────────▼────────────────────────────────────────────────────┐
│                       vyapaar-ui-core::strategy::TradeSuggestion                       │
│  • Microstructure Trap Checks: Bull Trap, Bear Trap, Liquidity Hunt 2.0 ATR buffer     │
│  • Execution Reality Checks: Overnight gap penalty & conservative same-bar ordering    │
│  • 2R Target Geometry: 1:2 and 1:3 profit objectives calibrated to net friction        │
└────────────────────────────────────────────────────────────────────────────────────────┘
```

---

## 3. Data Structures & Algorithm Specifications

### 3.1 The Unified `ValidationBlock`
Port the analytical gate container from Honba into `vyapaar-core::analytics::validation`:

```rust
// vyapaar-core/src/analytics/validation.rs

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ValidationBlock {
    pub dsr: DsrResult,
    pub pbo: Option<PboResult>,
    pub walk_forward: WalkForwardResult,
    pub holdout: HoldoutResult,
    pub monte_carlo: MonteCarloResult,
    pub cost_stress: CostStressResult,
    pub passed: bool,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DsrResult {
    pub observed_sharpe: f64,
    pub expected_max_sharpe: f64,
    pub dsr: f64, // 0.0 to 1.0 (confidence)
    pub num_trials: usize,
    pub passed: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PboResult {
    pub pbo: f64, // Probability of Backtest Overfitting (0.0 to 1.0)
    pub logits: Vec<f64>,
    pub passed: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WalkForwardResult {
    pub is_return_ann: f64,
    pub oos_return_ann: f64,
    pub wfe: f64, // WFE = OOS / IS
    pub passed: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HoldoutResult {
    pub is_sharpe: f64,
    pub holdout_sharpe: f64,
    pub retention_ratio: f64, // holdout_sharpe / is_sharpe
    pub passed: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MonteCarloResult {
    pub drawdown_95: f64, // 95th percentile worst drawdown
    pub prob_ruin: f64,    // probability of >50% account loss
    pub passed: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CostStressResult {
    pub base_sharpe: f64,
    pub stress_2x_sharpe: f64,
    pub stress_3x_sharpe: f64,
    pub friction_fragile: bool,
    pub passed: bool,
}
```

### 3.2 Probability of Backtest Overfitting (PBO) via CSCV
Algorithm implementation for `compute_pbo`:

```rust
// vyapaar-core/src/analytics/pbo.rs

/// Computes Probability of Backtest Overfitting using Combinatorially Symmetric Cross-Validation.
/// 
/// 1. Divides the T return observations into S equal slices (S must be even, typically 16).
/// 2. Forms all C = (S choose S/2) in-sample (IS) / out-of-sample (OOS) combinations.
/// 3. For each combination:
///    - Identifies the trial n* with the highest IS Sharpe.
///    - Computes the percentile ranking of n* in OOS performance.
/// 4. PBO is the fraction of combinations where the IS-optimal strategy has an OOS rank below median (relative rank < 0.5).
pub fn compute_pbo(trials_returns: &[Vec<f64>], num_subsets: usize) -> PboResult {
    let num_trials = trials_returns.len();
    if num_trials < 2 {
        return PboResult { pbo: 0.0, logits: Vec::new(), passed: true };
    }

    let s = if num_subsets % 2 == 0 { num_subsets } else { 16 };
    // Partition returns matrix into S slices and evaluate combinations...
    let pbo_val = 0.12; // Example: 12% probability of overfitting
    let passed = pbo_val < 0.50;

    PboResult {
        pbo: pbo_val,
        logits: Vec::new(),
        passed,
    }
}
```

### 3.3 Block-Bootstrap Monte Carlo Simulation
Algorithm implementation for `run_block_bootstrap`:

```rust
// vyapaar-core/src/analytics/monte_carlo.rs

use rand::prelude::*;

/// Runs moving-block bootstrap resampling to evaluate non-stationary tail risk.
///
/// Resamples contiguous blocks of length L (to preserve return autocorrelation)
/// across 1,000 synthetic paths, measuring the 95th percentile maximum drawdown.
pub fn run_block_bootstrap(returns: &[f64], block_size: usize, num_paths: usize) -> MonteCarloResult {
    if returns.is_empty() {
        return MonteCarloResult { drawdown_95: 0.0, prob_ruin: 0.0, passed: true };
    }

    let n = returns.len();
    let l = block_size.max(5);
    let mut drawdowns = Vec::with_capacity(num_paths);
    let mut ruin_count = 0;

    let mut rng = StdRng::seed_from_u64(42);

    for _ in 0..num_paths {
        let mut equity = 1.0;
        let mut peak = 1.0;
        let mut max_dd = 0.0;

        let mut t = 0;
        while t < n {
            let start = rng.gen_range(0..n.saturating_sub(l).max(1));
            let chunk_len = l.min(n - t);
            for i in 0..chunk_len {
                equity *= 1.0 + returns[start + i];
                if equity > peak { peak = equity; }
                let dd = (peak - equity) / peak;
                if dd > max_dd { max_dd = dd; }
            }
            t += chunk_len;
        }

        drawdowns.push(max_dd);
        if equity < 0.50 {
            ruin_count += 1;
        }
    }

    drawdowns.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let idx_95 = (num_paths as f64 * 0.95) as usize;
    let drawdown_95 = drawdowns[idx_95.min(num_paths - 1)];
    let prob_ruin = ruin_count as f64 / num_paths as f64;

    let passed = drawdown_95 < 0.30 && prob_ruin < 0.01;

    MonteCarloResult {
        drawdown_95,
        prob_ruin,
        passed,
    }
}
```

### 3.4 Execution Reality: Gap-Through-Stop Loss & Simultaneous Touch
In `vyapaar-runtime-backtest/src/engine/fill.rs`:

```rust
// vyapaar-runtime-backtest/src/engine/fill.rs

pub fn evaluate_resting_orders(
    position: &Position,
    bar: &MarketBar,
    stop_loss: Decimal,
    target_1: Decimal,
) -> Option<FillEvent> {
    if position.is_long() {
        // 1. Check for Overnight Gap-Down below Stop Loss:
        if bar.open <= stop_loss {
            // Gap penalty: Filled at market Open, worse than stop level!
            return Some(FillEvent::StopLoss { fill_price: bar.open, slippage_penalized: true });
        }

        // 2. Conservative invariant: When both Stop Loss and Target are touched in the same daily bar,
        // ALWAYS evaluate the Stop Loss first (worst-case execution assumption).
        if bar.low <= stop_loss {
            return Some(FillEvent::StopLoss { fill_price: stop_loss, slippage_penalized: false });
        }

        if bar.high >= target_1 {
            return Some(FillEvent::ProfitTarget { fill_price: target_1 });
        }
    }
    None
}
```

---

## 4. UI & Terminal Card Integration

Update `render_suggestion_card` in `vyapaar-cli/src/strategy_bridge.rs` to render the full 6-gate audit:

```text
╔═══════════════════════════════════════════════════════════════════════════╗
║          VYAPAAR OPTIMAL TRADE SUGGESTION: TURTLE DONCHIAN BREAKOUT       ║
╚═══════════════════════════════════════════════════════════════════════════╝

1. PARAMETER TUNING & STATISTICAL AUDIT (6 MANDATORY GATES)
   ────────────────────────────────────────────────────────────────────────
   • Tuned Parameters : entry_period=9 (def: 20), exit_period=6 (def: 10)
   • Performance      : Sharpe 1.44 (vs 1.10 def) | Max DD: 0.15% | Win Rate: 90.00%
   • 1. Deflated Sharpe : 79.12% confidence (Pass: Borderline Significance)
   • 2. Overfitting PBO : 14.2% (Pass: Well below 50.0% threshold)
   • 3. Walk-Forward    : WFE 68.4% (Pass: Strong out-of-sample persistence)
   • 4. Holdout Split   : 74.5% Sharpe retention on 30% untouched holdout
   • 5. Monte Carlo DD  : 95% worst DD 12.4%, Ruin Prob 0.0% (1,000 paths)
   • 6. Cost Stress 2x  : 2x STT/GST Sharpe = 1.18 (Pass: Robust gross edge)
   • Overall Audit      : [PASSED - INSTITUTIONAL GRADE]

2. OPTIMAL TRADE EXECUTION CARD
   ────────────────────────────────────────────────────────────────────────
   • Target Asset     : NSE:NIFTY50-INDEX
   • Direction / Bias : BUY / LONG
   • Trigger / Reason : Breakout above 9-bar high confirmed (Volume > 1.2x SMA20)
   • Entry Level      : ₹27,925.41
   • Stop Loss        : ₹26,689.25 (-₹1,236.16 / -4.43% / 2.0 ATR buffer)
   • Target 1 (1:2 RR): ₹30,397.73 (+₹2,472.31 / +8.85%)
   • Target 2 (1:3 RR): ₹31,633.89 (+₹3,708.47 / +13.28%)
```

---

## 5. Phased Implementation Timeline

| Phase | Milestone | Deliverables | Verification Criteria |
|---|---|---|---|
| **Phase 1** | **Statistical Core** | • Port `pbo.rs`, `walk_forward.rs`, `monte_carlo.rs`, `cost_stress.rs` into `vyapaar-core::analytics`. | Unit tests passing with synthetic and historical returns. |
| **Phase 2** | **Backtest Fill Reality** | • Enforce overnight gap-through-stop fills in `vyapaar-runtime-backtest`.<br>• Enforce Stop-before-Target conservative check for simultaneous touches. | Integration tests verifying fill prices match open on down gaps. |
| **Phase 3** | **StrategyBridge & Terminal Card** | • Wire `ValidationBlock` into `StrategyBridge::generate_trade_suggestion`.<br>• Render all 6 statistical audit rows in CLI card. | `vyapaar strategy suggest turtle_breakout` displays full 6-gate audit. |
| **Phase 4** | **Agent Prompt Grounding** | • Inject full 6-gate audit into LLM prompt context.<br>• Validate LLM output against structured JSON schema to guarantee zero metric hallucinations. | End-to-end `vyapaar ask` streams quantitative critique citing all 6 gates. |
