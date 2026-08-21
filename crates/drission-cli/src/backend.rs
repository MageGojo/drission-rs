use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Result, anyhow};
use base64::Engine;
use drission::prelude::FingerprintProbe;
use serde_json::{Value, json};

use crate::paths;
use crate::protocol::{AxFormat, BackendKind, PacketSummary};

pub enum BackendBrowser {
    #[cfg(feature = "cdp")]
    Cdp(drission::cdp::ChromiumBrowser),
    #[cfg(feature = "camoufox")]
    Camoufox(drission::browser::Browser),
}

#[derive(Clone)]
pub enum BackendTab {
    #[cfg(feature = "cdp")]
    Cdp(drission::cdp::ChromiumTab),
    #[cfg(feature = "camoufox")]
    Camoufox(drission::browser::Tab),
}

impl BackendBrowser {
    pub async fn launch(
        backend: BackendKind,
        headless: bool,
        user_data_dir: Option<PathBuf>,
    ) -> Result<Self> {
        match backend {
            BackendKind::Cdp => launch_cdp(headless, user_data_dir).await,
            BackendKind::Camoufox => launch_camoufox(headless, user_data_dir).await,
        }
    }

    pub async fn new_tab(&self, url: Option<&str>) -> Result<BackendTab> {
        match self {
            #[cfg(feature = "cdp")]
            BackendBrowser::Cdp(browser) => Ok(BackendTab::Cdp(browser.new_tab(url).await?)),
            #[cfg(feature = "camoufox")]
            BackendBrowser::Camoufox(browser) => {
                Ok(BackendTab::Camoufox(browser.new_tab(url).await?))
            }
        }
    }

    pub async fn quit(&self) -> Result<()> {
        match self {
            #[cfg(feature = "cdp")]
            BackendBrowser::Cdp(browser) => browser.quit().await?,
            #[cfg(feature = "camoufox")]
            BackendBrowser::Camoufox(browser) => browser.quit().await?,
        }
        Ok(())
    }

    pub fn list_contexts(&self) -> Result<Value> {
        match self {
            #[cfg(feature = "cdp")]
            BackendBrowser::Cdp(browser) => {
                let contexts: Vec<Value> = browser
                    .named_contexts()
                    .into_iter()
                    .map(|(name, id)| json!({ "name": name, "id": id }))
                    .collect();
                Ok(json!({ "contexts": contexts }))
            }
            #[cfg(feature = "camoufox")]
            BackendBrowser::Camoufox(_) => {
                anyhow::bail!("context 只支持 CDP 后端")
            }
        }
    }

    pub async fn ensure_context(&self, spec: ContextUseSpec<'_>) -> Result<Value> {
        match self {
            #[cfg(feature = "cdp")]
            BackendBrowser::Cdp(browser) => {
                let ctx = browser.context_with(spec.into_spec()).await?;
                Ok(json!({
                    "name": ctx.name(),
                    "id": ctx.id(),
                }))
            }
            #[cfg(feature = "camoufox")]
            BackendBrowser::Camoufox(_) => {
                let _ = spec;
                anyhow::bail!("context 只支持 CDP 后端")
            }
        }
    }

    pub async fn new_tab_in_context(
        &self,
        spec: ContextUseSpec<'_>,
        url: Option<&str>,
    ) -> Result<BackendTab> {
        match self {
            #[cfg(feature = "cdp")]
            BackendBrowser::Cdp(browser) => {
                let ctx = browser.context_with(spec.into_spec()).await?;
                Ok(BackendTab::Cdp(ctx.new_tab(url).await?))
            }
            #[cfg(feature = "camoufox")]
            BackendBrowser::Camoufox(_) => {
                let _ = (spec, url);
                anyhow::bail!("context 只支持 CDP 后端")
            }
        }
    }

    pub async fn close_context(&self, name: &str) -> Result<()> {
        match self {
            #[cfg(feature = "cdp")]
            BackendBrowser::Cdp(browser) => {
                let ctx = browser.context(name).await?;
                ctx.close().await?;
                Ok(())
            }
            #[cfg(feature = "camoufox")]
            BackendBrowser::Camoufox(_) => {
                let _ = name;
                anyhow::bail!("context 只支持 CDP 后端")
            }
        }
    }
}

pub struct ContextUseSpec<'a> {
    pub name: &'a str,
    pub proxy: Option<&'a str>,
    pub user_agent: Option<&'a str>,
    pub locale: Option<&'a str>,
    pub timezone: Option<&'a str>,
    pub viewport: Option<(u32, u32)>,
}

