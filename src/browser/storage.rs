//! 登录态 / 会话状态持久化(对标 Playwright `storageState`):
//! 把 **cookie + localStorage + sessionStorage** 一并导出/导入,跨进程或重启复用登录态。
//!
//! ```ignore
//! // 存:登录/过盾后导出整套状态到磁盘
//! tab.save_storage_state("state.json").await?;
//!
//! // 取:下次先导航到站点,再灌回 cookie + storage,然后刷新即"已登录"
//! tab.set_cookies(/* 或直接 */).await?;
//! tab.get("https://site.com").await?;
//! tab.load_storage_state("state.json").await?;   // cookie 全量 + 当前源的 storage
//! tab.get("https://site.com").await?;            // 刷新使页面读到已恢复状态
//! ```
//!
//! 说明:cookie 是浏览器级(BrowserContext),导入即全量生效;而 localStorage/sessionStorage
//! 受**同源**约束,只能在"当前页正处于该源"时写入,故 `load_storage_state` 只应用与当前源
//! 匹配的那一组 storage(cookie 不受此限,始终全量应用)。

use serde::{Deserialize, Serialize};

use crate::Result;
use crate::browser::Cookie;
use crate::browser::tab::{CookieParam, Tab};

/// 一整套可持久化的登录态:cookie + 各源的 web storage。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StorageState {
    #[serde(default)]
    pub cookies: Vec<CookieParam>,
    #[serde(default)]
    pub origins: Vec<OriginStorage>,
}

/// 某个源(`https://host[:port]`)下的 web storage。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OriginStorage {
    pub origin: String,
    #[serde(default)]
    pub local_storage: Vec<(String, String)>,
    #[serde(default)]
    pub session_storage: Vec<(String, String)>,
}

impl Tab {
    /// 导出当前登录态:**全部 cookie** + **当前源**的 localStorage / sessionStorage。
    pub async fn storage_state(&self) -> Result<StorageState> {
        let cookies = self
            .cookies()
            .await?
            .into_iter()
            .map(cookie_to_param)
            .collect();
        let origin = self
            .run_js("location.origin")
            .await?
            .as_str()
            .unwrap_or_default()
            .to_string();
        let local = read_storage(self, "localStorage").await;
        let session = read_storage(self, "sessionStorage").await;
        let mut origins = Vec::new();
        if !origin.is_empty() && origin != "null" && (!local.is_empty() || !session.is_empty()) {
            origins.push(OriginStorage {
                origin,
                local_storage: local,
                session_storage: session,
            });
        }
        Ok(StorageState { cookies, origins })
    }

    /// 导出并存盘(JSON)。
    pub async fn save_storage_state(&self, path: &str) -> Result<()> {
        let st = self.storage_state().await?;
        std::fs::write(path, serde_json::to_string_pretty(&st)?)?;
        Ok(())
    }

    /// 从磁盘读回并应用(cookie 全量;storage 仅应用与**当前源**匹配的那组)。
    pub async fn load_storage_state(&self, path: &str) -> Result<()> {
        let s = std::fs::read_to_string(path)?;
        let st: StorageState = serde_json::from_str(&s)?;
        self.apply_storage_state(&st).await
    }

    /// 应用一个 [`StorageState`](应用语义同 [`load_storage_state`](Self::load_storage_state))。
    pub async fn apply_storage_state(&self, state: &StorageState) -> Result<()> {
        if !state.cookies.is_empty() {
            self.set_cookies(state.cookies.clone()).await?;
        }
        let origin = self
            .run_js("location.origin")
            .await?
            .as_str()
            .unwrap_or_default()
            .to_string();
        if let Some(os) = state.origins.iter().find(|o| o.origin == origin) {
            write_storage(self, "localStorage", &os.local_storage).await?;
            write_storage(self, "sessionStorage", &os.session_storage).await?;
        }
        Ok(())
    }
}

fn cookie_to_param(c: Cookie) -> CookieParam {
    CookieParam {
        name: c.name,
        value: c.value,
        url: None,
        domain: Some(c.domain),
        path: Some(c.path),
        secure: Some(c.secure),
        http_only: Some(c.http_only),
        expires: if c.expires > 0.0 {
            Some(c.expires)
        } else {
            None
        },
    }
}

/// 读 `localStorage`/`sessionStorage` 全部键值(opaque 源会抛 SecurityError,已兜底为空)。
async fn read_storage(tab: &Tab, which: &str) -> Vec<(String, String)> {
    let js = format!(
        "(function(){{ try {{ var s={which}; var o=[]; for(var i=0;i<s.length;i++){{ var k=s.key(i); o.push([k, s.getItem(k)]); }} return JSON.stringify(o); }} catch(e){{ return '[]'; }} }})()"
    );
    let v = match tab.run_js(&js).await {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let s = v.as_str().unwrap_or("[]");
    serde_json::from_str::<Vec<[String; 2]>>(s)
        .map(|rows| rows.into_iter().map(|[k, v]| (k, v)).collect())
        .unwrap_or_default()
}

/// 把键值写回 `localStorage`/`sessionStorage`(同源前提下;非同源/opaque 会被 try 兜掉)。
async fn write_storage(tab: &Tab, which: &str, items: &[(String, String)]) -> Result<()> {
    if items.is_empty() {
        return Ok(());
    }
    let json = serde_json::to_string(items).unwrap_or_else(|_| "[]".into());
    let js = format!(
        "(function(){{ try {{ var d={json}; for(var i=0;i<d.length;i++){{ {which}.setItem(d[i][0], d[i][1]); }} }} catch(e){{}} return true; }})()"
    );
    tab.run_js(&js).await?;
    Ok(())
}
