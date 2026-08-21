//! 一等公民 [`ChromiumBrowserContext`]:一个账号 / 一条身份对应一个独立 CDP BrowserContext。
//!
//! Cookie / Cache 由 Chromium 原生隔离;UA / locale / timezone / headers / permissions / viewport
//! 在本 context 新开的每个 Tab 上应用。命名 context 在同一浏览器进程内复用。
//!
//! 仅用于你拥有或已获明确授权的系统上的账号与测试身份。

use serde_json::json;

use crate::cdp::attach::CdpAttach;
use crate::cdp::options::ChromiumContextOverride;
use crate::cdp::tab::ChromiumTab;
use crate::cdp::types::CookieParam;
use crate::{Error, Result};

/// Context 级身份规格:一个账号在浏览器里应看到的出口与环境。
#[derive(Debug, Clone, Default)]
pub struct ContextSpec {
    /// 可选稳定名(如 `account_001`);同名在同一 Browser 内复用同一 CDP context。
    pub name: Option<String>,
    pub proxy: Option<String>,
    pub proxy_bypass: Option<String>,
    pub user_agent: Option<String>,
    pub locale: Option<String>,
    pub timezone: Option<String>,
    pub extra_headers: Vec<(String, String)>,
    /// `(origin, permissions)`。origin 为空则不带 origin 授予。
    pub permissions: Vec<(String, Vec<String>)>,
    pub viewport: Option<(u32, u32)>,
}

impl ContextSpec {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }
    pub fn proxy(mut self, p: impl Into<String>) -> Self {
        self.proxy = Some(p.into());
        self
    }
    pub fn proxy_bypass(mut self, b: impl Into<String>) -> Self {
        self.proxy_bypass = Some(b.into());
        self
    }
    pub fn user_agent(mut self, ua: impl Into<String>) -> Self {
        self.user_agent = Some(ua.into());
        self
    }
    pub fn locale(mut self, l: impl Into<String>) -> Self {
        self.locale = Some(l.into());
        self
    }
    pub fn timezone(mut self, tz: impl Into<String>) -> Self {
        self.timezone = Some(tz.into());
        self
    }
    pub fn extra_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.extra_headers.push((name.into(), value.into()));
        self
    }
    pub fn extra_headers(mut self, headers: Vec<(String, String)>) -> Self {
        self.extra_headers = headers;
        self
    }
    pub fn grant(mut self, origin: impl Into<String>, permissions: &[&str]) -> Self {
        self.permissions.push((
            origin.into(),
            permissions.iter().map(|s| (*s).to_string()).collect(),
        ));
        self
    }
    pub fn viewport(mut self, width: u32, height: u32) -> Self {
        self.viewport = Some((width, height));
        self
    }

    pub fn from_override(ov: &ChromiumContextOverride) -> Self {
        Self {
            name: None,
            proxy: ov.proxy.clone(),
            proxy_bypass: ov.proxy_bypass.clone(),
            user_agent: ov.user_agent.clone(),
            locale: ov.locale.clone(),
            timezone: ov.timezone.clone(),
            extra_headers: Vec::new(),
            permissions: Vec::new(),
            viewport: None,
        }
    }
}

impl From<&ChromiumContextOverride> for ContextSpec {
    fn from(ov: &ChromiumContextOverride) -> Self {
        Self::from_override(ov)
    }
}

/// 一个独立的浏览器上下文(账号身份)。Clone 共享同一 CDP context。
#[derive(Clone)]
pub struct ChromiumBrowserContext {
    attach: CdpAttach,
    id: String,
    spec: ContextSpec,
}

impl ChromiumBrowserContext {
    pub(crate) async fn create(attach: CdpAttach, spec: ContextSpec) -> Result<Self> {
        if let Some(name) = spec.name.as_deref() {
            if let Some(id) = attach.lookup_named(name) {
                return Ok(Self { attach, id, spec });
            }
        }

        let mut params = json!({});
        if let Some(proxy) = &spec.proxy {
            params["proxyServer"] = json!(proxy);
            if let Some(b) = &spec.proxy_bypass {
                params["proxyBypassList"] = json!(b);
            }
        }
        let r = attach
            .conn
            .send("Target.createBrowserContext", params, None)
            .await?;
        let id = r["browserContextId"]
            .as_str()
            .ok_or_else(|| Error::msg("CDP: createBrowserContext 未返回 browserContextId"))?
            .to_string();
        if let Some(name) = spec.name.as_deref() {
            attach.register_named(name, &id);
        }
        let ctx = Self { attach, id, spec };
        ctx.apply_permissions().await;
        Ok(ctx)
    }

    /// CDP `browserContextId`。
    pub fn id(&self) -> &str {
        &self.id
    }

    /// 创建时的稳定名(如有)。
    pub fn name(&self) -> Option<&str> {
        self.spec.name.as_deref()
    }

    pub fn spec(&self) -> &ContextSpec {
        &self.spec
    }

