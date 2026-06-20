//! Session(HTTP)模式:不开浏览器,直接用 HTTP 请求 + 离线解析抓取——对标 DrissionPage 的
//! **会话(Session)模式**(`SessionPage`)。配合浏览器(Driver)模式即"Drission"双模:
//! 用浏览器过盾/登录,把 cookie 灌给 Session,之后列表/翻页/详情全走 HTTP——**省内存、省 CPU**,
//! 旧电脑也能高并发跑。
//!
//! ```ignore
//! // 纯 HTTP(完全不开浏览器):
//! let mut sess = SessionPage::new_default()?;
//! sess.get("https://example.com").await?;
//! println!("{} {}", sess.status(), sess.title()?);
//! let root = sess.s_root()?;                  // 解析一次,多次查询
//! for a in root.eles("tag:a")? { println!("{:?}", a.attr("href")?); }
//!
//! // 浏览器 → Session 的 cookie 交接(用浏览器过盾后,HTTP 接力):
//! let browser = Browser::launch_default().await?;
//! let tab = browser.latest_tab().await?;
//! tab.get("https://site.com/login").await?;   // 浏览器里登录/过盾
//! sess.load_cookies_from_tab(&tab).await?;     // 把 cookie 灌进 Session
//! sess.get("https://site.com/api/list").await?; // 之后纯 HTTP 接力
//! ```
//!
//! cookie 自管理(不依赖 reqwest 的不可枚举 jar),因此**双向互通**都顺手:浏览器→Session
//! (`load_cookies_from_tab`)、Session→浏览器(`apply_cookies_to_tab`)、以及存盘/读盘
//! (`save_cookies`/`load_cookies_file`)复用登录态。

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::browser::{Cookie, CookieParam, StaticElement, Tab};
use crate::launcher::Proxy;
use crate::{Error, Result};

/// 默认 UA(真实 Firefox;与 Driver 侧去 Camoufox 令牌后的形态一致)。
const DEFAULT_UA: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:135.0) Gecko/20100101 Firefox/135.0";

/// Session 模式选项(链式 builder)。
#[derive(Debug, Clone)]
pub struct SessionOptions {
    pub user_agent: String,
    /// 额外的默认请求头(每个请求都带)。
    pub headers: Vec<(String, String)>,
    pub proxy: Option<Proxy>,
    pub timeout: Duration,
    pub ignore_https_errors: bool,
    /// 最大重定向跟随次数。
    pub max_redirects: usize,
}

impl Default for SessionOptions {
    fn default() -> Self {
        Self {
            user_agent: DEFAULT_UA.to_string(),
            headers: Vec::new(),
            proxy: None,
            timeout: Duration::from_secs(30),
            ignore_https_errors: false,
            max_redirects: 10,
        }
    }
}

impl SessionOptions {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn user_agent(mut self, ua: impl Into<String>) -> Self {
        self.user_agent = ua.into();
        self
    }
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }
    pub fn proxy(mut self, proxy: Proxy) -> Self {
        self.proxy = Some(proxy);
        self
    }
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
    pub fn ignore_https_errors(mut self, yes: bool) -> Self {
        self.ignore_https_errors = yes;
        self
    }
    pub fn max_redirects(mut self, n: usize) -> Self {
        self.max_redirects = n;
        self
    }
}

/// POST 请求体。
pub enum PostData {
    /// `application/x-www-form-urlencoded` 表单。
    Form(Vec<(String, String)>),
    /// `application/json`。
    Json(Value),
    /// 原始字节体 + 可选 `Content-Type`。
    Raw(String, Option<String>),
}

/// 一个 HTTP 会话页(对标 DP `SessionPage`)。保存最近一次响应,并自管理 cookie。
pub struct SessionPage {
    client: reqwest::Client,
    jar: CookieJar,
    max_redirects: usize,
    last_url: String,
    last_status: u16,
    last_headers: Vec<(String, String)>,
    last_body: String,
}

impl SessionPage {
    /// 用默认选项创建。
    pub fn new_default() -> Result<Self> {
        Self::new(SessionOptions::default())
    }

