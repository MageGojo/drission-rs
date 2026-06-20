//! 启动选项与浏览器信息(指纹)配置。
//!
//! 对应 DrissionPage 的 `ChromiumOptions`,用链式 builder 配置:无头、参数、代理、
//! User-Agent、语言、时区、窗口大小、地理位置、操作系统指纹等。

use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Value, json};

/// 目标操作系统指纹(影响 navigator/字体等伪装)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OsType {
    Windows,
    MacOS,
    Linux,
}

impl OsType {
    /// Camoufox 期望的字符串值。
    pub fn as_camoufox(&self) -> &'static str {
        match self {
            OsType::Windows => "windows",
            OsType::MacOS => "macos",
            OsType::Linux => "linux",
        }
    }
}

/// 地理位置覆盖。
#[derive(Debug, Clone, Copy)]
pub struct Geolocation {
    pub latitude: f64,
    pub longitude: f64,
    pub accuracy: Option<f64>,
}

/// 代理配置。`server` 形如 `http://127.0.0.1:8080` 或 `socks5://host:1080`。
#[derive(Debug, Clone)]
pub struct Proxy {
    pub server: String,
    pub username: Option<String>,
    pub password: Option<String>,
    pub bypass: Vec<String>,
}

impl Proxy {
    pub fn new(server: impl Into<String>) -> Self {
        Self {
            server: server.into(),
            username: None,
            password: None,
            bypass: Vec::new(),
        }
    }

    pub fn auth(mut self, user: impl Into<String>, pass: impl Into<String>) -> Self {
        self.username = Some(user.into());
        self.password = Some(pass.into());
        self
    }
}

/// 浏览器信息 / 指纹覆盖。对应“修改浏览器信息”的需求。
#[derive(Debug, Clone, Default)]
pub struct Fingerprint {
    pub user_agent: Option<String>,
    pub platform: Option<String>,
    pub locale: Option<String>,
    pub timezone_id: Option<String>,
    pub geolocation: Option<Geolocation>,
    pub os: Option<OsType>,
}

impl Fingerprint {
    /// 是否为空(没有任何覆盖项)。
    pub fn is_empty(&self) -> bool {
        self.user_agent.is_none()
            && self.platform.is_none()
            && self.locale.is_none()
            && self.timezone_id.is_none()
            && self.geolocation.is_none()
            && self.os.is_none()
    }
}

/// 启动选项。通过链式调用配置后传给浏览器启动器。
#[derive(Debug, Clone)]
pub struct BrowserOptions {
    /// 显式指定 Camoufox 可执行文件路径;为空则走自动下载分发。
    pub binary_path: Option<PathBuf>,
    /// 用户数据目录(profile);为空则使用临时目录。
    pub user_data_dir: Option<PathBuf>,
    /// 是否无头。
    pub headless: bool,
    /// 额外命令行参数(禁止以 `-profile`/`-juggler` 开头)。
    pub args: Vec<String>,
    /// 启动超时(等待 “Juggler listening to the pipe”)。
    pub launch_timeout: Duration,
    /// 默认窗口/视口大小。
    pub window_size: Option<(u32, u32)>,
    /// 代理。
    pub proxy: Option<Proxy>,
    /// 浏览器信息 / 指纹。
    pub fingerprint: Fingerprint,
    /// 是否启用拟人化行为(反检测)。
    pub humanize: bool,
    /// 忽略 HTTPS 证书错误。
    pub ignore_https_errors: bool,
    /// 绕过 CSP(便于注入脚本)。
    pub bypass_csp: bool,
    /// 拟人化光标移动的最大时长(秒);设置即开启 `humanize`。
    pub humanize_max_time: Option<f64>,
    /// 阻断 WebRTC(防止经 STUN 暴露真实 IP)。
    pub block_webrtc: bool,
    /// 把 Camoufox 默认 UA 里的 `Camoufox/<ver>` 令牌伪装成真实 Firefox(`Firefox/<major>.0`)。
    /// 裸启动 Camoufox(不经其 Python 库)时,UA 默认带 `Camoufox` 字样,是明显的自动化指纹;
    /// 开启后启动时读 `Browser.getInfo` 的真实 UA、把令牌换成 `Firefox` 再经上下文覆盖下发。
    /// 仅当用户未显式 [`user_agent`](Self::user_agent) 时生效。默认 `true`。
    pub mask_ua: bool,
    /// 屏幕尺寸覆盖 `(width, height)`(CSS 像素)。裸启动 Camoufox 不跑 BrowserForge 自动补全,
    /// 其默认屏幕会与窗口不自洽(实测 `window.outer` 比 `screen` 还高),是破绽。给一个常见且
    /// 自洽的屏幕即可消除。默认 `Some((1920, 1080))`;`None` 表示用 Camoufox 原始屏幕。
    pub screen: Option<(u32, u32)>,
    /// 额外的 Firefox user preferences(逃生舱,直接下发到浏览器)。
    pub firefox_prefs: Vec<(String, Value)>,
    /// 额外的 Camoufox 指纹配置(`CAMOU_CONFIG_*` 透传,逃生舱):可设任意官方支持的字段,
    /// 如 `("navigator.hardwareConcurrency", json!(8))`、`("webGl:vendor", json!("Google Inc."))`。
    /// 会覆盖由便捷项(如 `screen`)生成的同名键。键名见 <https://camoufox.com/fingerprint/>。
    pub camou_config: Vec<(String, Value)>,
    /// 下载目录。设置后:文件自动存到此目录(不弹"另存为"框),并可用 `tab.wait_download()`
    /// 等下载完成。为空则用浏览器默认行为。
    pub download_path: Option<PathBuf>,
}

