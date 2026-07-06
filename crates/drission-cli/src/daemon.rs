use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use rand::{Rng, distributions::Alphanumeric};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex, watch};

use crate::engine::BrowserState;
use crate::paths;
use crate::protocol::{BackendKind, DaemonRequest, EngineCommand, JsonResponse, StateFile};

pub async fn run_server(
    backend: BackendKind,
    headless: bool,
    user_data_dir: Option<PathBuf>,
) -> Result<()> {
    paths::ensure_cli_dir().await?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let token = random_token();
    let state_file = StateFile {
        host: "127.0.0.1".to_string(),
        port: addr.port(),
        token,
        pid: std::process::id(),
        backend,
    };
    let browser = BrowserState::launch(backend, headless, user_data_dir).await?;
    write_state_file(&state_file).await?;
    let browser = Arc::new(Mutex::new(browser));
    let (stop_tx, mut stop_rx) = watch::channel(false);

    println!(
        "drs serve listening on {} (backend={backend}, pid={})",
        state_file.endpoint(),
        state_file.pid
    );

    loop {
        tokio::select! {
            accept = listener.accept() => {
                let (stream, _) = accept?;
                let browser = browser.clone();
                let token = state_file.token.clone();
                let stop_tx = stop_tx.clone();
                tokio::spawn(async move {
                    let _ = handle_connection(stream, browser, token, stop_tx).await;
                });
            }
            changed = stop_rx.changed() => {
                if changed.is_ok() && *stop_rx.borrow() {
                    break;
                }
            }
        }
    }

    let _ = remove_state_file().await;
    Ok(())
}

pub async fn send_to_daemon(command: EngineCommand) -> Result<JsonResponse> {
    let state = match read_state_file().await {
        Ok(state) => state,
        Err(e) => {
            return Ok(JsonResponse::err(
                "daemon_not_running",
                format!("drs daemon is not running: {e}"),
                Some("start it with `drs serve --headless`".to_string()),
            ));
        }
    };
    let mut stream = match TcpStream::connect(state.endpoint()).await {
        Ok(stream) => stream,
        Err(e) => {
            let _ = remove_state_file().await;
            return Ok(JsonResponse::err(
                "daemon_unreachable",
                format!("cannot connect to drs daemon: {e}"),
                Some("start it with `drs serve --headless`".to_string()),
            ));
        }
    };
    let req = DaemonRequest {
        token: state.token,
        command,
    };
    let line = serde_json::to_string(&req)? + "\n";
    stream.write_all(line.as_bytes()).await?;
    stream.flush().await?;

    let mut reader = BufReader::new(stream);
    let mut buf = String::new();
    reader.read_line(&mut buf).await?;
    if buf.trim().is_empty() {
        return Ok(JsonResponse::err(
            "empty_response",
            "daemon closed without a response",
            Some("check the `drs serve` terminal for errors".to_string()),
        ));
    }
    Ok(serde_json::from_str(buf.trim())?)
}

pub async fn read_state_file() -> Result<StateFile> {
    let path = paths::state_path()?;
    let data = tokio::fs::read_to_string(&path)
        .await
        .with_context(|| format!("read {}", path.display()))?;
    Ok(serde_json::from_str(&data)?)
}

pub async fn write_state_file(state: &StateFile) -> Result<()> {
    let path = paths::state_path()?;
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(&path, serde_json::to_string_pretty(state)?).await?;
    Ok(())
}

pub async fn remove_state_file() -> Result<()> {
    let path = paths::state_path()?;
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

async fn handle_connection(
    stream: TcpStream,
    browser: Arc<Mutex<BrowserState>>,
    token: String,
    stop_tx: watch::Sender<bool>,
) -> Result<()> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).await?;

    let response = match serde_json::from_str::<DaemonRequest>(line.trim()) {
        Ok(req) if req.token == token => {
            let mut guard = browser.lock().await;
            match guard.execute(req.command).await {
                Ok(result) => {
                    if result.stop {
                        let _ = stop_tx.send(true);
                    }
                    JsonResponse::ok(result.data)
                }
                Err(e) => JsonResponse::err("command_failed", e.to_string(), None),
            }
        }
        Ok(_) => JsonResponse::err(
            "unauthorized",
            "daemon token mismatch",
            Some("delete the stale state file or restart `drs serve`".to_string()),
        ),
        Err(e) => JsonResponse::err("bad_request", e.to_string(), None),
    };

    let mut stream = reader.into_inner();
    stream
        .write_all((serde_json::to_string(&response)? + "\n").as_bytes())
        .await?;
    stream.flush().await?;
    Ok(())
}

fn random_token() -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(40)
        .map(char::from)
        .collect()
}
