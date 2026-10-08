//! Douyu passport sign-in: QR code login and renewal of the main-site session.
//!
//! Douyu keeps two sessions. Confirming a QR code in the Douyu app makes the
//! passport (`passport.douyu.com`) issue `LTP0`, a credential that lasts for
//! months. The main site (`www.douyu.com`) session, `acf_uid` and `acf_auth`,
//! lasts about six days; `safeAuth` issues a new one from `LTP0` and the device
//! ID (`dy_did`) the login was made with, without the user.
//!
//! Cookies, `LTP0`, scan codes and exchange URLs are credentials: they never
//! appear in errors or logs, and request errors drop their URL.

use reqwest::header::{self, HeaderMap};
use reqwest::{Client, Response};
use serde::Deserialize;
use thiserror::Error;
use tracing::debug;

use crate::extractor::default::DEFAULT_UA;
use crate::extractor::error::ExtractorError;
use crate::extractor::utils::parse_cookie_header;

/// The passport credential that renews the main-site session.
pub const PASSPORT_CREDENTIAL_COOKIE: &str = "LTP0";
/// The web device ID a login and its renewals are bound to.
pub const DEVICE_ID_COOKIE: &str = "dy_did";
const APP_DEVICE_ID_COOKIE: &str = "acf_did";
const SESSION_COOKIE: &str = "acf_auth";
const UID_COOKIE: &str = "acf_uid";
const MAIN_LOGIN_PATH: &str = "/api/passport/login";

#[derive(Debug, Error)]
pub enum PassportError {
    /// A throttle or another HTTP status that is not success.
    #[error("Douyu passport response: {0}")]
    Response(ExtractorError),
    #[error("Douyu passport request failed: {0}")]
    Network(reqwest::Error),
    #[error("Douyu passport returned an unexpected response: {0}")]
    Parse(&'static str),
    /// Douyu answered with an error code.
    #[error("Douyu passport error {0}")]
    Api(i64),
    /// The renewed session belongs to another account than the stored one.
    #[error("Douyu renewed the session of a different account")]
    AccountMismatch,
}

fn network(error: reqwest::Error) -> PassportError {
    PassportError::Network(error.without_url())
}

fn checked(response: Response) -> Result<Response, PassportError> {
    ExtractorError::check_response(response).map_err(|error| match error {
        ExtractorError::HttpError(error) => network(error),
        error => PassportError::Response(error),
    })
}

/// Where the passport and the main site are; tests point both at a local server.
#[derive(Clone)]
struct Hosts {
    passport: String,
    main: String,
}

impl Hosts {
    fn douyu() -> Self {
        Self {
            passport: "https://passport.douyu.com".into(),
            main: "https://www.douyu.com".into(),
        }
    }

    fn login_page(&self) -> String {
        format!("{}/index/login?type=login&client_id=1", self.passport)
    }
}

/// A QR code to show, and what polling it needs.
pub struct QrCode {
    /// The QR code's content.
    pub url: String,
    pub code: String,
    /// Lifetime of the code in seconds, when Douyu states one.
    pub expires_in: Option<u64>,
    /// The login's passport cookies so far: its new device ID and any cookies
    /// the passport set. Every poll sends them.
    pub passport_cookies: String,
}

impl std::fmt::Debug for QrCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QrCode")
            .field("expires_in", &self.expires_in)
            .finish_non_exhaustive()
    }
}

/// A signed-in account.
pub struct Session {
    /// Main-site cookies, carrying the login's device ID as both `dy_did` and
    /// `acf_did`, so app playback signs with the same device.
    pub cookies: String,
    /// `LTP0`, which renews the cookies.
    pub passport_credential: String,
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
    Expired,
    Confirmed(Session),
}

/// A renewed main-site session.
pub struct Renewal {
    pub cookies: String,
    /// A replacement `LTP0`, when the passport issued one.
    pub passport_credential: Option<String>,
}

impl std::fmt::Debug for Renewal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Renewal").finish_non_exhaustive()
    }
}

