//! 顶象(Dingxiang)滑块:用**库能力**求解缺口。顶象拼图是**跨域 taint 锁死的 `<img>`**(读不到像素),
//! 库走 [`GapMethod::ContentNcc`](drission::prelude::GapMethod)——浏览器级截图拼图 + 绿环掩膜 +
//! 彩色内容 NCC + 暗度门控 + 描边对齐 / 暗度兜底,在**繁杂实拍图 + 同形诱饵 + 重度压暗**上找准缺口。
//!
//! 核心 API(`src/browser/slider.rs`):
//! - [`SliderConfig::dingxiang(i)`](drission::prelude::SliderConfig) 预设(实例后缀 `i`)。
//! - [`Tab::dingxiang_slide_gap(i)`] 纯视觉算位移;[`Tab::solve_slider`] 一把梭(匹配→闭环拖动→判定)。
//!
//! 本例演示**弹出式**(`#btn-popup`):动态识别弹框实例 + 隐藏页面其它验证码,逐张换图、实时画框
//! (红=算法落点、绿=home)验证算法通用性。**目标是缺口找得准 + 通用**,非过顶象(其轨迹/IP 行为
//! 风控会把对齐正确的拖动也弹回,与缺口算法无关)。
//!
//! 运行:`HL=0 cargo run --example dx_slide`(有头);`N=张数`(默认 5)、`NODRAG=1` 只算不拖。

use std::time::Duration;

use drission::prelude::*;
use tokio::time::sleep;

const DEFAULT_URL: &str = "https://cdn.dingxiang-inc.com/ctu-group/captcha-ui/demo/";

#[tokio::main]
async fn main() -> drission::Result<()> {
    let headless = std::env::var("HL").map(|v| v != "0").unwrap_or(true);
    let n: u32 = std::env::var("N")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5);
    let do_drag = std::env::var("NODRAG").is_err();
    let url = std::env::var("URL").unwrap_or_else(|_| DEFAULT_URL.to_string());

    let browser = Browser::launch(BrowserOptions::new().headless(headless)).await?;
    let tab = browser.latest_tab().await?;
    tab.apply_pointer_stealth().await?; // 反检测:导航前补 pointerType
    println!("[*] 打开 {url}(弹出式 #btn-popup)");
    tab.get(&url).await?;
    sleep(Duration::from_secs(3)).await;

    // 弹出式:点 #btn-popup 弹出 → **动态识别**弹框真实实例后缀(页面有多个验证码,写死会认错)
    // → 隐藏页面其它验证码,只留这一个弹框(消除"多个弹窗"干扰)。
    let before = dx_visible_suffixes(&tab).await;
    let _ = tab
        .run_js("(function(){var b=document.querySelector('#btn-popup'); if(b)b.click();})()")
        .await;
    sleep(Duration::from_secs(2)).await;
    let i = dx_new_popup_suffix(&tab, &before).await.unwrap_or(4);
    tab.run_js(&format!(
        "(function(){{document.querySelectorAll('[id^=dx_captcha_basic_wrapper_]').forEach(function(w){{if(!w.id.endsWith('_{i}')){{w.style.display='none';}}}});}})()"
    ))
    .await
    .ok();
    println!("[*] 弹出框实例 = _{i}(已隐藏页面其它验证码)");

    let handle = format!("#dx_captcha_basic_slider_{i}");
    tab.wait().ele_displayed(&handle, None).await?;
    sleep(Duration::from_millis(800)).await;

    // 逐张验证:库的 `dingxiang_slide_gap` 找缺口 → 实时画框 →(可选)`solve_slider` 拖动 → 换图。
    let mut report: Vec<String> = Vec::new();
    for k in 1..=n {
        let _ = tab
            .wait()
            .ele_displayed(&handle, Some(Duration::from_secs(8)))
            .await;
        sleep(Duration::from_millis(450)).await;
        let gap = match tab.dingxiang_slide_gap(i).await {
            Ok(g) => g,
            Err(e) => {
                println!("[!] #{k}:{e}(换图继续)");
                dx_refresh(&tab, i, &handle).await;
                continue;
            }
        };
        println!(
            "[*] #{k}:缺口位移 {:.0}px  方法={:?}  置信={:.2}",
            gap.displace, gap.method, gap.confidence
        );
        // 实时画出算法落点(红=缺口、绿=home),有头直接看红框是否盖住缺口。
        let _ = tab.run_js(&dx_show_box(i, gap.displace)).await;
        if do_drag {
            sleep(Duration::from_millis(700)).await; // 先让看清框
            match tab
                .solve_slider(&SliderConfig::dingxiang(i).max_attempts(1))
                .await
            {
                Ok(r) => {
                    println!(
                        "[*] #{k}:拖动对齐误差 {:.1}px  顶象判定={}",
                        r.align_error,
                        if r.passed {
                            "通过 ✅"
                        } else {
                            "弹回(算法已对齐;弹回属轨迹/IP 行为风控)"
                        }
                    );
                    report.push(format!(
                        "#{k} 位移={:.0}px {:?} 置信={:.2} 对齐={:.1}px",
                        gap.displace, gap.method, gap.confidence, r.align_error
                    ));
                }
                Err(e) => {
                    println!("[!] #{k}:拖动出错({e})");
                    report.push(format!(
                        "#{k} 位移={:.0}px {:?} 置信={:.2}",
                        gap.displace, gap.method, gap.confidence
                    ));
                }
            }
        } else {
            report.push(format!(
                "#{k} 位移={:.0}px {:?} 置信={:.2}",
                gap.displace, gap.method, gap.confidence
            ));
            sleep(Duration::from_millis(1700)).await; // 让有头看清红框
        }
        dx_refresh(&tab, i, &handle).await;
        sleep(Duration::from_millis(700)).await;
    }

    println!("\n==== 顶象弹出式缺口算法验证({n} 张,库 ContentNcc 法)====");
    for r in &report {
        println!("  {r}");
    }
    if !headless {
        sleep(Duration::from_secs(3)).await;
    }
    browser.quit().await?;
    Ok(())
}

