# `vyapaar-runtime` — DDD Architecture & Central Runtime Bounded Context

> Tier 3B (House). Ported from `honba-ya`; see `code-migration-map.md` for the historical
> migration record. Depends on `vyapaar-core`, `vyapaar-risk`, `vyapaar-ledger`,
> `vyapaar-market`, `vyapaar-exec`, `vyapaar-agent-core`.

## 1. Bounded Context & Responsibility

`vyapaar-runtime` is the **Central Application & Orchestration Runtime**: it wires the pure
domain heart (`vyapaar-ledger`), the pre-trade risk gate (`vyapaar-risk`), market feeds
(`vyapaar-market`), and execution gateways (`vyapaar-exec`) into four operational surfaces:

1. **Supervised Tokio actor pipeline** (`vyapaar-runtime-actors`) — live/paper event loop.
2. **Deterministic backtester + enhancer** (`vyapaar-runtime-backtest`,
   `vyapaar-runtime-enhancer`) — research engine with exact parity to live.
3. **Headless REST/WS gateway** (`vyapaar-runtime-api`, `vyapaar-runtime-api-rest`) — the
   `vyapaar serve` control plane, including the chat agent runtime.
4. **Job scheduler + persistence** (`vyapaar-runtime-jobs`, `vyapaar-runtime-persistence`) —
   scheduled runs, run journals, and relational state.

All four surfaces speak the same wire contract (`schema_version`, `api_version`) and the same
domain types from `vyapaar-core` (`Price`, `Quantity`, `Money`, `InstrumentId`, `OrderRequest`,
`FillReport`).

## 2. Sub-Crate Topology (14 crates)

| Sub-Crate | Layer | Responsibility |
| :--- | :--- | :--- |
| `vyapaar-runtime-config` | Config | Self-describing run-config schema (`BacktestRunConfig`), `deny_unknown_fields` everywhere, `[risk]` section (ADR 0018), `ResolvedUniverse` bridge. |
| `vyapaar-runtime-strategy` | Pure Domain | `Strategy` trait + `StrategyContext`, `OrderIntent`, `StrategyRunner`, `StrategyManifest` / `StrategyIr` / `step.toml` IR, `StrategyFactoryRegistry`, and the multi-broker `dispatch` layer (`BrokerAdapter` + paper/Zerodha/AngelOne adapters). |
| `vyapaar-runtime-backtest` | Sim Engine | Event-driven backtester driving the *real* `StrategyRunner`: Arrow/Parquet `CatalogLoader`, NSE calendar, L2 fill simulation, latency/slippage/adverse-fill models, E2/E3 stop-fill semantics, T+1 ICCL settlement, MIS 15:20 square-off, exact statutory costs. |
| `vyapaar-runtime-enhancer` | Optimizer | Automatic Strategy Enhancer: GA, Bayesian TPE, walk-forward (WFE ≥ 0.60), DSR/PBO guardrails, Monte Carlo permutation + shock stress, Odysseus rule-tree genetic backend, closed-loop AI refinement. |
| `vyapaar-runtime-actors` | Actor System | Supervised Tokio actors (`MarketDataActor`, `StrategyActor`, `PositionSizingActor`, `RiskActor`, `ExecutionActor`, `PortfolioActor`, `PythonStrategyActor`), Prometheus metrics, W3C trace-context spans, formal SLO verify functions. |
| `vyapaar-runtime-jobs` | Scheduler | `JobSpec`/`CronSchedule` (5-field Vixie), `FileJobRegistry` (atomic JSON), multi-dimensional `JobBudgets`, `FencingLease` heartbeat fencing, `BoundedWorkerPool` (HTTP-429 backpressure), built-in TOTP/EOD/snapshot jobs. |
| `vyapaar-runtime-persistence` | Storage Infra | `sqlx` SQLite/PostgreSQL stores (orders, fills, FIFO tax lots, traders, decisions, agent chat sessions), embedded migration runner, and the SEBI append-only `SqliteAuditLedger`. Implements `vyapaar-ledger` repository traits. |
| `vyapaar-runtime-testing` | Test Harness | Deterministic `MockBrokerExecutor`, `MockMarketFeed`, `simulated_fill`, fixed-epoch fixture helpers. |
| `vyapaar-runtime-api` | API Contracts | Zero-HTTP contract crate: `ResponseEnvelope` + `ErrorCode` (16 codes), 44-endpoint capability registry, `RunId` (26-char Crockford), run manifests/journals, request validation/resolution, screener AST evaluation, reader-adapter traits. |
| `vyapaar-runtime-api-rest` | HTTP/WS Server | Axum `vyapaar serve`: `/api/v1` + `/api/v2` routers, embedded backtest executor, NDJSON run journals + SSE streams, ADR 0018 order gate + approval queue, OpenAPI 3.1 + Swagger, bearer/JWT auth, profile/portfolio stores, chat WebSocket + alert store. |
| `vyapaar-runtime-feed` | Feed Shim | Re-export shim over `vyapaar-market-store` (Parquet tick store, Lakehouse catalog, live quote store); keeps the historical `vyapaar_runtime_feed` import path alive. |
| `vyapaar-runtime-market` | Placeholder | Stub crate (placeholder for market-data normalization). |
| `vyapaar-indicators` | Placeholder | Stub crate (indicators live in `vyapaar-market-indicators`). |
| `vyapaar-runtime-test-utils` | Placeholder | Stub crate reserved for shared test utilities. |

