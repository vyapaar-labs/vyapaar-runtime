# 0004 — Block/bulk deal chat tool and REST endpoint

**Status**: Proposed

---

## 0. Executive Summary & Objective

`vyapaar-ui-web`'s Research/Today tab ships a suggested chat question, "Any notable block deals
today?" (`vyapaar-ui-web/src/components/organisms/ResearchLanding.tsx` line 17), routed through
`/api/chat` → this repo's `GET /ws/chat` → the `vyapaar-agent-chat` conversational engine. Today
that question always produces an ungrounded, generic LLM disclaimer, because no tool in the chat
agent's catalog can answer it and no data source anywhere in the workspace has block/bulk deal
disclosures. This plan specifies the full vertical slice to close that: a new port method, chat
tool, runtime wiring, and (optionally) a direct REST endpoint — once the upstream data source
lands.

This plan's own actionable work is blocked on one upstream dependency:

- `vyapaar-data/docs/gaps/0005-block-bulk-deal-disclosure-ingestion.md` — no block/bulk deal data
  source exists anywhere in the workspace yet; the ingestion-source decision there is unmade.

Phases 2–3 below (the port/tool/wiring layer) can be implemented ahead of that dependency landing
— they return `PortError::unavailable` honestly, the same pattern this repo's own
`docs/gaps/0007-chat-market-reader-approximations-and-missing-sources.md` already uses for
`MarketReader::indicators`/`macro_series` — so the chat agent never fabricates deal data it
doesn't have. Only Phase 4 (wiring the real read) is blocked on gap 0005 closing.

## 1. Context & Gaps Covered

### 1.1 Verified current state

- The chat agent's market tool catalog lives in `vyapaar-agent`'s
  `crates/vyapaar-agent-chat-tools/src/market.rs`: five tools today —
  `MarketQuoteTool`, `MarketHistoryTool`, `MarketIndicatorsTool`, `MarketSearchTool`,
  `MacroSeriesTool` — none cover block/bulk deals.
- Each tool is backed by the `MarketReader` port trait
  (`crates/vyapaar-agent-chat-tools/src/ports/market.rs`), implemented in this repo by
  `FeedMarketReader` (`crates/vyapaar-runtime-api-rest/src/chat_market.rs`), which reads through
  to `AppState`'s existing `InstrumentMaster`/`BarReader`/`QuoteReader`/`RiskEngine` ports.
