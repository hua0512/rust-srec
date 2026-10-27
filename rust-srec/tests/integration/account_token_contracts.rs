//! Public query projections and storage contracts, independent of rotation policy.

use serde::Serialize;
use sqlx::{SqlitePool, sqlite::SqlitePoolOptions};

use rust_srec::database::models::{
    ApiKeyAccessLevel, ApiKeyDbModel, RefreshTokenDbModel, UserDbModel,
};
use rust_srec::database::repositories::{
    ApiKeyRepository, RefreshTokenRepository, SqlxApiKeyRepository, SqlxRefreshTokenRepository,
    SqlxUserRepository, UserRepository,
};
use rust_srec::database::time::now_ms;

const CREATED: i64 = 1_788_784_496_123;

async fn database() -> SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::migrate!("./migrations").run(&pool).await.unwrap();
    sqlx::query("DELETE FROM users")
        .execute(&pool)
        .await
        .unwrap();
    pool
}

fn user(id: &str, created_at: i64) -> UserDbModel {
    UserDbModel {
        id: id.into(),
        username: format!("name-{id}"),
        password_hash: format!("hash-{id}"),
        email: Some(format!("{id}@example.test")),
        roles: r#"["admin","viewer"]"#.into(),
        is_active: true,
        must_change_password: true,
        last_login_at: Some(created_at - 321),
        created_at,
        updated_at: created_at + 123,
    }
}

fn same<T: Serialize>(actual: &T, expected: &T) {
    assert_eq!(
        serde_json::to_value(actual).unwrap(),
        serde_json::to_value(expected).unwrap()
    );
}

async fn account_storage(pool: &SqlitePool) {
    for (table, columns) in [
        ("users", &["created_at", "updated_at", "last_login_at"][..]),
        (
            "refresh_tokens",
            &["created_at", "expires_at", "revoked_at"][..],
        ),
        (
            "auth_sessions",
            &["created_at", "expires_at", "revoked_at"][..],
        ),
        (
            "api_keys",
            &["created_at", "expires_at", "last_used_at", "revoked_at"][..],
        ),
    ] {
        for column in columns {
            let types: Vec<String> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
                "SELECT typeof({column}) FROM {table} WHERE {column} IS NOT NULL"
            )))
            .fetch_all(pool)
            .await
            .unwrap();
            // Each caller seeds populated clocks before using this helper.
            assert!(
                !types.is_empty(),
                "missing repository-write coverage for {table}.{column}"
            );
            assert!(
                types.iter().all(|kind| kind == "integer"),
                "{table}.{column}: {types:?}"
            );
        }
    }
}

#[tokio::test]
async fn user_list_count_and_lookup_preserve_full_projection_across_updates() {
    let pool = database().await;
    let users = SqlxUserRepository::new(pool.clone(), pool.clone());
    assert_eq!(users.count().await.unwrap(), 0);
    assert!(users.list(10, 0).await.unwrap().is_empty());
    let first = user("older", CREATED);
    let mut second = user("newer", CREATED + 1000);
    second.is_active = false;
    second.email = None;
    second.last_login_at = None;
    users.create(&first).await.unwrap();
    users.create(&second).await.unwrap();
    let all = users.list(10, 0).await.unwrap();
    assert_eq!(users.count().await.unwrap(), all.len() as i64);
    same(&all, &vec![second.clone(), first.clone()]);
    same(&users.list(1, 0).await.unwrap(), &vec![second.clone()]);
    same(&users.list(1, 1).await.unwrap(), &vec![first.clone()]);
    assert!(users.list(1, 2).await.unwrap().is_empty());
    assert!(users.list(0, 0).await.unwrap().is_empty());
    for expected in [&first, &second] {
        same(
            &users.find_by_id(&expected.id).await.unwrap().unwrap(),
            expected,
        );
        same(
            &users
                .find_by_username(&expected.username)
                .await
                .unwrap()
                .unwrap(),
            expected,
        );
        if let Some(email) = &expected.email {
            same(
                &users.find_by_email(email).await.unwrap().unwrap(),
                expected,
            );
        }
    }
    assert!(users.find_by_id("missing").await.unwrap().is_none());
    assert!(users.find_by_username("missing").await.unwrap().is_none());
    assert!(users.find_by_email("missing").await.unwrap().is_none());

    let mut changed = first.clone();
    changed.username = "renamed".into();
    changed.password_hash = "new-hash".into();
    changed.email = None;
    changed.roles = r#"["viewer"]"#.into();
    changed.is_active = false;
    changed.must_change_password = false;
    changed.last_login_at = None;
    changed.created_at = 7; // Updates retain the original creation time.
    let before = now_ms();
    users.update(&changed).await.unwrap();
    let stored = users.find_by_id(&first.id).await.unwrap().unwrap();
    assert!((before..=now_ms()).contains(&stored.updated_at));
    changed.created_at = first.created_at;
    changed.updated_at = stored.updated_at;
    same(&stored, &changed);
    same(
        &users.find_by_username("renamed").await.unwrap().unwrap(),
        &changed,
    );
    same(&users.list(1, 1).await.unwrap(), &vec![changed.clone()]);
    assert!(
        users
            .find_by_username(&first.username)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        users
            .find_by_email(first.email.as_deref().unwrap())
            .await
            .unwrap()
            .is_none()
    );

    users
        .update_last_login(&first.id, CREATED + 777)
        .await
        .unwrap();
    let logged_in = users.find_by_id(&first.id).await.unwrap().unwrap();
    assert_eq!(logged_in.last_login_at, Some(CREATED + 777));
    users
        .update_password(&second.id, "hash-retain", false)
        .await
        .unwrap();
    let password = users.find_by_id(&second.id).await.unwrap().unwrap();
    assert!(password.must_change_password);
    assert_eq!(password.password_hash, "hash-retain");
    users
        .update_password(&second.id, "hash-clear", true)
        .await
        .unwrap();
    let password = users.find_by_id(&second.id).await.unwrap().unwrap();
    assert!(!password.must_change_password);
    assert_eq!(password.password_hash, "hash-clear");
    let types: (String, String, String) = sqlx::query_as("SELECT typeof(created_at), typeof(updated_at), typeof(last_login_at) FROM users WHERE id = 'older'").fetch_one(&pool).await.unwrap();
    assert_eq!(
        types,
        ("integer".into(), "integer".into(), "integer".into())
    );
    users.delete(&first.id).await.unwrap();
    users.delete("missing").await.unwrap();
    assert_eq!(users.count().await.unwrap(), 1);
    assert!(users.find_by_id(&first.id).await.unwrap().is_none());
    assert_eq!(users.list(10, 0).await.unwrap()[0].id, second.id);
}

