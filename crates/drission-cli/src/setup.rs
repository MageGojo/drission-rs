//! `drs setup`: auto-configure the `drs` MCP server for Cursor and/or Codex.
//!
//! The whole point is that an AI agent (or a first-time user) can run a single
//! command and have the `drs` browser MCP wired into their editor, so that any
//! "hard to get" web data (login-gated, anti-bot, Cloudflare, JS-rendered) is
//! fetched through the persistent `drs` browser instead of raw HTTP.

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde_json::{Map, Value, json};

use crate::protocol::{BackendKind, JsonResponse};

/// Which editor/client to configure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum SetupTarget {
    /// Configure both Cursor and Codex.
    Both,
    /// Configure Cursor only.
    Cursor,
    /// Configure Codex only.
    Codex,
}

/// Where to write the Cursor MCP config.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum SetupScope {
    /// Project-local `.cursor/mcp.json` (recommended).
    Project,
    /// Global `~/.cursor/mcp.json`.
    Global,
}

pub struct SetupOptions {
    pub target: SetupTarget,
    pub scope: SetupScope,
    pub dir: Option<PathBuf>,
    pub backend: BackendKind,
    pub headless: bool,
    pub name: String,
    pub dry_run: bool,
}

/// Resolve the absolute path to the running `drs` binary so the generated MCP
/// config does not depend on the editor inheriting the user's `PATH`.
fn drs_exe() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|p| std::fs::canonicalize(&p).ok().or(Some(p)))
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "drs".to_string())
}

fn server_args(backend: BackendKind, headless: bool) -> Vec<String> {
    let mut args = vec![
        "mcp".to_string(),
        "--backend".to_string(),
        backend.to_string(),
    ];
    if headless {
        args.push("--headless".to_string());
    }
    args
}

fn home_dir() -> Result<PathBuf> {
    directories::BaseDirs::new()
        .map(|b| b.home_dir().to_path_buf())
        .context("cannot locate home directory")
}

fn cursor_config_path(opts: &SetupOptions) -> Result<PathBuf> {
    match opts.scope {
        SetupScope::Global => Ok(home_dir()?.join(".cursor").join("mcp.json")),
        SetupScope::Project => {
            let base = match &opts.dir {
                Some(dir) => dir.clone(),
                None => std::env::current_dir().context("resolve current directory")?,
            };
            Ok(base.join(".cursor").join("mcp.json"))
        }
    }
}

fn codex_config_path() -> Result<PathBuf> {
    Ok(home_dir()?.join(".codex").join("config.toml"))
}

/// Merge (or insert) the `drs` server into a Cursor `mcp.json` document.
async fn configure_cursor(opts: &SetupOptions, exe: &str, args: &[String]) -> Result<Value> {
    let path = cursor_config_path(opts)?;
    let mut root: Value = if path.exists() {
        let text = tokio::fs::read_to_string(&path)
            .await
            .with_context(|| format!("read {}", path.display()))?;
        if text.trim().is_empty() {
            json!({})
        } else {
            serde_json::from_str(&text)
                .with_context(|| format!("parse existing {}", path.display()))?
        }
    } else {
        json!({})
    };

    if !root.is_object() {
        anyhow::bail!("{} is not a JSON object", path.display());
    }
    let obj = root.as_object_mut().unwrap();
    let servers = obj
        .entry("mcpServers")
        .or_insert_with(|| Value::Object(Map::new()));
    if !servers.is_object() {
        anyhow::bail!("mcpServers in {} is not an object", path.display());
    }
    let servers = servers.as_object_mut().unwrap();
    let existed = servers.contains_key(&opts.name);
    servers.insert(
        opts.name.clone(),
        json!({
            "command": exe,
            "args": args,
            "env": {},
        }),
    );

    if !opts.dry_run {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let pretty = serde_json::to_string_pretty(&root)? + "\n";
        tokio::fs::write(&path, pretty)
            .await
            .with_context(|| format!("write {}", path.display()))?;
    }

    Ok(json!({
        "client": "cursor",
        "path": path.display().to_string(),
        "action": if existed { "updated" } else { "created" },
        "scope": match opts.scope { SetupScope::Global => "global", SetupScope::Project => "project" },
        "written": !opts.dry_run,
    }))
}