    /// 用指定选项创建。
    pub fn new(opts: SessionOptions) -> Result<Self> {
        let mut default_headers = reqwest::header::HeaderMap::new();
        for (k, v) in &opts.headers {
            if let (Ok(name), Ok(val)) = (
                reqwest::header::HeaderName::from_bytes(k.as_bytes()),
                reqwest::header::HeaderValue::from_str(v),
            ) {
                default_headers.insert(name, val);
            }
        }
        // 重定向自己处理(为了每跳都应用/抓取 cookie),故关掉 reqwest 内置跟随。
        let mut builder = reqwest::Client::builder()
            .user_agent(opts.user_agent.clone())
            .timeout(opts.timeout)
            .redirect(reqwest::redirect::Policy::none())
            .default_headers(default_headers);
        if opts.ignore_https_errors {
            builder = builder.danger_accept_invalid_certs(true);
        }
        if let Some(p) = &opts.proxy {
            let mut pr = reqwest::Proxy::all(&p.server)?;
            if let (Some(u), Some(pw)) = (&p.username, &p.password) {
                pr = pr.basic_auth(u, pw);
            }
            builder = builder.proxy(pr);
        }
        let client = builder.build()?;
        Ok(Self {
            client,
            jar: CookieJar::default(),
            max_redirects: opts.max_redirects,
            last_url: String::new(),
            last_status: 0,
            last_headers: Vec::new(),
            last_body: String::new(),
        })
    }

    /// GET 请求。成功(2xx)返回 `true`。
    pub async fn get(&mut self, url: &str) -> Result<bool> {
        self.request(reqwest::Method::GET, url, None).await
    }

    /// POST 请求(表单/JSON/原始体)。成功(2xx)返回 `true`。
    pub async fn post(&mut self, url: &str, data: PostData) -> Result<bool> {
        self.request(reqwest::Method::POST, url, Some(data)).await
    }

