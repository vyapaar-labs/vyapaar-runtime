# `vyapaar-runtime-persistence` — Persistence Reference

Crate `crates/vyapaar-runtime-persistence/` (11 src files, ~3.4k lines;
`migrations/{sqlite,postgres}` SQL pairs).
Crate doc: *"sqlx persistence for `vyapaar-runtime` (PostgreSQL / SQLite): relational
repositories, an embedded schema-migration runner, and the SEBI append-only regulatory audit
ledger. All monetary values are persisted as exact decimal strings (`DECIMAL(30,16)` on
PostgreSQL, `TEXT` on SQLite) and all timestamps are passed explicitly as
`chrono::DateTime<Utc>` parameters."*

## 1. Purpose

- Typed relational stores for orders, fills, FIFO tax lots, traders, LLM/strategy decisions,
  and agent chat sessions.
- Four `vyapaar_ledger::repository` trait implementations (DDD adapters over in-memory caches
  with an async `persist_to_db` write-back step).
- The append-only SEBI audit ledger for exchange compliance inspections.

## 2. Module Map

| File | Contents |
|---|---|
| `lib.rs` | Docs, modules, re-exports, repository type aliases, `Database` enum + `connect`/`is_sqlite`/`ping`, `with_executor!` macro, `parse_instrument` |
| `messages.rs` | `RiskBreachEvent` mirror (field-for-field of the actor message; breaks the actors → persistence dependency cycle) |
| `migrations.rs` | Embedded backend-aware migration runner (`migrate`, `rollback_last`, `connect_and_migrate`) |
| `audit.rs` | `SqliteAuditLedger` — SEBI append-only audit trail + `AuditLedgerRecord` + `AuditError` |
| `traders.rs` | `TraderStore` / `PersistedTrader` |
| `orders.rs` | `OrderStore` / `PersistedOrder` — idempotent upsert, state-machine transitions |
| `fills.rs` | `FillStore` / `PersistedFill` — idempotent fill ingest + exact Decimal order aggregation |
| `positions.rs` | `PositionStore` / `PersistedPosition` / `PersistedPositionLot` — long-only FIFO tax-lot ledger |
| `decisions.rs` | `DecisionStore` / `PersistedDecision` / `sha256_hex` |
| `sessions.rs` | `SqlxSessionStore` — agent chat sessions/messages implementing `vyapaar_agent_core::ports::SessionStore` |
| `domain_stores.rs` | DDD repository adapters over the stores above |

Type aliases: `OrderRepository = OrderStore`, `FillRepository = FillStore`,
`PositionRepository = PositionStore`, `DecisionRepository = DecisionStore`,
`TraderRepository = TraderStore`.

## 3. `Database`

```rust
pub enum Database { Postgres(PgPool), Sqlite(SqlitePool) }
```

- `connect(url)`: `postgres://`/`postgresql://` → `PgPoolOptions::max_connections(16)`;
  `sqlite:` → `create_if_missing(true)`, `busy_timeout(5s)`, **WAL journal mode for file-backed
  only**; `:memory:` pinned to 1 connection (file-backed max 10).
- `ping()` = `SELECT 1`; `with_executor!` runs the same body against either backend.

## 4. Migrations

- PG: **7 versions** — `001_traders`, `002_orders`, `003_fills`, `004_decisions`,
  `005_positions`, `006_audit_ledger`, `007_agent_chat`.
- SQLite: **6 versions** — same names 1–5, `006_agent_chat`. The SQLite `sebi_audit_ledger`
  table is deliberately **not** in the migration set — it is created at runtime by
  `SqliteAuditLedger::init_schema`.
- Applied transactionally per version against `schema_migrations(version, name, applied_at)`;
  idempotent; `rollback_last` executes the down SQL and deletes the marker;
  `connect_and_migrate(url, rollback)` is the bootstrap entry.

## 5. Stores

### `OrderStore`
- `upsert(req, ts)` — idempotent on `client_order_id` (pre-read + `ON CONFLICT DO NOTHING` +
  re-read); inserts with status `pending_new`, `filled_quantity = 0`.
