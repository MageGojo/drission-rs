//! 用**第三方指纹库 + 自算复合哈希**证明「每浏览器指纹已更换」(CDP 后端)。
//!
//! 对 N 个不同指纹画像([`CdpFingerprintPool`])各起一个浏览器,在真实页面里:
//! - 加载业界标准 **FingerprintJS v4**(`openfpcdn.io`)取 `visitorId` —— 第三方库给出的指纹 ID;
//! - 自算 **canvas / webgl / audio** 复合哈希(指纹站点同款信号),交叉印证;
//! - dump UA/platform/屏幕/时区/语言/硬件。
//!
//! 并把**画像 #1 复跑一次**(同一份指纹应得**相同** visitorId)——证明指纹「跨身份各异、同身份稳定」:
//! 随机乱跳的指纹本身就是机器人信号,稳定可复现才是真换指纹。
//!
//! 判定:不同画像的 `visitorId` 应**两两不同**;`#1` 与 `#1-复跑` 应**相同**。
//!
//! 运行:
//! - `cargo run --example fp_verify`                       默认**同 OS 变体**(保真 UA/WebGL,只变屏幕/时区/语言/硬件/canvas 噪声;换指纹**不撒谎**、Turnstile 友好)3 个 + 稳定性复跑
//! - `PERSONA=1 cargo run --example fp_verify`             跨 OS 画像(伪装 UA/platform/WebGL,差异最大;**建议配对应地区代理**,否则一致性检测站会标"撒谎")
//! - `N=4 cargo run --example fp_verify`                   4 个
//! - `HEADFUL=1 cargo run --example fp_verify`             有头可视
//! - `URL=https://example.com cargo run --example fp_verify`  自定义承载页(需 https 真源,供动态 import FingerprintJS)

use std::time::Duration;

use drission::Result;
use drission::cdp::{CdpFingerprint, CdpFingerprintPool, ChromiumBrowser, ChromiumOptions};
use serde_json::Value;

/// 页面内:自算 canvas/webgl/audio 复合哈希 + 加载 FingerprintJS 取 visitorId,返回 JSON 字符串。
/// `run_js` 走 `awaitPromise:true`,故这里返回的 Promise 会被自动 await。
const FP_JS: &str = r#"(async () => {
  const sha = async (s) => {
    const b = new TextEncoder().encode(s);
    const h = await crypto.subtle.digest('SHA-256', b);
    return [...new Uint8Array(h)].map(x => ('0' + x.toString(16)).slice(-2)).join('').slice(0, 16);
  };
  const out = {
    ua: navigator.userAgent,
    platform: navigator.platform,
    screen: screen.width + 'x' + screen.height + '@' + (window.devicePixelRatio || 1),
    tz: (Intl.DateTimeFormat().resolvedOptions() || {}).timeZone,
    langs: (navigator.languages || []).join(','),
    hc: navigator.hardwareConcurrency,
    dm: navigator.deviceMemory,
    webgl: '', canvas: '', audio: '', visitorId: ''
  };
  // canvas 信号(我们对它注入了每画像确定性微噪声)。
  try {
    const c = document.createElement('canvas'); c.width = 260; c.height = 64;
    const x = c.getContext('2d');
    x.textBaseline = 'top'; x.font = "16px 'Arial'";
    x.fillStyle = '#f60'; x.fillRect(10, 1, 90, 40);
    x.fillStyle = '#069'; x.fillText('drission-fp-😀,0', 2, 15);
    x.fillStyle = 'rgba(102,200,0,0.7)'; x.fillText('verify-指纹', 4, 35);
    out.canvas = await sha(c.toDataURL());
  } catch (e) { out.canvas = 'err'; }
  // WebGL renderer(persona 模式伪装它)。
  try {
    const g = document.createElement('canvas').getContext('webgl')
      || document.createElement('canvas').getContext('experimental-webgl');
    const d = g.getExtension('WEBGL_debug_renderer_info');
    out.webgl = d ? g.getParameter(d.UNMASKED_RENDERER_WEBGL) : g.getParameter(g.RENDERER);
  } catch (e) { out.webgl = 'err'; }
  // audio 信号(OfflineAudioContext 渲染特征)。
  try {
    out.audio = await new Promise((res) => {
      const Ctx = window.OfflineAudioContext || window.webkitOfflineAudioContext;
      const ctx = new Ctx(1, 5000, 44100);
      const osc = ctx.createOscillator(); osc.type = 'triangle'; osc.frequency.value = 10000;
      const comp = ctx.createDynamicsCompressor();
      osc.connect(comp); comp.connect(ctx.destination); osc.start(0); ctx.startRendering();
      ctx.oncomplete = (e) => {
        const d = e.renderedBuffer.getChannelData(0);
        let s = 0; for (let i = 4500; i < 5000; i++) s += Math.abs(d[i]);
        res(s.toString().slice(0, 14));
      };
    });
  } catch (e) { out.audio = 'err'; }
  // 第三方指纹库:FingerprintJS v4 visitorId(动态 import,需 https 真源页面)。
  try {
    const FP = await import('https://openfpcdn.io/fingerprintjs/v4');
    const agent = await FP.load();
    const r = await agent.get();
    out.visitorId = r.visitorId;
  } catch (e) { out.visitorId = 'ERR:' + (e && e.message ? e.message : e); }
  return JSON.stringify(out);
})()"#;

