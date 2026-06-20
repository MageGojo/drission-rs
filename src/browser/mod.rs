//! 高层浏览器 API(DrissionPage 风格)。
//!
//! - [`Browser`][]:启动 / 退出 / 标签管理。每个标签是一个独立 BrowserContext(cookie 隔离)。
//! - [`tab::Tab`][]:页面操作(get/run_js/ele/cookies/listen…)。
//! - [`element::Element`][]:元素操作(click/input/text/attr…)。
//! - [`listener::DataPacket`][]:网络监听数据包。

pub mod actions;
pub mod cloudflare;
pub mod console;
pub mod download;
pub mod dump_env;
pub mod element;
pub mod frame;
pub mod handles;
pub mod interceptor;
pub mod keys;
pub mod listener;
pub mod screencast;
pub mod serve;
pub mod shadow;
#[cfg(feature = "slider")]
pub mod slider;
pub mod static_element;
pub mod storage;
pub mod tab;
pub mod websocket;
pub(crate) mod xpath;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tokio::sync::Mutex;

use crate::launcher::{self, BrowserOptions, Launched};
use crate::protocol::{BROWSER_CLOSE_MESSAGE_ID, Connection};
use crate::{Error, Result};

pub use actions::{Actions, MouseButton};
pub use console::{Console, ConsoleData, ConsoleFilter, ConsoleSteps};
pub use download::{DownloadMission, DownloadState, Downloads};
pub use dump_env::{EnvDump, EnvDumper, EnvProbe, EnvScope, EnvTarget};
pub use element::{Element, ElementRect, ElementWait};
pub use frame::Frame;
pub use handles::{Intercept, Listen, Scroll, SetTab, Wait, Window};
pub use interceptor::{InterceptedRequest, ResumeOptions};
pub use keys::{KeyInput, Keys};
pub use listener::{DataPacket, ListenFilter, RequestData, ResponseData};
pub use screencast::{Screencast, ScreencastMode};
pub use serve::BrowserServer;
pub use shadow::ShadowRoot;
#[cfg(feature = "slider")]
pub use slider::{GapMethod, ImageSource, SliderConfig, SliderGap, SliderResult, SuccessCheck};
pub use static_element::StaticElement;
pub use storage::{OriginStorage, StorageState};
pub use tab::{
    ContextOverride, Cookie, CookieParam, DialogInfo, DownloadInfo, GetOptions, ImageFormat,
    ListenStream, LoadMode, PageRect, ShotOpts, Tab,
};
pub use websocket::{WsDirection, WsFilter, WsListener, WsMessage, WsSocket, WsSteps};

/// 一个浏览器实例。
pub struct Browser {
    conn: Connection,
    child: Mutex<Option<crate::transport::Child>>,
    options: Arc<BrowserOptions>,
    tabs: Mutex<Vec<Tab>>,
    profile_dir: PathBuf,
    profile_is_temp: bool,
}

impl Browser {
    /// 用默认配置启动:**有头 + 反检测开箱即用 + 自动定位浏览器**(见 [`BrowserOptions::default`])。
    ///
    /// 一行起步:`let browser = Browser::launch_default().await?;`
    /// 要无头/自定义就用 [`launch`](Self::launch):`Browser::launch(BrowserOptions::new().headless(true)).await?`。
    pub async fn launch_default() -> Result<Self> {
        Self::launch(BrowserOptions::default()).await
    }

    /// 启动浏览器(默认 Camoufox,必要时自动下载),并打开第一个标签。
    pub async fn launch(opts: BrowserOptions) -> Result<Self> {
        let mut opts = opts;
        let Launched {
            child,
            writer,
            reader,
            profile_dir,
            profile_is_temp,
        } = launcher::launch(&opts).await?;

        let conn = Connection::from_pipe(writer, reader);
        init_session(&conn, &mut opts).await?;

        let browser = Self {
            conn,
            child: Mutex::new(Some(child)),
            options: Arc::new(opts),
            tabs: Mutex::new(Vec::new()),
            profile_dir,
            profile_is_temp,
        };

        // 打开首个标签。
        let tab = Tab::open(browser.conn.clone(), &browser.options).await?;
        browser.tabs.lock().await.push(tab);

        Ok(browser)
    }

    /// 通过 **WebSocket** 接管一个已在运行的浏览器(对标 DrissionPage 接管已开浏览器)。
    ///
    /// 端点须由 [`BrowserServer`] 暴露(讲原始 Juggler;**不**兼容 `camoufox server` 的 Playwright RPC)。
    /// 采用默认反检测选项;要自定义用 [`connect_with`](Self::connect_with)。
    ///
    /// 与 [`launch`](Self::launch) 的区别:不启动子进程;[`quit`](Self::quit) **不会**关闭远端浏览器
    /// (仅断开本地连接),需要关闭远端请显式调用 [`close_remote`](Self::close_remote)。
    pub async fn connect(ws_url: &str) -> Result<Self> {
        Self::connect_with(ws_url, BrowserOptions::default()).await
    }

