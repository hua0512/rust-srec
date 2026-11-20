//! Weibo web sign-in by QR code, and the check of a signed-in account.
//!
//! Confirming a QR code in the Weibo app gives the login page an address on
//! `passport.weibo.com`. That address starts a chain of redirects through
//! `login.sina.com.cn` and `passport.weibo.cn` back to `weibo.com`, and each
//! hop sets its own site's cookies. The client used here must not follow
//! redirects: the walk below follows them itself, so it can keep every hop's
//! `Set-Cookie` and send each hop its `Referer`.
//!
//! A login yields the `.weibo.com` cookies, which the extractor uses, and an account token holding every cookie of the chain
//! with its domain and expiry, which can sign the account in again later.
//!
//! Cookies, the account token, QR ids and sign-in addresses are credentials:
//! they never appear in errors or logs, and request errors drop their URL.

use std::sync::LazyLock;

use regex::Regex;
use reqwest::header::{self, LOCATION};
use reqwest::{Client, Response};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::debug;
use url::Url;

use super::builder::{check_gateway, check_throttle, login_failure};
use super::jar::{Jar, on_site};
use crate::extractor::default::DEFAULT_UA;
use crate::extractor::error::ExtractorError;

const RETCODE_OK: i64 = 20_000_000;
const QR_NOT_SCANNED: i64 = 50_114_001;
const QR_SCANNED: i64 = 50_114_002;
const QR_TIMED_OUT: i64 = 50_114_003;
const QR_USED: i64 = 50_114_004;
const QR_ABNORMAL: i64 = 50_114_015;
/// The sign-in chain has four hops; more means Weibo changed it or loops.
const MAX_HOPS: usize = 10;
/// Sites the sign-in chain may visit. A redirect elsewhere is refused, so
/// the chain's tickets never leave Weibo.
const SIGN_IN_SITES: [&str; 3] = ["weibo.com", "weibo.cn", "sina.com.cn"];
/// The host the chain ends on.
const MAIN_HOST: &str = "weibo.com";
/// The domain of the cookies the extractor sends.
const SESSION_DOMAIN: &str = ".weibo.com";
const SESSION_COOKIE: &str = "SUB";
const TOKEN_VERSION: u8 = 1;

/// The account's uid in the user config `weibo.com` embeds in its page.
static PAGE_IDSTR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""idstr"\s*:\s*"(\d+)""#).unwrap());
static PAGE_UID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""uid"\s*:\s*"?(\d+)"#).unwrap());

#[derive(Debug, Error)]
pub enum PassportError {
    /// A throttle or another response Weibo turned down.
    #[error("Weibo passport response: {0}")]
    Response(ExtractorError),
    #[error("Weibo passport request failed: {0}")]
    Network(reqwest::Error),
    #[error("Weibo passport returned an unexpected response: {0}")]
    Parse(&'static str),
    /// Weibo answered with an error code.
    #[error("Weibo passport error {0}")]
    Api(i64),
}

fn network(error: reqwest::Error) -> PassportError {
    PassportError::Network(error.without_url())
}

/// Throttles (429, 418 or `Retry-After`) first, then any other status that
/// is not success.
fn checked(response: Response) -> Result<Response, PassportError> {
    check_throttle(&response).map_err(PassportError::Response)?;
    ExtractorError::check_response(response).map_err(|error| match error {
        ExtractorError::HttpError(error) => network(error),
        error => PassportError::Response(error),
    })
}

fn now() -> i64 {
    jiff::Timestamp::now().as_second()
}

/// Where the passport and the main site are; tests point both at a local server.
#[derive(Clone)]
struct Hosts {
    passport: String,
    main: String,
}

impl Hosts {
    fn weibo() -> Self {
        Self {
            passport: "https://passport.weibo.com".into(),
            main: "https://weibo.com".into(),
        }
    }
}

/// A QR code to show, and the id to poll it with.
pub struct QrCode {
    /// The QR code's content: the address the Weibo app opens.
    pub url: String,
    pub qrid: String,
}

impl std::fmt::Debug for QrCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QrCode").finish_non_exhaustive()
    }
}

