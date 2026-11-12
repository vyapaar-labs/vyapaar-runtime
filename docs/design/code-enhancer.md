# `vyapaar-runtime-enhancer` — Strategy Enhancer Reference

Crate `crates/vyapaar-runtime-enhancer/` — lib name `vyapaar_runtime_enhancer`.
Crate doc: *"Automatic Strategy Enhancer for Vyapaar"*.

## 1. Purpose

Provide every tier of the automatic strategy-enhancement stack over the **real**
`BacktestEngine` + `StrategyFactoryRegistry` (never a simulated fitness proxy):

- GA multi-objective search and Bayesian TPE hyperparameter search,
- walk-forward certification (WFE ≥ 0.60),
- López de Prado overfitting guardrails (DSR / PBO),
- Monte Carlo permutation + shock stress testing,
- closed-loop AI prompt-mutation refinement,
- the StockSharp-Odysseus rule-tree genetic backend (Rayon, target 1,000 genomes/sec).

## 2. Module Map

| File | Contents |
|---|---|
| `lib.rs` | `Optimizer` trait, `OptimizationResult`, `EnhancerEngine`, module decls + re-exports |
| `ga_optimizer.rs` | `GaConfig`, `Individual`, `GaOptimizer`, `GaOptimizationResult`, `GaError` |
| `bayesian_optimizer.rs` | `BayesianConfig`, `EvaluatedPoint`, `BayesianOptimizer`, `BayesOptimizationResult`, `BayesError` |
| `walk_forward.rs` | `WalkForwardConfig`, `WalkForwardResult`, `run_wfo<S>`, `FitnessResult` |
| `monte_carlo_stress.rs` | `MonteCarloResult`, `StressTestResult`, `ShockMagnitude`, `monte_carlo_permutation`, `apply_shock`, `stress_test` |
| `odysseus.rs` | `GenomeNode`, `ComparisonOp`, `Genome`, `EvalContext`, `GenomeFitness`, `OdysseusConfig`, `OdysseusEvaluator` |
| `overfitting_guardrails.rs` | `DSRCalculator`, `PBOCalculator`, `OverfittingAssessment`, `OverfittingDiagnostics` |
| `ai_refinement.rs` | `StrategyDiagnostic`, `AIRefinementConfig`, `AIRefinementResult`, refinement loop functions |

## 3. `EnhancerEngine`

```rust
EnhancerEngine::new()
    .with_config(ga_config, bayesian_config, wfo_config)
```

All optimization methods take
`(manifest, data: &[Tick], instrument, quantity, risk_engine, calendar)`:

| Method | Delegates to |
|---|---|
| `optimize_ga(...)` | `GaOptimizer::optimize` |
| `optimize_bayesian(...)` (`&mut self` — stateful TPE) | `BayesianOptimizer::optimize` |
| `run_wfo(..., optimizer: F)` | caller-injected optimizer (GA or Bayesian) over rolling windows |
| `run_monte_carlo(..., returns, n_simulations)` | `monte_carlo_permutation` (PnL-level) |
| `run_stress_test(..., shock_magnitudes, gap_probability)` | `stress_test` |
| `assess_overfitting(sharpe, returns, is_fitness, oos_fitness, n_trials, n_folds)` | DSR + PBO |
| `run_ai_refinement(strategy, held_out_data, config)` | closed-loop refinement |

## 4. Shared Fitness Function (GA / Bayesian / WFO)

Scored from the real `BacktestResult` (weights `(0.5, 0.3, 0.15, 0.05)` = sharpe, sortino,
profit factor, drawdown score):

- `sharpe = (net_pnl / max_drawdown_pct).clamp(−10, 10)`
- `sortino = sharpe × 0.9`
- `profit_factor = (gross_pnl / total_fees).min(10)` (1.0 when fees ≤ 0)
- `dd_score = 1/(1 + max_drawdown_pct/10)`; trade penalty `1/(1 + trades/1000) × 0.01`
- composite = weighted sum + trade penalty

