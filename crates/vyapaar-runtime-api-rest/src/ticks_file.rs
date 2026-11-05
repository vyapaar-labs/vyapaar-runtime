//! Versioned NDJSON tick parser/importer (ADR 0030) and Parquet bar summary helpers.
//!
//! Line 1 of a ticks file is the header `{"honba_ticks":1}`. Every later non-blank line is one
//! tick:
//!
//! ```text
//! {"kind":"quote","symbol":"TCS","exchange":"NSE","ts_event":1704067200000000000,"bid_price":99.9,"ask_price":100.1,"bid_size":500,"ask_size":400}
//! {"kind":"trade","symbol":"TCS","exchange":"NSE","ts_event":1704067200000000000,"price":100.0,"size":50,"aggressor":"buyer","trade_id":"T-1"}
//! ```
//!
//! `ts_event` is unix nanoseconds as a JSON integer; `ts_init` is optional and defaults to
//! `ts_event`; `aggressor` is `buyer`, `seller` or `no_aggressor`. Unknown fields are refused.
//! Ticks may come in any order (the reader sorts, stable for ties). Every error names its line.

use std::path::Path;

use anyhow::{Context, Result, bail};
use rust_decimal::prelude::ToPrimitive;
use serde::Deserialize;
use vyapaar_agent_analytics::RoundTrip;
use vyapaar_core::{Exchange, FillReport, InstrumentId};
use vyapaar_market_finance::india::exchange::resolve_instrument;
use vyapaar_runtime_feed::wire::{AggressorSide, QuoteTick, TradeTick, UnixNanos};
use vyapaar_runtime_feed::{
    InMemoryTickReader, ParquetBarSource, ParquetTickStore, TICK_DIR, write_tick_file,
};

/// The only NDJSON ticks format version supported by this build (ADR 0030).
pub const VERSION: u64 = 1;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Header {
    honba_ticks: u64,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Line {
    Quote {
        symbol: String,
        exchange: String,
        ts_event: u64,
        ts_init: Option<u64>,
        bid_price: f64,
        ask_price: f64,
        bid_size: f64,
        ask_size: f64,
    },
    Trade {
        symbol: String,
        exchange: String,
        ts_event: u64,
        ts_init: Option<u64>,
        price: f64,
        size: f64,
        aggressor: AggressorSide,
        trade_id: String,
    },
}

fn instrument(symbol: String, exchange: String) -> Result<serde_json::Value> {
    let exchange: Exchange = exchange
        .parse()
        .map_err(|_| anyhow::anyhow!("unknown exchange {exchange:?}; expected NSE, BSE or MCX"))?;
    Ok(serde_json::to_value(InstrumentId::new(exchange, symbol))
        .expect("an instrument id serializes"))
}

fn nanos(n: u64) -> serde_json::Value {
    serde_json::to_value(UnixNanos::from_u64(n)).expect("a timestamp serializes")
}

/// Parses the text of an ADR 0030 NDJSON ticks file into an [`InMemoryTickReader`].
///
/// Errors carry the 1-based line number where parsing or invariant validation failed.
pub fn parse_ticks_ndjson(text: &str) -> Result<InMemoryTickReader> {
    let mut lines = text
        .lines()
        .enumerate()
        .map(|(i, l)| (i + 1, l))
        .filter(|(_, l)| !l.trim().is_empty());
    let Some((n, head)) = lines.next() else {
        bail!("ticks file is empty; expected the header {{\"honba_ticks\":{VERSION}}}");
    };
    let header: Header = serde_json::from_str(head)
        .with_context(|| format!("line {n}: expected the header {{\"honba_ticks\":{VERSION}}}"))?;
    if header.honba_ticks != VERSION {
        bail!(
            "line {n}: unsupported ticks file version {}; this build reads version {VERSION}",
            header.honba_ticks
        );
    }
    let (mut quotes, mut trades) = (Vec::new(), Vec::new());
    for (n, raw) in lines {
        let line: Line = serde_json::from_str(raw).with_context(|| format!("line {n}"))?;
        match line {
            Line::Quote {
                symbol,
                exchange,
                ts_event,
                ts_init,
                bid_price,
                ask_price,
                bid_size,
                ask_size,
            } => {
                // `vyapaar_runtime_feed::wire::QuoteTick`'s custom `Deserialize` refuses crossed quotes
                // (bid > ask), while non-negative finite sizes are enforced here.
                if !bid_size.is_finite()
                    || bid_size < 0.0
                    || !ask_size.is_finite()
                    || ask_size < 0.0
                {
                    bail!("line {n}: invalid quote: sizes must be finite and >= 0");
                }
                let tick: QuoteTick = serde_json::from_value(serde_json::json!({
                    "instrument_id": instrument(symbol, exchange).with_context(|| format!("line {n}"))?,
                    "bid_price": bid_price,
                    "ask_price": ask_price,
                    "bid_size": bid_size,
                    "ask_size": ask_size,
                    "ts_event": nanos(ts_event),
                    "ts_init": nanos(ts_init.unwrap_or(ts_event)),
                }))
                .with_context(|| format!("line {n}: invalid quote"))?;
                quotes.push(tick);
            }
            Line::Trade {
                symbol,
                exchange,
                ts_event,
                ts_init,
                price,
                size,
                aggressor,
                trade_id,
            } => {
                if !size.is_finite() || size < 0.0 {
                    bail!("line {n}: invalid trade: size must be finite and >= 0");
                }
                let tick: TradeTick = serde_json::from_value(serde_json::json!({
                    "instrument_id": instrument(symbol, exchange).with_context(|| format!("line {n}"))?,
                    "price": price,
                    "size": size,
                    "aggressor_side": aggressor,
                    "trade_id": trade_id,
                    "ts_event": nanos(ts_event),
                    "ts_init": nanos(ts_init.unwrap_or(ts_event)),
                }))
                .with_context(|| format!("line {n}: invalid trade"))?;
                trades.push(tick);
            }
        }
    }
    if quotes.is_empty() && trades.is_empty() {
        bail!("ticks file holds a header but no ticks");
    }
    Ok(InMemoryTickReader::new()
        .with_quotes(quotes)
        .with_trades(trades))
}

/// Reads and validates a `--ticks` NDJSON file from disk. Errors carry both the path and line.
pub fn load_ticks_ndjson(path: &Path) -> Result<InMemoryTickReader> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading --ticks {}", path.display()))?;
    parse_ticks_ndjson(&text).with_context(|| format!("invalid --ticks {}", path.display()))
}

