# `vyapaar-runtime-api-rest` — Axum REST & WebSocket Gateway Reference

Crate `crates/vyapaar-runtime-api-rest/` (41 src files, 39 test files + `tests/support/`).

## 1. Purpose

The HTTP boundary of the runtime. It implements the pure contracts of
[`vyapaar-runtime-api`](code-api.md) as an Axum server: multi-version REST
routing, bearer/JWT auth, backtest/sweep run execution against the real engine,
bar ingest into the bar pump, order placement through risk + approval + execution
gateway, portfolio/profile persistence, the chat agent runtime behind `/ws/chat`,
and two zero-polling WebSocket feeds. It also hosts `dispatch.rs`, the in-process
driver the Python SDK speaks to instead of a socket.

## 2. Module Map

| File | Contents |
|---|---|
| `lib.rs` | Router assembly (`/api/v1`, `/api/v2`, legacy fallback), `legacy_v1_deprecation_notice`, `serve()` |
| `v1.rs` | The ~60-route v1 table (legacy, supported) + `v1_headers` middleware |
| `v2.rs` | Same routes behind `v2_response_envelope` (`api_version: "v2"`, `x-vyapaar-version: 2`) |
| `openapi.rs` | OpenAPI 3.1 documents for v1 ("supported for legacy software") and v2 |
| `state.rs` | `AppState` + builder (catalog, depth, quotes, strategies, run store, portfolio, profile, approval queue, bar-ingest sender) |
| `bootstrap.rs` | `ServeConfig` / startup wiring; `load_risk_limits` for `--risk-config` TOML |
| `service.rs` | `RunService`, `RunServiceConfig` (`DEFAULT_MAX_QUEUED = 64`), `ShutdownReport` |
| `caller.rs` | Caller identity extraction; missing/invalid token ⇒ 401 |
| `auth.rs` | `FileTokenStore` (hot-reloaded), JWT (HS256/RS256) claims, `RouteScope`, `ROUTE_SCOPES` table |
| `run_store.rs` | Run manifests/journals on disk + recovery of `Running` runs at boot |
| `runs.rs` | `POST/GET /backtests`, `GET /backtests/:id{,/journal}`, `/sweeps`, `/journals/:id` |
| `executor.rs` | `BacktestExecutor: RunExecutor` — runs jobs on the real engine |
| `dispatch.rs` | In-process `oneshot` driver (Python SDK transport) incl. SSE replay |
| `streams.rs` | SSE `GET /backtests/:id/events?resume_from_seq=N` |
| `journal.rs` | Run/audit journal query endpoint |
| `operator_journal.rs` | Operator-audit journal (approval actions) |
| `trade_journal.rs` | Trade-performance journal routes (`vyapaar-ledger-performance` backed) |
| `orders.rs` | `POST /orders`, `DELETE /orders/:id`, fills, approval queue handlers; collar/lot/tick checks |
| `exec.rs` | Execution-gateway integration for order routes |
| `approval.rs` | `ApprovalQueue` (E5-S4): `VecDeque` + statuses `Pending/Approved/Rejected/Expired` |
| `trading_state.rs` | `GET/PUT /trading-state` (`Running`/`Halted`/`ReduceOnly`, admin scope) |
| `market.rs` | `/instruments`, `/instruments/:id`, `/quotes`, `/bars/:id`, `/depth/:id`, `/market-data/{symbol-profile,calendar}`; `MAX_BAR_ROWS = 100_000` |
| `catalog.rs` | Instrument catalog reader |
| `ticks_file.rs` | Flat tick files (`VERSION = 1`) |
| `screener.rs` | `GET /screener/scan` over the API crate's AST evaluator |
| `ingest.rs` | `POST /bars`: batch validate, reserve permits, enqueue |
| `bar_pump.rs` | Bar-pump task: bounded channel → applies bars, counts out-of-order skips |
| `profile.rs` | `GET/PUT /profile`, layouts/modules CRUD; body limit `MAX_BODY_BYTES = 1 MiB` |
| `profile_store.rs` | Profile persistence: 2 MiB / 200 layouts / 100 modules / 100 watchlists / 5,000 symbols |
| `portfolio.rs` | Portfolio read routes — 4 of them answer **501 `not_implemented`** (`valuation_engine_mismatch`) |
| `portfolio_store.rs` | Portfolio persistence: 2 MiB / 100 accounts / 5,000 positions per account / 100k trades |
| `user_doc.rs` | `DEFAULT_USER_ID = "default"`, `MAX_USER_ID_LEN = 64` |
| `reports.rs` | `/reports/*` report pages (`MAX_PAGE_SIZE = 500`) |
| `retention.rs` | `RetentionConfig` defaults: `keep_runs = 1_000`, `keep_days = 30` |
| `chat.rs` | `ChatRuntime`, `/ws/chat` handler, LLM env bootstrap |
| `chat_alerts.rs` | `ProfileAlertStore` persisted on `Profile.alerts` |
| `chat_content.rs` | News/filings content readers (stub-backed, see gaps) |
| `chat_market.rs` | `FeedMarketReader` / `FeedContentReader` / `FeedSnapshotSource` (`MAX_BAR_CAP = 500`) |
| `ws.rs` | `GET /ws`, `GET /ws/market`: `WsTick`, `WsDepth`, `WsClientCommand` |

