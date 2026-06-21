//! CDP 后端启动选项 [`ChromiumOptions`](对标 Camoufox 后端的 `BrowserOptions`,但更精简)。
//!
//! 链式 builder:无头 / 窗口大小 / 反检测 / 指定可执行文件 / 持久 profile / 地区伪装 / 代理 /
//! 额外参数。**默认有头 + 反检测开箱即用**(对齐 Camoufox 后端取向)。

use std::path::PathBuf;

/// CDP 浏览器启动选项。
#[derive(Debug, Clone)]
pub struct ChromiumOptions {
    /// 显式可执行文件路径(Chrome/Edge/Brave/Chromium);为空走自动定位 + 自动下载。
    pub binary_path: Option<PathBuf>,
    /// 是否无头(默认 `false` = 有头)。
    pub headless: bool,
    /// 用户数据目录(profile)。设置即**持久**(quit 不删、登录态/Cookie 保留);
    /// 为空则用临时目录(quit 删除)。
    pub user_data_dir: Option<PathBuf>,
    /// 启用反检测(默认 `true`):反检测启动参数 + 导航前注入 + 不调用 `Runtime.enable`。
    pub stealth: bool,
    /// 无头时把 UA 里的 `HeadlessChrome` 令牌伪装成 `Chrome`(默认 `true`)。
    /// 无头 Chrome 的 `navigator.userAgent` 串默认带 `HeadlessChrome`,是 CF 识破的头号信号;
    /// 开启后探测 `chrome --version` 主版本、构造与之一致的精简 UA,经 **`--user-agent` 启动参数**
    /// (浏览器级,覆盖 Turnstile 跨域子帧 —— 对标 DrissionPage `set_user_agent`)下发。
    /// 仅在**无头 + stealth + 未显式 `user_agent`** 时生效。(新无头的低熵 Sec-CH-UA 品牌默认已不含
    /// Headless;但 `--user-agent` 会**清空高熵 Client Hints**,见 [`full_ua_metadata`](Self::full_ua_metadata)。)
    pub mask_ua: bool,
    /// 无头补全**高熵 Client Hints**(默认 `false` = 不改变现有行为)。
    ///
    /// `mask_ua` 用 `--user-agent` 抹掉 UA 串里的 `HeadlessChrome` 时,Chrome 会**清空**
    /// `navigator.userAgentData.getHighEntropyValues(['fullVersionList','architecture',…])`
    /// (留下空 `fullVersionList`、空 `architecture/bitness/platformVersion/uaFullVersion`)——
    /// 这是有头/无头 diff 里仅剩的强无头信号。开启后,每个标签 attach 时再用 CDP
    /// `Emulation.setUserAgentOverride` 补回完整 `userAgentMetadata`(低熵品牌**运行时读取**以保
    /// GREASE 正确;完整版本/架构/平台版本据真机推导)→ 高熵 CH 与有头一致。
    /// 仅在**无头 + stealth + mask_ua(未显式 `user_agent`)**时生效。
    pub full_ua_metadata: bool,
    /// 窗口大小 `(width, height)`(有头是初始窗口、无头是视口)。
    pub window_size: Option<(u32, u32)>,
    /// User-Agent 覆盖(默认不改,用真实 Chrome UA)。
    pub user_agent: Option<String>,
    /// 地区 locale(如 `en-US`)。**默认不设**:与出口 IP 不符反降可信度。
    pub locale: Option<String>,
    /// 时区(如 `America/New_York`)。默认不设(理由同 `locale`)。
    pub timezone: Option<String>,
    /// 代理服务器(如 `http://127.0.0.1:8080` / `socks5://host:1080`)。
    pub proxy: Option<String>,
    /// 下载目录。设置后浏览器允许下载并落盘到此目录(`Browser.setDownloadBehavior`),
    /// `tab.downloads()` 的 `start()` 即从此读取(对齐 camoufox `BrowserOptions::download_path`)。
    pub download_path: Option<PathBuf>,
    /// **导航前注入脚本**(`Page.addScriptToEvaluateOnNewDocument`):每个新标签 attach 时按序注入,
    /// 在页面脚本运行前最早期执行、覆盖后续导航与子帧。用于**深指纹伪装**(由 [`CdpFingerprint`]
    /// (crate::cdp::CdpFingerprint) 生成:platform / 硬件 / 屏幕 / 语言 / canvas·webgl·audio 噪声)。
    /// 在内置反检测脚本(`STEALTH_JS` 等)**之后**注入。默认空。
    pub init_scripts: Vec<String>,
    /// 额外命令行参数。
    pub args: Vec<String>,
}

