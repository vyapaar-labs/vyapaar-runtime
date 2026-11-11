//! `ProfileAlertStore` — the chat agent's price-alert storage over the per-user profile
//! document (agent plan 0006 Phase 2; the `docs/gaps/0004-*.md` profile-schema precedent's
//! "alerts already fit `Profile.alerts`" decision, made concrete and tested).

use std::sync::Arc;

use rust_decimal::Decimal;
use serde_json::Value;
use vyapaar_agent_chat_tools::ports::alert::{AlertReader, AlertStatus, AlertWriter, PriceAlert};
use vyapaar_agent_chat_tools::ports::error::PortErrorKind;
use vyapaar_core::Price;
use vyapaar_runtime_api_rest::chat_alerts::ProfileAlertStore;
use vyapaar_runtime_api_rest::{DEFAULT_USER_ID, Profile, ProfileStore};

/// A fixed alert, fully formed the way the chat tool's `add` operation builds one.
fn sample_alert(id: &str, workspace: &str) -> PriceAlert {
    PriceAlert {
        id: id.to_string(),
        workspace_id: workspace.to_string(),
        instrument_id: "NSE:RELIANCE".to_string(),
        price: Price::new(Decimal::new(250050, 2)),
        status: AlertStatus::Active,
        created_at: "2026-11-11".to_string(),
        triggered_at: None,
    }
}

fn profiles_dir(dir: &std::path::Path) -> Arc<ProfileStore> {
    Arc::new(ProfileStore::new(dir.join("profiles")))
}

#[tokio::test]
async fn add_then_list_roundtrips_through_the_profile_file() {
    let dir = tempfile::tempdir().unwrap();
    let store = ProfileAlertStore::new(profiles_dir(dir.path()));

    let created = store.add_alert(sample_alert("al_1", "ws-a")).await.unwrap();
    assert_eq!(created, sample_alert("al_1", "ws-a"));

    let listed = store.list_alerts(None).await.unwrap();
    assert_eq!(listed, vec![sample_alert("al_1", "ws-a")]);

    // Workspace scoping filters on the alert's own workspace id.
    let other = store.list_alerts(Some("ws-b")).await.unwrap();
    assert!(other.is_empty());

    // A fresh store over the same directory reads the same file: the alert survives the
    // process boundary the way every ui-web-written alert already does.
    let reopened = ProfileAlertStore::new(profiles_dir(dir.path()));
    assert_eq!(
        reopened.list_alerts(Some("ws-a")).await.unwrap(),
        vec![sample_alert("al_1", "ws-a")]
    );
}

#[tokio::test]
async fn stored_json_keeps_the_camel_case_alert_meta_shape_ui_web_reads() {
    let dir = tempfile::tempdir().unwrap();
    let store = ProfileAlertStore::new(profiles_dir(dir.path()));
    store.add_alert(sample_alert("al_1", "ws-a")).await.unwrap();

    let profile = profiles_dir(dir.path()).get(DEFAULT_USER_ID).unwrap();
    let alert = &profile.alerts.as_array().unwrap()[0];
    // Exact field names and value shapes `alerts-store.ts`'s `isAlertMeta` checks.
    assert_eq!(alert["id"], "al_1");
    assert_eq!(alert["workspaceId"], "ws-a");
    assert_eq!(alert["instrumentId"], "NSE:RELIANCE");
    assert_eq!(alert["price"], "2500.50");
    assert_eq!(alert["status"], "active");
    assert_eq!(alert["createdAt"], "2026-11-11");
    assert_eq!(alert["triggeredAt"], Value::Null);
}

#[tokio::test]
async fn duplicate_ids_are_rejected_not_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    let store = ProfileAlertStore::new(profiles_dir(dir.path()));
    store.add_alert(sample_alert("al_1", "ws-a")).await.unwrap();

    let err = store
        .add_alert(sample_alert("al_1", "ws-a"))
        .await
        .unwrap_err();
    assert_eq!(err.kind, PortErrorKind::InvalidArgument);
    // The first alert is untouched.
    assert_eq!(store.list_alerts(None).await.unwrap().len(), 1);
}