struct Row {
    label: String,
    fp: Value,
}

#[tokio::main]
async fn main() -> Result<()> {
    let n: usize = std::env::var("N")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(3);
    let headful = std::env::var("HEADFUL").as_deref() == Ok("1");
    // 默认**同 OS 变体**(保真 UA/WebGL,换指纹不被识破、Turnstile 友好);PERSONA=1 才用跨 OS 画像(伪装,配代理用)。
    let persona = std::env::var("PERSONA").as_deref() == Ok("1");
    let url = std::env::var("URL").unwrap_or_else(|_| "https://example.com/".to_string());

    let pool = if persona {
        CdpFingerprintPool::personas(n)
    } else {
        CdpFingerprintPool::generate(n)
    };
    // 任务列表:N 份不同画像 + 把 #1 再复跑一次(稳定性校验)。
    let mut jobs: Vec<(String, CdpFingerprint)> = pool
        .profiles()
        .iter()
        .enumerate()
        .map(|(i, fp)| (format!("#{}", i + 1), fp.clone()))
        .collect();
    if let Some(first) = pool.profiles().first() {
        jobs.push(("#1-复跑".to_string(), first.clone()));
    }

    println!(
        "模式: {} | 浏览器数: {}(+1 复跑) | headless: {} | 承载页: {}\n第三方库: FingerprintJS v4 (openfpcdn.io)\n",
        if persona {
            "跨 OS 画像(伪装 UA/platform/WebGL)"
        } else {
            "同 OS 变体(保真 UA/WebGL,Turnstile 友好)"
        },
        n,
        !headful,
        url
    );

    let base = ChromiumOptions::new().headless(!headful);
    let mut rows: Vec<Row> = Vec::new();
    for (label, fp) in &jobs {
        let opts = fp.apply_to_options(base.clone());
        let browser = ChromiumBrowser::launch(opts).await?;
        let tab = browser.new_tab(Some("about:blank")).await?;
        tab.get(&url).await?;
        // 给 FingerprintJS 充足时间(动态 import + 各信号采集)。
        let raw = tab.run_js(FP_JS).await?;
        let v: Value = match raw.as_str() {
            Some(s) => serde_json::from_str(s).unwrap_or(Value::Null),
            None => raw,
        };
        print_one(label, &v);
        rows.push(Row {
            label: label.clone(),
            fp: v,
        });
        let _ = browser.quit().await;
        // 浏览器间稍等,降低同机并发对网络/句柄的压力。
        tokio::time::sleep(Duration::from_millis(300)).await;
    }

    verdict(&rows);
    Ok(())
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("?")
}

