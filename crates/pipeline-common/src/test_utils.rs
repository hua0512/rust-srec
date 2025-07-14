use std::sync::Arc;

use crate::StreamerContext;

/// Macro to initialize tracing for tests
///
/// Usage:
/// - `init_test_tracing!()` - uses DEBUG level (default)
/// - `init_test_tracing!(INFO)` - uses specified level
#[macro_export]
macro_rules! init_test_tracing {
    () => {
        init_test_tracing!(DEBUG);
    };
    ($level:ident) => {
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::$level)
            .with_test_writer()
            .try_init();
    };
}

/// Create a test streamer context
#[inline]
pub fn create_test_context() -> Arc<StreamerContext> {
    Arc::new(StreamerContext::default())
}

// Re-export the macro
pub use crate::init_test_tracing;