    /// 发请求并手动跟随重定向(每跳都带上匹配 cookie、抓取 Set-Cookie)。
    async fn request(
        &mut self,
        method: reqwest::Method,
        url: &str,
        body: Option<PostData>,
    ) -> Result<bool> {
        let mut current =
            reqwest::Url::parse(url).map_err(|e| Error::Other(format!("非法 URL {url}: {e}")))?;
        let mut method = method;
        let mut body = body;
        let mut hops = 0usize;

        loop {
            let mut req = self.client.request(method.clone(), current.clone());
            if let Some(cookie) = self.jar.header_for(&current) {
                req = req.header(reqwest::header::COOKIE, cookie);
            }
            if let Some(b) = &body {
                req = match b {
                    PostData::Form(f) => req
                        .header(
                            reqwest::header::CONTENT_TYPE,
                            "application/x-www-form-urlencoded",
                        )
                        .body(form_encode(f)),
                    PostData::Json(j) => req.json(j),
                    PostData::Raw(s, ct) => {
                        let mut r = req.body(s.clone());
                        if let Some(c) = ct {
                            r = r.header(reqwest::header::CONTENT_TYPE, c.clone());
                        }
                        r
                    }
                };
            }

            let resp = req.send().await?;
            let status = resp.status();

            // 收下本跳的 Set-Cookie(供后续跳与导出浏览器使用)。
            for hv in resp.headers().get_all(reqwest::header::SET_COOKIE).iter() {
                if let Ok(s) = hv.to_str() {
                    self.jar.store(s, &current);
                }
            }

            // 重定向:解析 Location,继续下一跳。
            if status.is_redirection()
                && hops < self.max_redirects
                && let Some(loc) = resp
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|v| v.to_str().ok())
            {
                let next = current
                    .join(loc)
                    .map_err(|e| Error::Other(format!("非法重定向 Location {loc}: {e}")))?;
                hops += 1;
                current = next;
                // 301/302/303 → 改 GET 丢体;307/308 保持方法与体。
                let code = status.as_u16();
                if code != 307 && code != 308 {
                    method = reqwest::Method::GET;
                    body = None;
                }
                continue;
            }

            self.last_status = status.as_u16();
            self.last_url = current.to_string();
            self.last_headers = resp
                .headers()
                .iter()
                .filter_map(|(k, v)| {
                    v.to_str()
                        .ok()
                        .map(|s| (k.as_str().to_string(), s.to_string()))
                })
                .collect();
            self.last_body = resp.text().await.unwrap_or_default();
            return Ok(status.is_success());
        }
    }

    // ── 响应读取 ───────────────────────────────────────────────────────────

    /// 最近一次响应的正文(HTML/文本)。
    pub fn html(&self) -> &str {
        &self.last_body
    }
    /// 同 [`html`](Self::html)。
    pub fn text(&self) -> &str {
        &self.last_body
    }
    /// 最近一次响应状态码(未请求过为 0)。
    pub fn status(&self) -> u16 {
        self.last_status
    }
    /// 最近一次最终 URL(跟随重定向后)。
    pub fn url(&self) -> &str {
        &self.last_url
    }
    /// 最近一次响应头(原样,可能含重复名如 set-cookie)。
    pub fn headers(&self) -> &[(String, String)] {
        &self.last_headers
    }
    /// 取某个响应头(大小写不敏感)。
    pub fn header(&self, name: &str) -> Option<&str> {
        self.last_headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
    /// 把正文按 JSON 解析。
    pub fn json(&self) -> Result<Value> {
        Ok(serde_json::from_str(&self.last_body)?)
    }

    // ── 离线解析(复用静态元素,与 Driver 侧 `s_ele` 同语法)──────────────────

    /// 解析最近响应正文,返回根元素(解析一次、多次查询更高效)。
    pub fn s_root(&self) -> Result<StaticElement> {
        StaticElement::parse(&self.last_body)
    }
    /// 在最近响应里查单个元素(DP 定位语法)。
    pub fn s_ele(&self, selector: &str) -> Result<StaticElement> {
        self.s_root()?.ele(selector)
    }
    /// 在最近响应里查全部匹配元素。
    pub fn s_eles(&self, selector: &str) -> Result<Vec<StaticElement>> {
        self.s_root()?.eles(selector)
    }
    /// 页面标题(`<title>` 文本);没有则空串。
    pub fn title(&self) -> Result<String> {
        match self.s_root()?.ele("tag:title") {
            Ok(t) => t.text(),
            Err(_) => Ok(String::new()),
        }
    }

    // ── cookie 互通 ───────────────────────────────────────────────────────

    /// 当前会话的全部 cookie(可用于回灌浏览器或存盘)。
    pub fn cookies(&self) -> Vec<CookieParam> {
        self.jar.to_params()
    }
    /// 手动写入 cookie(覆盖同名同域同路径)。
    pub fn set_cookies(&mut self, cookies: Vec<CookieParam>) {
        for c in cookies {
            self.jar.upsert_param(c);
        }
    }
    /// 清空 cookie。
    pub fn clear_cookies(&mut self) {
        self.jar.cookies.clear();
    }

    /// **浏览器 → Session**:把某标签(其 BrowserContext)的 cookie 灌进本会话。
    /// 典型用法:浏览器里登录/过盾后,后续抓取改走 HTTP。
    pub async fn load_cookies_from_tab(&mut self, tab: &Tab) -> Result<()> {
        for c in tab.cookies().await? {
            self.jar.upsert_cookie(c);
        }
        Ok(())
    }

    /// **Session → 浏览器**:把本会话 cookie 回灌到某标签(其 BrowserContext)。
    pub async fn apply_cookies_to_tab(&self, tab: &Tab) -> Result<()> {
        tab.set_cookies(self.jar.to_params()).await
    }

    /// 把 cookie 存到磁盘(JSON),便于跨进程/重启复用登录态。
    pub fn save_cookies(&self, path: &str) -> Result<()> {
        std::fs::write(path, serde_json::to_string_pretty(&self.jar.cookies)?)?;
        Ok(())
    }
    /// 从磁盘读回 cookie(与现有合并)。
    pub fn load_cookies_file(&mut self, path: &str) -> Result<()> {
        let s = std::fs::read_to_string(path)?;
        let list: Vec<StoredCookie> = serde_json::from_str(&s)?;
        for c in list {
            self.jar.upsert(c);
        }
        Ok(())
    }
}

// ── 自管理 cookie jar ───────────────────────────────────────────────────────

#[derive(Default)]
struct CookieJar {
    cookies: Vec<StoredCookie>,
}

#[derive(Clone, Serialize, Deserialize)]
struct StoredCookie {
    name: String,
    value: String,
    /// 不带前导点的域名。
    domain: String,
    path: String,
    secure: bool,
    http_only: bool,
    /// Unix 秒;`None` 为会话 cookie(不过期)。
    expires: Option<f64>,
    /// `true`=精确主机匹配(无 Domain 属性);`false`=域及子域。
    host_only: bool,
}

impl CookieJar {
    fn upsert(&mut self, c: StoredCookie) {
        if let Some(e) = self
            .cookies
            .iter_mut()
            .find(|x| x.name == c.name && x.domain == c.domain && x.path == c.path)
        {
            *e = c;
        } else {
            self.cookies.push(c);
        }
    }