- `update_status(client_order_id, from, to, ts)` — validates `is_legal_transition`, then an
  atomic compare-and-set (`WHERE ... AND status = from`); 0 rows ⇒ `InvalidStateTransition`
  with the actual current status. Legal edges: `PendingNew→{New,Cancelled,Rejected,Expired}`;
  `New→{PartiallyFilled,Filled,Cancelled,Rejected,Expired}`;
  `PartiallyFilled→{PartiallyFilled,Filled,Cancelled,Rejected,Expired}`; terminal states have
  no outgoing edges.
- `attach_broker_order_id`, `get_by_client_order_id`, `get_by_broker_order_id`,
  `fills_for_order`.
- Encoders: order type as `market` / `limit:<p>` / `stoploss:<p>` / `stoplimit:<trigger>:<limit>`
  (exact Decimal round-trip); TIF `day`/`ioc`/`gtc`; side `buy`/`sell`.
- `PersistedOrder` exposes typed accessors (`status()`, `side()`, `order_type()`, `tif()`,
  `quantity()`, `avg_fill_price()`, `instrument()`, `client_order_id_uuid()`).

### `FillStore`
- `insert(fill, fee_breakdown_json, ts)` — one transaction: in-tx idempotency re-check on
  `trade_id`, `INSERT ... ON CONFLICT(trade_id) DO NOTHING`, then **exact Decimal aggregation**
  of the order's `filled_quantity = Σ qty` and `avg_fill_price = Σ(q·p)/Σq`, promoting the order
  to `filled` when Σqty ≥ order qty (and status ∈ {new, partially_filled}).

### `PositionStore` (long-only FIFO tax-lot ledger)
- `apply_fill(instrument, client_id, side, qty, price, ts)` — one transaction: Buy appends a
  `position_lots` row; Sell consumes lots `ORDER BY id ASC` (partial updates or deletes),
  realizes `Σ (price − lot_entry) × matched` exactly; oversell ⇒ `tx.rollback()` +
  `InsufficientPosition`; recomputes open qty and WAC from the lot ledger and upserts the
  `positions` row.
- `get_position`, `get_lots` (FIFO order).

### `TraderStore`
- `upsert(client_id, algo_id, risk_profile, capital: Decimal, ts)` (PG `ON CONFLICT(client_id)
  DO UPDATE ... RETURNING`; SQLite upsert + re-read); `get_by_client_id`, `get_by_algo_id`,
  `list`. Capital decoded via `CAST(capital AS TEXT)` → `Decimal::from_str`.

### `DecisionStore`
- `sha256_hex(prompt) -> String` (lowercase hex); `insert(decision_id, algo_id, prompt_sha256,
  model_name, seed, temperature: Decimal, decision_kind, spec_json, ts)` — validates the
  64-hex digest, idempotent on `decision_id`; `get_by_id`, `list_by_algo(algo_id, limit)`
  (newest first). Cryptographic replay support for AI decisions (seed + temperature +
  prompt hash).

### `SqliteAuditLedger` (SEBI append-only audit)
- `new_in_memory()` (pool max 5) / `new_file(path)` (max 10) / `from_pool`;
  `init_schema` creates `sebi_audit_ledger` + 4 indices (algo, correlation, timestamp, event).
- Writers (`-> Result<i64, AuditError>`): `record_order_attempt`, `record_risk_refusal`
  (with `eval_duration_us`), `record_risk_breach_event`, `record_broker_ack` (with dispatch
  duration), `record_cancellation`, `record_modification`, `record_fill`.
- Readers: `query_by_algo_id` / `query_by_correlation_id` / `query_by_event_type` / `query_all`
  (ordered by timestamp), `count()`, `export_ndjson(writer)`.

### `SqlxSessionStore` (agent chat)
- Implements `vyapaar_agent_core::ports::SessionStore`: `create`, `append` (transactional,
  updates `updated_at`), `messages`, `most_recent`, `list` (filter on profile/mode/archived,
  newest first, default limit 50), `rename`, `update_provider`, `truncate_after`.
- `RuntimeMode` mapped to `live`/`paper`/`backtest`; unique-violation ⇒ `StoreError::conflict`.

