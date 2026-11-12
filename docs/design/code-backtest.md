# `vyapaar-runtime-backtest` — Event-Driven Backtester Reference

Crate `crates/vyapaar-runtime-backtest/` — lib name `vyapaar_runtime_backtest`.
Crate doc: *"Research-live parity event-driven backtesting engine"*.

## 1. Purpose

- Drive the **real** sync `vyapaar_runtime_strategy::Strategy` through
  `vyapaar_runtime_strategy::StrategyRunner` — the identical runner live/paper use — so a
  backtest gets the same pre-trade risk gating, warm-up-bar suppression, and intent/order
  rejection bookkeeping instead of a reimplemented subset.
- Enforce the canonical event chain `MarketEvent → SignalEvent → OrderEvent → FillEvent`
  deterministically (no wall clock; every timestamp comes from the event).
- Simulate Indian microstructure: statutory taxes/brokerage via
  `vyapaar_ledger_journal::TaxCalculator`, double-entry journaling via `post_transaction`,
  FIFO position/PnL via `vyapaar_ledger_portfolio::AccountState`, ICCL T+1 Demat settlement &
  margin holdbacks via `ClearingCalendar`/`DematHoldings`/`SettlementEngine`, optional L2 depth
  simulation with latency, slippage, participation caps, adverse fills, and SEBI MIS 15:20
  square-off.

Parity is achieved **by delegation, not reimplementation**: costs, journaling, positions/PnL,
and settlement use the same canonical `vyapaar-ledger-*` implementations the live actor path
uses, so backtest and live agree byte-for-byte on cost and settlement math.

## 2. Module Map

| File | Contents |
|---|---|
| `lib.rs` | Module declarations + re-export surface (incl. ledger types) |
| `engine.rs` | `BacktestEngine<S>`, `BacktestConfig`, `BacktestResult`, `PendingOrder`, event loop, `execute_order`, MIS square-off, settlement flush, double-entry posting, helpers (`decimal_qty_to_u64`, `to_kokoro_product`, `apply_fill_with_add_support`) |
| `catalog.rs` | `CatalogLoader` (Arrow/Parquet), `CatalogError`, `CatalogInstrumentSummary`, `CatalogVerificationReport`, `decimal_from_f64_lossless` |
| `calendar.rs` | `SessionWindow`, `NseTradingCalendar` (+ `TradingCalendar` impl) |
| `events.rs` | Crate-local `MarketEvent`, `SignalEvent`, `OrderEvent`, `FillEvent` |
| `sim/mod.rs` | `pub mod l2_book; latency; mis; slippage;` |
| `sim/l2_book.rs` | `OrderBook`, `OrderBookSide`, `OrderBookLevel` |
| `sim/latency.rs` | `SeededRng` (xorshift), `NetworkLatency`, `MisSquareOffLatency` |
| `sim/mis.rs` | `SquareOffResult`, `MisSquareOffManager` |
| `sim/slippage.rs` | `SlippageModel`, `AdverseFillModel` |

## 3. `BacktestConfig`

```rust
pub struct BacktestConfig {
    pub starting_cash: Decimal,                    // INR; default ₹1,000,000.00
    pub product_type: ProductType,                 // default Delivery
    pub l2_simulation: bool,                       // default false
    pub latency: Option<NetworkLatency>,
    pub slippage: Option<SlippageModel>,
    pub adverse_fill: Option<AdverseFillModel>,
    pub mis_square_off: Option<MisSquareOffManager>,
    pub warmup_bars: u32,                          // default 0
    pub iccl_margin_holdback_pct: Decimal,         // default dec!(0.20) (SEBI VaR+ELM)
}
```

`with_l2_simulation(starting_cash, product_type, latency, slippage, mis_square_off)` sets
`l2_simulation: true`. `with_adverse_fill(model)` chains an adverse-fill model.

## 4. `BacktestEngine<S>`

