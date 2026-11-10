//! Handlers for `GET /instruments`, `GET /instruments/{id}`, `GET /bars/{id}`, `GET /quotes`
//! and `GET /depth/{id}`.
//!
//! Each handler resolves its request with the pure functions in `vyapaar-api`,
//! calls a read port, and answers inside the standard envelope. Failures use
//! the same envelope: 404 for an unknown instrument or an unavailable
//! timeframe, 422 for a bad query, and a port failure maps by kind.

use std::sync::Arc;

use axum::{
    async_trait,
    extract::{FromRequestParts, Path, Query, State, rejection::QueryRejection},
    http::{StatusCode, request::Parts},
    response::{IntoResponse, Json, Response},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::sync::OnceLock;
use vyapaar_core::rust_decimal::Decimal;
use vyapaar_runtime_api::{
    ApiResponse, BarsQuery, BarsResponse, DepthLevel, DepthQuery, DepthResponse, ErrorCode,
    ErrorDetail, InstrumentsQuery, InstrumentsResponse, QuotesQuery, QuotesResponse,
    ResponseEnvelope, instrument_json, parse_instrument_id,
};
use vyapaar_runtime_feed::ports::{BarRequest, PortError};
use vyapaar_runtime_feed::wire::UnixNanos;

use crate::AppState;

/// Converts `vyapaar-feed`'s own instrument-kind vocabulary (really just
/// `vyapaar_core::InstrumentKind`, reused as-is by that crate's `Instrument`) to the richer,
/// REST-contract `vyapaar_runtime_api::wire::InstrumentKind`. Every `vyapaar_core` variant has a direct
/// match; the wire enum's extra variants (`Etf`, `Bond`, `Ipo`, `Fx`, `MutualFund`, `Other`)
/// have no `vyapaar_core` source today and so are never produced here.
fn to_api_kind(kind: vyapaar_core::InstrumentKind) -> vyapaar_runtime_api::wire::InstrumentKind {
    match kind {
        vyapaar_core::InstrumentKind::Equity => vyapaar_runtime_api::wire::InstrumentKind::Equity,
        vyapaar_core::InstrumentKind::Future => vyapaar_runtime_api::wire::InstrumentKind::Future,
        vyapaar_core::InstrumentKind::Option => vyapaar_runtime_api::wire::InstrumentKind::Option,
        vyapaar_core::InstrumentKind::Index => vyapaar_runtime_api::wire::InstrumentKind::Index,
    }
}

/// Converts a `vyapaar-feed` reference-data record (this crate's instrument master port still
/// answers in that shape; see `state.rs`'s module doc) to the `vyapaar_runtime_api::wire::Instrument`
/// read model `GET /instruments` actually serializes. `lot_size`/`tick_size` are `f64` on the
/// feed side (its Parquet catalog predates `Decimal`) and `Decimal` on the wire, so they are
/// converted at this one JSON-response boundary, not threaded through as `f64`.
fn to_api_instrument(
    instrument: &vyapaar_runtime_feed::wire::Instrument,
) -> vyapaar_runtime_api::wire::Instrument {
    vyapaar_runtime_api::wire::Instrument::new(
        instrument.id().clone(),
        to_api_kind(instrument.kind()),
        instrument.currency(),
        Decimal::from_f64_retain(instrument.lot_size()).unwrap_or_default(),
        Decimal::from_f64_retain(instrument.tick_size()).unwrap_or_default(),
    )
}

/// Converts a `vyapaar-feed` quote (`f64`-valued, dual `UnixNanos` timestamps) to the
/// `vyapaar_runtime_api::wire::QuoteTick` `GET /quotes` serializes (`Decimal`-valued,
/// `chrono::DateTime<Utc>`), at the JSON-response boundary.
fn to_api_quote(
    quote: vyapaar_runtime_feed::wire::QuoteTick,
) -> vyapaar_runtime_api::wire::QuoteTick {
    vyapaar_runtime_api::wire::QuoteTick::new(
        quote.instrument_id().clone(),
        Decimal::from_f64_retain(quote.bid_price()).unwrap_or_default(),
        Decimal::from_f64_retain(quote.ask_price()).unwrap_or_default(),
        Decimal::from_f64_retain(quote.bid_size()).unwrap_or_default(),
        Decimal::from_f64_retain(quote.ask_size()).unwrap_or_default(),
        quote.ts_event().to_datetime(),
        quote.ts_init().to_datetime(),
    )
}

/// Converts a `vyapaar-feed` bar (`f64`-valued, dual `UnixNanos` timestamps) to the canonical
/// `vyapaar_core::Bar` that `BarsResponse` serializes directly (ADR: the REST bars contract
/// reuses the domain `Bar`, not a separate wire copy), at the JSON-response boundary.
fn to_core_bar(bar: vyapaar_runtime_feed::wire::Bar) -> vyapaar_core::Bar {
    vyapaar_core::Bar {
        instrument: bar.bar_type().instrument_id().clone(),
        open: vyapaar_core::Price::new(Decimal::from_f64_retain(bar.open()).unwrap_or_default()),
        high: vyapaar_core::Price::new(Decimal::from_f64_retain(bar.high()).unwrap_or_default()),
        low: vyapaar_core::Price::new(Decimal::from_f64_retain(bar.low()).unwrap_or_default()),
        close: vyapaar_core::Price::new(Decimal::from_f64_retain(bar.close()).unwrap_or_default()),
        volume: vyapaar_core::Quantity::new(
            Decimal::from_f64_retain(bar.volume()).unwrap_or_default(),
        ),
        timestamp: bar.ts_event().to_datetime(),
    }
}

/// Query-string extractor whose rejections are the standard error envelope
/// (`validation_invalid_request`, status 422) rather than axum's plain text.
#[derive(Debug)]
pub struct ApiQuery<T>(pub T);

/// Rejection of [`ApiQuery`].
#[derive(Debug)]
pub struct ApiQueryRejection(QueryRejection);

impl IntoResponse for ApiQueryRejection {
    fn into_response(self) -> Response {
        failure(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorDetail::new(ErrorCode::ValidationInvalidRequest, self.0.body_text()),
        )
    }
}

#[async_trait]
impl<S, T> FromRequestParts<S> for ApiQuery<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiQueryRejection;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        Query::<T>::from_request_parts(parts, state)
            .await
            .map(|Query(value)| Self(value))
            .map_err(ApiQueryRejection)
    }
}

