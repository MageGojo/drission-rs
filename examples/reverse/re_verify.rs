//! 真站验证:用 ④ `anti_anti_debug` 实测「反调试强站」(默认 `mh.yichengwlkj.com/pc`)。
//!
//! 对照两阶段(都开 `Debugger` 域 = 等价于「人打开 DevTools 调试」):
//!   Phase A 不 defuse → 站点「无限 debugger」把 `Debugger.paused` 刷爆(次数高、首栈直指反调试代码);
//!   Phase B `tab.anti_anti_debug()` defuse 后 → 暂停次数应骤降到 ~0,且页面照常加载可用。
//!
//! 运行:`cargo run --example re_verify`(无头默认;`HL=0` 有头;`URL=...` 换站;`SEC=8` 改窗口秒数)。

use std::time::Duration;

use drission::prelude::*;

#[tokio::main]
async fn main() -> drission::Result<()> {
    let headless = std::env::var("HL").map(|v| v != "0").unwrap_or(true);
    let url = std::env::var("URL").unwrap_or_else(|_| "https://mh.yichengwlkj.com/pc".to_string());
    let secs: u64 = std::env::var("SEC")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8);
    let window = Duration::from_secs(secs);

    println!("[*] 目标: {url}(headless={headless},窗口 {secs}s)");
    let browser = ChromiumBrowser::launch(ChromiumOptions::new().headless(headless)).await?;
    // 导航不等 load:开了 Debugger 域时,若页面一加载就 `debugger`,等 load 会被卡死;故 None 模式。
    let none = GetOptions::new().load_mode(LoadMode::None);

    // ── Phase A:基线(不 defuse)。开 Debugger 域 → 导航 → 统计暂停 ──
    println!("\n========== Phase A:不 defuse(基线)==========");
    let tab_a = browser.new_tab(Some("about:blank")).await?;
    let dbg_a = tab_a.debugger();
    dbg_a.enable().await?;
    tab_a.get_with(&url, &none).await?;
    let (n_a, bt_a) = dbg_a.count_pauses(window, true).await?;
    println!("[A] {secs}s 内 Debugger.paused 触发 {n_a} 次(数字越高=反调试越凶)");
    if !bt_a.is_empty() {
        println!("[A] 首个暂停的调用栈(直指反调试代码):");
        for l in bt_a.lines().take(6) {
            println!("      {l}");
        }
    }
    tab_a.close().await.ok();

    // ── Phase B:注入式 defuse(tab.anti_anti_debug,导航前)。对直接 setInterval/Function/eval 的 debugger 有效 ──
    println!("\n========== Phase B:tab.anti_anti_debug() 注入式 defuse ==========");
    let tab_b = browser.new_tab(Some("about:blank")).await?;
    tab_b.anti_anti_debug().await?; // 关键:必须在导航前
    let dbg_b = tab_b.debugger();
    dbg_b.enable().await?;
    tab_b.get_with(&url, &none).await?;
    let (n_b, _bt_b) = dbg_b.count_pauses(window, true).await?;
    println!("[B] {secs}s 内 Debugger.paused 触发 {n_b} 次(注入式:拦直接/eval 生成的 debugger)");

    // ── Phase C:CDP 原生通杀 setSkipAllPauses(true)。对计时型/间接/任何 debugger 都生效 ──
    println!(
        "\n========== Phase C:tab.debugger().set_skip_all_pauses(true)(CDP 原生通杀)=========="
    );
    let tab_c = browser.new_tab(Some("about:blank")).await?;
    let dbg_c = tab_c.debugger();
    dbg_c.enable().await?;
    dbg_c.set_skip_all_pauses(true).await?; // 通杀:调试器对一切暂停视而不见
    tab_c.get_with(&url, &none).await?;
    dbg_c.set_skip_all_pauses(true).await?; // 导航会重置该标志 → 导航后再重申一次
    let (n_c, _bt_c) = dbg_c.count_pauses(window, true).await?; // auto_resume 兜底:万一漏一个也不冻结
    println!("[C] {secs}s 内 Debugger.paused 触发 {n_c} 次(应 ~0)");
    let title = tab_c.title().await.unwrap_or_default();
    let href = tab_c.run_js("location.href").await.ok();
    let webdriver = tab_c.run_js("navigator.webdriver").await.ok();
    println!("[C] title={title:?} location={href:?} navigator.webdriver={webdriver:?}");

    // ── 顺带 ②:全新 tab,先导航(不开 Debugger 故 debugger 是 no-op、页面正常加载)→ 再 scripts() ──
    println!("\n========== ② 脚本工具(先导航再 list,list 内部自动 skip 防卡)==========");
    let tab_s = browser.new_tab(Some("about:blank")).await?;
    tab_s.get_with(&url, &none).await?;
    tokio::time::sleep(Duration::from_secs(5)).await; // 无 Debugger 附着,页面正常加载、脚本解析
    let sc = tab_s.scripts();
    let scripts = sc.list().await.unwrap_or_default();
    println!("[S] 解析到 {} 个脚本", scripts.len());
    for kw in [
        "x-ca-sign",
        "checkPerformance",
        "debugger",
        "HmacSHA256",
        "newSign",
    ] {
        let hits = sc.grep(kw).await.unwrap_or_default();
        println!("[S] grep {kw:?} → {} 处命中", hits.len());
        for m in hits.iter().take(2) {
            let frag: String = m.snippet.chars().take(80).collect();
            println!("      {} :{}  {frag}", short(&m.url), m.line_number);
        }
    }

    // ── 结论 ──
    println!("\n========== 结论 ==========");
    println!("[*] Phase A 基线暂停 {n_a} 次;B(注入式){n_b} 次;C(CDP 通杀){n_c} 次。");
    if n_a == 0 {
        println!(
            "ℹ️ 基线未触发(该站可能需交互触发反调试,或对无 DevTools 的 CDP 会话不启用);可 SEC=15 / HL=0 再试。"
        );
    } else {
        if n_b.saturating_mul(2) < n_a {
            println!("✅ 注入式 defuse 对该站直接/eval 型 debugger 有效(B 明显低于 A)。");
        } else {
            println!(
                "➖ 注入式 defuse 对该站收效有限(debugger 藏在间接调用/已解析脚本里,注入改不到)。"
            );
        }
        if n_c == 0 {
            println!(
                "✅ CDP 原生 set_skip_all_pauses 通杀:暂停 {n_a} → 0,页面照常加载(title={title:?}),可干净调试/dump。"
            );
        } else {
            println!("⚠️ Phase C 仍有 {n_c} 次暂停(异常,预期 0)。");
        }
    }

    browser.quit().await?;
    Ok(())
}

fn short(u: &str) -> String {
    u.rsplit('/').next().unwrap_or(u).chars().take(50).collect()
}
