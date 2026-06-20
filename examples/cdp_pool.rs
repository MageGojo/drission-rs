//! CDP 高并发池 `ChromiumPool` 端到端真机自验证(完全离线:进程内 HTTP 服务 + 本机 Chrome)。
//!
//! 覆盖:① 多 worker 并发(2 worker × 2 标签 = 并发 4,服务端记录在飞峰值证明真并行)
//! ② `map` 保序返回 ③ **每任务独立 BrowserContext**(起始 `document.cookie` 为空 → cookie 隔离)
//! ④ `map_resumable` 断点续抓(配合 `Checkpoint`,续跑只补未完成项)。
//!
//! 运行:`cargo run --example cdp_pool`(无头默认;`HL=0` 开窗口)。
//! 任一关键校验失败即非 0 退出;全部通过打印 `ALL CHECKS PASSED`。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use drission::prelude::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// 服务端在飞请求计数(当前 / 历史峰值),用于证明池确实并发打请求。
struct Conc {
    cur: AtomicUsize,
    max: AtomicUsize,
}

#[tokio::main]
async fn main() -> drission::Result<()> {
    let conc = Arc::new(Conc {
        cur: AtomicUsize::new(0),
        max: AtomicUsize::new(0),
    });
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    tokio::spawn(serve(listener, conc.clone()));
    let base = format!("http://127.0.0.1:{port}");
    println!("[*] 本地服务: {base}");

    let headless = std::env::var("HL").map(|v| v != "0").unwrap_or(true);
    println!("[*] 启动 ChromiumPool(2 worker × 2 标签,headless={headless})");
    let pool = ChromiumPool::launch(
        ChromiumPoolOptions::new()
            .size(2)
            .tabs_per_worker(2)
            .base_options(ChromiumOptions::new().headless(headless)),
    )
    .await?;
    println!(
        "[*] worker_count={} concurrency={}",
        pool.worker_count(),
        pool.concurrency()
    );

    let mut failed = false;
    macro_rules! check {
        ($cond:expr, $($arg:tt)*) => {{
            let ok = $cond;
            println!("[{}] {}", if ok { "ok" } else { "FAIL" }, format!($($arg)*));
            if !ok { failed = true; }
        }};
    }

    // ── ①②③ map 保序 + 并发 + 每任务独立 context ────────────────────────────
    let base_cl = base.clone();
    let results = pool
        .map((0..6usize).collect::<Vec<_>>(), move |i, tab| {
            let base = base_cl.clone();
            async move {
                tab.get(&format!("{base}/p/{i}")).await?;
                // 起始 cookie 应为空(每任务独立 BrowserContext → 不串其它任务设过的 cookie)。
                let cookie0 = tab
                    .run_js("document.cookie")
                    .await?
                    .as_str()
                    .unwrap_or("")
                    .to_string();
                let _ = tab.run_js("document.cookie='visited=1'").await?;
                let path = tab
                    .run_js("location.pathname")
                    .await?
                    .as_str()
                    .unwrap_or("")
                    .to_string();
                Ok::<(bool, String), drission::Error>((cookie0.is_empty(), path))
            }
        })
        .await;

    let mut all_ok = true;
    let mut order_ok = results.len() == 6;
    let mut all_isolated = true;
    for (idx, (item, res)) in results.iter().enumerate() {
        if *item != idx {
            order_ok = false;
        }
        match res {
            Ok((iso, path)) => {
                if !iso {
                    all_isolated = false;
                }
                if path != &format!("/p/{item}") {
                    order_ok = false;
                }
            }
            Err(e) => {
                all_ok = false;
                println!("    任务 {item} 失败: {e}");
            }
        }
    }
    check!(results.len() == 6 && all_ok, "map 6 项全部成功");
    check!(order_ok, "结果按输入顺序返回(item==索引、path 匹配)");
    check!(
        all_isolated,
        "每任务独立 BrowserContext(起始 document.cookie 为空 → cookie 隔离)"
    );

    let maxc = conc.max.load(Ordering::Relaxed);
    check!(maxc >= 2, "服务端在飞请求峰值 {maxc} >= 2(证明真并发)");

    // ── ④ map_resumable 断点续抓 ───────────────────────────────────────────
    let ckpt_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("cdp_pool_ckpt.jsonl");
    let _ = tokio::fs::remove_file(&ckpt_path).await;
    let ckpt = Checkpoint::load(&ckpt_path).await?;

    let base_cl2 = base.clone();
    let first = pool
        .map_resumable(
            vec![0usize, 1, 2],
            |i| format!("k{i}"),
            &ckpt,
            move |i, tab| {
                let base = base_cl2.clone();
                async move {
                    tab.get(&format!("{base}/p/{i}")).await?;
                    Ok::<(), drission::Error>(())
                }
            },
        )
        .await;
    check!(first.len() == 3, "首跑执行 3 项(实得 {})", first.len());

    let base_cl3 = base.clone();
    let second = pool
        .map_resumable(
            vec![0usize, 1, 2, 3],
            |i| format!("k{i}"),
            &ckpt,
            move |i, tab| {
                let base = base_cl3.clone();
                async move {
                    tab.get(&format!("{base}/p/{i}")).await?;
                    Ok::<(), drission::Error>(())
                }
            },
        )
        .await;
    check!(
        second.len() == 1,
        "续跑只补 1 项(前 3 项已完成被跳过,实得 {})",
        second.len()
    );

    pool.shutdown().await?;

    println!(
        "\n==== {} ====",
        if failed {
            "SOME CHECKS FAILED"
        } else {
            "ALL CHECKS PASSED"
        }
    );
    if failed {
        std::process::exit(1);
    }
    Ok(())
}

/// 极简 HTTP/1.1 服务:每请求计入在飞峰值,小睡 150ms 让并发请求重叠,返回最小 HTML。
async fn serve(listener: TcpListener, conc: Arc<Conc>) {
    loop {
        let Ok((mut sock, _)) = listener.accept().await else {
            break;
        };
        let conc = conc.clone();
        tokio::spawn(async move {
            let cur = conc.cur.fetch_add(1, Ordering::SeqCst) + 1;
            conc.max.fetch_max(cur, Ordering::SeqCst);
            let mut buf = vec![0u8; 4096];
            let _ = sock.read(&mut buf).await;
            tokio::time::sleep(Duration::from_millis(150)).await;
            let body = "<!doctype html><html><head><meta charset=\"utf-8\"><title>pool</title></head><body><h1>ok</h1></body></html>";
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = sock.write_all(resp.as_bytes()).await;
            conc.cur.fetch_sub(1, Ordering::SeqCst);
        });
    }
}