    /// 同 [`connect`](Self::connect),但可指定 [`BrowserOptions`](crate::launcher::BrowserOptions)
    /// (其中启动相关项如 headless/binary_path 会被忽略,只用到会话级覆盖与反检测项)。
    pub async fn connect_with(ws_url: &str, opts: BrowserOptions) -> Result<Self> {
        let mut opts = opts;
        let ws = crate::transport::ws_connect(ws_url).await?;
        let conn = Connection::from_ws(ws);
        init_session(&conn, &mut opts).await?;

        let browser = Self {
            conn,
            child: Mutex::new(None),
            options: Arc::new(opts),
            tabs: Mutex::new(Vec::new()),
            profile_dir: PathBuf::new(),
            profile_is_temp: false,
        };

        let tab = Tab::open(browser.conn.clone(), &browser.options).await?;
        browser.tabs.lock().await.push(tab);

        Ok(browser)
    }

    /// 显式关闭**远端**浏览器(用于 [`connect`](Self::connect) 接管后想真正退出浏览器时)。
    pub async fn close_remote(&self) -> Result<()> {
        self.conn
            .fire(BROWSER_CLOSE_MESSAGE_ID, "Browser.close", json!({}))
    }

    /// 新建一个标签(独立 BrowserContext)。可选直接访问 `url`。
    pub async fn new_tab(&self, url: Option<&str>) -> Result<Tab> {
        let tab = Tab::open(self.conn.clone(), &self.options).await?;
        if let Some(u) = url {
            tab.get(u).await?;
        }
        self.tabs.lock().await.push(tab.clone());
        Ok(tab)
    }

    /// 新建一个标签,并叠加 **per-context 覆盖**(代理 / UA / locale / 时区 / 地理 / 视口)。
    ///
    /// 用于"同一浏览器进程内、每个标签不同代理或指纹"——这正是并发池([`BrowserPool`](crate::pool::BrowserPool))
    /// 轮换代理 / 指纹的底层入口。覆盖项叠加在本浏览器启动基线之上(见 [`ContextOverride`])。
    pub async fn new_tab_with(&self, overrides: &ContextOverride) -> Result<Tab> {
        let merged = overrides.merge_into((*self.options).clone());
        let tab = Tab::open(self.conn.clone(), &merged).await?;
        self.tabs.lock().await.push(tab.clone());
        Ok(tab)
    }

    /// 最近打开的标签。
    pub async fn latest_tab(&self) -> Result<Tab> {
        self.tabs
            .lock()
            .await
            .last()
            .cloned()
            .ok_or_else(|| Error::Other("没有可用标签".into()))
    }

    /// 按索引取标签(从 0 开始,按打开顺序)。
    pub async fn get_tab(&self, index: usize) -> Result<Tab> {
        self.tabs
            .lock()
            .await
            .get(index)
            .cloned()
            .ok_or_else(|| Error::Other(format!("标签索引越界: {index}")))
    }

    /// 当前标签数量。
    pub async fn tab_count(&self) -> usize {
        self.tabs.lock().await.len()
    }

    /// 退出浏览器:优雅关闭 → 超时则强杀 → 清理临时 profile。
    pub async fn quit(&self) -> Result<()> {
        if let Some(mut child) = self.child.lock().await.take() {
            let _ = self
                .conn
                .fire(BROWSER_CLOSE_MESSAGE_ID, "Browser.close", json!({}));
            tokio::select! {
                _ = child.wait() => {}
                _ = tokio::time::sleep(Duration::from_secs(3)) => {
                    let _ = child.kill().await;
                }
            }
        }
        if self.profile_is_temp {
            let _ = tokio::fs::remove_dir_all(&self.profile_dir).await;
        }
        Ok(())
    }
}

