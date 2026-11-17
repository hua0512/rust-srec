//! Linkd tickets: the identity BIGO's website issues for a native login.
//!
//! BIGO keeps one native session per account and drops the older one when
//! another signs in. Recording therefore uses [`guest`] tickets, which carry
//! no account, so it never disconnects the user's phone app or the listener.
//! Only the listener signs in with an [`account`] ticket.

use std::fmt;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD_PAD_INDIFFERENT;
use rand::RngExt;
use reqwest::header::{COOKIE, ORIGIN, REFERER, USER_AGENT};

use super::{NativeError, Route};
use crate::extractor::default::DEFAULT_UA;
use crate::extractor::platforms::bigo::danmu::WS_LINK_URL;
use crate::extractor::platforms::bigo::models::{WsLinkData, WsLinkResponse};

const TICKET_TIMEOUT: Duration = Duration::from_secs(15);

/// The identity for one Linkd login. `Debug` leaves out the token.
#[derive(Clone)]
pub struct Ticket {
    pub(super) uid: u32,
    pub(super) user_name: String,
    pub(super) device_id: String,
    /// The decoded `uidToken`, which Linkd's prepare-login carries.
    pub(super) token: Vec<u8>,
}

impl fmt::Debug for Ticket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ticket")
            .field("uid", &self.uid)
            .field("user_name", &self.user_name)
            .field("device_id", &self.device_id)
            .finish_non_exhaustive()
    }
}

/// A random device id in the form the website sends: 32 lowercase hex digits.
pub fn device_id() -> String {
    let mut bytes = [0u8; 16];
    rand::rng().fill(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// A guest ticket under a fresh device id. The request carries no cookies,
/// so the ticket never belongs to an account.
pub async fn guest(route: &Route) -> Result<Ticket, NativeError> {
    let device_id = device_id();
    let data = request(route, None, &device_id).await?;
    Ticket::from_answer(&data, &device_id)
}

/// The ticket of the account signed in with `cookies`, requested under
/// `device_id`. [`NativeError::SignedOut`] when BIGO does not see the
/// cookies as signed in.
///
/// The caller keeps one stable device id per account (made once with
/// [`device_id`]), so BIGO sees the same device across the listener's
/// reconnects.
pub async fn account(route: &Route, cookies: &str, device_id: &str) -> Result<Ticket, NativeError> {
    let data = request(route, Some(cookies), device_id).await?;
    let user = data.real_user.as_deref().ok_or(NativeError::SignedOut)?;
    Ticket::from_answer(user, device_id)
}

async fn request(
    route: &Route,
    cookies: Option<&str>,
    device_id: &str,
) -> Result<WsLinkData, NativeError> {
    let mut request = route
        .client
        .post(WS_LINK_URL)
        .header(USER_AGENT, DEFAULT_UA)
        .header(ORIGIN, "https://www.bigo.tv")
        .header(REFERER, "https://www.bigo.tv/")
        .form(&[("deviceId", device_id)])
        .timeout(TICKET_TIMEOUT);
    if let Some(cookies) = cookies {
        request = request.header(COOKIE, cookies);
    }
    let answer: WsLinkResponse = request.send().await?.error_for_status()?.json().await?;
    if !answer.is_success() {
        return Err(NativeError::Protocol(format!(
            "ticket answer code {}",
            answer.code.unwrap_or_default()
        )));
    }
    answer
        .data
        .ok_or_else(|| NativeError::Protocol("ticket answer has no data".to_owned()))
}

impl Ticket {
    /// `user` is the answer's top level or its `realUser`. Its device id
    /// may be empty, and the top level's only repeats the request's, so the
    /// request's stands in.
    fn from_answer(user: &WsLinkData, request_device_id: &str) -> Result<Self, NativeError> {
        let missing = |field: &str| NativeError::Protocol(format!("ticket answer has no {field}"));
        let uid = non_empty(&user.user_id)
            .ok_or_else(|| missing("userId"))?
            .parse()
            .map_err(|_| {
                NativeError::Protocol("ticket userId is not a 32-bit number".to_owned())
            })?;
        let token = non_empty(&user.uid_token).ok_or_else(|| missing("uidToken"))?;
        let token = token.strip_suffix("###VER2").unwrap_or(token);
        let token = STANDARD_PAD_INDIFFERENT
            .decode(token)
            .map_err(|_| NativeError::Protocol("ticket uidToken is not base64".to_owned()))?;
        Ok(Self {
            uid,
            user_name: user.user_name.clone().unwrap_or_default(),
            device_id: non_empty(&user.device_id)
                .unwrap_or(request_device_id)
                .to_owned(),
            token,
        })
    }
}

fn non_empty(field: &Option<String>) -> Option<&str> {
    field
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
}
