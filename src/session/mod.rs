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
use crate::net::DataPacket;
use crate::{Error, Result};

mod http;
pub use http::BrowserProfile;
use http::{HttpBackend, RawResponse};

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
    /// 浏览器 TLS / JA3 / JA4 + HTTP2 指纹档(默认 [`BrowserProfile::None`] = 不伪装)。
    /// 设为某浏览器家族需开 `--features impersonate` 才实际生效(否则 `warn` 回退纯 reqwest)。
    pub profile: BrowserProfile,
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
            profile: BrowserProfile::None,
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
    /// 设置浏览器 TLS / JA3 / JA4 + HTTP2 指纹档(需 `--features impersonate` 生效)。
    ///
    /// ```
    /// use drission::prelude::{SessionOptions, BrowserProfile};
    /// let _ = SessionOptions::new().profile(BrowserProfile::Chrome);
    /// ```
    pub fn profile(mut self, profile: BrowserProfile) -> Self {
        self.profile = profile;
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
    backend: HttpBackend,
    /// 用户额外请求头(每个请求都附加;UA 由 plain 后端的 client 或 impersonate 模拟档提供)。
    extra_headers: Vec<(String, String)>,
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
    ///
    /// `opts.profile` 选了浏览器家族 + 开了 `impersonate` feature → 用 `wreq` 浏览器 TLS 指纹后端;
    /// 否则用纯 `reqwest`(默认行为)。用户额外头每请求附加;重定向由本模块逐跳处理(带 cookie)。
    pub fn new(opts: SessionOptions) -> Result<Self> {
        let extra_headers = opts.headers.clone();
        let backend = HttpBackend::build(&opts)?;
        Ok(Self {
            backend,
            extra_headers,
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
        self.request("GET", url, None).await
    }

    /// POST 请求(表单/JSON/原始体)。成功(2xx)返回 `true`。
    pub async fn post(&mut self, url: &str, data: PostData) -> Result<bool> {
        self.request("POST", url, Some(data)).await
    }

    /// 发请求并手动跟随重定向(每跳都带上匹配 cookie、抓取 Set-Cookie)。
    ///
    /// 体预处理成 `(正文, 可选 Content-Type)` 后,把「用户额外头」作为 base headers 交给
    /// [`run_loop`](Self::run_loop)(自动补 cookie / Content-Type)。
    async fn request(&mut self, method: &str, url: &str, body: Option<PostData>) -> Result<bool> {
        let body: Option<(String, Option<String>)> = match body {
            Some(PostData::Form(f)) => Some((
                form_encode(&f),
                Some("application/x-www-form-urlencoded".to_string()),
            )),
            Some(PostData::Json(j)) => Some((
                serde_json::to_string(&j)?,
                Some("application/json".to_string()),
            )),
            Some(PostData::Raw(s, ct)) => Some((s, ct)),
            None => None,
        };
        let base = self.extra_headers.clone();
        self.run_loop(method.to_string(), url, body, base, true)
            .await
    }

    /// 逐跳请求循环(后端无关、`request`/`replay` 共用):每跳用 `base_headers`,按需补 cookie /
    /// Content-Type,抓 Set-Cookie,跟随重定向,最终把响应落进 `last_*`。
    ///
    /// `add_jar_cookie=true` 时:当前 headers **不含** `Cookie` 才从 jar 注入(故重放可保留抓包原始
    /// Cookie、不重复)。`body` = `(正文, 可选 Content-Type)`;仅当 headers 无 `Content-Type` 才补。
    async fn run_loop(
        &mut self,
        method: String,
        url: &str,
        body: Option<(String, Option<String>)>,
        base_headers: Vec<(String, String)>,
        add_jar_cookie: bool,
    ) -> Result<bool> {
        let mut current =
            reqwest::Url::parse(url).map_err(|e| Error::Other(format!("非法 URL {url}: {e}")))?;
        let mut method = method;
        let mut body = body;
        let mut hops = 0usize;

        loop {
            let mut headers = base_headers.clone();
            let has_cookie = headers
                .iter()
                .any(|(k, _)| k.eq_ignore_ascii_case("cookie"));
            if add_jar_cookie && !has_cookie {
                if let Some(cookie) = self.jar.header_for(&current) {
                    headers.push(("Cookie".to_string(), cookie));
                }
            }
            if let Some((_, Some(ct))) = &body {
                let has_ct = headers
                    .iter()
                    .any(|(k, _)| k.eq_ignore_ascii_case("content-type"));
                if !has_ct {
                    headers.push(("Content-Type".to_string(), ct.clone()));
                }
            }
            let body_str = body.as_ref().map(|(b, _)| b.as_str());

            let resp: RawResponse = self
                .backend
                .send_once(&method, current.as_str(), &headers, body_str)
                .await?;

            // 收下本跳的 Set-Cookie(供后续跳与导出浏览器使用)。
            for (k, v) in &resp.headers {
                if k.eq_ignore_ascii_case("set-cookie") {
                    self.jar.store(v, &current);
                }
            }

            // 重定向:解析 Location,继续下一跳。
            let code = resp.status;
            if (300..=399).contains(&code)
                && hops < self.max_redirects
                && let Some(loc) = resp
                    .headers
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case("location"))
                    .map(|(_, v)| v.clone())
            {
                let next = current
                    .join(&loc)
                    .map_err(|e| Error::Other(format!("非法重定向 Location {loc}: {e}")))?;
                hops += 1;
                current = next;
                // 301/302/303 → 改 GET 丢体;307/308 保持方法与体。
                if code != 307 && code != 308 {
                    method = "GET".to_string();
                    body = None;
                }
                continue;
            }

            self.last_status = code;
            self.last_url = current.to_string();
            self.last_headers = resp.headers;
            self.last_body = resp.body;
            return Ok((200..=299).contains(&code));
        }
    }

    /// **抓包 → 重放闭环**:把 [`tab.listen()`](crate::cdp::ChromiumTab::listen) 抓到的
    /// [`DataPacket`] 转成可重放的请求——链式 `set`/`set_header`/`url`/`body` 覆盖字段后 `send()`,
    /// 复现签名后**立即验真**。走本会话既有 cookie / 重定向 / `impersonate`(wreq TLS/JA3 指纹)循环。
    ///
    /// 自动剔除逐跳头(host/content-length/connection/accept-encoding 等)与 HTTP/2 伪头,
    /// 保留签名头等业务头与原始 Cookie(jar 不重复注入)。
    ///
    /// ```ignore
    /// // 浏览器抓到带签名的请求 → 改时间戳重签重放验真:
    /// let pkt = tab.listen().wait(None).await?.unwrap();
    /// let ok = sess.replay(&pkt).set("t", &now_ms).set_header("x-ca-sign", &new_sign).send().await?;
    /// println!("{} {}", sess.status(), ok);
    /// ```
    pub fn replay(&mut self, packet: &DataPacket) -> ReplayBuilder<'_> {
        ReplayBuilder {
            session: self,
            method: packet.method.clone(),
            url: packet.url.clone(),
            headers: replay_filter_headers(&packet.request.headers),
            body: packet.request.post_data.clone(),
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

    /// **CDP 浏览器 → Session**:把某 Chromium 标签的 cookie(含 httpOnly)灌进本会话。
    ///
    /// 与 [`load_cookies_from_tab`](Self::load_cookies_from_tab) 等价,但接收 CDP 后端
    /// (Cloak-Browser / Chrome for Testing)的 [`ChromiumTab`](crate::cdp::ChromiumTab)。
    /// 典型用法:CDP 浏览器里登录/过盾后,后续抓取改走纯 HTTP。
    #[cfg(feature = "cdp")]
    pub async fn load_cookies_from_cdp_tab(&mut self, tab: &crate::cdp::ChromiumTab) -> Result<()> {
        let raw = tab.get_cookies().await?;
        let mut params = Vec::with_capacity(raw.len());
        for c in raw {
            let name = c["name"].as_str().unwrap_or_default().to_string();
            if name.is_empty() {
                continue;
            }
            params.push(CookieParam {
                name,
                value: c["value"].as_str().unwrap_or_default().to_string(),
                url: None,
                domain: c["domain"].as_str().map(str::to_string),
                path: c["path"].as_str().map(str::to_string),
                secure: c["secure"].as_bool(),
                http_only: c["httpOnly"].as_bool(),
                expires: c["expires"].as_f64(),
            });
        }
        self.set_cookies(params);
        Ok(())
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

// ── 抓包重放 builder ─────────────────────────────────────────────────────────

/// [`SessionPage::replay`] 返回的链式重放构建器。覆盖字段后 `send()` 发送(走会话的 cookie /
/// 重定向 / `impersonate` 循环)。
pub struct ReplayBuilder<'a> {
    session: &'a mut SessionPage,
    method: String,
    /// 完整 URL(含 query)。
    url: String,
    headers: Vec<(String, String)>,
    body: Option<String>,
}

impl ReplayBuilder<'_> {
    /// 覆盖请求方法(默认沿用抓包的)。
    pub fn method(mut self, method: impl Into<String>) -> Self {
        self.method = method.into();
        self
    }

    /// 覆盖完整 URL。
    pub fn url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }

    /// 覆盖请求体(POST/PUT 等)。
    pub fn body(mut self, body: impl Into<String>) -> Self {
        self.body = Some(body.into());
        self
    }

    /// upsert 一个**请求头**(同名覆盖,大小写不敏感)——改签名头等。
    pub fn set_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        let name = name.into();
        let value = value.into();
        if let Some(slot) = self
            .headers
            .iter_mut()
            .find(|(k, _)| k.eq_ignore_ascii_case(&name))
        {
            slot.1 = value;
        } else {
            self.headers.push((name, value));
        }
        self
    }

    /// 删一个请求头(大小写不敏感)。
    pub fn remove_header(mut self, name: &str) -> Self {
        self.headers.retain(|(k, _)| !k.eq_ignore_ascii_case(name));
        self
    }

    /// upsert URL 里的一个 **query 参数**(同名覆盖)——重放最常见的「改 t= 时间戳重签」。
    /// 值原样拼接(调用方自行 URL 编码;时间戳/数字无需编码)。
    pub fn set_query(mut self, key: &str, value: &str) -> Self {
        self.url = upsert_query(&self.url, key, value);
        self
    }

    /// [`set_query`](Self::set_query) 的别名(对齐设计稿 `replay(pkt).set("t", now)` 写法)。
    pub fn set(self, key: &str, value: &str) -> Self {
        self.set_query(key, value)
    }

    /// 发送重放请求,结果落进会话 `last_*`(`status()`/`text()`/`json()` 取),成功(2xx)返回 `true`。
    pub async fn send(self) -> Result<bool> {
        let ReplayBuilder {
            session,
            method,
            url,
            headers,
            body,
        } = self;
        // 重放体不强加 Content-Type(原始头里若有就用,run_loop 也不会重复补)。
        let body = body.map(|b| (b, None));
        session.run_loop(method, &url, body, headers, true).await
    }
}