```rust
BacktestEngine::new(strategy, risk_engine, config, calendar: Arc<dyn TradingCalendar>)
    .with_position_sizer(sizer, method, risk_pct /* default 0.01 */)
```

Construction wraps the strategy in
`StrategyRunner::with_context(LedgerContext::with_cash(Money::inr(starting_cash)))
.with_risk(risk_engine).with_warmup_bars(config.warmup_bars)`, builds
`ClearingCalendar::with_holidays(&calendar.holidays())` (injecting the real NSE holiday data so
`vyapaar_ledger_portfolio` never hardcodes its own copy), a `SettlementEngine`,
`AccountState`, `DematHoldings`, and (when L2 is on) an `OrderBook`.

Run entry points:
- `run_catalog(&loader, instruments, timeframe, from, to)` — `loader.load_market_events(...)` →
  `run_events`.
- `run_events(events)` — the deterministic loop below.

Accessors: `mis_square_off()`, `demat_holdings()`, `margin_blocked()`, `settled_cash()`,
`available_trading_cash()` (= `(settled_cash − margin_blocked).max(0)`),
`ledger_transactions()`.

### 4.1 `BacktestResult`

```rust
pub struct BacktestResult {
    pub initial_cash: Decimal,
    pub final_equity: Decimal,           // settled cash + MTM of live positions
    pub net_pnl: Decimal,                // final_equity − initial_cash
    pub gross_pnl: Decimal,              // FIFO realized PnL summed over fills
    pub total_fees: TaxBreakdown,        // vyapaar_ledger_journal::TaxBreakdown
    pub total_trades: usize,             // fills booked
    pub total_signals: usize,            // submitted + order/intent rejections + suppressed
    pub refused_orders: usize,           // runner order rejections
    pub max_drawdown_pct: Decimal,       // (max_drawdown / peak × 100).round_dp(2)
    pub price_impact: Decimal,           // cumulative L2/slippage impact, INR
    pub fills: Vec<FillReport>,          // execution order
    pub equity_curve: Vec<(DateTime<Utc>, Decimal)>,   // sampled after every event
    pub ledger_transactions: Vec<Transaction>,
}
```

## 5. The Event Loop (`run_events` → `process_market_event`)

`run_events`: `runner.on_start()` (errors logged, not fatal) → for each `MarketEvent`
chronologically `process_market_event` → `runner.on_stop()` → final settlement flush for the
next trading day → assemble `BacktestResult`.

`process_market_event` — **exact ordering**:

1. **Date advance** (T+1 settlement + daily reset): new `current_date` ⇒ clear
   `scrips_sold_today` (STT first-sell flag) and `flush_settlements(event_date)`.
2. **Price cache**: `Tick` → `last_price()` (Quote → mid when both sides > 0); `Bar` →
   `bar_opens[instrument] = open` (E2 gap-through cache) and `latest_prices[instrument] = close`.
3. **Mature pending orders**: every `PendingOrder { ready_ts, order }` with
   `ready_ts <= ts` → `execute_order(order, ts)` (uses the price just cached).