fn token(
    id: &str,
    owner: &str,
    created_at: i64,
    expires_at: i64,
    revoked_at: Option<i64>,
) -> RefreshTokenDbModel {
    RefreshTokenDbModel {
        id: id.into(),
        user_id: owner.into(),
        token_hash: format!("hash-{id}"),
        expires_at,
        created_at,
        revoked_at,
        device_info: Some(format!("device-{id}")),
        session_id: Some(format!("session-{id}")),
    }
}

async fn active_tokens(
    repo: &SqlxRefreshTokenRepository,
    owner: &str,
    expected: &[RefreshTokenDbModel],
) {
    let list = repo.find_active_by_user(owner).await.unwrap();
    assert_eq!(
        repo.count_active_by_user(owner).await.unwrap(),
        list.len() as i64
    );
    same(&list, &expected.to_vec());
    for item in &list {
        same(
            &repo
                .find_by_token_hash(&item.token_hash)
                .await
                .unwrap()
                .unwrap(),
            item,
        );
    }
}

#[tokio::test]
async fn token_and_key_queries_agree_on_owners_expiry_revocation_and_storage() {
    let pool = database().await;
    let users = SqlxUserRepository::new(pool.clone(), pool.clone());
    let tokens = SqlxRefreshTokenRepository::new(pool.clone(), pool.clone());
    let keys = SqlxApiKeyRepository::new(pool.clone(), pool.clone());
    for id in ["alice", "bob"] {
        users.create(&user(id, CREATED)).await.unwrap();
    }
    let future = now_ms() + 86_400_000;
    let older = token("older", "alice", CREATED, future, None);
    let newer = token("newer", "alice", CREATED + 1000, future, None);
    // Zero is certainly expired without relying on sleeps or a moving equality boundary.
    let expired = token("expired", "alice", CREATED + 2000, 0, None);
    let revoked = token(
        "revoked",
        "alice",
        CREATED + 3000,
        future,
        Some(CREATED + 4000),
    );
    let other = token("other", "bob", CREATED + 4000, future, None);
    for value in [&older, &newer, &expired, &revoked, &other] {
        tokens.create(value).await.unwrap();
        same(
            &tokens
                .find_by_token_hash(&value.token_hash)
                .await
                .unwrap()
                .unwrap(),
            value,
        );
    }
    active_tokens(&tokens, "alice", &[newer.clone(), older.clone()]).await;
    active_tokens(&tokens, "bob", std::slice::from_ref(&other)).await;
    active_tokens(&tokens, "missing", &[]).await;
    assert!(
        tokens
            .find_by_token_hash("missing")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        tokens
            .find_session("bob", older.session_id.as_deref().unwrap())
            .await
            .unwrap()
            .is_none()
    );
    let session = tokens
        .find_session("alice", older.session_id.as_deref().unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (session.created_at, session.expires_at, session.revoked_at),
        (CREATED, future, None)
    );
    let before_revoke = now_ms();
    tokens.revoke(&newer.id).await.unwrap();
    let revoked_newer = tokens
        .find_by_token_hash(&newer.token_hash)
        .await
        .unwrap()
        .unwrap();
    assert!((before_revoke..=now_ms()).contains(&revoked_newer.revoked_at.unwrap()));
    tokens.revoke(&newer.id).await.unwrap();
    same(
        &tokens
            .find_by_token_hash(&newer.token_hash)
            .await
            .unwrap()
            .unwrap(),
        &revoked_newer,
    );
    tokens.revoke("missing").await.unwrap();
    active_tokens(&tokens, "alice", std::slice::from_ref(&older)).await;
    let before_revoke_all = now_ms();
    tokens.revoke_all_for_user("alice").await.unwrap();
    active_tokens(&tokens, "alice", &[]).await;
    active_tokens(&tokens, "bob", std::slice::from_ref(&other)).await;
    let revoked_session = tokens
        .find_session("alice", older.session_id.as_deref().unwrap())
        .await
        .unwrap()
        .unwrap();
    assert!((before_revoke_all..=now_ms()).contains(&revoked_session.revoked_at.unwrap()));
    let revoked_older = tokens
        .find_by_token_hash(&older.token_hash)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(revoked_older.revoked_at, revoked_session.revoked_at);
    assert_eq!(
        tokens
            .find_by_token_hash(&revoked.token_hash)
            .await
            .unwrap()
            .unwrap()
            .revoked_at,
        revoked.revoked_at
    );

    let mut alice_keys = Vec::new();
    for (index, (expires, revoked_at)) in [
        (None, None),
        (Some(0), None),
        (Some(future), Some(CREATED + 1)),
    ]
    .into_iter()
    .enumerate()
    {
        let mut key = ApiKeyDbModel::new(
            "alice",
            format!("key-{index}"),
            format!("key-hash-{index}"),
            "srec_test",
            ApiKeyAccessLevel::Full,
            expires,
        );
        key.created_at = CREATED + index as i64;
        key.revoked_at = revoked_at;
        keys.create(&key).await.unwrap();
        alice_keys.push(key);
    }
    let other_key = ApiKeyDbModel::new(
        "bob",
        "other",
        "other-key-hash",
        "srec_other",
        ApiKeyAccessLevel::ReadOnly,
        None,
    );
    keys.create(&other_key).await.unwrap();
    alice_keys.reverse();
    same(&keys.list_by_user("alice").await.unwrap(), &alice_keys);
    same(
        &keys.list_by_user("bob").await.unwrap(),
        &vec![other_key.clone()],
    );
    assert!(keys.list_by_user("missing").await.unwrap().is_empty());
    for key in &alice_keys {
        same(
            &keys.find_by_key_hash(&key.key_hash).await.unwrap().unwrap(),
            key,
        );
        same(
            &keys.find_by_id("alice", &key.id).await.unwrap().unwrap(),
            key,
        );
        assert!(keys.find_by_id("bob", &key.id).await.unwrap().is_none());
        assert!(!keys.revoke("bob", &key.id).await.unwrap());
    }
    assert!(keys.find_by_key_hash("missing").await.unwrap().is_none());
    assert!(keys.find_by_id("alice", "missing").await.unwrap().is_none());
    assert!(!keys.revoke("alice", "missing").await.unwrap());
    let key = alice_keys.last_mut().unwrap();
    keys.update_last_used(&key.id, CREATED + 987).await.unwrap();
    key.last_used_at = Some(CREATED + 987);
    same(
        &keys.find_by_id("alice", &key.id).await.unwrap().unwrap(),
        key,
    );
    let before_key_revoke = now_ms();
    assert!(keys.revoke("alice", &key.id).await.unwrap());
    let stored = keys.find_by_id("alice", &key.id).await.unwrap().unwrap();
    assert!((before_key_revoke..=now_ms()).contains(&stored.revoked_at.unwrap()));
    key.revoked_at = stored.revoked_at;
    same(&stored, key);
    assert!(!keys.revoke("alice", &key.id).await.unwrap());
    same(&keys.list_by_user("alice").await.unwrap(), &alice_keys);
    same(
        &keys
            .find_by_id("bob", &other_key.id)
            .await
            .unwrap()
            .unwrap(),
        &other_key,
    );
    account_storage(&pool).await;
}
