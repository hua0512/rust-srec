use super::*;
use crate::proxies::{RouteKind, RouteSource};

async fn database() -> sqlx::SqlitePool {
    let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
        .await
        .unwrap();
    crate::database::run_migrations(&pool).await.unwrap();
    pool
}

fn endpoint(url: &str, login: Option<(&str, &str)>) -> ProxyEndpoint {
    ProxyEndpoint::new(
        url,
        login.map(|(user, pass)| (user.to_owned(), pass.to_owned())),
    )
}

async fn revision(connection: &mut sqlx::SqliteConnection, id: &str) -> i64 {
    sqlx::query_scalar("SELECT revision FROM credential_profiles WHERE id = ?")
        .bind(id)
        .fetch_one(connection)
        .await
        .unwrap()
}

async fn account(pool: &sqlx::SqlitePool, route: &ProxyRoute) -> String {
    let profiles =
        crate::database::repositories::CredentialProfileRepository::new(pool.clone(), pool.clone());
    profiles
        .create(
            "platform-huya",
            "Account",
            true,
            &crate::credentials::CredentialMaterial {
                cookies: "account=a".into(),
                refresh_token: None,
                access_token: None,
                reauth_config: None,
            },
            route,
        )
        .await
        .unwrap()
        .id
}

#[tokio::test]
async fn an_omitted_password_is_kept_and_a_removed_login_takes_it_along() {
    let pool = database().await;
    let mut connection = pool.acquire().await.unwrap();
    let saved = create(
        &mut connection,
        None,
        " Office ",
        &endpoint("HTTP://Proxy.Example:8080/", Some(("user", "secret"))),
    )
    .await
    .unwrap();
    assert_eq!(saved.name, "Office");
    assert_eq!(saved.url, "http://proxy.example:8080");
    let renamed = update(
        &mut connection,
        &saved.id,
        saved.version,
        ProxyUpdate {
            name: Some("Office 2".into()),
            ..ProxyUpdate::default()
        },
    )
    .await
    .unwrap();
    assert!(!renamed.exit_changed);
    assert_eq!(renamed.entry.password.as_deref(), Some("secret"));
    assert_eq!(renamed.entry.version, saved.version + 1);
    let new_user = update(
        &mut connection,
        &saved.id,
        renamed.entry.version,
        ProxyUpdate {
            username: Some(Some("other".into())),
            ..ProxyUpdate::default()
        },
    )
    .await
    .unwrap();
    assert!(new_user.exit_changed);
    assert_eq!(new_user.entry.password.as_deref(), Some("secret"));
    let anonymous = update(
        &mut connection,
        &saved.id,
        new_user.entry.version,
        ProxyUpdate {
            username: Some(None),
            password: Some("ignored".into()),
            ..ProxyUpdate::default()
        },
    )
    .await
    .unwrap();
    assert!(anonymous.entry.username.is_none() && anonymous.entry.password.is_none());
    assert!(matches!(
        update(
            &mut connection,
            &saved.id,
            saved.version,
            ProxyUpdate::default()
        )
        .await,
        Err(Error::Proxy(ProxyError::StaleVersion))
    ));
    assert!(!format!("{:?}", new_user.entry).contains("secret"));
}

#[tokio::test]
async fn names_and_exits_are_unique() {
    let pool = database().await;
    let mut connection = pool.acquire().await.unwrap();
    create(
        &mut connection,
        None,
        "Office",
        &endpoint("http://proxy.example:8080", Some(("user", "a"))),
    )
    .await
    .unwrap();
    assert!(matches!(
        create(
            &mut connection,
            None,
            "OFFICE",
            &endpoint("http://other.example:8080", None)
        )
        .await,
        Err(Error::Proxy(ProxyError::NameTaken(name))) if name == "Office"
    ));
    // The password does not make another exit.
    assert!(matches!(
        create(
            &mut connection,
            None,
            "Second",
            &endpoint("http://Proxy.Example:8080/", Some(("user", "b")))
        )
        .await,
        Err(Error::Proxy(ProxyError::DuplicateEndpoint { name })) if name == "Office"
    ));
    // Another username is another exit.
    create(
        &mut connection,
        None,
        "Second",
        &endpoint("http://proxy.example:8080", Some(("other", "b"))),
    )
    .await
    .unwrap();
    for invalid in [
        endpoint("socks4://proxy.example:1080", None),
        endpoint("http://user:pass@proxy.example:1", None),
        endpoint("proxy.example:1", None),
    ] {
        assert!(matches!(
            create(&mut connection, None, "Invalid", &invalid).await,
            Err(Error::Proxy(ProxyError::Invalid(_)))
        ));
    }
}

