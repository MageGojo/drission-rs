//! CDP 后端反检测(过 Cloudflare 盾的基础设施)。
//!
//! 三件事:**反检测启动参数** + **导航前注入脚本** + **无头 UA 去 `HeadlessChrome`**。配合
//! [`browser`](crate::cdp::browser) 里"**不调用 `Runtime.enable`**"(避免经典 CDP 探测泄漏),
//! 让 Google Chrome 在 CF 眼里与真人浏览器无异。**对标 Python 版 DrissionPage**(实测可过):
//! 它默认参数含 `--disable-site-isolation-trials`/`--test-type`、`set_user_agent` 走 `--user-agent`
//! **启动参数**(浏览器级、覆盖所有帧含 Turnstile 跨域 iframe),且**不**全局开 `Runtime.enable`。
//! 详见 `docs/CDP过盾.md`。

/// 反检测启动参数(在基础参数之外追加)。
///
/// 关键项 `--disable-blink-features=AutomationControlled` —— 关掉 blink 的"受自动化控制"特性,
/// `navigator.webdriver` 归 `false`、无自动化信息栏(且我们从不传 `--enable-automation`)。
/// 其余为良性硬化项(禁后台联网/首启向导/密码库弹窗等),不改变页面可见行为。
///
/// **实测教训**:不要加 `--test-type` / `--disable-site-isolation-trials`(DrissionPage 默认带它们,
/// 但实测对 exa.ai 的 Turnstile 反而从"有头 1s 出 token"变成"不出 token" —— `--test-type` 是已知
/// 的自动化信号、`--disable-site-isolation-trials` 改变了跨域 iframe 的进程模型)。保持最小集最稳。
pub(crate) fn stealth_args() -> Vec<String> {
    let v = vec![
        "--disable-blink-features=AutomationControlled".to_string(),
        "--no-first-run".to_string(),
        "--no-default-browser-check".to_string(),
        "--no-service-autorun".to_string(),
        "--password-store=basic".to_string(),
        "--disable-popup-blocking".to_string(),
        "--disable-background-networking".to_string(),
        "--disable-features=Translate,OptimizationHints,MediaRouter,InterestFeedContentSuggestions"
            .to_string(),
    ];
    // macOS:避免每次启动弹系统钥匙串授权框(其它平台 `v` 无需 mut,故条件 shadow)。
    #[cfg(target_os = "macos")]
    let v = {
        let mut v = v;
        v.push("--use-mock-keychain".to_string());
        v
    };
    v
}

/// 导航前注入的兜底脚本(`Page.addScriptToEvaluateOnNewDocument`)。
///
/// 故意**极小**:仅当 `navigator.webdriver` 仍为 `true`(理论上启动参数已置 false)才纠正,
/// 因此正常情况下是 no-op —— **不留下可被探测的多余 getter**。真实 Chrome 的
/// `chrome`/`plugins`/`languages`/`permissions` 本就齐全自洽,**不伪造**(伪造反而更易识破)。
pub(crate) const STEALTH_JS: &str = r#"(function () {
  try {
    if (navigator.webdriver === true) {
      Object.defineProperty(Object.getPrototypeOf(navigator), 'webdriver', {
        get: function () { return false; },
        configurable: true
      });
    }
  } catch (e) {}
})();"#;

/// 从一段文本(`chrome --version` 输出 / UA 串)里抽 Chrome 主版本号(如 `149`)。
/// 规则:找第一段"数字 + `.`"的版本主段。
pub(crate) fn parse_chrome_major(s: &str) -> Option<u32> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            // 紧跟 '.' 才认为是版本号主段(排除路径里的孤立数字)。
            if i < bytes.len() && bytes[i] == b'.' {
                return s[start..i].parse::<u32>().ok();
            }
        } else {
            i += 1;
        }
    }
    None
}

/// 按当前平台 + 主版本号构造**精简 UA**(Chrome 100+ reduced UA,与真实有头 Chrome 完全一致)。
/// 无头时经 `--user-agent` 启动参数下发,把 `HeadlessChrome` 抹掉(对标 DrissionPage set_user_agent)。
pub(crate) fn reduced_ua(major: u32) -> String {
    match std::env::consts::OS {
        "macos" => format!(
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{major}.0.0.0 Safari/537.36"
        ),
        "windows" => format!(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{major}.0.0.0 Safari/537.36"
        ),
        _ => format!(
            "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{major}.0.0.0 Safari/537.36"
        ),
    }
}

/// 无头屏幕补丁(导航前注入):把无头默认的 `screen` 800x600 改成常见真实显示器 1920x1080。
///
/// 实测无头(`--headless=new`)与有头的指纹差异**只剩** `screen` 尺寸(WebGL 已靠不禁用 GPU +
/// Metal 后端拿到真实 renderer;plugins/chrome/permissions/hardwareConcurrency 等新无头已自洽)。
/// 用 instance getter 覆盖 `screen.*`,与窗口尺寸自洽(window.outer ≤ screen)。
pub(crate) fn headless_screen_js() -> String {
    // 1920x1080,availHeight 留一点给任务栏(常见真实值),availTop/Left 归零。
    let (w, h, avail_h) = (1920u32, 1080u32, 1040u32);
    format!(
        r#"(function(){{
  try {{
    var def = function(o,p,v){{ try {{ Object.defineProperty(o,p,{{get:function(){{return v;}},configurable:true}}); }} catch(e){{}} }};
    def(screen,'width',{w}); def(screen,'height',{h});
    def(screen,'availWidth',{w}); def(screen,'availHeight',{avail_h});
    def(screen,'availLeft',0); def(screen,'availTop',0);
  }} catch(e){{}}
}})();"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stealth_args_have_core_flag() {
        let a = stealth_args();
        assert!(
            a.iter()
                .any(|s| s == "--disable-blink-features=AutomationControlled"),
            "必须含关掉 AutomationControlled 的核心反检测参数"
        );
        // 实测不加 --test-type(它是自动化信号,会让 Turnstile 不出 token)。
        assert!(!a.iter().any(|s| s == "--test-type"));
        // 绝不能把自动化开关加进来(那会显式暴露自动化)。
        assert!(
            !a.iter().any(|s| s.contains("enable-automation")),
            "不得传 --enable-automation"
        );
    }

    #[test]
    fn stealth_js_is_guarded_noop() {
        // 仅在 webdriver===true 时动手,默认不引入多余 getter。
        assert!(STEALTH_JS.contains("navigator.webdriver === true"));
        assert!(!STEALTH_JS.contains("addEventListener"));
    }

    #[test]
    fn version_parsing() {
        assert_eq!(
            parse_chrome_major("Google Chrome 149.0.7827.115 "),
            Some(149)
        );
        assert_eq!(
            parse_chrome_major("X HeadlessChrome/137.0.0.0 Y"),
            Some(137)
        );
        assert_eq!(parse_chrome_major("no version here"), None);
    }

    #[test]
    fn reduced_ua_has_no_headless_and_major() {
        let ua = reduced_ua(149);
        assert!(!ua.contains("Headless"));
        assert!(ua.contains("Chrome/149.0.0.0"));
        assert!(ua.starts_with("Mozilla/5.0"));
    }

    #[test]
    fn headless_screen_js_overrides_800x600() {
        let js = headless_screen_js();
        // 必须把无头默认 800x600 改成常见真实尺寸。
        assert!(js.contains("1920") && js.contains("1080"));
        assert!(js.contains("screen") && js.contains("availWidth"));
        assert!(!js.contains("800") && !js.contains("600"));
    }
}