/// A signed-in account.
pub struct Session {
    /// The `.weibo.com` cookies, which the extractor sends. Cookies only
    /// `weibo.com` itself receives, such as its one-day `WBPSESS`, stay in
    /// the account token with their expiry, so a stale one is never sent.
    pub cookies: String,
    /// The account token: the account's uid and every cookie of the sign-in
    /// chain, with its domain and expiry.
    pub account_token: String,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session").finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub enum QrStatus {
    NotScanned,
    /// Scanned; waiting for the user to confirm in the app.
    Scanned,
    /// Timed out, already used, or refused by Weibo.
    Expired,
    Confirmed(Session),
}

/// What Weibo's post timeline says about an account's cookies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountState {
    Valid,
    /// Weibo redirected to its sign-in or visitor page: the login expired,
    /// or the cookies carry none.
    SignedOut,
    /// The account's uid is unknown, so there is nothing to check.
    Unknown,
}

/// The stored form of a signed-in account, versioned so later releases can
/// still read it.
#[derive(Serialize, Deserialize)]
struct AccountToken {
    v: u8,
    uid: String,
    jar: Jar,
}

impl AccountToken {
    /// Reads a token, without the cookies that have expired since it was
    /// stored.
    fn read(token: &str, now: i64) -> Option<Self> {
        let mut account: Self = serde_json::from_str(token).ok()?;
        if account.v != TOKEN_VERSION || account.uid.is_empty() {
            return None;
        }
        account.jar.drop_expired(now);
        Some(account)
    }
}

/// The uid of the account an account token belongs to.
pub fn account_uid(account_token: &str) -> Option<String> {
    AccountToken::read(account_token, now()).map(|account| account.uid)
}

/// `retcode` and `data` of a passport response; Weibo sends the code as a
/// number or a string.
fn envelope(body: &str) -> Result<(i64, serde_json::Value), PassportError> {
    let mut value: serde_json::Value =
        serde_json::from_str(body).map_err(|_| PassportError::Parse("passport response"))?;
    let retcode = value
        .get("retcode")
        .and_then(|code| {
            code.as_i64()
                .or_else(|| code.as_str().and_then(|code| code.trim().parse().ok()))
        })
        .ok_or(PassportError::Parse("passport response without a code"))?;
    Ok((retcode, value["data"].take()))
}

/// The address a QR image encodes, from the `data` parameter of the image
/// address (`//v2.qr.weibo.cn/inf/gen?...&data=<address>`).
fn scan_address(image: &str) -> Option<String> {
    let image = match image.strip_prefix("//") {
        Some(rest) => Url::parse(&format!("https://{rest}")),
        None => Url::parse(image),
    }
    .ok()?;
    image
        .query_pairs()
        .find(|(key, _)| key == "data")
        .map(|(_, address)| address.into_owned())
        .filter(|address| !address.is_empty())
}

/// Starts a QR login.
pub async fn generate_qr(client: &Client) -> Result<QrCode, PassportError> {
    generate_qr_at(client, &Hosts::weibo()).await
}

async fn generate_qr_at(client: &Client, hosts: &Hosts) -> Result<QrCode, PassportError> {
    let response = client
        .get(format!("{}/sso/v2/qrcode/image", hosts.passport))
        .query(&[("entry", "weibo"), ("size", "180")])
        .header(header::USER_AGENT, DEFAULT_UA)
        .header(header::REFERER, format!("{}/", hosts.main))
        .send()
        .await
        .map_err(network)?;
    let body = checked(response)?.text().await.map_err(network)?;
    let (retcode, data) = envelope(&body)?;
    if retcode != RETCODE_OK {
        return Err(PassportError::Api(retcode));
    }
    let field = |name: &str| {
        data.get(name)
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.is_empty())
    };
    let qrid = field("qrid").ok_or(PassportError::Parse("QR code response without an id"))?;
    let url = field("image")
        .and_then(scan_address)
        .ok_or(PassportError::Parse("QR code response without an address"))?;
    Ok(QrCode {
        url,
        qrid: qrid.to_owned(),
    })
}

/// Polls a QR login; a confirmed login is followed through the sign-in
/// chain. `client` must not follow redirects.
pub async fn poll_qr(client: &Client, qrid: &str) -> Result<QrStatus, PassportError> {
    poll_qr_at(client, &Hosts::weibo(), qrid).await
}