/// Hard cap on the bars one `GET /bars/{id}` may return.
///
/// A request selecting more is a 422 (`reason: too_many_rows`); the caller narrows `from`/`to`.
/// There is no pagination.
pub const MAX_BAR_ROWS: usize = 100_000;

/// Rejects a selection of `rows` bars larger than `limit` with a 422-bound detail.
pub(crate) fn check_row_cap(rows: usize, limit: usize) -> Result<(), ErrorDetail> {
    if rows <= limit {
        return Ok(());
    }
    Err(ErrorDetail::new(
        ErrorCode::ValidationInvalidRequest,
        format!(
            "the selection has {rows} bars, over the limit of {limit}; narrow the range with from and to"
        ),
    )
    .with_context(serde_json::json!({
        "field": "to",
        "reason": "too_many_rows",
        "limit": limit,
    })))
}

pub(crate) fn failure(status: StatusCode, detail: ErrorDetail) -> Response {
    let body: ResponseEnvelope<serde_json::Value> = ApiResponse::error(detail);
    (status, Json(body)).into_response()
}

pub(crate) fn success<T: serde::Serialize>(data: T) -> Response {
    let body: ResponseEnvelope<T> = ApiResponse::success(data);
    (StatusCode::OK, Json(body)).into_response()
}

pub(crate) fn unprocessable(detail: ErrorDetail) -> Response {
    failure(StatusCode::UNPROCESSABLE_ENTITY, detail)
}

pub(crate) fn instrument_not_found(id: &vyapaar_core::InstrumentId) -> Response {
    failure(
        StatusCode::NOT_FOUND,
        ErrorDetail::new(ErrorCode::NotFound, format!("instrument {id} not found")),
    )
}

/// Reads one instrument from the master; a port failure becomes `internal_error`.
pub(crate) async fn master_get(
    state: &AppState,
    id: &vyapaar_core::InstrumentId,
) -> Result<Option<vyapaar_runtime_feed::wire::Instrument>, ErrorDetail> {
    state
        .instruments
        .get_instrument(id)
        .await
        .map_err(|e| ErrorDetail::new(ErrorCode::InternalError, e.to_string()))
}