## 3. Routing & Versioning

One v1 route table serves three audiences (`lib.rs:262-280`):

| Mount | Middleware | Effect |
|---|---|---|
| `/api/v1` | `v1_headers` | Legacy, supported; `api_version: "v1"` |
| `/api/v2` | `v2_response_envelope` | Latest: rewrites envelope `api_version` to `"v2"`, adds `x-vyapaar-version: 2` |
| un-prefixed (fallback) | `legacy_v1_deprecation_notice` | `Deprecation: true`, canonical `Link: </api/v1{path}>`; if `X-Vyapaar-Version: 2` or `Accept: application/vnd.vyapaar.v2+json`, serves the v2 envelope instead |

Route groups (v1, authoritative in `v1.rs`):

- **Read/meta**: `capabilities`, `health`, `schema`, `instruments{,/:id}`, `quotes`,
  `bars/:id`, `depth/:id`, `market-data/symbol-profile/:id`, `market-data/calendar`.
- **Runs**: `POST/GET /backtests`, `backtests/:id{,/journal,/events}`, `POST/GET /sweeps`,
  `sweeps/:id{,/journal}`, `journals/:id`.
- **Strategies**: `POST/GET /strategies`, `POST /strategies/verify`.
- **Trading**: `POST/GET /orders`, `DELETE /orders/:id`, `orders/:id/fills`,
  `positions/close`, `approval/{queue,approve,reject}`, `GET/PUT /trading-state`,
  `POST /bars` (bar ingest).
- **Portfolio & reports**: `portfolio{,/accounts,/positions,/holdings,/trades,/chart,`
  `/concentration,/wac}`, `ledger`, `tax/report`, `fees`, `positions{,/audit}`, `trades`,
  `reports/{ledger,tax,snapshots,audit,concentration,wac,fees,equity-curve,fills/:order_id}`.
  **`/portfolio`, `/portfolio/accounts`, `/portfolio/positions`, `/portfolio/holdings`
  answer 501 `not_implemented` with reason `valuation_engine_mismatch`** — the live
  valuation engine is not wired; `/portfolio/trades` works.
- **Profile**: `profile{,/layouts{,/:name},/modules{,/:id}}`.
- **Screener**: `GET /screener/scan` (AST filters, budget-capped).
- **Trade journal**: `trade-journal/{accounts,overview,trades{,/:id},calendar,notes,compare}`.
- **WebSockets**: `ws`, `ws/market` (market feed), `ws/chat` (chat agent).

## 4. Authentication

- `FileTokenStore` — tokens in a JSON file, reloaded on mtime change (hot reload).
- Optional JWT (`jsonwebtoken`): HS256 shared secret or RS256 public key; claims
  include `sub`, scopes, `jti` (RFC 7518).
- `RouteScope::{Public, Authenticated, Required(read|trade|admin)}` with a static
  `ROUTE_SCOPES: &[(&str, &str, RouteScope)]` table — e.g. `/capabilities|/health|/schema`
  public, `/orders` writes `trade`, `/trading-state` `admin`, `/profile` authenticated.
- `caller.rs` resolves the caller per request; a missing/invalid token is a 401 with
  the shared error taxonomy.

## 5. Bootstrap Order

`bootstrap.rs` wires, in order: the market-data **catalog** (a `catalog/` tree or flat
`SYMBOL.EXCHANGE.parquet` files) → optional **risk limits** from `--risk-config` TOML
(`[risk]` validated before any engine exists) → **run store recovery** (runs left
`Running` by a previous process are resolved from their journals) → the optional
**bar pump** (`bar_ingest_capacity`, default 1,024) behind `POST /bars`.

