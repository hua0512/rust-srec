use super::*;

async fn fixture(users: &[(&str, Option<&str>)]) -> (SqlitePool, ConfigExport) {
    let pool = init_pool_with_size("sqlite::memory:", 1).await.unwrap();
    run_migrations(&pool).await.unwrap();
    let global = sqlx::query_as("SELECT * FROM global_config ORDER BY rowid LIMIT 1")
        .fetch_one(&pool)
        .await
        .unwrap();
    let mut tx = begin_immediate(&pool).await.unwrap();
    for (name, email) in users {
        let mut user = UserDbModel::new(*name, VALID_PASSWORD_HASH, vec!["admin".to_string()]);
        user.email = email.map(str::to_string);
        persist_user(&mut tx, &user).await.unwrap();
    }
    tx.commit().await.unwrap();
    let mut config = import_config(&global);
    // Replace resolves engine references from the bundle, including the global default.
    let engines: Vec<EngineConfigurationDbModel> =
        sqlx::query_as("SELECT * FROM engine_configuration")
            .fetch_all(&pool)
            .await
            .unwrap();
    config.engines = engines
        .into_iter()
        .map(|engine| crate::config::backup::EngineExport {
            name: engine.name,
            engine_type: engine.engine_type,
            config: serde_json::from_str(&engine.config).unwrap(),
        })
        .collect();
    (pool, config)
}

fn user(name: &str, email: Option<&str>) -> UserExport {
    let mut user = imported_user(name, vec!["admin".to_string()], true);
    user.email = email.map(str::to_string);
    user
}

async fn checked_import(
    pool: &SqlitePool,
    config: &ConfigExport,
    mode: ImportMode,
) -> Result<(), ConfigurationImportError> {
    validate_import(config, mode)?;
    let mut tx = begin_immediate(pool).await?;
    let snapshot = ImportSnapshot::load(&mut tx).await?;
    snapshot.validate_references(config, mode)?;
    apply_import(&mut tx, &snapshot, config, mode).await?;
    tx.commit().await?;
    Ok(())
}

async fn users(pool: &SqlitePool) -> Vec<UserDbModel> {
    sqlx::query_as("SELECT * FROM users ORDER BY username")
        .fetch_all(pool)
        .await
        .unwrap()
}

async fn assert_unchanged(pool: &SqlitePool, before: &[UserDbModel], output: &str) {
    assert_eq!(
        serde_json::to_value(users(pool).await).unwrap(),
        serde_json::to_value(before).unwrap()
    );
    let stored: String = sqlx::query_scalar("SELECT output_folder FROM global_config LIMIT 1")
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(stored, output);
}

#[tokio::test]
async fn retained_email_collision_is_validation_before_any_writes() {
    for email in ["private@example.com", ""] {
        let (pool, mut config) = fixture(&[("retained", Some(email))]).await;
        let before = users(&pool).await;
        let output = config.global_config.output_folder.clone();
        config.global_config.output_folder = "/must-not-write".to_string();
        config.users = vec![user("incoming", Some(email))];
        // A write trap distinguishes validation from a database failure/rollback.
        sqlx::query("CREATE TRIGGER reject_global_write BEFORE UPDATE ON global_config BEGIN SELECT RAISE(ABORT, 'unexpected write'); END")
            .execute(&pool).await.unwrap();
        let error = checked_import(&pool, &config, ImportMode::Merge)
            .await
            .unwrap_err();
        assert!(matches!(error, ConfigurationImportError::Validation(_)));
        assert!(error.to_string().contains("email already assigned"));
        assert!(!error.to_string().contains("retained"));
        assert_unchanged(&pool, &before, &output).await;
    }
}

#[tokio::test]
async fn swaps_and_reassignment_are_order_independent_and_preserve_identity() {
    for reverse in [false, true] {
        for reassignment in [false, true] {
            let (pool, mut config) = fixture(&[("alice", Some("a")), ("bob", Some("b"))]).await;
            let before = users(&pool).await;
            config.users = if reassignment {
                vec![user("alice", None), user("new", Some("a"))]
            } else {
                vec![user("alice", Some("b")), user("bob", Some("a"))]
            };
            if reverse {
                config.users.reverse();
            }
            checked_import(&pool, &config, ImportMode::Merge)
                .await
                .unwrap();
            let after = users(&pool).await;
            for original in before {
                let actual = after
                    .iter()
                    .find(|u| u.username == original.username)
                    .unwrap();
                assert_eq!(actual.id, original.id);
                assert_eq!(actual.created_at, original.created_at);
            }
            for imported in &config.users {
                assert_eq!(
                    after
                        .iter()
                        .find(|u| u.username == imported.username)
                        .unwrap()
                        .email,
                    imported.email
                );
            }
        }
    }
}

