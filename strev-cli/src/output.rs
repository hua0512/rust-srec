use crate::{cli::OutputFormat, error::Result};
#[cfg(feature = "colored-output")]
use colored::*;
use platforms_parser::media::{MediaInfo, StreamInfo, is_sensitive_key};
use std::borrow::Cow;
use std::io::Write;
#[cfg(feature = "table-output")]
use tabled::{Table, Tabled, settings::Style};

pub struct OutputManager {
    colored: bool,
}

impl OutputManager {
    pub fn new(colored: bool) -> Self {
        Self { colored }
    }

    pub fn format_media_info(
        &self,
        media_info: &MediaInfo,
        stream_info: Option<&StreamInfo>,
        format: &OutputFormat,
    ) -> Result<String> {
        match format {
            OutputFormat::Pretty => self.format_pretty(media_info, stream_info),
            OutputFormat::Json => self.format_json(media_info, true),
            OutputFormat::JsonCompact => self.format_json(media_info, false),
            #[cfg(feature = "table-output")]
            OutputFormat::Table => self.format_table(media_info, stream_info),
            #[cfg(not(feature = "table-output"))]
            OutputFormat::Table => {
                // Fallback to pretty format when table feature is disabled
                self.format_pretty(media_info, stream_info)
            }
            OutputFormat::Csv => self.format_csv(media_info, stream_info),
        }
    }
    pub fn format_stream_info(
        &self,
        stream_info: &StreamInfo,
        format: &OutputFormat,
    ) -> Result<String> {
        match format {
            OutputFormat::Pretty => self.format_stream_pretty(stream_info),
            OutputFormat::Json => self.format_stream_json(stream_info, true),
            OutputFormat::JsonCompact => self.format_stream_json(stream_info, false),
            #[cfg(feature = "table-output")]
            OutputFormat::Table => self.format_stream_table(stream_info),
            #[cfg(not(feature = "table-output"))]
            OutputFormat::Table => self.format_stream_pretty(stream_info),
            OutputFormat::Csv => self.format_stream_csv(stream_info),
        }
    }

    fn format_stream_pretty(&self, stream: &StreamInfo) -> Result<String> {
        let mut output = String::new();
        output.push_str(&self.colorize("Stream Details:", &Color::Green, true));
        output.push('\n');

        output.push_str(&format!(
            "  {}: {}\n",
            self.colorize("Format", &Color::Yellow, false),
            self.colorize(stream.stream_format.as_str(), &Color::Cyan, false)
        ));
        output.push_str(&format!(
            "  {}: {}\n",
            self.colorize("Quality", &Color::Yellow, false),
            self.colorize(&stream.quality, &Color::Cyan, false)
        ));
        if stream.is_audio_only {
            output.push_str(&format!(
                "  {}: {}\n",
                self.colorize("Audio Only", &Color::Yellow, false),
                self.colorize("🔊 Yes", &Color::Cyan, false)
            ));
        }
        output.push_str(&format!(
            "  {}: {}\n",
            self.colorize("URL", &Color::Yellow, false),
            self.colorize(stream.url.as_str(), &Color::Blue, false)
        ));
        output.push_str(&format!(
            "  {}: {} kbps\n",
            self.colorize("Bitrate", &Color::Yellow, false),
            self.colorize(&stream.bitrate.to_string(), &Color::Cyan, false)
        ));
        output.push_str(&format!(
            "  {}: {}\n",
            self.colorize("Media Format", &Color::Yellow, false),
            self.colorize(stream.media_format.as_str(), &Color::Cyan, false)
        ));
        output.push_str(&format!(
            "  {}: {}\n",
            self.colorize("Codec", &Color::Yellow, false),
            self.colorize(&stream.codec, &Color::Cyan, false)
        ));
        output.push_str(&format!(
            "  {}: {}\n",
            self.colorize("FPS", &Color::Yellow, false),
            self.colorize(&stream.fps.to_string(), &Color::Cyan, false)
        ));
        output.push_str(&format!(
            "  {}: {}\n",
            self.colorize("Priority", &Color::Yellow, false),
            self.colorize(&stream.priority.to_string(), &Color::Cyan, false)
        ));

        if let Some(extras) = &stream.extras {
            output.push_str(&format!(
                "  {}:\n",
                self.colorize("Extras", &Color::Yellow, false)
            ));
            if let Some(extras_map) = extras.as_object() {
                for (key, value) in extras_map {
                    let rendered = Self::display_json_extra(key, value);
                    output.push_str(&format!(
                        "    {}: {}\n",
                        self.colorize(key, &Color::Green, false),
                        self.colorize(&rendered, &Color::Cyan, false)
                    ));
                }
            }
        }
        Ok(output)
    }

