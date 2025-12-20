//! JavaScript engine error types.

use std::fmt;

/// Errors that can occur during JavaScript execution.
#[derive(Debug)]
pub enum JsError {
    /// Failed to create a new JavaScript runtime.
    RuntimeCreation(String),
    /// Failed to create a JavaScript context.
    ContextCreation(String),
    /// JavaScript evaluation failed.
    Evaluation {
        message: String,
        stack: Option<String>,
    },
    /// Type conversion error (e.g., expected string, got object).
    TypeConversion(String),
    /// The runtime pool is exhausted.
    PoolExhausted,
}

impl fmt::Display for JsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            JsError::RuntimeCreation(msg) => write!(f, "Failed to create JS runtime: {}", msg),
            JsError::ContextCreation(msg) => write!(f, "Failed to create JS context: {}", msg),
            JsError::Evaluation { message, stack } => {
                if let Some(stack) = stack {
                    write!(f, "JS evaluation failed: {}\nStack: {}", message, stack)
                } else {
                    write!(f, "JS evaluation failed: {}", message)
                }
            }
            JsError::TypeConversion(msg) => write!(f, "JS type conversion error: {}", msg),
            JsError::PoolExhausted => write!(f, "JS runtime pool exhausted"),
        }
    }
}

impl std::error::Error for JsError {}

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

#[cfg(feature = "douyu")]
impl From<rquickjs::Error> for JsError {
    fn from(err: rquickjs::Error) -> Self {
        JsError::Evaluation {
            message: err.to_string(),
            stack: None,
        }
    }
}