impl Default for BrowserOptions {
    /// 大道至简的默认值:**有头 + 反检测开箱即用**。
    ///
    /// - `headless = false`(默认有头;无头加 `.headless(true)` 即可)
    /// - `humanize = true`、`block_webrtc = true`(反检测默认开,等价于以前要手写的那串)
    /// - `binary_path = None`(自动下载 / 定位 Camoufox 到默认缓存位置)
    ///
    /// 不默认 locale/timezone:强行设成与本机 IP 不符的地区反而**降低**反检测可信度,
    /// 需要时自己 `.locale(..)/.timezone(..)`(见 `examples/cf_check`)。
    fn default() -> Self {
        Self {
            binary_path: None,
            user_data_dir: None,
            headless: false,
            args: Vec::new(),
            launch_timeout: Duration::from_secs(180),
            window_size: None,
            proxy: None,
            fingerprint: Fingerprint::default(),
            humanize: true,
            ignore_https_errors: false,
            bypass_csp: false,
            humanize_max_time: None,
            block_webrtc: true,
            mask_ua: true,
            screen: Some((1920, 1080)),
            firefox_prefs: Vec::new(),
            camou_config: Vec::new(),
            download_path: None,
        }
    }
}

impl BrowserOptions {
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

    pub fn user_data_dir(mut self, p: impl Into<PathBuf>) -> Self {
        self.user_data_dir = Some(p.into());
        self
    }

    /// 追加一个命令行参数。
    pub fn add_arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    pub fn window_size(mut self, width: u32, height: u32) -> Self {
        self.window_size = Some((width, height));
        self
    }

    pub fn proxy(mut self, proxy: Proxy) -> Self {
        self.proxy = Some(proxy);
        self
    }

    pub fn user_agent(mut self, ua: impl Into<String>) -> Self {
        self.fingerprint.user_agent = Some(ua.into());
        self
    }

    pub fn locale(mut self, locale: impl Into<String>) -> Self {
        self.fingerprint.locale = Some(locale.into());
        self
    }

    pub fn timezone(mut self, tz: impl Into<String>) -> Self {
        self.fingerprint.timezone_id = Some(tz.into());
        self
    }

    pub fn platform(mut self, platform: impl Into<String>) -> Self {
        self.fingerprint.platform = Some(platform.into());
        self
    }

    pub fn os(mut self, os: OsType) -> Self {
        self.fingerprint.os = Some(os);
        self
    }

    pub fn geolocation(mut self, latitude: f64, longitude: f64) -> Self {
        self.fingerprint.geolocation = Some(Geolocation {
            latitude,
            longitude,
            accuracy: None,
        });
        self
    }

    pub fn humanize(mut self, yes: bool) -> Self {
        self.humanize = yes;
        self
    }

