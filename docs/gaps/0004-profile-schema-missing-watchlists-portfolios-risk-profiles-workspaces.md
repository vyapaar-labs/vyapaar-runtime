# 0004 — Profile schema has no room for watchlists metadata, portfolios, risk profiles, or workspaces

**Status**: open

## What's stubbed or hardcoded

`vyapaar-ui-web` (the Next.js frontend) has six localStorage-only persistence stores
(`src/lib/{watchlists,settings,portfolios,risk-profiles,alerts,workspaces}-store.ts`). Of these,
`settings-store.ts`'s `SettingsPreferences` and `alerts-store.ts`'s `AlertMeta[]` map cleanly onto
the existing `Profile.preferences: Value` and `Profile.alerts: Value` fields
(`crates/vyapaar-runtime-api/src/profile.rs`) — both are already opaque JSON, so no server change
was needed to sync them (closed via the `/profile` routes, see the vyapaar-ui-web side of this
work).

The other four do not fit the current `Profile` shape:
- `watchlists-store.ts`'s `WatchlistMeta { id, workspaceId, name, instrumentIds }` (one or more
  named lists per workspace) does not fit `Profile.watchlists: BTreeMap<String, Vec<String>>`,
  which is just `name -> symbols` with no room for a stable `id` or `workspaceId`.
- `portfolios-store.ts`, `risk-profiles-store.ts`, and `workspaces-store.ts` have no corresponding
  `Profile` field at all.

## Why

`Profile`'s `watchlists` field (and the rest of the schema) was ported from `honba-ya` before
`vyapaar-ui-web`'s richer, workspace-scoped store shapes existed, and nothing in either ecosystem
has ever consumed the `/profile` API over HTTP (there was no runnable server binary until this
batch added `vyapaar serve` to `vyapaar-cli`) — honba-fe doesn't wire it either, so the mismatch was
never exercised. `Profile.extra` (the `#[serde(flatten)]` catch-all) exists only to round-trip
fields a different client/server version doesn't know about; using it to intentionally store
portfolios/risk-profiles/workspaces would be a correctness-relevant abuse of that contract, not a
fix.

## How to close it

In `crates/vyapaar-runtime-api/src/profile.rs`:
1. Change `Profile.watchlists`'s value type from `Vec<String>` (bare symbol list) to a small
   struct carrying `name: String` and `instrument_ids: Vec<String>`, keyed by the watchlist's own
   stable id (workspace scoping can be a field on that struct, or a further level of nesting —
   whichever keeps `crates/vyapaar-runtime-api-rest/src/profile.rs`'s `check_profile`/`name_problem`
   validation straightforward).
2. Add new fields for `portfolios`, `risk_profiles`, and `workspaces` (opaque `Value`, matching the
   precedent set by `preferences`/`alerts`/`workbench`, unless/until any of those three need
   server-side structure).
3. Update `crates/vyapaar-runtime-api-rest/src/profile_store.rs`'s `encode_within_limits` count
   limits (`MAX_WATCHLISTS`, `MAX_SYMBOLS`, and new limits for the added fields) and
   `crates/vyapaar-runtime-api-rest/tests/profile.rs` (the existing watchlist-shape assertions,
   e.g. `put_profile_validates_map_keys_like_the_sub_resources`, assume the old `Vec<String>`
   value type).
4. Wire `vyapaar-ui-web`'s `watchlists-store.ts`, `portfolios-store.ts`, `risk-profiles-store.ts`,
   and `workspaces-store.ts` to the updated fields the same way `settings-store.ts`/
   `alerts-store.ts` were wired to `preferences`/`alerts`.
