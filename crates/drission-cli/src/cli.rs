use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use crate::protocol::{BackendKind, EngineCommand};

#[derive(Debug, Parser)]
#[command(name = "drs", version, about = "drission CLI and MCP runtime")]
pub struct Cli {
    /// Emit stable machine-readable JSON.
    #[arg(long)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Start the local browser daemon.
    Serve(RunBrowserArgs),
    /// Start the stdio MCP server.
    Mcp(RunBrowserArgs),
    /// Show daemon status.
    Status,
    /// Stop the daemon and browser.
    Stop,
    /// Open a URL in a new tab and make it active.
    Open { url: String },
    /// List daemon tabs.
    Tabs,
    /// Switch active tab by drs tab id.
    Use { tab_id: u64 },
    /// Close a tab, defaulting to the active tab.
    Close { tab_id: Option<u64> },
    /// Print an accessibility snapshot.
    Ax {
        /// Print outline text.
        #[arg(long, conflicts_with = "tree_json")]
        outline: bool,
        /// Return the full accessibility tree JSON.
        #[arg(long = "json", conflicts_with = "outline")]
        tree_json: bool,
    },
    /// Print current page HTML.
    Html,
    /// Print page text, or text of a selector.
    Text { selector: Option<String> },
    /// Evaluate JavaScript in the active tab.
    Eval { js: String },
    /// Save a screenshot.
    Screenshot {
        #[arg(long)]
        out: Option<PathBuf>,
        #[arg(long)]
        full: bool,
        #[arg(long)]
        inline: bool,
    },
    /// Click an element.
    Click { selector: String },
    /// Type text into an element.
    #[command(name = "type")]
    Type { selector: String, text: String },
    /// Press a key, optionally scoped to an element.
    Press {
        key: String,
        #[arg(long)]
        selector: Option<String>,
    },
    /// Wait for an element to be displayed.
    Wait {
        selector: String,
        #[arg(long)]
        timeout_ms: Option<u64>,
    },
    /// Network listener commands.
    Listen {
        #[command(subcommand)]
        command: ListenCommand,
    },
    /// Pass Cloudflare challenge in the active tab.
    PassCf {
        #[arg(long)]
        timeout_ms: Option<u64>,
    },
    /// OCR helpers.
    #[cfg(feature = "ocr")]
    Ocr {
        #[command(subcommand)]
        command: OcrCommand,
    },
}

#[derive(Debug, Args, Clone)]
pub struct RunBrowserArgs {
    /// Browser backend to run.
    #[arg(long, value_enum, default_value_t = BackendKind::Cdp)]
    pub backend: BackendKind,
    /// Run browser headless.
    #[arg(long)]
    pub headless: bool,
    /// Persistent browser profile directory.
    #[arg(long)]
    pub user_data_dir: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
pub enum ListenCommand {
    /// Start network listening.
    Start {
        keywords: Vec<String>,
        #[arg(long)]
        xhr_only: bool,
    },
    /// Wait for network packets.
    Wait {
        #[arg(long, default_value_t = 1)]
        count: usize,
        #[arg(long)]
        timeout_ms: Option<u64>,
    },
    /// Stop network listening.
    Stop,
}

#[cfg(feature = "ocr")]
#[derive(Debug, Subcommand)]
pub enum OcrCommand {
    /// Solve click-word coordinates from an image and target text.
    Clickword { image: PathBuf, targets: String },
}

impl Command {
    pub fn into_engine(self) -> Option<EngineCommand> {
        Some(match self {
            Command::Serve(_) | Command::Mcp(_) => return None,
            #[cfg(feature = "ocr")]
            Command::Ocr { .. } => return None,
            Command::Status => EngineCommand::Status,
            Command::Stop => EngineCommand::Stop,
            Command::Open { url } => EngineCommand::Open { url },
            Command::Tabs => EngineCommand::Tabs,
            Command::Use { tab_id } => EngineCommand::UseTab { tab_id },
            Command::Close { tab_id } => EngineCommand::Close { tab_id },
            Command::Ax { outline, tree_json } => {
                let format = if tree_json && !outline {
                    crate::protocol::AxFormat::Json
                } else {
                    crate::protocol::AxFormat::Outline
                };
                EngineCommand::Ax { format }
            }
            Command::Html => EngineCommand::Html,
            Command::Text { selector } => EngineCommand::Text { selector },
            Command::Eval { js } => EngineCommand::Eval { js },
            Command::Screenshot { out, full, inline } => {
                EngineCommand::Screenshot { out, full, inline }
            }
            Command::Click { selector } => EngineCommand::Click { selector },
            Command::Type { selector, text } => EngineCommand::Type { selector, text },
            Command::Press { key, selector } => EngineCommand::Press { key, selector },
            Command::Wait {
                selector,
                timeout_ms,
            } => EngineCommand::Wait {
                selector,
                timeout_ms,
            },
            Command::Listen { command } => match command {
                ListenCommand::Start { keywords, xhr_only } => {
                    EngineCommand::ListenStart { keywords, xhr_only }
                }
                ListenCommand::Wait { count, timeout_ms } => {
                    EngineCommand::ListenWait { count, timeout_ms }
                }
                ListenCommand::Stop => EngineCommand::ListenStop,
            },
            Command::PassCf { timeout_ms } => EngineCommand::PassCf { timeout_ms },
        })
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;

    #[test]
    fn parses_global_json_open() {
        let cli = Cli::try_parse_from(["drs", "--json", "open", "https://example.com"]).unwrap();
        assert!(cli.json);
        match cli.command.into_engine().unwrap() {
            EngineCommand::Open { url } => assert_eq!(url, "https://example.com"),
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parses_type_command() {
        let cli = Cli::try_parse_from(["drs", "type", "#kw", "hello"]).unwrap();
        match cli.command.into_engine().unwrap() {
            EngineCommand::Type { selector, text } => {
                assert_eq!(selector, "#kw");
                assert_eq!(text, "hello");
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parses_ax_json_flag() {
        let cli = Cli::try_parse_from(["drs", "ax", "--json"]).unwrap();
        match cli.command.into_engine().unwrap() {
            EngineCommand::Ax { format } => {
                assert!(matches!(format, crate::protocol::AxFormat::Json));
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }
}
