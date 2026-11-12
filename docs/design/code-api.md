# `vyapaar-runtime-api` — API Contract Reference

Crate `crates/vyapaar-runtime-api/` (18 src files, ~5k lines, 4 test files).

## 1. Purpose

The pure "API contract" crate — zero HTTP. It defines:

- the serde wire model (envelope, error codes, DTOs),
- the 44-endpoint capability registry,
- the run-id / manifest / journal domain model,
- request validation and resolution (limits, defaults),
- strategy-IR compilation and screener AST evaluation,
- the reader-adapter traits (`JournalWriter`, `RunExecutor`, `PortfolioReader`,
  `MarketReader`) that `vyapaar-runtime-api-rest` implements.

Both the Axum server and the Python FFI dispatch layer speak these types, so client and server
cannot drift.

## 2. Module Map

| File | Contents |
|---|---|
| `lib.rs` | Re-exports + `check_ingest_batch` / `check_scan_budget` / `check_screener_rows` helpers |
| `run_id.rs` | `RunId` (Crockford Base32, `RUN_ID_LEN = 26`), `RunIdGenerator`, `RunIdError` |
| `run_ports.rs` | `JournalWriter`, `RunExecutor`, `RunJob`, `RunOutcome` |
| `verify.rs` | Strategy verify logic (manifest → verified IR) |
| `capabilities.rs` | `CapabilityManifest` |
| `ingest.rs` | `MAX_INGEST_BARS = 1000`, `check_ingest_batch` |
| `profile.rs` | `Profile`, `LayoutDocument`, `Module`, `Source`, `ProfileRevision`, `LayoutsResponse`, `ModulesResponse` |
| `strategies.rs` | `StrategyCatalog`, `MAX_COMPILED_STRATEGIES = 1000`, SHA-256 content ids, `StrategyRecord` |
| `wire.rs` | `SCHEMA_VERSION = "4"`, `API_VERSION = "v1"`, `ResponseEnvelope`, `ErrorCode` (16 variants), `ErrorDetail`, `TradingState`, endpoint registry types, event/DTO types |
| `runs.rs` | `MANIFEST_VERSION = 1`, `RunEvent`, `TransitionError`, `RunStatus::next()` transition table, `RunKind`, `RunManifest` |
| `run_resolve.rs` | `DEFAULT_BAR_SPEC = "1d"`, `FILL_MODELS` (4), `DEFAULT_INITIAL_CAPITAL = 1_000_000`, `MAX_COMMISSION_PER_FILL = 1e9`, `MAX_SLIPPAGE_BPS = 10_000`, `ResolvedBacktest`/`ResolvedSweep`/`ResolvedRequest`, `unknown_strategy` |
| `requests.rs` | Query/request DTOs (`OrdersRequest`, `TradingStateRequest`, `BarsIngestRequest`, `BacktestEventsQuery`, `ScreenerQuery`) |
| `market.rs` | `DEFAULT_TIMEFRAME = "1m"`, `DEFAULT_DEPTH_LEVELS = 5`, `MAX_DEPTH_LEVELS = 50`, `ResolvedQueries`, `parse_timeframe`/`parse_bound`/`parse_instrument_id`/`instrument_json` |
| `strategy_ir.rs` | `StrategyManifest` (API-local), `StrategyIr::compile`, `IrError` |
| `cache.rs` | `CacheKey::compute`, `CachedResult`, `ResultCache`, `canonical_json_bytes` |
| `chat_adapters.rs` | `LedgerPortfolioReader` (5 methods), `RuntimeMarketReader` (9 methods + seed mutators), `RuntimeSnapshotSource` |
| `screener.rs` | Limits (universe 1,000 / rows 500 / bars 2M), `ScreenerFilterGroup`/`Predicate`, `ScreenerError`, `evaluate`, SMA/RSI metrics |
| `responses.rs` | ~40 response DTOs |

## 3. Wire Model (`wire.rs`)

- `ResponseEnvelope<T> { schema_version: "4", api_version: "v1", data, error }` — exactly one
  of `data`/`error`; `deny_unknown_fields`.
- `ErrorCode` (16 variants): `validation_invalid_request`, `unauthorized`, `not_found`,
  `conflict`, `not_implemented`, `unsupported`, `internal_error`, `timeout`,
  `risk_max_notional_exceeded`, `risk_trading_halted`, `risk_reduce_only_violation`,
  `risk_price_collar`, `risk_lot_multiple_violation`, `risk_tick_size_violation`,
  `risk_instrument_unknown`, `rate_limited`. `retryable()` = `Timeout` | `RateLimited`.
- `ErrorDetail { code, message, context }` + `with_context`.
- **Endpoint registry**: `Endpoint { path, method, access, params }`; `ENDPOINTS` holds 44
  entries; `WRITE_PATHS` = 5; `write_endpoints()` derives the write set — served at
  `GET /capabilities` and drift-tested.
- Event stream: `Message { schema_version, ts_event, event }`; `Event::Status(RunEvent)`
  | `Event::Fill(Trade)`; `Trade` is the transport `FillReport` (`From<FillReport>`), naming
  the instrument `instrument_id`.