    fn upsert_cookie(&mut self, c: Cookie) {
        let host_only = !c.domain.starts_with('.');
        self.upsert(StoredCookie {
            name: c.name,
            value: c.value,
            domain: c.domain.trim_start_matches('.').to_ascii_lowercase(),
            path: if c.path.is_empty() {
                "/".into()
            } else {
                c.path
            },
            secure: c.secure,
            http_only: c.http_only,
            expires: if c.expires > 0.0 {
                Some(c.expires)
            } else {
                None
            },
            host_only,
        });
    }

    fn upsert_param(&mut self, c: CookieParam) {
        let raw_domain = c.domain.clone().unwrap_or_default();
        let host_only = c
            .domain
            .as_deref()
            .map(|d| !d.starts_with('.'))
            .unwrap_or(true);
        let domain = if raw_domain.is_empty() {
            c.url
                .as_deref()
                .and_then(|u| reqwest::Url::parse(u).ok())
                .and_then(|u| u.host_str().map(|h| h.to_string()))
                .unwrap_or_default()
        } else {
            raw_domain.trim_start_matches('.').to_string()
        }
        .to_ascii_lowercase();
        self.upsert(StoredCookie {
            name: c.name,
            value: c.value,
            domain,
            path: c.path.unwrap_or_else(|| "/".into()),
            secure: c.secure.unwrap_or(false),
            http_only: c.http_only.unwrap_or(false),
            expires: c.expires.filter(|e| *e > 0.0),
            host_only,
        });
    }

    fn to_params(&self) -> Vec<CookieParam> {
        self.cookies
            .iter()
            .map(|c| CookieParam {
                name: c.name.clone(),
                value: c.value.clone(),
                url: None,
                domain: Some(c.domain.clone()),
                path: Some(c.path.clone()),
                secure: Some(c.secure),
                http_only: Some(c.http_only),
                expires: c.expires,
            })
            .collect()
    }

    /// 解析一条 `Set-Cookie` 并写入(过期则删除对应 cookie)。
    fn store(&mut self, set_cookie: &str, req: &reqwest::Url) {
        let mut parts = set_cookie.split(';');
        let nv = match parts.next() {
            Some(s) => s.trim(),
            None => return,
        };
        let (name, value) = match nv.split_once('=') {
            Some((n, v)) => (n.trim().to_string(), v.trim().to_string()),
            None => return,
        };
        if name.is_empty() {
            return;
        }

        let mut domain = String::new();
        let mut path = String::new();
        let mut secure = false;
        let mut http_only = false;
        let expires: Option<f64> = None;
        let mut max_age: Option<f64> = None;
        for attr in parts {
            let attr = attr.trim();
            let (k, v) = match attr.split_once('=') {
                Some((k, v)) => (k.trim().to_ascii_lowercase(), v.trim().to_string()),
                None => (attr.to_ascii_lowercase(), String::new()),
            };
            match k.as_str() {
                "domain" => domain = v.trim_start_matches('.').to_ascii_lowercase(),
                "path" => path = v,
                "secure" => secure = true,
                "httponly" => http_only = true,
                "max-age" => max_age = v.parse::<f64>().ok().map(|s| now_unix() + s),
                _ => {} // Expires 日期解析略(现代站点多用 Max-Age;无则当会话 cookie)。
            }
        }

        let host_only = domain.is_empty();
        let domain = if domain.is_empty() {
            req.host_str().unwrap_or_default().to_ascii_lowercase()
        } else {
            domain
        };
        let path = if path.starts_with('/') {
            path
        } else {
            default_path(req)
        };
        let exp = max_age.or(expires);

        // Max-Age<=0 / 过期 → 删除。
        if let Some(e) = exp
            && e <= now_unix()
        {
            self.cookies
                .retain(|c| !(c.name == name && c.domain == domain && c.path == path));
            return;
        }
        self.upsert(StoredCookie {
            name,
            value,
            domain,
            path,
            secure,
            http_only,
            expires: exp,
            host_only,
        });
    }