#[derive(Deserialize)]
struct Envelope<T> {
    error: i64,
    data: Option<T>,
}

#[derive(Deserialize)]
struct GeneratedCode {
    code: Option<String>,
    expire: Option<u64>,
    url: Option<String>,
}

#[derive(Deserialize)]
struct ScanResult {
    url: Option<String>,
}

/// A web device ID as Douyu's site creates one: `b` and 31 lowercase
/// letters or digits.
pub fn new_device_id() -> String {
    use rand::RngExt;
    const ALPHABET: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut rng = rand::rng();
    std::iter::once('b')
        .chain((0..31).map(|_| ALPHABET[rng.random_range(0..ALPHABET.len())] as char))
        .collect()
}

/// The value of cookie `name` in a Cookie header.
pub fn cookie_value(cookies: &str, name: &str) -> Option<String> {
    parse_cookie_header(cookies)
        .into_iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value)
}

/// The value a response's `Set-Cookie` headers give cookie `name`.
fn set_cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    set_cookies(headers)
        .filter(|(key, _)| key == name)
        .last()
        .and_then(|(_, value)| value)
}

/// `(name, value)` of each `Set-Cookie` header; `None` deletes the cookie: an
/// empty value or a `Max-Age` that is not positive, as a browser treats them.
/// Headers that are not visible ASCII are skipped, since the cookies are sent
/// back as a header.
fn set_cookies(headers: &HeaderMap) -> impl Iterator<Item = (String, Option<String>)> + '_ {
    headers
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .filter_map(|value| {
            let mut parts = value.split(';');
            let (name, pair_value) = parts.next()?.split_once('=')?;
            let name = name.trim();
            if name.is_empty() {
                return None;
            }
            let pair_value = pair_value.trim().trim_matches('"');
            let expired = parts.any(|attribute| {
                attribute.split_once('=').is_some_and(|(key, age)| {
                    key.trim().eq_ignore_ascii_case("max-age")
                        && age.trim().parse::<i64>().is_ok_and(|age| age <= 0)
                })
            });
            let value = (!pair_value.is_empty() && !expired).then(|| pair_value.to_owned());
            Some((name.to_owned(), value))
        })
}

/// Applies a response's `Set-Cookie` headers to a Cookie header, skipping the
/// names `keep_out` rejects.
fn apply_set_cookies(
    cookies: &str,
    headers: &HeaderMap,
    keep_out: impl Fn(&str) -> bool,
) -> String {
    let mut jar = parse_cookie_header(cookies);
    for (name, value) in set_cookies(headers).filter(|(name, _)| !keep_out(name)) {
        let existing = jar.iter().position(|(key, _)| *key == name);
        match (existing, value) {
            (Some(index), Some(value)) => jar[index].1 = value,
            (None, Some(value)) => jar.push((name, value)),
            (Some(index), None) => {
                jar.remove(index);
            }
            (None, None) => {}
        }
    }
    jar.into_iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("; ")
}

/// The JSON object inside a JSONP (`callback({...})`) or JSON body.
fn jsonp_object(body: &str) -> Option<serde_json::Value> {
    let start = body.find('{')?;
    let end = body.rfind('}')?;
    serde_json::from_str(body.get(start..=end)?).ok()
}

/// A JSONP body's error code; Douyu sends it as a number or a string.
fn jsonp_error(body: &str) -> Option<i64> {
    let error = jsonp_object(body)?.get("error")?.clone();
    error
        .as_i64()
        .or_else(|| error.as_str().and_then(|code| code.trim().parse().ok()))
}

fn now_ms() -> String {
    chrono::Utc::now().timestamp_millis().to_string()
}

/// Starts a QR login with a new device ID.
pub async fn generate_qr(client: &Client) -> Result<QrCode, PassportError> {
    generate_qr_at(client, &Hosts::douyu()).await
}