#[tokio::test]
async fn replace_can_reassign_deleted_users_email_and_existing_user_keeps_own_email() {
    let (pool, mut config) = fixture(&[("old", Some("same"))]).await;
    config.users = vec![user("old", Some("same"))];
    let original_id = users(&pool)
        .await
        .into_iter()
        .find(|user| user.username == "old")
        .unwrap()
        .id;
    checked_import(&pool, &config, ImportMode::Merge)
        .await
        .unwrap();
    assert_eq!(
        users(&pool)
            .await
            .into_iter()
            .find(|user| user.username == "old")
            .unwrap()
            .id,
        original_id
    );
    config.users = vec![user("replacement", Some("same"))];
    checked_import(&pool, &config, ImportMode::Replace)
        .await
        .unwrap();
    let after = users(&pool).await;
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].username, "replacement");
    assert_eq!(after[0].email.as_deref(), Some("same"));
}

#[tokio::test]
async fn email_validation_matches_sqlite_binary_and_null_semantics() {
    let (pool, mut config) = fixture(&[("retained", Some("Email"))]).await;
    let before = users(&pool).await;
    sqlx::query("CREATE TRIGGER protect_retained_email BEFORE UPDATE OF email ON users WHEN OLD.username = 'retained' BEGIN SELECT RAISE(ABORT, 'retained email touched'); END")
        .execute(&pool).await.unwrap();
    config.users = vec![
        user("lower", Some("email")),
        user("upper", Some("EMAIL")),
        user("space", Some(" email")),
        user("empty", Some("")),
        user("null1", None),
        user("null2", None),
    ];
    checked_import(&pool, &config, ImportMode::Merge)
        .await
        .unwrap();
    let after = users(&pool).await;
    assert_eq!(after.len(), before.len() + config.users.len());
    for expected in &config.users {
        let actual = after
            .iter()
            .find(|user| user.username == expected.username)
            .unwrap();
        assert_eq!(actual.email, expected.email);
    }
    for expected in &before {
        let actual = after
            .iter()
            .find(|user| user.username == expected.username)
            .unwrap();
        assert_eq!(actual.id, expected.id);
        assert_eq!(actual.email, expected.email);
    }
    config.users = vec![
        user("duplicate1", Some("same")),
        user("duplicate2", Some("same")),
    ];
    assert!(matches!(
        checked_import(&pool, &config, ImportMode::Merge).await,
        Err(ConfigurationImportError::Validation(_))
    ));
}

#[tokio::test]
async fn email_reassignment_does_not_override_existing_user_id_conflicts() {
    for mode in [ImportMode::Merge, ImportMode::Replace] {
        let (pool, mut config) = fixture(&[("old", Some("email"))]).await;
        let before = users(&pool).await;
        let mut replacement = user("new", Some("email"));
        replacement.id = before
            .iter()
            .find(|user| user.username == "old")
            .unwrap()
            .id
            .clone();
        config.users = vec![replacement];
        let error = checked_import(&pool, &config, mode).await.unwrap_err();
        assert!(matches!(error, ConfigurationImportError::Validation(_)));
        assert!(error.to_string().contains("User id"));
        assert_unchanged(&pool, &before, &config.global_config.output_folder).await;
    }
}

#[tokio::test]
async fn omitted_and_legacy_ignored_users_keep_email_slots() {
    for mode in [ImportMode::Merge, ImportMode::Replace] {
        let (pool, mut config) = fixture(&[("retained", Some("same"))]).await;
        let before = users(&pool).await;
        checked_import(&pool, &config, mode).await.unwrap();
        config.version = "0.1.2".to_string();
        config.users = vec![user("ignored", Some("same"))];
        checked_import(&pool, &config, mode).await.unwrap();
        assert_unchanged(&pool, &before, &config.global_config.output_folder).await;
    }
}

#[tokio::test]
async fn later_user_failure_rolls_back_email_slot_clearing_and_all_prior_writes() {
    for mode in [ImportMode::Merge, ImportMode::Replace] {
        let (pool, mut config) = fixture(&[("alice", Some("a")), ("bob", Some("b"))]).await;
        let before = users(&pool).await;
        let output = config.global_config.output_folder.clone();
        config.global_config.output_folder = "/must-roll-back".to_string();
        config.users = vec![
            user("alice", Some("b")),
            user("bob", Some("a")),
            user("broken", None),
        ];
        sqlx::query("CREATE TRIGGER reject_broken_user BEFORE INSERT ON users WHEN NEW.username = 'broken' BEGIN SELECT RAISE(ABORT, 'forced late failure'); END")
            .execute(&pool).await.unwrap();
        let error = checked_import(&pool, &config, mode).await.unwrap_err();
        assert!(matches!(error, ConfigurationImportError::Database(_)));
        assert!(error.to_string().contains("forced late failure"));
        assert_unchanged(&pool, &before, &output).await;
    }
}

#[test]
fn import_openapi_documents_unconditional_refresh_revocation() {
    use utoipa::OpenApi;
    let document = serde_json::to_value(crate::api::openapi::ApiDoc::openapi()).unwrap();
    let description = document["paths"]["/api/config/backup/import"]["post"]["description"]
        .as_str()
        .unwrap();
    assert!(description.contains("all refresh tokens"));
    assert!(description.contains("omit users"));
}
