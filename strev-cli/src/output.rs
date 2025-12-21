use crate::{cli::OutputFormat, error::Result};
#[cfg(feature = "colored-output")]
use colored::*;
use platforms_parser::media::{MediaInfo, StreamInfo};
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
                    output.push_str(&format!(
                        "    {}: {}\n",
                        self.colorize(key, &Color::Green, false),
                        self.colorize(&value.to_string(), &Color::Cyan, false)
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

        if let Some(extras) = &stream_info.extras
            && let Some(extras_obj) = extras.as_object()
        {
            for (key, value) in extras_obj {
                rows.push(StreamTableRow {
                    property: key,
                    value: Cow::Owned(value.to_string()),
                });
            }
        }

        let table = Table::new(rows).with(Style::modern()).to_string();
        Ok(table)
    }

    fn format_stream_csv(&self, stream_info: &StreamInfo) -> Result<String> {
        let mut output = String::new();
        let mut headers = vec![
            "quality",
            "stream_format",
            "media_format",
            "url",
            "bitrate",
            "codec",
            "fps",
            "priority",
        ];

        let mut extras_keys = Vec::new();
        if let Some(extras) = &stream_info.extras
            && let Some(extras_obj) = extras.as_object()
        {
            for key in extras_obj.keys() {
                headers.push(key);
                extras_keys.push(key.as_str());
            }
        }
        output.push_str(&headers.join(","));
        output.push('\n');

        let stream_format_str = stream_info.stream_format.as_str();
        let media_format_str = stream_info.media_format.as_str();

        let mut record = vec![
            Self::escape_csv(&stream_info.quality),
            Self::escape_csv(stream_format_str),
            Self::escape_csv(media_format_str),
            Self::escape_csv(&stream_info.url),
            Cow::Owned(stream_info.bitrate.to_string()),
            Self::escape_csv(&stream_info.codec),
            Cow::Owned(stream_info.fps.to_string()),
            Cow::Owned(stream_info.priority.to_string()),
        ];

        if let Some(extras) = &stream_info.extras
            && let Some(extras_obj) = extras.as_object()
        {
            for key in extras_keys {
                let value = extras_obj.get(key).and_then(|v| v.as_str()).unwrap_or("");
                record.push(Self::escape_csv(value));
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
                    output.push_str(&format!(
                        "    {}: {}\n",
                        self.colorize(key, &Color::Green, false),
                        self.colorize(&value.to_string(), &Color::Cyan, false)
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
                output.push_str(&format!(
                    "  {}: {}\n",
                    self.colorize(key, &Color::Yellow, false),
                    self.colorize(value, &Color::Cyan, false)
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
                output.push_str(&format!(
                    "  {}: {}\n",
                    self.colorize(key, &Color::Yellow, false),
                    self.colorize(value, &Color::Cyan, false)
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

        if let Some(headers) = &media_info.headers {
            for (key, value) in headers {
                rows.push(TableRow {
                    property: key,
                    value: Cow::Borrowed(value),
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
        }

        let table = Table::new(rows).with(Style::modern()).to_string();
        Ok(table)
    }

    fn format_csv(
        &self,
        media_info: &MediaInfo,
        stream_info: Option<&StreamInfo>,
    ) -> Result<String> {
        if let Some(stream) = stream_info {
            let mut output = String::new();
            output.push_str("property,value\n");

            output.push_str(&format!(
                "artist,\"{}\"\n",
                Self::escape_csv(&media_info.artist)
            ));
            output.push_str(&format!(
                "title,\"{}\"\n",
                Self::escape_csv(&media_info.title)
            ));
            output.push_str(&format!("is_live,{}\n", media_info.is_live));

            if let Some(cover_url) = &media_info.cover_url {
                output.push_str(&format!("cover_url,\"{}\"\n", Self::escape_csv(cover_url)));
            }

            if let Some(artist_url) = &media_info.artist_url {
                output.push_str(&format!(
                    "artist_url,\"{}\"\n",
                    Self::escape_csv(artist_url)
                ));
            }

            if let Some(headers) = &media_info.headers {
                if let Some(ua) = headers.get("User-Agent") {
                    output.push_str(&format!("user_agent,\"{}\"\n", Self::escape_csv(ua)));
                } else {
                    output.push_str("user_agent,\"\"\n");
                }
            } else {
                output.push_str("user_agent,\"\"\n");
            }

            output.push_str(&format!("stream_format,\"{}\"\n", stream.stream_format));
            output.push_str(&format!(
                "quality,\"{}\"\n",
                Self::escape_csv(&stream.quality)
            ));
            output.push_str(&format!(
                "url,\"{}\"\n",
                Self::escape_csv(stream.url.as_str())
            ));
            output.push_str(&format!("bitrate,{}\n", stream.bitrate));
            output.push_str(&format!(
                "media_format,\"{}\"\n",
                stream.media_format.as_str()
            ));
            output.push_str(&format!("codec,\"{}\"\n", Self::escape_csv(&stream.codec)));
            output.push_str(&format!("fps,{}\n", stream.fps));
            output.push_str(&format!("priority,{}\n", stream.priority));

            Ok(output)
        } else {
            let mut output = String::new();
            let headers = [
                "artist",
                "title",
                "is_live",
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
            ];
            output.push_str(&headers.join(","));
            output.push('\n');

            if media_info.streams.is_empty() {
                let row: [Cow<str>; 14] = [
                    Self::escape_csv(&media_info.artist),
                    Self::escape_csv(&media_info.title),
                    Cow::Owned(media_info.is_live.to_string()),
                    Self::escape_csv(media_info.cover_url.as_deref().unwrap_or("")),
                    Self::escape_csv(media_info.artist_url.as_deref().unwrap_or("")),
                    Cow::Borrowed(
                        media_info
                            .headers
                            .as_ref()
                            .and_then(|h| h.get("User-Agent").map(|s| s.as_str()))
                            .unwrap_or(""),
                    ),
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
                    let stream_format_str = stream.stream_format.as_str();
                    let media_format_str = stream.media_format.as_str();
                    let row = [
                        Self::escape_csv(&media_info.artist),
                        Self::escape_csv(&media_info.title),
                        Cow::Owned(media_info.is_live.to_string()),
                        Self::escape_csv(media_info.cover_url.as_deref().unwrap_or("")),
                        Self::escape_csv(media_info.artist_url.as_deref().unwrap_or("")),
                        Cow::Borrowed(
                            media_info
                                .headers
                                .as_ref()
                                .and_then(|h| h.get("User-Agent").map(|s| s.as_str()))
                                .unwrap_or(""),
                        ),
                        Self::escape_csv(stream_format_str),
                        Self::escape_csv(&stream.quality),
                        Self::escape_csv(stream.url.as_str()),
                        Cow::Owned(stream.bitrate.to_string()),
                        Self::escape_csv(media_format_str),
                        Self::escape_csv(&stream.codec),
                        Cow::Owned(stream.fps.to_string()),
                        Cow::Owned(stream.priority.to_string()),
                    ];
                    output.push_str(&row.join(","));
                    output.push('\n');
                }
            }
            Ok(output)
        }
    }

    // Helper method to avoid unnecessary allocations when escaping CSV
    fn escape_csv(s: &str) -> Cow<'_, str> {
        if s.contains('"') {
            Cow::Owned(s.replace('"', "\"\""))
        } else {
            Cow::Borrowed(s)
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
