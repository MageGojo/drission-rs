//! 预下载 / 校验本机 Camoufox 可执行文件(首次会自动下载到 `~/.cache/camoufox`)。
//!
//! 运行:`cargo run --example fetch_browser --no-default-features --features camoufox`

#[tokio::main]
async fn main() -> drission::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let path = drission::launcher::ensure_camoufox(None).await?;
    println!("Camoufox 可执行文件: {}", path.display());
    Ok(())
}
