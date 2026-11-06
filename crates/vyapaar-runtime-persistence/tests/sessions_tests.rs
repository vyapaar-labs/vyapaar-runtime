use chrono::Utc;
use vyapaar_agent_core::ports::{
    NewSession, PersistedMessage, RuntimeMode, SessionFilter, SessionId, SessionScope, SessionStore,
};
use vyapaar_runtime_persistence::{Database, SqlxSessionStore, migrate};

async fn setup_db() -> Database {
    let db = Database::connect("sqlite::memory:")
        .await
        .expect("connect sqlite::memory:");
    migrate(&db).await.expect("migrate");
    db
}

#[tokio::test]
async fn test_session_lifecycle_and_messages() {
    let db = setup_db().await;
    let store = SqlxSessionStore::new(&db);

    let session_id = SessionId::new();
    let new_session = NewSession {
        id: session_id,
        profile: "default".to_string(),
        mode: RuntimeMode::Paper,
        provider: "anthropic".to_string(),
        model: "claude-sonnet-3-7".to_string(),
        title: Some("Initial Exploration".to_string()),
    };

    // 1. Create session
    let created_id = store
        .create(new_session.clone())
        .await
        .expect("create session");
    assert_eq!(created_id, session_id);

    // 2. Duplicate create fails with conflict
    let conflict_err = store.create(new_session).await.unwrap_err();
    assert_eq!(
        conflict_err.kind,
        vyapaar_agent_core::ports::StoreErrorKind::Conflict
    );

    // 3. Most recent session lookup
    let scope = SessionScope::new("default", RuntimeMode::Paper);
    let recent = store
        .most_recent(&scope)
        .await
        .expect("most_recent")
        .expect("some session");
    assert_eq!(recent.id, session_id);
    assert_eq!(recent.title.as_deref(), Some("Initial Exploration"));

    // 4. Append messages
    let msg1 = PersistedMessage {
        seq: 1,
        role: "user".to_string(),
        content: serde_json::json!({"text": "Hello, how is my portfolio?"}),
        request_hash: None,
        input_tokens: None,
        output_tokens: None,
        created_at: Utc::now(),
    };
    let msg2 = PersistedMessage {
        seq: 2,
        role: "assistant".to_string(),
        content: serde_json::json!({"text": "Your portfolio is flat today."}),
        request_hash: Some("sha256-dummy-hash".to_string()),
        input_tokens: Some(150),
        output_tokens: Some(40),
        created_at: Utc::now(),
    };

    let appended_seqs = store
        .append(session_id, &[msg1.clone(), msg2.clone()])
        .await
        .expect("append messages");
    assert_eq!(appended_seqs, vec![1, 2]);

    // 5. Fetch messages
    let msgs = store.messages(session_id).await.expect("fetch messages");
    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[0].0, 1);
    assert_eq!(msgs[0].1.role, "user");
    assert_eq!(msgs[0].1.content, msg1.content);
    assert_eq!(msgs[1].0, 2);
    assert_eq!(msgs[1].1.role, "assistant");
    assert_eq!(msgs[1].1.request_hash.as_deref(), Some("sha256-dummy-hash"));
    assert_eq!(msgs[1].1.input_tokens, Some(150));
    assert_eq!(msgs[1].1.output_tokens, Some(40));

    // 6. Rename session
    store
        .rename(session_id, "Portfolio Review")
        .await
        .expect("rename");
    let updated_info = store
        .most_recent(&scope)
        .await
        .expect("most_recent")
        .expect("some session");
    assert_eq!(updated_info.title.as_deref(), Some("Portfolio Review"));

    // 7. Update provider
    store
        .update_provider(session_id, "openai", "gpt-4o")
        .await
        .expect("update_provider");
    let updated_provider = store
        .most_recent(&scope)
        .await
        .expect("most_recent")
        .expect("some session");
    assert_eq!(updated_provider.provider, "openai");
    assert_eq!(updated_provider.model, "gpt-4o");

    // 8. List sessions
    let list = store
        .list(SessionFilter {
            profile: Some("default".to_string()),
            mode: Some(RuntimeMode::Paper),
            archived: Some(false),
            limit: Some(10),
        })
        .await
        .expect("list sessions");
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, session_id);

    // 9. Truncate after seq 1
    store.truncate_after(session_id, 1).await.expect("truncate");
    let truncated_msgs = store
        .messages(session_id)
        .await
        .expect("fetch after truncate");
    assert_eq!(truncated_msgs.len(), 1);
    assert_eq!(truncated_msgs[0].0, 1);
}