/// 当前**可见**的全部滑块实例后缀(页面常有多个验证码:嵌入/浮动/内联/弹出)。
async fn dx_visible_suffixes(tab: &Tab) -> Vec<u32> {
    let s = tab
        .run_js(
            r#"(function(){var o=[];document.querySelectorAll('[id^=dx_captcha_basic_slider_]').forEach(function(e){var m=e.id.match(/_(\d+)$/);var r=e.getBoundingClientRect();if(m&&r.width>0&&r.height>0)o.push(m[1]);});return o.join(',');})()"#,
        )
        .await
        .ok()
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .unwrap_or_default();
    s.split(',').filter_map(|x| x.parse().ok()).collect()
}

/// 点 #btn-popup 后**新出现**的可见滑块即弹出框实例;取不到则退而取非嵌入(_1)的最大可见后缀。
async fn dx_new_popup_suffix(tab: &Tab, before: &[u32]) -> Option<u32> {
    let now = dx_visible_suffixes(tab).await;
    if let Some(s) = now.iter().copied().filter(|x| !before.contains(x)).max() {
        return Some(s);
    }
    now.iter()
        .copied()
        .filter(|&x| x != 1)
        .max()
        .or_else(|| now.iter().copied().max())
}

/// 换一张验证码:**只点本弹框自己**的刷新键 `#dx_captcha_basic_btn-refresh_{i}`(绝不重开弹窗),
/// 轮询等底图指纹变化(换图成功)。
async fn dx_refresh(tab: &Tab, i: u32, handle: &str) {
    let fp_js = format!(
        "(function(){{var c=document.querySelector('#dx_captcha_basic_bg_{i} canvas'); if(!c)return ''; try{{return c.toDataURL('image/png').slice(-64);}}catch(e){{return '';}}}})()"
    );
    let before = tab
        .run_js(&fp_js)
        .await
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default();
    let clicked = tab
        .run_js(&format!(
            "(function(){{var b=document.querySelector('#dx_captcha_basic_btn-refresh_{i}'); if(b&&b.getBoundingClientRect().width>0){{b.click(); return true;}} return false;}})()"
        ))
        .await
        .ok()
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if clicked {
        for _ in 0..16 {
            sleep(Duration::from_millis(300)).await;
            let now = tab
                .run_js(&fp_js)
                .await
                .ok()
                .and_then(|v| v.as_str().map(String::from))
                .unwrap_or_default();
            if !now.is_empty() && now != before {
                break;
            }
        }
    }
    let _ = tab
        .wait()
        .ele_displayed(handle, Some(Duration::from_secs(8)))
        .await;
}

/// 在**实时页面**画出算法落点(红框=检测到的缺口、绿框=home),供有头观看。
/// `disp` 为 CSS 像素位移;缺口视口位置 = 拼图 home 视口位置 + disp。
fn dx_show_box(i: u32, disp: f64) -> String {
    format!(
        r#"(function(){{
  var pe=document.querySelector('#dx_captcha_basic_sub-slider_{i} img'); if(!pe) return false;
  var pr=pe.getBoundingClientRect();
  ['__dxbox','__dxhome'].forEach(function(id){{var e=document.getElementById(id); if(e)e.remove();}});
  var mk=function(id,color,left){{var d=document.createElement('div'); d.id=id;
    d.style.cssText='position:fixed;z-index:2147483647;pointer-events:none;border:3px solid '+color
      +';box-shadow:0 0 0 1px rgba(0,0,0,.6);left:'+left+'px;top:'+pr.top+'px;width:'+pr.width+'px;height:'+pr.height+'px;';
    document.body.appendChild(d);}};
  mk('__dxhome','lime',pr.left);
  mk('__dxbox','red',pr.left+({disp}));
  return true;
}})()"#
    )
}
