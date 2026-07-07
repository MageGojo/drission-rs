use std::path::PathBuf;

use anyhow::{Context, Result, anyhow};
use directories::BaseDirs;

pub fn cli_dir() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("DRS_CLI_DIR") {
        return Ok(PathBuf::from(dir));
    }
    let base = BaseDirs::new().ok_or_else(|| anyhow!("cannot locate user cache directory"))?;
    Ok(base.cache_dir().join("drission").join("cli"))
}

pub fn state_path() -> Result<PathBuf> {
    Ok(cli_dir()?.join("drs-server.json"))
}

/// Stable browser profile directory reused across daemon restarts so that
/// cookies and logged-in sessions survive. Callers may override with an
/// explicit `--user-data-dir`.
pub fn default_profile_dir() -> Result<PathBuf> {
    Ok(cli_dir()?.join("profile"))
}

pub fn screenshots_dir() -> Result<PathBuf> {
    Ok(cli_dir()?.join("screenshots"))
}

pub async fn ensure_cli_dir() -> Result<PathBuf> {
    let dir = cli_dir()?;
    tokio::fs::create_dir_all(&dir)
        .await
        .with_context(|| format!("create {}", dir.display()))?;
    Ok(dir)
}