impl ContextUseSpec<'_> {
    #[cfg(feature = "cdp")]
    fn into_spec(self) -> drission::cdp::ContextSpec {
        let mut spec = drission::cdp::ContextSpec::new().name(self.name);
        if let Some(p) = self.proxy {
            spec = spec.proxy(p);
        }
        if let Some(ua) = self.user_agent {
            spec = spec.user_agent(ua);
        }
        if let Some(l) = self.locale {
            spec = spec.locale(l);
        }
        if let Some(tz) = self.timezone {
            spec = spec.timezone(tz);
        }
        if let Some((w, h)) = self.viewport {
            spec = spec.viewport(w, h);
        }
        spec
    }
}

pub enum BackendRoute {
    #[cfg(feature = "cdp")]
    Cdp(drission::cdp::NetworkRoute),
}

impl BackendRoute {
    pub async fn stop(self) -> Result<()> {
        match self {
            #[cfg(feature = "cdp")]
            Self::Cdp(route) => route.stop().await?,
        }
        Ok(())
    }
}

pub struct BackendRecorder {
    #[cfg(feature = "cdp")]
    inner: drission::cdp::HarRecorder,
}

pub struct BackendHarPlayer {
    #[cfg(feature = "cdp")]
    inner: Option<drission::cdp::HarPlayer>,
}

impl BackendHarPlayer {
    pub async fn stop(mut self) -> Result<()> {
        #[cfg(feature = "cdp")]
        if let Some(player) = self.inner.take() {
            player.stop().await?;
        }
        Ok(())
    }
}

pub struct BackendCodegen {
    #[cfg(feature = "cdp")]
    inner: drission::cdp::ChromiumRecorder,
}

impl BackendCodegen {
    pub async fn status(&self) -> Result<Value> {
        #[cfg(feature = "cdp")]
        {
            return Ok(json!({
                "recording": self.inner.is_recording(),
                "actions": self.inner.len().await,
            }));
        }
        #[cfg(not(feature = "cdp"))]
        anyhow::bail!("codegen record 只支持 CDP 后端")
    }

    pub async fn stop(self, out: Option<PathBuf>) -> Result<Value> {
        #[cfg(feature = "cdp")]
        {
            let script = self.inner.stop().await?;
            let rust = script.to_rust();
            let path = match out {
                Some(p) => p,
                None => default_codegen_path()?,
            };
            if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                tokio::fs::create_dir_all(parent).await?;
            }
            tokio::fs::write(&path, rust.as_bytes()).await?;
            return Ok(json!({
                "recording": false,
                "actions": script.len(),
                "out": path,
            }));
        }
        #[cfg(not(feature = "cdp"))]
        {
            let _ = out;
            anyhow::bail!("codegen record 只支持 CDP 后端")
        }
    }
}

enum NetworkAction {
    Block,
    Mock {
        status: u16,
        headers: Vec<(String, String)>,
        body: String,
    },
    Rewrite(drission::net::ResumeOptions),
}

impl BackendRecorder {
    pub async fn save(self, path: &std::path::Path) -> Result<PathBuf> {
        #[cfg(feature = "cdp")]
        {
            let log = self.inner.stop().await?;
            return Ok(log.save(path).await?);
        }
        #[cfg(not(feature = "cdp"))]
        {
            let _ = path;
            anyhow::bail!("network record 只支持 CDP 后端")
        }
    }
}

#[cfg(feature = "cdp")]
async fn launch_cdp(headless: bool, user_data_dir: Option<PathBuf>) -> Result<BackendBrowser> {
    let mut opts = drission::cdp::ChromiumOptions::new().headless(headless);
    if let Some(dir) = user_data_dir {
        opts = opts.user_data_dir(dir);
    }
    if let Ok(dir) = paths::downloads_dir() {
        let _ = std::fs::create_dir_all(&dir);
        opts = opts.download_path(dir);
    }
    Ok(BackendBrowser::Cdp(
        drission::cdp::ChromiumBrowser::launch(opts).await?,
    ))
}

#[cfg(not(feature = "cdp"))]
async fn launch_cdp(_headless: bool, _user_data_dir: Option<PathBuf>) -> Result<BackendBrowser> {
    anyhow::bail!("backend cdp is not compiled; rebuild drs with feature `cdp`")
}

