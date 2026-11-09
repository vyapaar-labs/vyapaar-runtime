# 0003 — REST endpoints for trade-journal analytics and market reference data

**Status**: Proposed

---

## 0. Executive Summary & Objective

`vyapaar-ui-web` (the Next.js web UI, a sibling repo) is planning to replicate
vela.luxalgo.com's Journal / Calendar / Reports features — see its own
`vyapaar-ui-web/docs/plans/0011-vela-journal-calendar-reports-domain-model.md`.
That UI needs two kinds of data this repo does not yet serve over HTTP:

1. **Trade-journal analytics**: per-trade annotations (tags, rating, review,
   planned risk / R-multiple) and derived performance metrics (win rate,
   profit factor, expectancy, edge score, drawdown, streaks, a daily P&L
   calendar, and account-vs-account comparison).
2. **Market reference data**: company fundamentals ("symbol profile") and an
   economic/earnings/dividends/splits/IPO calendar.

Neither concept is domain logic this repo owns. The actual domain logic and
data live upstream:

- `vyapaar-ledger/docs/plans/0002-trade-performance-journal.md` — a new pure
  domain crate, `vyapaar-ledger-performance`, computing trade annotations and
  the performance-metrics/edge-score math this plan's endpoints would expose.
- `vyapaar-data/docs/gaps/0001-economic-calendar-ingestion.md` — economic
  calendar ingestion.
- `vyapaar-data/docs/gaps/0002-company-fundamentals-symbol-profile.md` —
  company fundamentals / symbol-profile data.

This plan's objective is narrow: define the **REST wrapper layer** in
`vyapaar-runtime-api-rest` that exposes those upstream capabilities to
`vyapaar-ui-web`, once each upstream dependency lands. This plan is not the
source of truth for any of the underlying math or data — it only specifies
the HTTP surface, request/response shapes, and which existing handler files
the new routes belong in.

### Naming-collision warning (read this before implementing)

`vyapaar-runtime-api-rest` already has **two unrelated things both called
"journal"**, and this plan's new routes must not collide with either:

1. `crates/vyapaar-runtime-api-rest/src/journal.rs` — the NDJSON *run-journal*
   writer (`NdjsonJournal`, `FileJournalWriter`, implements
   `vyapaar_runtime_api::JournalWriter`): one line per backtest/strategy-run
   event (`events.ndjson`).
2. `crates/vyapaar-runtime-api-rest/src/operator_journal.rs` — a separate
   append-only audit log of accepted `PUT /trading-state` changes (operator
   halts/resumes, per ADR 0018).
3. Existing live routes already using the word "journal":
   `GET /backtests/:id/journal`, `GET /sweeps/:id/journal`,
   `GET /journals/:id` — all in `crates/vyapaar-runtime-api-rest/src/v1.rs`,
   all serving run/backtest *execution* journals. Completely unrelated to
   trade performance.

To keep these three meanings visually distinct in the crate's own file
listing:

- All new routes in this plan use a **`/trade-journal/*`** prefix. Never a
  bare `/journal*` path.
- The new handler source file is named **`trade_journal.rs`** — never
  `journal2.rs` or anything journal-adjacent-but-ambiguous.

---

## 1. Context & Gaps Covered

### 1.1 What this repo already has (verified against `crates/vyapaar-runtime-api-rest/src/v1.rs`)

Current route table, so this plan proposes only genuinely missing endpoints:

```
GET  /capabilities, /health, /schema
GET  /instruments, /instruments/:id, GET /quotes, GET /bars/:id, GET /depth/:id, POST /bars
GET/POST /profile/layouts, /profile/modules  (and nested variants)
GET  /portfolio, /portfolio/accounts, /portfolio/positions, /portfolio/holdings, /portfolio/trades,
     /portfolio/chart, /portfolio/concentration, /portfolio/wac
GET  /ledger, /tax/report, /fees
GET  /positions, /positions/audit, POST /positions/close
GET  /trades
GET  /reports/ledger, /reports/tax, /reports/snapshots, /reports/audit, /reports/concentration,
     /reports/wac, /reports/fees, /reports/equity-curve, /reports/fills/:order_id
POST/GET /strategies, POST /strategies/verify
GET  /backtests/:id, /backtests/:id/journal, /backtests/:id/events
POST/GET /sweeps, GET /sweeps/:id, /sweeps/:id/journal
POST /orders, GET /orders, DELETE /orders/:id, GET /orders/:id/fills
GET  /approval/queue, POST /approval/approve, POST /approval/reject
GET  /screener/scan
GET  /journals/:id
GET/POST /ws, /ws/market
```

