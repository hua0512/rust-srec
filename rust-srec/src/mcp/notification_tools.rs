//! Notification channel MCP tools.
//!
//! Thin wrappers over the `api::routes::notifications` handlers.

use axum::Json;
use axum::extract::{FromRef, Path, Query, State};
use rmcp::{
    ErrorData, RoleServer, handler::server::wrapper::Parameters, model::CallToolResult, schemars,
    service::RequestContext, tool, tool_router,
};

use super::config_tools::IdParams;
use super::{SrecMcpServer, tool_json, tool_status};
use crate::api::routes::notifications::{
    self, CreateChannelRequest, ListEventsQuery, NotificationRouteState, UpdateChannelRequest,
    UpdateSubscriptionsRequest,
};

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct CreateChannelParams {
    /// Channel display name
    pub name: String,
    /// Channel type: "Discord", "Email", "Gotify", "Telegram", or "Webhook"
    pub channel_type: String,
    /// Channel-type-specific settings object (e.g. webhook URL, SMTP
    /// settings). Same shape as `POST /api/notifications/channels` in the
    /// OpenAPI spec at /api/docs.
    pub settings: serde_json::Value,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct UpdateChannelParams {
    /// Channel ID
    pub id: String,
    /// Channel display name
    pub name: String,
    /// Channel-type-specific settings object (full replacement)
    pub settings: serde_json::Value,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct UpdateSubscriptionsParams {
    /// Channel ID
    pub id: String,
    /// Event type names the channel should receive (see
    /// notification_list_event_types for the catalog)
    pub events: Vec<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct ListEventLogParams {
    /// Max number of events to return (default 200, max 1000)
    pub limit: Option<i32>,
    /// Number of events to skip (default 0)
    pub offset: Option<i32>,
    /// Filter by event type name
    pub event_type: Option<String>,
    /// Filter by streamer ID
    pub streamer_id: Option<String>,
    /// Search by streamer name (case-insensitive)
    pub search: Option<String>,
    /// Filter by minimum priority level (low, normal, high, critical)
    pub priority: Option<String>,
}

#[tool_router(router = notification_tools, vis = "pub(crate)")]
impl SrecMcpServer {
    #[tool(
        name = "notification_list_event_types",
        description = "List all notification event types that channels can subscribe to (live start/end, recording events, errors, ...)."
    )]
    pub async fn notification_list_event_types(&self) -> Result<CallToolResult, ErrorData> {
        let Json(value) = notifications::list_event_types().await;
        tool_json(Ok(Json(value)))
    }

    #[tool(
        name = "notification_list_channels",
        description = "List configured notification channels (Discord, email, webhook, Telegram, ...). Requires a full-access API key because channel settings contain credentials."
    )]
    pub async fn notification_list_channels(
        &self,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.require_full_access(&context)?;
        let state = NotificationRouteState::from_ref(&self.app_state);
        tool_json(notifications::list_channels(State(state)).await)
    }

    #[tool(
        name = "notification_get_channel",
        description = "Get one notification channel by ID. Requires a full-access API key because channel settings contain credentials."
    )]
    pub async fn notification_get_channel(
        &self,
        Parameters(params): Parameters<IdParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.require_full_access(&context)?;
        let state = NotificationRouteState::from_ref(&self.app_state);
        tool_json(notifications::get_channel(State(state), Path(params.id)).await)
    }

    #[tool(
        name = "notification_create_channel",
        description = "Create a notification channel. Requires a full-access API key."
    )]
    pub async fn notification_create_channel(
        &self,
        Parameters(params): Parameters<CreateChannelParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.require_write(&context)?;
        let channel_type =
            serde_json::from_value(serde_json::Value::String(params.channel_type.clone()))
                .map_err(|error| {
                    ErrorData::invalid_params(
                        format!(
                            "Invalid 'channel_type' value '{}': {error}",
                            params.channel_type
                        ),
                        None,
                    )
                })?;
        let request = CreateChannelRequest {
            name: params.name,
            channel_type,
            settings: params.settings,
        };
        let state = NotificationRouteState::from_ref(&self.app_state);
        tool_json(notifications::create_channel(State(state), Json(request)).await)
    }

    #[tool(
        name = "notification_update_channel",
        description = "Update a notification channel's name and settings. Requires a full-access API key."
    )]
    pub async fn notification_update_channel(
        &self,
        Parameters(params): Parameters<UpdateChannelParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.require_write(&context)?;
        let request = UpdateChannelRequest {
            name: params.name,
            settings: params.settings,
        };
        let state = NotificationRouteState::from_ref(&self.app_state);
        tool_json(notifications::update_channel(State(state), Path(params.id), Json(request)).await)
    }

    #[tool(
        name = "notification_delete_channel",
        description = "Delete a notification channel. Requires a full-access API key."
    )]
    pub async fn notification_delete_channel(
        &self,
        Parameters(params): Parameters<IdParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.require_write(&context)?;
        let state = NotificationRouteState::from_ref(&self.app_state);
        tool_status(
            notifications::delete_channel(State(state), Path(params.id)).await,
            "Notification channel deleted",
        )
    }

    #[tool(
        name = "notification_test_channel",
        description = "Send a test notification through a channel. Requires a full-access API key."
    )]
    pub async fn notification_test_channel(
        &self,
        Parameters(params): Parameters<IdParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.require_write(&context)?;
        let state = NotificationRouteState::from_ref(&self.app_state);
        tool_status(
            notifications::test_channel(State(state), Path(params.id)).await,
            "Test notification sent",
        )
    }

    #[tool(
        name = "notification_get_subscriptions",
        description = "Get the event types a notification channel is subscribed to. Requires a full-access API key."
    )]
    pub async fn notification_get_subscriptions(
        &self,
        Parameters(params): Parameters<IdParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.require_full_access(&context)?;
        let state = NotificationRouteState::from_ref(&self.app_state);
        tool_json(notifications::get_subscriptions(State(state), Path(params.id)).await)
    }

    #[tool(
        name = "notification_update_subscriptions",
        description = "Replace the event subscriptions of a notification channel. Requires a full-access API key."
    )]
    pub async fn notification_update_subscriptions(
        &self,
        Parameters(params): Parameters<UpdateSubscriptionsParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.require_write(&context)?;
        let request = UpdateSubscriptionsRequest {
            events: params.events,
        };
        let state = NotificationRouteState::from_ref(&self.app_state);
        tool_json(
            notifications::update_subscriptions(State(state), Path(params.id), Json(request)).await,
        )
    }

    #[tool(
        name = "notification_event_log",
        description = "List recently sent notification events (delivery log)."
    )]
    pub async fn notification_event_log(
        &self,
        Parameters(params): Parameters<ListEventLogParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let state = NotificationRouteState::from_ref(&self.app_state);
        let query = ListEventsQuery {
            limit: params.limit,
            offset: params.offset,
            event_type: params.event_type,
            streamer_id: params.streamer_id,
            search: params.search,
            priority: params.priority,
        };
        tool_json(notifications::list_events(State(state), Query(query)).await)
    }
}
