use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aes::Aes128;
use base64::{Engine, engine::general_purpose::STANDARD};
use cbc::cipher::{BlockModeEncrypt, KeyIvInit, block_padding::Pkcs7};
use md5::{Digest, Md5};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use rand::RngExt;
use reqwest::{
    Client, RequestBuilder,
    header::{self, HeaderValue},
};
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::OnceCell;

use crate::extractor::{
    error::ExtractorError::{self, ValidationError},
    platform_configs::DouyuDeviceIdMode,
};

use super::app_sign::DEFAULT_DID;

const APP_VERSION: &str = "8.2.2.0";
const DEV_AES_KEY: [u8; 16] = [
    98, 65, 46, 228, 242, 135, 198, 146, 181, 13, 150, 1, 199, 213, 59, 189,
];
const DEV_AES_IV: [u8; 16] = [
    59, 96, 30, 218, 35, 200, 117, 87, 84, 43, 146, 248, 226, 205, 182, 2,
];

/// Validate options eagerly; initialize the device on the first playback request.
/// A completed identity is shared by retries and deferred CDN/quality resolution.
pub(super) struct AppDevice {
    device_name: Option<String>,
    os_version: Option<String>,
    mode: DouyuDeviceIdMode,
    explicit_did: Option<String>,
    identity: OnceCell<DeviceIdentity>,
}

pub(super) struct DeviceIdentity {
    pub query_device: String,
    pub user_agent: HeaderValue,
    pub did: String,
    pub user_device: HeaderValue,
    pub cookie: HeaderValue,
}

fn configured_string<'a>(
    extras: Option<&'a Value>,
    key: &str,
) -> Result<Option<&'a str>, ExtractorError> {
    match extras.and_then(|extras| extras.get(key)) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok((!value.is_empty()).then_some(value.as_str())),
        _ => Err(ValidationError(format!("Douyu {key} must be a string"))),
    }
}

/// Accept alphanumeric 32-byte IDs or UUID-shaped word groups.
/// Restrict word characters to ASCII for the HTTP cookie boundary.
fn is_valid_did(did: &str) -> bool {
    if did == DEFAULT_DID {
        return false;
    }
    let bytes = did.as_bytes();
    (bytes.len() == 32 && bytes.iter().all(u8::is_ascii_alphanumeric))
        || (bytes.len() == 36
            && bytes.iter().enumerate().all(|(i, byte)| {
                if matches!(i, 8 | 13 | 18 | 23) {
                    *byte == b'-'
                } else {
                    byte.is_ascii_alphanumeric() || *byte == b'_'
                }
            }))
}

fn random_android_device() -> String {
    let mut rng = rand::rng();
    let mut model = String::with_capacity(8);
    for _ in 0..3 {
        model.push(rng.random_range(b'A'..=b'Z') as char);
    }
    model.push('-');
    for _ in 0..2 {
        model.push(rng.random_range(b'A'..=b'Z') as char);
    }
    for _ in 0..2 {
        model.push(rng.random_range(b'0'..=b'9') as char);
    }
    model
}

fn quote_plus(value: &str) -> String {
    // The wire format keeps '~' and escapes '*'. Spaces use '+' in the UA
    // and '-' in the device query parameter.
    const SET: &percent_encoding::AsciiSet = &NON_ALPHANUMERIC
        .remove(b'-')
        .remove(b'_')
        .remove(b'.')
        .remove(b'~');
    utf8_percent_encode(value, SET)
        .to_string()
        .replace("%20", "+")
}

impl AppDevice {
    pub fn new(
        extras: Option<&Value>,
        cookies: &FxHashMap<String, String>,
    ) -> Result<Self, ExtractorError> {
        let device_name = configured_string(extras, "device_name")?.map(str::to_owned);
        let os_version = configured_string(extras, "os_version")?.map(str::to_owned);
        let mode = match extras
            .and_then(|extras| extras.get("device_id_mode"))
            .filter(|v| !v.is_null())
        {
            Some(value) => serde_json::from_value(value.clone())
                .map_err(|_| ValidationError("Invalid Douyu device_id_mode".into()))?,
            None => DouyuDeviceIdMode::default(),
        };
        let explicit_did = match configured_string(extras, "device_id")? {
            Some(did) if is_valid_did(did) || did == DEFAULT_DID => Some(did.to_owned()),
            Some(_) => {
                return Err(ValidationError(
                    "Douyu device_id must be 32 alphanumeric characters or an ASCII UUID-shaped ID"
                        .into(),
                ));
            }
            None => cookies
                .get("acf_did")
                .filter(|did| is_valid_did(did))
                .cloned(),
        };
        Ok(Self {
            device_name,
            os_version,
            mode,
            explicit_did,
            identity: OnceCell::new(),
        })
    }