- Tools are registered in `vyapaar-agent`'s
  `crates/vyapaar-agent-chat-tools/src/catalog.rs::create_read_only_tools` /
  `register_read_only_tools` (currently documented as "14 Phase B read-only conversational
  tools").
- No repo in this workspace (`vyapaar-market`, `vyapaar-data`, `vyapaar-adapters`) has a block or
  bulk deal data source, reader, or port today — confirmed by search; see
  `vyapaar-data/docs/gaps/0005-block-bulk-deal-disclosure-ingestion.md`.

### 1.2 Gaps covered

- `vyapaar-data/docs/gaps/0005-block-bulk-deal-disclosure-ingestion.md` (upstream data source;
  this plan's Phase 4 depends on it).

## 2. Component Specifications & Phases

### Phase 1 — `MarketReader` port extension (`vyapaar-agent`)

In `vyapaar-agent/crates/vyapaar-agent-chat-tools/src/ports/market.rs`, add alongside the
existing `MarketQuote`/`MarketHistory`/etc. DTOs:

```rust
/// A single disclosed block or bulk deal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BlockDeal {
    pub instrument: InstrumentId,
    pub exchange: String,
    pub client_name: String,
    pub side: String,       // "buy" | "sell"
    pub category: String,   // "block" | "bulk"
    pub quantity: Quantity,
    pub price: Price,
    pub trade_date: DateTime<Utc>,
}
```

Add one method to the `MarketReader` trait:

```rust
/// Fetch disclosed block/bulk deals for a date (defaults to latest trading day) or instrument.
async fn block_deals(
    &self,
    instrument: Option<&InstrumentId>,
    date: Option<DateTime<Utc>>,
) -> Result<Vec<BlockDeal>, PortError>;
```

### Phase 2 — `BlockDealsTool` chat tool (`vyapaar-agent`)

In `vyapaar-agent/crates/vyapaar-agent-chat-tools/src/market.rs`, add `BlockDealsTool` following
the exact shape of `MarketQuoteTool` (same file, lines ~61–126): a `BlockDealsInput` struct
(`instrument: Option<String>`, `date: Option<String>`, parsed via the file's existing
`parse_instrument`/`parse_timestamp` helpers), a `ToolSpec` named `"block_deals"` with a
description like "Fetch disclosed block and bulk deals for an instrument or trading day.", and an
`execute()` that calls `self.reader.block_deals(..)` and maps each `BlockDeal` into the
`ToolOutput::ok` JSON shape (mirroring `MarketQuoteTool::execute`'s `serde_json::json!` call).

Register it in `vyapaar-agent/crates/vyapaar-agent-chat-tools/src/catalog.rs`:
add `BlockDealsTool` to the `use crate::market::{...}` import list, add
`Arc::new(BlockDealsTool::new(Arc::clone(&market)))` to `create_read_only_tools`'s market-tools
block (bullet 1, currently 5 tools), and update the "14 Phase B read-only conversational tools"
doc comment on both `create_read_only_tools` and `register_read_only_tools` to 15.

### Phase 3 — `FeedMarketReader` wiring, stubbed (`vyapaar-runtime`)

In `vyapaar-runtime/crates/vyapaar-runtime-api-rest/src/chat_market.rs`, implement
`FeedMarketReader::block_deals` returning
`Err(PortError::unavailable("block/bulk deal disclosures are not yet ingested — see vyapaar-data/docs/gaps/0005-block-bulk-deal-disclosure-ingestion.md"))`,
the same honest-stub pattern `indicators`/`macro_series` already use there (per
`docs/gaps/0007-chat-market-reader-approximations-and-missing-sources.md` item 3). This closes
Phases 1–3 end to end: the chat agent gains a real tool that correctly reports "I don't have this
data yet" instead of the model's own ungrounded disclaimer — a materially better failure mode,
and shippable independent of Phase 4.

### Phase 4 — Real data wiring (blocked on `vyapaar-data` gap 0005)

Once `vyapaar-data/docs/gaps/0005-block-bulk-deal-disclosure-ingestion.md` lands a
`BlockDealsRepository`/reader and Parquet catalog table, replace Phase 3's stub in
`FeedMarketReader::block_deals` with a real read, following whatever read path this repo ends up
using for `vyapaar-data`-sourced tables (no such path exists yet for any `vyapaar-data` table as
of this plan — `macro_series`/`indicators` are *also* still stubbed per gap 0007 item 3, so this
phase may need to establish that read path for the first time, not just reuse one).

### Phase 5 (stretch, optional) — Direct REST endpoint

Add `GET /market-data/block-deals` to `vyapaar-runtime-api-rest` (per this workspace's
`AGENTS.md` §6.3 canonical-endpoints pattern: `instrument` and `date` query params, same
`BlockDeal` shape as Phase 1), for consumers that want this data outside the chat flow (e.g. a
future dedicated panel in `vyapaar-ui-web`, mirroring how `vyapaar-data`'s calendar gap feeds both
a chat path and `vyapaar-runtime/docs/plans/0003`'s REST path). Only worth doing once Phase 4
lands real data — a REST endpoint that only ever returns `unavailable` has no consumer value.

## 3. Sequencing

Phases 1–3 are independent of the data-source decision and can be implemented and merged now.
Phase 4 is blocked until `vyapaar-data` gap 0005 is closed. Phase 5 is optional and should not be
started before Phase 4.
