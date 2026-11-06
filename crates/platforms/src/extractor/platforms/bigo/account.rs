//! Bigo account session checks.
//!
//! The website calls `getUserInfo` on every page load. With a login it
//! answers `code: 0` and sets `nick_name`, `user_name` and `yyuid` again for
//! another fifteen days; once the login has lapsed it answers
//! [`SIGNED_OUT_CODE`], the same as for a visitor with no cookies.

use reqwest::Client;
use reqwest::header::{COOKIE, ORIGIN, REFERER, SET_COOKIE, USER_AGENT};
use serde::Deserialize;

use crate::extractor::default::DEFAULT_UA;
use crate::extractor::error::ExtractorError;
use crate::extractor::utils::merge_cookie_header_strs;

const USER_INFO_URL: &str = "https://ta.bigo.tv/official_website/usercenter/getUserInfo";
const SITE: &str = "https://www.bigo.tv";
/// The website reloads as a logged-out visitor on this code.
pub const SIGNED_OUT_CODE: i64 = 700001;

/// Whether a set of cookies holds a Bigo login.
#[derive(Debug)]
pub enum AccountSession {
    /// The login works; `cookies` are the request's cookies with the ones
    /// Bigo set again in the response.
    SignedIn {
        cookies: String,
    },
    SignedOut,
}

#[derive(Deserialize)]
struct UserInfoResponse {
    code: i64,
}

/// Asks Bigo whether `cookies` hold a login.
pub async fn user_info(client: &Client, cookies: &str) -> Result<AccountSession, ExtractorError> {
    let response = client
        .get(USER_INFO_URL)
        .header(USER_AGENT, DEFAULT_UA)
        .header(ORIGIN, SITE)
        .header(REFERER, format!("{SITE}/"))
        .header(COOKIE, cookies)
        .send()
        .await?;
    let response = ExtractorError::check_response(response)?;
    let renewed = response
        .headers()
        .get_all(SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .filter_map(|value| value.split(';').next())
        .map(str::trim)
        .filter(|pair| !pair.is_empty())
        .collect::<Vec<_>>()
        .join("; ");
    let body = response.json::<UserInfoResponse>().await?;
    session_from(body.code, cookies, &renewed)
}

fn session_from(code: i64, cookies: &str, renewed: &str) -> Result<AccountSession, ExtractorError> {
    match code {
        0 => Ok(AccountSession::SignedIn {
            cookies: merge_cookie_header_strs(cookies, renewed),
        }),
        SIGNED_OUT_CODE => Ok(AccountSession::SignedOut),
        code => Err(ExtractorError::ValidationError(format!(
            "bigo getUserInfo answered code {code}"
        ))),
    }
}
