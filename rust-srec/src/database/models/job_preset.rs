//! Job Preset database models.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

/// Valid processor types for presets.
pub const VALID_PROCESSORS: &[&str] = &[
    "remux",
    "rclone",
    "thumbnail",
    "execute",
    "audio_extract",
    "compression",
    "copy_move",
    "delete",
    "metadata",
];

/// Valid preset categories.
pub const VALID_CATEGORIES: &[&str] = &[
    "remux",       // Container format conversion (no re-encoding)
    "compression", // Re-encoding/transcoding
    "thumbnail",   // Image/preview generation
    "audio",       // Audio extraction
    "archive",     // Archiving/compression
    "upload",      // Cloud upload (rclone)
    "cleanup",     // File deletion
    "file_ops",    // Copy/move operations
    "custom",      // Custom execute commands
    "metadata",    // Metadata operations
];

/// Job Preset configuration.
///
/// Represents a reusable, named configuration for a specific processor.
/// Used in pipeline definitions to simplify configuration and promote reuse.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct JobPreset {
    /// Unique identifier (UUID).
    pub id: String,
    /// Unique name of the preset (e.g., "hq_remux", "fast_upload").
    pub name: String,
    /// Optional description of what this preset does.
    #[sqlx(default)]
    pub description: Option<String>,
    /// Category for organizing presets (e.g., "remux", "compression", "thumbnail").
    #[sqlx(default)]
    pub category: Option<String>,
    /// Processor type this preset applies to (e.g., "remux", "upload").
    pub processor: String,
    /// JSON blob for the processor configuration.
    /// This corresponds to the `config` field in `ProcessorInput`.
    pub config: String,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Last update timestamp.
    pub updated_at: DateTime<Utc>,
}

impl JobPreset {
    /// Create a new Job Preset.
    pub fn new(
        name: impl Into<String>,
        processor: impl Into<String>,
        config: serde_json::Value,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.into(),
            description: None,
            category: None,
            processor: processor.into(),
            config: config.to_string(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    /// Create a new Job Preset with description.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Create a new Job Preset with category.
    pub fn with_category(mut self, category: impl Into<String>) -> Self {
        self.category = Some(category.into());
        self
    }

    /// Validate the preset configuration.
    pub fn validate(&self) -> Result<(), String> {
        // Validate processor type
        if !VALID_PROCESSORS.contains(&self.processor.as_str()) {
            return Err(format!(
                "Invalid processor type '{}'. Valid types: {}",
                self.processor,
                VALID_PROCESSORS.join(", ")
            ));
        }

        // Validate category if provided
        if let Some(ref cat) = self.category {
            if !VALID_CATEGORIES.contains(&cat.as_str()) {
                return Err(format!(
                    "Invalid category '{}'. Valid categories: {}",
                    cat,
                    VALID_CATEGORIES.join(", ")
                ));
            }
        }

        // Validate config is valid JSON
        if serde_json::from_str::<serde_json::Value>(&self.config).is_err() {
            return Err("Config must be valid JSON".to_string());
        }

        // Validate name is not empty
        if self.name.trim().is_empty() {
            return Err("Name cannot be empty".to_string());
        }

        Ok(())
    }
}

/// Pipeline type identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum PipelineType {
    /// DAG pipeline with fan-in/fan-out support.
    #[default]
    Dag,
}

impl PipelineType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Dag => "dag",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            _ => Self::Dag,
        }
    }
}

/// Pipeline Preset configuration.
///
/// Represents a reusable DAG pipeline configuration that supports
/// fan-in, fan-out, and parallel execution.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct PipelinePreset {
    /// Unique identifier (UUID).
    pub id: String,
    /// Unique name of the preset (e.g., "Standard", "Archive to Cloud").
    pub name: String,
    /// Optional description of what this pipeline does.
    #[sqlx(default)]
    pub description: Option<String>,
    /// DAG pipeline definition (JSON-serialized DagPipelineDefinition).
    /// This is the primary field for defining pipeline structure.
    #[sqlx(default)]
    pub dag_definition: Option<String>,
    /// Pipeline type: 'dag' (default).
    #[sqlx(default)]
    #[serde(default)]
    pub pipeline_type: Option<String>,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Last update timestamp.
    pub updated_at: DateTime<Utc>,
}

use crate::database::models::job::DagPipelineDefinition;

impl PipelinePreset {
    /// Create a new DAG Pipeline Preset.
    pub fn new(name: impl Into<String>, dag: DagPipelineDefinition) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.into(),
            description: None,
            dag_definition: Some(serde_json::to_string(&dag).unwrap_or_else(|_| "{}".to_string())),
            pipeline_type: Some("dag".to_string()),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    /// Create a new Pipeline Preset with description.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Get the pipeline type.
    pub fn get_pipeline_type(&self) -> PipelineType {
        self.pipeline_type
            .as_ref()
            .map(|s| PipelineType::from_str(s))
            .unwrap_or(PipelineType::Dag)
    }

    /// Check if this is a DAG pipeline.
    pub fn is_dag(&self) -> bool {
        self.get_pipeline_type() == PipelineType::Dag
    }

    /// Get the DAG definition if this is a DAG pipeline.
    pub fn get_dag_definition(&self) -> Option<DagPipelineDefinition> {
        self.dag_definition
            .as_ref()
            .and_then(|s| serde_json::from_str(s).ok())
    }

    /// Validate the preset configuration.
    pub fn validate(&self) -> Result<(), String> {
        // Validate name is not empty
        if self.name.trim().is_empty() {
            return Err("Name cannot be empty".to_string());
        }

        // Validate DAG definition
        if let Some(ref dag_str) = self.dag_definition {
            let dag: Result<DagPipelineDefinition, _> = serde_json::from_str(dag_str);
            match dag {
                Ok(d) => {
                    // Validate the DAG structure
                    d.validate()?;
                }
                Err(e) => {
                    return Err(format!("Invalid DAG definition: {}", e));
                }
            }
        } else {
            return Err("Pipeline must have a dag_definition".to_string());
        }

        Ok(())
    }
}
