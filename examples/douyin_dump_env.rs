//! 通用吐环境能力示例(以抖音 a_bogus 为目标参数)。
//!
//! 用 `tab.dump_env()` 通用 API:**指定**目标参数(query `a_bogus`)+ 限定请求(detail),导航前注入探针,
//! 安全模式采集**全量真实种子**(吐全),用 related→直链稳定抓到每个视频各自签名的 detail;全部导出到
//! 当前目录 `./dump-env/`,含可被 Node 直接 `require` 的补环境 `env.js`,并**同构双跑自验证**。
//!
//! 设环境变量 `DUMP_PROXY=1` 开【诊断模式】:Proxy 追踪算法实际读取的环境路径(access.json),
//! 据此额外吐**只含关键字段**的精简补环境 `env.accessed.js`(注意:对抖音强检测会干扰签名)。
//!
//! 运行:`cargo run --example douyin_dump_env --no-default-features --features camoufox [-- <短链/视频URL> <数量>]`
//!      `DUMP_PROXY=1 cargo run --example douyin_dump_env --no-default-features --features camoufox`  # 诊断模式(只吐关键环境)

use std::collections::{HashSet, VecDeque};
use std::time::Duration;

use drission::prelude::*;
use serde_json::Value;

#[tokio::main]
async fn main() -> drission::Result<()> {
    tracing_subscriber::fmt().with_env_filter("warn").init();
    let mut a = std::env::args().skip(1);
    let start = a
        .next()
        .unwrap_or_else(|| "https://v.douyin.com/I1mlU0fBFhI/".into());
    let want: usize = a.next().and_then(|s| s.parse().ok()).unwrap_or(3);
    let proxy_on = std::env::var("DUMP_PROXY").is_ok();

    let browser = Browser::launch(
        BrowserOptions::new()
            .headless(true)
            .locale("zh-CN")
            .timezone("Asia/Shanghai"),
    )
    .await?;
    let tab = browser.latest_tab().await?;

    // —— 通用吐环境:目标 = query 参数 a_bogus,只针对 detail 请求;proxy 诊断可选(强检测站点默认关)。
    let mut probe = tab
        .dump_env()
        .target_query("a_bogus")
        .match_url("aweme/v1/web/aweme/detail")
        .proxy(proxy_on)
        .start()
        .await?;

    // 业务监听:detail(各视频签名) + related(取下一批 id)。探针已在 start() 导航前注入。
    tab.listen_xhr(&["aweme/v1/web/aweme/detail", "aweme/v1/web/aweme/related"])
        .await?;
    tab.get(&start).await?;
    let stream = tab.listen_stream().await?;
    tab.press_key("ArrowDown").await?; // 触发当前视频的 related 预取

    let mut queue: VecDeque<String> = VecDeque::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut got = 0usize;

    let mut rounds = 0usize;
    while got < want && rounds < 40 {
        rounds += 1;
        tokio::time::sleep(Duration::from_millis(1500)).await;
        let _ = probe.collect().await?; // 每轮累积 seed/access/sinks/targets(导航会重置页面探针)
        for p in stream.drain_ready().await {
            if p.url_has("aweme/detail") {
                if let Some(id) = p.query("aweme_id").filter(|id| seen.insert(id.clone())) {
                    got += 1;
                    let ab = p.query("a_bogus").unwrap_or_default();
                    println!("#{got} 视频 {id}  a_bogus={}", short(&ab));
                    probe.record_hit("query", "a_bogus", &ab, &p.url); // 真实上线值入账
                    if got >= want {
                        break;
                    }
                }
            } else if p.url_has("aweme/related") {
                queue.extend(related_ids(&p).into_iter().filter(|id| !seen.contains(id)));
            }
        }
        // 打开下一个未抓过的视频直链(触发它自己签名的 detail)。
        while let Some(id) = queue.pop_front() {
            if !seen.contains(&id) {
                tab.get(&format!("https://www.douyin.com/video/{id}"))
                    .await?;
                tab.press_key("ArrowDown").await?;
                break;
            }
        }
    }

    if got < want {
        println!("⚠ 仅抓到 {got}/{want} 个(网络慢或被检测),用现有数据继续吐环境。");
    }
    let dump = probe.collect().await?;

    // 导出到当前目录 ./dump-env/。
    let out = std::env::current_dir()?.join("dump-env");
    dump.write_to(&out)?;

    println!("\n==== 吐环境完成,产物目录: {} ====", out.display());
    println!(
        "  seed.json            环境种子(吐全:navigator/screen/document/storage + canvas/webgl/audio 指纹)"
    );
    println!("  env.js               Node 补环境(含指纹回放;require 即挂全局或 vm 沙箱 setup)");
    println!(
        "  targets.json         命中目标参数 {} 条(a_bogus 真实上线值 + 调用栈)",
        dump.targets.len()
    );
    println!(
        "  sinks.json           签名请求 writer(URL + 调用栈) {} 条",
        dump.sinks.len()
    );

    // 签名脚本通用化定位(从调用栈自动定位,任意站点通用)。
    let signers = dump.signers();
    println!("  signers.json         签名脚本定位 {} 个:", signers.len());
    for s in signers.iter().take(5) {
        println!(
            "      {} (:{}:{}) x{}",
            s["url"].as_str().unwrap_or("-"),
            s["line"],
            s["col"],
            s["count"]
        );
    }

    // —— 一键导出可直接 node 运行的补环境工程(npm 包 + 纯算签名 demo) ——
    let proj = std::env::current_dir()?.join("douyin-env");
    dump.export_project(&proj, EnvScope::Full)?;
    println!("\n==== 已导出补环境工程: {} ====", proj.display());
    println!(
        "  cd {} && node verify.js   # 验证 env.js 回放;node demo.js  # 纯算签名(先把 signer 下到 signer/)",
        proj.display()
    );
    if dump.has_access() {
        let n = dump.access["order"].as_array().map(Vec::len).unwrap_or(0);
        println!("  access.json          访问路径 {n} 项(诊断模式)");
        println!("  env.accessed.js      只吐关键环境(按访问路径从种子裁剪)");
    } else {
        println!(
            "  access.json          (空)—— 设 DUMP_PROXY=1 才追踪访问路径并吐 env.accessed.js"
        );
    }

    // ===== 自验证:吐的 env.js 是否忠实还原浏览器(同构双跑逐字段对比) =====
    let report = dump.verify(&tab, &out, EnvScope::Full).await?;
    std::fs::write(
        out.join("verify-report.json"),
        serde_json::to_string_pretty(&report)?,
    )?;
    print_verify(&report);

    browser.quit().await?;
    Ok(())
}

