//! Read-side UI and MCP filtered search and aggregation queries.
//!
//! Pure display projections, literal substring searches, and summary queries
//! live here rather than on domain write repositories so REST routes and MCP
//! tools share one SQL implementation against the read pool. Domain-derived
//! queries such as DAG display-status counting stay on their domain
//! repositories.

mod jobs;
mod notifications;
mod outputs;
mod presets;
mod sessions;

use sqlx::SqlitePool;

pub use jobs::JobFilters;
pub use outputs::{MediaOutputTypeSummary, OutputFilters};
pub use presets::{JobPresetFilters, PipelinePresetFilters};
pub use sessions::SessionFilters;

/// Read-pool query facade shared by REST routes and MCP tools.
#[derive(Clone)]
pub struct ApiQueries {
    pool: SqlitePool,
}

impl ApiQueries {
    /// Create a query facade backed by the read `SqlitePool`.
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}
