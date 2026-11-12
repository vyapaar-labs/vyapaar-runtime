# `vyapaar-runtime-feed` — Market-Data Storage Shim Reference

Crate `crates/vyapaar-runtime-feed/` (1 src file, 30 lines).

## 1. Purpose

A pure re-export shim. The Parquet tick store, the market-data catalog, and the
local wire types live in `vyapaar-market-store` (the `vyapaar-market` repo,
Tier 2A). This crate keeps the historical `vyapaar_runtime_feed` import path
working for `vyapaar-runtime-api-rest` and other runtime consumers, so runtime
crates don't have to be renamed in one sweep.

## 2. Public API (all re-exports of `vyapaar_market_store`)

### Root re-exports

| Symbol | Role |
|---|---|
| `ParquetTickStore` | columnar tick/bar storage on Parquet files |
| `Lakehouse`, `LakehouseCompression`, `LakehouseError` | dataset catalog over stored data |
| `TickRecorder`, `RecorderConfig`, `RecorderStats` | live tick capture into the store |
| `CatalogInstrumentMaster`, `CatalogError` | instrument master backed by the catalog |
| `LiveQuoteStore` | latest-quote read model |
| `Dataset`, `DatasetFeed`, `DatasetReader`, `DatasetChunk`, `DatasetBuildError`, `StreamedDatasetFeed` | run-time bar/tick iteration |
| `InMemoryTickReader` | in-memory reader for tests |
| `ParquetBarSource`, `ParquetError`, `ReaderError`, `TickStoreError` | source/error types |
| `ColumnarSlice`, `ColumnarSliceBuilder` | zero-copy column access |
| `ReportWriter`, `CsvReportWriter`, `JsonReportWriter`, `MarkdownReportWriter`, `ExportError` | report writers |
| `TICK_DIR`, `tick_schema`, `snapshot_schema`, `write_tick_file` | layout + flat tick files (`VERSION = 1`) |

### Sub-modules

- `ports` — `vyapaar_market_store::ports::*` (local port traits and data types).
- `import` — Parquet import helpers (historical `vyapaar_runtime_feed::import` path).
- `wire` — local market-data wire types.

## 3. Data Flow

`DataSourceConfig::Local { root }` ([`code-config.md`](code-config.md)) →
`Lakehouse`/`DatasetReader`/`ParquetBarSource` → bars/ticks into
`vyapaar-runtime-backtest`; `TickRecorder` + `write_tick_file` persist live
captures; `CatalogInstrumentMaster` answers `/instruments`.

## 4. Dependencies

`vyapaar-market-store` only — this crate has no logic of its own.

## 5. Tests

None in-crate; coverage lives with `vyapaar-market-store` (owner of the real
implementations) and the api-rest integration tests that read Parquet through
this path (`tests/rest_backtest_l2_parquet.rs`).

## 6. Placeholder Crates in This Workspace

The runtime workspace still carries three stub crates whose real content lives
elsewhere; documented here so nobody re-implements them in the wrong repo:

| Stub crate | Current content | Real home |
|---|---|---|
| `vyapaar-runtime-market` | `// Stub crate: vyapaar-me-market` | `vyapaar-market` repo (tick coalescing, L2/L3 depth) |
| `vyapaar-indicators` | `// Stub crate: vyapaar-indicators` | `vyapaar-market` repo (indicator library) |
| `vyapaar-runtime-test-utils` | `// Stub crate: vyapaar-testing` | superseded by `vyapaar-runtime-testing` |

## 7. Invariants & Notables

- **No logic**: the shim exists only so the import path survives; adding logic
  here would duplicate `vyapaar-market-store`.
- `vyapaar-runtime-feed` must not be confused with the *live feed* subsystem:
  broker WebSockets are `vyapaar-market`; this is the *storage + read* layer.

## Related

- `code-config.md` — `DataSourceConfig` that selects the local root
- `code-api-rest.md` — the HTTP consumer of the catalog and tick files
- `code-backtest.md` — the engine that iterates `DatasetFeed`s
- `vyapaar-market` repo — owner of the re-exported implementations
