//! 并发池 `BrowserPool` 端到端自验证(完全离线,用 `data:` 页)。
//!
//! 覆盖里程碑 29~32 的核心能力:
//! - **A 并发 + 指纹轮换**:2 worker × 2 标签 = 并发 4;`map` 跑 6 个任务,各任务读
//!   时区(`Intl…timeZone`)/ 视口宽 / 语言,验证 [`FingerprintPool::presets`] **逐 context 轮换**生效
//!   (时区 distinct=5、视口 distinct=4 即证明 per-context 覆盖正确;并发下任务-指纹对应不串),
//!   且各任务读到自己的页面内容(隔离 + 并发)。
//!   **坑**:Camoufox 下 `navigator.language` 由进程级指纹层固定,per-context `setLocaleOverride`
//!   只改 Accept-Language、不改 `navigator.language`(故 language distinct=1);需要每身份不同
//!   `navigator.language` 时用 worker 级 `BrowserOptions`(CAMOU_CONFIG)。时区 / 视口 per-context 生效。
//! - **B 失败重试**:任务首次注入失败、第二次成功,验证 [`RetryPolicy`] 生效。
//! - **C 断点续抓**:`map_resumable` 首跑完成若干 key 落盘;再跑只补未完成项([`Checkpoint`])。
//!
//! 注:**健康自愈**(worker 进程死后惰性重建)在真实失败时触发,离线难以确定性复现,故此处不单测,
//! 仅由 `run` 的重试路径间接覆盖。代理轮换的纯逻辑见 `cargo test --lib pool::`。
//!
//! 运行:`cargo run --example pool_crawl --no-default-features --features camoufox`
//! 末行打印 `ALL CHECKS PASSED` / `SOME CHECKS FAILED`,关键校验失败则进程非 0 退出。

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use drission::prelude::*;

/// 测试用临时文件路径(写到项目 target 下,在 home 内、已 gitignore,规避 /var/folders 沙箱)。
fn tmp_path(name: &str) -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("target");
    p.push("test-tmp");
    p.push(format!("pool_crawl-{}-{}.jsonl", name, std::process::id()));
    p
}

