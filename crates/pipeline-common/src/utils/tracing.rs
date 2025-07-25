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

// Re-export the macro
#[allow(unused_imports)]
pub use crate::init_test_tracing;