impl Default for ChromiumOptions {
    /// 大道至简:**有头 + 反检测开箱即用**。无头一行 `.headless(true)`。
    fn default() -> Self {
        Self {
            binary_path: None,
            headless: false,
            user_data_dir: None,
            stealth: true,
            mask_ua: true,
            full_ua_metadata: false,
            window_size: None,
            user_agent: None,
            locale: None,
            timezone: None,
            proxy: None,
            download_path: None,
            init_scripts: Vec::new(),
            args: Vec::new(),
        }
    }
}

impl ChromiumOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn headless(mut self, yes: bool) -> Self {
        self.headless = yes;
        self
    }

    pub fn binary_path(mut self, p: impl Into<PathBuf>) -> Self {
        self.binary_path = Some(p.into());
        self
    }

    /// 设持久 profile 目录(登录态/Cookie 跨进程复用,quit 不删)。
    pub fn user_data_dir(mut self, p: impl Into<PathBuf>) -> Self {
        self.user_data_dir = Some(p.into());
        self
    }

    /// 开/关反检测(默认开)。
    pub fn stealth(mut self, yes: bool) -> Self {
        self.stealth = yes;
        self
    }

    /// 开/关无头 UA 伪装(默认开,把 `HeadlessChrome` 改回 `Chrome`)。
    pub fn mask_ua(mut self, yes: bool) -> Self {
        self.mask_ua = yes;
        self
    }

    /// 开/关无头**高熵 Client Hints 补全**(默认关;见 [`full_ua_metadata`](Self::full_ua_metadata) 字段)。
    /// 开启后无头的 `getHighEntropyValues`(fullVersionList/architecture/…)与有头一致。
    pub fn full_ua_metadata(mut self, yes: bool) -> Self {
        self.full_ua_metadata = yes;
        self
    }

    pub fn window_size(mut self, width: u32, height: u32) -> Self {
        self.window_size = Some((width, height));
        self
    }

    pub fn user_agent(mut self, ua: impl Into<String>) -> Self {
        self.user_agent = Some(ua.into());
        self
    }

    /// 设地区(谨慎:应与出口 IP 一致)。
    pub fn locale(mut self, locale: impl Into<String>) -> Self {
        self.locale = Some(locale.into());
        self
    }

    /// 设时区(谨慎:应与出口 IP 一致)。
    pub fn timezone(mut self, tz: impl Into<String>) -> Self {
        self.timezone = Some(tz.into());
        self
    }

    pub fn proxy(mut self, server: impl Into<String>) -> Self {
        self.proxy = Some(server.into());
        self
    }

    /// 设下载目录(允许下载并落盘到此处;`tab.downloads().start()` 据此跟踪)。
    pub fn download_path(mut self, dir: impl Into<PathBuf>) -> Self {
        self.download_path = Some(dir.into());
        self
    }

    /// 追加一个命令行参数。
    pub fn add_arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// 追加一段导航前注入脚本(每个新标签 attach 时在页面脚本运行前最早期执行)。
    /// 多次调用按序累积;深指纹伪装请优先用 [`CdpFingerprint`](crate::cdp::CdpFingerprint)。
    pub fn add_init_script(mut self, js: impl Into<String>) -> Self {
        self.init_scripts.push(js.into());
        self
    }

    /// 设置导航前注入脚本列表(覆盖既有)。
    pub fn init_scripts(mut self, scripts: Vec<String>) -> Self {
        self.init_scripts = scripts;
        self
    }

    /// 校验用户参数不覆盖库内部管理的关键启动参数(否则会破坏调试端口/profile/无头管理)。
    pub fn validate(&self) -> crate::Result<()> {
        const PROTECTED: [&str; 4] = [
            "--remote-debugging-port",
            "--user-data-dir",
            "--headless",
            "--remote-debugging-pipe",
        ];
        for a in &self.args {
            let key = a.split('=').next().unwrap_or(a).trim();
            if PROTECTED.iter().any(|p| key.eq_ignore_ascii_case(p)) {
                return Err(crate::Error::Other(format!(
                    "非法启动参数 `{a}`:`{key}` 由库内部管理(改用 ChromiumOptions 的对应方法)"
                )));
            }
        }
        Ok(())
    }
}