## 6. Run Execution

- `service.rs` — `RunService` runs submitted jobs on a bounded queue
  (`DEFAULT_MAX_QUEUED = 64`); `ShutdownReport` carries the final bar-ingest drain.
- `executor.rs` — `BacktestExecutor` implements the API crate's `RunExecutor` port
  against the real `vyapaar-runtime-backtest` engine:
  - `FillModel::L2` requires a tick source; missing/empty ticks refuse with the
    stable `l2_requires_ticks` error.
  - A **sweep** builds the Cartesian parameter grid from `{low, high, step}` objects,
    runs one backtest per point, and ranks trials best-first by net PnL.
  - Sharpe is annualized with `PERIODS_PER_YEAR = 252` from per-event equity marks.
  - Every run emits an `assumptions` block; unmodelled factors are listed in
    `not_modelled` (e.g. `transaction_costs`, `slippage` when not configured).
- `streams.rs` — SSE: the handler replays `events.ndjson` from
  `resume_from_seq` (line-indexed journal), then tails the file, delivering frames
  with monotonic sequence numbers so clients survive reconnects without polling.
- `dispatch.rs` — in-process driver: builds the router and calls `oneshot` per
  request ("one request in, `(status, body)` out, no socket"). This is the
  Python SDK's transport (`honba._honba.api_request_stream`), including SSE replay.

## 7. Market Data & Ingest

- `market.rs` converts `vyapaar-feed` quotes/bars (`f64`, dual UnixNanos stamps)
  to canonical `Decimal` types **at this single boundary**, not threaded as floats;
  `lot_size`/`tick_size` likewise. Bars are bounded by `MAX_BAR_ROWS = 100_000`,
  depth by `1..=50` levels (default 5).
- `ingest.rs` + `bar_pump.rs` — `POST /bars` validates the batch
  (`MAX_INGEST_BARS = 1_000`, non-empty, ascending `ts_event`), reserves all
  channel permits up-front (whole batch or nothing), and returns once *queued*.
  Ordering is enforced downstream by the pump, which counts and skips
  out-of-order bars. Full queue ⇒ 429 `rate_limited` (`bar_ingest_full`).
- `screener.rs` runs the API crate's AST filter group per instrument within the
  scan budget (universe 1,000 / rows 500 / bars 2M).

## 8. Orders & Approvals

- `orders.rs` — the wire shape exposes quantity/price as **JSON numbers**
  (the documented exception; canonical core decimals serialize as strings) and
  converts to `rust_decimal::Decimal` at the boundary. Pre-trade checks: price
  collar vs reference price (bps), lot multiples, tick grid.
- `approval.rs` — placements (cancels and modifies bypass) are queued with
  `QueuedOrder { id, request, queued_at, status }`; operators act via
  `/approval/approve` and `/approval/reject`; the queue is bounded (default 100).
  Approvals are written to the operator journal (`operator_journal.rs`).
- `trading_state.rs` — the global `Running`/`Halted`/`ReduceOnly` gate consulted
  by order routes.

## 9. Portfolio, Profile & Journals

- `portfolio.rs`/`portfolio_store.rs` — the four valuation routes are honest 501s;
  the store itself persists accounts/positions/trades/holdings with the caps above
  (stand-in for the unwired valuation engine; see `docs/gaps/README.md`).
- `profile.rs`/`profile_store.rs` — per-user dashboard profile (layouts, modules,
  watchlists) with revisions; single-tenant default user `"default"`.
- `trade_journal.rs` — trade-performance journal (accounts, overview, trades,
  per-trade PATCH, calendar, notes, compare) backed by `vyapaar-ledger-performance`.
- `reports.rs` — derived report pages over stored runs/portfolio.

## 10. Chat Runtime (`/ws/chat`)

`chat.rs` builds a `ChatRuntime` from environment, reusing the `vyapaar-agent`
chat stack (`ChatAgent`, `SessionStore`, `ToolRegistry`):

- **LLM backend**: `VYAPAAR_GGUF_PATH` (local Candle-GGUF) takes priority;
  otherwise the OpenAI/Ollama-compatible endpoint `VYAPAAR_LLM_BASE_URL` /
  `VYAPAAR_LLM_MODEL` / `VYAPAAR_LLM_API_KEY`. Neither set ⇒ the socket answers
  with a clear `chat not configured` message.