async fn poll_qr_at(client: &Client, hosts: &Hosts, qrid: &str) -> Result<QrStatus, PassportError> {
    let response = client
        .get(format!("{}/sso/v2/qrcode/check", hosts.passport))
        .query(&[("entry", "weibo"), ("qrid", qrid)])
        .header(header::USER_AGENT, DEFAULT_UA)
        .header(header::REFERER, format!("{}/", hosts.main))
        .send()
        .await
        .map_err(network)?;
    let response = checked(response)?;
    let mut jar = Jar::default();
    jar.store(response.url(), response.headers(), now());
    let body = response.text().await.map_err(network)?;
    let (retcode, data) = envelope(&body)?;
    debug!(retcode, "Weibo QR status");
    match retcode {
        QR_NOT_SCANNED => return Ok(QrStatus::NotScanned),
        QR_SCANNED => return Ok(QrStatus::Scanned),
        QR_TIMED_OUT | QR_USED | QR_ABNORMAL => return Ok(QrStatus::Expired),
        RETCODE_OK => {}
        code => return Err(PassportError::Api(code)),
    }
    let start = data
        .get("url")
        .and_then(serde_json::Value::as_str)
        .and_then(|url| Url::parse(url).ok())
        .ok_or(PassportError::Parse(
            "confirmed login without a sign-in address",
        ))?;
    let referer = format!("{}/", hosts.passport);
    sign_in(client, start, referer, jar)
        .await
        .map(QrStatus::Confirmed)
}

fn on_sign_in_site(url: &Url) -> bool {
    url.host_str()
        .is_some_and(|host| SIGN_IN_SITES.iter().any(|site| on_site(host, site)))
}

/// The uid in the user config of a `weibo.com` page: `idstr`, else `uid`.
fn page_uid(page: &str) -> Option<String> {
    [&PAGE_IDSTR, &PAGE_UID].into_iter().find_map(|pattern| {
        pattern
            .captures(page)
            .and_then(|captures| captures.get(1))
            .map(|uid| uid.as_str().to_owned())
    })
}

/// Walks the redirects from `start` to `weibo.com` as a browser would: each
/// hop is sent the jar's cookies for its host and the previous hop's origin
/// as `Referer`, and its `Set-Cookie` headers go into the jar.
/// `login.sina.com.cn` answers an empty 403 to a hop without a `Referer`.
async fn sign_in(
    client: &Client,
    start: Url,
    mut referer: String,
    mut jar: Jar,
) -> Result<Session, PassportError> {
    let mut url = start;
    for _ in 0..MAX_HOPS {
        if !on_sign_in_site(&url) {
            return Err(PassportError::Parse("sign-in redirected away from Weibo"));
        }
        let mut request = client
            .get(url.clone())
            .header(header::USER_AGENT, DEFAULT_UA)
            .header(header::REFERER, &referer);
        if let Some(cookies) = jar.header_for(&url, now()) {
            request = request.header(header::COOKIE, cookies);
        }
        let response = request.send().await.map_err(network)?;
        check_throttle(&response).map_err(PassportError::Response)?;
        jar.store(&url, response.headers(), now());
        if response.status().is_redirection() {
            let next = response
                .headers()
                .get(LOCATION)
                .and_then(|location| location.to_str().ok())
                .and_then(|location| url.join(location).ok())
                .ok_or(PassportError::Parse("sign-in redirect without an address"))?;
            referer = format!("{}/", url.origin().ascii_serialization());
            url = next;
            continue;
        }
        let page = checked(response)?.text().await.map_err(network)?;
        if url.host_str() != Some(MAIN_HOST) {
            return Err(PassportError::Parse("sign-in ended away from weibo.com"));
        }
        if !jar.contains(SESSION_DOMAIN, SESSION_COOKIE, now()) {
            return Err(PassportError::Parse("sign-in returned no session"));
        }
        let uid = page_uid(&page).ok_or(PassportError::Parse("sign-in page without a user id"))?;
        let cookies = jar
            .domain_header(SESSION_DOMAIN, now())
            .ok_or(PassportError::Parse("sign-in returned no session"))?;
        let account_token = serde_json::to_string(&AccountToken {
            v: TOKEN_VERSION,
            uid,
            jar,
        })
        .map_err(|_| PassportError::Parse("sign-in could not be stored"))?;
        return Ok(Session {
            cookies,
            account_token,
        });
    }
    Err(PassportError::Parse("sign-in redirected too many times"))
}