    pub fn ignore_https_errors(mut self, yes: bool) -> Self {
        self.ignore_https_errors = yes;
        self
    }

    pub fn bypass_csp(mut self, yes: bool) -> Self {
        self.bypass_csp = yes;
        self
    }

    /// 开启拟人化光标移动,并指定最大移动时长(秒)。
    pub fn humanize_max_time(mut self, seconds: f64) -> Self {
        self.humanize = true;
        self.humanize_max_time = Some(seconds);
        self
    }

    /// 阻断 WebRTC(防止真实 IP 泄漏)。
    pub fn block_webrtc(mut self, yes: bool) -> Self {
        self.block_webrtc = yes;
        self
    }

    /// 追加一个 Firefox user preference(高级用法)。
    pub fn add_pref(mut self, name: impl Into<String>, value: Value) -> Self {
        self.firefox_prefs.push((name.into(), value));
        self
    }

    /// 是否把 Camoufox 的 UA 令牌伪装成真实 Firefox(默认 `true`,见 [`mask_ua`](Self::mask_ua) 字段)。
    pub fn mask_ua(mut self, yes: bool) -> Self {
        self.mask_ua = yes;
        self
    }

    /// 覆盖屏幕尺寸 `(width, height)`(CSS 像素),保证与窗口自洽(见 [`screen`](Self::screen) 字段)。
    pub fn screen(mut self, width: u32, height: u32) -> Self {
        self.screen = Some((width, height));
        self
    }

    /// 不覆盖屏幕,使用 Camoufox 原始屏幕值(慎用:裸启动下默认屏幕可能与窗口不自洽)。
    pub fn raw_screen(mut self) -> Self {
        self.screen = None;
        self
    }

    /// 追加一个 Camoufox 指纹配置字段(`CAMOU_CONFIG_*` 透传,见 [`camou_config`](Self::camou_config) 字段)。
    pub fn add_camou_config(mut self, name: impl Into<String>, value: Value) -> Self {
        self.camou_config.push((name.into(), value));
        self
    }

    /// 设置下载目录:文件自动存到此目录(不弹"另存为"框),配合 `tab.wait_download()` 等下载完成。
    pub fn download_path(mut self, p: impl Into<PathBuf>) -> Self {
        self.download_path = Some(p.into());
        self
    }

    /// 汇总要下发给 Camoufox 的指纹配置(`CAMOU_CONFIG_*` 的 JSON 内容):拟人化光标 + 屏幕一致性
    /// + 自定义透传。launcher 会把它序列化后**按字符分块**写入 `CAMOU_CONFIG_1..n`(浏览器侧拼接再解析)。
    ///
    /// 注意:UA 不走这里(走 `Browser.getInfo` + 上下文覆盖,见 [`mask_ua`](Self::mask_ua));此处只放
    /// 必须在进程启动前就位的、由 Camoufox C++ 层拦截的指纹字段。
    pub fn build_camou_config(&self) -> serde_json::Map<String, Value> {
        let mut cfg = serde_json::Map::new();
        // 拟人化光标(浏览器侧 MaskConfig 读 humanize / humanize:maxTime / showcursor)。
        if self.humanize {
            cfg.insert("humanize".into(), Value::Bool(true));
            if let Some(t) = self.humanize_max_time {
                cfg.insert("humanize:maxTime".into(), json!(t));
            }
            // 光标高亮只是视觉辅助、不进入页面上下文;关掉省渲染。
            cfg.insert("showcursor".into(), Value::Bool(false));
        }
        // 屏幕一致性:给一个常见且与窗口自洽的屏幕(window.outer 需 <= screen.avail)。
        if let Some((w, h)) = self.screen {
            let avail_top: u32 = 25; // 顶部菜单栏高度(mac 风格);availHeight 据此收一点。
            cfg.insert("screen.width".into(), json!(w));
            cfg.insert("screen.height".into(), json!(h));
            cfg.insert("screen.availWidth".into(), json!(w));
            cfg.insert("screen.availHeight".into(), json!(h.saturating_sub(avail_top)));
            cfg.insert("screen.availTop".into(), json!(avail_top));
            cfg.insert("screen.availLeft".into(), json!(0));
            cfg.insert("screen.colorDepth".into(), json!(24));
            cfg.insert("screen.pixelDepth".into(), json!(24));
        }
        // 自定义透传(覆盖以上便捷项的同名键)。
        for (k, v) in &self.camou_config {
            cfg.insert(k.clone(), v.clone());
        }
        cfg
    }

