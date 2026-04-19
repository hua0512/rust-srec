//! JavaScript engine error types.

/// Errors that can occur during JavaScript execution.
#[derive(Debug, thiserror::Error)]
pub enum JsError {
    /// Failed to create a new JavaScript runtime.
    #[error("Failed to create JS runtime: {0}")]
    RuntimeCreation(String),
    /// Failed to create a JavaScript context.
    #[error("Failed to create JS context: {0}")]
    ContextCreation(String),
    /// JavaScript evaluation failed.
    #[error(
        "JS evaluation failed: {message}{}",
        stack.as_deref().map(|s| format!("\nStack: {s}")).unwrap_or_default()
    )]
    Evaluation {
        message: String,
        stack: Option<String>,
    },
    /// Type conversion error (e.g., expected string, got object).
    #[error("JS type conversion error: {0}")]
    TypeConversion(String),
    /// The runtime pool is exhausted.
    #[error("JS runtime pool exhausted")]
    PoolExhausted,
}

impl JsError {
    /// Create an evaluation error from a message.
    pub fn eval(message: impl Into<String>) -> Self {
        JsError::Evaluation {
            message: message.into(),
            stack: None,
        }
    }

    /// Create an evaluation error with stack trace.
    pub fn eval_with_stack(message: impl Into<String>, stack: impl Into<String>) -> Self {
        JsError::Evaluation {
            message: message.into(),
            stack: Some(stack.into()),
        }
    }
}

impl From<rquickjs::Error> for JsError {
    fn from(err: rquickjs::Error) -> Self {
        JsError::Evaluation {
            message: err.to_string(),
            stack: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_matches_pre_migration_formatting() {
        assert_eq!(
            JsError::RuntimeCreation("boom".into()).to_string(),
            "Failed to create JS runtime: boom"
        );
        assert_eq!(
            JsError::ContextCreation("boom".into()).to_string(),
            "Failed to create JS context: boom"
        );
        assert_eq!(
            JsError::eval("boom").to_string(),
            "JS evaluation failed: boom"
        );
        assert_eq!(
            JsError::eval_with_stack("boom", "at foo.js:1").to_string(),
            "JS evaluation failed: boom\nStack: at foo.js:1"
        );
        assert_eq!(
            JsError::TypeConversion("boom".into()).to_string(),
            "JS type conversion error: boom"
        );
        assert_eq!(
            JsError::PoolExhausted.to_string(),
            "JS runtime pool exhausted"
        );
    }
}
