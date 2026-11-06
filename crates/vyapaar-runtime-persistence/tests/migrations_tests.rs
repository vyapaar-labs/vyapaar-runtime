use vyapaar_runtime_persistence::migrations::{MigrationOutcome, connect_and_migrate};

#[tokio::test]
async fn connect_and_migrate_applies_and_rolls_back_sqlite() {
    let dir = std::env::temp_dir().join(format!(
        "vyapaar-persistence-migrate-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db_path = dir.join("vyapaar.db");
    let url = format!("sqlite://{}", db_path.display());

    let first = connect_and_migrate(&url, false).await.unwrap();
    assert_eq!(first, MigrationOutcome::Applied(vec![1, 2, 3, 4, 5, 6]));

    let second = connect_and_migrate(&url, false).await.unwrap();
    assert_eq!(second, MigrationOutcome::Applied(vec![]));

    let rolled = connect_and_migrate(&url, true).await.unwrap();
    assert_eq!(rolled, MigrationOutcome::RolledBack(Some(6)));

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn connect_and_migrate_rejects_unsupported_scheme() {
    let err = connect_and_migrate("mysql://localhost/vyapaar", false)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("unsupported database URL scheme"));
}