One handler file per concern (this crate's existing convention, to be
followed by the new work): `market.rs` (instruments/quotes/bars/depth),
`portfolio.rs` (+ `portfolio_store.rs`), `reports.rs` (ledger/tax/snapshots/
audit/concentration/wac/fees/equity-curve/order-fills — all already
implemented: `get_ledger`, `get_tax_report`, `get_portfolio_snapshots`,
`get_position_audit`, `get_concentration_risk`, `get_weighted_average_cost`,
`get_fees_report`, `get_equity_curve`, `get_order_fills`), `orders.rs`,
`trades.rs`, `screener.rs`, `runs.rs` (backtest/sweep journals),
`journal.rs` + `operator_journal.rs` (the two unrelated things above),
`v1.rs` (route registration), `openapi.rs` (OpenAPI schema generation).

**Scoping note**: `/reports/*` is already a rich reporting surface, but it
reports on the live OMS/portfolio account — it has no concept of trade tags,
rating, review, R-multiple, edge score, win rate, or profit factor. This
plan's new surface is additive (annotation + performance-metrics concepts
`/reports/*` doesn't have), not a re-implementation of anything `/reports/*`
already does. Where sensible, the new handlers should reuse
`/portfolio/trades` and `/reports/equity-curve`'s underlying data rather than
re-fetching it a different way — this is an implementation-time optimization
note, not a hard requirement, since the new routes ultimately call
`vyapaar-ledger-performance`, which itself depends on
`vyapaar-ledger-portfolio` and does not need to re-derive anything
`vyapaar-ledger-portfolio` already computes.

### 1.2 Gaps covered (upstream, cited not owned)

- `vyapaar-ledger/docs/plans/0002-trade-performance-journal.md` — provides
  the `vyapaar-ledger-performance` crate (trade annotations + performance
  metrics + edge score + daily P&L calendar + account comparison) that
  Phase 1 and Phase 2 of this plan wrap.
- `vyapaar-data/docs/gaps/0001-economic-calendar-ingestion.md` — provides the
  economic/earnings/dividends/splits/IPO calendar data that Phase 3's
  `/market-data/calendar` wraps.
- `vyapaar-data/docs/gaps/0002-company-fundamentals-symbol-profile.md` —
  provides the company-fundamentals data that Phase 3's
  `/market-data/symbol-profile/:id` wraps.
- `vyapaar-ui-web/docs/plans/0011-vela-journal-calendar-reports-domain-model.md`
  — the consumer; its own three-way-account finding (journal accounts vs.
  live OMS accounts vs. UI workspace state) matches §2.1 below.

---

## 2. Component Specifications & Phases

### 2.1 New endpoints

Response shapes below mirror vela.luxalgo.com's own live API contract
(verified by direct authenticated `fetch()` against it on 2026-11-09),
adapted to this repo's existing path conventions (e.g. `/portfolio/trades`
style) rather than copied as literal paths.

**Trade journal** (new file: `crates/vyapaar-runtime-api-rest/src/trade_journal.rs`):

- `GET /trade-journal/accounts` — list journal accounts. This is the new
  `vyapaar-ledger-performance` entity from the ledger plan, and is **not**
  the same as `/portfolio/accounts` (live OMS/trading accounts). State this
  distinction explicitly in the handler doc-comment — it is the same
  three-way-account finding `vyapaar-ui-web`'s plan 0011 documents.
- `POST /trade-journal/accounts` — create one (manual / imported /
  broker-synced provenance).
