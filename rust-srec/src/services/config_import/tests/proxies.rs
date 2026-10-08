use super::*;
use crate::config::backup::{BackupRoute, PlatformExport, ProxyExport, TemplateExport};
use crate::database::repositories::proxies::{self, RouteOwner};
use crate::proxies::{ProxyEndpoint, ProxyRoute};

fn platform_export(model: &PlatformConfigDbModel) -> PlatformExport {
    serde_json::from_value(serde_json::to_value(model).unwrap()).unwrap()
}

fn template(name: &str) -> TemplateExport {
    serde_json::from_value(serde_json::json!({"name": name})).unwrap()
}

/// Imports `config` the way the import service does.
async fn import(
    pool: &SqlitePool,
    mut config: ConfigExport,
    mode: ImportMode,
) -> Result<(), ConfigurationImportError> {
    validate_import(&config, mode)?;
    let mut tx = begin_immediate(pool).await.unwrap();
    let snapshot = ImportSnapshot::load(&mut tx).await.unwrap();
    snapshot.upgrade_bundle(&mut config, mode)?;
    snapshot.validate_references(&config, mode)?;
    apply_import(&mut tx, &snapshot, &config, mode).await?;
    tx.commit().await.unwrap();
    Ok(())
}

async fn route_name(pool: &SqlitePool, owner: RouteOwner) -> String {
    let mut connection = pool.acquire().await.unwrap();
    match proxies::route_of(&mut connection, &owner).await.unwrap() {
        ProxyRoute::Proxy { id } => {
            format!(
                "proxy:{}",
                proxies::get(&mut connection, &id).await.unwrap().name
            )
        }
        route => route.kind().to_owned(),
    }
}

async fn database() -> (SqlitePool, ImportSnapshot) {
    let pool = init_pool_with_size("sqlite::memory:", 1).await.unwrap();
    run_migrations(&pool).await.unwrap();
    let mut tx = begin_immediate(&pool).await.unwrap();
    let snapshot = ImportSnapshot::load(&mut tx).await.unwrap();
    tx.commit().await.unwrap();
    (pool, snapshot)
}

#[tokio::test]
async fn older_backups_convert_settings_and_reuse_identical_saved_proxies() {
    let (pool, snapshot) = database().await;
    {
        let mut connection = pool.acquire().await.unwrap();
        proxies::create(
            &mut connection,
            None,
            "Office",
            &ProxyEndpoint::new(
                "http://proxy.example:3128",
                Some(("user".into(), "secret".into())),
            ),
        )
        .await
        .unwrap();
        proxies::create(
            &mut connection,
            None,
            "other.example:1",
            &ProxyEndpoint::new("http://unrelated.example:9", None),
        )
        .await
        .unwrap();
    }
    let mut config = import_config(&snapshot.global);
    config.global_config.proxy_config = serde_json::json!({"enabled": false});
    let mut bilibili = platform_export(&snapshot.platforms["bilibili"]);
    bilibili.proxy_config = Some(serde_json::json!(
        r#"{"enabled":true,"url":"proxy.example:3128","username":"user","password":"secret"}"#
    ));
    let mut huya = platform_export(&snapshot.platforms["huya"]);
    huya.proxy_config = Some(serde_json::json!({
        "enabled": true, "url": "http://other.example:1", "username": "u", "password": "p"
    }));
    config.platforms = vec![bilibili, huya];
    let mut system = template("System");
    system.proxy_config = Some(serde_json::json!({"enabled": true, "use_system_proxy": true}));
    config.templates.push(system);
    let mut streamer = imported_streamer("https://live.bilibili.com/77", "bilibili");
    streamer.streamer_specific_config =
        Some(serde_json::json!({"proxy_config": {"enabled": false}, "quality": "origin"}));
    config.streamers.push(streamer);
    import(&pool, config, ImportMode::Merge).await.unwrap();

    // A restored global setting means what the backup says; the environment
    // is not consulted.
    assert_eq!(route_name(&pool, RouteOwner::Global).await, "direct");
    assert_eq!(
        route_name(&pool, RouteOwner::Platform("platform-bilibili".into())).await,
        "proxy:Office"
    );
    assert_eq!(
        route_name(&pool, RouteOwner::Platform("platform-huya".into())).await,
        "proxy:other.example:1 2"
    );
    let template_id: String =
        sqlx::query_scalar("SELECT id FROM template_config WHERE name = 'System'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        route_name(&pool, RouteOwner::Template(template_id)).await,
        "system"
    );
    let (streamer_id, document): (String, String) = sqlx::query_as(
        "SELECT id, streamer_specific_config FROM streamers WHERE url = 'https://live.bilibili.com/77'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        route_name(&pool, RouteOwner::Streamer(streamer_id)).await,
        "direct"
    );
    assert!(!document.contains("proxy_config"));
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM proxies")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 3);
}

