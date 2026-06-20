//! Cloudflare 挑战自动通过(CDP 后端,对标 Camoufox 后端的 `Tab::pass_cloudflare`)。
//!
//! 两类挑战:
//! - **非交互式(JS challenge)**:浏览器指纹/环境过关后**自动放行**,只需轮询等待标题/标记消失;
//! - **交互式(Turnstile 复选框)**:页面嵌一个 `challenges.cloudflare.com` 的跨域 iframe,
//!   内容读不到但能读 iframe 的**视口位置**;于是用 CDP `Input.dispatchMouseEvent`(`isTrusted=true`)
//!   按坐标点它左侧的复选框处即可。
//!
//! 实现只用 [`ChromiumTab`] 的公开方法(`run_js` + `mouse_move`/`mouse_down`/`mouse_up`),
//! 逻辑与 [`crate::browser::cloudflare`](Camoufox 后端)保持一致。

use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::time::sleep;

use crate::Result;
use crate::cdp::ChromiumTab;

/// 探测当前是否处于 CF 挑战、以及(若有)Turnstile iframe 的视口位置。返回 `JSON.stringify` 字符串。
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

impl ChromiumTab {
    /// 自动通过 Cloudflare 挑战(默认 30s 超时)。见 [`pass_cloudflare`](Self::pass_cloudflare)。
    pub async fn pass_cloudflare_default(&self) -> Result<bool> {
        self.pass_cloudflare(Duration::from_secs(30)).await
    }

    /// 在 `timeout` 内尝试通过 Cloudflare 挑战:
    /// - 已不是挑战页 → 立即返回 `true`;
    /// - 发现 Turnstile 复选框 → 拟人移动 + **可信点击**,等待放行;
    /// - 只有非交互式挑战(无复选框)→ 轮询等待其自动放行。
    ///
    /// 返回是否在超时内通过。**非破坏**:不是挑战页时等价一次轻量探测后立即返回。
    pub async fn pass_cloudflare(&self, timeout: Duration) -> Result<bool> {
        let deadline = Instant::now() + timeout;
        let mut clicked = 0u32;
        loop {
            let (challenge, checkbox) = self.cf_state().await?;
            if !challenge {
                return Ok(true);
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            match checkbox {
                Some((cx, cy)) => {
                    tracing::debug!(x = cx, y = cy, "点击 Cloudflare Turnstile 复选框(CDP)");
                    self.trusted_click(cx, cy).await?;
                    clicked += 1;
                    // 给 CF 处理时间(交互式校验有网络往返)。
                    sleep(Duration::from_millis(2500)).await;
                }
                None => {
                    // 非交互式:等它自动放行。
                    sleep(Duration::from_millis(1000)).await;
                }
            }
            if clicked > 8 {
                sleep(Duration::from_millis(500)).await;
            }
        }
    }

    /// 当前是否处于 Cloudflare 挑战页(轻量探测,不点击)。
    pub async fn is_cloudflare(&self) -> Result<bool> {
        Ok(self.cf_state().await?.0)
    }

    /// 探测一次 CF 状态,并把 Turnstile iframe 折算成"复选框坐标"。
    /// 返回 `(是否挑战页, 复选框视口坐标)`。
    async fn cf_state(&self) -> Result<(bool, Option<(f64, f64)>)> {
        let probe = parse_probe(self.run_js(PROBE_JS).await?);
        let challenge = probe
            .get("challenge")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let checkbox = probe.get("box").filter(|b| b.is_object()).map(|b| {
            let x = b.get("x").and_then(Value::as_f64).unwrap_or(0.0);
            let y = b.get("y").and_then(Value::as_f64).unwrap_or(0.0);
            let w = b.get("w").and_then(Value::as_f64).unwrap_or(0.0);
            let h = b.get("h").and_then(Value::as_f64).unwrap_or(0.0);
            // Turnstile 复选框在 widget 左侧、垂直居中。
            let cx = x + 30.0_f64.min(w / 2.0).max(8.0);
            let cy = y + h / 2.0;
            (cx, cy)
        });
        Ok((challenge, checkbox))
    }

    /// 拟人移动到 `(x, y)` 后做一次可信单击(CDP `Input.dispatchMouseEvent`)。
    async fn trusted_click(&self, x: f64, y: f64) -> Result<()> {
        // 分几步移动,带轻微抖动,贴近真人轨迹。
        self.mouse_move(x - 8.0, y - 5.0).await?;
        sleep(Duration::from_millis(90)).await;
        self.mouse_move(x - 2.0, y + 1.0).await?;
        sleep(Duration::from_millis(70)).await;
        self.mouse_move(x, y).await?;
        sleep(Duration::from_millis(130)).await;
        self.mouse_down(x, y).await?;
        sleep(Duration::from_millis(60)).await;
        self.mouse_up(x, y).await?;
        Ok(())
    }
}

/// `run_js` 多数情况下返回 `JSON.stringify` 的字符串;为稳妥也兼容已是对象的返回。
fn parse_probe(v: Value) -> Value {
    match v.as_str() {
        Some(s) => serde_json::from_str(s).unwrap_or(Value::Null),
        None => v,
    }
}