#[cfg(feature = "camoufox")]
async fn launch_camoufox(headless: bool, user_data_dir: Option<PathBuf>) -> Result<BackendBrowser> {
    let mut opts = drission::launcher::BrowserOptions::new().headless(headless);
    if let Some(dir) = user_data_dir {
        opts = opts.user_data_dir(dir);
    }
    if let Ok(dir) = paths::downloads_dir() {
        let _ = std::fs::create_dir_all(&dir);
        opts = opts.download_path(dir);
    }
    Ok(BackendBrowser::Camoufox(
        drission::browser::Browser::launch(opts).await?,
    ))
}

#[cfg(not(feature = "camoufox"))]
async fn launch_camoufox(
    _headless: bool,
    _user_data_dir: Option<PathBuf>,
) -> Result<BackendBrowser> {
    anyhow::bail!("backend camoufox is not compiled; rebuild drs with feature `camoufox`")
}

impl BackendTab {
    pub async fn close(&self) -> Result<()> {
        match self {
            #[cfg(feature = "cdp")]
            BackendTab::Cdp(tab) => tab.close().await?,
            #[cfg(feature = "camoufox")]
            BackendTab::Camoufox(tab) => tab.close().await?,
        }
        Ok(())
    }

    pub async fn title(&self) -> Result<String> {
        match self {
            #[cfg(feature = "cdp")]
            BackendTab::Cdp(tab) => Ok(tab.title().await?),
            #[cfg(feature = "camoufox")]
            BackendTab::Camoufox(tab) => Ok(tab.title().await?),
        }
    }

    pub async fn url(&self) -> Result<String> {
        match self {
            #[cfg(feature = "cdp")]
            BackendTab::Cdp(tab) => Ok(tab.url().await?),
            #[cfg(feature = "camoufox")]
            BackendTab::Camoufox(tab) => Ok(tab.url().await?),
        }
    }

    pub async fn html(&self) -> Result<String> {
        match self {
            #[cfg(feature = "cdp")]
            BackendTab::Cdp(tab) => Ok(tab.html().await?),
            #[cfg(feature = "camoufox")]
            BackendTab::Camoufox(tab) => Ok(tab.html().await?),
        }
    }

    /// Body innerHTML only — cheaper input for HTML→Markdown than full document.
    pub async fn body_html(&self) -> Result<String> {
        let v = self
            .eval("document.body ? document.body.innerHTML : ''")
            .await?;
        Ok(v.as_str().unwrap_or_default().to_string())
    }

    /// AI-oriented interesting-only snapshot with interactive refs.
    pub async fn ai_snapshot(
        &self,
        budget_ms: Option<u64>,
        max_items: Option<usize>,
        max_text_chars: Option<usize>,
    ) -> Result<Value> {
        let js = drission::ai_snapshot::ai_snapshot_js(
            budget_ms.unwrap_or(drission::ai_snapshot::DEFAULT_BUDGET_MS),
            max_items.unwrap_or(drission::ai_snapshot::DEFAULT_MAX_ITEMS),
            max_text_chars.unwrap_or(drission::ai_snapshot::DEFAULT_MAX_TEXT_CHARS),
        );
        let raw = self.eval(&js).await?;
        let snap = drission::ai_snapshot::AiSnapshot::from_value(&raw)
            .ok_or_else(|| anyhow!("ai snapshot returned unexpected shape: {raw}"))?;
        Ok(snap.into_json_fields())
    }

    pub async fn text(&self, selector: Option<&str>) -> Result<String> {
        match (self, selector) {
            #[cfg(feature = "cdp")]
            (BackendTab::Cdp(tab), Some(selector)) => Ok(tab.ele(selector).await?.text().await?),
            #[cfg(feature = "camoufox")]
            (BackendTab::Camoufox(tab), Some(selector)) => {
                Ok(tab.ele(selector).await?.text().await?)
            }
            #[cfg(feature = "cdp")]
            (BackendTab::Cdp(tab), None) => Ok(tab
                .run_js("document.body ? document.body.innerText : ''")
                .await?
                .as_str()
                .unwrap_or_default()
                .to_string()),
            #[cfg(feature = "camoufox")]
            (BackendTab::Camoufox(tab), None) => Ok(tab
                .run_js("document.body ? document.body.innerText : ''")
                .await?
                .as_str()
                .unwrap_or_default()
                .to_string()),
        }
    }

    pub async fn eval(&self, js: &str) -> Result<Value> {
        match self {
            #[cfg(feature = "cdp")]
            BackendTab::Cdp(tab) => Ok(tab.run_js(js).await?),
            #[cfg(feature = "camoufox")]
            BackendTab::Camoufox(tab) => Ok(tab.run_js(js).await?),
        }
    }