## 5. Genetic Algorithm (`GaOptimizer`)

`GaConfig` defaults: population 50, generations 20, crossover 0.8, mutation 0.2, mutation
strength 0.1, elitism on (elite count 5), fitness weights `(0.5, 0.3, 0.15, 0.05)`.

1. `manifest_to_parameter_grid(manifest)` — empty grid ⇒ `GaError::EmptyParameterGrid`.
2. Population sampled from the grid; **individuals are never stored** — `Individual` holds
   `params` + `strategy_name` and recreates via `StrategyFactoryRegistry::create`
   (`Individual::create_strategy`), keeping `Clone` cheap.
3. Per generation: fitness via Rayon `par_iter_mut`, each individual running a **real**
   `BacktestEngine` (₹1,000,000 cash).
4. Sort descending; convergence declared when `generation > 5 && variance(top-5 fitness) < 1e-6`.
5. Operators: tournament selection (size 3); uniform crossover (50/50 per parameter key);
   mutation — floats `×(1 + U(−strength, +strength))` clamped > 0, integers ±1 floored at 1,
   per gene with probability `mutation_rate`; elite count carried verbatim.
6. **Pareto frontier**: domination on `(sharpe ≥, sortino ≥, profit_factor ≥,
   max_drawdown_pct ≤)`.
7. Result: `GaOptimizationResult { best_individual, pareto_frontier, generations_run,
   total_evaluations, duration, converged }`.

## 6. Bayesian TPE (`BayesianOptimizer`)

`BayesianConfig` defaults: `initial_points 10`, `total_points 50`, acquisition `"ei"`
(`kappa 2.5` for the documented `"ucb"` alternative, `gamma 0.25` percentile split).

1. `set_param_space_from_manifest` — integer when the param default is `i64`/`u64` or
   min/max/step are whole; empty space ⇒ `BayesError::EmptyParameterSpace`.
2. First `initial_points` sampled uniformly (`rand_distr::Uniform`), each evaluated via a real
   backtest and recorded as `EvaluatedPoint`.
3. `sample_next_tpe`: sort by fitness, split good/bad at `ceil(len × gamma)`; per parameter fit
   `Normal(good_mean, good_std.max(1e-6) or (max−min) × 0.1)`, sample, clamp, round for
   integers.
4. Integral-valued f64s are emitted as JSON **i64** so factory readers using `as_u64()` accept
   periods like `14.0`.
5. Strictly sequential (no Rayon) — each point depends on the posterior. `FitnessWeights`
   constants are byte-identical to `GaConfig`'s defaults so both optimizers rank identically.

## 7. Walk-Forward (`run_wfo`)

`WalkForwardConfig` defaults: in-sample 252 bars, out-of-sample 63, step 63, `compute_wfe`,
`wfe_threshold 0.60`.

1. Rolling windows: IS = `data[start .. start+252]`, OOS = the next 63; advance by step while
   `start + IS + OOS <= len`.
2. The injected optimizer runs on the IS slice; a failed run is skipped (window contributes
   nothing).
3. `evaluate_with_params` per window: fresh `StrategyFactoryRegistry::new()`,
   `BacktestConfig { starting_cash: ₹1,000,000, ..Default }`, real engine over the window's
   events; factory failure ⇒ `FitnessResult::rejection()` (−∞ sharpe/sortino, 100% DD,
   `Decimal::MIN` PnL).
4. WFE = `oos_sharpe / is_sharpe` (only when IS sharpe > 0 and `compute_wfe`); `n_runs` counts
   WFE-producing runs.
5. **Certification**: `certified = n_runs > 0 && every wfe >= 0.60`; `avg_wfe`, `wfe_std`
   (sample std, n−1).

## 8. Monte Carlo & Stress (`monte_carlo_stress.rs`)

