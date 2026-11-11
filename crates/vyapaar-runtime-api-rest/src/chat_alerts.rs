//! Chat-agent price-alert storage over the per-user profile store (`/profile` routes).
//!
//! Storage decision (plan 0006 Phase 2, `vyapaar-runtime/docs/gaps/0004-*.md` profile-schema
//! precedent): alerts are **not** a new store. They live in the existing opaque
//! `Profile.alerts` JSON array that `vyapaar-ui-web`'s `alerts-store.ts` already syncs through
//! the `/profile` routes (that part of gap 0004 closed with no server change). The chat agent
//! is the second writer on the same document: every change here is one atomic read-modify-write
//! via [`ProfileStore::update`] — revision bumped, file rewritten atomically, serialized in-
//! process by the store's write lock. Run one `vyapaar serve` per profiles directory, exactly
//! as the store's own doc requires (ADR 0032 §5).
//!
//! User identity: the chat WebSocket carries no bearer identity yet (see
//! `docs/gaps/0006-chat-agent-session-persistence-and-portfolio-tools.md`), so the alert tools
//! act on [`DEFAULT_USER_ID`]'s profile — the same single-user document the auth-off `/profile`
//! routes serve. When chat sessions gain per-connection tokens, thread the connection's
//! `token_id` through [`ProfileAlertStore::for_user`] instead.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::Value;
use vyapaar_agent_chat_tools::ports::alert::{AlertReader, AlertStatus, AlertWriter, PriceAlert};
use vyapaar_agent_chat_tools::ports::error::PortError;

use crate::profile_store::{DEFAULT_USER_ID, Profile, ProfileStore};

/// Chat-agent price-alert store backed by the per-user profile file.
///
/// Implements the `AlertReader`/`AlertWriter` port over the same `Profile.alerts` document
/// `vyapaar-ui-web` writes; see the module docs for the decision and its limits.
pub struct ProfileAlertStore {
    profiles: Arc<ProfileStore>,
    user_id: String,
}

impl ProfileAlertStore {
    /// Alert store acting on the default user's profile (the auth-off `/profile` document).
    pub fn new(profiles: Arc<ProfileStore>) -> Self {
        Self {
            profiles,
            user_id: DEFAULT_USER_ID.to_owned(),
        }
    }

    /// Alert store acting on `user_id`'s profile — the hook for when chat sessions carry a
    /// validated bearer identity.
    pub fn for_user(profiles: Arc<ProfileStore>, user_id: impl Into<String>) -> Self {
        Self {
            profiles,
            user_id: user_id.into(),
        }
    }

    /// Apply `op` to the stored alert list as one atomic read-modify-write. `op` returns
    /// `(outcome, persist)`; when `persist` is true the mutated list replaces `Profile.alerts`
    /// and the profile revision bumps once, otherwise nothing is written (a no-op change does
    /// not bump the revision, matching `ProfileStore::put`'s debounced-client behavior).
    ///
    /// `ProfileStore` does blocking file I/O (one JSON file per user), so the whole
    /// read-modify-write runs on the blocking pool, exactly like the `/profile` routes.
    async fn with_alerts<T, F>(&self, op: F) -> Result<T, PortError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Vec<PriceAlert>) -> Result<(T, bool), PortError> + Send + 'static,
    {
        let store = Arc::clone(&self.profiles);
        let user_id = self.user_id.clone();
        let joined = tokio::task::spawn_blocking(move || {
            let outcome: Mutex<Option<Result<T, PortError>>> = Mutex::new(None);
            let store_result = store.update(&user_id, |profile| match alerts_of(profile) {
                Err(e) => {
                    *outcome.lock().unwrap() = Some(Err(e));
                    false
                }
                Ok(mut alerts) => match op(&mut alerts) {
                    Ok((value, persist)) => {
                        if persist {
                            match serde_json::to_value(&alerts) {
                                Ok(encoded) => profile.alerts = encoded,
                                Err(e) => {
                                    *outcome.lock().unwrap() = Some(Err(PortError::internal(
                                        format!("serializing alerts failed: {e}"),
                                    )));
                                    return false;
                                }
                            }
                        }
                        *outcome.lock().unwrap() = Some(Ok(value));
                        persist
                    }
                    Err(e) => {
                        *outcome.lock().unwrap() = Some(Err(e));
                        false
                    }
                },
            });
            if let Err(e) = store_result {
                *outcome.lock().unwrap() = Some(Err(PortError::internal(format!(
                    "profile store write failed: {e}"
                ))));
            }
            outcome
        })
        .await;

        joined
            .map_err(|e| PortError::internal(format!("alert store task failed: {e}")))?
            .into_inner()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .unwrap_or_else(|| Err(PortError::internal("alert update produced no outcome")))
    }
}

