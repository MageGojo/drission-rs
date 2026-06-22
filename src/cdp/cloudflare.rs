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

/// 探测当前是否处于 CF 挑战、(若有)Turnstile 可点区域的视口位置、以及是否已拿到 token。
/// 三级定位(由强到弱):① 遍历 light DOM **及开放 shadow DOM** 找 CF/Turnstile **iframe**;
/// ② iframe 在**闭合 shadow DOM**(`querySelectorAll('iframe')` 查不到——exa 实测如此)时,用
/// `cf-turnstile-response` 隐藏域的"可见祖先盒"(实测=该 input 的父 div,~300×70 小部件框)定位;
/// ③ 再兜底 turnstile 宿主容器(`.cf-turnstile`/`[data-sitekey]` 等)。返回 `JSON.stringify` 字符串。
const PROBE_JS: &str = r#"(function () {
  var t = (document.title || '').toLowerCase();
  var titleChallenge = t.indexOf('just a moment') >= 0
    || t.indexOf('attention required') >= 0
    || t.indexOf('checking your browser') >= 0
    || t.indexOf('verifying') >= 0
    || t.indexOf('请稍候') >= 0;
  var optChallenge = (typeof window._cf_chl_opt !== 'undefined');
  function rectOf(el){var r=el.getBoundingClientRect();return {x:r.x,y:r.y,w:r.width,h:r.height};}
  var box = null, kind = null;
  // ① CF/Turnstile iframe:light DOM + 开放 shadow DOM(闭合 shadow 读不到,见 ②)。
  var stack=[document];
  while(stack.length && !box){
    var root=stack.pop(); var els;
    try{els=root.querySelectorAll('*');}catch(e){continue;}
    for(var i=0;i<els.length;i++){var el=els[i];
      if(el.shadowRoot){stack.push(el.shadowRoot);}
      if(el.tagName==='IFRAME'){
        var src=el.getAttribute('src')||'', id=el.id||'', ti=el.getAttribute('title')||'';
        if(/challenges\.cloudflare\.com|turnstile|cdn-cgi\/challenge/i.test(src)
           || /cf-chl-widget/i.test(id)
           || /cloudflare|challenge|verify|验证/i.test(ti)){
          var r=rectOf(el); if(r.w>0&&r.h>0){box=r;kind='iframe';break;}
        }
      }
    }
  }
  // ② respbox:cf-turnstile-response 隐藏域的可见祖先盒(iframe 在闭合 shadow 时唯一可定位的小部件框)。
  //    CDP 合成鼠标按**屏幕坐标**命中能穿透闭合 shadow / 跨域 iframe 点到复选框。
  var resp=document.querySelector('[name=cf-turnstile-response]')
        || document.querySelector('[id^=cf-chl-widget][id$=_response]');
  if(!box && resp){
    var p=resp.parentElement;
    for(var k=0;k<5&&p;k++){var rr=rectOf(p); if(rr.w>=60&&rr.w<=520&&rr.h>=40&&rr.h<=140){box=rr;kind='respbox';break;} p=p.parentElement;}
  }
  // ③ host 容器兜底。
  if(!box){
    var host=document.querySelector('.cf-turnstile,[data-sitekey],#cf-turnstile,[class*="turnstile" i]');
    if(host){var rh=rectOf(host); if(rh.w>0&&rh.h>0){box=rh;kind='host';}}
  }
  // 已产出的 Turnstile token(非空=已过盾;widget 过盾后仍留在 DOM,故必须据此判过,不能只看 challenge 消失)。
  var token = resp ? (resp.value||'') : '';
  var challenge = titleChallenge || optChallenge || box !== null;
  return JSON.stringify({ challenge: challenge, box: box, kind: kind, token: token.length, title: document.title });
})()"#;

/// 一次 CF 探测的折算结果。
struct CfState {
    /// 是否处于 Cloudflare 挑战(整页托管质询 / 表单内嵌 Turnstile)。
    challenge: bool,
    /// Turnstile 复选框的视口可信点击坐标(找不到可点区域时为 `None`)。
    checkbox: Option<(f64, f64)>,
    /// 是否已产出有效 Turnstile token(非空 → 已过盾)。
    has_token: bool,
    /// 可点区域的定位方式(`iframe`/`respbox`/`host`),仅用于日志诊断。
    kind: Option<String>,
}

impl ChromiumTab {
    /// 自动通过 Cloudflare 挑战(默认 30s 超时)。见 [`pass_cloudflare`](Self::pass_cloudflare)。
    pub async fn pass_cloudflare_default(&self) -> Result<bool> {
        self.pass_cloudflare(Duration::from_secs(30)).await
    }

    /// 在 `timeout` 内尝试通过 Cloudflare 挑战(整页托管质询 **或** 表单内嵌 Turnstile):
    /// - 已出有效 Turnstile token / 已不是挑战页 → 立即返回 `true`;
    /// - 发现 Turnstile 复选框(含**闭合 shadow DOM** 里的,经 respbox 定位)→ 拟人移动 + **可信点击**,等待放行;
    /// - 只有非交互式挑战(无复选框)→ 轮询等待其自动放行。
    ///
    /// 返回是否在超时内通过。**非破坏**:不是挑战页时等价一次轻量探测后立即返回。
    /// 内嵌 Turnstile 过盾后 widget 仍留在 DOM,故以 `cf-turnstile-response` 是否产出 token 为过盾判据
    /// (不能等"挑战消失")。
    pub async fn pass_cloudflare(&self, timeout: Duration) -> Result<bool> {
        let deadline = Instant::now() + timeout;
        let mut clicked = 0u32;
        loop {
            let st = self.cf_state().await?;
            // 内嵌 Turnstile:拿到 token 即过(widget 过盾后仍留在 DOM,故据 token 判过,不能等它消失)。
            if st.has_token {
                return Ok(true);
            }
            // 整页托管质询:挑战标记消失即过。
            if !st.challenge {
                return Ok(true);
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            match st.checkbox {
                Some((cx, cy)) => {
                    tracing::debug!(
                        x = cx,
                        y = cy,
                        kind = st.kind.as_deref().unwrap_or("?"),
                        "点击 Cloudflare Turnstile 复选框(CDP)"
                    );
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
        Ok(self.cf_state().await?.challenge)
    }

    /// 探测一次 CF 状态:是否挑战页、Turnstile 复选框坐标、是否已出 token、定位方式。
    async fn cf_state(&self) -> Result<CfState> {
        let probe = parse_probe(self.run_js(PROBE_JS).await?);
        let challenge = probe
            .get("challenge")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let has_token = probe.get("token").and_then(Value::as_u64).unwrap_or(0) > 20;
        let kind = probe.get("kind").and_then(Value::as_str).map(String::from);
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
        Ok(CfState {
            challenge,
            checkbox,
            has_token,
            kind,
        })
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
