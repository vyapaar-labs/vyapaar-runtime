# GAP-0002 — Strategy Traps, Pitfalls, and Statistical Validation Coverage

> **Status**: Approved / Critical Gap Audit  
> **Component Scope**: `vyapaar-ui-core::strategy`, `vyapaar-runtime-backtest`, `vyapaar-runtime-strategy`, `vyapaar-agent`  
> **Cross-Reference Benchmark**:  
> - Honba Statistical Validation Engine (`honba-nou/crates/honba-nou-analytics/src/validation.rs`)  
> - Honba StockSharp & Backtesting Pitfalls (`honba-examples/examples/stocksharp/03_turtle_breakout.py`, `02_cost_modeling.py`)  
> - Honba Strategy Execution Traps (`honba-examples/examples/strategies/04_risk_management.py` through `14_partial_fill_handling.py`)  
> - Honba AI Critique Pitfalls (`honba-examples/examples/ai_research/07_natural_language_critique.py`)  

---

## 1. Executive Summary

In financial quantitative engineering, strategy backtests frequently produce impressive Sharpe ratios that collapse in live trading due to hidden statistical, execution, and microstructure traps.

**Honba** established a rigorous multi-tier audit framework encompassing **18 distinct traps and pitfalls** across four functional layers:
1. **Tier 1: Statistical & Overfitting Gates** (DSR, PBO, Walk-Forward, Holdout, Monte Carlo, Cost Stress).
2. **Tier 2: Execution & Timing Pitfalls** (Next-Bar Open vs Trading the Close, Gap-Through-Stop, Same-Bar Ambiguity, Shorting Delivery).
3. **Tier 3: Market Microstructure & Regulatory Traps** (STT Leg Asymmetry, Liquidity Hunt Clusters, Bull/Bear Traps, Freeze Limits).
4. **Tier 4: AI & Quantitative Narrative Pitfalls** (Metric Hallucination, Hindsight Bias, Prompt Injection, Selection via Prompting).

This audit evaluates Vyapaar's current implementation against this standard, catalogs what is accounted for, identifies critical missing gates, and specifies the remediation plan.

---

## 2. Comprehensive Traps & Pitfalls Coverage Matrix