#[tokio::test]
async fn a_proxy_in_use_cannot_be_deleted_and_the_refusal_names_its_users() {
    let pool = database().await;
    let id = save_for_test(&pool, "Shared", "http://shared.example:8080").await;
    let route = ProxyRoute::Proxy { id: id.clone() };
    let mut connection = pool.acquire().await.unwrap();
    sqlx::query("INSERT INTO template_config(id, name) VALUES ('retiring', 'Old template'), ('live', 'Template')")
        .execute(&mut *connection)
        .await
        .unwrap();
    sqlx::query("INSERT INTO streamers(id, name, url, platform_config_id, state) VALUES ('kept', 'Kept', 'https://www.huya.com/kept', 'platform-huya', 'NOT_LIVE'), ('retired', 'Retired', 'https://www.huya.com/retired', 'platform-huya', 'NOT_LIVE')")
        .execute(&mut *connection)
        .await
        .unwrap();
    for owner in [
        RouteOwner::Global,
        RouteOwner::Platform("platform-huya".into()),
        RouteOwner::Template("retiring".into()),
        RouteOwner::Template("live".into()),
        RouteOwner::Streamer("kept".into()),
        RouteOwner::Streamer("retired".into()),
    ] {
        set_route(&mut connection, &owner, &route).await.unwrap();
    }
    // Marked after its route was written: any template update cancels a
    // deferred deletion.
    sqlx::query(
        "INSERT INTO retirement_config_deletions(kind, config_id) VALUES ('template', 'retiring')",
    )
    .execute(&mut *connection)
    .await
    .unwrap();
    drop(connection);
    let account_id = account(&pool, &route).await;
    let mut connection = pool.acquire().await.unwrap();
    // A streamer marked deleted stops holding the proxy.
    sqlx::query("UPDATE streamers SET deleted_at = 1 WHERE id = 'retired'")
        .execute(&mut *connection)
        .await
        .unwrap();
    assert_eq!(
        route_of(&mut connection, &RouteOwner::Streamer("retired".into()))
            .await
            .unwrap(),
        ProxyRoute::Inherit
    );
    let Err(Error::Proxy(ProxyError::Referenced(references))) =
        delete(&mut connection, &id, None).await
    else {
        panic!("a proxy in use must not be deleted");
    };
    assert!(references.global);
    assert_eq!(references.platforms[0].id, "platform-huya");
    assert_eq!(
        references
            .templates
            .iter()
            .map(|template| (template.id.as_str(), template.being_removed))
            .collect::<Vec<_>>(),
        [("retiring", true), ("live", false)]
    );
    assert_eq!(
        references
            .streamers
            .iter()
            .map(|streamer| streamer.id.as_str())
            .collect::<Vec<_>>(),
        ["kept"]
    );
    assert_eq!(references.accounts[0].id, account_id);
    assert_eq!(references.accounts[0].platform_name, "huya");
    assert_eq!(
        usage_counts(&mut connection).await.unwrap().get(&id),
        Some(&references.count())
    );

    // The database refuses too, whatever the caller checked.
    assert!(
        sqlx::query("DELETE FROM proxies WHERE id = ?")
            .bind(&id)
            .execute(&mut *connection)
            .await
            .is_err()
    );

    for owner in [
        RouteOwner::Global,
        RouteOwner::Platform("platform-huya".into()),
        RouteOwner::Template("retiring".into()),
        RouteOwner::Template("live".into()),
        RouteOwner::Streamer("kept".into()),
        RouteOwner::Account(account_id.clone()),
    ] {
        let replacement = if owner == RouteOwner::Global {
            ProxyRoute::Direct
        } else {
            ProxyRoute::Inherit
        };
        set_route(&mut connection, &owner, &replacement)
            .await
            .unwrap();
    }
    delete(&mut connection, &id, None).await.unwrap();
    let violations = sqlx::query("PRAGMA foreign_key_check")
        .fetch_all(&mut *connection)
        .await
        .unwrap();
    assert!(violations.is_empty());
}