    fn format_stream_json(&self, stream_info: &StreamInfo, pretty: bool) -> Result<String> {
        let stream_data = stream_info.to_value()?;
        if pretty {
            serde_json::to_string_pretty(&stream_data)
        } else {
            serde_json::to_string(&stream_data)
        }
        .map_err(Into::into)
    }

    #[cfg(feature = "table-output")]
    fn format_stream_table(&self, stream_info: &StreamInfo) -> Result<String> {
        #[derive(Tabled)]
        struct StreamTableRow<'a> {
            property: &'a str,
            value: Cow<'a, str>,
        }

        let mut rows = vec![
            StreamTableRow {
                property: "Format",
                value: Cow::Borrowed(stream_info.stream_format.as_str()),
            },
            StreamTableRow {
                property: "Quality",
                value: Cow::Borrowed(&stream_info.quality),
            },
            StreamTableRow {
                property: "URL",
                value: Cow::Borrowed(stream_info.url.as_str()),
            },
            StreamTableRow {
                property: "Bitrate",
                value: Cow::Owned(format!("{} kbps", stream_info.bitrate)),
            },
            StreamTableRow {
                property: "Media Format",
                value: Cow::Borrowed(stream_info.media_format.as_str()),
            },
            StreamTableRow {
                property: "Codec",
                value: Cow::Borrowed(&stream_info.codec),
            },
            StreamTableRow {
                property: "FPS",
                value: Cow::Owned(stream_info.fps.to_string()),
            },
            StreamTableRow {
                property: "Priority",
                value: Cow::Owned(stream_info.priority.to_string()),
            },
        ];

        if stream_info.is_audio_only {
            rows.push(StreamTableRow {
                property: "Audio Only",
                value: Cow::Borrowed("🔊 Yes"),
            });
        }

        if let Some(extras) = &stream_info.extras
            && let Some(extras_obj) = extras.as_object()
        {
            for (key, value) in extras_obj {
                rows.push(StreamTableRow {
                    property: key,
                    value: Cow::Owned(Self::display_json_extra(key, value)),
                });
            }
        }