### DDD domain-store adapters (`domain_stores.rs`)
Implement `vyapaar_ledger::repository` traits (synchronous trait objects over in-memory
`RwLock` caches; SQL persistence is the separate `persist_to_db` step):
- `SqlxOrderStore: OrderRepository` — `get_order` / `save_order` / `list_active_orders` +
  async `persist_to_db` (translates `OrderRecord` → upsert/status/broker-id updates).
- `SqlxPositionStore: PositionRepository` — `get_positions` / `save_lot` / `match_lots` +
  async `persist_positions_to_db` (replays each non-exhausted lot as a Buy).
- `SqlxAccountStore: AccountRepository` — `get_account` / `save_account` / `list_accounts` +
  async `persist_to_db` → `TraderStore::upsert`.
- `SqlxLedgerStore: LedgerRepository` — `post_entry` / `get_balance` (in-memory delegate,
  `new(db, base_currency)`).

## 6. SQLite vs PostgreSQL Split

Single `Database` enum; every store branches on the variant. PG: `$N` placeholders,
`DECIMAL(30,16)` (read back via `CAST(... AS TEXT)`), native `TIMESTAMPTZ`, `RETURNING` where
available. SQLite: positional binding, RFC-3339 text timestamps, upserts re-read after write.

## 7. Dependencies

`vyapaar-core`, `vyapaar-ledger` (repository traits, `MatchedLot`/`TaxLot`),
`vyapaar-agent-core` (`ports::SessionStore`); `sqlx 0.8` (runtime-tokio-rustls, postgres,
sqlite, uuid, chrono, rust_decimal, macros, migrate), `async-trait`, `chrono`,
`serde`/`serde_json`, `uuid`, `rust_decimal`, `thiserror`, `tracing`, `sha2 0.10`, `tokio`
(sync + time features).

## 8. Tests (`tests/`, 11 files)

| File | Coverage |
|---|---|
| `unit_tests.rs` | In-memory SQLite `Database::connect` smoke test |
| `migrations_tests.rs` | Apply + rollback on SQLite; unsupported URL scheme rejected |
| `traders_tests.rs` | Upsert + lookups; repository alias compile check |
| `orders_tests.rs` | Upsert idempotency; full legal transition matrix + illegal rejections; broker-id roundtrip; concurrent upserts dedupe |
| `fills_tests.rs` | Partial→full aggregation + auto-promotion; ordering; single-fill promotion |
| `positions_tests.rs` | FIFO 3-buys + partial-sell exact math; oversell fails + rollback; per-client isolation |
| `decisions_tests.rs` | `sha256_hex` known vector; idempotent insert; invalid digest rejected; listing order |
| `audit_tests.rs` | Full audit-ledger lifecycle (record + query) |
| `sessions_tests.rs` | Session lifecycle + messages (create/append/rename/truncate/list) |
| `domain_repository_tests.rs` | All four DDD repository adapters |
| `concurrency_tests.rs` | Concurrent persistence transactions under load |

## 9. Invariants & Notables

- **Zero floats**: every monetary/quantity column is an exact decimal string;
  `DECIMAL(30,16)` on PG; decoding via `Decimal::from_str`.
- **Explicit `DateTime<Utc>`** on every mutating method (exceptions: session-store metadata and
  some audit writers use `Utc::now()` for row timestamps, never for domain arithmetic).
- **Idempotency everywhere**: orders on `client_order_id`, fills on `trade_id`, decisions on
  `decision_id` — all `ON CONFLICT DO NOTHING` + re-read (concurrency-safe).
- **No snapshot/journal replay and no git2 versioning in this repo** — "restoring" state means
  reading rows back through the typed accessors; `SqlxPositionStore::persist_positions_to_db`
  is the only write-back bridge from the in-memory domain ledger to SQL.
- `RiskBreachEvent` is duplicated (not imported) from the actors crate to invert the dependency.

## Related

- `code-actors.md` — audit-ledger consumers
- `vyapaar-ledger` — repository trait definitions
- `vyapaar-agent-core` — `SessionStore` port