    pub async fn ax(&self, format: AxFormat) -> Result<Value> {
        match self {
            #[cfg(feature = "cdp")]
            BackendTab::Cdp(tab) => {
                let tree = tab.ax_snapshot().await?;
                Ok(match format {
                    AxFormat::Outline => json!({ "outline": tree.to_outline() }),
                    AxFormat::Json => serde_json::to_value(tree)?,
                })
            }
            #[cfg(feature = "camoufox")]
            BackendTab::Camoufox(tab) => {
                let tree = tab.ax_snapshot().await?;
                Ok(match format {
                    AxFormat::Outline => json!({ "outline": tree.to_outline() }),
                    AxFormat::Json => serde_json::to_value(tree)?,
                })
            }
        }
    }

    pub async fn screenshot(
        &self,
        out: Option<PathBuf>,
        full: bool,
        inline: bool,
    ) -> Result<Value> {
        let bytes = match self {
            #[cfg(feature = "cdp")]
            BackendTab::Cdp(tab) => {
                if full {
                    tab.screenshot_full_bytes().await?
                } else {
                    tab.screenshot_bytes().await?
                }
            }
            #[cfg(feature = "camoufox")]
            BackendTab::Camoufox(tab) => tab.screenshot_bytes(full).await?,
        };

        let path = match out {
            Some(path) => path,
            None => default_screenshot_path().await?,
        };
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&path, &bytes).await?;

