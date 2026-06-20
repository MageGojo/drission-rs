//! Windows 传输**诊断**:一次运行依次跑「无头 / 有头」两种模式的完整链路
//! (启动 → 取标签 → 导航 → 读标题 → 执行 JS → 退出),把**每个阶段的耗时/成败/错误**
//! 以及**浏览器进程的 stdout/stderr 原文**全部落盘到一个 JSON。
//!
//! 这样无需反复试:一份 `drission_win_diag.json` 就能判定到底卡在哪个阶段、是不是浏览器
//! 进程崩了(`browser_log_*` 里会有 GFX/XPCOM/缺 DLL 等真实报错)、还是纯传输问题
//! (日志里出现 `Juggler listening` 但首个命令 `连接已关闭` → 管道被提前关闭)。
//!
//! 运行:
//!   cargo run --example win_diag                 # 默认 https://example.com,先无头后有头
//!   cargo run --example win_diag -- https://example.com
//!   PowerShell 想另看实时日志:`$env:RUST_LOG="camoufox=debug,drission=debug"`(可选)
//!
//! 跑完把 `drission_win_diag.json` 发回即可。

use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use drission::prelude::*;
use serde_json::{Value, json};

/// 把 tracing 的格式化输出收集进内存缓冲——用于把**浏览器 stdout/stderr**(库内以
/// `target="camoufox"` 打到 tracing)原样塞进结果 JSON,免得用户还要单独抓日志。
#[derive(Clone)]
struct LogBuf(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for LogBuf {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if let Ok(mut g) = self.0.lock() {
            g.extend_from_slice(buf);
        }
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogBuf {
    type Writer = LogBuf;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

fn buf_len(buf: &Arc<Mutex<Vec<u8>>>) -> usize {
    buf.lock().map(|b| b.len()).unwrap_or(0)
}

/// 取出 `from` 偏移之后新增的日志,只保留与浏览器/传输相关的行(并截断到末尾 300 行)。
fn buf_lines_since(buf: &Arc<Mutex<Vec<u8>>>, from: usize) -> Vec<String> {
    let g = match buf.lock() {
        Ok(g) => g,
        Err(_) => return Vec::new(),
    };
    let start = from.min(g.len());
    let text = String::from_utf8_lossy(&g[start..]);
    let mut lines: Vec<String> = text
        .lines()
        .filter(|l| {
            l.contains("camoufox")
                || l.contains("Camoufox")
                || l.contains("Juggler")
                || l.contains("ERROR")
                || l.contains("WARN")
                || l.contains("就绪")
                || l.contains("启动参数")
        })
        .map(|l| l.trim_end().to_string())
        .collect();
    let n = lines.len();
    if n > 300 {
        lines = lines.split_off(n - 300);
    }
    lines
}

/// 跑一种模式(headless / headed)的完整链路,逐阶段记录,返回该模式的结果 JSON。
async fn run_mode(url: &str, headless: bool) -> Value {
    let phases = Arc::new(Mutex::new(Vec::<Value>::new()));
    let push = {
        let phases = phases.clone();
        move |name: &str, t: Instant, detail: std::result::Result<String, String>| {
            let ok = detail.is_ok();
            if let Ok(mut p) = phases.lock() {
                p.push(json!({
                    "phase": name,
                    "ok": ok,
                    "ms": t.elapsed().as_millis() as u64,
                    "detail": match detail { Ok(s) | Err(s) => s },
                }));
            }
        }
    };

    let mut ua = String::new();
    let mut wd = String::new();
    let mut first_err: Option<String> = None;
    let started = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let t0 = Instant::now();

    // 阶段 1:启动(内部含 launcher 启动 + Juggler 就绪 + `Browser.enable` 首个命令)。
    let t = Instant::now();
    let browser = match Browser::launch(BrowserOptions::new().headless(headless)).await {
        Ok(b) => {
            push(
                "launch",
                t,
                Ok("browser launched + Browser.enable ok".into()),
            );
            Some(b)
        }
        Err(e) => {
            first_err.get_or_insert(format!("launch: {e}"));
            push("launch", t, Err(e.to_string()));
            None
        }
    };

    if let Some(browser) = browser {
        let t = Instant::now();
        match browser.latest_tab().await {
            Ok(tab) => {
                push("latest_tab", t, Ok("tab ready".into()));

                let t = Instant::now();
                match tab.get(url).await {
                    Ok(ok) => push("get", t, Ok(format!("loaded={ok}"))),
                    Err(e) => {
                        first_err.get_or_insert(format!("get: {e}"));
                        push("get", t, Err(e.to_string()));
                    }
                }

                let t = Instant::now();
                match tab.title().await {
                    Ok(title) => push("title", t, Ok(format!("{title:?}"))),
                    Err(e) => {
                        first_err.get_or_insert(format!("title: {e}"));
                        push("title", t, Err(e.to_string()));
                    }
                }

                let t = Instant::now();
                match tab.run_js("navigator.userAgent").await {
                    Ok(v) => {
                        ua = v.as_str().unwrap_or_default().to_string();
                        push("js_userAgent", t, Ok(ua.clone()));
                    }
                    Err(e) => {
                        first_err.get_or_insert(format!("js_userAgent: {e}"));
                        push("js_userAgent", t, Err(e.to_string()));
                    }
                }

                let t = Instant::now();
                match tab.run_js("navigator.webdriver").await {
                    Ok(v) => {
                        wd = v.to_string();
                        push("js_webdriver", t, Ok(wd.clone()));
                    }
                    Err(e) => {
                        first_err.get_or_insert(format!("js_webdriver: {e}"));
                        push("js_webdriver", t, Err(e.to_string()));
                    }
                }
            }
            Err(e) => {
                first_err.get_or_insert(format!("latest_tab: {e}"));
                push("latest_tab", t, Err(e.to_string()));
            }
        }

        // 始终优雅退出(`Browser.close` 经 Juggler 通知浏览器关闭,避免遗留进程干扰下一模式)。
        let t = Instant::now();
        match browser.quit().await {
            Ok(_) => push("quit", t, Ok("ok".into())),
            Err(e) => push("quit", t, Err(e.to_string())),
        }
    }

    json!({
        "mode": if headless { "headless" } else { "headed" },
        "ok": first_err.is_none(),
        "error": first_err,
        "started_unix_ms": started,
        "elapsed_ms": t0.elapsed().as_millis() as u64,
        "navigator": { "userAgent": ua, "webdriver": wd },
        "phases": Value::Array(phases.lock().map(|p| p.clone()).unwrap_or_default()),
    })
}

#[tokio::main]
async fn main() {
    let buf = Arc::new(Mutex::new(Vec::<u8>::new()));
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new("warn,camoufox=debug,drission=debug")
            }),
        )
        .with_writer(LogBuf(buf.clone()))
        .with_ansi(false)
        .init();

    let url = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "https://example.com".to_string());

    println!("== drission-rs Windows 传输诊断 (win_diag) ==");
    println!(
        "  OS/ARCH : {}/{}",
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    println!("  URL     : {url}");
    println!();

    let cam = drission::launcher::ensure_camoufox(None).await.ok();
    match &cam {
        Some(c) => println!("  Camoufox: {}", c.display()),
        None => println!("  Camoufox: <定位/下载失败,见结果文件>"),
    }
    println!();

    println!("[1/2] 无头模式(headless)…");
    let off = buf_len(&buf);
    let headless_res = run_mode(&url, true).await;
    let headless_log = buf_lines_since(&buf, off);
    let hok = headless_res
        .get("ok")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    println!("      结果:{}", if hok { "通 ✅" } else { "不通 ❌" });

    println!("[2/2] 有头模式(headed)…");
    let off = buf_len(&buf);
    let headed_res = run_mode(&url, false).await;
    let headed_log = buf_lines_since(&buf, off);
    let dok = headed_res
        .get("ok")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    println!("      结果:{}", if dok { "通 ✅" } else { "不通 ❌" });

    let result = json!({
        "tool": "drission-rs",
        "test": "win_diag",
        "platform": { "os": std::env::consts::OS, "arch": std::env::consts::ARCH },
        "camoufox_path": cam.map(|c| c.display().to_string()),
        "url": url,
        "headless": headless_res,
        "headed": headed_res,
        "browser_log_headless": headless_log,
        "browser_log_headed": headed_log,
    });

    let out = "drission_win_diag.json";
    let pretty = serde_json::to_string_pretty(&result).unwrap_or_else(|_| result.to_string());
    if let Err(e) = std::fs::write(out, &pretty) {
        eprintln!("写结果文件失败: {e}");
    }

    println!();
    println!(
        "无头:{}    有头:{}",
        if hok { "✅" } else { "❌" },
        if dok { "✅" } else { "❌" }
    );
    println!("结果文件:{out}(请把这个文件整份发回核对)");
}