    /// 在本 context 里开一个新标签(`about:blank` 或指定 URL)。
    /// 关掉该标签**不会**销毁 context。
    pub async fn new_tab(&self, url: Option<&str>) -> Result<ChromiumTab> {
        let mut tparams = json!({ "url": url.unwrap_or("about:blank") });
        tparams["browserContextId"] = json!(self.id);
        let r = self
            .attach
            .conn
            .send("Target.createTarget", tparams, None)
            .await?;
        let target_id = r["targetId"]
            .as_str()
            .ok_or_else(|| Error::msg("CDP: 创建标签无 targetId"))?
            .to_string();
        let tab = self
            .attach
            .attach_in_context(target_id, Some(self.id.clone()), false)
            .await?;
        self.apply_session(&tab).await;
        Ok(tab)
    }

    /// 为本 context 写入 cookie(不依赖当前页同源)。
    pub async fn set_cookies(&self, cookies: Vec<CookieParam>) -> Result<()> {
        let arr: Vec<serde_json::Value> = cookies
            .iter()
            .map(|c| {
                let mut o = json!({ "name": c.name, "value": c.value });
                if let Some(u) = &c.url {
                    o["url"] = json!(u);
                }
                if let Some(d) = &c.domain {
                    o["domain"] = json!(d);
                }
                if let Some(p) = &c.path {
                    o["path"] = json!(p);
                }
                if let Some(v) = c.secure {
                    o["secure"] = json!(v);
                }
                if let Some(v) = c.http_only {
                    o["httpOnly"] = json!(v);
                }
                if let Some(v) = c.expires {
                    o["expires"] = json!(v);
                }
                o
            })
            .collect();
        self.attach
            .conn
            .send(
                "Storage.setCookies",
                json!({ "cookies": arr, "browserContextId": self.id }),
                None,
            )
            .await?;
        Ok(())
    }

    /// 读取本 context 的全部 cookie。
    pub async fn cookies(&self) -> Result<Vec<crate::cdp::types::Cookie>> {
        let r = self
            .attach
            .conn
            .send(
                "Storage.getCookies",
                json!({ "browserContextId": self.id }),
                None,
            )
            .await?;
        let s = |c: &serde_json::Value, k: &str| {
            c.get(k)
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string()
        };
        Ok(r["cookies"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|c| crate::cdp::types::Cookie {
                name: s(c, "name"),
                value: s(c, "value"),
                domain: s(c, "domain"),
                path: s(c, "path"),
                expires: c
                    .get("expires")
                    .and_then(serde_json::Value::as_f64)
                    .unwrap_or(-1.0),
                http_only: c
                    .get("httpOnly")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
                secure: c
                    .get("secure")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
            })
            .collect())
    }

    /// 销毁本 context 及其所有标签。命名项从浏览器注册表移除。
    pub async fn close(self) -> Result<()> {
        if let Some(name) = self.spec.name.as_deref() {
            self.attach.unregister_named(name);
        }
        self.attach
            .conn
            .send(
                "Target.disposeBrowserContext",
                json!({ "browserContextId": self.id }),
                None,
            )
            .await?;
        Ok(())
    }

    async fn apply_permissions(&self) {
        for (origin, perms) in &self.spec.permissions {
            let mut p = json!({ "permissions": perms, "browserContextId": self.id });
            if !origin.is_empty() {
                p["origin"] = json!(origin);
            }
            let _ = self
                .attach
                .conn
                .send("Browser.grantPermissions", p, None)
                .await;
        }
    }

    async fn apply_session(&self, tab: &ChromiumTab) {
        let spec = ChromiumContextOverride {
            proxy: None,
            proxy_bypass: None,
            user_agent: self.spec.user_agent.clone(),
            locale: self.spec.locale.clone(),
            timezone: self.spec.timezone.clone(),
        };
        spec.apply_emulation(tab).await;
        if !self.spec.extra_headers.is_empty() {
            let mut obj = serde_json::Map::new();
            for (k, v) in &self.spec.extra_headers {
                obj.insert(k.clone(), json!(v));
            }
            let _ = tab.core.send("Network.enable", json!({})).await;
            let _ = tab
                .core
                .send(
                    "Network.setExtraHTTPHeaders",
                    json!({ "headers": serde_json::Value::Object(obj) }),
                )
                .await;
        }
        if let Some((w, h)) = self.spec.viewport {
            let _ = tab
                .core
                .send(
                    "Emulation.setDeviceMetricsOverride",
                    json!({
                        "width": w,
                        "height": h,
                        "deviceScaleFactor": 1,
                        "mobile": false
                    }),
                )
                .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_builder_chains() {
        let s = ContextSpec::new()
            .name("account_001")
            .proxy("http://127.0.0.1:8080")
            .user_agent("UA/1")
            .locale("zh-CN")
            .timezone("Asia/Shanghai")
            .extra_header("X-Test", "1")
            .grant("https://example.com", &["geolocation"])
            .viewport(1280, 800);
        assert_eq!(s.name.as_deref(), Some("account_001"));
        assert_eq!(s.proxy.as_deref(), Some("http://127.0.0.1:8080"));
        assert_eq!(s.extra_headers.len(), 1);
        assert_eq!(s.permissions.len(), 1);
        assert_eq!(s.viewport, Some((1280, 800)));
    }
}
