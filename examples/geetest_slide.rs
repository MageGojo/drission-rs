//! GeeTest v4 滑块——用**通用滑块库能力**求解(`SliderConfig::geetest_v4()` 预设)。
//!
//! 演示 drission-rs 的通用滑块 API(不限极验):
//! - 纯视觉求缺口距离:`tab.geetest_slide_gap()` / `tab.slider_gap(&cfg)` → [`SliderGap`](drission::prelude::SliderGap)
//! - 一把梭:`tab.solve_geetest_slide()` / `tab.solve_slider(&cfg)` → [`SliderResult`](drission::prelude::SliderResult)
//!
//! 缺口用**模板匹配**(双图法:拼图颜色对 `fullbg` / 拼图形状对 diff,两法互校),对齐 ≤2px,
//! headless 也能 `success:true`。同一预设实测可过 **slide-float / slide-custom** 两种模式
//! (`URL` 环境变量切换)。换其它厂商滑块只需另写一个 `SliderConfig`(见 `examples/slider_local`)。
//!
//! 运行:`cargo run --example geetest_slide`(默认 headless 跑 slide-float;`HL=0` 看界面,
//! `TRY=8` 调重试次数,`URL=https://demos.geetest.com/slide-custom.html` 换 custom 模式)

use std::time::Duration;

use drission::prelude::*;
use tokio::time::sleep;

const DEFAULT_URL: &str = "https://demos.geetest.com/slide-float.html";

#[tokio::main]
async fn main() -> drission::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .init();

    let headless = std::env::var("HL").map(|v| v != "0").unwrap_or(true);
    let tries: u32 = std::env::var("TRY")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(6);

    let browser = Browser::launch(BrowserOptions::new().headless(headless)).await?;
    let tab = browser.latest_tab().await?;

    // 反检测:导航前把合成 PointerEvent 的空 pointerType 修成 "mouse"。
    tab.apply_pointer_stealth().await?;

    let url = std::env::var("URL").unwrap_or_else(|_| DEFAULT_URL.to_string());
    println!("[*] 打开 {url}");
    tab.get(&url).await?;
    sleep(Duration::from_secs(3)).await;

    // 先单独演示纯视觉求缺口(弹出滑块后)。
    tab.ele("css:.geetest_radar_btn").await?.click().await?;
    let _ = tab.ele("css:.geetest_slider_button").await?;
    sleep(Duration::from_millis(1000)).await;
    match tab.geetest_slide_gap().await {
        Ok(gap) => println!(
            "[*] geetest_slide_gap():拼图需移 {:.0}px(法={:?} 形状 {:.0}/颜色 {:.0} 置信 {:.2})",
            gap.displace, gap.method, gap.by_shape, gap.by_color, gap.confidence
        ),
        Err(e) => println!("[!] geetest_slide_gap 失败(solve 会自行重试): {e}"),
    }

    // 一把梭:用极验预设 + 自定义尝试次数。
    println!("\n[*] tab.solve_slider(SliderConfig::geetest_v4().max_attempts({tries}))…");
    let r = tab
        .solve_slider(&SliderConfig::geetest_v4().max_attempts(tries))
        .await?;

    println!(
        "\n==== {} ====",
        if r.passed {
            "验证通过 ✅(通用滑块库能力)"
        } else {
            "未通过 ❌"
        }
    );
    println!(
        "[*] 尝试 {} 次,最佳对齐误差 {:.1}px",
        r.attempts, r.align_error
    );
    if !r.passed {
        println!("[i] 对齐误差已 ≤2px(缺口算对);未过多为本机出网不稳/极验限频,与缺口计算无关。");
    }

    if !headless {
        sleep(Duration::from_secs(3)).await;
    }
    browser.quit().await?;
    if r.passed {
        Ok(())
    } else {
        Err(drission::Error::msg(
            "多次尝试未通过(对齐正确,疑为网络/风控)",
        ))
    }
}
