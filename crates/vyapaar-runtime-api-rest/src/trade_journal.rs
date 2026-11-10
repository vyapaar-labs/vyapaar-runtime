//! `/trade-journal/*` routes: trade-performance journal.
//!
//! These wrap the pure [`vyapaar_ledger_performance`] crate — trade
//! annotations, performance metrics, edge score, daily P&L calendar and
//! account comparison — behind HTTP.
//!
//! Journal accounts are deliberately **not** `/portfolio/accounts` (live
//! OMS/trading accounts): a journal account is an analytics grouping, not a
//! live-execution account. This mirrors the three-way-account finding in
//! `vyapaar-ui-web`'s plan 0011 (trading account vs brokerage connection vs
//! journal account).

use std::sync::{Arc, Mutex};

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use chrono::{NaiveDate, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use vyapaar_runtime_api::{ErrorCode, ErrorDetail};

use crate::AppState;
use crate::market::{failure, success};
use vyapaar_ledger_performance::calendar::build_calendar;
use vyapaar_ledger_performance::metrics::{MetricsInput, compute_edge_score, compute_metrics};
use vyapaar_ledger_performance::store::StoreError;
use vyapaar_ledger_performance::{
    model::{AccountProvenance, ClosedTrade, JournalAccountId, TradeAnnotation},
    store::JournalStore,
};
// ---------------------------------------------------------------------------
// Request / response DTOs
// ---------------------------------------------------------------------------

/// `POST /trade-journal/accounts` request body.
#[derive(Debug, Deserialize)]
pub struct CreateAccountRequest {
    pub display_name: String,
    pub provenance: String, // "Manual" | "Imported" | "BrokerSynced"
    pub currency: String,   // ISO 4217 code, e.g. "INR"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub starting_balance: Option<String>,
}

/// `GET /trade-journal/overview` query parameters.
#[derive(Debug, Deserialize)]
pub struct OverviewQuery {
    #[serde(default)]
    pub from: Option<NaiveDate>,
    #[serde(default)]
    pub to: Option<NaiveDate>,
    #[serde(default)]
    pub compare: Option<String>, // "previous" | "none"
}

/// `GET /trade-journal/trades` query parameters.
#[derive(Debug, Deserialize)]
pub struct TradesQuery {
    #[serde(default)]
    pub from: Option<NaiveDate>,
    #[serde(default)]
    pub to: Option<NaiveDate>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub cursor: Option<String>, // last trade id of the previous page
}

/// `GET /trade-journal/calendar` query parameters.
#[derive(Debug, Deserialize)]
pub struct CalendarQuery {
    pub month: String, // "YYYY-MM"
}

/// `GET /trade-journal/notes` query parameters.
#[derive(Debug, Deserialize)]
pub struct NotesQuery {
    #[serde(default)]
    pub account: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

/// `POST /trade-journal/notes` request body.
#[derive(Debug, Deserialize)]
pub struct CreateNoteRequest {
    pub account_id: String,
    pub day: String, // "YYYY-MM-DD"
    pub text: String,
}

/// `GET /trade-journal/compare` query parameters.
#[derive(Debug, Deserialize)]
pub struct CompareQuery {
    #[serde(default, rename = "accountA")]
    pub account_a: Option<String>,
    #[serde(default, rename = "accountB")]
    pub account_b: Option<String>,
    #[serde(default)]
    pub account: Option<String>,
    #[serde(default)]
    pub mode: Option<String>, // "previous"
}

/// `PATCH /trade-journal/trades/:id` request body.
#[derive(Debug, Default, Deserialize)]
pub struct PatchTradeRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rating: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub planned_risk: Option<Decimal>,
}

/// Lightweight account summary for the journal (distinct from OMS accounts).
#[derive(Debug, Clone, Serialize)]
pub struct JournalAccountDto {
    pub id: String,
    pub display_name: String,
    pub provenance: String,
    pub currency: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub starting_balance: Option<String>,
}

/// One closed trade with its annotation (the `/trade-journal/trades` row).
#[derive(Debug, Clone, Serialize)]
pub struct ClosedTradeDto {
    pub trade_id: String,
    pub instrument: String,
    pub side: String,
    pub quantity: String,
    pub entry_price: String,
    pub exit_price: String,
    pub net_pnl: String,
    pub fees: String,
    pub holding_period_days: i64,
    pub is_long_term: bool,
    pub exit_date: String,
    pub duration_ms: Option<u64>,
    pub annotation: TradeAnnotationDto,
}

/// Trade annotation.
#[derive(Debug, Clone, Serialize)]
pub struct TradeAnnotationDto {
    pub tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rating: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub review: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub planned_risk: Option<MoneyDto>,
}

/// Money: exact Decimal amount serialized as a string.
#[derive(Debug, Clone, Serialize)]
pub struct MoneyDto {
    pub amount: String,
    pub currency: String,
}

/// `GET /trade-journal/overview` response (vela `/api/journal/overview` shape).
#[derive(Debug, Clone, Serialize)]
pub struct OverviewResponse {
    pub as_of: String,
    pub time_zone: String,
    pub range: RangeDto,
    pub metrics: PerformanceMetricsDto,
    pub edge_score: EdgeScoreDto,
    pub days: String,
    pub equity: String,
    pub open_positions: String,
    pub previous: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RangeDto {
    pub from: Option<String>,
    pub to: Option<String>,
}

/// Performance metrics (vela field names, exact-Decimal amounts as strings).
#[derive(Debug, Clone, Serialize)]
pub struct PerformanceMetricsDto {
    pub total_trades: u64,
    pub closed_trades: u64,
    pub open_trades: u64,
    pub wins: u64,
    pub losses: u64,
    pub breakevens: u64,
    pub net_pnl: String,
    pub gross_pnl: String,
    pub fees: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub win_rate: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub day_win_rate: Option<String>,
    pub trading_days: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profit_factor: Option<String>,
    pub profit_factor_is_infinite: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avg_win: Option<MoneyDto>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avg_loss: Option<MoneyDto>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avg_win_loss_ratio: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expectancy: Option<MoneyDto>,
    pub largest_win: String,
    pub largest_loss: String,
    pub max_win_streak: u64,
    pub max_loss_streak: u64,
    pub current_streak: i64,
    pub total_volume: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avg_duration_ms: Option<u64>,
    pub max_drawdown: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_drawdown_pct: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovery_factor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profit_concentration: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avg_realized_r: Option<String>,
    pub trades_with_risk: u64,
}

/// Edge score composite (0-100).
#[derive(Debug, Clone, Serialize)]
pub struct EdgeScoreDto {
    pub version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<String>,
    pub components: EdgeScoreComponentsDto,
    pub closed_trades: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct EdgeScoreComponentsDto {
    pub win_rate: String,
    pub profit_factor: String,
    pub avg_win_loss: String,
    pub drawdown: String,
    pub recovery: String,
    pub consistency: String,
}

/// `GET /trade-journal/trades` response (cursor-paginated).
#[derive(Debug, Clone, Serialize)]
pub struct TradesListResponse {
    pub trades: Vec<ClosedTradeDto>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// `GET /trade-journal/calendar` response (vela `/api/journal/calendar` shape).
#[derive(Debug, Clone, Serialize)]
pub struct CalendarResponse {
    pub calendar: CalendarDto,
}

#[derive(Debug, Clone, Serialize)]
pub struct CalendarDto {
    pub year: i32,
    pub month: u32,
    pub weeks: Vec<WeekDto>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WeekDto {
    pub days: Vec<DayDto>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DayDto {
    pub date: String,
    pub net_pnl: String,
    pub gross_pnl: String,
    pub fees: String,
    pub trades: u64,
    pub wins: u64,
    pub losses: u64,
    pub breakevens: u64,
    pub volume: String,
}

/// `GET /trade-journal/notes` response.
#[derive(Debug, Clone, Serialize)]
pub struct NotesListResponse {
    pub notes: Vec<NoteDto>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NoteDto {
    pub account_id: String,
    pub day: String,
    pub text: String,
}

/// `GET /trade-journal/compare` response.
#[derive(Debug, Clone, Serialize)]
pub struct CompareResponse {
    pub account_a: AccountComparisonSideDto,
    pub account_b: AccountComparisonSideDto,
    pub is_prior_period: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct AccountComparisonSideDto {
    pub account_id: String,
    pub metrics: PerformanceMetricsDto,
    pub edge_score: EdgeScoreDto,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

#[allow(clippy::result_large_err)]
fn journal_store(state: &AppState) -> Result<Arc<Mutex<JournalStore>>, Response> {
    state.journal.as_ref().cloned().ok_or_else(|| {
        failure(
            StatusCode::SERVICE_UNAVAILABLE,
            ErrorDetail::new(
                ErrorCode::NotImplemented,
                "no trade-performance journal store is configured",
            )
            .with_context(serde_json::json!({"reason": "no_journal_store"})),
        )
    })
}

fn money_dto(money: &vyapaar_ledger_performance::Money) -> MoneyDto {
    MoneyDto {
        amount: money.amount.to_string(),
        currency: money.currency.code().to_string(),
    }
}

fn optional_money_dto(money: &Option<vyapaar_ledger_performance::Money>) -> Option<MoneyDto> {
    money.as_ref().map(money_dto)
}

fn trade_dto(trade: &ClosedTrade) -> ClosedTradeDto {
    ClosedTradeDto {
        trade_id: trade.trade_id.clone(),
        instrument: trade.instrument.to_string(),
        side: format!("{:?}", trade.side),
        quantity: trade.quantity.value().to_string(),
        entry_price: trade.entry_price.value().to_string(),
        exit_price: trade.exit_price.value().to_string(),
        net_pnl: trade.net_pnl().amount.to_string(),
        fees: trade.fees.amount.to_string(),
        holding_period_days: trade.holding_period_days,
        is_long_term: trade.is_long_term,
        exit_date: trade.exit_date.to_string(),
        duration_ms: trade.duration_ms,
        annotation: TradeAnnotationDto {
            tags: trade.annotation.tags.clone(),
            rating: trade.annotation.rating,
            review: trade.annotation.review.clone(),
            planned_risk: optional_money_dto(&trade.annotation.planned_risk),
        },
    }
}

#[allow(clippy::result_large_err)]
fn parse_provenance(s: &str) -> Result<AccountProvenance, Response> {
    match s {
        "Manual" => Ok(AccountProvenance::Manual),
        "Imported" => Ok(AccountProvenance::Imported),
        "BrokerSynced" => Ok(AccountProvenance::BrokerSynced),
        _ => Err(failure(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorDetail::new(
                ErrorCode::ValidationInvalidRequest,
                format!("invalid provenance {s:?}; must be Manual, Imported or BrokerSynced"),
            ),
        )),
    }
}

fn metrics_dto(
    m: &vyapaar_ledger_performance::metrics::PerformanceMetrics,
) -> PerformanceMetricsDto {
    PerformanceMetricsDto {
        total_trades: m.total_trades,
        closed_trades: m.closed_trades,
        open_trades: m.open_trades,
        wins: m.wins,
        losses: m.losses,
        breakevens: m.breakevens,
        net_pnl: m.net_pnl.amount.to_string(),
        gross_pnl: m.gross_pnl.amount.to_string(),
        fees: m.fees.amount.to_string(),
        win_rate: m.win_rate.map(|d| d.to_string()),
        day_win_rate: m.day_win_rate.map(|d| d.to_string()),
        trading_days: m.trading_days,
        profit_factor: m.profit_factor.map(|d| d.to_string()),
        profit_factor_is_infinite: m.profit_factor_is_infinite,
        avg_win: m.avg_win.as_ref().map(money_dto),
        avg_loss: m.avg_loss.as_ref().map(money_dto),
        avg_win_loss_ratio: m.avg_win_loss_ratio.map(|d| d.to_string()),
        expectancy: m.expectancy.as_ref().map(money_dto),
        largest_win: m.largest_win.amount.to_string(),
        largest_loss: m.largest_loss.amount.to_string(),
        max_win_streak: m.max_win_streak,
        max_loss_streak: m.max_loss_streak,
        current_streak: m.current_streak,
        total_volume: m.total_volume.amount.to_string(),
        avg_duration_ms: m.avg_duration_ms,
        max_drawdown: m.max_drawdown.amount.to_string(),
        max_drawdown_pct: m.max_drawdown_pct.map(|d| d.to_string()),
        recovery_factor: m.recovery_factor.map(|d| d.to_string()),
        profit_concentration: m.profit_concentration.map(|d| d.to_string()),
        avg_realized_r: m.avg_realized_r.map(|d| d.to_string()),
        trades_with_risk: m.trades_with_risk,
    }
}

fn edge_score_dto(e: &vyapaar_ledger_performance::metrics::EdgeScore) -> EdgeScoreDto {
    EdgeScoreDto {
        version: e.version,
        score: e.score.map(|d| d.to_string()),
        components: EdgeScoreComponentsDto {
            win_rate: e.components.win_rate.to_string(),
            profit_factor: e.components.profit_factor.to_string(),
            avg_win_loss: e.components.avg_win_loss.to_string(),
            drawdown: e.components.drawdown.to_string(),
            recovery: e.components.recovery.to_string(),
            consistency: e.components.consistency.to_string(),
        },
        closed_trades: e.closed_trades,
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `GET /trade-journal/accounts` — list journal accounts.
///
/// Distinct from `/portfolio/accounts` (live OMS/trading accounts): a journal
/// account is an analytics grouping, not a live-execution account.
pub async fn get_trade_journal_accounts(State(state): State<Arc<AppState>>) -> Response {
    let store = match journal_store(&state) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let store = store.lock().expect("journal store lock poisoned");
    let accounts: Vec<JournalAccountDto> = store
        .accounts()
        .iter()
        .map(|a| JournalAccountDto {
            id: a.id.to_string(),
            display_name: a.display_name.clone(),
            provenance: format!("{:?}", a.provenance),
            currency: a.currency.code().to_string(),
            starting_balance: a.starting_balance.as_ref().map(|m| m.amount.to_string()),
        })
        .collect();
    success(serde_json::json!({"accounts": accounts}))
}

/// `POST /trade-journal/accounts` — create a journal account.
pub async fn post_trade_journal_account(
    State(state): State<Arc<AppState>>,
    axum::extract::Json(request): axum::extract::Json<CreateAccountRequest>,
) -> Response {
    let store = match journal_store(&state) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let provenance = match parse_provenance(&request.provenance) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let currency = match vyapaar_core::Currency::from_code(&request.currency) {
        Ok(c) => c,
        Err(_) => {
            return failure(
                StatusCode::UNPROCESSABLE_ENTITY,
                ErrorDetail::new(
                    ErrorCode::ValidationInvalidRequest,
                    format!("invalid currency {:?}", request.currency),
                ),
            );
        }
    };
    let mut store = store.lock().expect("journal store lock poisoned");
    let id = JournalAccountId::new(format!("jn-{}", Uuid::new_v4()));
    let starting_balance = request
        .starting_balance
        .as_deref()
        .and_then(|s| s.parse::<Decimal>().ok())
        .map(vyapaar_ledger_performance::Money::inr);
    match starting_balance {
        Some(bal) => match store.create_account_with_balance(
            id,
            request.display_name,
            provenance,
            currency,
            bal,
        ) {
            Ok(account) => (
                StatusCode::CREATED,
                Json(serde_json::json!({
                    "id": account.id,
                    "display_name": account.display_name,
                    "provenance": format!("{:?}", account.provenance),
                    "currency": account.currency.code(),
                    "starting_balance": account.starting_balance.map(|m| m.amount.to_string()),
                })),
            )
                .into_response(),
            Err(StoreError::AccountAlreadyExists(id)) => failure(
                StatusCode::CONFLICT,
                ErrorDetail::new(
                    ErrorCode::Conflict,
                    format!("journal account {id} already exists"),
                ),
            ),
            Err(_) => failure(
                StatusCode::INTERNAL_SERVER_ERROR,
                ErrorDetail::new(ErrorCode::InternalError, "failed to create journal account"),
            ),
        },
        None => failure(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorDetail::new(
                ErrorCode::ValidationInvalidRequest,
                "invalid starting_balance",
            ),
        ),
    }
}

/// `GET /trade-journal/overview` — metrics + edge score.
pub async fn get_trade_journal_overview(
    State(state): State<Arc<AppState>>,
    Query(query): Query<OverviewQuery>,
) -> Response {
    let store = match journal_store(&state) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let store = store.lock().expect("journal store lock poisoned");
    let trades = store.closed_trades_in_range(query.from, query.to);
    let input = MetricsInput::new(trades.into_iter().cloned().collect());
    let metrics = compute_metrics(&input).unwrap_or_else(|_| default_metrics());
    let edge_score = compute_edge_score(&metrics);

    let range_dto = RangeDto {
        from: query.from.map(|d| d.to_string()),
        to: query.to.map(|d| d.to_string()),
    };
    let previous = if query.compare.as_deref() == Some("previous") {
        // Compare against the prior period of equal length: split the traded
        // range at its midpoint and score the earlier half.
        let (from, to) = match (query.from, query.to) {
            (Some(f), Some(t)) => {
                let days = t.signed_duration_since(f).num_days();
                let mid = f + chrono::Duration::days(days / 2);
                (Some(f), Some(mid))
            }
            _ => (None, None),
        };
        let prior = compute_metrics(&MetricsInput::new(
            store
                .closed_trades_in_range(from, to)
                .into_iter()
                .cloned()
                .collect(),
        ))
        .unwrap_or_else(|_| default_metrics());
        Some(format!("{:?}", compute_edge_score(&prior).score))
    } else {
        None
    };

    let now = Utc::now();
    let response = OverviewResponse {
        as_of: now.to_rfc3339(),
        time_zone: "UTC".to_string(),
        range: range_dto,
        metrics: metrics_dto(&metrics),
        edge_score: edge_score_dto(&edge_score),
        days: metrics.trading_days.to_string(),
        equity: metrics.net_pnl.amount.to_string(),
        open_positions: metrics.open_trades.to_string(),
        previous: previous.unwrap_or_default(),
    };
    success(response)
}

/// `GET /trade-journal/trades` — closed trades with annotations, cursor-paginated.
pub async fn get_trade_journal_trades(
    State(state): State<Arc<AppState>>,
    Query(query): Query<TradesQuery>,
) -> Response {
    let store = match journal_store(&state) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let store = store.lock().expect("journal store lock poisoned");
    let mut trades: Vec<&ClosedTrade> = store.closed_trades_in_range(query.from, query.to);
    // Start after the cursor (exclusive), if given.
    if let Some(cursor) = &query.cursor {
        trades.retain(|t| t.trade_id.as_str() > cursor.as_str());
    }
    let limit = query.limit.unwrap_or(100);
    let total = trades.len();
    let page: Vec<&ClosedTrade> = trades.into_iter().take(limit).collect();
    let has_more = total > page.len();
    let next_cursor = has_more.then_some(page.last().unwrap().trade_id.clone());
    let dtos: Vec<ClosedTradeDto> = page.into_iter().map(trade_dto).collect();
    success(TradesListResponse {
        trades: dtos,
        next_cursor,
    })
}

/// `PATCH /trade-journal/trades/:id` — set a trade's annotation.
pub async fn patch_trade_journal_trade(
    State(state): State<Arc<AppState>>,
    Path(trade_id): Path<String>,
    axum::extract::Json(request): axum::extract::Json<PatchTradeRequest>,
) -> Response {
    let store = match journal_store(&state) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let mut store = store.lock().expect("journal store lock poisoned");
    let existing = store.annotation(&trade_id).cloned().unwrap_or_default();
    let annotation = TradeAnnotation {
        tags: request.tags.unwrap_or(existing.tags),
        rating: request.rating.or(existing.rating),
        review: request.review.or(existing.review),
        planned_risk: request
            .planned_risk
            .map(vyapaar_ledger_performance::Money::inr)
            .or(existing.planned_risk),
    };
    if store.set_annotation(&trade_id, annotation.clone()).is_err() {
        return failure(
            StatusCode::INTERNAL_SERVER_ERROR,
            ErrorDetail::new(ErrorCode::InternalError, "failed to store trade annotation"),
        );
    }
    success(TradeAnnotationDto {
        tags: annotation.tags,
        rating: annotation.rating,
        review: annotation.review,
        planned_risk: optional_money_dto(&annotation.planned_risk),
    })
}

/// `GET /trade-journal/calendar` — daily P&L heatmap for a month.
pub async fn get_trade_journal_calendar(
    State(state): State<Arc<AppState>>,
    Query(query): Query<CalendarQuery>,
) -> Response {
    let store = match journal_store(&state) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let store = store.lock().expect("journal store lock poisoned");
    let (year, month) = match parse_month(&query.month) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let calendar = build_calendar(
        &store
            .closed_trades()
            .into_iter()
            .cloned()
            .collect::<Vec<ClosedTrade>>(),
        year,
        month,
    );
    success(CalendarResponse {
        calendar: CalendarDto {
            year: calendar.year,
            month: calendar.month,
            weeks: calendar
                .weeks
                .into_iter()
                .map(|w| WeekDto {
                    days: w
                        .days
                        .into_iter()
                        .map(|d| DayDto {
                            date: d.date.to_string(),
                            net_pnl: d.net_pnl.amount.to_string(),
                            gross_pnl: d.gross_pnl.amount.to_string(),
                            fees: d.fees.amount.to_string(),
                            trades: d.trades,
                            wins: d.wins,
                            losses: d.losses,
                            breakevens: d.breakevens,
                            volume: d.volume.amount.to_string(),
                        })
                        .collect(),
                })
                .collect(),
        },
    })
}

/// `GET /trade-journal/notes` — day-keyed journal notes for an account.
pub async fn get_trade_journal_notes(
    State(state): State<Arc<AppState>>,
    Query(query): Query<NotesQuery>,
) -> Response {
    let store = match journal_store(&state) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let store = store.lock().expect("journal store lock poisoned");
    let account_id = match query.account {
        Some(a) => JournalAccountId::new(a),
        None => {
            return failure(
                StatusCode::UNPROCESSABLE_ENTITY,
                ErrorDetail::new(
                    ErrorCode::ValidationInvalidRequest,
                    "account query parameter is required",
                ),
            );
        }
    };
    let notes = store.notes_for_account(&account_id);
    let limit = query.limit.unwrap_or(usize::MAX);
    let dtos: Vec<NoteDto> = notes
        .into_iter()
        .take(limit)
        .map(|n| NoteDto {
            account_id: n.account_id.to_string(),
            day: n.day.to_string(),
            text: n.text,
        })
        .collect();
    success(NotesListResponse { notes: dtos })
}

/// `POST /trade-journal/notes` — create or replace a day-keyed note.
pub async fn post_trade_journal_note(
    State(state): State<Arc<AppState>>,
    axum::extract::Json(request): axum::extract::Json<CreateNoteRequest>,
) -> Response {
    let store = match journal_store(&state) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let account_id = JournalAccountId::new(request.account_id);
    let day = match NaiveDate::parse_from_str(&request.day, "%Y-%m-%d") {
        Ok(d) => d,
        Err(_) => {
            return failure(
                StatusCode::UNPROCESSABLE_ENTITY,
                ErrorDetail::new(
                    ErrorCode::ValidationInvalidRequest,
                    format!("invalid day {:?}; expected YYYY-MM-DD", request.day),
                ),
            );
        }
    };
    let mut store = store.lock().expect("journal store lock poisoned");
    if store.set_note(&account_id, day, request.text).is_err() {
        return failure(
            StatusCode::NOT_FOUND,
            ErrorDetail::new(
                ErrorCode::NotFound,
                format!("journal account {account_id} not found"),
            ),
        );
    }
    (StatusCode::CREATED, Json(serde_json::json!({"ok": true}))).into_response()
}

/// `GET /trade-journal/compare` — two-sided metrics comparison.
pub async fn get_trade_journal_compare(
    State(state): State<Arc<AppState>>,
    Query(query): Query<CompareQuery>,
) -> Response {
    let store = match journal_store(&state) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let store = store.lock().expect("journal store lock poisoned");

    let (trades_a, trades_b, is_prior_period, id_a, id_b) = if let (Some(a), Some(b)) =
        (&query.account_a, &query.account_b)
    {
        (
            store.closed_trades().into_iter().cloned().collect(),
            store.closed_trades().into_iter().cloned().collect(),
            false,
            a.clone(),
            b.clone(),
        )
    } else if let Some(account) = &query.account {
        if query.mode.as_deref() != Some("previous") {
            return failure(
                StatusCode::UNPROCESSABLE_ENTITY,
                ErrorDetail::new(
                    ErrorCode::ValidationInvalidRequest,
                    "compare with a single account requires mode=previous",
                ),
            );
        }
        let mut all_trades: Vec<ClosedTrade> = store.closed_trades().into_iter().cloned().collect();
        all_trades.sort_by_key(|t| t.exit_date);
        let mid = all_trades.len() / 2;
        let prior: Vec<ClosedTrade> = all_trades.iter().take(mid).cloned().collect();
        let current: Vec<ClosedTrade> = all_trades.iter().skip(mid).cloned().collect();
        (
            current,
            prior,
            true,
            account.clone(),
            format!("{account} (prior)"),
        )
    } else {
        return failure(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorDetail::new(
                ErrorCode::ValidationInvalidRequest,
                "provide accountA+accountB or account+mode=previous",
            ),
        );
    };

    let metrics_a =
        compute_metrics(&MetricsInput::new(trades_a)).unwrap_or_else(|_| default_metrics());
    let edge_a = compute_edge_score(&metrics_a);
    let metrics_b =
        compute_metrics(&MetricsInput::new(trades_b)).unwrap_or_else(|_| default_metrics());
    let edge_b = compute_edge_score(&metrics_b);

    success(CompareResponse {
        account_a: AccountComparisonSideDto {
            account_id: id_a,
            metrics: metrics_dto(&metrics_a),
            edge_score: edge_score_dto(&edge_a),
        },
        account_b: AccountComparisonSideDto {
            account_id: id_b,
            metrics: metrics_dto(&metrics_b),
            edge_score: edge_score_dto(&edge_b),
        },
        is_prior_period,
    })
}

#[allow(clippy::result_large_err)]
fn parse_month(s: &str) -> Result<(i32, u32), Response> {
    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() != 2 {
        return Err(failure(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorDetail::new(
                ErrorCode::ValidationInvalidRequest,
                format!("invalid month {s:?}; expected YYYY-MM"),
            ),
        ));
    }
    let year: i32 = parts[0].parse().map_err(|_| {
        failure(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorDetail::new(
                ErrorCode::ValidationInvalidRequest,
                format!("invalid year {:?}", parts[0]),
            ),
        )
    })?;
    let month: u32 = parts[1].parse().map_err(|_| {
        failure(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorDetail::new(
                ErrorCode::ValidationInvalidRequest,
                format!("invalid month {:?}", parts[1]),
            ),
        )
    })?;
    if !(1..=12).contains(&month) {
        return Err(failure(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorDetail::new(
                ErrorCode::ValidationInvalidRequest,
                format!("month {month} out of range 1..=12"),
            ),
        ));
    }
    Ok((year, month))
}

fn default_metrics() -> vyapaar_ledger_performance::metrics::PerformanceMetrics {
    vyapaar_ledger_performance::metrics::PerformanceMetrics {
        total_trades: 0,
        closed_trades: 0,
        open_trades: 0,
        wins: 0,
        losses: 0,
        breakevens: 0,
        net_pnl: vyapaar_ledger_performance::Money::zero_inr(),
        gross_pnl: vyapaar_ledger_performance::Money::zero_inr(),
        fees: vyapaar_ledger_performance::Money::zero_inr(),
        win_rate: None,
        day_win_rate: None,
        trading_days: 0,
        profit_factor: None,
        profit_factor_is_infinite: false,
        avg_win: None,
        avg_loss: None,
        avg_win_loss_ratio: None,
        expectancy: None,
        largest_win: vyapaar_ledger_performance::Money::zero_inr(),
        largest_loss: vyapaar_ledger_performance::Money::zero_inr(),
        max_win_streak: 0,
        max_loss_streak: 0,
        current_streak: 0,
        total_volume: vyapaar_ledger_performance::Money::zero_inr(),
        avg_duration_ms: None,
        max_drawdown: vyapaar_ledger_performance::Money::zero_inr(),
        max_drawdown_pct: None,
        recovery_factor: None,
        profit_concentration: None,
        avg_realized_r: None,
        trades_with_risk: 0,
    }
}