    pub async fn identity(&self, client: &Client) -> Result<&DeviceIdentity, ExtractorError> {
        self.identity
            .get_or_try_init(|| async {
                let name = self
                    .device_name
                    .clone()
                    .unwrap_or_else(random_android_device);
                let user_agent = HeaderValue::from_str(&format!(
                    "android/{APP_VERSION} (android {}; ; {})",
                    quote_plus(self.os_version.as_deref().unwrap_or("14")),
                    quote_plus(&name)
                ))
                .map_err(|_| ValidationError("Invalid Douyu User-Agent header".into()))?;
                let did = if let Some(did) = &self.explicit_did {
                    did.clone()
                } else {
                    match self.mode {
                        DouyuDeviceIdMode::Default => DEFAULT_DID.to_owned(),
                        DouyuDeviceIdMode::Local => {
                            // DYRandomGUID hashes the millisecond Unix timestamp.
                            let timestamp = SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .map_err(|_| {
                                    ValidationError(
                                        "Invalid system time for Douyu device ID".into(),
                                    )
                                })?
                                .as_millis();
                            super::app_sign::hex(&Md5::digest(timestamp.to_string().as_bytes()))
                        }
                        DouyuDeviceIdMode::Server => Self::register(client, &user_agent).await?,
                    }
                };
                Ok(DeviceIdentity {
                    query_device: name.replace(' ', "-"),
                    user_agent,
                    user_device: HeaderValue::from_str(
                        &STANDARD.encode(format!("{did}|v{APP_VERSION}")),
                    )
                    .map_err(|_| ValidationError("Invalid Douyu User-Device header".into()))?,
                    cookie: HeaderValue::from_str(&format!("acf_did={did}"))
                        .map_err(|_| ValidationError("Invalid Douyu device cookie".into()))?,
                    did,
                })
            })
            .await
    }

    fn registration_request(
        client: &Client,
        user_agent: &HeaderValue,
        timestamp: u64,
        android_id: &str,
        oaid: &str,
    ) -> Result<RequestBuilder, ExtractorError> {
        let user_device = STANDARD.encode(format!("{DEFAULT_DID}|v{APP_VERSION}"));
        Ok(client
            .post("https://passport.douyu.com/japi/app/did/android")
            .timeout(Duration::from_secs(5))
            .header(header::USER_AGENT, user_agent.clone())
            .header("dev", registration_dev(android_id, oaid)?)
            .header("User-Device", user_device)
            .header("aid", "android1")
            .header("client_sys", "android")
            .header("time", timestamp.to_string())
            .header("channel", "447")
            .form(&[("biz_type", "12"), ("channel_id", "447"), ("token", "")]))
    }

    async fn register(client: &Client, user_agent: &HeaderValue) -> Result<String, ExtractorError> {
        let android_id = uuid::Uuid::new_v4().simple().to_string();
        let oaid = uuid::Uuid::new_v4().to_string();
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| {
                ValidationError("Invalid system time for Douyu device registration".into())
            })?
            .as_secs();
        let response: RegistrationResponse =
            Self::registration_request(client, user_agent, timestamp, &android_id[..16], &oaid)?
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
        response.into_did()
    }
}

fn registration_dev(android_id: &str, oaid: &str) -> Result<String, ExtractorError> {
    // The encrypted payload requires compact JSON in this field order;
    // serializing a map could reorder its bytes.
    #[derive(Serialize)]
    struct DeviceData<'a> {
        ad: &'a str,
        od: &'a str,
        im: &'a str,
        ii: &'a str,
        id: &'a str,
    }
    let mut data = serde_json::to_vec(&DeviceData {
        ad: android_id,
        od: oaid,
        im: "",
        ii: "",
        id: "",
    })?;
    let len = data.len();
    data.resize((len / 16 + 1) * 16, 0);
    let encrypted = cbc::Encryptor::<Aes128>::new(&DEV_AES_KEY.into(), &DEV_AES_IV.into())
        .encrypt_padded::<Pkcs7>(&mut data, len)
        .map_err(|_| ValidationError("Could not encode Douyu device registration".into()))?;
    Ok(STANDARD.encode(encrypted))
}

#[derive(Deserialize)]
struct RegistrationResponse {
    error: i32,
    #[serde(default)]
    data: Value,
}

