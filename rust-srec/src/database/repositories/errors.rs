use thiserror::Error;

#[derive(Debug, Error)]
pub enum RepositoryError {
    #[error("entity not found")]
    NotFound,
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("validation error: {0}")]
    Validation(String),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

pub type RepositoryResult<T> = Result<T, RepositoryError>;