/// Maps a port failure to a status and an envelope code.
///
/// `vyapaar_runtime_feed::ports::PortError` has only three variants (`Unsupported`,
/// `Internal`, `Terminal`) — leaner than the old `vyapaar_runtime_api` taxonomy
/// this used to branch on (`InvalidRequest`/`Unavailable`/`Timeout`/
/// `Transport`), so the mapping collapses to that.
pub(crate) fn port_failure(error: PortError) -> Response {
    let (status, code) = match &error {
        PortError::Unsupported(_) => (StatusCode::NOT_FOUND, ErrorCode::NotFound),
        PortError::Terminal(_) => (StatusCode::SERVICE_UNAVAILABLE, ErrorCode::Conflict),
        PortError::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, ErrorCode::InternalError),
    };
    failure(status, ErrorDetail::new(code, error.to_string()))
}

/// `GET /instruments`: every instrument passing the query filters, in id order.
pub(crate) async fn get_instruments(
    State(state): State<Arc<AppState>>,
    ApiQuery(query): ApiQuery<InstrumentsQuery>,
) -> Response {
    match state.instruments.list_instruments().await {
        Ok(mut all) => {
            all.sort_by(|a, b| a.id().cmp(b.id()));
            let instruments = all
                .iter()
                .filter(|instrument| query.matches(instrument.id()))
                .map(|instrument| instrument_json(&to_api_instrument(instrument)))
                .collect();
            success(InstrumentsResponse { instruments })
        }
        Err(error) => port_failure(error),
    }
}

/// `GET /instruments/{id}`: one instrument, or 404 `instrument_not_found`.
pub(crate) async fn get_instrument_by_id(
    State(state): State<Arc<AppState>>,
    Path(raw): Path<String>,
) -> Response {
    let id = match parse_instrument_id(&raw) {
        Ok(id) => id,
        Err(detail) => return unprocessable(detail),
    };
    match state.instruments.get_instrument(&id).await {
        Ok(Some(instrument)) => success(instrument_json(&to_api_instrument(&instrument))),
        Ok(None) => instrument_not_found(&id),
        Err(error) => port_failure(error),
    }
}

/// `GET /bars/{id}`: bars in ascending `ts_event` over `[from, to)`.
pub(crate) async fn get_bars(
    State(state): State<Arc<AppState>>,
    Path(raw): Path<String>,
    ApiQuery(query): ApiQuery<BarsQuery>,
) -> Response {
    let id = match parse_instrument_id(&raw) {
        Ok(id) => id,
        Err(detail) => return unprocessable(detail),
    };
    let resolved = match query.resolve() {
        Ok(resolved) => resolved,
        Err(detail) => return unprocessable(detail),
    };
    match state.instruments.get_instrument(&id).await {
        Ok(Some(_)) => {}
        Ok(None) => return instrument_not_found(&id),
        Err(error) => return port_failure(error),
    }
    let request = match BarRequest::new(
        id,
        crate::catalog::to_feed_spec(resolved.spec),
        resolved.from.map(UnixNanos::from_datetime),
        resolved.to.map(UnixNanos::from_datetime),
    ) {
        Ok(request) => request,
        Err(error) => return port_failure(error),
    };
    match state.bars.read_bars(&request).await {
        Ok(bars) => match check_row_cap(bars.len(), MAX_BAR_ROWS) {
            Ok(()) => success(BarsResponse {
                bars: bars.into_iter().map(to_core_bar).collect(),
            }),
            Err(detail) => unprocessable(detail),
        },
        Err(error) => port_failure(error),
    }
}

