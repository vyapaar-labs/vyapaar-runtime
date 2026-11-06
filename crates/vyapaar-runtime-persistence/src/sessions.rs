//! Agent chat session and message persistence implementing [`vyapaar_agent_core::ports::SessionStore`].

use crate::{Database, with_executor};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use vyapaar_agent_core::ports::{
    MessageSeq, NewSession, PersistedMessage, RuntimeMode, SessionFilter, SessionId, SessionInfo,
    SessionScope, SessionStore, StoreError,
};

/// SQLx implementation of [`SessionStore`].
#[derive(Debug, Clone)]
pub struct SqlxSessionStore {
    db: Database,
}

impl SqlxSessionStore {
    /// Create a new session store over the given database handle.
    pub fn new(db: &Database) -> Self {
        Self { db: db.clone() }
    }
}

fn mode_to_str(mode: RuntimeMode) -> &'static str {
    match mode {
        RuntimeMode::Live => "live",
        RuntimeMode::Paper => "paper",
        RuntimeMode::Backtest => "backtest",
    }
}

fn sqlx_to_store_error(e: sqlx::Error) -> StoreError {
    match e {
        sqlx::Error::Database(ref db_err) if db_err.is_unique_violation() => {
            StoreError::conflict(db_err.message())
        }
        other => StoreError::unavailable(other.to_string()),
    }
}

enum FilterBind {
    Text(String),
    Int(i32),
}

fn message_from_row<R: sqlx::Row>(row: &R) -> Result<(MessageSeq, PersistedMessage), sqlx::Error>
where
    for<'a> &'a str: sqlx::ColumnIndex<R>,
    i64: sqlx::Type<R::Database> + for<'r> sqlx::Decode<'r, R::Database>,
    String: sqlx::Type<R::Database> + for<'r> sqlx::Decode<'r, R::Database>,
    Option<String>: sqlx::Type<R::Database> + for<'r> sqlx::Decode<'r, R::Database>,
    Option<i64>: sqlx::Type<R::Database> + for<'r> sqlx::Decode<'r, R::Database>,
    DateTime<Utc>: sqlx::Type<R::Database> + for<'r> sqlx::Decode<'r, R::Database>,
{
    let seq: i64 = row.try_get("seq")?;
    let role: String = row.try_get("role")?;
    let content_raw: String = row.try_get("content")?;
    let content: serde_json::Value =
        serde_json::from_str(&content_raw).map_err(|e| sqlx::Error::Decode(Box::new(e)))?;
    let request_hash: Option<String> = row.try_get("request_hash")?;
    let input_tokens: Option<i64> = row.try_get("input_tokens")?;
    let output_tokens: Option<i64> = row.try_get("output_tokens")?;
    let created_at: DateTime<Utc> = row.try_get("created_at")?;

    let seq_u64 = seq as MessageSeq;
    Ok((
        seq_u64,
        PersistedMessage {
            seq: seq_u64,
            role,
            content,
            request_hash,
            input_tokens: input_tokens.map(|t| t as u32),
            output_tokens: output_tokens.map(|t| t as u32),
            created_at,
        },
    ))
}

fn session_info_from_row<R: sqlx::Row>(row: &R) -> Result<SessionInfo, sqlx::Error>
where
    for<'a> &'a str: sqlx::ColumnIndex<R>,
    String: sqlx::Type<R::Database> + for<'r> sqlx::Decode<'r, R::Database>,
    Option<String>: sqlx::Type<R::Database> + for<'r> sqlx::Decode<'r, R::Database>,
    i32: sqlx::Type<R::Database> + for<'r> sqlx::Decode<'r, R::Database>,
    DateTime<Utc>: sqlx::Type<R::Database> + for<'r> sqlx::Decode<'r, R::Database>,
{
    let id_str: String = row.try_get("id")?;
    let id: SessionId = id_str
        .parse()
        .map_err(|e| sqlx::Error::Decode(Box::new(e)))?;
    let profile: String = row.try_get("profile")?;
    let mode_raw: String = row.try_get("mode")?;
    let mode = match mode_raw.as_str() {
        "live" => RuntimeMode::Live,
        "paper" => RuntimeMode::Paper,
        "backtest" => RuntimeMode::Backtest,
        other => return Err(sqlx::Error::Decode(format!("invalid mode {other}").into())),
    };
    let title: Option<String> = row.try_get("title")?;
    let provider: String = row.try_get("provider")?;
    let model: String = row.try_get("model")?;
    let archived_int: i32 = row.try_get("archived")?;
    let created_at: DateTime<Utc> = row.try_get("created_at")?;
    let updated_at: DateTime<Utc> = row.try_get("updated_at")?;

    Ok(SessionInfo {
        id,
        profile,
        mode,
        provider,
        model,
        title,
        archived: archived_int != 0,
        created_at,
        updated_at,
    })
}

