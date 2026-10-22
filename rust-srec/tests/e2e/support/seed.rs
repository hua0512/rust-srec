//! Database seeds for scenarios where the scheduler monitors streamers.

use std::fmt::Display;

use serde_json::Value;

use super::fake_platform::{FAKE_PLATFORM_ID, FAKE_PLATFORM_NAME, FakeRoom};
use super::http::ConnectRecorder;

const DEAD_END_PROXY_ID: &str = "proxy-dead-end";

/// Settings applied before the backend boots, for
/// [`super::TestApp::start_with`].
///
/// [`Self::live`] makes monitoring fast and fail-closed: a check every 200 ms,
/// one offline signal to confirm an end, offline checks as close together as
/// the backend allows (it raises the seeded 100 ms to its 1 s floor), no
/// minimum segment size, no thumbnail jobs, and a global route through a
/// [`ConnectRecorder`], so every platform but the fake one (routed `direct`)
/// reaches only that recorder. The scheduler and the session lifecycle read
/// the timing when the services are built, which is why it is seeded rather
/// than set over HTTP.
pub struct Seed {
    global: Vec<(String, String)>,
    platform: Vec<(String, String)>,
    rows: String,
}

/// `value` as an SQL string literal.
pub fn sql_text(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// A streamer row on `room` in `state`, with `columns` set to SQL literals,
/// for [`Seed::rows`].
pub fn seeded_streamer(id: &str, room: &FakeRoom, state: &str, columns: &[(&str, &str)]) -> String {
    let names: String = columns
        .iter()
        .map(|(name, _)| format!(", {name}"))
        .collect();
    let values: String = columns
        .iter()
        .map(|(_, value)| format!(", {value}"))
        .collect();
    format!(
        "INSERT INTO streamers (id, name, url, platform_config_id, priority, state{names}) \
         VALUES ('{id}', '{id}', '{url}', '{FAKE_PLATFORM_ID}', 'NORMAL', '{state}'{values});\n",
        url = room.url(),
    )
}

fn set(columns: &mut Vec<(String, String)>, column: &str, literal: String) {
    columns.retain(|(existing, _)| existing != column);
    columns.push((column.to_owned(), literal));
}

impl Seed {
    pub fn live() -> Self {
        Self {
            global: Vec::new(),
            platform: Vec::new(),
            rows: String::new(),
        }
        .global("streamer_check_delay_ms", 200)
        .global("offline_check_delay_ms", 100)
        .global("offline_check_count", 1)
        .global("min_segment_size_bytes", 0)
        .global("auto_thumbnail", 0)
        .platform(
            "download_retry_policy",
            sql_text(r#"{"max_retries":1,"initial_delay_ms":10}"#),
        )
    }

    /// Set a `global_config` column to an SQL literal.
    pub fn global(mut self, column: &str, literal: impl Display) -> Self {
        set(&mut self.global, column, literal.to_string());
        self
    }

    /// Set a column of the fake platform's `platform_config` row to an SQL
    /// literal.
    pub fn platform(mut self, column: &str, literal: impl Display) -> Self {
        set(&mut self.platform, column, literal.to_string());
        self
    }

    /// Run `sql` after the settings, for rows the services load when they
    /// start: a streamer's state and counters, or a session a restart left
    /// open.
    pub fn rows(mut self, sql: &str) -> Self {
        self.rows.push_str(sql);
        self
    }

    /// Run `dag` when a fake-platform session ends.
    pub fn session_complete_pipeline(self, dag: &Value) -> Self {
        self.platform("session_complete_pipeline", sql_text(&dag.to_string()))
    }

    pub(super) fn sql(&self, dead_end: &ConnectRecorder) -> String {
        let now = chrono::Utc::now().timestamp_millis();
        let mut global = self.global.clone();
        set(&mut global, "proxy_route", sql_text("proxy"));
        set(&mut global, "proxy_id", sql_text(DEAD_END_PROXY_ID));
        let mut platform = vec![
            ("id".to_owned(), sql_text(FAKE_PLATFORM_ID)),
            (
                "platform_name".to_owned(),
                sql_text(&FAKE_PLATFORM_NAME.to_lowercase()),
            ),
            ("proxy_route".to_owned(), sql_text("direct")),
            ("record_danmu".to_owned(), "0".to_owned()),
        ];
        platform.extend(self.platform.iter().cloned());
        let assignments = global
            .iter()
            .map(|(column, literal)| format!("{column} = {literal}"))
            .collect::<Vec<_>>()
            .join(", ");
        let (columns, values): (Vec<_>, Vec<_>) = platform.into_iter().unzip();
        format!(
            "INSERT INTO proxies (id, name, url, created_at, updated_at) \
             VALUES ({id}, 'Dead end', {url}, {now}, {now});\n\
             UPDATE global_config SET {assignments};\n\
             INSERT INTO platform_config ({columns}) VALUES ({values});\n\
             {rows}",
            id = sql_text(DEAD_END_PROXY_ID),
            url = sql_text(&dead_end.proxy_url()),
            columns = columns.join(", "),
            values = values.join(", "),
            rows = self.rows,
        )
    }
}
