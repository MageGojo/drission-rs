//! CDP 后端**多任务下载管理** `tab.downloads()` 端到端真机自验证(完全离线)。
//!
//! 进程内起一个极简 HTTP 服务,以 `Content-Disposition: attachment` 返回若干文件;真实 Chrome
//! 点击 `<a download>` 触发**真实下载**,从而走通 CDP 原生 `Page.downloadWillBegin` /
//! `Page.downloadProgress` 事件(按 `guid` 聚合,自带 received/total 字节)。覆盖:
//!   ① `start()` 开始跟踪 + `listening()`;
//!   ② 顺序下载:`wait_new`(下一个新任务)+ `wait_done`(下一个完成);
//!   ③ **并发下载**:一次触发两个,`wait_count_done(2, ..)` 收齐;
//!   ④ `missions()` 任务列表快照(含实时进度字节);
//!   ⑤ 落盘内容核对 + `downloaded_bytes()`/`total_bytes` 与文件大小一致;
//!   ⑥ `save_as` 自定义重命名;⑦ `stop()` 后 `listening()` 翻转。
//!
//! 运行:`cargo run --example cdp_download`(无头默认;`HL=0` 开窗口;`CHROME_BIN` 指定浏览器)。
//! 任一关键校验失败即非 0 退出;全部通过打印 `ALL CHECKS PASSED`。

use std::collections::HashMap;
use std::time::Duration;

use drission::prelude::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const PAGE_HTML: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>CDP Downloads</title></head>
<body>
<h1 id="title">CDP 下载管理</h1>
<a id="d1" href="/f/alpha" download="alpha.txt">下载 alpha</a>
<a id="d2" href="/f/beta" download="beta.txt">下载 beta</a>
<a id="d3" href="/f/gamma" download="gamma.bin">下载 gamma</a>
<button id="burst">并发下载 beta+gamma</button>
<script>
  // 在一次可信点击的用户手势内同时触发两个下载,验证 guid 并发聚合。
  document.getElementById('burst').addEventListener('click', function(){
    document.getElementById('d2').click();
    document.getElementById('d3').click();
  });
</script>
</body></html>"#;

/// 服务端与校验端**同一来源**:`/f/<name>` 的 (落盘文件名, 内容)。
fn payload(name: &str) -> Option<(&'static str, String)> {
    match name {
        "alpha" => Some(("alpha.txt", "alpha-content-from-cdp".to_string())),
        "beta" => Some(("beta.txt", "beta-content-from-cdp".to_string())),
        // ~20KB,确保 downloadProgress 带出真实 total/received 字节(非零)。
        "gamma" => Some(("gamma.bin", "0123456789".repeat(2000))),
        _ => None,
    }
}