## 3. Event Pipeline

```text
                    ┌───────────────────────────────────────────────────────────┐
                    │                  vyapaar-runtime                          │
                    │                                                           │
  Arrow/Parquet ────┤ CatalogLoader ─▶ MarketEvent stream ─▶ StrategyRunner   │
  (backtest)        │                                    │  (risk gate inside)  │
                    │                                    ▼                      │
                    │                    OrderRequest → L2/latency/slippage     │
                    │                    → FillReport → TaxCalculator           │
                    │                    → double-entry Transaction             │
                    │                    → T+1 settlement / MIS square-off      │
                    │                                                           │
  live ticks ───────┤ MarketDataActor → StrategyActor → (PositionSizingActor)   │
  (live/paper)      │      │                            │                       │
                    │      │                            ▼                       │
                    │      │                     RiskActor (check_order)        │
                    │      │                            │                       │
                    │      │                            ▼                       │
                    │      │                     ExecutionActor → BrokerExecutor│
                    │      │                            │                       │
                    │      └──────────── broadcast Tick ┘                       │
                    │                            │                              │
                    │                            ▼                              │
                    │                     PortfolioActor / SqliteAuditLedger    │
                    └───────────────────────────────────────────────────────────┘
```

Backtest and live share the **same** strategy trait, runner, risk engine, ledger crates, and
statutory cost calculators — parity is achieved by delegation, not reimplementation.

## 4. Cross-Cutting Invariants

| Invariant | Where enforced |
| :--- | :--- |
| Zero floats in financial accounting | `rust_decimal::Decimal` in strategy context, backtest engine, ledger postings, persistence columns (`DECIMAL(30,16)` / decimal strings). Known float seams are confined to: optimizer fitness layer (f64 by design), config-boundary `starting_cash`/`slippage_multiplier` (f64 schema fields), indicator math inside strategy math, and the paper-adapter fee path (`notional.to_f64()` for the cost schedule). |
| No wall clock in domain state machines | Every state transition takes an explicit `DateTime<Utc>` / `ts_init` from the caller. Remaining `Utc::now()`/`Instant::now()` uses are audit-row timestamps, metric instrumentation, and lifecycle logging — never trading math. |
| Zero-I/O domain logic | `vyapaar-runtime-strategy` is I/O-free; the backtest engine's only I/O is the catalog read at the edge; actors receive injected ports (`Arc<dyn BrokerExecutor>`, audit ledger). |
| Deterministic replay | Client order ids are `Uuid::new_v5(NAMESPACE_OID, "{name}-{n}")`; fill ids are `"{client_order_id}-{seq}"`; run ids are monotonic Crockford Base32; `dispatch_intent_with_id` is the reproducible dispatch path; byte-identical journals are tested at 1/4/16 workers. |
| Tests in `tests/` only | Zero `#[cfg(test)]` in `crates/*/src/` (grep-verified in CI). |
| Canonical `prod` profile | `make prod` / `cargo test --workspace --profile prod`. |
| Wire strictness | `deny_unknown_fields` on configs, manifests, envelopes, tick files; monetary values serialized as strings (documented exception: the order wire uses JSON numbers). |

## 5. Open Gaps & Plans (as of 2026-11-12)

- Open gaps: `docs/gaps/0004` (profile schema), `0006` (chat session persistence + portfolio
  tools stubs), `0007` (chat market/content reader approximations).
- Active plans: `docs/plans/0003` (trade-journal + market-reference REST endpoints), `0004`
  (block/bulk deal chat tool; Phases 1–3 implemented, Phase 4 blocked on `vyapaar-data` gap 0005).
- Closed: gaps 0001–0003, plans 0001–0002 — see the `docs/{gaps,plans}/README.md` indexes.

## 6. Per-Crate Design Docs

| Doc | Crate |
| :--- | :--- |
| `code-actors.md` | `vyapaar-runtime-actors` |
| `code-strategy.md` | `vyapaar-runtime-strategy` |
| `code-backtest.md` | `vyapaar-runtime-backtest` |
| `code-enhancer.md` | `vyapaar-runtime-enhancer` |
| `code-jobs.md` | `vyapaar-runtime-jobs` |
| `code-persistence.md` | `vyapaar-runtime-persistence` |
| `code-api.md` | `vyapaar-runtime-api` |
| `code-api-rest.md` | `vyapaar-runtime-api-rest` |
| `code-config.md` | `vyapaar-runtime-config` |
| `code-testing.md` | `vyapaar-runtime-testing` |
| `code-feed.md` | `vyapaar-runtime-feed` (+ placeholders: `vyapaar-runtime-market`, `vyapaar-indicators`, `vyapaar-runtime-test-utils`) |
| `code-migration-map.md` | Historical honba-ya → vyapaar-runtime migration record |
