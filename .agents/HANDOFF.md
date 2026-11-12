# HANDOFF — vyapaar-runtime

**Date**: 2026-11-12 · Branch: `main` · Full cross-repo context: `.agents/HANDOFF.md` in the
`vyapaar-labs` superproject.

## Gate status — ✅ GREEN (2026-11-12)

- `cargo fmt --check`: ✅
- `cargo clippy --workspace --all-targets --profile prod -- -D warnings`: ✅
- `cargo test --workspace --profile prod`: ✅
- `make prod` (§11 end-of-turn rebuild): ✅ fresh `target/prod`

## Phase / Slice Status

- **Per-crate design docs — complete (2026-11-12, docs-only)**: `docs/design/`
  now has a source-verified design doc for every non-stub crate —
  `architecture.md` (rewritten, 14-crate map + gaps/plans snapshot) plus
  `code-{strategy,backtest,enhancer,actors,jobs,persistence,api,api-rest,config,
  testing,feed}.md`; the stale combined `code-config-testing-feed.md` was
  replaced by the split `code-config.md` / `code-testing.md` / `code-feed.md`
  (feed doc also covers the placeholder stubs `vyapaar-runtime-market`,
  `vyapaar-indicators`, `vyapaar-runtime-test-utils`); `code-migration-map.md`
  kept as historical. Key corrections baked in: audit via injected
  `SqliteAuditLedger` (no `AuditPersistenceActor`), coalescing defaults
  100/50 ms, `JobSpec` has no `Trigger` enum, guardrail gates are DSR ≥ 3.0 /
  PBO < 0.5, 501 `valuation_engine_mismatch` on the four portfolio routes,
  legacy `honba_*` metric prefix. **No Rust code touched** — no `make prod`
  required by §11; the gate status below remains from the 2026-11-12 code pass.
  Mirrored into `vyapaar-docs`: new `runtime_overview.md` + `runtime_chat.md`,
  rewritten `event_loop.md` / `runtime_jobs.md`, corrected
  `strategy_tester_and_enhancer.md` + `api/rest.md` + conformance matrix
  (`mkdocs build --strict` PASS).