/// Checks `cookies` on the post timeline of the account in `account_token`,
/// or, without one, of the account `weibo.com` shows them signed in as.
/// Only a redirect to the sign-in or visitor page means the account is
/// signed out; a 403 or a gateway check is an error. `client` must not
/// follow redirects.
pub async fn check_account(
    client: &Client,
    cookies: &str,
    account_token: Option<&str>,
) -> Result<AccountState, PassportError> {
    check_account_at(client, &Hosts::weibo(), cookies, account_token).await
}

async fn check_account_at(
    client: &Client,
    hosts: &Hosts,
    cookies: &str,
    account_token: Option<&str>,
) -> Result<AccountState, PassportError> {
    let uid = match account_token.and_then(account_uid) {
        Some(uid) => uid,
        None => match home_uid(client, hosts, cookies).await? {
            Some(uid) => uid,
            None => return Ok(AccountState::Unknown),
        },
    };
    let api = format!("{}/ajax/statuses/mymblog", hosts.main);
    let response = client
        .get(&api)
        .query(&[("uid", uid.as_str()), ("page", "1"), ("feature", "0")])
        .header(header::USER_AGENT, DEFAULT_UA)
        .header(header::REFERER, format!("{}/u/{uid}", hosts.main))
        .header(header::COOKIE, cookies)
        .send()
        .await
        .map_err(network)?;
    check_throttle(&response).map_err(PassportError::Response)?;
    if login_failure(&response, &api).is_some() {
        return Ok(AccountState::SignedOut);
    }
    let body = checked(response)?.text().await.map_err(network)?;
    let value: serde_json::Value =
        serde_json::from_str(&body).map_err(|_| PassportError::Parse("account check response"))?;
    check_gateway(value.get("code").and_then(serde_json::Value::as_i64))
        .map_err(PassportError::Response)?;
    match value.get("ok").and_then(serde_json::Value::as_i64) {
        Some(1) => Ok(AccountState::Valid),
        Some(code) => Err(PassportError::Api(code)),
        None => Err(PassportError::Parse(
            "account check response without a result",
        )),
    }
}

