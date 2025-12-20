//! JavaScript engine manager with thread-local runtime caching.

use std::cell::RefCell;

use super::context::JsContext;
use super::error::JsError;

/// Thread-local runtime cache.
/// Each thread gets its own cached runtime to avoid the overhead of creating
/// a new runtime for each JS execution.
#[cfg(feature = "douyu")]
thread_local! {
    static THREAD_RUNTIME: RefCell<Option<rquickjs::Runtime>> = const { RefCell::new(None) };
}

/// A manager for JavaScript execution using thread-local runtimes.
///
/// Since QuickJS runtimes are not thread-safe (they use Rc internally),
/// we use thread-local storage to cache one runtime per thread.
/// This avoids the expensive runtime creation for repeated calls within
/// the same thread.
#[cfg(feature = "douyu")]
pub struct JsEngineManager;

#[cfg(feature = "douyu")]
impl JsEngineManager {
    /// Get the global engine manager instance.
    /// This is a zero-cost abstraction since JsEngineManager has no state.
    pub fn global() -> Self {
        Self
    }

    /// Get or create the thread-local runtime.
    fn with_runtime<F, T>(f: F) -> Result<T, JsError>
    where
        F: FnOnce(&rquickjs::Runtime) -> Result<T, JsError>,
    {
        THREAD_RUNTIME.with(|cell| {
            let mut runtime_ref = cell.borrow_mut();

            // Create runtime if not cached
            if runtime_ref.is_none() {
                let runtime = rquickjs::Runtime::new()
                    .map_err(|e| JsError::RuntimeCreation(e.to_string()))?;
                *runtime_ref = Some(runtime);
            }

            // Safe to unwrap since we just ensured it's Some
            f(runtime_ref.as_ref().unwrap())
        })
    }

    /// Execute a function with a JavaScript context.
    ///
    /// The context is automatically created from the thread-local runtime.
    ///
    /// # Example
    /// ```ignore
    /// let result = JsEngineManager::global().execute(|ctx| {
    ///     ctx.setup_browser_env()?;
    ///     ctx.load_script("function add(a, b) { return a + b; }")?;
    ///     ctx.eval_string("add(1, 2)")
    /// })?;
    /// ```
    pub fn execute<F, T>(&self, f: F) -> Result<T, JsError>
    where
        F: FnOnce(&JsContext) -> Result<T, JsError>,
    {
        Self::with_runtime(|runtime| {
            let ctx = JsContext::new(runtime)?;
            f(&ctx)
        })
    }

    /// Execute with browser environment pre-configured.
    ///
    /// This is a convenience method that sets up window, document, navigator
    /// stubs before executing the provided function.
    pub fn execute_with_browser_env<F, T>(&self, f: F) -> Result<T, JsError>
    where
        F: FnOnce(&JsContext) -> Result<T, JsError>,
    {
        self.execute(|ctx| {
            ctx.setup_browser_env()?;
            f(ctx)
        })
    }

    /// Execute with browser environment and a custom user agent.
    pub fn execute_with_browser_env_ua<F, T>(&self, user_agent: &str, f: F) -> Result<T, JsError>
    where
        F: FnOnce(&JsContext) -> Result<T, JsError>,
    {
        self.execute(|ctx| {
            ctx.setup_browser_env_with_ua(user_agent)?;
            f(ctx)
        })
    }

    /// Execute with pre-loaded scripts.
    ///
    /// Loads all provided scripts before executing the function.
    pub fn execute_with_scripts<F, T>(&self, scripts: &[&str], f: F) -> Result<T, JsError>
    where
        F: FnOnce(&JsContext) -> Result<T, JsError>,
    {
        self.execute(|ctx| {
            ctx.setup_browser_env()?;
            for script in scripts {
                ctx.load_script(script)?;
            }
            f(ctx)
        })
    }

    /// Clear the thread-local runtime cache.
    /// This is useful for testing or to free memory.
    #[allow(dead_code)]
    pub fn clear_cache() {
        THREAD_RUNTIME.with(|cell| {
            *cell.borrow_mut() = None;
        });
    }
}

#[cfg(feature = "douyu")]
impl Default for JsEngineManager {
    fn default() -> Self {
        Self::global()
    }
}

#[cfg(test)]
#[cfg(feature = "douyu")]
mod tests {
    use super::*;

    #[test]
    fn test_basic_execution() {
        let manager = JsEngineManager::global();

        let result = manager.execute(|ctx| ctx.eval_string("1 + 2"));

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "3");
    }

    #[test]
    fn test_runtime_reuse() {
        let manager = JsEngineManager::global();

        // First execution creates runtime
        let result1 = manager.execute(|ctx| ctx.eval_string("1"));
        assert!(result1.is_ok());

        // Second execution reuses runtime (same thread)
        let result2 = manager.execute(|ctx| ctx.eval_string("2"));
        assert!(result2.is_ok());
    }

    #[test]
    fn test_browser_env() {
        let manager = JsEngineManager::global();

        let result = manager.execute_with_browser_env(|ctx| ctx.eval_string("typeof window"));

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "object");
    }

    #[test]
    fn test_clear_cache() {
        let manager = JsEngineManager::global();

        // Create runtime
        let _ = manager.execute(|ctx| ctx.eval_string("1"));

        // Clear cache
        JsEngineManager::clear_cache();

        // Should still work (creates new runtime)
        let result = manager.execute(|ctx| ctx.eval_string("2"));
        assert!(result.is_ok());
    }
}