        let mut data = json!({
            "path": path,
            "bytes": bytes.len(),
            "mimeType": "image/png",
            "full": full,
        });
        if inline {
            data["base64"] = json!(base64::engine::general_purpose::STANDARD.encode(&bytes));
        }
        Ok(data)
    }

    pub async fn click(&self, selector: &str) -> Result<()> {
        let selector = drission::ai_snapshot::resolve_selector(selector);
        match self {
            #[cfg(feature = "cdp")]
            BackendTab::Cdp(tab) => tab.click(&selector).await?,
            #[cfg(feature = "camoufox")]
            BackendTab::Camoufox(tab) => tab.click(&selector).await?,
        }
        Ok(())
    }

    pub async fn type_text(&self, selector: &str, text: &str) -> Result<()> {
        let selector = drission::ai_snapshot::resolve_selector(selector);
        match self {
            #[cfg(feature = "cdp")]
            BackendTab::Cdp(tab) => tab.input(&selector, text).await?,
            #[cfg(feature = "camoufox")]
            BackendTab::Camoufox(tab) => tab.input(&selector, text).await?,
        }
        Ok(())
    }

    pub async fn press(&self, key: &str, selector: Option<&str>) -> Result<()> {
        let selector = selector.map(drission::ai_snapshot::resolve_selector);
        match (self, selector.as_deref()) {
            #[cfg(feature = "cdp")]
            (BackendTab::Cdp(tab), Some(selector)) => {
                tab.ele(selector)
                    .await?
                    .input_keys(&[drission::keys::KeyInput::key(key)])
                    .await?
            }
            #[cfg(feature = "camoufox")]
            (BackendTab::Camoufox(tab), Some(selector)) => {
                tab.ele(selector)
                    .await?
                    .input_keys(&[drission::keys::KeyInput::key(key)])
                    .await?
            }
            #[cfg(feature = "cdp")]
            (BackendTab::Cdp(tab), None) => tab.press_key(key).await?,
            #[cfg(feature = "camoufox")]
            (BackendTab::Camoufox(tab), None) => tab.press_key(key).await?,
        }
        Ok(())
    }

    pub async fn wait(&self, selector: &str, timeout: Option<Duration>) -> Result<bool> {
        let selector = drission::ai_snapshot::resolve_selector(selector);
        match self {
            #[cfg(feature = "cdp")]
            BackendTab::Cdp(tab) => Ok(tab.wait().ele_displayed(&selector, timeout).await?),
            #[cfg(feature = "camoufox")]
            BackendTab::Camoufox(tab) => Ok(tab.wait().ele_displayed(&selector, timeout).await?),
        }
    }

    pub async fn save_state(&self, path: &str) -> Result<()> {
        match self {
            #[cfg(feature = "cdp")]
            BackendTab::Cdp(tab) => tab.save_storage_state(path).await?,
            #[cfg(feature = "camoufox")]
            BackendTab::Camoufox(tab) => tab.save_storage_state(path).await?,
        }
        Ok(())
    }

    pub async fn load_state(&self, path: &str) -> Result<()> {
        match self {
            #[cfg(feature = "cdp")]
            BackendTab::Cdp(tab) => tab.load_storage_state(path).await?,
            #[cfg(feature = "camoufox")]
            BackendTab::Camoufox(tab) => tab.load_storage_state(path).await?,
        }
        Ok(())
    }

    #[cfg(feature = "ocr")]
    pub async fn ocr(&self, selector: &str) -> Result<String> {
        Ok(match self {
            #[cfg(feature = "cdp")]
            BackendTab::Cdp(tab) => tab.ocr_image(selector).await?,
            #[cfg(feature = "camoufox")]
            BackendTab::Camoufox(tab) => tab.ocr_image(selector).await?,
        })
    }

    #[cfg(feature = "slider")]
    pub async fn solve_slider(&self, preset: &str, index: Option<u32>) -> Result<Value> {
        let r = match self {
            #[cfg(feature = "cdp")]
            BackendTab::Cdp(tab) => match preset {
                "dingxiang" | "dx" => tab.solve_dingxiang_slide(index.unwrap_or(0), None).await?,
                _ => tab.solve_geetest_slide().await?,
            },
            #[cfg(feature = "camoufox")]
            BackendTab::Camoufox(tab) => match preset {
                "dingxiang" | "dx" => tab.solve_dingxiang_slide(index.unwrap_or(0), None).await?,
                _ => tab.solve_geetest_slide().await?,
            },
        };
        Ok(json!({ "passed": r.passed, "attempts": r.attempts, "align_error": r.align_error }))
    }

    pub async fn listen_start(
        &self,
        keywords: &[String],
        xhr_only: bool,
        methods: &[String],
    ) -> Result<Value> {
        match self {
            #[cfg(feature = "cdp")]
            BackendTab::Cdp(tab) => {
                let mut handle = tab.network();
                for keyword in keywords {
                    handle = handle.filter(keyword);
                }
                for method in methods {
                    handle = handle.method(method);
                }
                if xhr_only {
                    handle = handle.xhr_only();
                }
                handle.listen().await?;
                Ok(json!({
                    "listening": true,
                    "xhrOnly": xhr_only,
                    "methods": methods,
                }))
            }
            #[cfg(feature = "camoufox")]
            BackendTab::Camoufox(tab) => {
                let refs: Vec<&str> = keywords.iter().map(String::as_str).collect();
                tab.listen().start(&refs).await?;
                Ok(json!({
                    "listening": true,
                    "xhrOnly": false,
                    "warning": if xhr_only || !methods.is_empty() {
                        Some("camoufox listener ignores xhr_only/method; URL filters still apply")
                    } else {
                        None
                    }
                }))
            }
        }
    }

    pub async fn network_block(
        &self,
        keywords: &[String],
        methods: &[String],
    ) -> Result<BackendRoute> {
        self.network_route(keywords, methods, NetworkAction::Block)
            .await
    }

    pub async fn network_mock(
        &self,
        keywords: &[String],
        methods: &[String],
        status: u16,
        headers: Vec<(String, String)>,
        body: String,
    ) -> Result<BackendRoute> {
        self.network_route(
            keywords,
            methods,
            NetworkAction::Mock {
                status,
                headers,
                body,
            },
        )
        .await
    }

    pub async fn network_rewrite(
        &self,
        keywords: &[String],
        methods: &[String],
        opts: drission::net::ResumeOptions,
    ) -> Result<BackendRoute> {
        self.network_route(keywords, methods, NetworkAction::Rewrite(opts))
            .await
    }

    pub async fn network_record(&self) -> Result<BackendRecorder> {
        match self {
            #[cfg(feature = "cdp")]
            BackendTab::Cdp(tab) => Ok(BackendRecorder {
                inner: tab.network().record().await?,
            }),
            #[cfg(feature = "camoufox")]
            BackendTab::Camoufox(_) => anyhow::bail!("network record 只支持 CDP 后端"),
        }
    }

    async fn network_route(
        &self,
        keywords: &[String],
        methods: &[String],
        action: NetworkAction,
    ) -> Result<BackendRoute> {
        match self {
            #[cfg(feature = "cdp")]
            BackendTab::Cdp(tab) => {
                let mut handle = tab.network();
                for keyword in keywords {
                    handle = handle.filter(keyword);
                }
                for method in methods {
                    handle = handle.method(method);
                }
                let route = match action {
                    NetworkAction::Block => handle.block().await?,
                    NetworkAction::Mock {
                        status,
                        headers,
                        body,
                    } => handle.mock(status, headers, body).await?,
                    NetworkAction::Rewrite(opts) => handle.rewrite(opts).await?,
                };
                Ok(BackendRoute::Cdp(route))
            }
            #[cfg(feature = "camoufox")]
            BackendTab::Camoufox(_) => {
                let _ = (keywords, methods, action);
                anyhow::bail!("network block/mock/rewrite 只支持 CDP 后端")
            }
        }
    }

    pub async fn listen_wait(&self, count: usize, timeout: Option<Duration>) -> Result<Value> {
        let count = count.max(1);
        let packets: Vec<PacketSummary> = match self {
            #[cfg(feature = "cdp")]
            BackendTab::Cdp(tab) => tab
                .listen()
                .wait_count(count, timeout)
                .await?
                .into_iter()
                .map(Into::into)
                .collect(),
            #[cfg(feature = "camoufox")]
            BackendTab::Camoufox(tab) => tab
                .listen()
                .wait_count(count, timeout)
                .await?
                .into_iter()
                .map(Into::into)
                .collect(),
        };
        Ok(json!({ "count": packets.len(), "packets": packets }))
    }

    pub async fn listen_stop(&self) -> Result<()> {
        match self {
            #[cfg(feature = "cdp")]
            BackendTab::Cdp(tab) => tab.listen().stop().await?,
            #[cfg(feature = "camoufox")]
            BackendTab::Camoufox(tab) => tab.listen().stop().await?,
        }
        Ok(())
    }

    pub async fn pass_cf(&self, timeout: Duration) -> Result<bool> {
        match self {
            #[cfg(feature = "cdp")]
            BackendTab::Cdp(tab) => Ok(tab.pass_cloudflare(timeout).await?),
            #[cfg(feature = "camoufox")]
            BackendTab::Camoufox(tab) => Ok(tab.pass_cloudflare(timeout).await?),
        }
    }

    pub async fn fingerprint_snapshot(&self) -> Result<drission::fingerprint::FingerprintSnapshot> {
        match self {
            #[cfg(feature = "cdp")]
            BackendTab::Cdp(tab) => Ok(tab.fingerprint_snapshot().await?),
            #[cfg(feature = "camoufox")]
            BackendTab::Camoufox(tab) => Ok(tab.fingerprint_snapshot().await?),
        }
    }

    pub async fn identity_report(&self) -> Result<drission::fingerprint::IdentityReport> {
        match self {
            #[cfg(feature = "cdp")]
            BackendTab::Cdp(tab) => Ok(tab.identity_report().await?),
            #[cfg(feature = "camoufox")]
            BackendTab::Camoufox(tab) => Ok(tab.identity_report().await?),
        }
    }

    pub async fn listen_inspect(&self, limit: Option<usize>) -> Result<Value> {
        let (listening, mut packets): (bool, Vec<PacketSummary>) = match self {
            #[cfg(feature = "cdp")]
            BackendTab::Cdp(tab) => {
                let listen = tab.listen();
                (
                    listen.is_listening().await,
                    listen.peek().await.into_iter().map(Into::into).collect(),
                )
            }
            #[cfg(feature = "camoufox")]
            BackendTab::Camoufox(tab) => {
                let listen = tab.listen();
                (
                    listen.is_listening().await,
                    listen.peek().await.into_iter().map(Into::into).collect(),
                )
            }
        };
        let total = packets.len();
        if let Some(limit) = limit.filter(|n| *n > 0) {
            if packets.len() > limit {
                packets = packets[packets.len() - limit..].to_vec();
            }
        }
        Ok(json!({
            "listening": listening,
            "count": total,
            "shown": packets.len(),
            "packets": packets,
        }))
    }

    pub async fn network_replay(
        &self,
        path: &str,
        ignore_query: bool,
        fallback: bool,
    ) -> Result<BackendHarPlayer> {
        match self {
            #[cfg(feature = "cdp")]
            BackendTab::Cdp(tab) => {
                let opts = drission::cdp::HarReplayOptions::new()
                    .ignore_query(ignore_query)
                    .not_found(if fallback {
                        drission::cdp::HarNotFound::Fallback
                    } else {
                        drission::cdp::HarNotFound::Abort
                    });
                Ok(BackendHarPlayer {
                    inner: Some(tab.network().replay(path, &opts).await?),
                })
            }
            #[cfg(feature = "camoufox")]
            BackendTab::Camoufox(_) => {
                let _ = (path, ignore_query, fallback);
                anyhow::bail!("HAR replay 只支持 CDP 后端")
            }
        }
    }

    pub async fn save_pdf(
        &self,
        out: Option<PathBuf>,
        landscape: bool,
        print_background: bool,
    ) -> Result<Value> {
        match self {
            #[cfg(feature = "cdp")]
            BackendTab::Cdp(tab) => {
                let path = match out {
                    Some(p) => p,
                    None => default_pdf_path()?,
                };
                let opts = drission::cdp::PdfOptions::new()
                    .landscape(landscape)
                    .print_background(print_background);
                let saved = tab.save_pdf(&path, &opts).await?;
                Ok(
                    json!({ "path": saved, "landscape": landscape, "printBackground": print_background }),
                )
            }
            #[cfg(feature = "camoufox")]
            BackendTab::Camoufox(_) => {
                let _ = (out, landscape, print_background);
                anyhow::bail!("pdf 只支持 CDP 后端(需无头 Chrome)")
            }
        }
    }

    pub async fn codegen_start(&self) -> Result<BackendCodegen> {
        match self {
            #[cfg(feature = "cdp")]
            BackendTab::Cdp(tab) => {
                let rec = tab.recorder();
                rec.start().await?;
                Ok(BackendCodegen { inner: rec })
            }
            #[cfg(feature = "camoufox")]
            BackendTab::Camoufox(_) => anyhow::bail!("codegen record 只支持 CDP 后端"),
        }
    }

    pub async fn console_start(
        &self,
        level: Option<String>,
        text: Option<String>,
    ) -> Result<Value> {
        match self {
            #[cfg(feature = "cdp")]
            BackendTab::Cdp(tab) => {
                let mut filter = drission::cdp::ConsoleFilter::new();
                if let Some(level) = level.as_deref() {
                    filter = filter.level(level);
                }
                if let Some(text) = text.as_deref() {
                    filter = filter.text(text);
                }
                tab.console().start_with(filter).await?;
                Ok(json!({ "listening": true, "level": level, "text": text }))
            }
            #[cfg(feature = "camoufox")]
            BackendTab::Camoufox(tab) => {
                let mut filter = drission::browser::console::ConsoleFilter::new();
                if let Some(level) = level.as_deref() {
                    filter = filter.level(level);
                }
                if let Some(text) = text.as_deref() {
                    filter = filter.contains(text);
                }
                tab.console().start_with(filter).await?;
                Ok(json!({ "listening": true, "level": level, "text": text }))
            }
        }
    }

    pub async fn console_wait(&self, timeout: Option<Duration>) -> Result<Value> {
        match self {
            #[cfg(feature = "cdp")]
            BackendTab::Cdp(tab) => {
                let msg = tab.console().wait(timeout).await?;
                Ok(
                    json!({ "message": msg.as_ref().map(console_cdp_json), "timeout": msg.is_none() }),
                )
            }
            #[cfg(feature = "camoufox")]
            BackendTab::Camoufox(tab) => {
                let msg = tab.console().wait(timeout).await?;
                Ok(
                    json!({ "message": msg.as_ref().map(console_fx_json), "timeout": msg.is_none() }),
                )
            }
        }
    }

    pub async fn console_list(&self) -> Result<Value> {
        match self {
            #[cfg(feature = "cdp")]
            BackendTab::Cdp(tab) => {
                let messages: Vec<Value> = tab
                    .console()
                    .peek()
                    .await
                    .iter()
                    .map(console_cdp_json)
                    .collect();
                let listening = tab.console().listening().await;
                Ok(json!({ "listening": listening, "count": messages.len(), "messages": messages }))
            }
            #[cfg(feature = "camoufox")]
            BackendTab::Camoufox(tab) => {
                let messages: Vec<Value> = tab
                    .console()
                    .peek()
                    .await
                    .iter()
                    .map(console_fx_json)
                    .collect();
                Ok(json!({ "listening": true, "count": messages.len(), "messages": messages }))
            }
        }
    }

    pub async fn console_stop(&self) -> Result<Value> {
        match self {
            #[cfg(feature = "cdp")]
            BackendTab::Cdp(tab) => tab.console().stop().await?,
            #[cfg(feature = "camoufox")]
            BackendTab::Camoufox(tab) => tab.console().stop().await?,
        }
        Ok(json!({ "listening": false }))
    }

    pub async fn download_start(&self, dir: Option<&str>) -> Result<Value> {
        let dir = match dir {
            Some(d) => PathBuf::from(d),
            None => paths::downloads_dir()?,
        };
        tokio::fs::create_dir_all(&dir).await?;
        match self {
            #[cfg(feature = "cdp")]
            BackendTab::Cdp(tab) => {
                tab.set_download_path(&dir).await?;
                tab.downloads().start().await?;
            }
            #[cfg(feature = "camoufox")]
            BackendTab::Camoufox(tab) => {
                tab.downloads().start().await?;
            }
        }
        Ok(json!({ "tracking": true, "dir": dir }))
    }

    pub async fn download_list(&self) -> Result<Value> {
        match self {
            #[cfg(feature = "cdp")]
            BackendTab::Cdp(tab) => {
                let missions: Vec<Value> = tab
                    .downloads()
                    .missions()
                    .await
                    .into_iter()
                    .map(download_cdp_json)
                    .collect();
                Ok(json!({ "count": missions.len(), "missions": missions }))
            }
            #[cfg(feature = "camoufox")]
            BackendTab::Camoufox(tab) => {
                let missions: Vec<Value> = tab
                    .downloads()
                    .missions()
                    .await
                    .into_iter()
                    .map(download_fx_json)
                    .collect();
                Ok(json!({ "count": missions.len(), "missions": missions }))
            }
        }
    }

    pub async fn download_wait(
        &self,
        timeout: Option<Duration>,
        count: Option<usize>,
    ) -> Result<Value> {
        let timeout = timeout.unwrap_or(Duration::from_secs(30));
        let count = count.unwrap_or(1).max(1);
        match self {
            #[cfg(feature = "cdp")]
            BackendTab::Cdp(tab) => {
                let missions: Vec<Value> = tab
                    .downloads()
                    .wait_count_done(count, timeout)
                    .await?
                    .into_iter()
                    .map(download_cdp_json)
                    .collect();
                Ok(json!({ "count": missions.len(), "missions": missions }))
            }
            #[cfg(feature = "camoufox")]
            BackendTab::Camoufox(tab) => {
                let missions: Vec<Value> = tab
                    .downloads()
                    .wait_count_done(count, timeout)
                    .await?
                    .into_iter()
                    .map(download_fx_json)
                    .collect();
                Ok(json!({ "count": missions.len(), "missions": missions }))
            }
        }
    }

    pub async fn download_stop(&self) -> Result<Value> {
        match self {
            #[cfg(feature = "cdp")]
            BackendTab::Cdp(tab) => tab.downloads().stop().await?,
            #[cfg(feature = "camoufox")]
            BackendTab::Camoufox(tab) => tab.downloads().stop().await?,
        }
        Ok(json!({ "tracking": false }))
    }
}

