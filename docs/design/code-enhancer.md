# `vyapaar-runtime` — Strategy Enhancer Reference

Documents `crates/vyapaar-runtime-enhancer/src/`: Automatic Strategy Enhancer with
multi-tier optimization.

## Optimizers

| Optimizer | File | Purpose |
|-----------|------|---------|
| Genetic Algorithm | `ga_optimizer.rs` | Evolutionary parameter optimization |
| Bayesian TPE | `bayesian_optimizer.rs` | Tree-structured Parzen Estimator |
| Walk-Forward | `walk_forward.rs` | WFO with in-sample/out-of-sample windows |
| DSR | (integrated) | Deflated Sharpe Ratio for multiple-testing correction |
| Odysseus | `odysseus.rs` | Genetic strategy synthesizer |
| Monte Carlo Stress | `monte_carlo_stress.rs` | Block-bootstrap stress testing |
| AI Refinement | `ai_refinement.rs` | LLM-assisted strategy refinement |
| Overfitting Guardrails | `overfitting_guardrails.rs` | PBO, holdout, cost-stress gates |

## Enhancement Pipeline

1. **Vectorized pre-filter** — Fast elimination of obviously bad parameter sets
2. **Event-driven simulation** — Full backtest for promising candidates
3. **Concurrent parameter sweeps** — Parallel optimization across parameter space
4. **Overfitting guardrails** — DSR, PBO, holdout, cost-stress validation

## Zero-Floats Policy

All accounting, prices, and performance metrics use `rust_decimal::Decimal`.

## Related

- `vyapaar-runtime-backtest` — Backtest engine feeds results to enhancer
- `vyapaar-core::analytics` — Six-gate validation kernel (DSR, PBO, WFO, holdout, Monte Carlo, cost-stress)
- `vyapaar-runtime-strategy` — Strategies are the optimization targets