    /// 为某 URL 构造 `Cookie` 请求头值(无匹配返回 `None`)。
    fn header_for(&self, url: &reqwest::Url) -> Option<String> {
        let host = url.host_str()?.to_ascii_lowercase();
        let path = url.path();
        let secure_ctx = url.scheme() == "https";
        let now = now_unix();

        let mut matched: Vec<&StoredCookie> = self
            .cookies
            .iter()
            .filter(|c| {
                if let Some(e) = c.expires
                    && e <= now
                {
                    return false;
                }
                if c.secure && !secure_ctx {
                    return false;
                }
                let domain_ok = if c.host_only {
                    host == c.domain
                } else {
                    host == c.domain || host.ends_with(&format!(".{}", c.domain))
                };
                domain_ok && path_match(path, &c.path)
            })
            .collect();
        if matched.is_empty() {
            return None;
        }
        // 路径更长的在前(RFC 6265)。
        matched.sort_by_key(|c| std::cmp::Reverse(c.path.len()));
        Some(
            matched
                .iter()
                .map(|c| format!("{}={}", c.name, c.value))
                .collect::<Vec<_>>()
                .join("; "),
        )
    }
}

fn now_unix() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// `application/x-www-form-urlencoded` 编码(自实现,不引第三方)。
fn form_encode(pairs: &[(String, String)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", percent_encode(k), percent_encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// cookie 默认 path = 请求路径的"目录"部分(末段去掉);无则 `/`。
fn default_path(url: &reqwest::Url) -> String {
    let p = url.path();
    match p.rfind('/') {
        None | Some(0) => "/".to_string(),
        Some(i) => p[..i].to_string(),
    }
}

/// RFC 6265 path-match:cookie-path 是请求路径的前缀(到 `/` 边界)。
fn path_match(req: &str, cookie: &str) -> bool {
    if cookie == req {
        return true;
    }
    if !req.starts_with(cookie) {
        return false;
    }
    cookie.ends_with('/') || req[cookie.len()..].starts_with('/')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(u: &str) -> reqwest::Url {
        reqwest::Url::parse(u).unwrap()
    }

    #[test]
    fn store_and_send_basic() {
        let mut jar = CookieJar::default();
        jar.store("sid=abc; Path=/; HttpOnly", &url("https://x.com/login"));
        let h = jar.header_for(&url("https://x.com/anything")).unwrap();
        assert_eq!(h, "sid=abc");
        // 不同域不发送。
        assert!(jar.header_for(&url("https://y.com/")).is_none());
    }

    #[test]
    fn secure_only_on_https() {
        let mut jar = CookieJar::default();
        jar.store("s=1; Secure", &url("https://x.com/"));
        assert!(jar.header_for(&url("http://x.com/")).is_none());
        assert_eq!(
            jar.header_for(&url("https://x.com/")).as_deref(),
            Some("s=1")
        );
    }

    #[test]
    fn domain_attr_includes_subdomains() {
        let mut jar = CookieJar::default();
        jar.store("t=2; Domain=x.com", &url("https://www.x.com/"));
        assert_eq!(
            jar.header_for(&url("https://api.x.com/")).as_deref(),
            Some("t=2")
        );
        assert_eq!(
            jar.header_for(&url("https://x.com/")).as_deref(),
            Some("t=2")
        );
    }

    #[test]
    fn max_age_zero_deletes() {
        let mut jar = CookieJar::default();
        jar.store("k=v; Path=/", &url("https://x.com/"));
        assert!(jar.header_for(&url("https://x.com/")).is_some());
        jar.store("k=; Path=/; Max-Age=0", &url("https://x.com/"));
        assert!(jar.header_for(&url("https://x.com/")).is_none());
    }

    #[test]
    fn path_scoping() {
        let mut jar = CookieJar::default();
        jar.store("a=1; Path=/admin", &url("https://x.com/admin/x"));
        assert!(jar.header_for(&url("https://x.com/")).is_none());
        assert_eq!(
            jar.header_for(&url("https://x.com/admin/y")).as_deref(),
            Some("a=1")
        );
    }

    #[test]
    fn interop_param_roundtrip() {
        let mut jar = CookieJar::default();
        jar.upsert_param(CookieParam {
            name: "u".into(),
            value: "1".into(),
            url: None,
            domain: Some(".x.com".into()),
            path: Some("/".into()),
            secure: Some(false),
            http_only: Some(true),
            expires: None,
        });
        let params = jar.to_params();
        assert_eq!(params.len(), 1);
        assert_eq!(params[0].name, "u");
        // .x.com → 子域可发送。
        assert_eq!(
            jar.header_for(&url("https://a.x.com/")).as_deref(),
            Some("u=1")
        );
    }
}
