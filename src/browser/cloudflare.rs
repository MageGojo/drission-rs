//! Cloudflare 挑战自动通过(对标"过盾"诉求)。
//!
//! 两类挑战:
//! - **非交互式(JS challenge)**:浏览器指纹/环境过关后**自动放行**(标题从 "Just a moment..."
//!   变成真实页),我们只需轮询等待。
//! - **交互式(Turnstile 复选框)**:页面里嵌一个 `challenges.cloudflare.com` 的 iframe,
//!   左侧有"确认你是真人"复选框,**必须点一下**。复选框在跨域 iframe(还套 shadow DOM)里,
//!   页面 JS 读不到内容,但能读到 **iframe 元素的位置**;于是用**可信鼠标事件**
//!   (`Page.dispatchMouseEvent`,`isTrusted=true`)按坐标点它的复选框处即可。
//!
//! 关键点:点击必须是**可信**事件(我们的 `tab.actions()` 走的就是 `dispatchMouseEvent`,
//! 实测 `isTrusted=true`),且鼠标移动带拟人轨迹;配合本库默认的干净指纹(`webdriver=false`、
//! 屏幕自洽、`block_webrtc` 等),交互式 Turnstile 一般点一下即过。

use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::time::sleep;

use crate::Result;
use crate::browser::tab::Tab;

/// 探测当前是否处于 CF 挑战、以及(若有)Turnstile iframe 的视口位置。
/// 返回 `JSON.stringify` 的字符串(由 Rust 侧解析)。
const PROBE_JS: &str = r#"(function () {
  var t = (document.title || '').toLowerCase();
  var titleChallenge = t.indexOf('just a moment') >= 0
    || t.indexOf('attention required') >= 0
    || t.indexOf('checking your browser') >= 0
    || t.indexOf('verifying') >= 0
    || t.indexOf('请稍候') >= 0;
  var optChallenge = (typeof window._cf_chl_opt !== 'undefined');
  var box = null;
  var ifr = document.querySelector(
    'iframe[src*="challenges.cloudflare.com"], iframe[id^="cf-chl-widget"], iframe[title*="Cloudflare"], iframe[title*="challenge"], iframe[title*="验证"]'
  );
  if (ifr) {
    var r = ifr.getBoundingClientRect();
    if (r.width > 0 && r.height > 0) {
      box = { x: r.x, y: r.y, w: r.width, h: r.height };
    }
  }
  var challenge = titleChallenge || optChallenge || box !== null;
  return JSON.stringify({ challenge: challenge, box: box, title: document.title });
})()"#;

/// 列出页面上所有 iframe + 关键标记,供示例/诊断落盘(点歪了能据此调偏移)。
const DEBUG_JS: &str = r#"(function () {
  var ifrs = [];
  var list = document.querySelectorAll('iframe');
  for (var i = 0; i < list.length; i++) {
    var f = list[i];
    var r = f.getBoundingClientRect();
    ifrs.push({
      src: (f.getAttribute('src') || '').slice(0, 100),
      id: f.id || '',
      title: f.title || '',
      x: r.x, y: r.y, w: r.width, h: r.height
    });
  }
  return JSON.stringify({
    title: document.title,
    iframes: ifrs,
    hasChallengeStage: !!document.querySelector('#challenge-stage, #challenge-form, .cf-turnstile, #turnstile-wrapper'),
    cfOpt: (typeof window._cf_chl_opt !== 'undefined')
  });
})()"#;

/// 一次 CF 状态快照。
struct CfState {
    challenge: bool,
    /// Turnstile 复选框的视口坐标(已算好偏移),没有交互式控件则为 `None`。
    checkbox: Option<(f64, f64)>,
}

impl Tab {
    /// 自动通过 Cloudflare 挑战(默认 30s 超时)。见 [`pass_cloudflare`](Self::pass_cloudflare)。
    pub async fn pass_cloudflare_default(&self) -> Result<bool> {
        self.pass_cloudflare(Duration::from_secs(30)).await
    }

    /// 在 `timeout` 内尝试通过 Cloudflare 挑战:
    /// - 已经不是挑战页 → 立即返回 `true`;
    /// - 发现 Turnstile 复选框 → 拟人移动 + **可信点击**,等待放行;
    /// - 只有非交互式挑战(无复选框)→ 轮询等待其自动放行。
    ///
    /// 返回是否在超时内通过。**非破坏**:不是挑战页时等价于一次轻量探测后立即返回。
    pub async fn pass_cloudflare(&self, timeout: Duration) -> Result<bool> {
        let deadline = Instant::now() + timeout;
        let mut clicked = 0u32;
        loop {
            let st = self.cf_state().await?;
            if !st.challenge {
                return Ok(true);
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            match st.checkbox {
                Some((cx, cy)) => {
                    tracing::debug!(x = cx, y = cy, "点击 Cloudflare Turnstile 复选框");
                    // 拟人移动到复选框 + 短暂停顿 + 可信单击。
                    self.actions()
                        .move_to(cx, cy, 0.6)
                        .wait(0.2)
                        .click()
                        .perform()
                        .await?;
                    clicked += 1;
                    // 给 CF 处理时间(交互式校验有网络往返)。
                    sleep(Duration::from_millis(2500)).await;
                }
                None => {
                    // 非交互式:等它自动放行。
                    sleep(Duration::from_millis(1000)).await;
                }
            }
            // 防御:点了很多次仍不过,缩短无谓等待(仍受 deadline 兜底)。
            if clicked > 8 {
                sleep(Duration::from_millis(500)).await;
            }
        }
    }

    /// 当前是否处于 Cloudflare 挑战页(轻量探测,不点击)。
    pub async fn is_cloudflare(&self) -> Result<bool> {
        Ok(self.cf_state().await?.challenge)
    }

    /// CF 诊断信息(页面所有 iframe + 关键标记),用于排查"点歪/没点"。
    pub async fn cloudflare_debug(&self) -> Result<Value> {
        let v = self.run_js(DEBUG_JS).await?;
        Ok(parse_probe(v))
    }

    /// 探测一次 CF 状态,并把 Turnstile iframe 折算成"复选框坐标"。
    async fn cf_state(&self) -> Result<CfState> {
        let v = self.run_js(PROBE_JS).await?;
        let probe = parse_probe(v);
        let challenge = probe
            .get("challenge")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let checkbox = probe.get("box").filter(|b| b.is_object()).map(|b| {
            let x = b.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let y = b.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let w = b.get("w").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let h = b.get("h").and_then(|v| v.as_f64()).unwrap_or(0.0);
            // Turnstile 复选框在 widget 左侧、垂直居中。
            let cx = x + 30.0_f64.min(w / 2.0).max(8.0);
            let cy = y + h / 2.0;
            (cx, cy)
        });
        Ok(CfState { challenge, checkbox })
    }
}

/// `run_js` 多数情况下返回 `JSON.stringify` 的字符串;为稳妥也兼容已是对象的返回。
fn parse_probe(v: Value) -> Value {
    match v.as_str() {
        Some(s) => serde_json::from_str(s).unwrap_or(Value::Null),
        None => v,
    }
}
