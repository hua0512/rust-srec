//! Centralized JavaScript engine management.
//!
//! This module provides a pooled QuickJS runtime manager for efficient
//! JavaScript execution across the codebase. It's used by:
//! - Douyin: Signature generation for WebSocket danmu
//!
//! # Example
//!
//! ```ignore
//! use platforms_parser::js_engine::JsEngineManager;
//!
//! let result = JsEngineManager::global().execute(|ctx| {
//!     ctx.setup_browser_env()?;
//!     ctx.load_script("function greet(name) { return 'Hello, ' + name; }")?;
//!     ctx.eval_string("greet('World')")
//! })?;
//! ```

#[cfg(feature = "rquickjs")]
mod context;
#[cfg(feature = "rquickjs")]
mod error;
#[cfg(feature = "rquickjs")]
mod manager;

#[cfg(feature = "rquickjs")]
pub use context::BROWSER_ENV_SETUP;
#[cfg(feature = "rquickjs")]
pub use context::JsContext;
#[cfg(feature = "rquickjs")]
pub use error::JsError;
#[cfg(feature = "rquickjs")]
pub use manager::JsEngineManager;
