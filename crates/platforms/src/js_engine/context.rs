//! JavaScript context wrapper with ergonomic API.

use super::error::JsError;

#[cfg(feature = "douyu")]
use rquickjs::CatchResultExt;

/// Default browser environment setup code.
/// Provides stubs for window, document, navigator, etc.
pub const BROWSER_ENV_SETUP: &str = r#"
    var window = window || {};
    var document = document || {};
    var navigator = navigator || {
        userAgent: 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/142.0.0.0 Safari/537.36',
        platform: 'Win32',
        language: 'zh-CN',
        appCodeName: 'Mozilla',
        appVersion: '5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/142.0.0.0 Safari/537.36',
        onLine: true,
        cookieEnabled: true
    };
    window.navigator = navigator;
    window.innerHeight = 910;
    window.innerWidth = 1920;
    window.outerHeight = 28;
    window.outerWidth = 160;
    window.screenX = 0;
    window.screenY = 9;
    window.pageYOffset = 0;
    window.pageXOffset = 0;
    window.screen = {};
    window.onwheelx = { "_Ax": "0X21" };
    window.addEventListener = function() {};
    window.sessionStorage = {};
    window.localStorage = {};
    document.hidden = true;
    document.webkitHidden = true;
    document.cookie = '';
    var Request = {};
    var Headers = {};
"#;

/// A wrapper around rquickjs::Context providing an ergonomic API.
#[cfg(feature = "douyu")]
pub struct JsContext {
    ctx: rquickjs::Context,
}

#[cfg(feature = "douyu")]
impl JsContext {
    /// Create a new JsContext from a rquickjs Runtime.
    pub fn new(runtime: &rquickjs::Runtime) -> Result<Self, JsError> {
        let ctx = rquickjs::Context::full(runtime)
            .map_err(|e| JsError::ContextCreation(e.to_string()))?;
        Ok(Self { ctx })
    }

    /// Set up browser environment stubs (window, document, navigator, etc.).
    pub fn setup_browser_env(&self) -> Result<(), JsError> {
        self.eval_void(BROWSER_ENV_SETUP)
    }

    /// Set up browser environment with a custom user agent.
    pub fn setup_browser_env_with_ua(&self, user_agent: &str) -> Result<(), JsError> {
        let code = BROWSER_ENV_SETUP.replace(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
            user_agent
        );
        self.eval_void(&code)
    }

    /// Load a script into the context without returning a value.
    pub fn load_script(&self, script: &str) -> Result<(), JsError> {
        self.eval_void(script)
    }

    /// Evaluate JavaScript code and return the result as a String.
    pub fn eval_string(&self, code: &str) -> Result<String, JsError> {
        self.ctx.with(|ctx| {
            let result: Result<String, _> = ctx.eval(code);
            result
                .catch(&ctx)
                .map_err(|caught| Self::convert_caught_error(caught))
        })
    }

    /// Evaluate JavaScript code without returning a value.
    pub fn eval_void(&self, code: &str) -> Result<(), JsError> {
        self.ctx.with(|ctx| {
            let result: Result<(), _> = ctx.eval(code);
            result
                .catch(&ctx)
                .map_err(|caught| Self::convert_caught_error(caught))
        })
    }

    /// Evaluate JavaScript code and return a generic value.
    /// The value must implement FromJs.
    pub fn eval<T>(&self, code: &str) -> Result<T, JsError>
    where
        T: for<'js> rquickjs::FromJs<'js>,
    {
        self.ctx.with(|ctx| {
            let result: Result<T, _> = ctx.eval(code);
            result
                .catch(&ctx)
                .map_err(|caught| Self::convert_caught_error(caught))
        })
    }

    /// Convert a CaughtError to JsError with detailed information.
    fn convert_caught_error(caught: rquickjs::CaughtError) -> JsError {
        use rquickjs::CaughtError;
        match caught {
            CaughtError::Exception(exc) => {
                let msg = exc.message().unwrap_or_default();
                let stack = exc.stack();
                if let Some(stack) = stack {
                    if !stack.is_empty() {
                        return JsError::eval_with_stack(msg, stack);
                    }
                }
                JsError::eval(msg)
            }
            CaughtError::Value(val) => JsError::eval(format!(
                "JS threw value: {:?}",
                val.as_string().map(|s| s.to_string())
            )),
            CaughtError::Error(err) => JsError::eval(err.to_string()),
        }
    }
}