/// 业务:从 `aweme/related` 响应体取下一批视频 id。
fn related_ids(p: &DataPacket) -> Vec<String> {
    p.json()
        .and_then(|v| {
            v.get("aweme_list").and_then(|a| a.as_array()).map(|arr| {
                arr.iter()
                    .filter_map(|x| x["aweme_id"].as_str().map(String::from))
                    .collect()
            })
        })
        .unwrap_or_default()
}

fn short(s: &str) -> String {
    if s.chars().count() > 40 {
        format!("{}…", s.chars().take(40).collect::<String>())
    } else {
        s.to_string()
    }
}

fn print_verify(report: &Value) {
    if let Some(err) = report.get("error").and_then(Value::as_str) {
        println!("\n[环境验证] 跳过(需要 node): {err}");
        return;
    }
    let pass = report["pass"].as_u64().unwrap_or(0);
    let fail = report["fail"].as_u64().unwrap_or(0);
    let total = report["total"].as_u64().unwrap_or(0);
    println!("\n==== 环境验证(浏览器真实环境 vs 吐的 env.js): {pass}/{total} 字段一致 ====");
    if fail == 0 {
        println!("  ✅ 全部一致——吐的环境忠实还原了浏览器(详见 verify-report.json)。");
    } else if let Some(arr) = report["fields"].as_array() {
        println!("  ⚠ {fail} 个字段不一致:");
        for f in arr.iter().filter(|f| !f["ok"].as_bool().unwrap_or(true)) {
            println!(
                "    {} : 浏览器={} | env.js={}",
                f["field"].as_str().unwrap_or(""),
                f["browser"],
                f["node"]
            );
        }
    }
}
