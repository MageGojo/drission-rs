mod backend;
mod cli;
mod daemon;
mod engine;
mod mcp;
mod ocr_cmd;
mod paths;
mod protocol;

use anyhow::Result;
use clap::Parser;
use serde_json::Value;

use crate::cli::{Cli, Command};
use crate::protocol::JsonResponse;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let json_mode = cli.json;

    match cli.command {
        Command::Serve(args) => {
            engine::validate_backend_or_bail(args.backend)?;
            daemon::run_server(args.backend, args.headless, args.user_data_dir).await?;
            Ok(())
        }
        Command::Mcp(args) => {
            init_tracing();
            engine::validate_backend_or_bail(args.backend)?;
            mcp::run_mcp(args.backend, args.headless, args.user_data_dir).await?;
            Ok(())
        }
        #[cfg(feature = "ocr")]
        Command::Ocr { command } => {
            let response = match command {
                cli::OcrCommand::Clickword { image, targets } => {
                    ocr_cmd::clickword(&image, &targets).await?
                }
            };
            print_response(response, json_mode)?;
            Ok(())
        }
        other => {
            let command = other
                .into_engine()
                .expect("non-local command must map to EngineCommand");
            let response = daemon::send_to_daemon(command).await?;
            let ok = response.ok;
            print_response(response, json_mode)?;
            if !ok {
                std::process::exit(1);
            }
            Ok(())
        }
    }
}

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .try_init();
}

fn print_response(response: JsonResponse, json_mode: bool) -> Result<()> {
    if json_mode {
        println!("{}", serde_json::to_string_pretty(&response)?);
        return Ok(());
    }

    if response.ok {
        let data = response.data.unwrap_or(Value::Null);
        if let Some(s) = data.get("text").and_then(Value::as_str) {
            println!("{s}");
        } else if let Some(s) = data.get("html").and_then(Value::as_str) {
            println!("{s}");
        } else if let Some(s) = data.get("outline").and_then(Value::as_str) {
            println!("{s}");
        } else if let Some(value) = data.get("value") {
            println!("{}", serde_json::to_string_pretty(value)?);
        } else {
            println!("{}", serde_json::to_string_pretty(&data)?);
        }
    } else if let Some(error) = response.error {
        eprintln!("{}: {}", error.code, error.message);
        if let Some(hint) = error.hint {
            eprintln!("hint: {hint}");
        }
    }
    Ok(())
}