        let table = Table::new(rows).with(Style::modern()).to_string();
        Ok(table)
    }

    fn format_stream_csv(&self, stream_info: &StreamInfo) -> Result<String> {
        let mut output = String::new();
        let mut headers: Vec<Cow<'_, str>> = vec![
            Cow::Borrowed("quality"),
            Cow::Borrowed("stream_format"),
            Cow::Borrowed("media_format"),
            Cow::Borrowed("url"),
            Cow::Borrowed("bitrate"),
            Cow::Borrowed("codec"),
            Cow::Borrowed("fps"),
            Cow::Borrowed("priority"),
            Cow::Borrowed("is_audio_only"),
        ];

        let mut extras_keys = Vec::new();
        if let Some(extras) = &stream_info.extras
            && let Some(extras_obj) = extras.as_object()
        {
            for key in extras_obj.keys() {
                headers.push(Self::csv_field(key));
                extras_keys.push(key.as_str());
            }
        }
        output.push_str(&headers.join(","));
        output.push('\n');

        let stream_format_str = stream_info.stream_format.as_str();
        let media_format_str = stream_info.media_format.as_str();

        let mut record = vec![
            Self::csv_field(&stream_info.quality),
            Self::csv_field(stream_format_str),
            Self::csv_field(media_format_str),
            Self::csv_field(&stream_info.url),
            Cow::Owned(stream_info.bitrate.to_string()),
            Self::csv_field(&stream_info.codec),
            Cow::Owned(stream_info.fps.to_string()),
            Cow::Owned(stream_info.priority.to_string()),
            Cow::Owned(stream_info.is_audio_only.to_string()),
        ];

        if let Some(extras) = &stream_info.extras
            && let Some(extras_obj) = extras.as_object()
        {
            for key in extras_keys {
                let cell = if is_sensitive_key(key) {
                    Cow::Borrowed("[redacted]")
                } else {
                    match extras_obj.get(key) {
                        None | Some(serde_json::Value::Null) => Cow::Borrowed(""),
                        Some(serde_json::Value::String(s)) => Self::csv_field(s),
                        Some(other) => Cow::Owned(Self::csv_field(&other.to_string()).into_owned()),
                    }
                };
                record.push(cell);
            }
        }

        output.push_str(&record.join(","));
        output.push('\n');

        Ok(output)
    }

    fn format_pretty(
        &self,
        media_info: &MediaInfo,
        stream_info: Option<&StreamInfo>,
    ) -> Result<String> {
        let mut output = String::new();

        // Media Information
        output.push_str(&self.colorize("Media Information:", &Color::Green, true));
        output.push('\n');

        output.push_str(&format!(
            "  {}: {}\n",
            self.colorize("Artist", &Color::Yellow, false),
            self.colorize(&media_info.artist, &Color::Cyan, false)
        ));

        output.push_str(&format!(
            "  {}: {}\n",
            self.colorize("Title", &Color::Yellow, false),
            self.colorize(&media_info.title, &Color::Cyan, false)
        ));

        output.push_str(&format!(
            "  {}: {}\n",
            self.colorize("Live", &Color::Yellow, false),
            self.colorize(&media_info.is_live.to_string(), &Color::Cyan, false)
        ));

        if let Some(ref categories) = media_info.category
            && !categories.is_empty()
        {
            output.push_str(&format!(
                "  {}: {}\n",
                self.colorize("Category", &Color::Yellow, false),
                self.colorize(&categories.join(", "), &Color::Cyan, false)
            ));
        }

        if let Some(ref start_time) = media_info.live_start_time {
            let time_str = start_time.strftime("%Y-%m-%d %H:%M:%S UTC").to_string();
            output.push_str(&format!(
                "  {}: {}\n",
                self.colorize("Live Started", &Color::Yellow, false),
                self.colorize(&time_str, &Color::Cyan, false)
            ));
        }

        if let Some(cover_url) = &media_info.cover_url {
            output.push_str(&format!(
                "  {}: {}\n",
                self.colorize("Cover URL", &Color::Yellow, false),
                self.colorize(cover_url, &Color::Blue, false)
            ));
        }

        if let Some(artist_url) = &media_info.artist_url {
            output.push_str(&format!(
                "  {}: {}\n",
                self.colorize("Artist URL", &Color::Yellow, false),
                self.colorize(artist_url, &Color::Blue, false)
            ));
        }

        // Stream Information
        if let Some(stream) = stream_info {
            output.push('\n');
            output.push_str(&self.colorize("Selected Stream Details:", &Color::Green, true));
            output.push('\n');

            output.push_str(&format!(
                "  {}: {}\n",
                self.colorize("Format", &Color::Yellow, false),
                self.colorize(stream.stream_format.as_str(), &Color::Cyan, false)
            ));

            output.push_str(&format!(
                "  {}: {}\n",
                self.colorize("Quality", &Color::Yellow, false),
                self.colorize(&stream.quality, &Color::Cyan, false)
            ));

            output.push_str(&format!(
                "  {}: {}\n",
                self.colorize("URL", &Color::Yellow, false),
                self.colorize(stream.url.as_str(), &Color::Blue, false)
            ));

            output.push_str(&format!(
                "  {}: {} kbps\n",
                self.colorize("Bitrate", &Color::Yellow, false),
                self.colorize(&stream.bitrate.to_string(), &Color::Cyan, false)
            ));

            output.push_str(&format!(
                "  {}: {}\n",
                self.colorize("Media Format", &Color::Yellow, false),
                self.colorize(stream.media_format.as_str(), &Color::Cyan, false)
            ));

            output.push_str(&format!(
                "  {}: {}\n",
                self.colorize("Codec", &Color::Yellow, false),
                self.colorize(&stream.codec, &Color::Cyan, false)
            ));

            output.push_str(&format!(
                "  {}: {}\n",
                self.colorize("FPS", &Color::Yellow, false),
                self.colorize(&stream.fps.to_string(), &Color::Cyan, false)
            ));

            output.push_str(&format!(
                "  {}: {}\n",
                self.colorize("Priority", &Color::Yellow, false),
                self.colorize(&stream.priority.to_string(), &Color::Cyan, false)
            ));
            if stream.is_audio_only {
                output.push_str(&format!(
                    "  {}: {}\n",
                    self.colorize("Audio Only", &Color::Yellow, false),
                    self.colorize("🔊 Yes", &Color::Cyan, false)
                ));
            }
            if let Some(extras_obj) = stream
                .extras
                .as_ref()
                .and_then(|v| v.as_object())
                .filter(|m| !m.is_empty())
            {
                output.push_str(&format!(
                    "  {}:\n",
                    self.colorize("Extras", &Color::Yellow, false)
                ));
                for (key, value) in extras_obj {
                    let rendered = Self::display_json_extra(key, value);
                    output.push_str(&format!(
                        "    {}: {}\n",
                        self.colorize(key, &Color::Green, false),
                        self.colorize(&rendered, &Color::Cyan, false)
                    ));
                }
            }
        }

        // Media Extras
        if let Some(extras) = &media_info.extras
            && !extras.is_empty()
        {
            output.push('\n');
            output.push_str(&self.colorize("Media Extras:", &Color::Green, true));
            output.push('\n');
            for (key, value) in extras {
                let rendered = Self::display_str_entry(key, value);
                output.push_str(&format!(
                    "  {}: {}\n",
                    self.colorize(key, &Color::Yellow, false),
                    self.colorize(rendered, &Color::Cyan, false)
                ));
            }
        }

        // Headers
        if let Some(headers) = &media_info.headers
            && !headers.is_empty()
        {
            output.push('\n');
            output.push_str(&self.colorize("Headers:", &Color::Green, true));
            output.push('\n');
            for (key, value) in headers {
                let rendered = Self::display_str_entry(key, value);
                output.push_str(&format!(
                    "  {}: {}\n",
                    self.colorize(key, &Color::Yellow, false),
                    self.colorize(rendered, &Color::Cyan, false)
                ));
            }
        }

        Ok(output)
    }

    fn format_json(&self, media_info: &MediaInfo, pretty: bool) -> Result<String> {
        let media_data = media_info.to_value()?;
        let output_data = serde_json::json!({ "media": media_data });

        if pretty {
            serde_json::to_string_pretty(&output_data)
        } else {
            serde_json::to_string(&output_data)
        }
        .map_err(Into::into)
    }

    #[cfg(feature = "table-output")]
    fn format_table(
        &self,
        media_info: &MediaInfo,
        stream_info: Option<&StreamInfo>,
    ) -> Result<String> {
        #[derive(Tabled)]
        struct TableRow<'a> {
            property: &'a str,
            value: Cow<'a, str>,
        }

        let mut rows = vec![
            TableRow {
                property: "Artist",
                value: Cow::Borrowed(&media_info.artist),
            },
            TableRow {
                property: "Title",
                value: Cow::Borrowed(&media_info.title),
            },
            TableRow {
                property: "Live",
                value: Cow::Owned(media_info.is_live.to_string()),
            },
        ];

        if let Some(ref categories) = media_info.category
            && !categories.is_empty()
        {
            rows.push(TableRow {
                property: "Category",
                value: Cow::Owned(categories.join(", ")),
            });
        }

        if let Some(ref start_time) = media_info.live_start_time {
            rows.push(TableRow {
                property: "Live Started",
                value: Cow::Owned(start_time.strftime("%Y-%m-%d %H:%M:%S UTC").to_string()),
            });
        }

        if let Some(cover_url) = &media_info.cover_url {
            rows.push(TableRow {
                property: "Cover URL",
                value: Cow::Borrowed(cover_url),
            });
        }

        if let Some(artist_url) = &media_info.artist_url {
            rows.push(TableRow {
                property: "Artist URL",
                value: Cow::Borrowed(artist_url),
            });
        }

        if let Some(extras) = &media_info.extras {
            for (key, value) in extras {
                rows.push(TableRow {
                    property: key,
                    value: Cow::Borrowed(Self::display_str_entry(key, value)),
                });
            }
        }

        if let Some(headers) = &media_info.headers {
            for (key, value) in headers {
                rows.push(TableRow {
                    property: key,
                    value: Cow::Borrowed(Self::display_str_entry(key, value)),
                });
            }
        }

        if let Some(stream) = stream_info {
            rows.push(TableRow {
                property: "Stream Format",
                value: Cow::Borrowed(stream.stream_format.as_str()),
            });
            rows.push(TableRow {
                property: "Quality",
                value: Cow::Borrowed(&stream.quality),
            });
            rows.push(TableRow {
                property: "Stream URL",
                value: Cow::Borrowed(stream.url.as_str()),
            });
            rows.push(TableRow {
                property: "Bitrate",
                value: Cow::Owned(format!("{} kbps", stream.bitrate)),
            });
            rows.push(TableRow {
                property: "Media Format",
                value: Cow::Borrowed(stream.media_format.as_str()),
            });
            rows.push(TableRow {
                property: "Codec",
                value: Cow::Borrowed(&stream.codec),
            });
            rows.push(TableRow {
                property: "FPS",
                value: Cow::Owned(stream.fps.to_string()),
            });
            if stream.is_audio_only {
                rows.push(TableRow {
                    property: "Audio Only",
                    value: Cow::Borrowed("🔊 Yes"),
                });
            }
        }

        let table = Table::new(rows).with(Style::modern()).to_string();
        Ok(table)
    }

    fn format_csv(
        &self,
        media_info: &MediaInfo,
        stream_info: Option<&StreamInfo>,
    ) -> Result<String> {
        let category_str = media_info
            .category
            .as_ref()
            .filter(|c| !c.is_empty())
            .map(|c| c.join(", "));
        let start_time_str = media_info
            .live_start_time
            .map(|t| t.strftime("%Y-%m-%d %H:%M:%S UTC").to_string());
        let user_agent = media_info
            .headers
            .as_ref()
            .and_then(|h| h.get("User-Agent").map(String::as_str))
            .unwrap_or("");

        if let Some(stream) = stream_info {
            let mut output = String::new();
            output.push_str("property,value\n");

            output.push_str(&format!("artist,{}\n", Self::csv_field(&media_info.artist)));
            output.push_str(&format!("title,{}\n", Self::csv_field(&media_info.title)));
            output.push_str(&format!("is_live,{}\n", media_info.is_live));

            if let Some(ref categories) = category_str {
                output.push_str(&format!("category,{}\n", Self::csv_field(categories)));
            }

            if let Some(ref started) = start_time_str {
                output.push_str(&format!("live_started,{}\n", Self::csv_field(started)));
            }

            if let Some(cover_url) = &media_info.cover_url {
                output.push_str(&format!("cover_url,{}\n", Self::csv_field(cover_url)));
            }

            if let Some(artist_url) = &media_info.artist_url {
                output.push_str(&format!("artist_url,{}\n", Self::csv_field(artist_url)));
            }

            output.push_str(&format!("user_agent,{}\n", Self::csv_field(user_agent)));
            output.push_str(&format!(
                "stream_format,{}\n",
                Self::csv_field(stream.stream_format.as_str())
            ));
            output.push_str(&format!("quality,{}\n", Self::csv_field(&stream.quality)));
            output.push_str(&format!("url,{}\n", Self::csv_field(stream.url.as_str())));
            output.push_str(&format!("bitrate,{}\n", stream.bitrate));
            output.push_str(&format!(
                "media_format,{}\n",
                Self::csv_field(stream.media_format.as_str())
            ));
            output.push_str(&format!("codec,{}\n", Self::csv_field(&stream.codec)));
            output.push_str(&format!("fps,{}\n", stream.fps));
            output.push_str(&format!("priority,{}\n", stream.priority));
            if stream.is_audio_only {
                output.push_str("is_audio_only,true\n");
            }
            if let Some(extras_obj) = stream.extras.as_ref().and_then(|v| v.as_object()) {
                for (key, value) in extras_obj {
                    let rendered = Self::display_json_extra(key, value);
                    output.push_str(&format!(
                        "{},{}\n",
                        Self::csv_field(key),
                        Self::csv_field(&rendered)
                    ));
                }
            }
            if let Some(extras) = &media_info.extras {
                for (key, value) in extras {
                    let rendered = Self::display_str_entry(key, value);
                    output.push_str(&format!(
                        "{},{}\n",
                        Self::csv_field(key),
                        Self::csv_field(rendered)
                    ));
                }
            }
            if let Some(headers) = &media_info.headers {
                for (key, value) in headers {
                    let rendered = Self::display_str_entry(key, value);
                    output.push_str(&format!(
                        "{},{}\n",
                        Self::csv_field(key),
                        Self::csv_field(rendered)
                    ));
                }
            }

            Ok(output)
        } else {
            let mut output = String::new();
            let headers = [
                "artist",
                "title",
                "is_live",
                "category",
                "live_started",
                "cover_url",
                "artist_url",
                "user_agent",
                "stream_format",
                "quality",
                "url",
                "bitrate",
                "media_format",
                "codec",
                "fps",
                "priority",
                "is_audio_only",
            ];
            output.push_str(&headers.join(","));
            output.push('\n');

            let category_cell = Self::csv_field(category_str.as_deref().unwrap_or(""));
            let start_time_cell = Self::csv_field(start_time_str.as_deref().unwrap_or(""));
            let cover_cell = Self::csv_field(media_info.cover_url.as_deref().unwrap_or(""));
            let artist_url_cell = Self::csv_field(media_info.artist_url.as_deref().unwrap_or(""));
            let ua_cell = Self::csv_field(user_agent);

            if media_info.streams.is_empty() {
                let row: [Cow<str>; 17] = [
                    Self::csv_field(&media_info.artist),
                    Self::csv_field(&media_info.title),
                    Cow::Owned(media_info.is_live.to_string()),
                    category_cell,
                    start_time_cell,
                    cover_cell,
                    artist_url_cell,
                    ua_cell,
                    Cow::Borrowed(""),
                    Cow::Borrowed(""),
                    Cow::Borrowed(""),
                    Cow::Borrowed(""),
                    Cow::Borrowed(""),
                    Cow::Borrowed(""),
                    Cow::Borrowed(""),
                    Cow::Borrowed(""),
                    Cow::Borrowed(""),
                ];
                output.push_str(&row.join(","));
                output.push('\n');
            } else {
                for stream in &media_info.streams {
                    let row = [
                        Self::csv_field(&media_info.artist),
                        Self::csv_field(&media_info.title),
                        Cow::Owned(media_info.is_live.to_string()),
                        category_cell.clone(),
                        start_time_cell.clone(),
                        cover_cell.clone(),
                        artist_url_cell.clone(),
                        ua_cell.clone(),
                        Self::csv_field(stream.stream_format.as_str()),
                        Self::csv_field(&stream.quality),
                        Self::csv_field(stream.url.as_str()),
                        Cow::Owned(stream.bitrate.to_string()),
                        Self::csv_field(stream.media_format.as_str()),
                        Self::csv_field(&stream.codec),
                        Cow::Owned(stream.fps.to_string()),
                        Cow::Owned(stream.priority.to_string()),
                        Cow::Owned(stream.is_audio_only.to_string()),
                    ];
                    output.push_str(&row.join(","));
                    output.push('\n');
                }
            }
            Ok(output)
        }
    }

    pub(crate) fn csv_field(s: &str) -> Cow<'_, str> {
        if s.contains([',', '"', '\r', '\n']) {
            let mut quoted = String::with_capacity(s.len() + 2);
            quoted.push('"');
            for ch in s.chars() {
                if ch == '"' {
                    quoted.push_str("\"\"");
                } else {
                    quoted.push(ch);
                }
            }
            quoted.push('"');
            Cow::Owned(quoted)
        } else {
            Cow::Borrowed(s)
        }
    }

    fn display_str_entry<'a>(key: &str, value: &'a str) -> &'a str {
        if is_sensitive_key(key) {
            "[redacted]"
        } else {
            value
        }
    }

    fn display_json_extra(key: &str, value: &serde_json::Value) -> String {
        if is_sensitive_key(key) {
            "[redacted]".to_string()
        } else {
            value.to_string()
        }
    }

    fn colorize(&self, text: &str, color: &Color, bold: bool) -> String {
        #[cfg(feature = "colored-output")]
        {
            if self.colored {
                let colored_text = match color {
                    Color::Green => text.green(),
                    Color::Yellow => text.yellow(),
                    Color::Blue => text.blue(),
                    Color::Cyan => text.cyan(),
                };
                if bold {
                    colored_text.bold().to_string()
                } else {
                    colored_text.to_string()
                }
            } else {
                text.to_string()
            }
        }

        #[cfg(not(feature = "colored-output"))]
        {
            let _ = (self.colored, color, bold);
            text.to_string()
        }
    }
}

