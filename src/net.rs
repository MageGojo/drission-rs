//! 后端无关的**网络数据类型**(监听 / 拦截共享)。
//!
//! Camoufox(Juggler)与 Chromium(CDP)两后端的网络监听 / 请求拦截 API 一致,数据结构在此
//! 统一定义、始终编译;各后端的具体抓取 / 拦截实现仍在各自模块里(`browser::{listener,interceptor}`
//! 与 `cdp::{listener,interceptor}`)。
//!
//! - [`DataPacket`](监听到的一个请求+响应)与其 [`RequestData`]/[`ResponseData`];
//! - [`ListenFilter`](URL / 类型过滤);
//! - [`ResumeOptions`](改写放行请求时的可选覆盖字段)。
//!
//! `ListenFilter::matches` 仅被 cdp / camoufox 的拦截器调用;两后端都不开时它闲置,故在该退化
//! 配置下允许 dead_code(其余类型是公开 API,本就不触发 dead_code)。
#![cfg_attr(not(any(feature = "cdp", feature = "camoufox")), allow(dead_code))]

use serde_json::Value;

/// 请求侧数据。
#[derive(Debug, Clone, Default)]
pub struct RequestData {
    pub headers: Vec<(String, String)>,
    pub post_data: Option<String>,
}

/// 响应侧数据。`body` 为文本响应体;`body_base64` 保留字段(hook 模式下通常为空)。
#[derive(Debug, Clone, Default)]
pub struct ResponseData {
    pub status: u16,
    pub status_text: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
    pub body_base64: String,
}

/// 一个被监听到的网络数据包(请求 + 响应)。
#[derive(Debug, Clone)]
pub struct DataPacket {
    pub url: String,
    pub method: String,
    /// 资源类型(`fetch`/`xhr`)。
    pub resource_type: String,
    pub request: RequestData,
    pub response: ResponseData,
}

impl DataPacket {
    /// 请求 URL 的 path 部分(去掉 `?query`)。
    pub fn path(&self) -> &str {
        self.url.split('?').next().unwrap_or(&self.url)
    }

    /// URL 是否包含某子串(便捷过滤)。
    pub fn url_has(&self, needle: &str) -> bool {
        self.url.contains(needle)
    }

    /// 取 URL query 中某参数的**原始值**(未 URL 解码,即上线值);不存在返回 `None`。
    pub fn query(&self, key: &str) -> Option<String> {
        let q = self.url.split_once('?')?.1;
        q.split('&').find_map(|kv| {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            (k == key).then(|| v.to_string())
        })
    }

    /// URL query 的全部键值对(原始值)。
    pub fn queries(&self) -> Vec<(String, String)> {
        let Some((_, q)) = self.url.split_once('?') else {
            return Vec::new();
        };
        q.split('&')
            .filter(|s| !s.is_empty())
            .map(|kv| {
                let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
                (k.to_string(), v.to_string())
            })
            .collect()
    }

    /// 把响应体按 JSON 解析;非 JSON 返回 `None`。
    pub fn json(&self) -> Option<Value> {
        serde_json::from_str(&self.response.body).ok()
    }
}

/// 监听过滤条件。
#[derive(Debug, Clone, Default)]
pub struct ListenFilter {
    /// URL 子串集合;为空表示匹配所有 URL。
    pub url_keywords: Vec<String>,
    /// 仅匹配 XHR/fetch 类请求(hook 模式天然只覆盖 fetch/XHR)。
    pub xhr_only: bool,
}

impl ListenFilter {
    /// URL 与资源类型是否同时匹配(供请求拦截复用)。
    pub(crate) fn matches(&self, url: &str, resource_type: &str) -> bool {
        self.url_matches(url) && self.type_matches(resource_type)
    }

    fn url_matches(&self, url: &str) -> bool {
        self.url_keywords.is_empty() || self.url_keywords.iter().any(|k| url.contains(k))
    }

    fn type_matches(&self, resource_type: &str) -> bool {
        if !self.xhr_only {
            return true;
        }
        let t = resource_type.to_ascii_lowercase();
        t.contains("xhr") || t.contains("fetch") || t.contains("xmlhttprequest")
    }
}

/// 改写放行时可覆盖的字段(均为可选;`None` 表示保持原值)。
#[derive(Debug, Clone, Default)]
pub struct ResumeOptions {
    pub url: Option<String>,
    pub method: Option<String>,
    pub headers: Option<Vec<(String, String)>>,
    pub post_data: Option<String>,
}

impl ResumeOptions {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn url(mut self, url: impl Into<String>) -> Self {
        self.url = Some(url.into());
        self
    }
    pub fn method(mut self, method: impl Into<String>) -> Self {
        self.method = Some(method.into());
        self
    }
    pub fn headers(mut self, headers: Vec<(String, String)>) -> Self {
        self.headers = Some(headers);
        self
    }
    pub fn post_data(mut self, post_data: impl Into<String>) -> Self {
        self.post_data = Some(post_data.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_matches() {
        let f = ListenFilter {
            url_keywords: vec!["/api/".into()],
            xhr_only: false,
        };
        assert!(f.matches("https://x.com/api/v1", "fetch"));
        assert!(!f.matches("https://x.com/static.js", "fetch"));
        let all = ListenFilter::default();
        assert!(all.matches("anything", "document"));
    }

    #[test]
    fn datapacket_url_and_json_helpers() {
        let p = DataPacket {
            url: "https://x.com/api/detail/?aweme_id=123&a_bogus=ZZ%2F1".into(),
            method: "GET".into(),
            resource_type: "xhr".into(),
            request: RequestData::default(),
            response: ResponseData {
                body: r#"{"aweme_list":[{"aweme_id":"a1"},{"aweme_id":"a2"}]}"#.into(),
                ..Default::default()
            },
        };
        assert_eq!(p.path(), "https://x.com/api/detail/");
        assert!(p.url_has("aweme_id="));
        assert_eq!(p.query("aweme_id").as_deref(), Some("123"));
        assert_eq!(p.query("a_bogus").as_deref(), Some("ZZ%2F1")); // 原始(未解码)
        assert_eq!(p.query("missing"), None);
        assert_eq!(p.queries().len(), 2);
        let j = p.json().unwrap();
        assert_eq!(j["aweme_list"][1]["aweme_id"], "a2");
    }

    #[test]
    fn resume_options_builder() {
        let o = ResumeOptions::new()
            .url("https://x.com")
            .method("POST")
            .headers(vec![("X-A".into(), "1".into())])
            .post_data("body");
        assert_eq!(o.url.as_deref(), Some("https://x.com"));
        assert_eq!(o.method.as_deref(), Some("POST"));
        assert_eq!(o.headers.unwrap()[0].0, "X-A");
        assert_eq!(o.post_data.as_deref(), Some("body"));
    }
}
