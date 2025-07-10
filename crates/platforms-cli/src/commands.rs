use crate::{
    cli::OutputFormat,
    config::AppConfig,
    error::{CliError, Result},
    output::{OutputManager, write_output},
};
use colored::*;
use indicatif::{ProgressBar, ProgressStyle};
use platforms_parser::{
    extractor::factory::ExtractorFactory,
    media::{MediaInfo, StreamInfo},
};
use regex::Regex;
use std::{path::Path, sync::Arc, time::Duration};
use tokio::{
    sync::Semaphore,
    time::{sleep, timeout},
};

// Type alias for complex type to satisfy clippy
type BatchResult = Result<(MediaInfo, StreamInfo)>;
type BatchResultTuple = (usize, String, BatchResult);

pub struct CommandExecutor {
    config: AppConfig,
    extractor_factory: ExtractorFactory,
}

impl CommandExecutor {
    pub fn new(config: AppConfig) -> Self {
        let client = reqwest::Client::new();
        let extractor_factory = ExtractorFactory::new(client);
        Self {
            config,
            extractor_factory,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn extract_single(
        &self,
        url: &str,
        cookies: Option<&str>,
        extras: Option<&str>,
        output_file: Option<&Path>,
        quality: Option<&str>,
        format: Option<&str>,
        auto_select: bool,
        include_extras: bool,
        output_format: OutputFormat,
        timeout_duration: Duration,
        retries: u32,
    ) -> Result<()> {
        let pb = self.create_progress_bar("Extracting...");

        let result = self
            .extract_with_retry(url, cookies, extras, timeout_duration, retries)
            .await;
        pb.finish_and_clear();

        match result {
            Ok(media_info) => {
                let selected_stream = if media_info.streams.is_empty() {
                    return Err(CliError::no_streams_found());
                } else if auto_select {
                    self.auto_select_stream(&media_info.streams)?
                } else if media_info.streams.len() == 1 {
                    media_info.streams[0].clone()
                } else {
                    self.interactive_select_stream(media_info.streams.clone())?
                };

                let filtered_stream = self.apply_filters(selected_stream, quality, format)?;

                let output_manager = OutputManager::new(self.config.colored_output);
                let output = output_manager.format_media_info(
                    &media_info,
                    Some(&filtered_stream),
                    &output_format,
                    include_extras,
                )?;

                write_output(&output, output_file)?;
                Ok(())
            }
            Err(e) => {
                eprintln!("{}", e.to_string().red());
                Err(e)
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn batch_process(
        &self,
        input_file: &Path,
        output_dir: Option<&Path>,
        concurrency: usize,
        _quality: Option<&str>,
        _format: Option<&str>,
        auto_select: bool,
        output_format: OutputFormat,
        timeout_duration: Duration,
        _retries: u32,
    ) -> Result<()> {
        let content = std::fs::read_to_string(input_file)?;
        let urls: Vec<String> = content
            .lines()
            .filter_map(|line| {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    None
                } else {
                    Some(line.to_string())
                }
            })
            .collect();

        if urls.is_empty() {
            return Err(CliError::invalid_input("No valid URLs found in input file"));
        }

        let pb = Arc::new(ProgressBar::new(urls.len() as u64));
        pb.set_style(
            ProgressStyle::default_bar()
                .template("[{elapsed_precise}] {bar:40.cyan/blue} {pos}/{len} {msg}")
                .unwrap()
                .progress_chars("█▉▊▋▌▍▎▏  "),
        );

        let semaphore = Arc::new(Semaphore::new(concurrency));
        let mut tasks = Vec::new();

        for (index, url) in urls.iter().enumerate() {
            let url = url.clone();
            let pb = Arc::clone(&pb);
            let permit = Arc::clone(&semaphore).acquire_owned().await?;
            let client = reqwest::Client::new();

            let task = tokio::spawn(async move {
                let _permit = permit;

                pb.set_message(format!("Processing: {url}"));

                let result = match timeout(timeout_duration, async {
                    let factory = ExtractorFactory::new(client);
                    let extractor = factory.create_extractor(&url, None, None)?;
                    // Extract media info directly using the platforms API
                    extractor.extract().await
                })
                .await
                {
                    Ok(Ok(media_info)) => {
                        if media_info.streams.is_empty() {
                            Err(CliError::no_streams_found())
                        } else {
                            let stream = if auto_select {
                                media_info
                                    .streams
                                    .iter()
                                    .max_by_key(|s| s.priority)
                                    .cloned()
                                    .unwrap_or_else(|| media_info.streams.first().cloned().unwrap())
                            } else {
                                media_info.streams.first().cloned().unwrap()
                            };
                            Ok((media_info, stream))
                        }
                    }
                    Ok(Err(e)) => Err(e.into()),
                    Err(_) => Err(CliError::timeout()),
                };

                pb.inc(1);
                (index, url, result)
            });

            tasks.push(task);
        }

        let mut results: Vec<BatchResultTuple> = Vec::new();
        for task in tasks {
            match task.await {
                Ok(result) => results.push(result),
                Err(e) => return Err(CliError::Extraction(e.to_string())),
            }
        }

        pb.finish_with_message("Batch processing completed");

        match &output_format {
            OutputFormat::Json | OutputFormat::JsonCompact => {
                self.output_batch_json(&results, output_dir, &output_format)
                    .await?;
            }
            _ => {
                self.output_batch_summary(&results, output_dir).await?;
            }
        }

        Ok(())
    }

    pub async fn list_platforms(&self, output_format: &OutputFormat) -> Result<()> {
        let platforms = vec![
            ("Bilibili", "live.bilibili.com/{room_id}"),
            ("Douyin", "live.douyin.com/{room_id}"),
            ("Douyu", "douyu.com/{room_id}"),
            ("Huya", "huya.com/{room_id}"),
            ("Twitch", "twitch.tv/{channel_name}"),
            ("TikTok", "tiktok.com/@{username}/live"),
            ("Twitcasting", "twitcasting.tv/{username}"),
            ("Picarto", "picarto.tv/{channel_name}"),
            ("PandaTV", "pandalive.co.kr/play/{user_id} (Defunct)"),
            (
                "Weibo",
                "weibo.com/u/{user_id}, weibo.com/l/wblive/p/show/{live_id}",
            ),
            (
                "Redbook",
                "xiaohongshu.com/user/profile/{user_id}, xhslink.com/{share_id}",
            ),
        ];

        match output_format {
            OutputFormat::Json | OutputFormat::JsonCompact => {
                let platforms_json: Vec<serde_json::Value> = platforms
                    .iter()
                    .map(|(name, pattern)| {
                        serde_json::json!({
                            "name": name,
                            "url_pattern": pattern
                        })
                    })
                    .collect();

                let output = if matches!(output_format, OutputFormat::Json) {
                    serde_json::to_string_pretty(&platforms_json)?
                } else {
                    serde_json::to_string(&platforms_json)?
                };

                println!("{output}");
            }
            _ => {
                let title = if self.config.colored_output {
                    "Supported Platforms:".green().bold().to_string()
                } else {
                    "Supported Platforms:".to_string()
                };

                println!("{title}");

                for (name, pattern) in platforms {
                    if self.config.colored_output {
                        println!("  {} - {}", name.cyan().bold(), pattern.blue());
                    } else {
                        println!("  {name} - {pattern}");
                    }
                }
            }
        }

        Ok(())
    }

    fn create_progress_bar(&self, message: &str) -> ProgressBar {
        let pb = ProgressBar::new_spinner();
        pb.set_style(
            ProgressStyle::default_spinner()
                .template("{spinner:.cyan} {msg}")
                .unwrap()
                .tick_chars("⠁⠂⠄⡀⢀⠠⠐⠈ "),
        );
        pb.set_message(message.to_string());
        pb
    }

    async fn extract_with_retry(
        &self,
        url: &str,
        cookies: Option<&str>,
        extras: Option<&str>,
        timeout_duration: Duration,
        retries: u32,
    ) -> Result<MediaInfo> {
        let mut last_error = None;

        for attempt in 0..=retries {
            match timeout(timeout_duration, async {
                let extras_json = extras.and_then(|s| serde_json::from_str(s).ok());
                let extractor = self.extractor_factory.create_extractor(
                    url,
                    cookies.map(String::from),
                    extras_json,
                )?;
                extractor.extract().await
            })
            .await
            {
                Ok(Ok(result)) => return Ok(result),
                Ok(Err(e)) => {
                    last_error = Some(e.into());
                    if attempt < retries {
                        let delay = Duration::from_millis(1000 * (1 << attempt));
                        sleep(delay).await;
                    }
                }
                Err(_) => {
                    last_error = Some(CliError::timeout());
                    if attempt < retries {
                        let delay = Duration::from_millis(1000 * (1 << attempt));
                        sleep(delay).await;
                    }
                }
            }
        }

        Err(last_error.unwrap_or_else(CliError::timeout))
    }

    fn auto_select_stream(&self, streams: &[StreamInfo]) -> Result<StreamInfo> {
        streams
            .iter()
            .max_by_key(|s| s.priority)
            .cloned()
            .ok_or_else(CliError::no_streams_found)
    }

    fn interactive_select_stream(&self, streams: Vec<StreamInfo>) -> Result<StreamInfo> {
        if streams.is_empty() {
            return Err(CliError::no_streams_found());
        }

        let options: Vec<String> = streams
            .iter()
            .enumerate()
            .map(|(i, stream)| {
                format!(
                    "{}: {} - {} ({})",
                    i + 1,
                    stream.quality,
                    stream.stream_format,
                    stream.codec
                )
            })
            .collect();

        let selection = inquire::Select::new("Select a stream:", options)
            .prompt()
            .map_err(|_| CliError::user_cancelled())?;

        let index = selection
            .split(':')
            .next()
            .and_then(|s| s.parse::<usize>().ok())
            .and_then(|i| i.checked_sub(1))
            .ok_or_else(|| CliError::invalid_input("Invalid selection"))?;

        streams
            .into_iter()
            .nth(index)
            .ok_or_else(|| CliError::invalid_input("Invalid stream index"))
    }

    fn apply_filters(
        &self,
        stream: StreamInfo,
        quality: Option<&str>,
        format: Option<&str>,
    ) -> Result<StreamInfo> {
        // Quality filter
        if let Some(quality_filter) = quality {
            let quality_regex = Regex::new(quality_filter)
                .map_err(|e| CliError::invalid_filter(format!("Invalid quality regex: {e}")))?;

            if !quality_regex.is_match(&stream.quality) {
                return Err(CliError::no_matching_stream());
            }
        }

        // Format filter
        if let Some(format_filter) = format {
            let format_regex = Regex::new(format_filter)
                .map_err(|e| CliError::invalid_filter(format!("Invalid format regex: {e}")))?;

            if !format_regex.is_match(&stream.stream_format.to_string()) {
                return Err(CliError::no_matching_stream());
            }
        }

        Ok(stream)
    }

    async fn output_batch_json(
        &self,
        results: &[BatchResultTuple],
        output_dir: Option<&Path>,
        output_format: &OutputFormat,
    ) -> Result<()> {
        let json_results: Vec<serde_json::Value> = results
            .iter()
            .map(|(index, url, result)| match result {
                Ok((media_info, stream_info)) => {
                    serde_json::json!({
                        "index": index,
                        "url": url,
                        "status": "success",
                        "media_info": media_info,
                        "stream_info": stream_info
                    })
                }
                Err(e) => {
                    serde_json::json!({
                        "index": index,
                        "url": url,
                        "status": "error",
                        "error": e.to_string()
                    })
                }
            })
            .collect();

        let output = match output_format {
            OutputFormat::Json => serde_json::to_string_pretty(&json_results)?,
            _ => serde_json::to_string(&json_results)?,
        };

        let output_file = output_dir.map(|dir| dir.join("batch_results.json"));
        write_output(&output, output_file.as_deref())?;
        Ok(())
    }

    async fn output_batch_summary(
        &self,
        results: &[BatchResultTuple],
        output_dir: Option<&Path>,
    ) -> Result<()> {
        let mut summary = String::new();

        summary.push_str("=== Batch Processing Summary ===\n\n");

        let successful = results.iter().filter(|(_, _, r)| r.is_ok()).count();
        let failed = results.len() - successful;

        summary.push_str(&format!("Total URLs: {}\n", results.len()));
        summary.push_str(&format!("Successful: {successful}\n"));
        summary.push_str(&format!("Failed: {failed}\n\n"));

        for (index, url, result) in results {
            let status_line = match result {
                Ok(_) => format!("[{}] ✓ SUCCESS: {}", index + 1, url),
                Err(e) => format!("ERROR for URL {url}: {e}"),
            };
            summary.push_str(&status_line);
            summary.push('\n');
        }

        let output_file = output_dir.map(|dir| dir.join("batch_summary.txt"));
        write_output(&summary, output_file.as_deref())?;
        Ok(())
    }
}
