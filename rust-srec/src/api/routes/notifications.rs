use axum::Json;
use axum::Router;
use axum::routing::get;

use crate::api::server::AppState;
use crate::notification::events::{NotificationEventTypeInfo, notification_event_types};

pub fn router() -> Router<AppState> {
    Router::new().route("/event-types", get(list_event_types))
}

async fn list_event_types() -> Json<Vec<NotificationEventTypeInfo>> {
    Json(notification_event_types().to_vec())
}