impl RegistrationResponse {
    fn into_did(self) -> Result<String, ExtractorError> {
        if self.error != 0 {
            return Err(ValidationError(format!(
                "Douyu device registration error {}",
                self.error
            )));
        }
        self.data
            .get("did")
            .and_then(Value::as_str)
            .filter(|did| is_valid_did(did))
            .map(str::to_owned)
            .ok_or_else(|| {
                ValidationError("Douyu device registration returned an invalid DID".into())
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extractor::default::default_client;
    use serde_json::json;

    #[tokio::test]
    async fn device_wire_vectors() {
        let device = AppDevice::new(
            Some(&json!({"device_name":"OnePlus 华为~*+/%", "os_version":"14 Q+~*"})),
            &FxHashMap::default(),
        )
        .unwrap();
        assert_eq!(
            device.identity(&default_client()).await.unwrap().user_agent,
            "android/8.2.2.0 (android 14+Q%2B~%2A; ; OnePlus+%E5%8D%8E%E4%B8%BA~%2A%2B%2F%25)"
        );
        assert_eq!(
            registration_dev("0123456789abcdef", "01234567-89ab-cdef-0123-456789abcdef").unwrap(),
            "ltefIlZbCcCaYPR0OhjGfsmNtZCRBgKIsX4qZMwZghlKzQV4n4TM7JHPJGwlFTSLjyQgtGOV2pD3kT3WeQavkwuHd8t0xF/raU7iGe3c6J4yCC61T7xsIMO68LkkSZuL"
        );
    }

    #[tokio::test]
    async fn dynamic_profile_is_consistent_and_cached() {
        let device = AppDevice::new(None, &FxHashMap::default()).unwrap();
        assert!(device.identity.get().is_none());
        let client = default_client();
        let (a, b) = tokio::join!(device.identity(&client), device.identity(&client));
        let (a, b) = (a.unwrap(), b.unwrap());
        assert!(std::ptr::eq(a, b));
        let model = a.query_device.as_bytes();
        assert_eq!(model.len(), 8);
        assert!(model[..3].iter().all(u8::is_ascii_uppercase));
        assert_eq!(model[3], b'-');
        assert!(model[4..6].iter().all(u8::is_ascii_uppercase));
        assert!(model[6..].iter().all(u8::is_ascii_digit));
        assert!(a.user_agent.to_str().unwrap().contains(&a.query_device));
        assert!(is_valid_did(&a.did));
        assert_eq!(a.cookie.to_str().unwrap(), format!("acf_did={}", a.did));
        assert_eq!(
            STANDARD.decode(a.user_device.as_bytes()).unwrap(),
            format!("{}|v8.2.2.0", a.did).as_bytes()
        );
    }

    #[tokio::test]
    async fn explicit_id_then_cookie_then_mode() {
        let cookie_did = "0123456789abcdef0123456789abcdef";
        let explicit_did = "ABCDEF12-3456-7890-ABCD-EF1234567890";
        let client = default_client();
        let cookies = FxHashMap::from_iter([("acf_did".into(), cookie_did.into())]);
        // Server mode must not register when a supplied ID is available.
        let device = AppDevice::new(
            Some(&json!({"device_id_mode":"server", "device_id":explicit_did})),
            &cookies,
        )
        .unwrap();
        assert_eq!(device.identity(&client).await.unwrap().did, explicit_did);
        let device = AppDevice::new(Some(&json!({"device_id_mode":"server"})), &cookies).unwrap();
        assert_eq!(device.identity(&client).await.unwrap().did, cookie_did);
        let device = AppDevice::new(
            Some(&json!({"device_id_mode":"default"})),
            &FxHashMap::default(),
        )
        .unwrap();
        assert_eq!(device.identity(&client).await.unwrap().did, DEFAULT_DID);
        let cookies = FxHashMap::from_iter([("acf_did".into(), "bad-cookie".into())]);
        let device = AppDevice::new(None, &cookies).unwrap();
        assert!(is_valid_did(&device.identity(&client).await.unwrap().did));
        for options in [
            json!({"device_id":"bad"}),
            json!({"device_id_mode":"unknown"}),
            json!({"device_name":4}),
            json!({"os_version":14}),
        ] {
            assert!(AppDevice::new(Some(&options), &cookies).is_err());
        }
    }

    #[tokio::test]
    async fn registration_request_and_response_contract() {
        let device = AppDevice::new(
            Some(&json!({"device_name":"OnePlus 12","os_version":"15"})),
            &FxHashMap::default(),
        )
        .unwrap();
        let client = default_client();
        let identity = device.identity(&client).await.unwrap();
        let request = AppDevice::registration_request(
            &client,
            &identity.user_agent,
            1790312470,
            "0123456789abcdef",
            "01234567-89ab-cdef-0123-456789abcdef",
        )
        .unwrap()
        .build()
        .unwrap();
        assert_eq!(request.method(), reqwest::Method::POST);
        assert_eq!(
            request.url().as_str(),
            "https://passport.douyu.com/japi/app/did/android"
        );
        assert_eq!(
            request.headers()["User-Agent"],
            "android/8.2.2.0 (android 15; ; OnePlus+12)"
        );
        assert_eq!(request.headers()["time"], "1790312470");
        assert_eq!(
            request.body().unwrap().as_bytes().unwrap(),
            b"biz_type=12&channel_id=447&token="
        );
        for value in [
            json!({"error":1,"data":""}),
            json!({"error":0,"data":{"did":"invalid"}}),
            json!({"error":0,"data":{"did":DEFAULT_DID}}),
        ] {
            assert!(
                serde_json::from_value::<RegistrationResponse>(value)
                    .unwrap()
                    .into_did()
                    .is_err()
            );
        }
        let did = "0123456789abcdef0123456789abcdef";
        assert_eq!(
            serde_json::from_value::<RegistrationResponse>(json!({"error":0,"data":{"did":did}}))
                .unwrap()
                .into_did()
                .unwrap(),
            did
        );
    }
}