/// root 会话初始化(launch / connect 共用):启用 `Browser` 域 + 下发 Firefox user prefs +
/// (按需)屏蔽 Camoufox UA 令牌。可重复调用(Juggler 端幂等),故接管已运行浏览器时也安全。
async fn init_session(conn: &Connection, opts: &mut BrowserOptions) -> Result<()> {
    // 启用浏览器域(开启对新建上下文中页面的自动 attach),并下发
    // Firefox user prefs(如 block_webrtc → media.peerconnection.enabled=false)。
    let prefs = opts.collect_firefox_prefs();
    let mut enable_params = json!({ "attachToDefaultContext": false });
    if !prefs.is_empty() {
        let user_prefs: Vec<serde_json::Value> = prefs
            .into_iter()
            .map(|(name, value)| json!({ "name": name, "value": value }))
            .collect();
        enable_params["userPrefs"] = json!(user_prefs);
    }
    conn.send("Browser.enable", enable_params, None).await?;

    // 补环境:把 Camoufox 默认 UA 里的 `Camoufox/<ver>` 令牌伪装成真实 Firefox。
    // 裸启动(不经 Camoufox Python 库)时 UA 会带 `Camoufox` 字样,是明显的自动化指纹。
    // 仅当用户未显式设置 UA 时介入;读 `Browser.getInfo` 拿真实 UA(含正确 rv 版本)再替换。
    if opts.mask_ua && opts.fingerprint.user_agent.is_none() {
        if let Ok(info) = conn.send("Browser.getInfo", json!({}), None).await {
            if let Some(ua) = info.get("userAgent").and_then(|v| v.as_str()) {
                if let Some(cleaned) = clean_camoufox_ua(ua) {
                    tracing::debug!(to = %cleaned, "补环境:屏蔽 Camoufox UA 令牌");
                    opts.fingerprint.user_agent = Some(cleaned);
                }
            }
        }
    }
    Ok(())
}

/// 把 Camoufox 默认 UA 里的 `Camoufox/<ver>` 令牌伪装成真实 Firefox(`Firefox/<major>.0`)。
///
/// 主版本号优先取自 UA 中的 `rv:<major>.0`(与真实 Firefox 完全一致),取不到再回退用 `Camoufox/`
/// 后的主版本号。返回 `None` 表示 UA 里没有 `Camoufox` 令牌、无需改动。
///
/// 例:`...Gecko/20100101 Camoufox/150.0.2-beta.25` → `...Gecko/20100101 Firefox/150.0`。
fn clean_camoufox_ua(ua: &str) -> Option<String> {
    const TOKEN: &str = "Camoufox/";
    let idx = ua.find(TOKEN)?;
    let digits = |s: &str| -> Option<String> {
        let d: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
        if d.is_empty() { None } else { Some(d) }
    };
    let major = ua
        .find("rv:")
        .and_then(|i| digits(&ua[i + 3..]))
        .or_else(|| digits(&ua[idx + TOKEN.len()..]));
    let prefix = &ua[..idx];
    Some(match major {
        Some(m) => format!("{prefix}Firefox/{m}.0"),
        None => format!("{prefix}Firefox"),
    })
}

impl Drop for Browser {
    /// 兜底清理:即使调用方没有显式 `quit()`(提前 `?` 返回 / panic 展开 / 忘记调用),
    /// 也确保**子进程被杀、临时 profile 目录被删**,避免进程与磁盘泄漏(反复启动时尤甚)。
    ///
    /// 仍建议显式 `quit().await`——它会优雅关闭并 `wait` 回收子进程(无僵尸);此处为同步兜底:
    /// `start_kill` 发送终止信号(配合 spawn 时设置的 `kill_on_drop`),临时目录同步删除。
    /// 若 `quit()` 已执行,`child` 已被取走、临时目录已删,这里都成为安全的空操作。
    fn drop(&mut self) {
        if let Ok(mut guard) = self.child.try_lock() {
            if let Some(mut child) = guard.take() {
                let _ = child.start_kill();
            }
        }
        if self.profile_is_temp {
            let _ = std::fs::remove_dir_all(&self.profile_dir);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::clean_camoufox_ua;

    #[test]
    fn masks_camoufox_token_to_firefox() {
        let ua = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10.15; rv:150.0) Gecko/20100101 Camoufox/150.0.2-beta.25";
        assert_eq!(
            clean_camoufox_ua(ua).as_deref(),
            Some(
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10.15; rv:150.0) Gecko/20100101 Firefox/150.0"
            )
        );
    }

    #[test]
    fn major_falls_back_to_token_when_no_rv() {
        let ua = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) Gecko/20100101 Camoufox/133.1";
        assert_eq!(
            clean_camoufox_ua(ua).as_deref(),
            Some("Mozilla/5.0 (Windows NT 10.0; Win64; x64) Gecko/20100101 Firefox/133.0")
        );
    }

    #[test]
    fn leaves_clean_firefox_ua_untouched() {
        let ua = "Mozilla/5.0 (X11; Linux x86_64; rv:128.0) Gecko/20100101 Firefox/128.0";
        assert_eq!(clean_camoufox_ua(ua), None);
    }
}
