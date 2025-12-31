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

mod context;
mod error;
mod manager;

pub use context::BROWSER_ENV_SETUP;
pub use context::JsContext;
pub use error::JsError;
pub use manager::JsEngineManager;