#[tokio::main]
async fn main() -> drission::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .init();

    println!("[*] 启动并发池(2 worker × 2 标签,headless,指纹池=presets)…");
    let pool = BrowserPool::launch(
        PoolOptions::new()
            .size(2)
            .tabs_per_worker(2)
            .base_options(BrowserOptions::new().headless(true))
            .fingerprints(FingerprintPool::presets())
            .retry(RetryPolicy::new(2)),
    )
    .await?;
    println!(
        "    workers={} concurrency={}",
        pool.worker_count(),
        pool.concurrency()
    );

    // ---------- A: 并发 + 指纹轮换 + 隔离 ----------
    let items: Vec<u32> = (0..6).collect();
    let results = pool
        .map(items.clone(), |i, tab| async move {
            let url = format!("data:text/html,<html><body><h1 id=t>item-{i}</h1></body></html>");
            tab.get(&url).await?;
            // 收三个 per-context 信号:语言 / 时区 / 视口宽——验证哪些随 context 轮换生效。
            let sig = tab
                .run_js(
                    "JSON.stringify([\
                     navigator.language||'', \
                     (Intl.DateTimeFormat().resolvedOptions().timeZone)||'', \
                     ''+window.innerWidth])",
                )
                .await?;
            let arr: Vec<String> =
                serde_json::from_str(sig.as_str().unwrap_or("[]")).unwrap_or_default();
            let txt = tab.ele("#t").await?.text().await?;
            Ok::<(String, Vec<String>), drission::Error>((txt, arr))
        })
        .await;

    if let Some((_, Err(e))) = results.iter().find(|(_, r)| r.is_err()) {
        println!("    [debug] A 首个错误: {e}");
    }
    let all_ok = results.len() == 6 && results.iter().all(|(_, r)| r.is_ok());
    let content_ok = results.iter().enumerate().all(|(idx, (i, r))| {
        *i == idx as u32
            && r.as_ref()
                .map(|(t, _)| t == &format!("item-{i}"))
                .unwrap_or(false)
    });
    let sig_at = |k: usize| -> Vec<String> {
        results
            .iter()
            .filter_map(|(_, r)| r.as_ref().ok().and_then(|(_, v)| v.get(k).cloned()))
            .collect()
    };
    let langs = sig_at(0);
    let tzs = sig_at(1);
    let vws = sig_at(2);
    let distinct = |v: &[String]| -> usize { v.iter().collect::<HashSet<_>>().len() };
    println!(
        "    [diag] language: distinct={} {:?}",
        distinct(&langs),
        langs
    );
    println!("    [diag] timezone: distinct={} {:?}", distinct(&tzs), tzs);
    println!(
        "    [diag] innerWidth: distinct={} {:?}",
        distinct(&vws),
        vws
    );
    // 至少一个轻量指纹信号随 context 轮换(distinct >= 2)即证明 per-context 覆盖生效。
    let rotate_ok = distinct(&langs).max(distinct(&tzs)).max(distinct(&vws)) >= 2;
    println!(
        "[A] 并发 map: {}/6 成功; 顺序&内容 ok={content_ok}; 指纹轮换 ok={rotate_ok}",
        results.iter().filter(|(_, r)| r.is_ok()).count(),
    );
    let a_ok = all_ok && content_ok && rotate_ok;

    // ---------- B: 失败重试 ----------
    let attempts = Arc::new(AtomicUsize::new(0));
    let r_retry = {
        let attempts = attempts.clone();
        pool.run(move |tab| {
            let attempts = attempts.clone();
            async move {
                let n = attempts.fetch_add(1, Ordering::SeqCst) + 1;
                tab.get("data:text/html,<h1>retry</h1>").await?;
                if n < 2 {
                    return Err(drission::Error::msg("注入的首次失败"));
                }
                Ok::<usize, drission::Error>(n)
            }
        })
        .await
    };
    let b_ok =
        r_retry.as_ref().map(|n| *n == 2).unwrap_or(false) && attempts.load(Ordering::SeqCst) == 2;
    println!(
        "[B] 重试: 结果={:?} 总尝试={} (ok={b_ok})",
        r_retry,
        attempts.load(Ordering::SeqCst)
    );

    // ---------- C: 断点续抓 ----------
    let ckpt_path = tmp_path("resume");
    let _ = tokio::fs::remove_file(&ckpt_path).await;
    let ckpt = Checkpoint::load(&ckpt_path).await?;

    let crawl = |i: u32, tab: Tab| async move {
        tab.get("data:text/html,<h1>ok</h1>").await?;
        Ok::<u32, drission::Error>(i)
    };

    // 首跑:0..4 全部完成并落盘。
    let r1 = pool
        .map_resumable(
            (0..4).collect::<Vec<_>>(),
            |i| format!("k{i}"),
            &ckpt,
            crawl,
        )
        .await;
    let first_ok = r1.len() == 4 && r1.iter().all(|(_, r)| r.is_ok());
    let done1 = ckpt.done_count().await;

    // 续跑:0..6,但 0..4 已完成 → 只补 4、5 两项。
    let r2 = pool
        .map_resumable(
            (0..6).collect::<Vec<_>>(),
            |i| format!("k{i}"),
            &ckpt,
            crawl,
        )
        .await;
    let resume_ok = r2.len() == 2 && r2.iter().all(|(_, r)| r.is_ok());
    let done2 = ckpt.done_count().await;
    let c_ok = first_ok && done1 == 4 && resume_ok && done2 == 6;
    println!(
        "[C] 断点续抓: 首跑 {} 项(done={done1}); 续跑只补 {} 项(done={done2}) (ok={c_ok})",
        r1.len(),
        r2.len()
    );
    let _ = tokio::fs::remove_file(&ckpt_path).await;

    let pass = a_ok && b_ok && c_ok;
    println!(
        "\n==== {} ====",
        if pass {
            "ALL CHECKS PASSED"
        } else {
            "SOME CHECKS FAILED"
        }
    );

    pool.shutdown().await?;
    if pass {
        Ok(())
    } else {
        Err(drission::Error::msg("pool_crawl 自验证未通过"))
    }
}