#[cfg(feature = "cdp")]
fn console_cdp_json(d: &drission::cdp::ConsoleData) -> Value {
    json!({
        "level": d.level,
        "text": d.text,
        "url": d.url,
        "line": d.line,
    })
}

#[cfg(feature = "camoufox")]
fn console_fx_json(d: &drission::browser::console::ConsoleData) -> Value {
    json!({
        "level": d.level,
        "text": d.text,
        "url": d.url,
        "line": d.line,
    })
}

#[cfg(feature = "cdp")]
fn download_cdp_json(m: drission::cdp::DownloadMission) -> Value {
    json!({
        "guid": m.guid,
        "url": m.url,
        "filename": m.suggested_filename,
        "path": m.path,
        "state": format!("{:?}", m.state),
        "receivedBytes": m.received_bytes,
        "totalBytes": m.total_bytes,
    })
}

#[cfg(feature = "camoufox")]
fn download_fx_json(m: drission::browser::download::DownloadMission) -> Value {
    json!({
        "filename": m.suggested_filename,
        "path": m.path,
        "state": format!("{:?}", m.state),
    })
}

fn default_pdf_path() -> Result<PathBuf> {
    let dir = paths::pdf_dir()?;
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join(format!("page-{}.pdf", now_ms())))
}

fn default_codegen_path() -> Result<PathBuf> {
    let dir = paths::codegen_dir()?;
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join(format!("script-{}.rs", now_ms())))
}

async fn default_screenshot_path() -> Result<PathBuf> {
    let dir = paths::screenshots_dir()?;
    tokio::fs::create_dir_all(&dir).await?;
    Ok(dir.join(format!("shot-{}.png", now_ms())))
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

pub fn ensure_backend_available(kind: BackendKind) -> Result<()> {
    match kind {
        BackendKind::Cdp => {
            if cfg!(feature = "cdp") {
                Ok(())
            } else {
                Err(anyhow!(
                    "backend cdp is not compiled; rebuild drs with feature `cdp`"
                ))
            }
        }
        BackendKind::Camoufox => {
            if cfg!(feature = "camoufox") {
                Ok(())
            } else {
                Err(anyhow!(
                    "backend camoufox is not compiled; rebuild drs with feature `camoufox`"
                ))
            }
        }
    }
}
