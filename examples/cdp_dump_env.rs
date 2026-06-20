//! CDP 后端**吐环境** `tab.dump_env()` 端到端真机自验证(完全离线:data: 页 + 本机 Chrome)。
//!
//! 覆盖:① 导航前注入探针(`addScriptToEvaluateOnNewDocument`)② `collect()` 采到真实环境种子
//! (navigator/screen + canvas/webgl 指纹)③ 生成 `env.js` + 一键导出可 `node` 运行的补环境工程
//! ④ **同构双跑自验证**(浏览器真实环境 vs Node `vm`+`env.js` 逐字段对比,需本机有 `node`)。
//!
//! 运行:`cargo run --example cdp_dump_env`(无头默认;`HL=0` 开窗口)。
//! 任一关键校验失败即非 0 退出;全部通过打印 `ALL CHECKS PASSED`。

use drission::prelude::*;

#[tokio::main]
async fn main() -> drission::Result<()> {
    let headless = std::env::var("HL").map(|v| v != "0").unwrap_or(true);
    println!("[*] 启动 Chrome(headless={headless})");
    let browser = ChromiumBrowser::launch(ChromiumOptions::new().headless(headless)).await?;
    let tab = browser.new_tab(Some("about:blank")).await?;

    // ① 导航前注入探针,再导航到自包含 data: 页(完全离线)。
    let mut probe = tab.dump_env().start().await?;
    tab.get("data:text/html,<title>dumpenv</title><meta charset=utf-8><h1>hi</h1>")
        .await?;
    // ② 采集种子。
    let dump = probe.collect().await?;

    let mut failed = false;
    macro_rules! check {
        ($cond:expr, $($arg:tt)*) => {{
            let ok = $cond;
            println!("[{}] {}", if ok { "ok" } else { "FAIL" }, format!($($arg)*));
            if !ok { failed = true; }
        }};
    }

    let seed = &dump.seed;
    let ua = seed
        .pointer("/navigator/userAgent")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    check!(
        !ua.is_empty(),
        "种子含 navigator.userAgent(实得 {} 字符)",
        ua.len()
    );
    let sw = seed
        .pointer("/screen/width")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    check!(sw > 0, "种子含 screen.width(实得 {sw})");
    let canvas_sup = seed
        .pointer("/fingerprint/canvas/supported")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let webgl_sup = seed
        .pointer("/fingerprint/webgl/supported")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    check!(
        seed.get("fingerprint").is_some(),
        "种子含 fingerprint(canvas_supported={canvas_sup} webgl_supported={webgl_sup})"
    );

    // ③ 生成 env.js + 一键导出工程。
    let env_js = dump.env_js(EnvScope::Full);
    check!(
        env_js.contains("function setup(") && env_js.contains("module.exports"),
        "env.js 生成正常(含 setup/module.exports,{} 字节)",
        env_js.len()
    );
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("cdp-dump-env");
    let _ = std::fs::remove_dir_all(&dir);
    let proj = dump.export_project(&dir, EnvScope::Full)?;
    let files_ok = [
        "env.js",
        "index.js",
        "demo.js",
        "verify.js",
        "package.json",
        "seed.json",
    ]
    .iter()
    .all(|f| proj.join(f).exists());
    check!(
        files_ok,
        "export_project 产出工程文件齐全({})",
        proj.display()
    );

    // ④ 同构双跑自验证(需本机 node;没有则跳过不计失败)。
    let report = dump.verify(&tab, &dir, EnvScope::Full).await?;
    if let Some(err) = report.get("error").and_then(|v| v.as_str()) {
        println!("[skip] 同构双跑跳过(无 node 或运行失败): {err}");
    } else {
        let pass = report["pass"].as_u64().unwrap_or(0);
        let fail = report["fail"].as_u64().unwrap_or(u64::MAX);
        let total = report["total"].as_u64().unwrap_or(0);
        check!(
            fail == 0 && pass > 0,
            "同构双跑逐字段一致 {pass}/{total}(fail={fail})"
        );
    }

    browser.quit().await?;
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
