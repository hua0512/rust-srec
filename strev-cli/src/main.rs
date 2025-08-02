mod cli;
mod commands;
mod config;
mod error;
mod output;

use crate::{
    cli::{Args, Commands},
    commands::CommandExecutor,
    config::AppConfig,
    error::Result,
};
use clap::Parser;
#[cfg(feature = "colored-output")]
use colored::*;
use std::{
    io::{self, Read},
    process,
};
use tracing::{Level, error, info};
use tracing_subscriber::{filter::EnvFilter, fmt, prelude::*};

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let result = run(args).await;

    if let Err(e) = result {
        let output_format = match &Args::parse().command {
            Commands::Extract { output, .. } => Some(*output),
            Commands::Batch { output_format, .. } => Some(*output_format),
            Commands::Resolve { output, .. } => Some(*output),
            _ => None,
        };

        match output_format {
            Some(crate::cli::OutputFormat::Json) | Some(crate::cli::OutputFormat::JsonCompact) => {
                let error_json = serde_json::json!({
                    "status": "error",
                    "message": e.to_string(),
                });
                println!("{}", serde_json::to_string(&error_json).unwrap());
            }
            _ => {
                error!("Application error: {}", e);
                #[cfg(feature = "colored-output")]
                {
                    eprintln!("{} {}", "Error:".red().bold(), e);
                }
                #[cfg(not(feature = "colored-output"))]
                {
                    eprintln!("Error: {}", e);
                }
            }
        }
        process::exit(1);
    }
}

#[allow(clippy::println_empty_string)]
async fn run(args: Args) -> Result<()> {
    let output_format = match &args.command {
        Commands::Extract { output, .. } => Some(*output),
        Commands::Batch { output_format, .. } => Some(*output_format),
        Commands::Resolve { output, .. } => Some(*output),
        _ => None,
    };

    let show_banner = matches!(output_format, Some(crate::cli::OutputFormat::Pretty));

    if show_banner {
        println!("==================================================================");
        println!("███████╗████████╗██████╗ ███████╗██╗   ██╗");
        println!("██╔════╝╚══██╔══╝██╔══██╗██╔════╝██║   ██║");
        println!("███████╗   ██║   ██████╔╝█████╗  ██║   ██║");
        println!("╚════██║   ██║   ██╔══██╗██╔══╝  ╚██╗ ██╔╝");
        println!("███████║   ██║   ██║  ██║███████╗ ╚████╔╝ ");
        println!("╚══════╝   ╚═╝   ╚═╝  ╚═╝╚══════╝  ╚═══╝  ");
        println!("");
        println!(
            "Streev - CLI tool for streaming media extraction and retrieval from various platforms"
        );
        println!("GitHub: https://github.com/hua0512/rust-srec");
        println!("==================================================================");
        println!("");
    } else {
        init_logging(args.verbose, args.quiet)?;
    }

    // Load configuration
    let config = AppConfig::load(args.config.as_deref())?;

    if show_banner {
        info!("Starting platforms-cli with config: {:?}", config);
    }

    // Create command executor with proxy support
    let executor =
        if args.proxy.is_some() || args.proxy_username.is_some() || args.proxy_password.is_some() {
            CommandExecutor::new_with_proxy(
                config,
                args.proxy,
                args.proxy_username,
                args.proxy_password,
            )
        } else {
            CommandExecutor::new(config)
        };

    // Execute command
    match args.command {
        Commands::Extract {
            url,
            cookies,
            extras,
            output,
            output_file,
            quality,
            format,
            auto_select,
            no_extras,
        } => {
            executor
                .extract_single(
                    &url,
                    cookies.as_deref(),
                    extras.as_deref(),
                    output_file.as_deref(),
                    quality.as_deref(),
                    format.as_deref(),
                    auto_select,
                    !no_extras, // Include extras by default, exclude only if --no-extras is specified
                    output,
                    std::time::Duration::from_secs(args.timeout),
                    args.retries,
                )
                .await?;
        }

        Commands::Batch {
            input,
            output_dir,
            output_format,
            max_concurrent,
            continue_on_error: _,
        } => {
            executor
                .batch_process(
                    &input,
                    output_dir.as_deref(),
                    max_concurrent,
                    None, // quality filter
                    None, // format filter
                    true, // auto_select
                    output_format,
                    std::time::Duration::from_secs(args.timeout),
                    args.retries,
                )
                .await?;
        }

        Commands::Platforms { detailed: _ } => {
            executor
                .list_platforms(&crate::cli::OutputFormat::Pretty)
                .await?;
        }

        Commands::Completions { shell } => {
            use clap::CommandFactory;
            use clap_complete::generate;

            let mut cmd = Args::command();
            let bin_name = cmd.get_name().to_string();
            generate(shell, &mut cmd, bin_name, &mut std::io::stdout());
        }

        Commands::Config { show, reset } => {
            if reset {
                AppConfig::reset(args.config.as_deref())?;
                println!("✓ Configuration reset to defaults");
            } else if show {
                let config = AppConfig::load(args.config.as_deref())?;
                println!("{}", config.show()?);
            } else {
                println!(
                    "Use --show to display current configuration or --reset to reset to defaults"
                );
            }
        }
        Commands::Resolve {
            url,
            cookies,
            extras,
            payload,
            output,
            output_file,
            no_extras,
        } => {
            let payload_str = if let Some(p) = payload {
                p
            } else {
                let mut buffer = String::new();
                io::stdin().read_to_string(&mut buffer)?;
                buffer
            };
            executor
                .resolve_stream(
                    &url,
                    cookies.as_deref(),
                    extras.as_deref(),
                    &payload_str,
                    &output,
                    output_file.as_deref(),
                    !no_extras,
                )
                .await?;
        }
    }

    Ok(())
}

fn init_logging(verbose: bool, quiet: bool) -> Result<()> {
    let filter = if quiet {
        EnvFilter::new("error")
    } else if verbose {
        EnvFilter::new("debug")
    } else {
        EnvFilter::from_default_env().add_directive(Level::INFO.into())
    };

    let subscriber = tracing_subscriber::registry().with(filter);

    subscriber
        .with(fmt::layer().with_target(false).with_level(verbose))
        .init();
    Ok(())
}
