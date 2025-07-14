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
        include_extras: bool,
    ) -> Result<String> {
        match format {
            OutputFormat::Pretty => self.format_pretty(media_info, stream_info, include_extras),
            OutputFormat::Json => self.format_json(media_info, stream_info, include_extras, true),
            OutputFormat::JsonCompact => {
                self.format_json(media_info, stream_info, include_extras, false)
            }
            #[cfg(feature = "table-output")]
            OutputFormat::Table => self.format_table(media_info, stream_info),
            #[cfg(not(feature = "table-output"))]
            OutputFormat::Table => {
                // Fallback to pretty format when table feature is disabled
                self.format_pretty(media_info, stream_info, include_extras)
            }
            OutputFormat::Csv => self.format_csv(media_info, stream_info),
        }
    }

    fn format_pretty(
        &self,
        media_info: &MediaInfo,
        stream_info: Option<&StreamInfo>,
        include_extras: bool,
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
                self.colorize(&stream.stream_format.to_string(), &Color::Cyan, false)
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
                self.colorize(&stream.media_format.to_string(), &Color::Cyan, false)
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

            if include_extras {
                if let Some(extras) = &stream.extras {
                    if let Some(extras_obj) = extras.as_object().filter(|m| !m.is_empty()) {
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
            }
        }

        // Media Extras
        if include_extras {
            if let Some(ref extras) = media_info.extras {
                if !extras.is_empty() {
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
            }
        }

        Ok(output)
    }

    fn format_json(
        &self,
        media_info: &MediaInfo,
        stream_info: Option<&StreamInfo>,
        include_extras: bool,
        pretty: bool,
    ) -> Result<String> {
        let mut output = serde_json::json!({
            "media": {
                "artist": &media_info.artist,
                "title": &media_info.title,
                "is_live": media_info.is_live,
                "cover_url": &media_info.cover_url,
                "artist_url": &media_info.artist_url,
            }
        });

        if include_extras {
            if let Some(ref extras) = media_info.extras {
                if !extras.is_empty() {
                    output["media"]["extras"] = serde_json::to_value(extras)?;
                }
            }
        }

        if let Some(stream) = stream_info {
            let mut stream_data = serde_json::json!({
                "stream_format": stream.stream_format.to_string(),
                "quality": &stream.quality,
                "url": stream.url.as_str(),
                "bitrate": stream.bitrate,
                "media_format": stream.media_format.to_string(),
                "codec": &stream.codec,
                "fps": stream.fps,
                "priority": stream.priority,
            });

            if include_extras {
                if let Some(extras) = &stream.extras {
                    stream_data["extras"] = extras.clone();
                }
            }

            output["stream"] = stream_data;
        }

        let result = if pretty {
            serde_json::to_string_pretty(&output)?
        } else {
            serde_json::to_string(&output)?
        };

        Ok(result)
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

        if let Some(stream) = stream_info {
            rows.push(TableRow {
                property: "Stream Format",
                value: Cow::Owned(stream.stream_format.to_string()),
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
                value: Cow::Owned(stream.media_format.to_string()),
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

        if let Some(stream) = stream_info {
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
            output.push_str(&format!("media_format,\"{}\"\n", stream.media_format));
            output.push_str(&format!("codec,\"{}\"\n", Self::escape_csv(&stream.codec)));
            output.push_str(&format!("fps,{}\n", stream.fps));
            output.push_str(&format!("priority,{}\n", stream.priority));
        }

        Ok(output)
    }

    // Helper method to avoid unnecessary allocations when escaping CSV
    fn escape_csv(s: &str) -> Cow<str> {
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