- **Agent plan 0006 Phase 4 (options-chain / corporate-actions chat tools) — wiring implemented
  2026-11-12**: research-verified first per plan 0006 §4 (at most 1 subagent) — the plan's
  corporate-actions assumption was **stale**: a historical reader *does* exist Python-side
  (`vyapaar_data`'s `CorporateActionsRepository`/`DuckDbCorporateActionsStore`), while
  `vyapaar-market` has contract-reference/Greeks building blocks but no chain reader. Gaps
  filed before any tool code: `vyapaar-market/docs/gaps/0001-options-chain-reader.md` (that
  repo's first), `vyapaar-data/docs/gaps/0008-corporate-actions-rust-bridge.md` +
  `0009-corporate-actions-ingestion.md`. Landed here: `FeedMarketReader::options_chain` +
  `::corporate_actions`, both honest `PortError::unavailable` stubs citing those gaps;
  seedable `RuntimeMarketReader` gained `set_options_chain` (keyed by underlying) +
  `set_corporate_actions` (empty ⇒ unavailable, same shape as block deals) with
  `chat_adapters_tests.rs` coverage; tracked as items 8–9 of `docs/gaps/0007-*.md`. Catalogue
  count 17 → 19 upstream.
- **Agent plan 0006 Phase 3 (news/filings chat tools) — wiring implemented 2026-11-12**: new
  `crates/vyapaar-runtime-api-rest/src/chat_content.rs::FeedContentReader` implements
  `vyapaar-agent-chat-tools`'s new `ContentReader` port with honest `PortError::unavailable`
  stubs — news cites `vyapaar-data/docs/gaps/0006-news-ingestion.md` (no source anywhere),
  filings cite `vyapaar-data/docs/gaps/0007-filings-rust-bridge.md` (ingested in Python, closed
  gap 0003, but no Rust-consumer bridge — same pending bridge as the `market.rs` sample stores).
  Registered in `ChatRuntime::from_env` via the main `register_read_only_tools` bundle
  (catalogue count 15 → 17 upstream). Tracked as item 7 of `docs/gaps/0007-*.md` (retitled to
  cover the content adapter).
- **Agent plan 0006 Phase 2 (price alerts) — storage decision + wiring implemented
  2026-11-11**: the storage decision the plan demanded: **no new store**. Per
  `docs/gaps/0004-*.md`'s profile-schema precedent (alerts were explicitly closed against the
  opaque `Profile.alerts: Value` field), chat-created alerts live in the **same**
  `Profile.alerts` array `vyapaar-ui-web`'s `alerts-store.ts` already syncs via the `/profile`
  routes — the chat agent is the second writer on the same document. New
  `crates/vyapaar-runtime-api-rest/src/chat_alerts.rs::ProfileAlertStore`: every change is one
  atomic `ProfileStore::update` read-modify-write on the blocking pool (revision bump, atomic
  file rewrite, in-process write lock); acts on `DEFAULT_USER_ID`'s document because the chat WS
  has no bearer identity yet (`docs/gaps/0006`); refuses non-array/garbage `alerts` values with
  explicit errors (never an empty success); 10 tests in
  `tests/chat_alert_store_tests.rs` (round-trip, ui-web `AlertMeta` camelCase shape, duplicate-id
  rejection, triggered-no-op transitions with no revision bump, delete/unknown-id, revision +
  other-field preservation, per-user scoping via `ProfileAlertStore::for_user`).
  `ChatRuntime::from_env` gained a `profiles: Arc<ProfileStore>` parameter and registers the
  alert tools; `bootstrap.rs` passes the store (configured before the chat runtime).
- **Plan 0004 Phases 1–3 (block/bulk deal chat tool) — implemented 2026-11-11** (one line:
  honest `FeedMarketReader::block_deals` stub + seedable `RuntimeMarketReader` store; see
  `docs/plans/0004`). Phase 4 stays blocked on `vyapaar-data` gap 0005; Phase 5 optional after
  that.
- **PLAN-0001 Stages 1–3 + Stage 4** (RunId/journals, worker pool, strategy dispatch, cron
  scheduler, typed manifests): implemented — recorded in `docs/plans/README.md` Implemented
  table. Stage 4's CLI/agent half lives in `vyapaar-ui` (`vyapaar-cli`).
- **GAP-0002/PLAN-0002** (statistical gates, stop-fill engine): closed — recorded in
  `docs/gaps/README.md` + `docs/plans/README.md`.
- **GAP-0003** (Zerodha wire transport via `vyapaar-exec-zerodha`): implemented; Angel One
  remains a simulation scaffold pending `vyapaar-exec-angelone` (upstream placeholder);
  golden-vector conformance over recorded broker sessions deferred.
- **PLAN-0003** (trade-journal & market-reference REST endpoints): implemented — recorded in
  `docs/plans/README.md`; market-reference sample stores pending the `vyapaar-data` → Rust FFI
  bridge (`vyapaar-data` gaps 0001+0002 supply the real data).

## Immediate Next Step

Agent plan 0006 is fully implemented (Phases 1–4). Remaining chat-tool data blockers are all
upstream gaps, tracked in `docs/gaps/0007-*.md`: `vyapaar-market/docs/gaps/0001` (options-chain
reader) for `options_chain`; `vyapaar-data/docs/gaps/0008`+`0009` (Rust bridge + ingestion) for
`corporate_actions`; `vyapaar-data/docs/gaps/0005` for plan 0004 Phase 4 (real block/bulk data);
`vyapaar-data/docs/gaps/0006`+`0007` for the content tools. When chat sessions gain bearer
identity (`docs/gaps/0006`), switch `ProfileAlertStore::for_user` to the connection's
`token_id`.

## Do not regress

- Zero in-file `#[test]` / `#[cfg(test)]` in `crates/*/src/`.
- Strict canonical `prod` profile (`target/prod`); `make prod` before saying done.
- Zero floats in all financial accounting (`rust_decimal::Decimal`); no wall-clock/random inside
  sim state machines — state transitions take explicit `DateTime<Utc>`/IDs from callers.
- ADR 0018 order gate, algo registration in the executor, `TestBarReader` fixture bars,
  `service.rs` queue-full `RateLimited`, `live_quotes.rs::read_fallback`.
- WebSocket tests must always give the awaited stream a timeout (a hung `ws_stream.next()`
  blocks the whole workspace run).
- Explicit clock at the adapter boundary: `BrokerAdapter::{submit_order, cancel_order}` take
  `now: DateTime<Utc>`; per-venue monotonic `FILL-####` fill IDs; caller-owned
  `client_order_id` minting (`OrderIntent::into_order_with_id` /
  `Dispatcher::dispatch_intent_with_id`).
- One `vyapaar serve` per profiles directory (in-process-only write lock); the chat agent's
  alert store depends on the same constraint (`chat_alerts.rs`).

## Decisions & Discoveries

- **Docs source of truth**: `vyapaar-runtime/docs/design/code-*.md` are the
  per-crate references; vyapaar-docs pages link to them rather than
  duplicating facts (single writer per fact). Gap citations inside the design
  docs name the owning repos' `docs/gaps/*` (`vyapaar-data` 0005–0008,
  `vyapaar-market` 0001).
- **Alert storage = `Profile.alerts`, no new store**: gap 0004 had already mapped ui-web's
  `AlertMeta[]` onto the opaque field with no server change; making the chat agent a second
  writer reuses that contract instead of duplicating a store. DTO round-trips ui-web exactly
  (camelCase, decimal-string `price`, `createdAt` date string, `triggeredAt` optional ISO).
  Port contract: caller mints ids, duplicates rejected; `Triggered` never flipped by
  enable/disable; unknown id → `Ok(None)`.
- `BrokerAdapter` trait has no consumers outside `vyapaar-runtime-strategy` (verified) —
  signature extensions are safe; the pairwise-adapter economic-parity test intentionally stays
  on the random-ID path.
- `vyapaar_runtime_api::chat_adapters::RuntimeMarketReader` is a seedable test adapter: empty
  block-deals store ⇒ `PortError::unavailable` (gap 0005 citation); seeded ⇒ filter matches.
  Same honest-stub shape as `FeedMarketReader` (gap 0007 item 6). Agent plan 0006 Phase 4 added
  the same shape for `set_options_chain` (keyed by underlying, empty ⇒ unavailable citing
  `vyapaar-market/docs/gaps/0001`) and `set_corporate_actions` (empty ⇒ unavailable citing
  `vyapaar-data/docs/gaps/0008`); expiry filters match on calendar day, not exact instant.
- `chat_content.rs::FeedContentReader` takes no ports and answers every call with an explicit
  `unavailable` (gap 0006 news / gap 0007 filings citations): `ContentReader` is a new port with
  exactly one implementor (alert-port precedent — no seedable test adapter until a consumer
  needs one), so it needs nothing from `AppState` today.
