//! 吐环境深化 · canvas/webgl/audio 指纹补环境 + 一键导出工程(离线自验证)。
//!
//! 完全离线(`data:` 页,不出网):
//!   1) `tab.dump_env()` 导航前注入探针(已含 Function.prototype.toString 防 hook 检测);
//!   2) 采集**全量种子**——重点是 canvas / webgl / audioContext 指纹;
//!   3) `export_project()` 一键导出**可直接 `node` 运行的补环境工程**(npm 包 + 纯算签名 demo);
//!   4) 双重验证补环境忠实回放浏览器指纹:
//!      - 库内 `verify`:浏览器真实环境 vs Node 补环境沙箱,逐字段(含 canvas/webgl/audio);
//!      - 导出工程自带 `node verify.js`:env.js 回放 vs seed.json 录制值。
//!
//! 末行打印 ALL CHECKS PASSED(任一关键校验失败则进程非 0 退出)。
//!
//! 运行:`cargo run --example dump_env_fingerprint`

use drission::prelude::*;
use serde_json::Value;

#[tokio::main]
async fn main() -> drission::Result<()> {
    tracing_subscriber::fmt().with_env_filter("warn").init();

    let browser = Browser::launch(BrowserOptions::new().headless(true)).await?;
    let tab = browser.latest_tab().await?;

    // 导航前注入吐环境探针(无特定目标参数,只为采集指纹环境)。
    let mut probe = tab.dump_env().start().await?;
    // about:blank 是离线下最可靠、且支持 canvas/webgl/audio 与 init 脚本的文档。
    tab.get("about:blank").await?;
    // 等 OfflineAudioContext 渲染 + 指纹采集就绪。
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    let dump = probe.collect().await?;

    let mut chk = Checker::default();

    // —— 反 hook 检测:探针 hook 了 fetch/XHR,但 Function.prototype.toString 让它们自报 native ——
    let stealth = tab
        .run_js(
            "({fetch: ('' + window.fetch), open: ('' + XMLHttpRequest.prototype.open), ts: ('' + Function.prototype.toString)})",
        )
        .await?;
    let is_native = |k: &str| {
        stealth
            .get(k)
            .and_then(Value::as_str)
            .is_some_and(|s| s.contains("[native code]"))
    };
    println!("==== 反 hook 检测(toString 自报 native) ====");
    println!(
        "  ('' + fetch) = {}",
        stealth["fetch"].as_str().unwrap_or("").replace('\n', " ")
    );
    chk.ok("fetch.toString 显示 [native code]", is_native("fetch"));
    chk.ok("XHR.open.toString 显示 [native code]", is_native("open"));
    chk.ok(
        "Function.prototype.toString 自身也显示 native",
        is_native("ts"),
    );

    // —— 采集到的指纹概览 ——
    let fp = dump.seed.get("fingerprint").cloned().unwrap_or(Value::Null);
    let canvas_ok = fp
        .pointer("/canvas/supported")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let webgl_ok = fp
        .pointer("/webgl/supported")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let audio_ok = fp
        .pointer("/audio/supported")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    println!("==== 采集到的指纹 ====");
    println!(
        "  canvas : supported={canvas_ok}  dataURL.len={}",
        fp.pointer("/canvas/dataURL")
            .and_then(Value::as_str)
            .map(str::len)
            .unwrap_or(0)
    );
    println!(
        "  webgl  : supported={webgl_ok}  vendor={:?}  renderer={:?}  ext={}",
        fp.pointer("/webgl/unmaskedVendor")
            .and_then(Value::as_str)
            .unwrap_or("-"),
        fp.pointer("/webgl/unmaskedRenderer")
            .and_then(Value::as_str)
            .unwrap_or("-"),
        fp.pointer("/webgl/extensions")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0),
    );
    println!(
        "  audio  : supported={audio_ok}  sampleRate={:?}  sum={:?}",
        fp.pointer("/audio/sampleRate")
            .and_then(Value::as_f64)
            .unwrap_or(0.0),
        fp.pointer("/audio/sum")
            .and_then(Value::as_f64)
            .unwrap_or(0.0),
    );

    // 至少要采到 canvas(headless 下 webgl/audio 视构建可能缺失,缺则跳过其校验但不算失败)。
    chk.ok("canvas 指纹已采集", canvas_ok);

    // —— 一键导出补环境工程 ——
    let proj = std::env::current_dir()?.join("dump-env-fp");
    let _ = std::fs::remove_dir_all(&proj);
    dump.export_project(&proj, EnvScope::Full)?;
    println!("\n==== 已导出补环境工程: {} ====", proj.display());
    for f in [
        "env.js",
        "index.js",
        "demo.js",
        "verify.js",
        "package.json",
        "README.md",
        "seed.json",
        "signers.json",
    ] {
        let exists = proj.join(f).exists();
        chk.ok(&format!("工程含 {f}"), exists);
    }
    chk.ok("工程含 signer/ 目录", proj.join("signer").is_dir());

    // —— 验证一:库内同构双跑(浏览器真实环境 vs Node 补环境,含指纹) ——
    let report = dump.verify(&tab, &proj, EnvScope::Full).await?;
    if let Some(err) = report.get("error").and_then(Value::as_str) {
        println!("\n[库内 verify] 跳过(需要 node): {err}");
    } else {
        let pass = report["pass"].as_u64().unwrap_or(0);
        let fail = report["fail"].as_u64().unwrap_or(0);
        let total = report["total"].as_u64().unwrap_or(0);
        println!("\n==== 验证一 · 库内同构双跑: {pass}/{total} 字段一致 ====");
        if fail != 0
            && let Some(arr) = report["fields"].as_array()
        {
            for f in arr.iter().filter(|f| !f["ok"].as_bool().unwrap_or(true)) {
                println!(
                    "    ✗ {} : 浏览器={} | env.js={}",
                    f["field"], f["browser"], f["node"]
                );
            }
        }
        chk.ok("库内 verify 全字段一致", fail == 0 && total >= 5);
        // 指纹字段确实进了对比(被支持时)。
        let has_field = |k: &str| {
            report["fields"]
                .as_array()
                .is_some_and(|a| a.iter().any(|f| f["field"] == k))
        };
        if canvas_ok {
            chk.ok("verify 覆盖 canvas.dataURL", has_field("canvas.dataURL"));
        }
        if webgl_ok {
            chk.ok(
                "verify 覆盖 webgl.unmaskedVendor",
                has_field("webgl.unmaskedVendor"),
            );
        }
        if audio_ok {
            chk.ok("verify 覆盖 audio.sum", has_field("audio.sum"));
        }
    }

    // —— 验证二:导出工程自带 node verify.js(env.js 回放 vs seed.json) ——
    match std::process::Command::new("node")
        .arg("verify.js")
        .current_dir(&proj)
        .output()
    {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            print!("\n==== 验证二 · 导出工程 node verify.js ====\n{stdout}");
            if !out.status.success() {
                eprintln!("{}", String::from_utf8_lossy(&out.stderr));
            }
            chk.ok("工程 verify.js 全部一致", out.status.success());
        }
        Err(e) => println!("\n[工程 verify.js] 跳过(需要 node): {e}"),
    }

    browser.quit().await?;

    println!();
    if chk.failed == 0 {
        println!("ALL CHECKS PASSED ({} 项)", chk.passed);
        Ok(())
    } else {
        eprintln!(
            "FAILED: {} 项未通过 / 共 {}",
            chk.failed,
            chk.passed + chk.failed
        );
        std::process::exit(1);
    }
}

#[derive(Default)]
struct Checker {
    passed: usize,
    failed: usize,
}

impl Checker {
    fn ok(&mut self, name: &str, cond: bool) {
        if cond {
            self.passed += 1;
            println!("  ✓ {name}");
        } else {
            self.failed += 1;
            println!("  ✗ {name}");
        }
    }
}
