use anyhow::Context;
use clap::Parser;
use colored::*;
use indicatif::{ProgressBar, ProgressStyle};
use inquire::Select;
use platforms_parser::{extractor::default_factory, media::StreamInfo};
use std::time::Duration;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// The URL of the media to parse
    #[arg(short, long)]
    url: String,

    /// The cookies to use for the request
    #[clap(long)]
    cookies: Option<String>,

    /// Output the result in JSON format
    #[clap(long)]
    json: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let url = args.url;

    let pb = ProgressBar::new_spinner();
    pb.enable_steady_tick(Duration::from_millis(120));
    pb.set_style(
        ProgressStyle::with_template("{spinner:.blue} {msg}")
            .unwrap()
            .tick_strings(&[
                "▹▹▹▹▹",
                "▸▹▹▹▹",
                "▹▸▹▹▹",
                "▹▹▸▹▹",
                "▹▹▹▸▹",
                "▹▹▹▹▸",
                "▪▪▪▪▪",
            ]),
    );
    pb.set_message("Extracting media information...");

    let cookies = args.cookies;
    let factory = default_factory();
    let extractor = factory
        .create_extractor(&url, cookies)
        .with_context(|| format!("Failed to create extractor for URL: {}", &url))?;
    let media_info = extractor
        .extract()
        .await
        .context("Failed to fetch media information")?;

    pb.finish_with_message("Done");

    // handle errors
    println!("\n{}", "Media Information:".green().bold());

    println!("{} {}", "Artist:".green(), media_info.artist.cyan());

    println!("{} {}", "Title:".green(), media_info.title.cyan());

    if let Some(cover_url) = &media_info.cover_url {
        println!("{} {}", "Cover URL:".green(), cover_url.blue());
    }
    if let Some(artist_url) = &media_info.artist_url {
        println!("{} {}", "Artist URL:".green(), artist_url.blue());
    }

    println!(
        "{} {}",
        "Live:".green(),
        media_info.is_live.to_string().cyan()
    );

    let selected_stream: StreamInfo = match media_info.streams.len() {
        0 => {
            // there are no streams
            anyhow::bail!("No streams available for this media.");
        }
        1 => {
            // there is only one stream
            media_info.streams.into_iter().next().unwrap()
        }
        _ => {
            // there are multiple streams
            println!(
                "{}",
                "Multiple streams available, please select one:"
                    .yellow()
                    .bold()
            );

            let options: Vec<String> = media_info.streams.iter().map(|s| s.to_string()).collect();
            let selection = Select::new("Select a stream:", options)
                .prompt()
                .context("Failed to select stream")?;

            media_info
                .streams
                .into_iter()
                .find(|s| s.to_string() == selection)
                .unwrap()
        }
    };

    let pb = ProgressBar::new_spinner();
    pb.enable_steady_tick(Duration::from_millis(120));
    pb.set_style(
        ProgressStyle::with_template("{spinner:.blue} {msg}")
            .unwrap()
            .tick_strings(&[
                "▹▹▹▹▹",
                "▸▹▹▹▹",
                "▹▸▹▹▹",
                "▹▹▸▹▹",
                "▹▹▹▸▹",
                "▹▹▹▹▸",
                "▪▪▪▪▪",
            ]),
    );
    pb.set_message("Fetching final stream URL...");

    let final_stream_info = extractor
        .get_url(selected_stream)
        .await
        .context("Failed to fetch final stream URL")?;

    pb.finish_with_message("Done");

    if args.json {
        let json = serde_json::to_string_pretty(&final_stream_info).unwrap();
        println!("{}", json);
    } else {
        println!("\n{}", "Selected Stream Details:".green().bold());
        println!(
            "  {}: {}",
            "Format".yellow(),
            final_stream_info.format.to_string().cyan()
        );
        println!(
            "  {}: {}",
            "Quality".yellow(),
            final_stream_info.quality.cyan()
        );
        println!(
            "  {}: {}",
            "URL".yellow(),
            final_stream_info.url.as_str().blue()
        );
        println!(
            "  {}: {} kbps",
            "Bitrate".yellow(),
            final_stream_info.bitrate.to_string().cyan()
        );
        println!("  {}: {}", "Codec".yellow(), final_stream_info.codec.cyan());
        println!(
            "  {}: {}",
            "Priority".yellow(),
            final_stream_info.priority.to_string().cyan()
        );
        println!(
            "  {}: {}",
            "FPS".yellow(),
            final_stream_info.fps.to_string().cyan()
        );
        // println!(
        //     "  {}: {}",
        //     "Headers Needed".yellow(),
        //     final_stream_info.is_headers_needed.to_string().cyan()
        // );

        if let Some(extras) = &final_stream_info.extras {
            if let Some(extras_obj) = extras.as_object().filter(|m| !m.is_empty()) {
                println!("  {}:", "Extras".yellow());
                for (key, value) in extras_obj {
                    println!("    {}: {}", key.green(), value.to_string().cyan());
                }
            }
        }
        if let Some(extras) = &media_info.extras {
            if !extras.is_empty() {
                println!("\n{}", "Media Extras:".green().bold());
                for (key, value) in extras {
                    println!("  {}: {}", key.yellow(), value.cyan());
                }
            }
        }
    }

    Ok(())
}