async fn generate_qr_at(client: &Client, hosts: &Hosts) -> Result<QrCode, PassportError> {
    let device = new_device_id();
    let device_cookies = format!("dy_did={device}; acf_did={device}; game_did={device}");
    let response = client
        .post(format!("{}/scan/generateCode", hosts.passport))
        .header(
            header::CONTENT_TYPE,
            "application/x-www-form-urlencoded; charset=UTF-8",
        )
        .header("X-Requested-With", "XMLHttpRequest")
        .header(header::USER_AGENT, DEFAULT_UA)
        .header(header::REFERER, hosts.login_page())
        .header(header::COOKIE, &device_cookies)
        .body("client_id=1&isMultiAccount=0")
        .send()
        .await
        .map_err(network)?;
    let response = checked(response)?;
    let passport_cookies = apply_set_cookies(&device_cookies, response.headers(), |_| false);
    let body = response.text().await.map_err(network)?;
    let generated: Envelope<GeneratedCode> =
        serde_json::from_str(&body).map_err(|_| PassportError::Parse("QR code response"))?;
    if generated.error != 0 {
        return Err(PassportError::Api(generated.error));
    }
    let data = generated
        .data
        .ok_or(PassportError::Parse("QR code response without data"))?;
    match (data.code, data.url) {
        (Some(code), Some(url)) if !code.is_empty() && !url.is_empty() => Ok(QrCode {
            url,
            code,
            expires_in: data.expire.filter(|seconds| *seconds > 0),
            passport_cookies,
        }),
        _ => Err(PassportError::Parse("QR code response without a code")),
    }
}

/// Polls a QR login; a confirmed login is exchanged for main-site cookies.
pub async fn poll_qr(
    client: &Client,
    code: &str,
    passport_cookies: &str,
) -> Result<QrStatus, PassportError> {
    poll_qr_at(client, &Hosts::douyu(), code, passport_cookies).await
}

async fn poll_qr_at(
    client: &Client,
    hosts: &Hosts,
    code: &str,
    passport_cookies: &str,
) -> Result<QrStatus, PassportError> {
    let response = client
        .get(format!("{}/japi/scan/auth", hosts.passport))
        .query(&[("time", now_ms().as_str()), ("code", code)])
        .header("X-Requested-With", "XMLHttpRequest")
        .header(header::USER_AGENT, DEFAULT_UA)
        .header(header::REFERER, hosts.login_page())
        .header(header::COOKIE, passport_cookies)
        .send()
        .await
        .map_err(network)?;
    let response = checked(response)?;
    let jar = apply_set_cookies(passport_cookies, response.headers(), |_| false);
    let body = response.text().await.map_err(network)?;
    let result: Envelope<ScanResult> =
        serde_json::from_str(&body).map_err(|_| PassportError::Parse("QR status response"))?;
    debug!(status = result.error, "Douyu QR status");
    match result.error {
        -2 => return Ok(QrStatus::NotScanned),
        1 => return Ok(QrStatus::Scanned),
        -3 | 2 => return Ok(QrStatus::Expired),
        0 => {}
        code => return Err(PassportError::Api(code)),
    }
    let login_url = result
        .data
        .and_then(|data| data.url)
        .ok_or(PassportError::Parse(
            "confirmed login without a main-site address",
        ))?;
    let passport_credential = cookie_value(&jar, PASSPORT_CREDENTIAL_COOKIE).ok_or(
        PassportError::Parse("confirmed login without a passport credential"),
    )?;
    let device = cookie_value(&jar, DEVICE_ID_COOKIE)
        .ok_or(PassportError::Parse("confirmed login without a device ID"))?;
    let cookies = exchange(client, hosts, &login_url, &jar, &device).await?;
    Ok(QrStatus::Confirmed(Session {
        cookies,
        passport_credential,
    }))
}

