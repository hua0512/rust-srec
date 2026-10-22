//! A credential provider for the [`super::FakePlatform`]'s accounts, which
//! tests script: QR logins the test scans and confirms, and refreshes that
//! swap one cookie string for another.
//!
//! The provider is registered for the whole process, so its state is keyed by
//! what each test makes unique: a login's auth code, an account's cookies.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, Once};

use async_trait::async_trait;
use rust_srec::credentials::{
    CredentialError, CredentialMaterial, CredentialProvider, ProviderCapabilities, QrLoginPoll,
    QrLoginStart, RefreshedCredentials, register_provider_for_process,
};

use super::fake_platform::FAKE_PLATFORM_NAME;

/// Where a QR login stands, as the fake user moves it along.
enum Login {
    Shown,
    Scanned,
    Confirmed(String),
}

/// A refresh the provider performs for an account's cookies.
struct Refresh {
    cookies: String,
    /// Runs when the refresh happens, before the new cookies are handed back.
    on_refresh: Box<dyn Fn() + Send + Sync>,
}

#[derive(Default)]
struct State {
    logins: HashMap<String, Login>,
    refreshes: HashMap<String, Refresh>,
    /// The cookies of every account refreshed, in order.
    refreshed: Vec<String>,
}

static STATE: LazyLock<Mutex<State>> = LazyLock::new(Mutex::default);

fn with_state<T>(f: impl FnOnce(&mut State) -> T) -> T {
    f(&mut STATE.lock().expect("fake provider state lock"))
}

struct FakeProvider;

#[async_trait]
impl CredentialProvider for FakeProvider {
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            qr_login: true,
            refresh: true,
            ..ProviderCapabilities::default()
        }
    }

    async fn refresh(
        &self,
        _client: &reqwest::Client,
        material: &CredentialMaterial,
    ) -> Result<RefreshedCredentials, CredentialError> {
        let cookies = with_state(|state| {
            let refresh = state.refreshes.get(&material.cookies)?;
            (refresh.on_refresh)();
            let cookies = refresh.cookies.clone();
            state.refreshed.push(material.cookies.clone());
            Some(cookies)
        })
        .ok_or_else(|| CredentialError::InvalidCredentials("unknown fake account".into()))?;
        Ok(RefreshedCredentials {
            cookies,
            refresh_token: None,
            access_token: None,
            expires_at: None,
        })
    }

    async fn start_qr_login(
        &self,
        _client: &reqwest::Client,
    ) -> Result<QrLoginStart, CredentialError> {
        let code = uuid::Uuid::new_v4().simple().to_string();
        with_state(|state| state.logins.insert(code.clone(), Login::Shown));
        Ok(QrLoginStart {
            url: format!("https://fake.invalid/qr/{code}"),
            auth_code: code,
            expires_in: Some(120),
        })
    }

    async fn poll_qr_login(
        &self,
        _client: &reqwest::Client,
        auth_code: &str,
    ) -> Result<QrLoginPoll, CredentialError> {
        with_state(|state| match state.logins.get(auth_code) {
            None => Ok(QrLoginPoll::Expired),
            Some(Login::Shown) => Ok(QrLoginPoll::Waiting { scanned: false }),
            Some(Login::Scanned) => Ok(QrLoginPoll::Waiting { scanned: true }),
            Some(Login::Confirmed(cookies)) => Ok(QrLoginPoll::Completed(CredentialMaterial {
                cookies: cookies.clone(),
                refresh_token: None,
                access_token: None,
                reauth_config: None,
            })),
        })
    }
}

/// Control over the fake platform's account provider.
pub struct FakeAccounts(());

impl FakeAccounts {
    /// Make sure the provider is registered in this process.
    pub fn install() -> Self {
        static REGISTER: Once = Once::new();
        REGISTER.call_once(|| {
            register_provider_for_process(FAKE_PLATFORM_NAME, Arc::new(FakeProvider))
                .expect("register the fake credential provider");
        });
        Self(())
    }

    /// The auth code a QR login's code URL carries.
    pub fn auth_code(qr_url: &str) -> String {
        qr_url
            .strip_prefix("https://fake.invalid/qr/")
            .unwrap_or_else(|| panic!("not a fake QR code URL: {qr_url}"))
            .to_owned()
    }

    /// The user scans the code of the login with `auth_code`.
    pub fn scan(&self, auth_code: &str) {
        self.advance(auth_code, Login::Scanned);
    }

    /// The user confirms the login with `auth_code`, which signs in with `cookies`.
    pub fn confirm(&self, auth_code: &str, cookies: &str) {
        self.advance(auth_code, Login::Confirmed(cookies.to_owned()));
    }

    fn advance(&self, auth_code: &str, login: Login) {
        with_state(|state| {
            let current = state
                .logins
                .get_mut(auth_code)
                .unwrap_or_else(|| panic!("no QR login has auth code {auth_code}"));
            *current = login;
        });
    }

    /// Refreshing the account holding `stale` cookies runs `on_refresh` and
    /// replaces them with `fresh`. Any other account fails to refresh.
    pub fn refresh_to(
        &self,
        stale: &str,
        fresh: &str,
        on_refresh: impl Fn() + Send + Sync + 'static,
    ) {
        with_state(|state| {
            state.refreshes.insert(
                stale.to_owned(),
                Refresh {
                    cookies: fresh.to_owned(),
                    on_refresh: Box::new(on_refresh),
                },
            )
        });
    }

    /// How many times the account holding `cookies` was refreshed.
    pub fn refreshes_of(&self, cookies: &str) -> usize {
        with_state(|state| state.refreshed.iter().filter(|c| *c == cookies).count())
    }
}