#[tokio::main]
async fn main() -> drission::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .init();

    // ── 进程内极简 HTTP 服务(localhost,不受出网拦截影响)──────────────────
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    tokio::spawn(serve(listener));
    let base = format!("http://127.0.0.1:{port}");
    println!("[*] 本地服务: {base}");

    // ── 下载目录(项目 target 下,Chrome 可写)──────────────────────────────
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("drission-cdp-dl");
    let _ = tokio::fs::remove_dir_all(&root).await; // 清理上次残留
    let dl_dir = root.join("downloads");
    tokio::fs::create_dir_all(&dl_dir).await?;

    let headless = std::env::var("HL").map(|v| v != "0").unwrap_or(true);
    println!(
        "[*] 启动 Chrome(headless={headless}),下载目录={}",
        dl_dir.display()
    );
    let browser = Browser::launch(
        BrowserOptions::new()
            .headless(headless)
            .download_path(&dl_dir),
    )
    .await?;
    let tab = browser.new_tab(Some("about:blank")).await?;
    tab.get(&format!("{base}/")).await?;
    tab.ele("#title").await?; // 等页面就绪

    let mut failed = false;
    macro_rules! check {
        ($cond:expr, $($arg:tt)*) => {{
            let ok = $cond;
            println!("[{}] {}", if ok { "ok" } else { "FAIL" }, format!($($arg)*));
            if !ok { failed = true; }
        }};
    }

    // ── ① 开始跟踪 ─────────────────────────────────────────────────────────
    let dl = tab.downloads();
    dl.start().await?;
    check!(
        dl.listening().await,
        "downloads().start() → listening == true"
    );

    // ── ② 顺序下载 alpha:wait_new + wait_done ──────────────────────────────
    tab.ele("#d1").await?.click().await?; // 可信点击触发下载
    let new1 = dl.wait_new(Duration::from_secs(10)).await?;
    check!(
        new1.as_ref().map(|m| m.suggested_filename.as_str()) == Some("alpha.txt"),
        "wait_new → 新任务 alpha.txt(实得 {:?})",
        new1.as_ref().map(|m| &m.suggested_filename)
    );
    let m1 = dl.wait_done(Duration::from_secs(15)).await?;
    let m1 = match m1 {
        Some(m) => m,
        None => return finish(&browser, false, "未等到 alpha 下载完成").await,
    };
    let c1 = tokio::fs::read_to_string(&m1.path)
        .await
        .unwrap_or_default();
    let want1 = payload("alpha").map(|p| p.1).unwrap_or_default();
    check!(
        m1.succeeded() && c1 == want1,
        "alpha 完成且内容一致(succeeded={} len={})",
        m1.succeeded(),
        c1.len()
    );
    check!(
        m1.downloaded_bytes() == want1.len() as u64 && m1.total_bytes == want1.len() as u64,
        "alpha 字节: received={} total={} 期望={}",
        m1.downloaded_bytes(),
        m1.total_bytes,
        want1.len()
    );

    // ── ③ 并发下载 beta + gamma:wait_count_done(2) ─────────────────────────
    tab.ele("#burst").await?.click().await?; // 一次手势触发两个下载
    let done = dl.wait_count_done(2, Duration::from_secs(25)).await?;
    check!(
        done.len() == 2,
        "wait_count_done(2) 收齐 2 个(实得 {})",
        done.len()
    );
    let by_name: HashMap<String, DownloadMission> = done
        .into_iter()
        .map(|m| (m.suggested_filename.clone(), m))
        .collect();

    // 校验并发的两个文件各自落盘内容/字节。
    let mut concurrent_ok = true;
    for key in ["beta", "gamma"] {
        let (fname, body) = payload(key).unwrap();
        match by_name.get(fname) {
            Some(m) => {
                let disk = tokio::fs::read_to_string(&m.path).await.unwrap_or_default();
                let ok = m.succeeded()
                    && disk == body
                    && m.downloaded_bytes() == body.len() as u64
                    && m.total_bytes == body.len() as u64;
                println!(
                    "    {fname}: succeeded={} disk_len={} received={} total={} (ok={ok})",
                    m.succeeded(),
                    disk.len(),
                    m.downloaded_bytes(),
                    m.total_bytes
                );
                concurrent_ok &= ok;
            }
            None => {
                println!("    {fname}: 缺失(并发下载未收到)");
                concurrent_ok = false;
            }
        }
    }
    check!(concurrent_ok, "并发 beta/gamma 内容与字节均一致");

    // ── ④ 任务列表快照(3 个全成功)──────────────────────────────────────
    let missions = dl.missions().await;
    check!(
        missions.len() == 3 && missions.iter().all(|m| m.succeeded()),
        "missions() == 3 且全部成功(实得 {})",
        missions.len()
    );

    // ── ⑤ save_as 自定义重命名(把 gamma 移走)─────────────────────────────
    if let Some(gamma) = by_name.get("gamma.bin") {
        let renamed = root.join("renamed-gamma.bin");
        let saved = gamma.save_as(&renamed).await?;
        let exists = tokio::fs::try_exists(&saved).await.unwrap_or(false);
        let same = tokio::fs::read_to_string(&saved).await.unwrap_or_default()
            == payload("gamma").map(|p| p.1).unwrap_or_default();
        check!(
            exists && same,
            "save_as → {saved:?}(存在={exists} 内容一致={same})"
        );
    } else {
        check!(false, "save_as 跳过:gamma 缺失");
    }

    // ── ⑥ 停止跟踪 ─────────────────────────────────────────────────────────
    dl.stop().await?;
    check!(!dl.listening().await, "stop() → listening == false");

    finish(&browser, !failed, "cdp_download 自验证未通过").await
}

async fn finish(browser: &Browser, pass: bool, err: &str) -> drission::Result<()> {
    println!(
        "\n==== {} ====",
        if pass {
            "ALL CHECKS PASSED"
        } else {
            "SOME CHECKS FAILED"
        }
    );
    browser.quit().await?;
    if pass {
        Ok(())
    } else {
        Err(drission::Error::msg(err.to_string()))
    }
}

/// 极简 HTTP/1.1 服务:`/f/<name>` 以 attachment 返回文件,其余返回页面 HTML。每连接应答后关闭。
async fn serve(listener: TcpListener) {
    loop {
        let Ok((mut sock, _)) = listener.accept().await else {
            break;
        };
        tokio::spawn(async move {
            let mut buf = vec![0u8; 8192];
            let n = sock.read(&mut buf).await.unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]);
            let path = req
                .lines()
                .next()
                .and_then(|l| l.split_whitespace().nth(1))
                .unwrap_or("/");
            let resp = if let Some(name) = path.strip_prefix("/f/") {
                let name = name.split('?').next().unwrap_or(name);
                match payload(name) {
                    Some((filename, body)) => format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Disposition: attachment; filename=\"{}\"\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        filename,
                        body.len(),
                        body
                    ),
                    None => {
                        "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                            .to_string()
                    }
                }
            } else {
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    PAGE_HTML.len(),
                    PAGE_HTML
                )
            };
            let _ = sock.write_all(resp.as_bytes()).await;
        });
    }
}