#[tokio::test]
async fn set_status_transitions_match_ui_web_semantics() {
    let dir = tempfile::tempdir().unwrap();
    let profiles = profiles_dir(dir.path());
    let store = ProfileAlertStore::new(Arc::clone(&profiles));
    store.add_alert(sample_alert("al_1", "ws-a")).await.unwrap();

    // active -> paused -> active, each bumping the revision once.
    let paused = store
        .set_alert_status("al_1", AlertStatus::Paused)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(paused.status, AlertStatus::Paused);
    let active = store
        .set_alert_status("al_1", AlertStatus::Active)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(active.status, AlertStatus::Active);

    // A triggered alert is never flipped by enable/disable (ui-web toggle parity), and a
    // no-op change writes nothing (revision unchanged, matching ProfileStore::put).
    store
        .set_alert_status("al_1", AlertStatus::Paused)
        .await
        .unwrap()
        .unwrap();
    {
        let mut profile = profiles.get(DEFAULT_USER_ID).unwrap();
        profile.alerts[0]["status"] = "triggered".into();
        profile.alerts[0]["triggeredAt"] = "2026-11-11T10:00:00Z".into();
        profiles
            .put(DEFAULT_USER_ID, profile.revision, profile)
            .unwrap();
    }
    let revision = profiles.get(DEFAULT_USER_ID).unwrap().revision;
    let unchanged = store
        .set_alert_status("al_1", AlertStatus::Active)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(unchanged.status, AlertStatus::Triggered);
    assert!(unchanged.triggered_at.is_some());
    assert_eq!(profiles.get(DEFAULT_USER_ID).unwrap().revision, revision);

    // Unknown id → None, not an error.
    assert!(
        store
            .set_alert_status("nope", AlertStatus::Active)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn delete_removes_the_alert_and_reports_unknown_ids() {
    let dir = tempfile::tempdir().unwrap();
    let store = ProfileAlertStore::new(profiles_dir(dir.path()));
    store.add_alert(sample_alert("al_1", "ws-a")).await.unwrap();
    store.add_alert(sample_alert("al_2", "ws-a")).await.unwrap();

    let removed = store.delete_alert("al_1").await.unwrap().unwrap();
    assert_eq!(removed.id, "al_1");
    let remaining = store.list_alerts(None).await.unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].id, "al_2");

    assert!(store.delete_alert("nope").await.unwrap().is_none());
}

#[tokio::test]
async fn null_alerts_value_means_an_empty_list_not_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let store = ProfileAlertStore::new(profiles_dir(dir.path()));
    // A default profile carries `alerts: null`.
    let listed = store.list_alerts(None).await.unwrap();
    assert!(listed.is_empty());
}

#[tokio::test]
async fn unexpected_alerts_shape_is_an_explicit_error() {
    let dir = tempfile::tempdir().unwrap();
    let profiles = profiles_dir(dir.path());
    profiles
        .put(
            DEFAULT_USER_ID,
            0,
            Profile {
                alerts: serde_json::json!({ "legacy": true }),
                ..Default::default()
            },
        )
        .unwrap();

    let store = ProfileAlertStore::new(profiles);
    let err = store.list_alerts(None).await.unwrap_err();
    assert_eq!(err.kind, PortErrorKind::Internal);
    assert!(
        err.message.contains("expected"),
        "must say the value was unexpected, got: {err}"
    );
}

#[tokio::test]
async fn unreadable_alert_entry_is_an_error_naming_the_cause() {
    let dir = tempfile::tempdir().unwrap();
    let profiles = profiles_dir(dir.path());
    profiles
        .put(
            DEFAULT_USER_ID,
            0,
            Profile {
                alerts: serde_json::json!([{ "id": "al_1", "price": "not-a-price" }]),
                ..Default::default()
            },
        )
        .unwrap();

    let store = ProfileAlertStore::new(profiles);
    let err = store.list_alerts(None).await.unwrap_err();
    assert_eq!(err.kind, PortErrorKind::Internal);
    assert!(
        err.message.contains("unreadable alert entry"),
        "must name the unreadable entry, got: {err}"
    );
}

#[tokio::test]
async fn alert_writes_bump_the_revision_and_leave_other_profile_fields_intact() {
    let dir = tempfile::tempdir().unwrap();
    let profiles = profiles_dir(dir.path());
    profiles
        .put(
            DEFAULT_USER_ID,
            0,
            Profile {
                watchlists: std::collections::BTreeMap::from([(
                    "core".to_string(),
                    vec!["NSE:TCS".to_string()],
                )]),
                preferences: serde_json::json!({ "theme": "dark" }),
                ..Default::default()
            },
        )
        .unwrap();

    let store = ProfileAlertStore::new(Arc::clone(&profiles));
    store.add_alert(sample_alert("al_1", "ws-a")).await.unwrap();

    let profile = profiles.get(DEFAULT_USER_ID).unwrap();
    assert_eq!(
        profile.revision, 2,
        "one bump for the seed put, one for the alert"
    );
    assert_eq!(
        profile.watchlists.get("core").map(Vec::as_slice),
        Some(&["NSE:TCS".to_string()][..])
    );
    assert_eq!(profile.preferences["theme"], "dark");
    assert_eq!(profile.alerts.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn for_user_scopes_the_store_to_a_specific_profile_document() {
    let dir = tempfile::tempdir().unwrap();
    let profiles = profiles_dir(dir.path());
    let alice = ProfileAlertStore::for_user(Arc::clone(&profiles), "alice");
    let bob = ProfileAlertStore::for_user(Arc::clone(&profiles), "bob");

    alice.add_alert(sample_alert("al_1", "ws-a")).await.unwrap();
    assert!(bob.list_alerts(None).await.unwrap().is_empty());
    bob.add_alert(sample_alert("al_2", "ws-a")).await.unwrap();

    let alice_alerts = profiles_dir(dir.path())
        .get("alice")
        .unwrap()
        .alerts
        .as_array()
        .unwrap()
        .len();
    let bob_alerts = profiles_dir(dir.path())
        .get("bob")
        .unwrap()
        .alerts
        .as_array()
        .unwrap()
        .len();
    assert_eq!(alice_alerts, 1);
    assert_eq!(bob_alerts, 1);
}
