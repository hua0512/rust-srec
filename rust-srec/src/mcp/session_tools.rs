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
    /// value returned by the previous call to continue). Offsets inside a UTF-8
    /// character and invalid UTF-8 in the requested window are rejected.
    pub offset_bytes: Option<u64>,
    /// Maximum bytes to return (default 65536, max 262144). Must be positive
    /// and large enough for the next complete UTF-8 character (up to 4 bytes).
    pub max_bytes: Option<u64>,
}

async fn read_danmu_window(
    file: &mut tokio::fs::File,
    file_size: u64,
    offset: u64,
    max_bytes: u64,
) -> Result<(String, Option<u64>), ApiError> {
    if max_bytes == 0 {
        return Err(ApiError::bad_request("max_bytes must be greater than zero"));
    }
    if offset >= file_size {
        return Ok((String::new(), None));
    }
    let budget = max_bytes.min(DANMU_READ_MAX_BYTES) as usize;
    file.seek(std::io::SeekFrom::Start(offset))
        .await
        .map_err(|error| ApiError::internal(format!("Failed to seek danmu file: {error}")))?;
    // Three lookahead bytes distinguish a scalar split by the requested window
    // from malformed or truncated UTF-8. They are never counted as returned bytes.
    let read_size = (file_size - offset).min(budget as u64 + 3) as usize;
    let mut buffer = vec![0; read_size];
    let mut filled = 0;
    while filled < buffer.len() {
        let count = file
            .read(&mut buffer[filled..])
            .await
            .map_err(|error| ApiError::internal(format!("Failed to read danmu file: {error}")))?;
        if count == 0 {
            break;
        }
        filled += count;
    }
    buffer.truncate(filled);
    if buffer.is_empty() {
        // The file may have been truncated after its size was observed.
        return Ok((String::new(), None));
    }
    if matches!(buffer.first(), Some(0x80..=0xbf)) {
        return Err(ApiError::bad_request(
            "offset_bytes must point to a UTF-8 character boundary",
        ));
    }
    let valid_end = match std::str::from_utf8(&buffer) {
        Ok(text) => text.len(),
        Err(error) if error.valid_up_to() >= budget => error.valid_up_to(),
        Err(error) => {
            return Err(ApiError::bad_request(format!(
                "Danmu file contains invalid or incomplete UTF-8 at byte {}",
                offset + error.valid_up_to() as u64,
            )));
        }
    };
    let valid = std::str::from_utf8(&buffer[..valid_end])
        .map_err(|_| ApiError::internal("Validated danmu prefix could not be decoded"))?;
    let mut end = budget.min(valid.len());
    while !valid.is_char_boundary(end) {
        end -= 1;
    }
    if end == 0 {
        return Err(ApiError::bad_request(
            "max_bytes is too small for the next UTF-8 character; use at least 4 bytes",
        ));
    }
    let next = offset + end as u64;
    // A short read proves EOF even if the original metadata was larger.
    let observed_end = if filled < read_size {
        offset + filled as u64
    } else {
        file_size
    };
    Ok((
        valid[..end].to_owned(),
        (next < observed_end).then_some(next),
    ))
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
        description = "Read a UTF-8 byte window of a danmu XML file. Returned content ends on a character boundary and never exceeds max_bytes; continue with next_offset. Offsets inside a character, invalid UTF-8, zero byte limits, and limits too small for the next character are errors (use at least 4 bytes). Only the requested window is validated; XML nodes may span pages. For aggregate analysis use session_danmu_statistics."
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

        let (text, next_offset) =
            match read_danmu_window(&mut file, file_size, offset, max_bytes).await {
                Ok(page) => page,
                Err(error) => return Ok(api_error_result(error)),
            };

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

#[cfg(test)]
mod tests {
    use super::*;

    async fn file_with_bytes(bytes: &[u8]) -> (tempfile::TempDir, tokio::fs::File) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("danmu.xml");
        tokio::fs::write(&path, bytes).await.unwrap();
        let file = tokio::fs::File::open(&path).await.unwrap();
        (directory, file)
    }

    #[tokio::test]
    async fn multilingual_byte_pages_round_trip_without_replacement_or_lost_bytes() {
        let original = "<i><d>中文é😀か한 العربية e\u{301} &amp;\n</d></i>".repeat(3);
        let (_directory, mut file) = file_with_bytes(original.as_bytes()).await;
        for budget in [4, 5, 7, 16, DANMU_READ_DEFAULT_BYTES] {
            let mut offset = 0;
            let mut recovered = String::new();
            loop {
                let (text, next) =
                    read_danmu_window(&mut file, original.len() as u64, offset, budget)
                        .await
                        .unwrap();
                assert!(!text.is_empty());
                assert!(text.len() as u64 <= budget);
                assert_eq!(
                    text.as_bytes(),
                    &original.as_bytes()[offset as usize..offset as usize + text.len()]
                );
                recovered.push_str(&text);
                match next {
                    Some(next) => {
                        assert_eq!(next, offset + text.len() as u64);
                        assert!(original.is_char_boundary(next as usize));
                        offset = next;
                    }
                    None => break,
                }
            }
            assert_eq!(recovered, original);
        }
    }

    #[tokio::test]
    async fn default_window_preserves_split_emoji_and_eof_offsets() {
        let original = format!("{}😀end", "x".repeat(DANMU_READ_DEFAULT_BYTES as usize - 1));
        let (_directory, mut file) = file_with_bytes(original.as_bytes()).await;
        let (first, next) = read_danmu_window(
            &mut file,
            original.len() as u64,
            0,
            DANMU_READ_DEFAULT_BYTES,
        )
        .await
        .unwrap();
        assert_eq!(first.len(), DANMU_READ_DEFAULT_BYTES as usize - 1);
        let (last, end) = read_danmu_window(
            &mut file,
            original.len() as u64,
            next.unwrap(),
            DANMU_READ_DEFAULT_BYTES,
        )
        .await
        .unwrap();
        assert_eq!(last, "😀end");
        assert!(end.is_none());
        for offset in [original.len() as u64, u64::MAX] {
            assert_eq!(
                read_danmu_window(&mut file, original.len() as u64, offset, 4)
                    .await
                    .unwrap(),
                (String::new(), None)
            );
        }
    }

    #[tokio::test]
    async fn invalid_offsets_encoding_and_nonprogressing_limits_are_errors() {
        let original = "😀中";
        let (_directory, mut file) = file_with_bytes(original.as_bytes()).await;
        for offset in [1, 2, 3, 5, 6] {
            let error = read_danmu_window(&mut file, original.len() as u64, offset, 8)
                .await
                .unwrap_err();
            assert_eq!(error.status, axum::http::StatusCode::BAD_REQUEST);
            assert!(error.message.contains("character boundary"));
        }
        for budget in 0..4 {
            let error = read_danmu_window(&mut file, original.len() as u64, 0, budget)
                .await
                .unwrap_err();
            assert_eq!(error.status, axum::http::StatusCode::BAD_REQUEST);
            assert!(error.message.contains("max_bytes"));
        }
        for invalid in [&b"good\xffbad"[..], &b"x\xf0\x9f"[..], &b"\xed\xa0\x80"[..]] {
            let (_directory, mut file) = file_with_bytes(invalid).await;
            assert!(
                read_danmu_window(&mut file, invalid.len() as u64, 0, 16)
                    .await
                    .is_err()
            );
        }
        // Invalid data beyond the requested window is reported by its own page.
        let (_directory, mut file) = file_with_bytes(b"a\xff").await;
        assert_eq!(
            read_danmu_window(&mut file, 2, 0, 1).await.unwrap(),
            ("a".to_owned(), Some(1))
        );
        assert!(read_danmu_window(&mut file, 2, 1, 1).await.is_err());
    }

    #[tokio::test]
    async fn byte_pages_respect_hard_limit_and_do_not_repeat_offset_after_truncation() {
        let original = vec![b'x'; DANMU_READ_MAX_BYTES as usize + 1];
        let (_directory, mut file) = file_with_bytes(&original).await;
        let (text, next) = read_danmu_window(&mut file, original.len() as u64, 0, u64::MAX)
            .await
            .unwrap();
        assert_eq!(text.len(), DANMU_READ_MAX_BYTES as usize);
        assert_eq!(next, Some(DANMU_READ_MAX_BYTES));
        let (_directory, mut file) = file_with_bytes(b"abc").await;
        assert_eq!(
            read_danmu_window(&mut file, 100, 0, 8).await.unwrap(),
            ("abc".to_owned(), None)
        );
        assert_eq!(
            read_danmu_window(&mut file, 100, 3, 8).await.unwrap(),
            (String::new(), None)
        );
    }
}