/// Imports an ADR 0030 NDJSON ticks file into `{out}/ticks/ticks.parquet` and reads it back via
/// [`ParquetTickStore::open`] before returning `(quote_count, trade_count)`.
pub fn import_ticks_ndjson(ticks: &Path, out: &Path) -> Result<(usize, usize)> {
    let text =
        std::fs::read_to_string(ticks).with_context(|| format!("reading {}", ticks.display()))?;
    let reader = parse_ticks_ndjson(&text)
        .with_context(|| format!("invalid ticks file {}", ticks.display()))?;
    let path = out.join(TICK_DIR).join("ticks.parquet");
    write_tick_file(&path, reader.quotes(), reader.trades())
        .with_context(|| format!("writing {}", path.display()))?;
    let store = ParquetTickStore::open(out)
        .with_context(|| format!("reading back the tick store {}", out.display()))?;
    Ok(store.len())
}

/// Summary of a single Parquet bar file loaded via [`load_parquet_bars_summary`].
#[derive(Debug, Clone, PartialEq)]
pub struct ParquetBarsSummary {
    /// Canonical `EXCHANGE:SYMBOL` string.
    pub qualified_instrument: String,
    /// Number of bars loaded.
    pub bar_count: usize,
    /// First bar `(ts_event, close)` when non-empty.
    pub first: Option<(UnixNanos, f64)>,
    /// Last bar `(ts_event, close)` when non-empty.
    pub last: Option<(UnixNanos, f64)>,
}

/// Resolves `symbol` / `exchange`, verifies `.parquet` extension, and loads a summary of bars.
pub fn load_parquet_bars_summary(
    source: &str,
    symbol: &str,
    exchange: Option<&str>,
) -> Result<ParquetBarsSummary> {
    let instrument = resolve_instrument(symbol, exchange)?;
    let qualified_instrument = format!("{}:{}", instrument.exchange, instrument.symbol);

    let bars = if source.to_ascii_lowercase().ends_with(".parquet") {
        ParquetBarSource::new(source, instrument)
            .bars()
            .with_context(|| format!("reading {source}"))?
    } else {
        bail!("unsupported source extension: {source} (expected .parquet)");
    };

    let first = bars.first().map(|b| (b.ts_event(), b.close()));
    let last = bars.last().map(|b| (b.ts_event(), b.close()));
    Ok(ParquetBarsSummary {
        qualified_instrument,
        bar_count: bars.len(),
        first,
        last,
    })
}

/// Pairs consecutive opposite-side [`FillReport`]s into [`RoundTrip`] records.
pub fn pair_fills(fills: &[FillReport]) -> Vec<RoundTrip> {
    let mut trips = Vec::new();
    let mut i = 0;
    while i + 1 < fills.len() {
        if let Ok(rt) = RoundTrip::from_fills(&fills[i], &fills[i + 1]) {
            trips.push(rt);
        }
        i += 2;
    }
    trips
}

/// Builds a cumulative equity curve starting from `starting` across `trips`.
pub fn equity_curve(starting: f64, trips: &[RoundTrip]) -> Vec<f64> {
    let mut equity = vec![starting];
    let mut cur = starting;
    for t in trips {
        cur += t.net_pnl.to_f64().unwrap_or(0.0);
        equity.push(cur);
    }
    equity
}