```
┌─────────────────────────────────────────────────────────────────────────────────────────────────────────────┐
│                                   TRAPS & PITFALLS AUDIT MATRIX                                             │
├────┬─────────────────────────────┬────────────────────────────────────┬───────────┬────────────────────────┤
│ ID │ Trap / Pitfall Description  │ Honba Reference                    │ Vyapaar   │ Current Status         │
├────┼─────────────────────────────┼────────────────────────────────────┼───────────┼────────────────────────┤
│    │ TIER 1: STATISTICAL & OVERFITTING GATES                                                               │
├────┼─────────────────────────────┼────────────────────────────────────┼───────────┼────────────────────────┤
│ S1 │ Deflated Sharpe Ratio (DSR) │ `validation.rs::compute_dsr`       │ Accounted │ Implemented in bridge  │
│    │ Multiple trial search bias  │ Threshold: >= 95% (or borderline)  │           │ (DSR % & verdict)      │
├────┼─────────────────────────────┼────────────────────────────────────┼───────────┼────────────────────────┤
│ S2 │ Probability of Backtest     │ `validation.rs::compute_pbo`       │ MISSING   │ Not computed across    │
│    │ Overfitting (PBO via CSCV)  │ Combinatorially symmetric CV < 50% │           │ parameter sweeps       │
├────┼─────────────────────────────┼────────────────────────────────────┼───────────┼────────────────────────┤
│ S3 │ Walk-Forward Efficiency     │ `validation.rs::walk_forward`      │ MISSING   │ No OOS/IS annualized   │
│    │ (WFE = OOS / IS Return)     │ Threshold: >= 50.0%                │           │ ratio calculated       │
├────┼─────────────────────────────┼────────────────────────────────────┼───────────┼────────────────────────┤
│ S4 │ Holdout Retention Ratio     │ `validation.rs::holdout`           │ Partial   │ Train/Test in verify,  │
│    │ OOS Sharpe / IS Sharpe      │ 70/30 split; Retention >= 50.0%    │           │ but no retention ratio │
├────┼─────────────────────────────┼────────────────────────────────────┼───────────┼────────────────────────┤
│ S5 │ Block-Bootstrap Monte Carlo │ `validation.rs::monte_carlo`       │ MISSING   │ No resampled bootstrap │
│    │ Ruin prob & 95% worst DD    │ Autocorrelation block; DD < 30%    │           │ for tail risk          │
├────┼─────────────────────────────┼────────────────────────────────────┼───────────┼────────────────────────┤
│ S6 │ Cost-Stress Sensitivity     │ `validation.rs::cost_stress`       │ MISSING   │ Computes 1x STT/GST,   │
│    │ 2x and 3x friction test     │ Fails if 2x Sharpe collapses <= 0  │           │ but no 2x stress gate  │
├────┼─────────────────────────────┼────────────────────────────────────┼───────────┼────────────────────────┤
│    │ TIER 2: EXECUTION & TIMING PITFALLS                                                                   │
├────┼─────────────────────────────┼────────────────────────────────────┼───────────┼────────────────────────┤
│ E1 │ Lookahead / Trading Close   │ `07_declarative_long_short.py`     │ Accounted │ Static AST verification│
│    │ Filling at signal close     │ Orders execute on Next Open        │           │ + C_t -> O_{t+1} fills │
├────┼─────────────────────────────┼────────────────────────────────────┼───────────┼────────────────────────┤
│ E2 │ Gap-Through-Stop Loss       │ `07_declarative_long_short.py`     │ Partial   │ Fills at stop price;   │
│    │ Overnight gap below SL      │ If Open < SL, fills at Open        │           │ gaps not penalized     │
├────┼─────────────────────────────┼────────────────────────────────────┼───────────┼────────────────────────┤
│ E3 │ Same-Bar Touch Ambiguity    │ `07_declarative_long_short.py`     │ Partial   │ Evaluates stop first,  │
│    │ High >= Target & Low <= SL  │ Invariant: SL checked before Target│           │ but unverified in L1   │
├────┼─────────────────────────────┼────────────────────────────────────┼───────────┼────────────────────────┤
│ E4 │ Unrealistic Delivery Short  │ `07_declarative_long_short.py`     │ Accounted │ Long-only delivery or  │
│    │ Selling equity overnight    │ Requires F&O or SLB borrowing      │           │ intraday MIS flags     │
├────┼─────────────────────────────┼────────────────────────────────────┼───────────┼────────────────────────┤
│    │ TIER 3: MARKET MICROSTRUCTURE & REGULATORY TRAPS                                                      │
├────┼─────────────────────────────┼────────────────────────────────────┼───────────┼────────────────────────┤
│ M1 │ Indian Statutory Schedule   │ `02_cost_modeling.py`              │ Accounted │ TaxCalculator: STT, GST│
│    │ STT leg asymmetry, Stamp    │ Buy STT=0, Sell STT=0.1%, Stamp,GST│           │ SEBI fee, Stamp duty   │
├────┼─────────────────────────────┼────────────────────────────────────┼───────────┼────────────────────────┤
│ M2 │ Liquidity Hunt Cluster      │ `03_turtle_breakout.py`            │ Accounted │ 2.0 ATR buffer stops   │
│    │ Stop sweeping round numbers │ Minimum 2.0 ATR protective buffer  │           │ outside retail clusters│
├────┼─────────────────────────────┼────────────────────────────────────┼───────────┼────────────────────────┤
│ M3 │ Bull Trap (Exhaustion Break)│ `03_turtle_breakout.py`            │ Accounted │ Volume confirmation &  │
│    │ Breakout into resistance    │ Rejects low-volume thrusts         │           │ 9-bar Donchian check   │
├────┼─────────────────────────────┼────────────────────────────────────┼───────────┼────────────────────────┤
│ M4 │ Bear Trap (Short Squeeze)   │ `03_turtle_breakout.py`            │ Accounted │ Breakout support test  │
│    │ False breakdown at support  │ Squeeze probability veto           │           │ verified               │
├────┼─────────────────────────────┼────────────────────────────────────┼───────────┼────────────────────────┤
│ M5 │ NSE Freeze & Lot Limits     │ `07_large_qty_options_slicer.py`   │ MISSING   │ Slices large orders    │
│    │ Order exceeding max freeze  │ Slices orders >= 1,800 units       │           │ into freeze lots       │
├────┼─────────────────────────────┼────────────────────────────────────┼───────────┼────────────────────────┤
│    │ TIER 4: AI & QUANTITATIVE NARRATIVE PITFALLS                                                          │
├────┼─────────────────────────────┼────────────────────────────────────┼───────────┼────────────────────────┤
│ A1 │ Metric Hallucination        │ `07_natural_language_critique.py`  │ Accounted │ Engine metrics injected│
│    │ Model inventing returns     │ Strict ground-truth context        │           │ directly into prompt   │
├────┼─────────────────────────────┼────────────────────────────────────┼───────────┼────────────────────────┤
│ A2 │ Hindsight Narrative Bias    │ `07_natural_language_critique.py`  │ Accounted │ Model restricted to    │
│    │ Explaining after the fact   │ Quantitative review format         │           │ geometric synthesis    │
├────┼─────────────────────────────┼────────────────────────────────────┼───────────┼────────────────────────┤
│ A3 │ Prompt Injection via Code   │ `07_natural_language_critique.py`  │ Partial   │ System prompt guards,  │
│    │ Comments manipulating bias  │ Untrusted text sanitized           │           │ but no AST comment strip│
└────┴─────────────────────────────┴────────────────────────────────────┴───────────┴────────────────────────┘
```

