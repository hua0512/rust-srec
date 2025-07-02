use clap::Parser;
use colored::*;
use indicatif::{ProgressBar, ProgressStyle};
use inquire::Select;
use platforms_parser::{
    extractor::{default_factory, error::ExtractorError},
    media::stream_info::StreamInfo,
};
use std::time::Duration;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// The URL of the media to parse
    #[arg(short, long)]
    url: String,
    /// Output the result in JSON format
    #[clap(long)]
    json: bool,
}

#[tokio::main]
async fn main() -> Result<(), ExtractorError> {
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

    let factory = default_factory();
    let extractor = factory.create_extractor(&url)?;
    let media_info = extractor.extract().await;

    pb.finish_with_message("Done");

    if let Err(e) = media_info {
        eprintln!("{} {}", "Error extracting media information:".red(), e);
        return Err(e);
    }

    let media_info = media_info.unwrap();

    // handle errors
    println!("\n{}", "Media Information:".green().bold());

    println!("{} {}", "Artist:".green(), media_info.artist.cyan());
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

    let selected_stream: StreamInfo = if media_info.streams.len() > 1 {
        let options: Vec<String> = media_info.streams.iter().map(|s| s.to_string()).collect();
        let selection = Select::new("Select a stream:", options)
            .prompt()
            .map_err(|e| ExtractorError::Other(format!("Failed to select stream: {}", e)))?;

        media_info
            .streams
            .into_iter()
            .find(|s| s.to_string() == selection)
            .unwrap()
    } else {
        media_info.streams.into_iter().next().unwrap()
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

    let final_stream_info = extractor.get_url(selected_stream).await?;

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
        // println!(
        //     "  {}: {}",
        //     "Headers Needed".yellow(),
        //     final_stream_info.is_headers_needed.to_string().cyan()
        // );

        if let Some(extras) = &final_stream_info.extras {
            if !extras.is_empty() {
                println!("  {}:", "Extras".yellow());
                for (key, value) in extras {
                    println!("    {}: {}", key.green(), value.cyan());
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
