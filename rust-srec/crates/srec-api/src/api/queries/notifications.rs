//! Filtered notification event log queries.

use sqlx::QueryBuilder;

use crate::Result;
use crate::database::models::NotificationEventLogDbModel;
use crate::database::repositories::literal_substring_pattern;

use super::ApiQueries;

impl ApiQueries {
    /// List notification event logs with optional filtering and pagination.
    pub async fn list_event_logs(
        &self,
        event_type: Option<&str>,
        streamer_id: Option<&str>,
        search: Option<&str>,
        priority: Option<&str>,
        offset: i32,
        limit: i32,
    ) -> Result<Vec<NotificationEventLogDbModel>> {
        let limit = limit.clamp(1, 1000);
        let offset = offset.max(0);

        let mut query = QueryBuilder::new("SELECT nel.* FROM notification_event_log nel");
        if search.is_some() {
            query.push(" LEFT JOIN streamers s ON nel.streamer_id = s.id");
        }
        let mut separator = " WHERE ";
        let mut next_separator = || std::mem::replace(&mut separator, " AND ");

        if let Some(search) = search {
            let pattern = literal_substring_pattern(search);
            query
                .push(next_separator())
                .push("(LOWER(s.name) LIKE LOWER(")
                .push_bind(pattern.clone())
                .push(r") ESCAPE '\' OR LOWER(nel.payload) LIKE LOWER(")
                .push_bind(pattern)
                .push(r") ESCAPE '\')");
        }
        if let Some(event_type) = event_type {
            query
                .push(next_separator())
                .push("nel.event_type = ")
                .push_bind(event_type);
        }
        if let Some(streamer_id) = streamer_id {
            query
                .push(next_separator())
                .push("nel.streamer_id = ")
                .push_bind(streamer_id);
        }
        if let Some(p) = priority {
            // Accept both integer (new format) and string (legacy format).
            let priority_value: i32 = if let Ok(int_val) = p.parse::<i32>() {
                int_val
            } else {
                match p.trim() {
                    s if s.eq_ignore_ascii_case("low") => 2,
                    s if s.eq_ignore_ascii_case("normal") => 5,
                    s if s.eq_ignore_ascii_case("high") => 8,
                    s if s.eq_ignore_ascii_case("critical") => 10,
                    _ => 2,
                }
            };
            query
                .push(next_separator())
                .push("nel.priority >= ")
                .push_bind(priority_value);
        }
        query
            .push(" ORDER BY nel.created_at DESC LIMIT ")
            .push_bind(limit)
            .push(" OFFSET ")
            .push_bind(offset);

        let entries = query
            .build_query_as::<NotificationEventLogDbModel>()
            .fetch_all(&self.pool)
            .await?;
        Ok(entries)
    }
}
