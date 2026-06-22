//! Session 的 HTTP 后端抽象:纯 `reqwest`(默认)/ `wreq` 浏览器 TLS 指纹(`impersonate`)。
//!
//! 把"发一次请求"收敛成 [`HttpBackend::send_once`],返回**急取**的 [`RawResponse`]
//! (状态 + 全部响应头(含重复 `set-cookie`)+ 正文),让 [`SessionPage`](super::SessionPage)
//! 的重定向 / cookie 循环**完全后端无关、只写一份**。
//!
//! 设计见 `docs/TLS指纹.md`。

use super::SessionOptions;
use crate::Result;

/// 浏览器 TLS / JA3 / JA4 + HTTP2 指纹档(后端无关;实际生效需 `impersonate` feature)。
///
/// 选一个浏览器家族,本库按该家族**一个较新的版本**对齐其 TLS + HTTP2 指纹
/// (映射集中在内部 `profile_to_emulation`,随 `wreq-util` 升级一键上调)。
/// [`None`](BrowserProfile::None)(默认)= 不伪装,走纯 `reqwest`(行为与历史一致)。
///
/// ```
/// use drission::prelude::BrowserProfile;
/// assert_eq!(BrowserProfile::default(), BrowserProfile::None);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BrowserProfile {
    /// 不伪装 TLS 指纹(默认,纯 `reqwest`)。
    #[default]
    None,
    /// 对齐 Google Chrome 的 TLS + HTTP2 指纹。
    Chrome,
    /// 对齐 Mozilla Firefox。
    Firefox,
    /// 对齐 Apple Safari。
    Safari,
    /// 对齐 Microsoft Edge。
    Edge,
}

/// 一次 HTTP 应答(急取:状态码 + 全部响应头(保留重复名,如多条 `set-cookie`)+ 正文)。
pub(crate) struct RawResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

/// Session 用的 HTTP 客户端后端。
pub(crate) enum HttpBackend {
    /// 纯 `reqwest`(始终可用,零额外依赖)。
    Plain(reqwest::Client),
    /// `wreq` 浏览器 TLS 指纹客户端(`profile != None` 且开了 `impersonate`)。
    #[cfg(feature = "impersonate")]
    Impersonate(Box<wreq::Client>),
}

impl HttpBackend {
    /// 按选项构建后端:`profile != None` 且开了 `impersonate` → wreq 指纹客户端;否则纯 reqwest。
    ///
    /// 设了 `profile` 却没编 `impersonate` 时:`warn` 一次并**优雅回退**纯 reqwest(不报错)。
    pub(crate) fn build(opts: &SessionOptions) -> Result<Self> {
        #[cfg(feature = "impersonate")]
        if opts.profile != BrowserProfile::None {
            return Ok(HttpBackend::Impersonate(Box::new(build_impersonate(opts)?)));
        }
        #[cfg(not(feature = "impersonate"))]
        if opts.profile != BrowserProfile::None {
            tracing::warn!(
                "SessionOptions.profile 已设但未启用 `impersonate` feature → 回退纯 reqwest(无 TLS 指纹);\
                 如需生效请加 `--features impersonate`。"
            );
        }
        Ok(HttpBackend::Plain(build_plain(opts)?))
    }

    /// 发一次请求(**不**跟随重定向,交由上层 cookie 循环处理),急取状态 / 全部头 / 正文。
    pub(crate) async fn send_once(
        &self,
        method: &str,
        url: &str,
        headers: &[(String, String)],
        body: Option<&str>,
    ) -> Result<RawResponse> {
        match self {
            HttpBackend::Plain(c) => {
                let m =
                    reqwest::Method::from_bytes(method.as_bytes()).unwrap_or(reqwest::Method::GET);
                let mut req = c.request(m, url);
                for (k, v) in headers {
                    req = req.header(k.as_str(), v.as_str());
                }
                if let Some(b) = body {
                    req = req.body(b.to_string());
                }
                let resp = req.send().await?;
                let status = resp.status().as_u16();
                let headers = collect_headers_reqwest(resp.headers());
                let body = resp.text().await.unwrap_or_default();
                Ok(RawResponse {
                    status,
                    headers,
                    body,
                })
            }
            #[cfg(feature = "impersonate")]
            HttpBackend::Impersonate(c) => {
                let m = wreq::Method::from_bytes(method.as_bytes()).unwrap_or(wreq::Method::GET);
                let mut req = c.request(m, url);
                for (k, v) in headers {
                    req = req.header(k.as_str(), v.as_str());
                }
                if let Some(b) = body {
                    req = req.body(b.to_string());
                }
                let resp = req.send().await?;
                let status = resp.status().as_u16();
                let headers = collect_headers_wreq(resp.headers());
                let body = resp.text().await.unwrap_or_default();
                Ok(RawResponse {
                    status,
                    headers,
                    body,
                })
            }
        }
    }
}