/// Insert or update the `[mcp_servers.<name>]` table in Codex `config.toml`.
async fn configure_codex(opts: &SetupOptions, exe: &str, args: &[String]) -> Result<Value> {
    let path = codex_config_path()?;
    let text = if path.exists() {
        tokio::fs::read_to_string(&path)
            .await
            .with_context(|| format!("read {}", path.display()))?
    } else {
        String::new()
    };

    let mut doc: toml_edit::DocumentMut = text
        .parse()
        .with_context(|| format!("parse existing {}", path.display()))?;

    let existed = doc
        .get("mcp_servers")
        .and_then(|s| s.get(&opts.name))
        .is_some();

    // Ensure a `[mcp_servers]` parent table that is *implicit* (only the
    // `[mcp_servers.<name>]` child headers are printed, matching Codex style).
    let servers = doc["mcp_servers"].or_insert(toml_edit::table());
    if let Some(table) = servers.as_table_mut() {
        table.set_implicit(true);
    }

    let mut arr = toml_edit::Array::new();
    for a in args {
        arr.push(a.as_str());
    }
    let mut entry = toml_edit::Table::new();
    entry["command"] = toml_edit::value(exe);
    entry["args"] = toml_edit::value(arr);
    entry["startup_timeout_sec"] = toml_edit::value(120);
    doc["mcp_servers"][&opts.name] = toml_edit::Item::Table(entry);

    if !opts.dry_run {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&path, doc.to_string())
            .await
            .with_context(|| format!("write {}", path.display()))?;
    }

    Ok(json!({
        "client": "codex",
        "path": path.display().to_string(),
        "action": if existed { "updated" } else { "created" },
        "written": !opts.dry_run,
    }))
}

pub async fn run_setup(opts: SetupOptions) -> Result<JsonResponse> {
    let exe = drs_exe();
    let args = server_args(opts.backend, opts.headless);
    let mut results = Vec::new();

    if matches!(opts.target, SetupTarget::Both | SetupTarget::Cursor) {
        results.push(configure_cursor(&opts, &exe, &args).await?);
    }
    if matches!(opts.target, SetupTarget::Both | SetupTarget::Codex) {
        results.push(configure_codex(&opts, &exe, &args).await?);
    }

    Ok(JsonResponse::ok(json!({
        "serverName": opts.name,
        "command": exe,
        "args": args,
        "dryRun": opts.dry_run,
        "configured": results,
        "nextSteps": [
            "restart Cursor / Codex so the new MCP server is picked up",
            "then ask the agent to use the `drs` browser tools for hard-to-get web data",
        ],
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cursor_config_merges_without_clobbering() {
        let dir = std::env::temp_dir().join(format!("drs-setup-cursor-{}", std::process::id()));
        let cfg = dir.join(".cursor").join("mcp.json");
        tokio::fs::create_dir_all(cfg.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&cfg, r#"{"mcpServers":{"other":{"command":"x"}}}"#)
            .await
            .unwrap();

        let opts = SetupOptions {
            target: SetupTarget::Cursor,
            scope: SetupScope::Project,
            dir: Some(dir.clone()),
            backend: BackendKind::Cdp,
            headless: true,
            name: "drs".to_string(),
            dry_run: false,
        };
        configure_cursor(&opts, "/abs/drs", &server_args(BackendKind::Cdp, true))
            .await
            .unwrap();

        let text = tokio::fs::read_to_string(&cfg).await.unwrap();
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["mcpServers"]["other"]["command"], "x");
        assert_eq!(value["mcpServers"]["drs"]["command"], "/abs/drs");
        assert_eq!(value["mcpServers"]["drs"]["args"][0], "mcp");
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[test]
    fn codex_toml_renders_header_table_and_preserves_others() {
        let mut doc: toml_edit::DocumentMut =
            "model = \"gpt-5\"\n\n[mcp_servers.node_repl]\ncommand = \"x\"\n\n[mcp_servers.node_repl.env]\nFOO = \"bar\"\n"
                .parse()
                .unwrap();

        let servers = doc["mcp_servers"].or_insert(toml_edit::table());
        if let Some(table) = servers.as_table_mut() {
            table.set_implicit(true);
        }
        let mut arr = toml_edit::Array::new();
        for a in server_args(BackendKind::Cdp, true) {
            arr.push(a.as_str());
        }
        let mut entry = toml_edit::Table::new();
        entry["command"] = toml_edit::value("/abs/drs");
        entry["args"] = toml_edit::value(arr);
        entry["startup_timeout_sec"] = toml_edit::value(120);
        doc["mcp_servers"]["drs"] = toml_edit::Item::Table(entry);

        let out = doc.to_string();
        assert!(out.contains("[mcp_servers.drs]"), "got:\n{out}");
        assert!(out.contains("[mcp_servers.node_repl]"));
        assert!(out.contains("FOO = \"bar\""));
        assert!(out.contains("model = \"gpt-5\""));
        // must still be valid TOML after edit
        let _: toml_edit::DocumentMut = out.parse().unwrap();
    }
}