#[async_trait]
impl SessionStore for SqlxSessionStore {
    async fn create(&self, s: NewSession) -> Result<SessionId, StoreError> {
        let now = Utc::now();
        let mode_str = mode_to_str(s.mode);
        let id_str = s.id.to_string();

        with_executor!(&self.db, |ex| {
            sqlx::query(
                "INSERT INTO agent_chat_session (
                     id, profile, mode, title, provider, model, archived, created_at, updated_at
                 ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
            )
            .bind(&id_str)
            .bind(&s.profile)
            .bind(mode_str)
            .bind(&s.title)
            .bind(&s.provider)
            .bind(&s.model)
            .bind(0i32)
            .bind(now)
            .bind(now)
            .execute(ex)
            .await
            .map(|_| ())
            .map_err(sqlx_to_store_error)
        })?;

        Ok(s.id)
    }

    async fn append(
        &self,
        id: SessionId,
        msgs: &[PersistedMessage],
    ) -> Result<Vec<MessageSeq>, StoreError> {
        if msgs.is_empty() {
            return Ok(Vec::new());
        }

        let id_str = id.to_string();
        let now = Utc::now();

        with_executor!(&self.db, |ex| {
            async {
                let mut tx = ex.begin().await.map_err(sqlx_to_store_error)?;

                for msg in msgs {
                    let content_str = serde_json::to_string(&msg.content).map_err(|e| {
                        StoreError::corrupt(format!("failed to serialize content: {e}"))
                    })?;
                    let seq_i64 = msg.seq as i64;
                    let input_tokens = msg.input_tokens.map(|t| t as i64);
                    let output_tokens = msg.output_tokens.map(|t| t as i64);

                    sqlx::query(
                        "INSERT INTO agent_chat_message (
                             session_id, seq, role, content, request_hash, input_tokens, output_tokens, created_at
                         ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
                    )
                    .bind(&id_str)
                    .bind(seq_i64)
                    .bind(&msg.role)
                    .bind(&content_str)
                    .bind(&msg.request_hash)
                    .bind(input_tokens)
                    .bind(output_tokens)
                    .bind(msg.created_at)
                    .execute(&mut *tx)
                    .await
                    .map_err(sqlx_to_store_error)?;
                }

                let update_res =
                    sqlx::query("UPDATE agent_chat_session SET updated_at = $1 WHERE id = $2")
                        .bind(now)
                        .bind(&id_str)
                        .execute(&mut *tx)
                        .await
                        .map_err(sqlx_to_store_error)?;

                if update_res.rows_affected() == 0 {
                    return Err(StoreError::not_found(format!("session {id} not found")));
                }

                tx.commit().await.map_err(sqlx_to_store_error)?;
                Ok::<(), StoreError>(())
            }
            .await
        })?;

        Ok(msgs.iter().map(|m| m.seq).collect())
    }

    async fn messages(
        &self,
        id: SessionId,
    ) -> Result<Vec<(MessageSeq, PersistedMessage)>, StoreError> {
        let id_str = id.to_string();

        let msgs = with_executor!(&self.db, |ex| {
            async {
                let rows = sqlx::query(
                    "SELECT seq, role, content, request_hash, input_tokens, output_tokens, created_at
                     FROM agent_chat_message
                     WHERE session_id = $1
                     ORDER BY seq ASC",
                )
                .bind(&id_str)
                .fetch_all(ex)
                .await
                .map_err(sqlx_to_store_error)?;

                rows.iter()
                    .map(message_from_row)
                    .collect::<Result<Vec<_>, sqlx::Error>>()
                    .map_err(sqlx_to_store_error)
            }
            .await
        })?;

        Ok(msgs)
    }

    async fn most_recent(&self, scope: &SessionScope) -> Result<Option<SessionInfo>, StoreError> {
        let mode_str = mode_to_str(scope.mode);

        let info = with_executor!(&self.db, |ex| {
            async {
                let opt = sqlx::query(
                    "SELECT id, profile, mode, title, provider, model, archived, created_at, updated_at
                     FROM agent_chat_session
                     WHERE profile = $1 AND mode = $2 AND archived = 0
                     ORDER BY updated_at DESC
                     LIMIT 1",
                )
                .bind(&scope.profile)
                .bind(mode_str)
                .fetch_optional(ex)
                .await
                .map_err(sqlx_to_store_error)?;

                opt.as_ref()
                    .map(session_info_from_row)
                    .transpose()
                    .map_err(sqlx_to_store_error)
            }
            .await
        })?;

        Ok(info)
    }