/// 过滤重放请求头:剔除逐跳/自动管理头(host/content-length/connection/accept-encoding 等)与
/// HTTP/2 伪头(`:authority` 等),保留签名头、Cookie 等业务头(由后端按 URL 重新设置那些被剔除的)。
fn replay_filter_headers(headers: &[(String, String)]) -> Vec<(String, String)> {
    headers
        .iter()
        .filter(|(k, _)| {
            let low = k.to_ascii_lowercase();
            !low.starts_with(':')
                && !matches!(
                    low.as_str(),
                    "host"
                        | "content-length"
                        | "connection"
                        | "proxy-connection"
                        | "keep-alive"
                        | "transfer-encoding"
                        | "upgrade"
                        | "accept-encoding"
                )
        })
        .cloned()
        .collect()
}

/// 在 URL 上 upsert 一个 query 参数(存在则改值,不存在则追加)。值原样拼接。
fn upsert_query(url: &str, key: &str, value: &str) -> String {
    let (base, frag) = match url.split_once('#') {
        Some((b, f)) => (b, Some(f)),
        None => (url, None),
    };
    let (path, query) = match base.split_once('?') {
        Some((p, q)) => (p, q),
        None => (base, ""),
    };
    let mut pairs: Vec<(String, String)> = query
        .split('&')
        .filter(|s| !s.is_empty())
        .map(|kv| match kv.split_once('=') {
            Some((k, v)) => (k.to_string(), v.to_string()),
            None => (kv.to_string(), String::new()),
        })
        .collect();
    if let Some(slot) = pairs.iter_mut().find(|(k, _)| k == key) {
        slot.1 = value.to_string();
    } else {
        pairs.push((key.to_string(), value.to_string()));
    }
    let q = pairs
        .iter()
        .map(|(k, v)| {
            if v.is_empty() {
                k.clone()
            } else {
                format!("{k}={v}")
            }
        })
        .collect::<Vec<_>>()
        .join("&");
    let mut out = if q.is_empty() {
        path.to_string()
    } else {
        format!("{path}?{q}")
    };
    if let Some(f) = frag {
        out.push('#');
        out.push_str(f);
    }
    out
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
    fn upsert_query_add_update_and_keep_fragment() {
        // 无 query → 追加。
        assert_eq!(upsert_query("https://x/a", "t", "1"), "https://x/a?t=1");
        // 已有其它参数 → 追加(保序)。
        assert_eq!(
            upsert_query("https://x/a?b=2", "t", "1"),
            "https://x/a?b=2&t=1"
        );
        // 已有同名 → 改值(就地,不重复)。
        assert_eq!(
            upsert_query("https://x/a?t=0&b=2", "t", "9"),
            "https://x/a?t=9&b=2"
        );
        // 保留 fragment。
        assert_eq!(
            upsert_query("https://x/a?b=2#frag", "t", "1"),
            "https://x/a?b=2&t=1#frag"
        );
        // 无值参数原样(不强加 =)。
        assert_eq!(
            upsert_query("https://x/a?flag", "t", "1"),
            "https://x/a?flag&t=1"
        );
    }

    #[test]
    fn replay_filter_drops_hop_and_pseudo_headers() {
        let raw = vec![
            (":authority".to_string(), "x.com".to_string()),
            ("Host".to_string(), "x.com".to_string()),
            ("Content-Length".to_string(), "10".to_string()),
            ("Accept-Encoding".to_string(), "gzip, br".to_string()),
            ("X-Ca-Sign".to_string(), "abc".to_string()),
            ("Cookie".to_string(), "s=1".to_string()),
        ];
        let kept = replay_filter_headers(&raw);
        let names: Vec<String> = kept.iter().map(|(k, _)| k.to_ascii_lowercase()).collect();
        // 业务头/Cookie 保留。
        assert!(names.contains(&"x-ca-sign".to_string()));
        assert!(names.contains(&"cookie".to_string()));
        // 逐跳/伪头/自动头剔除。
        assert!(!names.iter().any(|n| n.starts_with(':')));
        assert!(!names.contains(&"host".to_string()));
        assert!(!names.contains(&"content-length".to_string()));
        assert!(!names.contains(&"accept-encoding".to_string()));
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
