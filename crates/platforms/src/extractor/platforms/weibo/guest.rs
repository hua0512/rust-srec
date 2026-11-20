//! The anonymous `.weibo.cn` visitor session that m.weibo.cn's profile API
//! accepts, so a user page resolves without an account.

use std::borrow::Cow;
use std::sync::{Arc, LazyLock, Mutex, PoisonError};

use regex::Regex;
use reqwest::Client;
use reqwest::header::{HeaderMap, HeaderValue, REFERER};
use tokio::sync::Mutex as AsyncMutex;
use tracing::{debug, warn};

use crate::extractor::error::ExtractorError;

const ENTER_URL: &str = "https://visitor.passport.weibo.cn/visitor/visitor";
const GENERATE_URL: &str = "https://visitor.passport.weibo.cn/visitor/genvisitor2";
const RETURN_URL: &str = "https://m.weibo.cn/";
/// The visitor page's script version, sent as `ver`.
const VISITOR_VERSION: &str = "20250916";
const RETCODE_OK: i64 = 20_000_000;

static REQUEST_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"request_id\s*=\s*"([0-9a-f]+)""#).unwrap());

/// The session shared by every Weibo check in the process. The factory
/// builds a new extractor for each check, and the session stays valid for
/// months, so it is fetched once and kept until m.weibo.cn rejects it.
pub(super) static GLOBAL_GUEST: LazyLock<Arc<GuestSession>> =
    LazyLock::new(|| Arc::new(GuestSession::default()));

/// Where the visitor flow is requested; tests point it at a local server.
pub(super) struct VisitorApi {
    pub(super) enter: Cow<'static, str>,
    pub(super) generate: Cow<'static, str>,
}

impl Default for VisitorApi {
    fn default() -> Self {
        Self {
            enter: Cow::Borrowed(ENTER_URL),
            generate: Cow::Borrowed(GENERATE_URL),
        }
    }
}

/// A cached visitor `SUB` with a single-flight fetch: concurrent checks that
/// find the cache empty share one visitor flow.
#[derive(Default)]
pub(super) struct GuestSession {
    sub: Mutex<Option<String>>,
    fetch_lock: AsyncMutex<()>,
}

impl GuestSession {
    fn cached(&self) -> Option<String> {
        self.sub
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The cached visitor `SUB`, fetching one when there is none.
    ///
    /// `headers` are the platform's default headers; the visitor flow never
    /// carries the user's cookies.
    pub(super) async fn sub(
        &self,
        client: &Client,
        headers: &HeaderMap,
        api: &VisitorApi,
    ) -> Result<String, ExtractorError> {
        if let Some(sub) = self.cached() {
            return Ok(sub);
        }
        let _fetch_guard = self.fetch_lock.lock().await;
        if let Some(sub) = self.cached() {
            return Ok(sub);
        }
        let sub = fetch_visitor_sub(client, headers, api).await?;
        *self.sub.lock().unwrap_or_else(PoisonError::into_inner) = Some(sub.clone());
        Ok(sub)
    }

    /// Forgets `rejected` if it is still the cached session. A session that
    /// another check already fetched in its place is kept, so checks that
    /// were rejected together refresh it only once.
    pub(super) fn invalidate(&self, rejected: &str) {
        let mut sub = self.sub.lock().unwrap_or_else(PoisonError::into_inner);
        if sub.as_deref() == Some(rejected) {
            *sub = None;
        }
    }
}

/// Runs the visitor flow: the `a=enter` page names a `request_id`, and
/// `genvisitor2` exchanges it for a `.weibo.cn` visitor `SUB`.
async fn fetch_visitor_sub(
    client: &Client,
    headers: &HeaderMap,
    api: &VisitorApi,
) -> Result<String, ExtractorError> {
    let now_ms = jiff::Timestamp::now().as_millisecond();
    let mut enter_headers = headers.clone();
    enter_headers.insert(REFERER, HeaderValue::from_static(RETURN_URL));
    let page = client
        .get(api.enter.as_ref())
        .headers(enter_headers)
        .query(&[
            ("entry", "sinawap"),
            ("a", "enter"),
            ("url", RETURN_URL),
            ("domain", ".weibo.cn"),
            ("sudaref", ""),
            ("ua", "php-sso_sdk_client-0.6.36"),
            ("_rand", &format!("{}.{:03}", now_ms / 1000, now_ms % 1000)),
        ])
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    let request_id = REQUEST_ID
        .captures(&page)
        .and_then(|captures| captures.get(1))
        .ok_or_else(|| ExtractorError::Other("weibo_visitor_request_id_missing".to_owned()))?
        .as_str();

    let mut generate_headers = headers.clone();
    let referer = format!("{}?entry=sinawap&a=enter", api.enter);
    if let Ok(referer) = HeaderValue::from_str(&referer) {
        generate_headers.insert(REFERER, referer);
    }
    let body = client
        .post(api.generate.as_ref())
        .headers(generate_headers)
        .header("X-Requested-With", "XMLHttpRequest")
        .form(&[
            ("cb", "visitor_gray_callback"),
            ("ver", VISITOR_VERSION),
            ("request_id", request_id),
            ("tid", ""),
            ("from", "weibo"),
            ("webdriver", "false"),
            ("rid", &now_ms.to_string()),
            ("return_url", RETURN_URL),
        ])
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    parse_visitor_sub(&body)
}

/// Reads `data.sub` from the JSONP `visitor_gray_callback({...})` body.
fn parse_visitor_sub(body: &str) -> Result<String, ExtractorError> {
    let json = body
        .find('{')
        .zip(body.rfind('}'))
        .and_then(|(start, end)| body.get(start..=end))
        .ok_or_else(|| ExtractorError::Other("weibo_visitor_response_unreadable".to_owned()))?;
    let value: serde_json::Value = serde_json::from_str(json)?;
    let retcode = value.get("retcode").and_then(serde_json::Value::as_i64);
    match value
        .pointer("/data/sub")
        .and_then(serde_json::Value::as_str)
    {
        Some(sub) if retcode == Some(RETCODE_OK) && !sub.is_empty() => {
            debug!("weibo: fetched a guest session");
            Ok(sub.to_owned())
        }
        _ => {
            warn!(
                ?retcode,
                "weibo: the visitor service refused a guest session"
            );
            Err(ExtractorError::Other("weibo_visitor_refused".to_owned()))
        }
    }
}