#[tokio::test]
async fn routes_name_existing_proxies_and_the_global_route_never_inherits() {
    let pool = database().await;
    let mut connection = pool.acquire().await.unwrap();
    assert!(matches!(
        set_route(&mut connection, &RouteOwner::Global, &ProxyRoute::Inherit).await,
        Err(Error::Proxy(ProxyError::GlobalInherit))
    ));
    assert!(matches!(
        set_route(
            &mut connection,
            &RouteOwner::Platform("platform-huya".into()),
            &ProxyRoute::Proxy { id: "missing".into() }
        )
        .await,
        Err(Error::Proxy(ProxyError::Missing(id))) if id == "missing"
    ));
    assert!(matches!(
        set_route(
            &mut connection,
            &RouteOwner::Platform("missing-platform".into()),
            &ProxyRoute::Direct
        )
        .await,
        Err(Error::NotFound { .. })
    ));
    assert_eq!(
        route_of(&mut connection, &RouteOwner::Global)
            .await
            .unwrap(),
        ProxyRoute::Direct
    );
}

#[tokio::test]
async fn a_new_exit_moves_pinned_accounts_to_a_new_revision_but_a_new_name_does_not() {
    let pool = database().await;
    let id = save_for_test(&pool, "Pinned", "http://pinned.example:8080").await;
    let pinned = account(&pool, &ProxyRoute::Proxy { id: id.clone() }).await;
    let following = account(&pool, &ProxyRoute::Inherit).await;
    let mut connection = pool.acquire().await.unwrap();
    for account in [&pinned, &following] {
        sqlx::query("INSERT INTO credential_profile_health(profile_id, revision, validity) VALUES (?, 1, 'valid')")
            .bind(account)
            .execute(&mut *connection)
            .await
            .unwrap();
    }
    let entry = get(&mut connection, &id).await.unwrap();
    let renamed = update(
        &mut connection,
        &id,
        entry.version,
        ProxyUpdate {
            name: Some("Renamed".into()),
            password: Some("unused".into()),
            ..ProxyUpdate::default()
        },
    )
    .await
    .unwrap();
    assert!(!renamed.exit_changed && renamed.pinned_platforms.is_empty());
    assert_eq!(revision(&mut connection, &pinned).await, 1);
    let moved = update(
        &mut connection,
        &id,
        renamed.entry.version,
        ProxyUpdate {
            url: Some("socks5h://moved.example:1080".into()),
            ..ProxyUpdate::default()
        },
    )
    .await
    .unwrap();
    assert!(moved.exit_changed);
    assert_eq!(moved.pinned_platforms, ["platform-huya"]);
    assert_eq!(revision(&mut connection, &pinned).await, 2);
    assert_eq!(revision(&mut connection, &following).await, 1);
    let health: Vec<String> =
        sqlx::query_scalar("SELECT profile_id FROM credential_profile_health")
            .fetch_all(&mut *connection)
            .await
            .unwrap();
    assert_eq!(health, [following]);
}