/// `GET /quotes`: the latest (or `as_of`) quote of every instrument the symbols name.
///
/// Symbols expand across venues unless `venue` is set; results are in instrument-id order. A
/// symbol that matches no instrument is a 404 `instrument_not_found`; an instrument with no quote
/// at that time is a 404 `not_found`, so a partial answer is never passed off as
/// complete.
pub(crate) async fn get_quotes(
    State(state): State<Arc<AppState>>,
    ApiQuery(query): ApiQuery<QuotesQuery>,
) -> Response {
    let resolved = match query.resolve() {
        Ok(resolved) => resolved,
        Err(detail) => return unprocessable(detail),
    };
    let mut known = match state.instruments.list_instruments().await {
        Ok(all) => all,
        Err(error) => return port_failure(error),
    };
    known.sort_by(|a, b| a.id().cmp(b.id()));
    let mut wanted = Vec::new();
    for symbol in &resolved.symbols {
        let before = wanted.len();
        wanted.extend(known.iter().map(|instrument| instrument.id()).filter(|id| {
            id.symbol == *symbol && resolved.venue.as_ref().is_none_or(|v| id.exchange == *v)
        }));
        if wanted.len() == before {
            return failure(
                StatusCode::NOT_FOUND,
                ErrorDetail::new(
                    ErrorCode::NotFound,
                    format!("no instrument for symbol {symbol}"),
                ),
            );
        }
    }
    wanted.sort();
    wanted.dedup();
    let mut quotes = Vec::with_capacity(wanted.len());
    for id in wanted {
        match state
            .quotes
            .read_quote(id, resolved.as_of.map(UnixNanos::from_datetime))
            .await
        {
            Ok(Some(quote)) => quotes.push(to_api_quote(quote)),
            Ok(None) => {
                return failure(
                    StatusCode::NOT_FOUND,
                    ErrorDetail::new(
                        ErrorCode::NotFound,
                        format!("no quote for {id} at the requested time"),
                    ),
                );
            }
            Err(error) => return port_failure(error),
        }
    }
    success(QuotesResponse { quotes })
}

/// `GET /depth/{id}`: the order book, or 404 `not_found` when the source has none.
pub(crate) async fn get_depth(
    State(state): State<Arc<AppState>>,
    Path(raw): Path<String>,
    ApiQuery(query): ApiQuery<DepthQuery>,
) -> Response {
    let id = match parse_instrument_id(&raw) {
        Ok(id) => id,
        Err(detail) => return unprocessable(detail),
    };
    let levels = match query.levels() {
        Ok(levels) => levels,
        Err(detail) => return unprocessable(detail),
    };
    match state.instruments.get_instrument(&id).await {
        Ok(Some(_)) => {}
        Ok(None) => return instrument_not_found(&id),
        Err(error) => return port_failure(error),
    }
    match state.depth.read_depth(&id, levels).await {
        Ok(book) => {
            let side = |levels: Vec<(f64, f64)>| {
                levels
                    .into_iter()
                    .map(|(price, qty)| DepthLevel {
                        price: Decimal::from_f64_retain(price).unwrap_or_default(),
                        qty: Decimal::from_f64_retain(qty).unwrap_or_default(),
                    })
                    .collect()
            };
            success(DepthResponse {
                bids: side(book.bids),
                asks: side(book.asks),
            })
        }
        Err(error) => port_failure(error),
    }
}

// ---------------------------------------------------------------------------
// Phase 3 — market reference data (symbol profile + calendar).
//
// These endpoints are backed by vyapaar-data's calendar (gap 0001) and
// symbol-profile (gap 0002) ingestion. vyapaar-data is a Python service and
// is not yet FFI-wired into this Rust REST server, so the stores below are
// populated with illustrative sample data. Once the vyapaar-data → Rust FFI
// bridge exists, these handlers should read from it instead. The routes and
// response shapes match
// vyapaar-runtime/docs/plans/0003-trade-journal-and-market-reference-rest-endpoints.md.

/// One symbol profile (vela `public-get-symbol-profile` shape).
#[derive(Debug, Clone, Serialize, Default)]
pub struct SymbolProfileDto {
    pub symbol: String,
    pub name: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ceo: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub employees: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub website: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headquarters: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub industry: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sector: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ipo_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exchange: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub market_cap: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub beta: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_dividend: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range_52_week: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub average_volume: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_etf: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_actively_trading: Option<bool>,
}

