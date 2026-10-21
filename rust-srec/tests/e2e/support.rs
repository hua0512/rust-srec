//! Harness that runs the real backend in-process for end-to-end tests.
//!
//! [`TestApp::start`] follows the desktop shell's bootstrap: migrations on a
//! fresh SQLite file, the full `ServiceContainer`, and the HTTP server with JWT
//! authentication bound to an ephemeral loopback port. Tests talk to it only
//! over HTTP, so routing, authentication, validation and JSON shapes are
//! exercised exactly as the frontend sees them.
//!
//! Reliability rules for tests built on this harness: never reach the public
//! network (create streamers disabled; use [`LocalHttpServer`] for outbound
//! targets), never sleep for a fixed time (use [`eventually`]), keep every
//! wait bounded, and write files only under [`TestApp::output_dir`].
//!
//! Scenarios that monitor or record streamers start the app with
//! [`TestApp::start_with`] and a [`Seed`], which routes every platform but the
//! offline [`FakePlatform`] to a proxy that refuses everything, and script the
//! fake platform's rooms. They run on a multi-threaded runtime and shut the
//! app down before the fake platform drops.
//!
//! The backend reads some settings from the process environment, and the
//! scenarios assert the defaults: [`TestApp::start`] refuses to run when one
//! of [`DEFAULT_ONLY_ENV`] is set, or when a proxy variable is exported
//! without `NO_PROXY` covering `127.0.0.1` (outbound clients such as the
//! webhook sender honor the proxy variables, so requests to a
//! [`LocalHttpServer`] would go through the proxy).