- `GET /trade-journal/overview?range=<range>&compare=<previous|none>` — the
  metrics + edge-score payload. Quote the response shape verbatim (it is
  vela.luxalgo.com's own live contract):
  ```json
  {
    "asOf": "...", "timeZone": "...", "range": {"from": "...", "to": "..."},
    "metrics": {
      "totalTrades": 0, "closedTrades": 0, "openTrades": 0, "wins": 0, "losses": 0, "breakevens": 0,
      "netPnl": 0, "grossPnl": 0, "fees": 0, "winRate": null, "dayWinRate": null, "tradingDays": 0,
      "profitFactor": null, "profitFactorIsInfinite": false, "avgWin": null, "avgLoss": null,
      "avgWinLossRatio": null, "expectancy": null, "largestWin": 0, "largestLoss": 0,
      "maxWinStreak": 0, "maxLossStreak": 0, "currentStreak": 0, "totalVolume": 0, "avgDurationMs": null,
      "maxDrawdown": 0, "maxDrawdownPct": null, "recoveryFactor": null, "profitConcentration": null,
      "avgRealizedR": null, "tradesWithRisk": 0
    },
    "edgeScore": {"version": 1, "score": null, "components": {"winRate": 0, "profitFactor": 0, "avgWinLoss": 0, "drawdown": 50, "recovery": 0, "consistency": 0}, "closedTrades": 0},
    "days": "...", "equity": "...", "openPositions": "...", "previous": "..."
  }
  ```
- `GET /trade-journal/trades?from=&to=&limit=&cursor=` — closed trades with
  annotation fields (tags/rating/review/R), cursor-paginated:
  `{"trades": [...], "nextCursor": null}`.
- `PATCH /trade-journal/trades/:id` — set a trade's tags/rating/review/
  planned_risk.
- `GET /trade-journal/calendar?month=YYYY-MM` — daily P&L heatmap:
  `{"calendar": {"year":, "month":, "weeks": [{"days": [{"date":, "netPnl":, "grossPnl":, "fees":, "trades":, "wins":, "losses":, "breakevens":, "volume":}]}]}}`.
- `GET /trade-journal/notes?limit=` / `POST /trade-journal/notes` — day-keyed
  journal notes.
- `GET /trade-journal/compare?accountA=&accountB=` or
  `?account=&mode=previous` — two-sided metrics comparison.

**Market reference data** (new functions in the existing
`crates/vyapaar-runtime-api-rest/src/market.rs` — these are market-reference
concerns, same file as the existing instrument/quote/bar/depth handlers, not
a new file):

- `GET /market-data/symbol-profile/:id` — company fundamentals (backed by
  `vyapaar-data`'s gap 0002):
  ```json
  {"symbol":, "name":, "description":, "ceo":, "employees":, "website":, "headquarters":, "industry":, "sector":, "ipoDate":, "exchange":, "country":, "image":, "marketCap":, "beta":, "lastDividend":, "range":, "averageVolume":, "isEtf":, "isActivelyTrading":}
  ```
  This repo already has `GET /instruments/:id`. Decision for this plan:
  symbol-profile is a **new standalone route** (not a sub-path of
  `/instruments/:id`), because fundamentals are a distinct, heavier/
  rarer-fetched payload than the existing lightweight instrument lookup —
  don't bloat the hot-path `/instruments/:id` response. (An
  `/instruments/:id/profile` sub-path was considered and rejected for this
  reason; revisit only if a future caller needs fundamentals and the
  instrument record in one round trip.)
- `GET /market-data/calendar?from=&to=&category=` — economic/earnings/
  dividends/splits/IPO events (backed by `vyapaar-data`'s gap 0001),
  category-filterable per vela.luxalgo.com's own observed filter pills
  (`Economic` / `Earnings` / `Dividends` / `Splits` / `IPO`).

### 2.2 Explicitly not proposed

- A `/workspaces` endpoint for saving UI tab-layout state. `vyapaar-ui-web`'s
  own plan 0011 reasoned this is a UI-session-state concern best owned by
  that repo's own Next.js backend-for-frontend, not this Rust
  domain-focused REST crate. Not second-guessed here — noted only so a
  reader doesn't wonder why it's missing from this plan.

### 2.3 Phases

**Phase 1** — core read/annotate surface:
`/trade-journal/accounts` (GET, POST), `/trade-journal/overview`,
`/trade-journal/trades` (GET + PATCH). New `trade_journal.rs` handler file
calling into the new `vyapaar-ledger-performance` crate. **Depends on**
`vyapaar-ledger/docs/plans/0002-trade-performance-journal.md` landing first.

**Phase 2** — calendar, notes, comparison:
`/trade-journal/calendar`, `/trade-journal/notes` (GET + POST),
`/trade-journal/compare`. Same `trade_journal.rs` file. Depends on the same
upstream ledger plan as Phase 1.

**Phase 3** — market reference data:
`/market-data/symbol-profile/:id`, `/market-data/calendar`. New functions in
the existing `market.rs` (not a new file). **Depends on**
`vyapaar-data/docs/gaps/0001-economic-calendar-ingestion.md` and
`vyapaar-data/docs/gaps/0002-company-fundamentals-symbol-profile.md` landing
first.

Every phase above also requires a matching OpenAPI entry in
`crates/vyapaar-runtime-api-rest/src/openapi.rs` as an implementation-phase
task (not drafted in this plan — the OpenAPI JSON itself is generated code,
not design).

### 2.4 Blocking note

Every phase in this plan is blocked on its cited upstream repo's own plan or
gap landing first. This document specifies a REST-wrapper layer only — it is
not the source of truth for any of the underlying domain logic
(`vyapaar-ledger-performance`) or data (`vyapaar-data`'s calendar and
fundamentals ingestion), and implementation should not begin on a phase
until its upstream dependency is itself implemented.