fn print_one(label: &str, v: &Value) {
    println!("── 浏览器 {label} ───────────────────────────────");
    println!("  visitorId: {}", s(v, "visitorId"));
    println!("  canvas#  : {}", s(v, "canvas"));
    println!("  webgl    : {}", s(v, "webgl"));
    println!("  audio#   : {}", s(v, "audio"));
    println!("  screen   : {}", s(v, "screen"));
    println!("  timezone : {}", s(v, "tz"));
    println!("  langs    : {}", s(v, "langs"));
    println!(
        "  hw/mem   : {} cores / {} GB",
        v.get("hc").unwrap_or(&Value::Null),
        v.get("dm").unwrap_or(&Value::Null)
    );
    println!("  UA       : {}", s(v, "ua"));
    println!("  platform : {}", s(v, "platform"));
    println!();
}

/// 判定:不同画像 visitorId 两两不同 + #1 与 #1-复跑 相同。
fn verdict(rows: &[Row]) {
    println!("════════════════ 判定 ════════════════");
    let valid: Vec<&Row> = rows
        .iter()
        .filter(|r| {
            let id = s(&r.fp, "visitorId");
            !id.is_empty() && !id.starts_with("ERR")
        })
        .collect();
    if valid
        .iter()
        .any(|r| s(&r.fp, "visitorId").starts_with("ERR"))
    {
        println!(
            "⚠ 有浏览器未取到 FingerprintJS visitorId(看上面的 ERR:…);canvas/webgl 哈希仍可作证。"
        );
    }

    // 不同画像(排除复跑)两两不同?
    let distinct: Vec<&Row> = rows.iter().filter(|r| r.label != "#1-复跑").collect();
    let ids: Vec<&str> = distinct
        .iter()
        .map(|r| s(&r.fp, "visitorId"))
        .filter(|i| !i.is_empty() && !i.starts_with("ERR"))
        .collect();
    let mut uniq = ids.clone();
    uniq.sort_unstable();
    uniq.dedup();
    let all_diff = !ids.is_empty() && uniq.len() == ids.len();
    println!(
        "① 各浏览器 visitorId 两两不同 : {}  ({} 个有效 → {} 个唯一)",
        if all_diff {
            "✅ 是(指纹确实换了)"
        } else {
            "❌ 否"
        },
        ids.len(),
        uniq.len()
    );

    // canvas 也两两不同?(我们直接改它,最能说明问题)
    let cv: Vec<&str> = distinct
        .iter()
        .map(|r| s(&r.fp, "canvas"))
        .filter(|c| *c != "?" && *c != "err")
        .collect();
    let mut cu = cv.clone();
    cu.sort_unstable();
    cu.dedup();
    println!(
        "② 各浏览器 canvas 哈希两两不同: {}  ({} 个 → {} 个唯一)",
        if !cv.is_empty() && cu.len() == cv.len() {
            "✅ 是"
        } else {
            "❌ 否"
        },
        cv.len(),
        cu.len()
    );

    // 稳定性:#1 与 #1-复跑 的 visitorId 相同?
    let first = rows.iter().find(|r| r.label == "#1");
    let rerun = rows.iter().find(|r| r.label == "#1-复跑");
    if let (Some(a), Some(b)) = (first, rerun) {
        let (ia, ib) = (s(&a.fp, "visitorId"), s(&b.fp, "visitorId"));
        let same = !ia.is_empty() && !ia.starts_with("ERR") && ia == ib;
        println!(
            "③ 同一画像复跑 visitorId 稳定 : {}  (#1={} / 复跑={})",
            if same {
                "✅ 是(同身份可复现,非乱跳)"
            } else {
                "❌ 否"
            },
            short(ia),
            short(ib)
        );
    }
    println!("\n第三方可视化验证(用同一份指纹手动打开看):");
    println!("  · https://abrahamjuliot.github.io/creepjs/   指纹哈希 + 信任分 + 撒谎检测(最权威)");
    println!("  · https://browserleaks.com/canvas            canvas/webgl 签名哈希");
    println!("  · https://fingerprint.com/products/...        商用 botD demo");
    println!("  · https://amiunique.org / https://pixelscan.net  唯一度 / 一致性");
}

fn short(s: &str) -> String {
    if s.len() > 12 {
        format!("{}…", &s[..12])
    } else {
        s.to_string()
    }
}