use std::future::Future;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use futures::StreamExt;
use reqwest::header::{AsHeaderName, HeaderMap};
use reqwest::{Method, StatusCode};
use rust_srec::backend::{self, ApiServerConfig, ServiceContainer, ServiceContainerConfig};
use rust_srec::logging::LoggingConfig;
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::net::TcpStream;
use tokio::sync::OnceCell;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::{self, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

mod fake_platform;
mod http;
mod media;
mod seed;
mod smtp;

pub use fake_platform::{FakePlatform, FakeRoom};
pub use http::{ClosedPort, ConnectRecorder, LocalHttpServer, ReceivedRequest};
pub use media::FlvLoop;
pub use seed::Seed;
pub use smtp::SmtpFixture;

pub const ADMIN_USERNAME: &str = "admin";
/// Password of the admin account seeded by the initial migration.
pub const SEEDED_ADMIN_PASSWORD: &str = "admin123!";
/// Password [`TestApp::admin`] sets when it completes the forced change.
pub const ADMIN_PASSWORD: &str = "e2e-admin-Password-1";
/// Secret the API server signs access tokens with, for tests that mint tokens.
pub const JWT_SECRET: &str = "e2e-test-jwt-secret-with-enough-entropy-0123456789";

/// Bound for [`eventually`] and for every WebSocket read. Background work the
/// tests wait for (event delivery, streamer reaping, workflow steps, socket
/// revalidation every 5 s) settles well within it.
pub const WAIT: Duration = Duration::from_secs(10);
/// Bound on one HTTP request and on shutdown. Neither is a latency check:
/// many backends start in parallel and hash passwords, so a single request
/// or a service drain can stall for seconds on a loaded machine; these only
/// turn a hang into a failure.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(30);
/// Cooperative part of [`SHUTDOWN_TIMEOUT`]; after it the backend aborts
/// what is still running.
const SHUTDOWN_GRACE_PERIOD: Duration = Duration::from_secs(20);

/// Settings the backend reads from the environment that change what the
/// scenarios assert: token lifetimes, the password and lockout rules, the JWT
/// claims tests decode, whether Web Push is configured, and the locale of the
/// messages some tests pin.
const DEFAULT_ONLY_ENV: [&str; 13] = [
    "ACCESS_TOKEN_EXPIRATION_SECS",
    "REFRESH_TOKEN_EXPIRATION_SECS",
    "REFRESH_TOKEN_REUSE_GRACE_SECS",
    "REVOKE_ALL_ON_REFRESH_TOKEN_REUSE",
    "MIN_PASSWORD_LENGTH",
    "JWT_ISSUER",
    "JWT_AUDIENCE",
    "API_LOGIN_MAX_FAILURES",
    "API_LOGIN_IP_MAX_FAILURES",
    "API_LOGIN_WINDOW_SECS",
    "WEB_PUSH_VAPID_PUBLIC_KEY",
    "WEB_PUSH_VAPID_PRIVATE_KEY",
    "RUST_SREC_LOCALE",
];

const PROXY_ENV: [&str; 6] = [
    "HTTP_PROXY",
    "http_proxy",
    "HTTPS_PROXY",
    "https_proxy",
    "ALL_PROXY",
    "all_proxy",
];

fn assert_default_environment() {
    let overridden: Vec<&str> = DEFAULT_ONLY_ENV
        .into_iter()
        .filter(|name| std::env::var_os(name).is_some())
        .collect();
    assert!(
        overridden.is_empty(),
        "the end-to-end tests assert the backend's defaults; unset {overridden:?} to run them"
    );

    if PROXY_ENV
        .iter()
        .any(|name| std::env::var_os(name).is_some())
    {
        let no_proxy = std::env::var("NO_PROXY")
            .or_else(|_| std::env::var("no_proxy"))
            .unwrap_or_default();
        assert!(
            no_proxy
                .split(',')
                .any(|entry| matches!(entry.trim(), "*" | "127.0.0.1" | "127.0.0.0/8")),
            "a proxy variable is set; add 127.0.0.1 to NO_PROXY so requests to the local test servers are not proxied"
        );
    }
}

/// The global `tracing` subscriber can be installed once per process, so every
/// [`TestApp`] in a process shares one logging configuration. Its directory is
/// shared by all test processes too; the log store coordinates writers across
/// processes and bounds the files it keeps.
fn logging() -> Arc<LoggingConfig> {
    static LOGGING: OnceLock<(
        Arc<LoggingConfig>,
        tracing_appender::non_blocking::WorkerGuard,
    )> = OnceLock::new();
    LOGGING
        .get_or_init(|| {
            let log_dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("e2e-logs");
            backend::init_logging(&log_dir.to_string_lossy()).expect("initialize logging")
        })
        .0
        .clone()
}

/// A running backend with its own temporary data directory.
pub struct TestApp {
    base_url: String,
    database_url: String,
    client: reqwest::Client,
    container: Arc<ServiceContainer>,
    admin: OnceCell<Tokens>,
    output_dir: PathBuf,
    /// The proxy every route but the fake platform's ends at, for apps
    /// started with [`TestApp::start_with`].
    dead_end: Option<ConnectRecorder>,
    _data_dir: TempDir,
}

/// Access and refresh tokens returned by `/api/auth/login` or `/refresh`.
#[derive(Debug, Clone)]
pub struct Tokens {
    pub access: String,
    pub refresh: String,
}

/// An API key as `POST /api/auth/api-keys` returns it.
pub struct ApiKey {
    pub id: String,
    /// The secret itself, shown only on creation.
    pub raw: String,
}

/// An HTTP response with its body parsed as JSON (`Value::Null` when empty or
/// not JSON; the raw text is kept in `text`).
#[derive(Debug)]
pub struct Response {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Value,
    pub text: String,
}

impl Response {
    /// Panics with the response body unless the status matches.
    #[track_caller]
    pub fn expect_status(self, expected: StatusCode) -> Self {
        assert_eq!(
            self.status, expected,
            "unexpected status; response body: {}",
            self.text
        );
        self
    }

    /// Panics with the response body unless this is an API error with
    /// `status` and `code`.
    #[track_caller]
    pub fn assert_error(&self, status: StatusCode, code: &str) {
        assert_eq!(
            (self.status, self.error_code()),
            (status, Some(code)),
            "unexpected error; response body: {}",
            self.text
        );
    }

    /// The `code` field of an API error response.
    pub fn error_code(&self) -> Option<&str> {
        self.body.get("code").and_then(Value::as_str)
    }

    /// A header's value, when present and ASCII.
    pub fn header(&self, name: impl AsHeaderName) -> Option<&str> {
        self.headers.get(name).and_then(|value| value.to_str().ok())
    }
}

/// A client WebSocket to the app.
pub type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

impl TestApp {
    /// Start a backend on a fresh database.
    pub async fn start() -> Self {
        Self::start_seeded("").await
    }

    /// Start a backend after running `seed_sql` on the migrated database.
    ///
    /// For state the services load once at startup (for example streamer
    /// rows in LIVE or FATAL_ERROR, which `GET /api/streamers` serves from a
    /// cache filled during initialization). Data that handlers read from the
    /// database on each request can be inserted after start with
    /// [`Self::run_sql`] or [`Self::database`] instead.
    pub async fn start_seeded(seed_sql: &str) -> Self {
        Self::boot(seed_sql, None).await
    }

    /// Start a backend that monitors streamers, seeded with `seed` (see
    /// [`Seed::live`]).
    pub async fn start_with(seed: Seed) -> Self {
        let dead_end = ConnectRecorder::start().await;
        let sql = seed.sql(&dead_end);
        Self::boot(&sql, Some(dead_end)).await
    }

    async fn boot(seed_sql: &str, dead_end: Option<ConnectRecorder>) -> Self {
        assert_default_environment();
        backend::install_rustls_provider();
        let data_dir = tempfile::tempdir().expect("create the test data directory");
        let output_dir = data_dir.path().join("output");
        std::fs::create_dir_all(&output_dir).expect("create the test output directory");

        let database_url = format!(
            "sqlite:{}?mode=rwc",
            data_dir.path().join("srec.db").display()
        );
        let migration_pool = backend::init_migration_pool(&database_url)
            .await
            .expect("open the migration pool");
        // Convert proxy settings as if no proxy variables were set, so the
        // fresh database (and with it every outbound route) does not depend on
        // the developer's shell.
        rust_srec::database::run_migrations_with_environment_proxy(&migration_pool, false)
            .await
            .expect("run migrations");
        if !seed_sql.is_empty() {
            sqlx::raw_sql(sqlx::AssertSqlSafe(seed_sql.to_owned()))
                .execute(&migration_pool)
                .await
                .expect("run the seed SQL");
        }
        migration_pool.close().await;
        let (pool, write_pool) = backend::init_database_pools(&database_url)
            .await
            .expect("open the runtime pools");

        let container = ServiceContainer::with_full_config(
            pool,
            write_pool,
            ServiceContainerConfig {
                cache_ttl: Duration::from_secs(3600),
                event_capacity: 256,
                download_config: backend::DownloadManagerConfig::default(),
                pipeline_config: backend::PipelineManagerConfig::default(),
                api_config: ApiServerConfig {
                    bind_address: "127.0.0.1".to_string(),
                    port: 0,
                    enable_cors: true,
                    body_limit: 10 * 1024 * 1024,
                },
            },
        )
        .await
        .expect("build the service container");
        let container = Arc::new(container);

        let config = container.config_service();
        let mut global = config
            .get_global_config()
            .await
            .expect("read the global config");
        global.output_folder = output_dir.to_string_lossy().into_owned();
        config
            .update_global_config(&global)
            .await
            .expect("point recordings at the test output directory");

        let logging = logging();
        logging.apply_persisted_filter(config).await;
        container.set_logging_config(logging);
        container.initialize().await.expect("initialize services");
        let address: SocketAddr = container
            .start_api_server_bound_with_jwt_secret(JWT_SECRET.to_string())
            .await
            .expect("start the API server");

        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .expect("build the HTTP client");

        Self {
            base_url: format!("http://{address}"),
            database_url,
            client,
            container,
            admin: OnceCell::new(),
            output_dir,
            dead_end,
            _data_dir: data_dir,
        }
    }

    /// Stop every service. Call at the end of each test so background tasks do
    /// not outlive the temporary directory.
    pub async fn shutdown(self) {
        self.container
            .shutdown_with_hard_cap(SHUTDOWN_GRACE_PERIOD, SHUTDOWN_TIMEOUT)
            .await
            .expect("backend shutdown failed");
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    /// The dead-end proxy of an app started with [`Self::start_with`].
    pub fn dead_end(&self) -> &ConnectRecorder {
        self.dead_end
            .as_ref()
            .expect("the app was started with a Seed")
    }

    /// A separate connection pool on the app's database, for seeding rows
    /// that no offline API creates and for asserting persisted state.
    pub async fn database(&self) -> sqlx::SqlitePool {
        sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect(&self.database_url)
            .await
            .expect("open a test connection to the app database")
    }

    /// Run SQL statements on the app's database, for rows no offline API
    /// creates. Use [`Self::database`] for statements that bind values.
    pub async fn run_sql(&self, sql: &str) {
        let pool = self.database().await;
        sqlx::raw_sql(sqlx::AssertSqlSafe(sql.to_owned()))
            .execute(&pool)
            .await
            .expect("run SQL on the app database");
        pool.close().await;
    }

    /// The single integer a query on the app's database returns.
    pub async fn query_i64(&self, sql: &str) -> i64 {
        let pool = self.database().await;
        let value = sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(sql.to_owned()))
            .fetch_one(&pool)
            .await
            .expect("query the app database");
        pool.close().await;
        value
    }

    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }

    /// The directory the global config points recordings at.
    pub fn output_dir(&self) -> &Path {
        &self.output_dir
    }

    /// The absolute path of `relative` under [`Self::output_dir`], as the
    /// backend receives it.
    pub fn output_path(&self, relative: &str) -> String {
        self.output_dir
            .join(relative)
            .to_string_lossy()
            .into_owned()
    }

    /// Write `contents` to `relative` under [`Self::output_dir`], creating its
    /// parent directories, and return [`Self::output_path`].
    pub fn write_output_file(&self, relative: &str, contents: impl AsRef<[u8]>) -> String {
        let path = self.output_dir.join(relative);
        std::fs::create_dir_all(path.parent().expect("a file has a parent"))
            .expect("create the file's directory");
        std::fs::write(&path, contents).expect("write the file");
        self.output_path(relative)
    }

    /// Send a request and parse the response.
    pub async fn send(
        &self,
        method: Method,
        path: &str,
        token: Option<&str>,
        body: Option<&Value>,
    ) -> Response {
        let mut request = self.client.request(method, self.url(path));
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request.send().await.expect("send the request");
        Self::read(response).await
    }

    /// Parse a raw `reqwest` response (for requests built with [`Self::client`]).
    pub async fn read(response: reqwest::Response) -> Response {
        let status = response.status();
        let headers = response.headers().clone();
        let text = response.text().await.expect("read the response body");
        let body = serde_json::from_str(&text).unwrap_or(Value::Null);
        Response {
            status,
            headers,
            body,
            text,
        }
    }

    pub async fn get(&self, path: &str, token: &str) -> Response {
        self.send(Method::GET, path, Some(token), None).await
    }

    pub async fn post(&self, path: &str, token: &str, body: &Value) -> Response {
        self.send(Method::POST, path, Some(token), Some(body)).await
    }

    pub async fn put(&self, path: &str, token: &str, body: &Value) -> Response {
        self.send(Method::PUT, path, Some(token), Some(body)).await
    }

    pub async fn patch(&self, path: &str, token: &str, body: &Value) -> Response {
        self.send(Method::PATCH, path, Some(token), Some(body))
            .await
    }

    pub async fn delete(&self, path: &str, token: &str) -> Response {
        self.send(Method::DELETE, path, Some(token), None).await
    }

    /// `POST /api/auth/login`.
    pub async fn login(&self, username: &str, password: &str) -> Response {
        let body = json!({ "username": username, "password": password });
        self.send(Method::POST, "/api/auth/login", None, Some(&body))
            .await
    }

    /// Log in and return the tokens, panicking unless the login succeeds.
    pub async fn login_tokens(&self, username: &str, password: &str) -> Tokens {
        let response = self
            .login(username, password)
            .await
            .expect_status(StatusCode::OK);
        tokens_from(&response.body)
    }

    /// `POST /api/auth/refresh`.
    pub async fn refresh(&self, refresh_token: &str) -> Response {
        let body = json!({ "refresh_token": refresh_token });
        self.send(Method::POST, "/api/auth/refresh", None, Some(&body))
            .await
    }

    /// Tokens for the seeded admin after completing its forced password
    /// change ([`ADMIN_PASSWORD`]). The first call performs the change; later
    /// calls return the same session.
    pub async fn admin(&self) -> &Tokens {
        self.admin
            .get_or_init(|| async {
                let first = self
                    .login(ADMIN_USERNAME, SEEDED_ADMIN_PASSWORD)
                    .await
                    .expect_status(StatusCode::OK);
                assert_eq!(first.body["must_change_password"], true);
                let seeded = tokens_from(&first.body);
                self.post(
                    "/api/auth/change-password",
                    &seeded.access,
                    &json!({
                        "current_password": SEEDED_ADMIN_PASSWORD,
                        "new_password": ADMIN_PASSWORD,
                    }),
                )
                .await
                .expect_status(StatusCode::OK);
                self.login_tokens(ADMIN_USERNAME, ADMIN_PASSWORD).await
            })
            .await
    }

    /// The admin's access token.
    pub async fn admin_token(&self) -> String {
        self.admin().await.access.clone()
    }

    /// Create an API key with `access_level` (`full` or `read_only`).
    pub async fn create_api_key(&self, token: &str, name: &str, access_level: &str) -> ApiKey {
        let created = self
            .post(
                "/api/auth/api-keys",
                token,
                &json!({ "name": name, "access_level": access_level }),
            )
            .await
            .expect_status(StatusCode::OK);
        ApiKey {
            id: created.body["key"]["id"]
                .as_str()
                .expect("key id in the create response")
                .to_owned(),
            raw: created.body["api_key"]
                .as_str()
                .expect("raw key in the create response")
                .to_owned(),
        }
    }

    /// Create a streamer with `enabled: false` added to `body` (its name, URL
    /// and any other fields), so the scheduler never checks its platform, and
    /// return it.
    pub async fn create_disabled_streamer(&self, token: &str, mut body: Value) -> Value {
        body["enabled"] = json!(false);
        let created = self
            .post("/api/streamers", token, &body)
            .await
            .expect_status(StatusCode::CREATED)
            .body;
        assert_eq!(created["state"], "DISABLED", "{created}");
        created
    }

    /// Create an enabled streamer from `body` (its name, URL and any other
    /// fields), so the scheduler starts checking its platform, and return it.
    /// Only for apps started with [`Self::start_with`], whose platforms other
    /// than the fake one reach only the dead-end proxy.
    pub async fn create_enabled_streamer(&self, token: &str, mut body: Value) -> Value {
        assert!(
            self.dead_end.is_some(),
            "monitor streamers only in an app started with a Seed"
        );
        body["enabled"] = json!(true);
        let created = self
            .post("/api/streamers", token, &body)
            .await
            .expect_status(StatusCode::CREATED)
            .body;
        assert_eq!(created["state"], "NOT_LIVE", "{created}");
        created
    }

    /// Make a cookie account the selection on `platform_id`, then disable it,
    /// so the platform's only account is unusable and a managed parse there
    /// fails before any request. Cookie-only platforms make no request to
    /// create, select or disable an account.
    pub async fn select_disabled_account(&self, token: &str, platform_id: &str) {
        let profile = self
            .post(
                "/api/credentials/profiles",
                token,
                &json!({
                    "platform_id": platform_id,
                    "label": format!("{platform_id} account"),
                    "enabled": true,
                    "material": {
                        "cookies": "session=e2e",
                        "refresh_token": null,
                        "access_token": null,
                        "reauth_config": null,
                    },
                }),
            )
            .await
            .expect_status(StatusCode::OK);
        let profile_id = id_of(&profile.body);

        let path = format!("/api/config/platforms/{platform_id}");
        let mut platform = self
            .get(&path, token)
            .await
            .expect_status(StatusCode::OK)
            .body;
        // The platform route carries the selection as a JSON document in a string.
        platform["credential_selection"] =
            json!(json!({ "mode": "fixed", "credential_id": profile_id }).to_string());
        self.put(&path, token, &platform)
            .await
            .expect_status(StatusCode::OK);

        self.patch(
            &format!("/api/credentials/profiles/{profile_id}"),
            token,
            &json!({ "expected_version": profile.body["version"], "enabled": false }),
        )
        .await
        .expect_status(StatusCode::OK);
    }

    /// Open a WebSocket to `path`, sending `bearer` as an `Authorization`
    /// header; panics with the response if the server refuses the upgrade.
    pub async fn connect_socket(&self, path: &str, bearer: Option<&str>) -> Socket {
        match self.upgrade(path, bearer).await {
            Ok(socket) => socket,
            Err(refused) => panic!("the server refused the upgrade to {path}: {refused:?}"),
        }
    }

    /// The response to a WebSocket upgrade the server must refuse.
    pub async fn refused_socket(&self, path: &str, bearer: Option<&str>) -> Response {
        match self.upgrade(path, bearer).await {
            Ok(_) => panic!("the server accepted the upgrade to {path}"),
            Err(refused) => *refused,
        }
    }

    async fn upgrade(&self, path: &str, bearer: Option<&str>) -> Result<Socket, Box<Response>> {
        let mut request = format!("ws://{}{path}", self.base_url.trim_start_matches("http://"))
            .into_client_request()
            .expect("valid WebSocket request");
        if let Some(bearer) = bearer {
            request.headers_mut().insert(
                reqwest::header::AUTHORIZATION,
                format!("Bearer {bearer}")
                    .parse()
                    .expect("valid Authorization header"),
            );
        }
        match tokio::time::timeout(WAIT, tokio_tungstenite::connect_async(request))
            .await
            .expect("WebSocket handshake timed out")
        {
            Ok((socket, _)) => Ok(socket),
            Err(tungstenite::Error::Http(response)) => {
                let text = String::from_utf8_lossy(response.body().as_deref().unwrap_or_default())
                    .into_owned();
                Err(Box::new(Response {
                    status: response.status(),
                    headers: response.headers().clone(),
                    body: serde_json::from_str(&text).unwrap_or(Value::Null),
                    text,
                }))
            }
            Err(error) => panic!("WebSocket handshake failed: {error}"),
        }
    }
}

