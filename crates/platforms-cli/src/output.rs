use crate::{cli::OutputFormat, error::Result};
use colored::*;
use platforms_parser::media::{MediaInfo, StreamInfo};
use std::io::Write;
use tabled::{settings::Style, Table, Tabled};

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
            OutputFormat::JsonCompact => self.format_json(media_info, stream_info, include_extras, false),
            OutputFormat::Table => self.format_table(media_info, stream_info),
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
        
        output.push_str(&format!("  {}: {}\n", 
            self.colorize("Artist", &Color::Yellow, false),
            self.colorize(&media_info.artist, &Color::Cyan, false)
        ));
        
        output.push_str(&format!("  {}: {}\n", 
            self.colorize("Title", &Color::Yellow, false),
            self.colorize(&media_info.title, &Color::Cyan, false)
        ));
        
        output.push_str(&format!("  {}: {}\n", 
            self.colorize("Live", &Color::Yellow, false),
            self.colorize(&media_info.is_live.to_string(), &Color::Cyan, false)
        ));

        if let Some(cover_url) = &media_info.cover_url {
            output.push_str(&format!("  {}: {}\n", 
                self.colorize("Cover URL", &Color::Yellow, false),
                self.colorize(cover_url, &Color::Blue, false)
            ));
        }

        if let Some(artist_url) = &media_info.artist_url {
            output.push_str(&format!("  {}: {}\n", 
                self.colorize("Artist URL", &Color::Yellow, false),
                self.colorize(artist_url, &Color::Blue, false)
            ));
        }

        // Stream Information
        if let Some(stream) = stream_info {
            output.push('\n');
            output.push_str(&self.colorize("Selected Stream Details:", &Color::Green, true));
            output.push('\n');
            
            output.push_str(&format!("  {}: {}\n", 
                self.colorize("Format", &Color::Yellow, false),
                self.colorize(&stream.stream_format.to_string(), &Color::Cyan, false)
            ));
            
            output.push_str(&format!("  {}: {}\n", 
                self.colorize("Quality", &Color::Yellow, false),
                self.colorize(&stream.quality, &Color::Cyan, false)
            ));
            
            output.push_str(&format!("  {}: {}\n", 
                self.colorize("URL", &Color::Yellow, false),
                self.colorize(stream.url.as_str(), &Color::Blue, false)
            ));
            
            output.push_str(&format!("  {}: {} kbps\n", 
                self.colorize("Bitrate", &Color::Yellow, false),
                self.colorize(&stream.bitrate.to_string(), &Color::Cyan, false)
            ));
            
            output.push_str(&format!("  {}: {}\n", 
                self.colorize("Media Format", &Color::Yellow, false),
                self.colorize(&stream.media_format.to_string(), &Color::Cyan, false)
            ));
            
            output.push_str(&format!("  {}: {}\n", 
                self.colorize("Codec", &Color::Yellow, false),
                self.colorize(&stream.codec, &Color::Cyan, false)
            ));
            
            output.push_str(&format!("  {}: {}\n", 
                self.colorize("FPS", &Color::Yellow, false),
                self.colorize(&stream.fps.to_string(), &Color::Cyan, false)
            ));
            
            output.push_str(&format!("  {}: {}\n", 
                self.colorize("Priority", &Color::Yellow, false),
                self.colorize(&stream.priority.to_string(), &Color::Cyan, false)
            ));

            if include_extras {
                if let Some(extras) = &stream.extras {
                    if let Some(extras_obj) = extras.as_object().filter(|m| !m.is_empty()) {
                        output.push_str(&format!("  {}:\n", self.colorize("Extras", &Color::Yellow, false)));
                        for (key, value) in extras_obj {
                            output.push_str(&format!("    {}: {}\n", 
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
                        output.push_str(&format!("  {}: {}\n", 
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
                "artist": media_info.artist,
                "title": media_info.title,
                "is_live": media_info.is_live,
                "cover_url": media_info.cover_url,
                "artist_url": media_info.artist_url,
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
                "quality": stream.quality,
                "url": stream.url.as_str(),
                "bitrate": stream.bitrate,
                "media_format": stream.media_format.to_string(),
                "codec": stream.codec,
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

    fn format_table(&self, media_info: &MediaInfo, stream_info: Option<&StreamInfo>) -> Result<String> {
        #[derive(Tabled)]
        struct TableRow {
            property: String,
            value: String,
        }

        let mut rows = vec![
            TableRow { property: "Artist".to_string(), value: media_info.artist.clone() },
            TableRow { property: "Title".to_string(), value: media_info.title.clone() },
            TableRow { property: "Live".to_string(), value: media_info.is_live.to_string() },
        ];

        if let Some(cover_url) = &media_info.cover_url {
            rows.push(TableRow { property: "Cover URL".to_string(), value: cover_url.clone() });
        }

        if let Some(artist_url) = &media_info.artist_url {
            rows.push(TableRow { property: "Artist URL".to_string(), value: artist_url.clone() });
        }

        if let Some(stream) = stream_info {
            rows.extend([
                TableRow { property: "Stream Format".to_string(), value: stream.stream_format.to_string() },
                TableRow { property: "Quality".to_string(), value: stream.quality.clone() },
                TableRow { property: "URL".to_string(), value: stream.url.to_string() },
                TableRow { property: "Bitrate".to_string(), value: format!("{} kbps", stream.bitrate) },
                TableRow { property: "Media Format".to_string(), value: stream.media_format.to_string() },
                TableRow { property: "Codec".to_string(), value: stream.codec.clone() },
                TableRow { property: "FPS".to_string(), value: stream.fps.to_string() },
                TableRow { property: "Priority".to_string(), value: stream.priority.to_string() },
            ]);
        }

        let table = Table::new(rows).with(Style::rounded()).to_string();
        Ok(table)
    }

    fn format_csv(&self, media_info: &MediaInfo, stream_info: Option<&StreamInfo>) -> Result<String> {
        let mut output = String::new();
        output.push_str("property,value\n");
        
        output.push_str(&format!("artist,\"{}\"\n", media_info.artist.replace('"', "\"\"")));
        output.push_str(&format!("title,\"{}\"\n", media_info.title.replace('"', "\"\"")));
        output.push_str(&format!("is_live,{}\n", media_info.is_live));

        if let Some(cover_url) = &media_info.cover_url {
            output.push_str(&format!("cover_url,\"{}\"\n", cover_url.replace('"', "\"\"")));
        }

        if let Some(artist_url) = &media_info.artist_url {
            output.push_str(&format!("artist_url,\"{}\"\n", artist_url.replace('"', "\"\"")));
        }

        if let Some(stream) = stream_info {
            output.push_str(&format!("stream_format,\"{}\"\n", stream.stream_format));
            output.push_str(&format!("quality,\"{}\"\n", stream.quality.replace('"', "\"\"")));
            output.push_str(&format!("url,\"{}\"\n", stream.url.as_str().replace('"', "\"\"")));
            output.push_str(&format!("bitrate,{}\n", stream.bitrate));
            output.push_str(&format!("media_format,\"{}\"\n", stream.media_format));
            output.push_str(&format!("codec,\"{}\"\n", stream.codec.replace('"', "\"\"")));
            output.push_str(&format!("fps,{}\n", stream.fps));
            output.push_str(&format!("priority,{}\n", stream.priority));
        }

        Ok(output)
    }

    fn colorize(&self, text: &str, color: &Color, bold: bool) -> String {
        if !self.colored {
            return text.to_string();
        }

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
    }
}

#[derive(Debug)]
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