- **Tools**: `vyapaar_agent_chat_tools::catalog::register_read_only_tools` plus
  the alert tools (`chat_alerts.rs`); alerts persist on `Profile.alerts`.
- **Sessions**: `InMemorySessionStore` — a restart loses sessions (documented,
  intentional).
- **Readers** (`chat_market.rs`): `FeedMarketReader` (quote/depth/bars, bar cap 500),
  `FeedContentReader` (news/filings — stub-backed), `FeedSnapshotSource`.
  The stubs cite their owning gaps: news → `vyapaar-data/docs/gaps/0006`,
  filings → `vyapaar-data/docs/gaps/0007`, block deals → `0005`,
  options chain → `vyapaar-market/docs/gaps/0001`, corporate actions → `0008`.
- `chat.rs` is env-aware and marked experimental in `openapi.rs` docs.

## 11. WebSockets (`ws.rs`)

`GET /ws` and `GET /ws/market` share one handler:

- Server → client: `WsTick` (instrument, bid/ask/last, prev close, volumes) and
  `WsDepth { bids, asks }` (level + qty); `tick_to_ws` converts a canonical
  `Tick` (+ optional prev price) at the boundary.
- Client → server: `WsClientCommand` — subscribe/unsubscribe actions by symbol,
  or direct symbol switches. Pure command-driven push: no client polling.

## 12. Dependencies

Axum (ws, SSE), `tokio` (mpsc, oneshot, tasks), `serde_json`, `rust_decimal`,
`jsonwebtoken`, `vyapaar-runtime-api` (contracts), `vyapaar-runtime-backtest`
(the real engine), `vyapaar-runtime-feed`/`vyapaar-market-store` (Parquet),
`vyapaar-agent` + `vyapaar-agent-chat` + `vyapaar-agent-chat-tools` (chat),
`vyapaar-ledger-performance` (trade journal), `vyapaar-feed`/`vyapaar-market-core`
(conversion source), `vyapaar-exec-core` (broker gateway), `vyapaar-risk`.

## 13. Tests (`tests/`, 39 files + `support/`)

| Area | Files |
|---|---|
| Server basics | `serve.rs`, `version_routing.rs`, `cors.rs`, `capabilities.rs`, `malformed_bodies.rs`, `not_implemented.rs` |
| Auth | `auth_scope.rs` (scope table, 401/403 paths) |
| Runs | `runs.rs`, `runs_store.rs`, `runs_cache.rs`, `runs_determinism.rs`, `runs_restart.rs`, `rest_backtest.rs`, `rest_backtest_l2.rs`, `rest_backtest_l2_parquet.rs`, `rest_sweep.rs`, `verify_strategy.rs`, `strategies.rs`, `dispatch.rs` |
| Market data | `market_data.rs`, `market_reference.rs`, `ticks_tests.rs`, `bar_ingest.rs`, `bar_pump.rs`, `screener.rs` |
| Orders | `orders_risk.rs`, `orders_exec.rs`, `orders_gateway.rs`, `trading_state.rs`, `journal_tests.rs`, `trade_journal.rs` |
| Persistence | `profile.rs`, `bootstrap.rs`, `bootstrap_tests.rs`, `runs_store.rs` |
| Chat | `chat_alert_store_tests.rs`, `chat_market_reader_tests.rs`, `ws.rs`, `ws_tests.rs`, `unit_tests.rs` |

## 14. Invariants & Notables

- **Contract-first**: every DTO, error code, and cap lives in `vyapaar-runtime-api`;
  this crate never invents a wire shape.
- **Decimal at the boundary**: broker-feed floats and JSON numbers convert to
  `Decimal` exactly once, at `market.rs`/`orders.rs`.
- **Honest 501s**: unimplemented valuation routes answer `not_implemented`
  (`valuation_engine_mismatch`) instead of fabricating numbers.
- **Gap citations**: chat content stubs name their owning gap docs in code comments.
- **No polling**: clients consume SSE (`resume_from_seq`) or `/ws`; the server pushes.
- **Determinism**: run journals are line-indexed NDJSON so a restart replays exactly.
- Retention trims by both count (`keep_runs = 1_000`) and age (`keep_days = 30`).

## Related

- `code-api.md` — the pure contracts this crate implements
- `code-backtest.md` — the engine behind `BacktestExecutor`
- `code-strategy.md` — manifests the `/strategies` routes compile
- `vyapaar-agent-chat` — chat agent stack reused by `/ws/chat`
- `vyapaar-runtime/docs/gaps/README.md` — open stubs cited above
