//! Multi-dimensional job execution budgets and guards (ADR 0017 / Jesse pattern).

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Multi-dimensional budget constraints for a background job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct JobBudgets {
    /// Maximum allowed agent turns or iteration steps.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_turns: Option<usize>,
    /// Maximum allowed statutory and commission friction in INR.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_inr: Option<Decimal>,
    /// Maximum allowed statutory and commission friction in USD.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_usd: Option<Decimal>,
    /// Maximum allowed LLM or token usage count.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<usize>,
    /// Maximum allowed tool call count.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tool_calls: Option<usize>,
    /// Maximum allowed data fetch calls.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_data_calls: Option<usize>,
}

impl JobBudgets {
    /// Creates an empty (unconstrained) budget.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the maximum turn budget.
    pub fn with_max_turns(mut self, turns: usize) -> Self {
        self.max_turns = Some(turns);
        self
    }

    /// Sets the maximum INR statutory and fee friction budget.
    pub fn with_max_inr(mut self, inr: Decimal) -> Self {
        self.max_inr = Some(inr);
        self
    }

    /// Sets the maximum USD fee friction budget.
    pub fn with_max_usd(mut self, usd: Decimal) -> Self {
        self.max_usd = Some(usd);
        self
    }

    /// Sets the maximum LLM token budget.
    pub fn with_max_tokens(mut self, tokens: usize) -> Self {
        self.max_tokens = Some(tokens);
        self
    }

    /// Sets the maximum tool call budget.
    pub fn with_max_tool_calls(mut self, tool_calls: usize) -> Self {
        self.max_tool_calls = Some(tool_calls);
        self
    }

    /// Sets the maximum data fetch call budget.
    pub fn with_max_data_calls(mut self, data_calls: usize) -> Self {
        self.max_data_calls = Some(data_calls);
        self
    }

    /// Validates `usage` against the configured budgets.
    pub fn validate(&self, usage: &BudgetUsage) -> Result<(), BudgetExceededError> {
        if let Some(limit) = self.max_turns {
            if usage.turns > limit {
                return Err(BudgetExceededError::TurnsExceeded {
                    current: usage.turns,
                    limit,
                });
            }
        }
        if let Some(limit) = self.max_inr {
            if usage.accrued_inr > limit {
                return Err(BudgetExceededError::InrCostExceeded {
                    current: usage.accrued_inr,
                    limit,
                });
            }
        }
        if let Some(limit) = self.max_usd {
            if usage.accrued_usd > limit {
                return Err(BudgetExceededError::UsdCostExceeded {
                    current: usage.accrued_usd,
                    limit,
                });
            }
        }
        if let Some(limit) = self.max_tokens {
            if usage.tokens > limit {
                return Err(BudgetExceededError::TokensExceeded {
                    current: usage.tokens,
                    limit,
                });
            }
        }
        if let Some(limit) = self.max_tool_calls {
            if usage.tool_calls > limit {
                return Err(BudgetExceededError::ToolCallsExceeded {
                    current: usage.tool_calls,
                    limit,
                });
            }
        }
        if let Some(limit) = self.max_data_calls {
            if usage.data_calls > limit {
                return Err(BudgetExceededError::DataCallsExceeded {
                    current: usage.data_calls,
                    limit,
                });
            }
        }
        Ok(())
    }
}

/// Cumulative resource usage of a job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct BudgetUsage {
    /// Number of executed turns.
    pub turns: usize,
    /// Accrued statutory transaction costs and broker fees in INR.
    pub accrued_inr: Decimal,
    /// Accrued fees in USD.
    pub accrued_usd: Decimal,
    /// Total tokens consumed.
    pub tokens: usize,
    /// Total tool invocations made.
    pub tool_calls: usize,
    /// Total data queries made.
    pub data_calls: usize,
    /// Timestamp of last resource usage update.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_updated: Option<DateTime<Utc>>,
}

impl BudgetUsage {
    /// Creates a zeroed usage tracker.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records one turn completion.
    pub fn record_turn(&mut self, now: DateTime<Utc>) {
        self.turns = self.turns.saturating_add(1);
        self.last_updated = Some(now);
    }

    /// Records INR friction cost.
    pub fn record_inr_cost(&mut self, amount: Decimal, now: DateTime<Utc>) {
        self.accrued_inr += amount;
        self.last_updated = Some(now);
    }

    /// Records USD friction cost.
    pub fn record_usd_cost(&mut self, amount: Decimal, now: DateTime<Utc>) {
        self.accrued_usd += amount;
        self.last_updated = Some(now);
    }

    /// Records LLM token consumption.
    pub fn record_tokens(&mut self, count: usize, now: DateTime<Utc>) {
        self.tokens = self.tokens.saturating_add(count);
        self.last_updated = Some(now);
    }

    /// Records tool calls.
    pub fn record_tool_calls(&mut self, count: usize, now: DateTime<Utc>) {
        self.tool_calls = self.tool_calls.saturating_add(count);
        self.last_updated = Some(now);
    }

    /// Records data calls.
    pub fn record_data_calls(&mut self, count: usize, now: DateTime<Utc>) {
        self.data_calls = self.data_calls.saturating_add(count);
        self.last_updated = Some(now);
    }

    /// Merges incremental usage from an execution slice.
    pub fn merge(&mut self, other: &BudgetUsage, now: DateTime<Utc>) {
        self.turns = self.turns.saturating_add(other.turns);
        self.accrued_inr += other.accrued_inr;
        self.accrued_usd += other.accrued_usd;
        self.tokens = self.tokens.saturating_add(other.tokens);
        self.tool_calls = self.tool_calls.saturating_add(other.tool_calls);
        self.data_calls = self.data_calls.saturating_add(other.data_calls);
        self.last_updated = Some(now);
    }
}

/// Reason a budget threshold was exceeded.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum BudgetExceededError {
    #[error("Exceeded turn budget: {current} > {limit}")]
    TurnsExceeded { current: usize, limit: usize },

    #[error("Exceeded INR cost budget: ₹{current} > ₹{limit}")]
    InrCostExceeded { current: Decimal, limit: Decimal },

    #[error("Exceeded USD cost budget: ${current} > ${limit}")]
    UsdCostExceeded { current: Decimal, limit: Decimal },

    #[error("Exceeded token budget: {current} > {limit}")]
    TokensExceeded { current: usize, limit: usize },

    #[error("Exceeded tool call budget: {current} > {limit}")]
    ToolCallsExceeded { current: usize, limit: usize },

    #[error("Exceeded data call budget: {current} > {limit}")]
    DataCallsExceeded { current: usize, limit: usize },
}