4. **L2 book update**: `Tick::Ltp` → `add_order(price, volume)`; `Tick::Quote` →
   `add_bid`/`add_ask`; `Tick::Depth` → `set_depth`; `Bar` → `set_avg_daily_volume(volume)` +
   `add_order(close, volume)` (synthetic depth; a bar's traded volume is the ADV proxy).
5. **Strategy dispatch**: `runner.on_event(&event.to_core_event(), ts)` — sets the context
   clock, calls the typed hook, validates / warm-up-gates / risk-checks the submitted intents.
6. **Drain + sizing + latency**: `runner.drain_orders()` → **E3 partition**: stop orders
   (`StopLoss`/`StopLimit`) first, then others, relative order preserved — a stop and a target
   exit dispatched in the same batch execute stop-first. Per order: optional position re-size,
   then `ready_ts = order_book.ready_ts(ts)` (submit_ts + latency) — future ⇒ queue as
   `PendingOrder`, else `execute_order` immediately.
7. **MIS square-off check** (`check_mis_square_off(ts)`).
8. **Drawdown / equity curve** (`update_drawdown(ts)`).

## 6. Fill Simulation (`execute_order`)

- Look up `latest_prices[instrument]`; missing ⇒ no fill (no market price to match against).
- **L2 path** (book present): `execute_limit_order` (fills only at asks ≤ limit / bids ≥ limit —
  price improvement emerges from the walk) or `execute_market_order` (walks best levels, VWAP
  `round_dp(4)`, tracks the worst price); quantities are clamped to the participation cap
  (`ADV × cap / 2` per side; capped excess returns as `remaining`, i.e. a partial fill, never a
  rejection). Impact = `|matched_price − current_price| × filled` + spread-crossing penalty +
  participation slippage + adverse-fill slippage (`final_price` becomes the adverse worst
  price).
- **Direct path** (no book) — GAP-0002 E2/E3 stop modeling:
  - `StopLoss(trigger)`: crossed = Sell `current ≤ trigger || bar_open ≤ trigger` (Buy
    mirrored). Not crossed ⇒ fills at `current_price` (an untriggered stop behaves like a
    market fill). Crossed ⇒ a bar that *opened* through the trigger fills at the **open**
    (gap-through), else at the **trigger** (never better).
  - `StopLimit { trigger, limit }`: same crossed check; realized = gap open else current;
    **unmarketable ⇒ the order is dropped entirely**; marketable ⇒ fills at `limit`.
- Direct-path slippage (no L2 but a model configured): `compute_slippage` over a 1,000-level
  synthetic depth; Buy pays `price + adj`, Sell receives `(price − adj).max(0.01)`.
- **Statutory taxes**: `TaxCalculator::compute_charges(side, product_type, exchange, price,
  qty, is_first_sell)` — the STT first-sell flag is per scrip per day; `total_fees.add_assign`.
- **Deterministic fill id**: `"{client_order_id}-{fill_seq}"` (no random v4).
- **Double-entry ledger**: balanced `Transaction` posted via `post_transaction` — Buy debits
  `Assets:Holdings:EquityDelivery` + `Expenses:Brokerage` + `Expenses:Taxes:{STT, StampDuty,
  GST}` + `Expenses:ExchangeFees` (exchange + SEBI + DP), credits `Assets:Cash:BrokerAccount`;
  Sell debits cash + expenses, credits holdings + `Liabilities:MarginPayable` (fees).
  `FeeBreakdown` maps `exchange_charges = exchange_turnover + dp_charge` (DP has no dedicated
  slot) so `fee_breakdown.total() == fees.total()` exactly.
- **Research-live parity feedback**: `runner.on_fill(&fill_report)` books into the strategy's
  `LedgerContext`, records the fill in the risk engine, and calls `Strategy::on_fill`.
- **T+0 position/FIFO**: `apply_fill_with_add_support` (handles same-direction pyramiding with
  a quantity-weighted entry blend; everything else delegates to `apply_fill_to_account`) →
  `gross_pnl += realized`.
- **ICCL T+1 settlement**: Buy blocks `notional + fees.total()` as margin until T+1 and marks
  the shares as unsettled receivables; Sell blocks `notional × iccl_margin_holdback_pct` (20%
  VaR+ELM) and only queues the Demat instruction when free settled shares cover the quantity.
  `pending_settlements` records `(settle_date, cash_flow − fees)` — notional and fees settle
  together on the same `settle_date`.

`flush_settlements(as_of)`: settles due cash into `settled_cash`, releases due margin
holdbacks, and moves blocked shares into free Demat holdings via `process_settlements`.

## 7. MIS Square-Off

`check_mis_square_off` is a no-op unless `product_type == ProductType::Intraday` and a
`MisSquareOffManager` is attached. For every open position inside the square-off window
(session close − configurable offset, default 10 min ⇒ 15:20 IST for NSE's 15:30 close), a
forced closing `OrderRequest::new(..., OrderType::Market, ..., "mis_square_off")` is executed —
**deliberately bypassing the strategy's risk gate** (the broker forces the close, not the
strategy).

## 8. Catalog (`CatalogLoader`)

- Layout 1: `catalog/{TF}/{EXCHANGE}/{SYMBOL}/*.parquet` (case-insensitive timeframes);
  layout 2 (Lakehouse v2 Hive): `{root}/bars/timeframe={tf}/exchange={EXCHANGE}/year=*/month=*/`
  with `"{SYMBOL}.parquet"` or `"{SYMBOL}.part-*"` files.
- `load_bars` dedups overlapping partition boundaries (`timestamp <= last_ts` skipped);
  `load_market_events` sorts by `(timestamp, exchange, symbol)` — the deterministic interleaving.
- `inspect_summary` (bar counts, first/last timestamps/closes) and `verify_integrity` (empty
  partitions, retrograde timestamps, OHLC bounds, negative volume → `CatalogVerificationReport`).
- Implements `vyapaar_core::BarReader` (`M1→"1M"` … `W1→"1W"` timeframe mapping).
- Column decoding: timestamp `"ts"`|`"timestamp"` as `Int64` (ns) or `Timestamp(...)`; prices as
  `Decimal128` (native, no float) or `Float64` via `decimal_from_f64_lossless` (4-dp, normalized,
  kills binary-float artifacts); volume clamped ≥ 0.
- `CatalogError`: `Io`, `Parquet`, `Arrow`, `MissingColumn`, `UnsupportedColumnType`,
  `InvalidRow`, `NotFound` (+ `From<CatalogError> for PortError`).

## 9. Calendar (`NseTradingCalendar`)

- `SessionWindow::regular()` = 09:15–15:30 IST; `muhurat()` = 18:15–19:15 IST.
- Seeds declared NSE holidays for 2025 (15 dates) and 2026 (9 dates) plus Muhurat sessions
  (2025-10-21, 2026-11-08); `add_holiday`/`add_muhurat_session` for extensions.
- `is_trading_day` (Muhurat always trading; weekends never), `session_for_date`,
  `next_trading_day` (strictly after), `is_market_open(utc)` (converts to IST), and
  `holidays()` (Muhurat dates **excluded** — they are trading days).
- Implements the object-safe `vyapaar_core::TradingCalendar` port, so the engine and enhancer
  stay decoupled from the concrete NSE venue.

## 10. `sim/` Modules

| Type | Semantics |
|---|---|
| `OrderBook` | BTreeMap ladders (signed levels: +bids / −asks); `execute_market_order` / `execute_limit_order` walk best levels; participation cap `ADV × cap/2` per side; `ready_ts = submit_ts + latency_us` (pure arithmetic on the caller's timestamp). |
| `NetworkLatency` | Seeded xorshift triangular approximation of `mean ± std_dev/2` (min 1 µs); `advance_step` trends toward `mean + step`. |
| `MisSquareOffManager` | Window `[close − offset, close)` from the calendar's session; `ForcedSquareOff` / `NoAction` / `AlreadyClosed` results; per-instrument eligibility via `process_trade`. |
| `SlippageModel` | Defaults: `daily_range_pct 0.02`, `volatility_factor 1`, `avg_spread_pct 0.001`; spread-crossing penalty = half-spread (or `reference × avg_spread/2`); participation slippage clamped ±5%; level-walking variant. |
| `AdverseFillModel` | Walks contra levels up to `levels_per_round`; per-level `%` impact, compounded multiplicatively when `compounding`; returns `(total_slippage.min(max), worst_price)`. |

## 11. Dependencies

`vyapaar-core` (types, `BarReader`, `TradingCalendar`, sizing ports), `vyapaar-risk`
(`RiskEngine`), `vyapaar-ledger-portfolio` (`AccountState`, `ClearingCalendar`, `DematHoldings`,
`SettlementEngine`), `vyapaar-ledger-journal` (`ProductType`, `TaxBreakdown`, `TaxCalculator`,
`Transaction`, `post_transaction`), `vyapaar-runtime-strategy` (`Strategy`, `StrategyRunner`,
`LedgerContext`), `arrow`/`parquet` (catalog), `rust_decimal`, `chrono`, `serde`, `uuid`,
`tracing`, `tokio`. Dev: `tempfile` (synthetic catalogs), `vyapaar-agent` (deterministic
LLM-prompt replay test).

## 12. Tests (`tests/`, 10 files, 41 tests)

| File | Coverage |
|---|---|
| `unit_tests.rs` | NSE calendar weekday/holiday behaviour |
| `calendar_tests.rs` | Regular weekdays, weekends/holidays, Muhurat session |
| `catalog_tests.rs` | Inspect/verify over tempfile partitions (valid + anomalous OHLC) |
| `backtest_tests.rs` | Historical day backtest with Indian cost + settlement; deterministic LLM-prompt replay; fees match the ledger `TaxCalculator`; `decimal_qty_to_u64` truncation/saturation; T+1 uses the clearing calendar |
| `engine_strategy_runner_tests.rs` | Engine drives runner intents; warm-up bars respected; intent rejections surfaced; fills exposed |
| `l2_latency_tests.rs` | Latency delays the fill to a later event timestamp; zero latency fills immediately |
| `l2_participation_cap_tests.rs` | Market orders clamped to the participation cap; within-cap fills fully |
| `mis_square_off_tests.rs` | All open MIS positions closed at 15:20 IST; eligibility tracking; no square-off for delivery |
| `stop_fill_tests.rs` | Gap-below sell stop fills at bar open; no gap fills at trigger; gap-above buy stop; untriggered stop keeps market fill; same-bar stop-before-target (E3); stop-limit unmarketable then marketable |
| `catalog_and_settlement_tests.rs` | L2 depth-walking VWAP + limit matching; spread-crossing + adverse-fill slippage; ICCL T+1 settlement, margin holdbacks, double-entry ledger; intraday statutory rates; synthetic partitions + `BarReader`; Lakehouse v2 `Decimal128` schema; real `vyapaar-data` catalog ingestion when present |

## 13. Invariants & Notables

- **Determinism chain**: events sorted by `(timestamp, exchange, symbol)`; explicit `ts_init`
  everywhere; `ready_ts` pure arithmetic; seeded latency; identical event sequences reproduce
  byte-identical fills, journals, and equity curves.
- **Errors are logged, not fatal**: `on_start`/`on_stop`/`on_event`/`on_fill`/`process_trade`
  failures are `tracing::error!`-ed and the loop continues; the only hard failure is a
  `CatalogError` from `run_catalog`.
- **L2 wiring quirk**: `with_l2_simulation` passes `slippage.daily_range_pct()` as the
  `OrderBook` participation cap (default 2%), and the cap only activates once a bar has been
  seen (ADV 0 ⇒ uncapped).
- **Latency applies only through the L2 book**; with no book, orders execute on the same event.
  `MisSquareOffLatency` (15:20 countdown model) exists but the engine uses
  `MisSquareOffManager` + the trading calendar instead.
- **Product mapping**: `Futures`/`Options` tax products settle as `Product::Intraday` (never
  demat); non-Indian country codes resolve to `Exchange::Nse` for cash-segment tax math.
- **Fill bookkeeping details**: `FillReport.order_id` is `order.tag.unwrap_or_default()` (empty
  for MIS forced closes); `account_id = Some("default")`.

## Related

- `code-strategy.md` — the runner this engine drives
- `code-enhancer.md` — optimizer evaluates candidates through this engine
- `vyapaar-ledger-portfolio` / `vyapaar-ledger-journal` — canonical accounting the engine delegates to
- `vyapaar-risk` — the pre-trade gate inside the runner