/// The uid `weibo.com` shows `cookies` signed in as. A redirect, such as to
/// the visitor page, shows none.
async fn home_uid(
    client: &Client,
    hosts: &Hosts,
    cookies: &str,
) -> Result<Option<String>, PassportError> {
    let response = client
        .get(format!("{}/", hosts.main))
        .header(header::USER_AGENT, DEFAULT_UA)
        .header(header::COOKIE, cookies)
        .send()
        .await
        .map_err(network)?;
    check_throttle(&response).map_err(PassportError::Response)?;
    if response.status().is_redirection() {
        return Ok(None);
    }
    let page = checked(response)?.text().await.map_err(network)?;
    Ok(page_uid(&page))
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;
    use std::time::Duration;

    use super::*;
    use crate::extractor::default::create_client_builder;
    use crate::extractor::test_support::{ReceivedRequests, Reply, serve_in_order};

    const SIGN_IN_PAGE: &str = r#"<script>window.$CONFIG = {"user":{"id":1000000002,"idstr":"1000000002","screen_name":"Account"}};</script>"#;

    /// A client that does not follow redirects and resolves Weibo's hosts to
    /// a local server answering `replies` in order, and the hosts to start at.
    async fn local_passport(replies: Vec<Reply>) -> (Client, Hosts, ReceivedRequests) {
        let (base, received) = serve_in_order(replies).await;
        let addr: SocketAddr = base
            .trim_start_matches("http://")
            .trim_end_matches('/')
            .parse()
            .unwrap();
        let mut builder = create_client_builder(None)
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none());
        for host in [
            "passport.weibo.com",
            "login.sina.com.cn",
            "passport.weibo.cn",
            "weibo.com",
        ] {
            builder = builder.resolve(host, addr);
        }
        let hosts = Hosts {
            passport: format!("http://passport.weibo.com:{}", addr.port()),
            main: format!("http://weibo.com:{}", addr.port()),
        };
        (builder.build().unwrap(), hosts, received)
    }

    async fn bounded<T>(future: impl std::future::Future<Output = T>) -> T {
        tokio::time::timeout(Duration::from_secs(10), future)
            .await
            .expect("local passport request timed out")
    }

    fn request(received: &ReceivedRequests, index: usize) -> String {
        received.lock().unwrap()[index].clone()
    }

    fn header_of(request: &str, name: &str) -> Option<String> {
        request.lines().find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case(name)
                .then(|| value.trim().to_owned())
        })
    }

    fn redirect(location: &'static str) -> Reply {
        Reply::raw("302 Found", "text/html", "").header("location", location)
    }

    fn page(body: &'static str) -> Reply {
        Reply::raw("200 OK", "text/html", body)
    }

    #[tokio::test]
    async fn a_qr_code_shows_the_address_its_image_encodes() {
        let (client, hosts, received) = local_passport(vec![
            Reply::json(
                "200 OK",
                r#"{"retcode":20000000,"msg":"succ","data":{"qrid":"QRID-test","image":"//v2.qr.weibo.cn/inf/gen?api_key=k&data=https%3A%2F%2Fpassport.weibo.cn%2Fsignin%2Fqrcode%2Fscan%3Fqr%3DQRID-test%26showmenu%3D0&size=180"}}"#,
            ),
            Reply::json("200 OK", r#"{"retcode":50114099,"msg":"private"}"#),
        ])
        .await;
        let qr = bounded(generate_qr_at(&client, &hosts)).await.unwrap();
        assert_eq!(qr.qrid, "QRID-test");
        assert_eq!(
            qr.url,
            "https://passport.weibo.cn/signin/qrcode/scan?qr=QRID-test&showmenu=0"
        );
        let sent = request(&received, 0);
        assert!(sent.starts_with("GET /sso/v2/qrcode/image?entry=weibo&size=180 "));
        assert_eq!(
            header_of(&sent, "referer"),
            Some(format!("{}/", hosts.main))
        );

        let error = bounded(generate_qr_at(&client, &hosts)).await.unwrap_err();
        assert!(matches!(error, PassportError::Api(50_114_099)));
        assert!(!error.to_string().contains("private"));
    }

    #[tokio::test]
    async fn qr_codes_report_waiting_scanned_and_expired_without_signing_in() {
        let (client, hosts, received) = local_passport(vec![
            Reply::json("200 OK", r#"{"retcode":50114001,"msg":"","data":null}"#),
            Reply::json("200 OK", r#"{"retcode":"50114002","msg":"","data":null}"#),
            Reply::json("200 OK", r#"{"retcode":50114003,"msg":"","data":null}"#),
            Reply::json("200 OK", r#"{"retcode":50114004,"msg":"","data":null}"#),
            Reply::json("200 OK", r#"{"retcode":50114015,"msg":"","data":null}"#),
            Reply::json("200 OK", r#"{"retcode":50114099,"msg":"","data":null}"#),
        ])
        .await;
        for expected in ["NotScanned", "Scanned", "Expired", "Expired", "Expired"] {
            let status = bounded(poll_qr_at(&client, &hosts, "QRID-test"))
                .await
                .unwrap();
            assert_eq!(format!("{status:?}"), expected);
        }
        assert!(matches!(
            bounded(poll_qr_at(&client, &hosts, "QRID-test")).await,
            Err(PassportError::Api(50_114_099))
        ));
        assert!(request(&received, 0).contains("qrid=QRID-test"));
        assert_eq!(received.lock().unwrap().len(), 6);
    }

    #[tokio::test]
    async fn a_confirmed_login_walks_the_sign_in_chain_like_a_browser() {
        let (client, hosts, received) = local_passport(vec![
            Reply::json(
                "200 OK",
                r#"{"retcode":20000000,"msg":"succ","data":{"url":"http://passport.weibo.com:{port}/sso/v2/login?entry=weibo&type=3&alt=ALT-test&url=https%3A%2F%2Fweibo.com"}}"#,
            ),
            redirect("http://login.sina.com.cn:{port}/sso/v2/crossdomain?ticket=ST-test1")
                .header(
                    "set-cookie",
                    "SUB=com-sub; expires=Sat, 20-Nov-2027 15:58:26 GMT; path=/; domain=.weibo.com; httponly",
                )
                .header("set-cookie", "SUBP=com-subp; Max-Age=31536000; path=/; domain=.weibo.com")
                .header("set-cookie", "ALF=com-alf; Max-Age=2592000; path=/; domain=.weibo.com")
                .header("set-cookie", "ALC=passport-alc; Max-Age=2592000; path=/"),
            redirect("http://passport.weibo.cn:{port}/sso/crossdomain?ticket=ST-test2")
                .header("set-cookie", "SUB=sina-sub; Max-Age=31536000; path=/; domain=.sina.com.cn")
                .header("set-cookie", "SUBP=sina-subp; Max-Age=604800; path=/; domain=.sina.com.cn")
                .header("set-cookie", "ALC=sina-alc; Max-Age=2592000; path=/"),
            redirect("http://weibo.com:{port}/")
                .header("set-cookie", "SUB=cn-sub; Max-Age=31536000; path=/; domain=.weibo.cn")
                .header("set-cookie", "SSOLoginState=1700000000; path=/; domain=.weibo.cn")
                .header(
                    "set-cookie",
                    "SUHB=deleted; expires=Thu, 01-Jan-1970 00:00:01 GMT; path=/; domain=.weibo.cn",
                ),
            page(SIGN_IN_PAGE)
                .header("set-cookie", "WBPSESS=main-session; Max-Age=86400; path=/")
                .header("set-cookie", "XSRF-TOKEN=xsrf; path=/"),
        ])
        .await;
        let QrStatus::Confirmed(session) = bounded(poll_qr_at(&client, &hosts, "QRID-test"))
            .await
            .unwrap()
        else {
            panic!("the login was confirmed");
        };
        assert_eq!(session.cookies, "SUB=com-sub; SUBP=com-subp; ALF=com-alf");
        assert!(!format!("{session:?}").contains("com-sub"));
        assert_eq!(
            account_uid(&session.account_token).as_deref(),
            Some("1000000002")
        );

        // Each hop names the previous hop's origin, and receives only its
        // own site's cookies.
        let port = hosts.main.rsplit(':').next().unwrap().to_owned();
        let hops = [
            ("/sso/v2/login?", "passport.weibo.com", None),
            ("/sso/v2/crossdomain?", "passport.weibo.com", None),
            ("/sso/crossdomain?", "login.sina.com.cn", None),
            (
                "/ ",
                "passport.weibo.cn",
                Some("SUB=com-sub; SUBP=com-subp; ALF=com-alf"),
            ),
        ];
        for (index, (path, referer_host, cookies)) in hops.into_iter().enumerate() {
            let sent = request(&received, index + 1);
            assert!(sent.starts_with(&format!("GET {path}")), "{sent}");
            assert_eq!(
                header_of(&sent, "referer"),
                Some(format!("http://{referer_host}:{port}/"))
            );
            assert_eq!(header_of(&sent, "cookie").as_deref(), cookies);
        }

        let token: serde_json::Value = serde_json::from_str(&session.account_token).unwrap();
        assert_eq!(token["v"], 1);
        let stored = |domain: &str, name: &str| {
            token["jar"]
                .as_array()
                .unwrap()
                .iter()
                .find(|cookie| cookie["domain"] == domain && cookie["name"] == name)
                .cloned()
        };
        assert_eq!(
            stored(".weibo.com", "SUB").unwrap()["expires"],
            1_826_726_306
        );
        assert_eq!(stored(".sina.com.cn", "SUB").unwrap()["value"], "sina-sub");
        assert_eq!(stored(".weibo.cn", "SUB").unwrap()["value"], "cn-sub");
        assert_eq!(
            stored("passport.weibo.com", "ALC").unwrap()["value"],
            "passport-alc"
        );
        assert_eq!(
            stored("login.sina.com.cn", "ALC").unwrap()["value"],
            "sina-alc"
        );
        let subp_expires = stored(".sina.com.cn", "SUBP").unwrap()["expires"]
            .as_i64()
            .unwrap();
        assert!((subp_expires - (now() + 604_800)).abs() < 60);
        assert!(
            stored(".weibo.cn", "SSOLoginState")
                .unwrap()
                .get("expires")
                .is_none()
        );
        assert!(stored(".weibo.cn", "SUHB").is_none());
        assert!(stored("weibo.com", "WBPSESS").unwrap()["expires"].is_i64());
    }

    #[tokio::test]
    async fn a_sign_in_without_a_uid_or_leaving_weibo_fails() {
        let (client, hosts, received) = local_passport(vec![
            Reply::json(
                "200 OK",
                r#"{"retcode":20000000,"data":{"url":"http://passport.weibo.com:{port}/sso/v2/login?alt=ALT-test"}}"#,
            ),
            redirect("http://weibo.com:{port}/")
                .header("set-cookie", "SUB=com-sub; path=/; domain=.weibo.com"),
            page("<html>no user config</html>"),
            Reply::json(
                "200 OK",
                r#"{"retcode":20000000,"data":{"url":"http://passport.weibo.com:{port}/sso/v2/login?alt=ALT-test"}}"#,
            ),
            redirect("http://127.0.0.1:{port}/collect?alt=ALT-test"),
        ])
        .await;
        assert!(matches!(
            bounded(poll_qr_at(&client, &hosts, "QRID-test")).await,
            Err(PassportError::Parse("sign-in page without a user id"))
        ));
        assert!(matches!(
            bounded(poll_qr_at(&client, &hosts, "QRID-test")).await,
            Err(PassportError::Parse("sign-in redirected away from Weibo"))
        ));
        assert_eq!(received.lock().unwrap().len(), 5);
    }

    #[tokio::test]
    async fn only_sign_in_redirects_mean_an_account_is_signed_out() {
        const COOKIES: &str = "SUB=com-sub; SUBP=com-subp";
        let token = r#"{"v":1,"uid":"1000000003","jar":[]}"#;
        let (client, hosts, received) = local_passport(vec![
            Reply::json("200 OK", r#"{"ok":1,"data":{"list":[]}}"#),
            redirect("http://weibo.com:{port}/login.php?url=x").header("x-login-failed", "true"),
            redirect("http://passport.weibo.com:{port}/visitor/visitor?entry=miniblog&a=enter"),
            Reply::raw("403 Forbidden", "text/html", "<h2>403 Forbidden</h2>"),
            Reply::json("200 OK", r#"{"code":100004,"msg":"SinaSec check failure"}"#),
            Reply::json("429 Too Many Requests", "{}").header("retry-after", "120"),
            // Without an account token the uid comes from the home page.
            page(SIGN_IN_PAGE),
            Reply::json("200 OK", r#"{"ok":1,"data":{"list":[]}}"#),
            redirect("http://passport.weibo.com:{port}/visitor/visitor?a=enter"),
        ])
        .await;
        let check = |token| bounded(check_account_at(&client, &hosts, COOKIES, token));
        assert_eq!(check(Some(token)).await.unwrap(), AccountState::Valid);
        let sent = request(&received, 0);
        assert!(sent.starts_with("GET /ajax/statuses/mymblog?uid=1000000003&page=1&feature=0 "));
        assert_eq!(header_of(&sent, "cookie").as_deref(), Some(COOKIES));
        assert_eq!(
            header_of(&sent, "referer"),
            Some(format!("{}/u/1000000003", hosts.main))
        );
        assert_eq!(check(Some(token)).await.unwrap(), AccountState::SignedOut);
        assert_eq!(check(Some(token)).await.unwrap(), AccountState::SignedOut);
        assert!(matches!(
            check(Some(token)).await,
            Err(PassportError::Network(_))
        ));
        assert!(matches!(
            check(Some(token)).await,
            Err(PassportError::Response(ExtractorError::ValidationError(_)))
        ));
        assert!(matches!(
            check(Some(token)).await,
            Err(PassportError::Response(ExtractorError::RateLimited {
                retry_after: Some(delay),
                ..
            })) if delay == Duration::from_secs(120)
        ));

        assert_eq!(check(None).await.unwrap(), AccountState::Valid);
        assert!(request(&received, 6).starts_with("GET / "));
        assert!(request(&received, 7).contains("uid=1000000002"));
        assert_eq!(check(None).await.unwrap(), AccountState::Unknown);
        assert_eq!(received.lock().unwrap().len(), 9);
    }

    #[test]
    fn the_uid_comes_from_idstr_else_uid() {
        assert_eq!(
            page_uid(r#"{"uid":"1000000005","user":{"idstr":"1000000004"}}"#).as_deref(),
            Some("1000000004")
        );
        assert_eq!(
            page_uid(r#"$CONFIG = {"uid": 1000000005};"#).as_deref(),
            Some("1000000005")
        );
        assert_eq!(page_uid(r#"{"user":{}}"#), None);
    }
}