- `monte_carlo_permutation(returns, n_simulations)`: per simulation, shuffle a copy of the
  returns **without replacement**, walk cumulative PnL (`Decimal`, `from_f64_retain`), track
  peak/drawdown; report `min/max/mean/median/std_pnl`, `var_5pct/1pct/95pct/99pct`,
  `cvar_5pct/1pct` (mean of the tail beyond VaR), `prob_ruin`, `prob_profit`, skewness, excess
  kurtosis, worst drawdown. All PnL fields are `Decimal`; skew/kurtosis are f64 statistics.
- `apply_shock(returns, magnitude, gap_probability)`: spread/slippage shock — every 10th bar,
  with 30% probability, multiplies the return by `magnitude.multiplier()` (1.5× / 2.0× / 3.0× /
  custom); gap shock — with `gap_probability` prepends `−3.0 × multiplier` at index 0.
- `stress_test`: base MC + one MC per shock magnitude; `worst_case_pnl` = minimum across all;
  `survives_stress = worst_case > −(₹1,000,000 × 0.5)` (hardcoded ₹10L capital).
- Note: the manifest/factory/instrument/quantity/risk/calendar parameters are currently
  accepted but unused (`_`-prefixed) — PnL-level simulation, not per-permutation backtests.

## 9. Odysseus Rule-Tree Genetic Backend (`odysseus.rs`)

- `GenomeNode`: `Condition { signal, op, threshold }`, `And(Vec)`, `Or(Vec)`, `Not(Box)`,
  `Action { side, size_fraction, tif }`; `ComparisonOp { Gt, Lt, Ge, Le, Eq, Ne }`
  (Eq/Ne with 1e-9 epsilon).
- `EvalContext` L2 snapshot: mid price, spread, bid/ask depth (+ top-5), imbalance
  `(bid − ask)/(bid + ask)`. Recognized signals: `mid_price`, `spread`, `bid_depth`,
  `ask_depth`, `bid_depth_5`, `ask_depth_5`, `imbalance` (unknown → 0.0).
- `OdysseusConfig` defaults: target 1,000 genomes/sec, population 200, generations 50, max
  tree depth 6, mutation 0.15, crossover 0.7, elites 10.
- Evaluation: recursive condition walk over the tree; trade PnL =
  `(mid × bid_depth) × size_fraction` signed by side; per-trade metrics with Sharpe/Sortino
  annualized at `√(252 × 100)` (252 days × ~100 trades/day); composite score weights
  `0.40 / 0.25 / 0.20 / 0.15` (sharpe / sortino / win-rate / drawdown penalty).
- Parallelism: `evaluate_batch` via Rayon `par_iter`; atomic eval-count/nanoseconds telemetry
  (`avg_eval_time_us`).
- Genetic operators: `random_genome` (recursive random tree), subtree crossover (per-node
  donor probability 0.3), mutation (0.3 whole-subtree replace; per-node field mutation; side
  flip / size re-roll on actions), `evolve` loop (elite + tournament + crossover + mutate,
  progress every 10 generations). Genomes are serde-serializable for persistence.

## 10. Overfitting Guardrails (`overfitting_guardrails.rs`)

- `DSRCalculator { n_trials, confidence 0.95 }`: simplified López de Prado-style
  `DSR = sharpe / (1 + √(2γ(1−Φ(z))) × (1+skew²/4) × (1+kurtosis/4))` with
  `γ = 0.5772156649` (Euler–Mascheroni), `z = sharpe × √n`, Φ via `statrs` normal CDF;
  returns with < 30 samples fall back to the raw Sharpe. Bands: `≥ 3.0` robust, `< 1.5` likely
  overfitting.
- `PBOCalculator { n_folds, significance 0.05 }`: CSCV-style — PBO = fraction of folds where
  `OOS − IS < 0`; `compute_log_rank` weights by `1 + avg|diff|.min(1)`, capped at 1.0;
  IS/OOS length mismatch ⇒ `eprintln!` + return 1.0 (maximal overfitting probability).