/// The main-site login address with its JSONP parameters. Only the main
/// site's own login path receives the passport cookies.
fn main_login_url(hosts: &Hosts, login_url: &str) -> Result<url::Url, PassportError> {
    let foreign = || PassportError::Parse("main-site login address is not Douyu's");
    let main = url::Url::parse(&hosts.main).map_err(|_| foreign())?;
    let mut url = url::Url::parse(login_url).map_err(|_| foreign())?;
    if url.origin() != main.origin() || url.path() != MAIN_LOGIN_PATH {
        return Err(foreign());
    }
    let has = |name: &str| url.query_pairs().any(|(key, _)| key == name);
    let (callback, timestamp) = (!has("callback"), !has("_"));
    if callback {
        url.query_pairs_mut()
            .append_pair("callback", "appClient_json_callback");
    }
    if timestamp {
        url.query_pairs_mut().append_pair("_", &now_ms());
    }
    Ok(url)
}

/// Trades a confirmed login for main-site cookies. `LTP0` stays out of them:
/// the main site does not need it, and it should not travel with every
/// recording request.
async fn exchange(
    client: &Client,
    hosts: &Hosts,
    login_url: &str,
    passport_cookies: &str,
    device: &str,
) -> Result<String, PassportError> {
    let response = client
        .get(main_login_url(hosts, login_url)?)
        .header(header::USER_AGENT, DEFAULT_UA)
        .header(header::REFERER, format!("{}/", hosts.main))
        .header(header::COOKIE, passport_cookies)
        .send()
        .await
        .map_err(network)?;
    let response = checked(response)?;
    let base = format!("{DEVICE_ID_COOKIE}={device}; {APP_DEVICE_ID_COOKIE}={device}");
    let cookies = apply_set_cookies(&base, response.headers(), |name| {
        name == PASSPORT_CREDENTIAL_COOKIE
    });
    let body = response.text().await.map_err(network)?;
    if let Some(code) = jsonp_error(&body).filter(|code| *code != 0) {
        return Err(PassportError::Api(code));
    }
    if cookie_value(&cookies, UID_COOKIE).is_none()
        || cookie_value(&cookies, SESSION_COOKIE).is_none()
    {
        return Err(PassportError::Parse("main-site login returned no session"));
    }
    Ok(cookies)
}

/// Renews the main-site session in `cookies` with `LTP0` and the cookies'
/// device ID. The passport accepts only these two cookies on this request.
/// `LTP0` is written into the returned cookies only if they already held it.
pub async fn renew_session(
    client: &Client,
    cookies: &str,
    passport_credential: &str,
) -> Result<Renewal, PassportError> {
    renew_session_at(client, &Hosts::douyu(), cookies, passport_credential).await
}

