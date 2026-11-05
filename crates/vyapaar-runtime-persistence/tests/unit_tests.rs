use vyapaar_runtime_persistence::Database;

#[tokio::test]
async fn in_memory_sqlite_database_connects() {
    let db = Database::connect("sqlite::memory:").await.expect("connect");
    assert!(db.is_sqlite());
    db.ping().await.expect("ping");
}