- `OverfittingAssessment::assess(...)`: `passes_guards = dsr >= 3.0 && pbo < 0.5`;
  `report()` renders a human-readable ✓/~/✗ block; `OverfittingDiagnostics` carries
  sharpe/skew/kurtosis/n_trials/n_folds and the IS/OOS fitness vectors.

## 11. AI Refinement Loop (`ai_refinement.rs`)

- `StrategyDiagnostic` (serde): metrics, WFE/DSR/PBO options, parameter sensitivity, observed
  regimes, suggested prompt mutations.
- `refine_strategy_with_ai` mutation triggers: `sharpe < 1.0` → halve position sizes / extend
  holding; `max_drawdown_pct > 10` → stricter per-trade risk limits; `win_rate < 0.4` → add a
  trending-regime entry filter; `pbo > 0.5` → extend OOS validation; `dsr < 2.0` → apply DSR
  adjustment; empty sensitivity → sensitivity-analysis prompt; fallback "fine-tune entry/exit
  thresholds by 0.5%".
- `run_closed_loop_refinement`: diff-based mutations against the previous diagnostic (Sharpe
  declined → regime filter; DD increased → tighten circuit breakers; win rate fell > 0.05 →
  review entry criteria).
- `AIRefinementConfig` defaults: enabled, max 5 iterations, min improvement 0.05,
  `auto_apply: false` (no-op today). `run_ai_refinement_loop` converges when the Sharpe
  improvement drops below `min_improvement`.
- **Scaffolding note**: `evaluate_strategy_diagnostic` is a placeholder (hardcoded zeros) —
  the diagnostic pipeline awaits real metric wiring.

## 12. Dependencies

`vyapaar-core` (types), `vyapaar-risk` (engine passthrough), `vyapaar-runtime-backtest`
(`BacktestEngine`), `vyapaar-runtime-strategy` (`Strategy`, `DynStrategy`,
`StrategyManifest`, `StrategyFactoryRegistry`, `manifest_to_parameter_grid`),
`vyapaar-runtime-actors` (workspace umbrella), `rayon`, `rand`/`rand_distr` (all
`thread_rng()`), `statrs` (normal CDF), `rust_decimal`, `serde`/`serde_json`/`thiserror`/
`chrono`/`num_cpus`.

## 13. Tests

`tests/unit_tests.rs` — one test: `dsr_calculator_returns_sharpe_below_minimum_sample`
(fewer than 30 returns ⇒ raw Sharpe unchanged). GA/Bayesian/WFO/MC/Odysseus behaviour is
exercised indirectly through the backtest crate's dev-dependency tests and the agent crate.

## 14. Invariants & Notables

- **Real-engine fitness everywhere** in GA/Bayesian/WFO — only the PnL-level Monte Carlo and
  the Odysseus L2 evaluator bypass the engine.
- **Determinism seam**: the engine path is deterministic, but GA sampling/mutation, TPE
  sampling, MC shuffles, and Odysseus randomness use `rand::thread_rng()` — optimizer runs
  themselves are not reproducible.
- **Quirks (documented)**: GA initializes population fitness against an empty data slice;
  `optimize_bayesian` needs `&mut self` while `optimize_ga` is `&self`; stress survival
  hardcodes ₹10L × −50%; WFO builds a fresh factory registry per evaluation; the `Optimizer`
  trait in `lib.rs` is vestigial (neither optimizer implements it — their signatures are
  manifest-based).
- **Floats**: the optimizer/fitness layer is deliberately f64 (Sharpe/Sortino/PF/DD, DSR/PBO,
  Odysseus); `Decimal` is kept for PnL fields and backtest cash — the optimizer layer sits
  outside the strict zero-float accounting path.

## Related

- `code-backtest.md` — the engine feeding every fitness evaluation
- `code-strategy.md` — manifests, parameter grids, and the factory registry being optimized
- `vyapaar-core::analytics` — the six-gate validation kernel (DSR, PBO/CSCV, walk-forward,
  holdout, block-bootstrap Monte Carlo, cost stress)
