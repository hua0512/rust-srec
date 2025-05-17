use std::sync::Arc;

use crate::StreamerContext;

/// Initialize tracing for tests with appropriate settings
#[inline]
pub fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_test_writer() // Write to test output
        .try_init();
}

/// Create a test streamer context
#[inline]
pub fn create_test_context() -> Arc<StreamerContext> {
    Arc::new(StreamerContext::default())
}