/// **每标签上下文覆盖**(对齐 camoufox `ContextOverride`):代理 + UA/locale/时区。
/// 喂给 [`ChromiumBrowser::new_tab_with`](crate::cdp::ChromiumBrowser::new_tab_with):带 `proxy`
/// 时新建独立 `BrowserContext`(CDP 原生 per-context 代理),其余项经会话级 `Emulation` 覆盖。
/// 并发池([`ChromiumPool`](crate::cdp::ChromiumPool))每任务据此轮换出口/指纹。
#[derive(Debug, Clone, Default)]
pub struct ChromiumContextOverride {
    /// 出口代理(如 `http://host:port` / `socks5://host:1080`)。设置即新建独立上下文。
    pub proxy: Option<String>,
    /// 代理绕过列表(如 `<-loopback>`)。
    pub proxy_bypass: Option<String>,
    /// User-Agent 覆盖(`Emulation.setUserAgentOverride`)。
    pub user_agent: Option<String>,
    /// 地区 locale(`Emulation.setLocaleOverride`)。
    pub locale: Option<String>,
    /// 时区(`Emulation.setTimezoneOverride`)。
    pub timezone: Option<String>,
}

impl ChromiumContextOverride {
    /// 空覆盖。
    pub fn new() -> Self {
        Self::default()
    }
    /// 设出口代理(新建独立 BrowserContext)。
    pub fn proxy(mut self, p: impl Into<String>) -> Self {
        self.proxy = Some(p.into());
        self
    }
    /// 设代理绕过列表。
    pub fn proxy_bypass(mut self, b: impl Into<String>) -> Self {
        self.proxy_bypass = Some(b.into());
        self
    }
    /// 设 UA 覆盖。
    pub fn user_agent(mut self, ua: impl Into<String>) -> Self {
        self.user_agent = Some(ua.into());
        self
    }
    /// 设地区 locale。
    pub fn locale(mut self, l: impl Into<String>) -> Self {
        self.locale = Some(l.into());
        self
    }
    /// 设时区。
    pub fn timezone(mut self, tz: impl Into<String>) -> Self {
        self.timezone = Some(tz.into());
        self
    }

    /// 把 UA/locale/时区经会话级 `Emulation` 覆盖应用到标签(best-effort,单项失败忽略)。
    pub(crate) async fn apply_emulation(&self, tab: &crate::cdp::ChromiumTab) {
        if let Some(ua) = &self.user_agent {
            let mut p = serde_json::json!({ "userAgent": ua });
            if let Some(l) = &self.locale {
                p["acceptLanguage"] = serde_json::json!(l);
            }
            let _ = tab.core.send("Emulation.setUserAgentOverride", p).await;
        }
        if let Some(tz) = &self.timezone {
            let _ = tab
                .core
                .send(
                    "Emulation.setTimezoneOverride",
                    serde_json::json!({ "timezoneId": tz }),
                )
                .await;
        }
        if let Some(l) = &self.locale {
            let _ = tab
                .core
                .send(
                    "Emulation.setLocaleOverride",
                    serde_json::json!({ "locale": l }),
                )
                .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_headful_and_stealth() {
        let o = ChromiumOptions::new();
        assert!(!o.headless, "默认有头");
        assert!(o.stealth, "默认开反检测");
        assert!(o.binary_path.is_none(), "默认自动定位浏览器");
        assert!(o.locale.is_none() && o.timezone.is_none(), "默认不设地区");
    }

    #[test]
    fn builder_chains() {
        let o = ChromiumOptions::new()
            .headless(true)
            .window_size(1280, 800)
            .user_agent("UA/1.0")
            .locale("en-US")
            .timezone("America/New_York")
            .add_arg("--mute-audio");
        assert!(o.headless);
        assert_eq!(o.window_size, Some((1280, 800)));
        assert_eq!(o.user_agent.as_deref(), Some("UA/1.0"));
        assert_eq!(o.locale.as_deref(), Some("en-US"));
        assert_eq!(o.timezone.as_deref(), Some("America/New_York"));
        assert!(o.args.iter().any(|a| a == "--mute-audio"));
    }
}