- DTOs: `QuoteTick`, `Position`, `PortfolioAccount/Position/Holding`,
  `BacktestMetrics { trades, net_pnl, sharpe, max_drawdown, total_return }`, `ValidationBlock`,
  `Instrument`/`InstrumentKind`, `BarSpecification { step, aggregation, price_type }`,
  `StreamFrame`, `TradingState { Running, Halted, ReduceOnly }`.

## 4. Run Domain

- `RunId` — validated 26-char Crockford Base32 id with checksum
  (`RunIdError::{TooLong, TooShort, InvalidChar, InvalidChecksum, Decode}`);
  `RunIdGenerator::generate` mints monotonic ids.
- `RunStatus` — `Pending → Running → (Succeeded | Failed | Cancelled)`; `is_terminal()`,
  `next()` transition table (`TransitionError::InvalidTransition`).
- `RunKind { Backtest, Sweep }`; `RunEvent` lifecycle events; `RunManifest { schema_version,
  run_id, kind, status, strategy_id, strategy_name, request_json, created_at, started_at,
  finished_at, error, journals_dir, pid, host }` — written atomically per run.
- `JournalWriter` / `RunExecutor` ports: `RunJob { strategy_id, strategy: CompiledStrategy,
  request }` → `RunOutcome::Backtest { metrics, assumptions }` | `RunOutcome::Sweep { report }`.
- Resolution: `ResolvedBacktest { strategy_id, universe, start, end, bar_spec,
  initial_capital, seed, fill_model, commission_per_fill, slippage_bps }`; `ResolvedSweep`
  adds `params` + `trials`; validation limits (`MAX_SLIPPAGE_BPS = 10_000`, etc.);
  `unknown_strategy(name) -> ErrorDetail`.
- Result cache (ADR 0020): `CacheKey::compute(strategy_id, params_json, dataset_id,
  cost_model_version)` → `CachedResult { metrics, assumptions, journal_path }`; TTL'd
  in-memory `ResultCache`; `cost_model_version = "1"`.

## 5. Strategy Compilation & Verification

- `StrategyCatalog { store, max = 1000 }`: `compile(request)` → `StrategyRecord` with the
  SHA-256 of the canonical manifest JSON as id (same manifest ⇒ same id, stored once);
  `list`/`get`/`has`. A catalog-full insert is a typed error.
- `verify(manifest)` → verified IR or a stable 422 error.
- `StrategyIr::compile` (API-local manifest shape) with `IrError` stable codes.

## 6. Screener AST

`ScreenerQuery.resolve() -> ResolvedScan`; filters are groups of predicates (`price_gt/lt`,
`sma_cross_above/below`, `rsi_above/below`, `volume_gt`, …); `evaluate(instrument, &bars)` →
`Option<ScreenerResultRow>`; budget guards `check_scan_budget` / `check_screener_rows`
(universe ≤ 1,000 instruments, rows ≤ 500, bars ≤ 2,000,000) and `check_ingest_batch`
(≤ 1,000 bars).

## 7. Chat Reader Adapters (`chat_adapters.rs`)

- `LedgerPortfolioReader` — `snapshot` / `positions` / `holdings` / `trades` / `accounts`.
- `RuntimeMarketReader` — `quote` / `depth` / `bars` / `instruments` / `instrument` /
  `calendar` / `search` / `ticks` / `options_chain`, plus `set_*` seed mutators for tests.
  Unseeded sources and options chains return **honest `PortError::unavailable`** stubs citing
  the owning gaps (`vyapaar-runtime/docs/gaps/0007`, `vyapaar-market/docs/gaps/0001`,
  `vyapaar-data/docs/gaps/0008`).
- `RuntimeSnapshotSource` — chat snapshot feed.

## 8. Dependencies

`vyapaar-core`, `serde`, `serde_json`, `rust_decimal`, `chrono`, `thiserror`, `sha2`,
`uuid` (v4). Dev: `proptest`.

## 9. Tests (`tests/`, 4 files)

| File | Coverage |
|---|---|
| `run_id_tests.rs` | `RunId` validation/generation, Crockford Base32, checksum errors |
| `runs_wire.rs` | Wire-compat: `error` field optional, old payloads parse |
| `unknown_fields.rs` | ADR 0012: envelopes tolerate extra fields; typo'd request bodies refused |
| `chat_adapters_tests.rs` | Portfolio/market reader adapter behaviour incl. seeded stores |

## 10. Invariants

- Envelope + `ErrorCode` taxonomy is the single wire contract for every route and every
  client SDK.
- Monetary values serialize as strings (`Decimal` exactness).
- Caps live here, not in the server: ingest 1,000 bars, screener 1,000/500/2M, strategies
  1,000, slippage ≤ 10,000 bps.
- Registry drift is a test failure — `ENDPOINTS` must match the served router.

## Related

- `code-api-rest.md` — the Axum implementation of these contracts
- `code-jobs.md` — the run lifecycle consumed by job-driven runs
- `vyapaar-core` — shared `FillReport`/`OrderRequest` types behind the DTOs