#[tokio::test]
async fn managed_backups_name_proxies_and_replace_drops_only_unused_ones() {
    let (pool, snapshot) = database().await;
    let kept = proxies::save_for_test(&pool, "Kept", "http://kept.example:1").await;
    proxies::save_for_test(&pool, "Stale", "http://stale.example:1").await;
    proxies::set_route(
        &mut pool.acquire().await.unwrap(),
        &RouteOwner::Platform("platform-douyu".into()),
        &ProxyRoute::Proxy { id: kept },
    )
    .await
    .unwrap();

    let mut config = import_config(&snapshot.global);
    config.version = crate::config::backup::MANAGED_CREDENTIAL_SCHEMA_VERSION.into();
    config.global_config.proxy_config = serde_json::Value::Null;
    config.global_config.proxy_route = Some(BackupRoute::Proxy {
        name: "office".into(),
    });
    config.proxies.push(ProxyExport {
        name: "Office".into(),
        url: "socks5h://office.example:1080".into(),
        username: Some("user".into()),
        password: Some("secret".into()),
    });
    // Replace resolves engine names from the bundle alone.
    config.engines = snapshot
        .engines
        .values()
        .map(|engine| crate::config::backup::EngineExport {
            name: engine.name.clone(),
            engine_type: engine.engine_type.clone(),
            config: serde_json::from_str(&engine.config).unwrap(),
        })
        .collect();
    let mut huya = platform_export(&snapshot.platforms["huya"]);
    huya.proxy_config = None;
    huya.proxy_route = Some(BackupRoute::Direct);
    config.platforms.push(huya);
    import(&pool, config.clone(), ImportMode::Replace)
        .await
        .unwrap();

    assert_eq!(route_name(&pool, RouteOwner::Global).await, "proxy:Office");
    assert_eq!(
        route_name(&pool, RouteOwner::Platform("platform-huya".into())).await,
        "direct"
    );
    // A platform the bundle omits keeps its route, and the proxy it names.
    assert_eq!(
        route_name(&pool, RouteOwner::Platform("platform-douyu".into())).await,
        "proxy:Kept"
    );
    let names: Vec<String> = sqlx::query_scalar("SELECT name FROM proxies ORDER BY name")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(names, ["Kept", "Office"]);
    let office: (String, i64) =
        sqlx::query_as("SELECT password, version FROM proxies WHERE name = 'Office'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(office, ("secret".to_owned(), 1));

    // Importing the same bundle again changes nothing.
    import(&pool, config.clone(), ImportMode::Merge)
        .await
        .unwrap();
    let version: i64 = sqlx::query_scalar("SELECT version FROM proxies WHERE name = 'Office'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(version, 1);

    let mut unknown = config;
    unknown.global_config.proxy_route = Some(BackupRoute::Proxy {
        name: "Nowhere".into(),
    });
    assert!(import(&pool, unknown, ImportMode::Merge).await.is_err());
    assert_eq!(route_name(&pool, RouteOwner::Global).await, "proxy:Office");
}

#[tokio::test]
async fn routes_name_bundle_proxies_as_the_installation_stores_them() {
    let (pool, snapshot) = database().await;
    let mut config = import_config(&snapshot.global);
    config.version = crate::config::backup::MANAGED_CREDENTIAL_SCHEMA_VERSION.into();
    config.global_config.proxy_config = serde_json::Value::Null;
    // Names are stored trimmed and compare without case; a route must find
    // the entry under the name the bundle wrote.
    config.global_config.proxy_route = Some(BackupRoute::Proxy {
        name: "  Spaced Office ".into(),
    });
    config.proxies.push(ProxyExport {
        name: " Spaced Office  ".into(),
        url: "http://spaced.example:3128".into(),
        username: None,
        password: None,
    });
    let mut huya = platform_export(&snapshot.platforms["huya"]);
    huya.proxy_config = None;
    huya.proxy_route = Some(BackupRoute::Proxy {
        name: "SPACED OFFICE".into(),
    });
    config.platforms.push(huya);
    import(&pool, config, ImportMode::Merge).await.unwrap();

    assert_eq!(
        route_name(&pool, RouteOwner::Global).await,
        "proxy:Spaced Office"
    );
    assert_eq!(
        route_name(&pool, RouteOwner::Platform("platform-huya".into())).await,
        "proxy:Spaced Office"
    );
}

#[test]
fn bundles_without_saved_proxies_keep_the_shape_older_releases_read() {
    let mut config = import_config(&GlobalConfigDbModel::default());
    config.global_config.proxy_config = serde_json::Value::Null;
    config.global_config.proxy_route = Some(BackupRoute::System);
    let mut platform: PlatformExport =
        serde_json::from_value(serde_json::json!({"platform_name": "huya"})).unwrap();
    platform.proxy_route = Some(BackupRoute::Direct);
    config.platforms.push(platform);
    let mut streamer = imported_streamer("https://www.huya.com/1", "huya");
    streamer.proxy_route = Some(BackupRoute::Direct);
    config.streamers.push(streamer);
    let mut inheriting = template("Inherits");
    inheriting.proxy_route = Some(BackupRoute::Inherit);
    config.templates.push(inheriting);
    config.update_schema_version();
    assert_eq!(config.version, "0.1.8");
    let value = serde_json::to_value(&config).unwrap();
    assert_eq!(
        value["global_config"]["proxy_config"],
        serde_json::json!({"enabled": true, "use_system_proxy": true})
    );
    assert_eq!(
        value["platforms"][0]["proxy_config"],
        serde_json::json!({"enabled": false})
    );
    assert_eq!(
        value["streamers"][0]["streamer_specific_config"]["proxy_config"],
        serde_json::json!({"enabled": false})
    );
    assert!(value["templates"][0].get("proxy_config").is_none());
    assert!(!value.to_string().contains("proxy_route"));

    // Reading it back restores the same routes.
    let mut restored: ConfigExport = serde_json::from_value(value).unwrap();
    crate::database::legacy_proxy_upgrade::upgrade_bundle(&mut restored, &[], true).unwrap();
    assert_eq!(
        restored.global_config.proxy_route,
        Some(BackupRoute::System)
    );
    assert_eq!(restored.platforms[0].proxy_route, Some(BackupRoute::Direct));
    assert_eq!(restored.streamers[0].proxy_route, Some(BackupRoute::Direct));
    assert_eq!(
        restored.templates[0].proxy_route,
        Some(BackupRoute::Inherit)
    );
    assert!(restored.proxies.is_empty());
}