    async fn list(&self, filter: SessionFilter) -> Result<Vec<SessionInfo>, StoreError> {
        let mut sql = String::from(
            "SELECT id, profile, mode, title, provider, model, archived, created_at, updated_at
             FROM agent_chat_session WHERE 1=1",
        );
        let mut binds: Vec<FilterBind> = Vec::new();
        let mut idx = 1;

        if let Some(ref profile) = filter.profile {
            sql.push_str(&format!(" AND profile = ${idx}"));
            binds.push(FilterBind::Text(profile.clone()));
            idx += 1;
        }

        if let Some(mode) = filter.mode {
            sql.push_str(&format!(" AND mode = ${idx}"));
            binds.push(FilterBind::Text(mode_to_str(mode).to_string()));
            idx += 1;
        }

        if let Some(archived) = filter.archived {
            let archived_val = if archived { 1 } else { 0 };
            sql.push_str(&format!(" AND archived = ${idx}"));
            binds.push(FilterBind::Int(archived_val));
            idx += 1;
        }

        sql.push_str(" ORDER BY updated_at DESC");

        let limit = filter.limit.unwrap_or(50) as i64;
        sql.push_str(&format!(" LIMIT ${idx}"));

        let list = with_executor!(&self.db, |ex| {
            async {
                let mut q = sqlx::query(&sql);
                for b in &binds {
                    match b {
                        FilterBind::Text(s) => q = q.bind(s),
                        FilterBind::Int(i) => q = q.bind(*i),
                    }
                }
                q = q.bind(limit);

                let rows = q.fetch_all(ex).await.map_err(sqlx_to_store_error)?;
                rows.iter()
                    .map(session_info_from_row)
                    .collect::<Result<Vec<_>, sqlx::Error>>()
                    .map_err(sqlx_to_store_error)
            }
            .await
        })?;

        Ok(list)
    }

    async fn rename(&self, id: SessionId, title: &str) -> Result<(), StoreError> {
        let id_str = id.to_string();
        let now = Utc::now();

        let rows_affected = with_executor!(&self.db, |ex| {
            sqlx::query("UPDATE agent_chat_session SET title = $1, updated_at = $2 WHERE id = $3")
                .bind(title)
                .bind(now)
                .bind(&id_str)
                .execute(ex)
                .await
                .map(|res| res.rows_affected())
                .map_err(sqlx_to_store_error)
        })?;

        if rows_affected == 0 {
            Err(StoreError::not_found(format!("session {id} not found")))
        } else {
            Ok(())
        }
    }

    async fn update_provider(
        &self,
        id: SessionId,
        provider: &str,
        model: &str,
    ) -> Result<(), StoreError> {
        let id_str = id.to_string();
        let now = Utc::now();

        let rows_affected = with_executor!(&self.db, |ex| {
            sqlx::query(
                "UPDATE agent_chat_session SET provider = $1, model = $2, updated_at = $3 WHERE id = $4",
            )
            .bind(provider)
            .bind(model)
            .bind(now)
            .bind(&id_str)
            .execute(ex)
            .await
            .map(|res| res.rows_affected())
            .map_err(sqlx_to_store_error)
        })?;

        if rows_affected == 0 {
            Err(StoreError::not_found(format!("session {id} not found")))
        } else {
            Ok(())
        }
    }

    async fn truncate_after(&self, id: SessionId, seq: MessageSeq) -> Result<(), StoreError> {
        let id_str = id.to_string();
        let seq_i64 = seq as i64;
        let now = Utc::now();

        with_executor!(&self.db, |ex| {
            async {
                let mut tx = ex.begin().await.map_err(sqlx_to_store_error)?;

                sqlx::query("DELETE FROM agent_chat_message WHERE session_id = $1 AND seq > $2")
                    .bind(&id_str)
                    .bind(seq_i64)
                    .execute(&mut *tx)
                    .await
                    .map_err(sqlx_to_store_error)?;

                let update_res =
                    sqlx::query("UPDATE agent_chat_session SET updated_at = $1 WHERE id = $2")
                        .bind(now)
                        .bind(&id_str)
                        .execute(&mut *tx)
                        .await
                        .map_err(sqlx_to_store_error)?;

                if update_res.rows_affected() == 0 {
                    return Err(StoreError::not_found(format!("session {id} not found")));
                }

                tx.commit().await.map_err(sqlx_to_store_error)?;
                Ok::<(), StoreError>(())
            }
            .await
        })
    }
}
