# HANDOFF — vyapaar-runtime

**Date**: 2026-11-11 · Branch: `main` · Full cross-repo context: `.agents/HANDOFF.md` in the
`vyapaar-labs` superproject.

## Gate status — ✅ GREEN (2026-11-11)

- `cargo fmt --check`: ✅
- `cargo clippy --workspace --all-targets --profile prod -- -D warnings`: ✅
- `cargo test --workspace --profile prod`: ✅
- `make prod` (§11 end-of-turn rebuild): ✅ fresh `target/prod`

## Phase / Slice Status

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

Support `vyapaar-agent` plan 0006 Phase 3 (news/filings search tools) once its upstream check
against `vyapaar-data` (filings reader, closed gap 0003) decides tool-layer-only vs new-gap.
Plan 0004 Phase 4 (real block/bulk deal data) remains blocked on `vyapaar-data` gap 0005. When
chat sessions gain bearer identity (`docs/gaps/0006`), switch `ProfileAlertStore::for_user` to
the connection's `token_id`.

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
  Same honest-stub shape as `FeedMarketReader` (gap 0007 item 6).