/// One calendar event (vela calendar shape).
#[derive(Debug, Clone, Serialize)]
pub struct CalendarEventDto {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instrument: Option<String>,
    pub scheduled_at: String,
    pub country: String,
    pub currency: String,
    pub category: String,
    pub event_name: String,
    pub impact_level: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forecast: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prior: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub period_label: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MarketCalendarResponse {
    pub events: Vec<CalendarEventDto>,
}

/// Sample symbol profiles — placeholder until the vyapaar-data FFI bridge
/// backs this endpoint with real ingestion (gap 0002).
fn sample_profiles() -> Vec<SymbolProfileDto> {
    vec![
        SymbolProfileDto {
            symbol: "NSE:RELIANCE".to_string(),
            name: "Reliance Industries Ltd.".to_string(),
            description: "Integrated energy and conglomerate.".to_string(),
            ceo: Some("Mukesh Ambani".to_string()),
            sector: Some("Energy".to_string()),
            industry: Some("Oil & Gas".to_string()),
            market_cap: Some("1800000000000.00".to_string()),
            beta: Some("0.95".to_string()),
            is_actively_trading: Some(true),
            ..Default::default()
        },
        SymbolProfileDto {
            symbol: "NSE:TCS".to_string(),
            name: "Tata Consultancy Services".to_string(),
            description: "IT services and consulting.".to_string(),
            sector: Some("Technology".to_string()),
            industry: Some("IT Services".to_string()),
            market_cap: Some("1400000000000.00".to_string()),
            beta: Some("0.78".to_string()),
            is_actively_trading: Some(true),
            ..Default::default()
        },
    ]
}

/// Sample calendar events — placeholder until the vyapaar-data FFI bridge
/// backs this endpoint with real ingestion (gap 0001).
fn sample_calendar_events() -> Vec<CalendarEventDto> {
    vec![
        CalendarEventDto {
            instrument: Some("NSE:RELIANCE-EQ".to_string()),
            scheduled_at: "2026-11-10T06:30:00Z".to_string(),
            country: "IN".to_string(),
            currency: "INR".to_string(),
            category: "Earnings".to_string(),
            event_name: "Reliance Industries Q3 FY27 Earnings".to_string(),
            impact_level: "High".to_string(),
            actual: None,
            forecast: Some("12400.00".to_string()),
            prior: Some("11850.00".to_string()),
            period_label: Some("Q3 FY27".to_string()),
        },
        CalendarEventDto {
            instrument: None,
            scheduled_at: "2026-11-16T06:00:00Z".to_string(),
            country: "US".to_string(),
            currency: "USD".to_string(),
            category: "Economic".to_string(),
            event_name: "US Retail Sales MoM".to_string(),
            impact_level: "High".to_string(),
            actual: None,
            forecast: Some("0.40".to_string()),
            prior: Some("0.20".to_string()),
            period_label: Some("Oct".to_string()),
        },
    ]
}

static PROFILES: OnceLock<Vec<SymbolProfileDto>> = OnceLock::new();
static CALENDAR_EVENTS: OnceLock<Vec<CalendarEventDto>> = OnceLock::new();

fn profiles_store() -> &'static Vec<SymbolProfileDto> {
    PROFILES.get_or_init(sample_profiles)
}

fn calendar_events_store() -> &'static Vec<CalendarEventDto> {
    CALENDAR_EVENTS.get_or_init(sample_calendar_events)
}

/// `GET /market-data/symbol-profile/:id` — company fundamentals.
///
/// Backed by vyapaar-data's symbol-profile ingestion (gap 0002). Until the
/// vyapaar-data → Rust FFI bridge is wired, this serves illustrative sample
/// data.
pub(crate) async fn get_symbol_profile(
    State(_state): State<Arc<AppState>>,
    Path(raw): Path<String>,
) -> Response {
    let id = match parse_instrument_id(&raw) {
        Ok(id) => id,
        Err(detail) => return unprocessable(detail),
    };
    let profile = profiles_store().iter().find(|p| p.symbol == id.to_string());
    match profile {
        Some(p) => success(p.clone()),
        None => instrument_not_found(&id),
    }
}

/// `GET /market-data/calendar` — economic/earnings/dividends/splits/IPO events.
///
/// Backed by vyapaar-data's calendar ingestion (gap 0001). Until the
/// vyapaar-data → Rust FFI bridge is wired, this serves illustrative sample
/// data.
pub(crate) async fn get_market_calendar(
    State(_state): State<Arc<AppState>>,
    ApiQuery(query): ApiQuery<MarketCalendarQuery>,
) -> Response {
    let events: Vec<CalendarEventDto> = calendar_events_store()
        .iter()
        .filter(|e| {
            if let Some(from) = &query.from {
                if e.scheduled_at.as_str() < from.as_str() {
                    return false;
                }
            }
            if let Some(to) = &query.to {
                if e.scheduled_at.as_str() > to.as_str() {
                    return false;
                }
            }
            if let Some(cat) = &query.category {
                if e.category != *cat {
                    return false;
                }
            }
            true
        })
        .cloned()
        .collect();
    success(MarketCalendarResponse { events })
}

/// `GET /market-data/calendar` query parameters.
#[derive(Debug, Deserialize)]
pub struct MarketCalendarQuery {
    #[serde(default)]
    pub from: Option<String>,
    #[serde(default)]
    pub to: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
}