async fn renew_session_at(
    client: &Client,
    hosts: &Hosts,
    cookies: &str,
    passport_credential: &str,
) -> Result<Renewal, PassportError> {
    let device = cookie_value(cookies, DEVICE_ID_COOKIE)
        .ok_or(PassportError::Parse("account has no device ID"))?;
    let timestamp = now_ms();
    let response = client
        .get(format!("{}/lapi/passport/iframe/safeAuth", hosts.passport))
        .query(&[
            ("client_id", "1"),
            ("t", timestamp.as_str()),
            ("_", timestamp.as_str()),
            ("callback", "axiosJsonpCallback"),
        ])
        .header(header::USER_AGENT, DEFAULT_UA)
        .header(header::REFERER, format!("{}/", hosts.main))
        .header(header::ORIGIN, &hosts.main)
        .header(
            header::COOKIE,
            format!(
                "{DEVICE_ID_COOKIE}={device}; {PASSPORT_CREDENTIAL_COOKIE}={passport_credential}"
            ),
        )
        .send()
        .await
        .map_err(network)?;
    let response = checked(response)?;
    let headers = response.headers().clone();
    let body = response.text().await.map_err(network)?;
    if let Some(code) = jsonp_error(&body).filter(|code| *code != 0) {
        return Err(PassportError::Api(code));
    }
    if set_cookie_value(&headers, SESSION_COOKIE).is_none() {
        return Err(PassportError::Parse("renewal returned no session"));
    }
    let credential_in_cookies = cookie_value(cookies, PASSPORT_CREDENTIAL_COOKIE).is_some();
    let renewed = apply_set_cookies(cookies, &headers, |name| {
        name == PASSPORT_CREDENTIAL_COOKIE && !credential_in_cookies
    });
    if let Some(uid) = cookie_value(cookies, UID_COOKIE)
        && cookie_value(&renewed, UID_COOKIE).as_deref() != Some(uid.as_str())
    {
        return Err(PassportError::AccountMismatch);
    }
    Ok(Renewal {
        cookies: renewed,
        passport_credential: set_cookie_value(&headers, PASSPORT_CREDENTIAL_COOKIE),
    })
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    /// A canned reply to a request for `path`.
    struct Reply {
        path: &'static str,
        status: &'static str,
        headers: Vec<&'static str>,
        body: String,
    }

    fn reply(path: &'static str, headers: Vec<&'static str>, body: impl Into<String>) -> Reply {
        Reply {
            path,
            status: "200 OK",
            headers,
            body: body.into(),
        }
    }

    /// Serves the replies in order, one per connection, and records each
    /// request's head. `{base}` in a body becomes the server's own address.
    async fn serve(replies: Vec<Reply>) -> (Hosts, Arc<Mutex<Vec<String>>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let seen = requests.clone();
        let hosts = Hosts {
            passport: base.clone(),
            main: base.clone(),
        };
        tokio::spawn(async move {
            for reply in replies {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let mut head = Vec::new();
                let mut buf = [0u8; 4096];
                while !head.windows(4).any(|window| window == b"\r\n\r\n") {
                    match socket.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(read) => head.extend_from_slice(&buf[..read]),
                    }
                }
                let head = String::from_utf8_lossy(&head).into_owned();
                assert!(
                    head.lines().next().is_some_and(|line| line
                        .split(' ')
                        .nth(1)
                        .is_some_and(|target| target.split('?').next() == Some(reply.path))),
                    "unexpected request: {head}"
                );
                seen.lock().unwrap().push(head);
                let mut response = format!("HTTP/1.1 {}\r\n", reply.status);
                for header in &reply.headers {
                    response.push_str(header);
                    response.push_str("\r\n");
                }
                let body = reply.body.replace("{base}", &base);
                response.push_str(&format!(
                    "content-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len(),
                ));
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
            }
        });
        (hosts, requests)
    }

    fn client() -> Client {
        crate::extractor::default::create_client_builder(None)
            .no_proxy()
            .build()
            .unwrap()
    }

    fn header_of<'a>(request: &'a str, name: &str) -> Option<&'a str> {
        request.lines().find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case(name).then(|| value.trim())
        })
    }

    async fn bounded<T>(future: impl std::future::Future<Output = T>) -> T {
        tokio::time::timeout(std::time::Duration::from_secs(10), future)
            .await
            .expect("local passport request timed out")
    }

    #[test]
    fn device_ids_look_like_the_sites_own() {
        let id = new_device_id();
        assert_eq!(id.len(), 32);
        assert!(id.starts_with('b'));
        assert!(
            id.bytes()
                .all(|byte| byte.is_ascii_digit() || byte.is_ascii_lowercase())
        );
        assert_ne!(id, new_device_id());
    }

    #[test]
    fn set_cookies_replace_add_and_delete_like_a_browser() {
        let mut headers = HeaderMap::new();
        for value in [
            "acf_auth=fresh; Path=/; Domain=.douyu.com",
            "acf_stk=new; Max-Age=518400",
            "acf_ct=; Path=/",
            "dy_auth=deleted; expires=Thu, 01-Jan-1970 00:00:01 GMT; Max-Age=0",
        ] {
            headers.append(header::SET_COOKIE, value.parse().unwrap());
        }
        assert_eq!(
            apply_set_cookies(
                "dy_did=d; acf_auth=stale; acf_ct=0; dy_auth=x",
                &headers,
                |_| false
            ),
            "dy_did=d; acf_auth=fresh; acf_stk=new"
        );
        assert_eq!(
            apply_set_cookies("dy_did=d", &headers, |name| name.starts_with("acf_")),
            "dy_did=d"
        );
    }

    #[tokio::test]
    async fn a_qr_code_carries_its_device_and_passport_cookies() {
        let (hosts, requests) = serve(vec![reply(
            "/scan/generateCode",
            vec!["set-cookie: PHPSESSID=s1; path=/"],
            r#"{"error":0,"data":{"code":"c0de","expire":300,"url":"https://m.douyu.com/topic/scan-login-middle-page?scan_code=c0de"}}"#,
        )])
        .await;
        let qr = bounded(generate_qr_at(&client(), &hosts)).await.unwrap();
        assert_eq!(qr.code, "c0de");
        assert_eq!(qr.expires_in, Some(300));
        assert!(qr.url.ends_with("scan_code=c0de"));
        let device = cookie_value(&qr.passport_cookies, DEVICE_ID_COOKIE).unwrap();
        assert_eq!(
            cookie_value(&qr.passport_cookies, APP_DEVICE_ID_COOKIE).as_deref(),
            Some(device.as_str())
        );
        assert_eq!(
            cookie_value(&qr.passport_cookies, "PHPSESSID").as_deref(),
            Some("s1")
        );
        let request = requests.lock().unwrap()[0].clone();
        assert!(request.starts_with("POST /scan/generateCode "));
        assert_eq!(
            header_of(&request, "x-requested-with"),
            Some("XMLHttpRequest")
        );
        assert!(
            header_of(&request, "cookie")
                .is_some_and(|cookie| cookie.contains(&device) && cookie.contains("game_did="))
        );
        assert!(
            header_of(&request, "referer")
                .is_some_and(|referer| referer.ends_with("/index/login?type=login&client_id=1"))
        );
    }

    #[tokio::test]
    async fn a_refused_qr_code_reports_the_error_code() {
        let (hosts, _) = serve(vec![reply(
            "/scan/generateCode",
            vec![],
            r#"{"error":-1,"msg":"private message"}"#,
        )])
        .await;
        let error = bounded(generate_qr_at(&client(), &hosts))
            .await
            .unwrap_err();
        assert!(matches!(error, PassportError::Api(-1)));
        assert!(!error.to_string().contains("private"));
    }

    #[tokio::test]
    async fn waiting_scanned_and_expired_codes_poll_without_an_exchange() {
        for (error, expected) in [
            (-2, "NotScanned"),
            (1, "Scanned"),
            (-3, "Expired"),
            (2, "Expired"),
        ] {
            let (hosts, requests) = serve(vec![reply(
                "/japi/scan/auth",
                vec![],
                format!(r#"{{"error":{error},"msg":"","data":null}}"#),
            )])
            .await;
            let status = bounded(poll_qr_at(
                &client(),
                &hosts,
                "c0de",
                "dy_did=d; PHPSESSID=s1",
            ))
            .await
            .unwrap();
            assert_eq!(format!("{status:?}"), expected);
            let request = requests.lock().unwrap()[0].clone();
            assert!(request.contains("code=c0de"));
            assert_eq!(
                header_of(&request, "cookie"),
                Some("dy_did=d; PHPSESSID=s1")
            );
        }
        let (hosts, _) = serve(vec![reply("/japi/scan/auth", vec![], r#"{"error":-5}"#)]).await;
        assert!(matches!(
            bounded(poll_qr_at(&client(), &hosts, "c0de", "dy_did=d")).await,
            Err(PassportError::Api(-5))
        ));
    }

    #[tokio::test]
    async fn a_confirmed_login_exchanges_the_passport_session_for_main_site_cookies() {
        let (hosts, requests) = serve(vec![
            reply(
                "/japi/scan/auth",
                vec!["set-cookie: LTP0=long-lived; Domain=.douyu.com; Path=/"],
                r#"{"error":0,"data":{"url":"{base}/api/passport/login?token=exchange-secret"}}"#,
            ),
            reply(
                "/api/passport/login",
                vec![
                    "set-cookie: acf_uid=42; Path=/",
                    "set-cookie: acf_auth=main-session; Path=/",
                    "set-cookie: acf_nickname=%E7%94%A8%E6%88%B7; Path=/",
                    "set-cookie: LTP0=long-lived; Domain=.douyu.com",
                ],
                r#"appClient_json_callback({"error":0,"data":{"nickname":"user"}})"#,
            ),
        ])
        .await;
        let QrStatus::Confirmed(session) = bounded(poll_qr_at(
            &client(),
            &hosts,
            "c0de",
            "dy_did=bdevice; acf_did=bdevice; PHPSESSID=s1",
        ))
        .await
        .unwrap() else {
            panic!("the login was confirmed");
        };
        assert_eq!(session.passport_credential, "long-lived");
        assert_eq!(
            session.cookies,
            "dy_did=bdevice; acf_did=bdevice; acf_uid=42; acf_auth=main-session; acf_nickname=%E7%94%A8%E6%88%B7"
        );
        let requests = requests.lock().unwrap();
        let exchange = &requests[1];
        assert!(exchange.contains("token=exchange-secret"));
        assert!(exchange.contains("callback=appClient_json_callback"));
        // The exchange carries the passport session, now holding LTP0.
        assert!(header_of(exchange, "cookie").is_some_and(|cookie| {
            cookie.contains("LTP0=long-lived") && cookie.contains("PHPSESSID=s1")
        }));
        assert!(header_of(exchange, "referer").is_some_and(|referer| referer.ends_with('/')));
    }

    #[tokio::test]
    async fn a_confirmed_login_refuses_foreign_addresses_and_missing_sessions() {
        let (hosts, requests) = serve(vec![reply(
            "/japi/scan/auth",
            vec!["set-cookie: LTP0=long-lived"],
            r#"{"error":0,"data":{"url":"https://elsewhere.example/api/passport/login?token=t"}}"#,
        )])
        .await;
        assert!(matches!(
            bounded(poll_qr_at(&client(), &hosts, "c0de", "dy_did=bdevice")).await,
            Err(PassportError::Parse(_))
        ));
        assert_eq!(requests.lock().unwrap().len(), 1);

        let (hosts, _) = serve(vec![reply(
            "/japi/scan/auth",
            vec![],
            r#"{"error":0,"data":{"url":"{base}/api/passport/login?token=t"}}"#,
        )])
        .await;
        assert!(matches!(
            bounded(poll_qr_at(&client(), &hosts, "c0de", "dy_did=bdevice")).await,
            Err(PassportError::Parse(_))
        ));

        let (hosts, _) = serve(vec![
            reply(
                "/japi/scan/auth",
                vec!["set-cookie: LTP0=long-lived"],
                r#"{"error":0,"data":{"url":"{base}/api/passport/login?token=t"}}"#,
            ),
            reply(
                "/api/passport/login",
                vec!["set-cookie: acf_uid=42"],
                r#"appClient_json_callback({"error":"0"})"#,
            ),
        ])
        .await;
        assert!(matches!(
            bounded(poll_qr_at(&client(), &hosts, "c0de", "dy_did=bdevice")).await,
            Err(PassportError::Parse("main-site login returned no session"))
        ));
    }

    #[tokio::test]
    async fn renewal_sends_only_the_device_and_passport_credential() {
        let (hosts, requests) = serve(vec![reply(
            "/lapi/passport/iframe/safeAuth",
            vec![
                "set-cookie: acf_auth=renewed; Domain=.douyu.com",
                "set-cookie: acf_stk=new-stk; Domain=.douyu.com",
                "set-cookie: LTP0=rotated; Domain=.douyu.com",
            ],
            r#"axiosJsonpCallback({"error":0})"#,
        )])
        .await;
        let renewal = bounded(renew_session_at(
            &client(),
            &hosts,
            "dy_did=bdevice; acf_did=bdevice; acf_uid=42; acf_auth=old",
            "long-lived",
        ))
        .await
        .unwrap();
        assert_eq!(
            renewal.cookies,
            "dy_did=bdevice; acf_did=bdevice; acf_uid=42; acf_auth=renewed; acf_stk=new-stk"
        );
        assert_eq!(renewal.passport_credential.as_deref(), Some("rotated"));
        let request = requests.lock().unwrap()[0].clone();
        assert_eq!(
            header_of(&request, "cookie"),
            Some("dy_did=bdevice; LTP0=long-lived")
        );
        assert!(request.contains("client_id=1"));
        assert!(request.contains("callback=axiosJsonpCallback"));
        assert_eq!(header_of(&request, "origin"), Some(hosts.main.as_str()));

        // Cookies that already carried LTP0 keep it current.
        let (hosts, _) = serve(vec![reply(
            "/lapi/passport/iframe/safeAuth",
            vec!["set-cookie: acf_auth=renewed", "set-cookie: LTP0=rotated"],
            r#"axiosJsonpCallback({"error":0})"#,
        )])
        .await;
        let renewal = bounded(renew_session_at(
            &client(),
            &hosts,
            "dy_did=bdevice; LTP0=long-lived; acf_auth=old",
            "long-lived",
        ))
        .await
        .unwrap();
        assert_eq!(
            renewal.cookies,
            "dy_did=bdevice; LTP0=rotated; acf_auth=renewed"
        );
    }

    #[tokio::test]
    async fn renewal_failures_are_typed_and_keep_secrets_out_of_errors() {
        let renew = |replies| async move {
            let (hosts, _) = serve(replies).await;
            bounded(renew_session_at(
                &client(),
                &hosts,
                "dy_did=bdevice; acf_uid=42; acf_auth=old",
                "long-lived",
            ))
            .await
        };
        let error = renew(vec![reply(
            "/lapi/passport/iframe/safeAuth",
            vec![],
            r#"axiosJsonpCallback({"error":-1,"msg":"long-lived"})"#,
        )])
        .await
        .unwrap_err();
        assert!(matches!(error, PassportError::Api(-1)));
        assert!(matches!(
            renew(vec![reply(
                "/lapi/passport/iframe/safeAuth",
                vec![],
                r#"axiosJsonpCallback({"error":0})"#,
            )])
            .await,
            Err(PassportError::Parse("renewal returned no session"))
        ));
        assert!(matches!(
            renew(vec![reply(
                "/lapi/passport/iframe/safeAuth",
                vec!["set-cookie: acf_auth=other", "set-cookie: acf_uid=7"],
                r#"axiosJsonpCallback({"error":0})"#,
            )])
            .await,
            Err(PassportError::AccountMismatch)
        ));
        let mut throttled = reply(
            "/lapi/passport/iframe/safeAuth",
            vec!["retry-after: 120"],
            "",
        );
        throttled.status = "429 Too Many Requests";
        assert!(matches!(
            renew(vec![throttled]).await,
            Err(PassportError::Response(ExtractorError::RateLimited {
                retry_after: Some(delay),
                ..
            })) if delay == std::time::Duration::from_secs(120)
        ));
        let mut failed = reply("/lapi/passport/iframe/safeAuth", vec![], "");
        failed.status = "502 Bad Gateway";
        let error = renew(vec![failed]).await.unwrap_err();
        assert!(matches!(error, PassportError::Network(_)));
        assert!(!error.to_string().contains("safeAuth"));

        let (hosts, requests) = serve(vec![]).await;
        assert!(matches!(
            renew_session_at(&client(), &hosts, "acf_did=bdevice", "long-lived").await,
            Err(PassportError::Parse("account has no device ID"))
        ));
        assert!(requests.lock().unwrap().is_empty());
    }
}