/// 构建纯 reqwest 客户端(关内置重定向:cookie 循环每跳自己处理)。
fn build_plain(opts: &SessionOptions) -> Result<reqwest::Client> {
    let mut b = reqwest::Client::builder()
        .user_agent(opts.user_agent.clone())
        .timeout(opts.timeout)
        .redirect(reqwest::redirect::Policy::none());
    if opts.ignore_https_errors {
        b = b.danger_accept_invalid_certs(true);
    }
    if let Some(p) = &opts.proxy {
        let mut pr = reqwest::Proxy::all(&p.server)?;
        if let (Some(u), Some(pw)) = (&p.username, &p.password) {
            pr = pr.basic_auth(u, pw);
        }
        b = b.proxy(pr);
    }
    Ok(b.build()?)
}

fn collect_headers_reqwest(h: &reqwest::header::HeaderMap) -> Vec<(String, String)> {
    h.iter()
        .filter_map(|(k, v)| {
            v.to_str()
                .ok()
                .map(|s| (k.as_str().to_string(), s.to_string()))
        })
        .collect()
}

// ── impersonate(wreq)分支 ───────────────────────────────────────────────────

/// 构建 wreq 指纹客户端:UA + 默认头由**模拟档**驱动(故不设 `opts.user_agent`,避免与 TLS 指纹打架)。
#[cfg(feature = "impersonate")]
fn build_impersonate(opts: &SessionOptions) -> Result<wreq::Client> {
    let mut b = wreq::Client::builder()
        .emulation(profile_to_emulation(opts.profile))
        .timeout(opts.timeout)
        .redirect(wreq::redirect::Policy::none());
    if opts.ignore_https_errors {
        // wreq 把 reqwest 的 danger_accept_invalid_certs 改名为 cert_verification(false)。
        b = b.cert_verification(false);
    }
    if let Some(p) = &opts.proxy {
        let mut pr = wreq::Proxy::all(&p.server)?;
        if let (Some(u), Some(pw)) = (&p.username, &p.password) {
            pr = pr.basic_auth(u, pw);
        }
        b = b.proxy(pr);
    }
    Ok(b.build()?)
}

/// 档 → `wreq-util` 模拟档(集中一处;升级 `wreq-util` 时只改这里的版本号)。
#[cfg(feature = "impersonate")]
fn profile_to_emulation(p: BrowserProfile) -> wreq_util::Emulation {
    use wreq_util::Emulation;
    match p {
        // None 不会走到这里(build 已分流);兜底给 Chrome。
        BrowserProfile::None | BrowserProfile::Chrome => Emulation::Chrome137,
        BrowserProfile::Firefox => Emulation::Firefox139,
        BrowserProfile::Safari => Emulation::Safari18_5,
        BrowserProfile::Edge => Emulation::Edge134,
    }
}

#[cfg(feature = "impersonate")]
fn collect_headers_wreq(h: &wreq::header::HeaderMap) -> Vec<(String, String)> {
    h.iter()
        .filter_map(|(k, v)| {
            v.to_str()
                .ok()
                .map(|s| (k.as_str().to_string(), s.to_string()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_default_is_none() {
        assert_eq!(BrowserProfile::default(), BrowserProfile::None);
    }

    #[test]
    fn profile_variants_distinct() {
        // 防回归:几个家族互不相等(避免重构时手滑写成同一个)。
        let all = [
            BrowserProfile::None,
            BrowserProfile::Chrome,
            BrowserProfile::Firefox,
            BrowserProfile::Safari,
            BrowserProfile::Edge,
        ];
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(a, b);
            }
        }
    }

    #[cfg(feature = "impersonate")]
    #[test]
    fn each_profile_maps_to_distinct_emulation() {
        // 每个家族映射到不同的模拟档(防手滑把 Edge 也写成 Chrome137 之类)。
        let chrome = profile_to_emulation(BrowserProfile::Chrome);
        let firefox = profile_to_emulation(BrowserProfile::Firefox);
        let safari = profile_to_emulation(BrowserProfile::Safari);
        let edge = profile_to_emulation(BrowserProfile::Edge);
        assert_ne!(chrome, firefox);
        assert_ne!(chrome, safari);
        assert_ne!(chrome, edge);
        assert_ne!(firefox, safari);
    }
}