#[cfg(feature = "colored-output")]
enum Color {
    Green,
    Yellow,
    Blue,
    Cyan,
}

#[cfg(not(feature = "colored-output"))]
enum Color {
    Green,
    Yellow,
    Blue,
    Cyan,
}

pub fn write_output(content: &str, output_file: Option<&std::path::Path>) -> Result<()> {
    match output_file {
        Some(path) => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(path, content)?;
        }
        None => {
            print!("{content}");
            std::io::stdout().flush()?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use platforms_parser::media::{MediaFormat, MediaInfo, StreamFormat, StreamInfo};
    use serde_json::json;

    use super::OutputManager;
    use crate::cli::OutputFormat;

    #[test]
    fn csv_output_quotes_commas_quotes_and_newlines_per_rfc4180() {
        let mgr = OutputManager::new(false);
        let stream = StreamInfo::builder(
            "https://cdn.example.com/live.flv?a=1&b=2",
            StreamFormat::Flv,
            MediaFormat::Flv,
        )
        .quality("1080p, 60fps")
        .bitrate(6_000)
        .codec("h264")
        .fps(60.0)
        .extras(json!({ "note": "x,y\"z\nw" }))
        .build();

        let media = MediaInfo::builder(
            "https://example.com/room",
            "Say \"Hello\"\r\nWorld",
            "Alice, Bob",
        )
        .is_live(true)
        .category_opt(Some(vec!["Gaming".to_string(), "Chatting".to_string()]))
        .streams(vec![stream.clone()])
        .build();

        let multi_csv = mgr
            .format_media_info(&media, None, &OutputFormat::Csv)
            .unwrap();
        assert!(multi_csv.contains("\"Alice, Bob\""));
        assert!(multi_csv.contains("\"Say \"\"Hello\"\"\r\nWorld\""));
        assert!(multi_csv.contains("\"Gaming, Chatting\""));
        assert!(multi_csv.contains("\"1080p, 60fps\""));

        let single_csv = mgr
            .format_media_info(&media, Some(&stream), &OutputFormat::Csv)
            .unwrap();
        assert!(single_csv.contains("artist,\"Alice, Bob\"\n"));
        assert!(single_csv.contains("title,\"Say \"\"Hello\"\"\r\nWorld\"\n"));
        assert!(single_csv.contains("stream_format,flv\n"));

        let stream_csv = mgr.format_stream_info(&stream, &OutputFormat::Csv).unwrap();
        assert!(stream_csv.contains("\"1080p, 60fps\""));
        assert!(stream_csv.contains("\"x,y\"\"z\nw\""));
    }

    #[test]
    fn display_and_csv_outputs_redact_secrets_while_json_keeps_resolve_payload() {
        let mgr = OutputManager::new(false);
        let stream = StreamInfo::builder(
            "https://cdn.example.com/live.m3u8",
            StreamFormat::Hls,
            MediaFormat::Ts,
        )
        .quality("Origin")
        .bitrate(8_000)
        .extras(json!({ "pwd": "secret_stream_pwd", "cdn": "main" }))
        .build();

        let mut media = MediaInfo::builder("https://example.com", "Title", "Artist")
            .is_live(true)
            .streams(vec![stream.clone()])
            .build();
        let headers = media.headers.insert(Default::default());
        headers.insert("Cookie".to_string(), "SESS=secret_cookie".to_string());
        headers.insert("Authorization".to_string(), "OAuth secret_auth".to_string());
        headers.insert("User-Agent".to_string(), "Mozilla/5.0".to_string());

        let media_extras = media.extras.insert(Default::default());
        media_extras.insert(
            "session_cookies".to_string(),
            "AUTH=secret_sess".to_string(),
        );
        media_extras.insert("stream_password".to_string(), "secret_room_pw".to_string());
        media_extras.insert("room_id".to_string(), "12345".to_string());

        for rendered in [
            mgr.format_media_info(&media, Some(&stream), &OutputFormat::Pretty)
                .unwrap(),
            mgr.format_media_info(&media, Some(&stream), &OutputFormat::Table)
                .unwrap(),
            mgr.format_stream_info(&stream, &OutputFormat::Pretty)
                .unwrap(),
            mgr.format_stream_info(&stream, &OutputFormat::Table)
                .unwrap(),
            mgr.format_stream_info(&stream, &OutputFormat::Csv).unwrap(),
        ] {
            for secret in [
                "secret_cookie",
                "secret_auth",
                "secret_sess",
                "secret_room_pw",
                "secret_stream_pwd",
            ] {
                assert!(
                    !rendered.contains(secret),
                    "leaked {secret} in:\n{rendered}"
                );
            }
            assert!(rendered.contains("[redacted]"));
        }

        // JSON output feeds `strev resolve`, so `pwd` must remain round-trippable there.
        let stream_json = mgr
            .format_stream_info(&stream, &OutputFormat::JsonCompact)
            .unwrap();
        assert!(stream_json.contains("secret_stream_pwd"));
    }
}