#[async_trait]
impl AlertReader for ProfileAlertStore {
    async fn list_alerts(&self, workspace: Option<&str>) -> Result<Vec<PriceAlert>, PortError> {
        let workspace = workspace.map(str::to_owned);
        self.with_alerts(move |alerts| {
            let filtered = match workspace.as_deref() {
                Some(w) => alerts
                    .iter()
                    .filter(|a| a.workspace_id == w)
                    .cloned()
                    .collect(),
                None => alerts.clone(),
            };
            Ok((filtered, false))
        })
        .await
    }
}

#[async_trait]
impl AlertWriter for ProfileAlertStore {
    async fn add_alert(&self, alert: PriceAlert) -> Result<PriceAlert, PortError> {
        self.with_alerts(move |alerts| {
            if alerts.iter().any(|a| a.id == alert.id) {
                return Err(PortError::invalid_argument(format!(
                    "an alert with id `{}` already exists",
                    alert.id
                )));
            }
            alerts.push(alert.clone());
            Ok((alert, true))
        })
        .await
    }

    async fn set_alert_status(
        &self,
        id: &str,
        status: AlertStatus,
    ) -> Result<Option<PriceAlert>, PortError> {
        let id = id.to_string();
        self.with_alerts(move |alerts| {
            let Some(alert) = alerts.iter_mut().find(|a| a.id == id) else {
                return Ok((None, false));
            };
            // ui-web parity (`alerts-store.ts`'s toggle): a triggered alert is never flipped,
            // and setting the status it already has changes nothing (no revision bump).
            let changed = alert.status != AlertStatus::Triggered && alert.status != status;
            if changed {
                alert.status = status;
            }
            Ok((Some(alert.clone()), changed))
        })
        .await
    }

    async fn delete_alert(&self, id: &str) -> Result<Option<PriceAlert>, PortError> {
        let id = id.to_string();
        self.with_alerts(move |alerts| {
            let index = alerts.iter().position(|a| a.id == id);
            match index {
                Some(i) => {
                    let removed = alerts.remove(i);
                    Ok((Some(removed), true))
                }
                None => Ok((None, false)),
            }
        })
        .await
    }
}

/// Decode the profile's opaque `alerts` value into the typed list.
///
/// A missing/null value (a default profile) is an empty list. A non-array value is refused
/// rather than silently treated as empty — never fabricate a plausible-looking empty answer
/// (the gap 0007 honest-stub rule). An entry that fails to parse is an error naming the cause;
/// the profile file itself is not quarantined here (it still deserializes as a `Profile`), so
/// the user can fix or reset the field via `PUT /profile`.
fn alerts_of(profile: &Profile) -> Result<Vec<PriceAlert>, PortError> {
    if profile.alerts.is_null() {
        return Ok(Vec::new());
    }
    if !profile.alerts.is_array() {
        return Err(PortError::internal(format!(
            "profile.alerts holds {} where an array of alert rules is expected",
            json_kind(&profile.alerts)
        )));
    }
    serde_json::from_value(profile.alerts.clone()).map_err(|e| {
        PortError::internal(format!(
            "profile.alerts holds an unreadable alert entry: {e}"
        ))
    })
}

/// Human name of a JSON value's kind, for the unexpected-shape error.
fn json_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}