#[tokio::test]
async fn account_management_follows_the_account_then_its_platform_then_global() {
    let pool = database().await;
    let global = save_for_test(&pool, "Global", "http://global.example:8080").await;
    let platform = save_for_test(&pool, "Platform", "http://platform.example:8080").await;
    let own = save_for_test(&pool, "Own", "http://own.example:8080").await;
    let system = SystemProxy::default();
    let mut connection = pool.acquire().await.unwrap();
    set_route(
        &mut connection,
        &RouteOwner::Global,
        &ProxyRoute::Proxy { id: global },
    )
    .await
    .unwrap();
    let management = resolve_account(
        &mut connection,
        "platform-huya",
        &ProxyRoute::Inherit,
        None,
        &system,
    )
    .await
    .unwrap();
    assert_eq!(
        (management.source, management.proxy_name()),
        (RouteSource::Global, Some("Global"))
    );
    set_route(
        &mut connection,
        &RouteOwner::Platform("platform-huya".into()),
        &ProxyRoute::Proxy { id: platform },
    )
    .await
    .unwrap();
    let management = resolve_account(
        &mut connection,
        "platform-huya",
        &ProxyRoute::Inherit,
        None,
        &system,
    )
    .await
    .unwrap();
    // Checks and refreshes of an account without its own route do not
    // bypass the configured proxy.
    assert_eq!(
        (management.source, management.proxy_name()),
        (RouteSource::Platform, Some("Platform"))
    );
    let operation = ResolvedRoute::direct(RouteSource::Streamer);
    let following = resolve_account(
        &mut connection,
        "platform-huya",
        &ProxyRoute::Inherit,
        Some(&operation),
        &system,
    )
    .await
    .unwrap();
    assert_eq!(following, operation);
    let pinned = resolve_account(
        &mut connection,
        "platform-huya",
        &ProxyRoute::Proxy { id: own.clone() },
        Some(&operation),
        &system,
    )
    .await
    .unwrap();
    assert_eq!(
        (pinned.kind(), pinned.source, pinned.key),
        (
            RouteKind::Proxy,
            RouteSource::Account,
            crate::proxies::RouteKey::Proxy { id: own }
        )
    );
    assert!(matches!(
        resolve_account(
            &mut connection,
            "platform-huya",
            &ProxyRoute::Proxy { id: "gone".into() },
            Some(&operation),
            &system,
        )
        .await,
        Err(Error::Proxy(ProxyError::Missing(_)))
    ));
}

#[tokio::test]
async fn streamer_documents_carry_routes_in_and_out_and_refuse_proxy_config() {
    let mut raw = Some(r#"{"quality":"best","proxy_route":{"kind":"system"}}"#.to_owned());
    assert_eq!(take_document(&mut raw).unwrap(), Some(ProxyRoute::System));
    assert_eq!(raw.as_deref(), Some(r#"{"quality":"best"}"#));
    let mut untouched = Some(r#"{"quality":"best"}"#.to_owned());
    assert_eq!(take_document(&mut untouched).unwrap(), None);
    assert_eq!(untouched.as_deref(), Some(r#"{"quality":"best"}"#));
    let mut invalid = Some(r#"{"proxy_route":{"kind":"proxy"}}"#.to_owned());
    assert!(take_document(&mut invalid).is_err());

    let injected = inject_document(
        Some(serde_json::json!({"quality": "best", "proxy_route": "stale"})),
        Some(&ProxyRoute::Direct),
    )
    .unwrap();
    assert_eq!(
        injected["proxy_route"],
        serde_json::json!({"kind": "direct"})
    );
    let inherited = inject_document(
        Some(serde_json::json!({"proxy_route": "stale"})),
        Some(&ProxyRoute::Inherit),
    )
    .unwrap();
    assert!(inherited.get("proxy_route").is_none());

    assert!(reject_legacy_document(Some(r#"{"proxy_config":null}"#)).is_ok());
    assert!(matches!(
        reject_legacy_document(Some(r#"{"proxy_config":{"enabled":false}}"#)),
        Err(Error::Proxy(ProxyError::ConfigReplaced))
    ));
    assert!(matches!(
        reject_legacy_overrides(Some(r#"{"bilibili":{"proxy_config":{"enabled":true}}}"#)),
        Err(Error::Proxy(ProxyError::ConfigReplaced))
    ));
}