---

## 3. Deep Analysis of Critical Missing Gaps

### Gap 1: Probability of Backtest Overfitting (PBO via CSCV)
- **What it is**: In parameter optimization (e.g. tuning `entry_period` from 10 to 50 across 100 trials), the highest-ranking trial may be an artifact of data mining rather than genuine predictive edge.
- **Honba Solution**: Combinatorially Symmetric Cross-Validation (CSCV). Divides historical bars into $S$ sub-matrices (e.g. 16 blocks), generates all $\binom{S}{S/2}$ combinations of in-sample and out-of-sample splits, and measures the probability that the in-sample optimal strategy underperforms the median out-of-sample strategy.
- **Vyapaar Gap**: `vyapaar-ui-core` computes the Deflated Sharpe Ratio (DSR), which penalizes trial count and variance, but does not compute PBO.
- **Remediation**: Port Honba’s `pbo.rs` module into `vyapaar-ui-core::strategy::validation`. Reject any tuned parameter set with $\text{PBO} \ge 50\%$.

### Gap 2: Walk-Forward Efficiency (WFE)
- **What it is**: Robert Pardo’s Walk-Forward Efficiency measures parameter stability over time by rolling optimization windows across historical regimes.
  $$\text{WFE} = \frac{\text{Annualized Return}_{\text{Out-of-Sample}}}{\text{Annualized Return}_{\text{In-Sample}}}$$
- **Honba Standard**: $\text{WFE} \ge 50\%$. A strategy yielding 30% annualized in-sample but only 6% out-of-sample ($\text{WFE} = 20\%$) fails the gate.
- **Vyapaar Gap**: Vyapaar runs static optimization across the full sample without evaluating rolling window degradation.

### Gap 3: Block-Bootstrap Monte Carlo Simulation & Ruin Probability
- **What it is**: Historical returns occur in one single historical path. Monte Carlo block bootstrap resamples contiguous blocks of returns (preserving autocorrelation and volatility clustering) 1,000 to 10,000 times.
- **Metrics Evaluated**:
  1. **95th Percentile Maximum Drawdown**: What is the drawdown in the 5% worst paths? (Must be $< 30\%$).
  2. **Probability of Ruin**: Probability of equity falling below 50% of starting capital (Must be $< 1\%$).
- **Vyapaar Gap**: Vyapaar reports only historical peak-to-trough drawdown (0.15%), understating tail risk under alternative path realizations.

### Gap 4: Cost-Stress Sensitivity Testing (2x / 3x Friction)
- **What it is**: In India, friction includes STT (0.1% on delivery sell), GST (18%), exchange turnover fees, SEBI turnover, stamp duty, plus bid-ask slippage. Strategies that churn capital often look profitable on paper but collapse under realistic execution conditions.
- **Honba Standard**: Runs backtest under $1\times$, $2\times$, and $3\times$ statutory costs.
  - If $2\times$ cost collapses the Sharpe ratio $\le 0.0$, the strategy is designated **"Friction Fragile"** and rejected.
- **Vyapaar Gap**: Vyapaar accurately calculates 1x friction fees (e.g. ₹265.74 yielding net RR 1:1.92), but does not run the automated 2x stress test.

### Gap 5: Gap-Through-Stop Execution Reality
- **What it is**: In backtests, a stop loss at ₹26,689.25 is assumed to execute at ₹26,689.25. If the market gaps down overnight and opens at ₹26,400.00, real execution occurs at ₹26,400.00.
- **Vyapaar Gap**: `vyapaar-runtime-backtest` must enforce:
  ```rust
  if bar.open < stop_loss {
      fill_price = bar.open; // penalized for overnight gap
  } else {
      fill_price = stop_loss;
  }
  ```

---

## 4. Remediation Plan

To achieve 100% parity with Honba's quantitative standards, implement `PLAN-0002`:
1. Port `pbo.rs`, `walk_forward.rs`, `monte_carlo.rs`, and `cost_stress.rs` into `vyapaar-core::analytics` and `vyapaar-ui-core::strategy::validation`.
2. Integrate `ValidationBlock` into `StrategyBridge::generate_trade_suggestion`, displaying the full 6-gate audit card before trade recommendation.
3. Update `vyapaar-runtime-backtest` fill engine to enforce overnight gap-through-stop penalties and conservative same-bar stop ordering.