    /// 汇总要下发的 Firefox user preferences(便捷项 + 自定义),供 `Browser.enable` 使用。
    pub fn collect_firefox_prefs(&self) -> Vec<(String, Value)> {
        let mut prefs: Vec<(String, Value)> = Vec::new();
        if self.block_webrtc {
            prefs.push(("media.peerconnection.enabled".to_string(), Value::Bool(false)));
        }
        // 下载:存到指定目录、不弹框、PDF 直接下载而非内嵌查看。
        if let Some(dir) = &self.download_path {
            prefs.push(("browser.download.folderList".into(), json!(2)));
            prefs.push(("browser.download.dir".into(), json!(dir.display().to_string())));
            prefs.push(("browser.download.useDownloadDir".into(), json!(true)));
            prefs.push(("browser.download.manager.showWhenStarting".into(), json!(false)));
            prefs.push(("browser.download.alwaysOpenPanel".into(), json!(false)));
            prefs.push(("pdfjs.disabled".into(), json!(true)));
            prefs.push((
                "browser.helperApps.neverAsk.saveToDisk".into(),
                json!(
                    "application/octet-stream,application/pdf,application/zip,application/x-zip-compressed,application/x-msdownload,application/msword,application/vnd.ms-excel,text/csv,text/plain,application/json,image/png,image/jpeg,application/x-binary,application/force-download"
                ),
            ));
        }
        prefs.extend(self.firefox_prefs.iter().cloned());
        prefs
    }

    /// 校验用户提供的参数是否合法(不得覆盖受保护的启动参数)。
    pub fn validate(&self) -> crate::Result<()> {
        for a in &self.args {
            let lower = a.trim_start_matches('-').to_ascii_lowercase();
            if lower.starts_with("profile") || lower.starts_with("juggler") {
                return Err(crate::Error::Other(format!(
                    "非法启动参数 `{a}`:`-profile`/`-juggler` 由库内部管理"
                )));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_chains() {
        let opts = BrowserOptions::new()
            .headless(true)
            .window_size(1280, 800)
            .user_agent("UA/1.0")
            .locale("zh-CN")
            .timezone("Asia/Shanghai")
            .os(OsType::MacOS)
            .geolocation(31.23, 121.47);
        assert!(opts.headless);
        assert_eq!(opts.window_size, Some((1280, 800)));
        assert_eq!(opts.fingerprint.user_agent.as_deref(), Some("UA/1.0"));
        assert_eq!(opts.fingerprint.locale.as_deref(), Some("zh-CN"));
        assert_eq!(opts.fingerprint.os, Some(OsType::MacOS));
        assert!(!opts.fingerprint.is_empty());
    }

    #[test]
    fn defaults_are_headful_and_stealth() {
        let o = BrowserOptions::new();
        assert!(!o.headless, "默认有头");
        assert!(o.humanize, "默认开启拟人化");
        assert!(o.block_webrtc, "默认阻断 WebRTC");
        assert!(o.binary_path.is_none(), "默认自动定位浏览器");
        // 反检测默认应下发 WebRTC 关闭的 user pref。
        assert!(
            o.collect_firefox_prefs()
                .iter()
                .any(|(k, _)| k == "media.peerconnection.enabled")
        );
        // 不默认地区,避免与 IP 不符。
        assert!(o.fingerprint.locale.is_none() && o.fingerprint.timezone_id.is_none());
        // 一行关无头。
        assert!(BrowserOptions::new().headless(true).headless);
    }

    #[test]
    fn rejects_protected_args() {
        let opts = BrowserOptions::new().add_arg("-profile /tmp/x");
        assert!(opts.validate().is_err());
        let opts = BrowserOptions::new().add_arg("--juggler-pipe");
        assert!(opts.validate().is_err());
        let ok = BrowserOptions::new().add_arg("--no-remote");
        assert!(ok.validate().is_ok());
    }
}