/// Wait until the server drops `socket`, skipping frames still in flight.
pub async fn assert_closes(socket: &mut Socket) {
    tokio::time::timeout(WAIT, async {
        loop {
            match socket.next().await {
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return,
                Some(Ok(_)) => continue,
            }
        }
    })
    .await
    .expect("the server kept the socket open");
}

pub fn tokens_from(body: &Value) -> Tokens {
    Tokens {
        access: body["access_token"]
            .as_str()
            .expect("access_token in login response")
            .to_string(),
        refresh: body["refresh_token"]
            .as_str()
            .expect("refresh_token in login response")
            .to_string(),
    }
}

/// The `id` of a created resource.
#[track_caller]
pub fn id_of(body: &Value) -> String {
    body["id"]
        .as_str()
        .unwrap_or_else(|| panic!("no id in {body}"))
        .to_string()
}

/// The keys of a JSON object, sorted.
#[track_caller]
pub fn sorted_keys(value: &Value) -> Vec<&str> {
    let mut keys: Vec<&str> = value
        .as_object()
        .unwrap_or_else(|| panic!("expected a JSON object, got {value}"))
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    keys
}

/// `path` with `pairs` appended as a URL-encoded query string.
pub fn with_query(path: &str, pairs: &[(&str, &str)]) -> String {
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    for (name, value) in pairs {
        query.append_pair(name, value);
    }
    format!("{path}?{}", query.finish())
}

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Panics unless `ms`, in epoch milliseconds, is within a minute of now. The
/// backend stamps times on its own clock and a loaded machine can delay a
/// response by seconds, so "now" is checked with this tolerance.
#[track_caller]
pub fn assert_recent(ms: i64) {
    assert!(
        (now_ms() - ms).abs() < 60_000,
        "{ms} is not within a minute of now"
    );
}

/// Epoch milliseconds of an RFC 3339 timestamp in a response.
#[track_caller]
pub fn millis(value: &Value) -> i64 {
    let text = value
        .as_str()
        .unwrap_or_else(|| panic!("expected an RFC 3339 string, got {value}"));
    chrono::DateTime::parse_from_rfc3339(text)
        .unwrap_or_else(|error| panic!("{text} is not RFC 3339: {error}"))
        .timestamp_millis()
}

/// Poll `check` until it returns `Some`, failing after `timeout`.
pub async fn eventually<T, F, Fut>(timeout: Duration, what: &str, mut check: F) -> T
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Option<T>>,
{
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if let Some(value) = check().await {
            return value;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out after {timeout:?} waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}
