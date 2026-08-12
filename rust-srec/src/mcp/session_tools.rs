//! Recording session and danmu MCP tools.
//!
//! List/detail/segment/statistics tools wrap the `api::routes::sessions`
//! handlers. The danmu-content tools read the per-segment danmu XML files
//! registered in `media_outputs` (`file_type = 'DANMU_XML'`) directly, with
//! byte-range pagination so multi-megabyte chat logs cannot blow up the
//! model context.

use axum::extract::{FromRef, Path, Query, State};
use rmcp::{
    ErrorData, RoleServer,
    handler::server::wrapper::Parameters,
    model::{CallToolResult, ContentBlock},
    schemars,
    service::RequestContext,
    tool, tool_router,
};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

use super::config_tools::{IdParams, PageParams};
use super::{SrecMcpServer, api_error_result, tool_json, tool_unit};
use crate::api::error::ApiError;
use crate::api::models::SessionFilterParams;
use crate::api::routes::sessions::{self, SessionRouteState};
use crate::database::models::MediaFileType;

/// Default / maximum byte window served by `session_read_danmu` per call.
const DANMU_READ_DEFAULT_BYTES: u64 = 65_536;
const DANMU_READ_MAX_BYTES: u64 = 262_144;

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct SessionListParams {
    /// Maximum number of items to return (default 20, max 100)
    pub limit: Option<u32>,
    /// Number of items to skip (default 0)
    pub offset: Option<u32>,
    /// Filter by streamer ID
    pub streamer_id: Option<String>,
    /// Only sessions started after this RFC 3339 timestamp (e.g. "2026-08-01T00:00:00Z")
    pub from_date: Option<chrono::DateTime<chrono::Utc>>,
    /// Only sessions started before this RFC 3339 timestamp
    pub to_date: Option<chrono::DateTime<chrono::Utc>>,
    /// Only sessions still recording (no end time)
    pub active_only: Option<bool>,
    /// Search query (matches title, streamer name, ...)
    pub search: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct SessionSegmentsParams {
    /// Session ID
    pub id: String,
    /// Maximum number of items to return (default 20, max 100)
    pub limit: Option<u32>,
    /// Number of items to skip (default 0)
    pub offset: Option<u32>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct SessionIdParams {
    /// Session ID
    pub session_id: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct ReadDanmuParams {
    /// Media output ID of a danmu XML file (from session_list_danmu_files)
    pub media_output_id: String,
    /// Byte offset to start reading from (default 0; use the next_offset
    /// value returned by the previous call to continue)
    pub offset_bytes: Option<u64>,
    /// Maximum bytes to return (default 65536, max 262144)
    pub max_bytes: Option<u64>,
}

#[tool_router(router = session_tools, vis = "pub(crate)")]
impl SrecMcpServer {
    #[tool(
        name = "session_list",
        description = "List recording sessions (paginated, filterable by streamer/date/active). Each item includes title history, duration, output count, total size, and danmu count."
    )]
    pub async fn session_list(
        &self,
        Parameters(params): Parameters<SessionListParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let state = SessionRouteState::from_ref(&self.app_state);
        let pagination = PageParams {
            limit: params.limit,
            offset: params.offset,
        }
        .to_pagination();
        let filters = SessionFilterParams {
            streamer_id: params.streamer_id,
            from_date: params.from_date,
            to_date: params.to_date,
            active_only: params.active_only,
            search: params.search,
            include_empty: None,
        };
        tool_json(sessions::list_sessions(State(state), Query(pagination), Query(filters)).await)
    }

    #[tool(
        name = "session_get",
        description = "Get one recording session by ID, including outputs and the event timeline."
    )]
    pub async fn session_get(
        &self,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let state = SessionRouteState::from_ref(&self.app_state);
        tool_json(sessions::get_session(State(state), Path(params.id)).await)
    }

    #[tool(
        name = "session_segments",
        description = "List the recorded file segments of a session (paths, sizes, durations, split reasons)."
    )]
    pub async fn session_segments(
        &self,
        Parameters(params): Parameters<SessionSegmentsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let state = SessionRouteState::from_ref(&self.app_state);
        let pagination = PageParams {
            limit: params.limit,
            offset: params.offset,
        }
        .to_pagination();
        tool_json(
            sessions::list_session_segments(State(state), Path(params.id), Query(pagination)).await,
        )
    }

    #[tool(
        name = "session_danmu_statistics",
        description = "Get aggregated danmu (chat) statistics for a session: total count, rate time series, top talkers, and word frequency. Prefer this over reading raw XML for analysis."
    )]
    pub async fn session_danmu_statistics(
        &self,
        Parameters(params): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let state = SessionRouteState::from_ref(&self.app_state);
        tool_json(sessions::get_session_danmu_statistics(State(state), Path(params.id)).await)
    }

    #[tool(
        name = "session_list_danmu_files",
        description = "List the danmu XML files recorded for a session (one per segment). Returns media_output_id, file path, and size for use with session_read_danmu."
    )]
    pub async fn session_list_danmu_files(
        &self,
        Parameters(params): Parameters<SessionIdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let outputs = match self
            .app_state
            .session_repository
            .get_media_outputs_for_session(&params.session_id)
            .await
        {
            Ok(outputs) => outputs,
            Err(error) => return Ok(api_error_result(ApiError::from(error))),
        };

        let files: Vec<serde_json::Value> = outputs
            .into_iter()
            .filter(|output| output.file_type == MediaFileType::DanmuXml.as_str())
            .map(|output| {
                serde_json::json!({
                    "media_output_id": output.id,
                    "file_path": output.file_path,
                    "size_bytes": output.size_bytes,
                    "created_at": output.created_at,
                })
            })
            .collect();

        Ok(CallToolResult::success(vec![ContentBlock::json(
            serde_json::json!({ "files": files }),
        )?]))
    }

    #[tool(
        name = "session_read_danmu",
        description = "Read a byte window of a danmu XML file (Bilibili-style <d> elements with timestamps and text). Paginate with offset_bytes/next_offset for large files; for aggregate analysis use session_danmu_statistics instead."
    )]
    pub async fn session_read_danmu(
        &self,
        Parameters(params): Parameters<ReadDanmuParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let output = match self
            .app_state
            .session_repository
            .get_media_output(&params.media_output_id)
            .await
        {
            Ok(output) => output,
            Err(error) => return Ok(api_error_result(ApiError::from(error))),
        };

        if output.file_type != MediaFileType::DanmuXml.as_str() {
            return Ok(api_error_result(ApiError::bad_request(format!(
                "Media output '{}' is {} content, not DANMU_XML",
                output.id, output.file_type
            ))));
        }

        let offset = params.offset_bytes.unwrap_or(0);
        let max_bytes = params
            .max_bytes
            .unwrap_or(DANMU_READ_DEFAULT_BYTES)
            .min(DANMU_READ_MAX_BYTES);

        let mut file = match tokio::fs::File::open(&output.file_path).await {
            Ok(file) => file,
            Err(error) => {
                return Ok(api_error_result(ApiError::not_found(format!(
                    "Danmu file '{}' is not readable: {error}",
                    output.file_path
                ))));
            }
        };
        let file_size = match file.metadata().await {
            Ok(metadata) => metadata.len(),
            Err(error) => {
                return Ok(api_error_result(ApiError::internal(format!(
                    "Failed to stat danmu file: {error}"
                ))));
            }
        };

        let mut text = String::new();
        let mut next_offset = None;
        if offset < file_size {
            if let Err(error) = file.seek(std::io::SeekFrom::Start(offset)).await {
                return Ok(api_error_result(ApiError::internal(format!(
                    "Failed to seek danmu file: {error}"
                ))));
            }
            let mut buffer = vec![0u8; max_bytes as usize];
            let mut filled = 0usize;
            loop {
                match file.read(&mut buffer[filled..]).await {
                    Ok(0) => break,
                    Ok(n) => {
                        filled += n;
                        if filled == buffer.len() {
                            break;
                        }
                    }
                    Err(error) => {
                        return Ok(api_error_result(ApiError::internal(format!(
                            "Failed to read danmu file: {error}"
                        ))));
                    }
                }
            }
            buffer.truncate(filled);
            text = String::from_utf8_lossy(&buffer).into_owned();
            let end = offset + filled as u64;
            if end < file_size {
                next_offset = Some(end);
            }
        }

        Ok(CallToolResult::success(vec![ContentBlock::json(
            serde_json::json!({
                "media_output_id": output.id,
                "file_size_bytes": file_size,
                "offset_bytes": offset,
                "returned_bytes": text.len(),
                "next_offset": next_offset,
                "content": text,
            }),
        )?]))
    }

    #[tool(
        name = "session_delete",
        description = "Delete a recording session's database records (recorded files on disk are not removed). Requires a full-access API key."
    )]
    pub async fn session_delete(
        &self,
        Parameters(params): Parameters<IdParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.require_write(&context)?;
        let state = SessionRouteState::from_ref(&self.app_state);
        tool_unit(
            sessions::delete_session(State(state), Path(params.id)).await,
            "Session deleted",
        )
    }
}
