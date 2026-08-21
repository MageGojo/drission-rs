use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Result, anyhow};
use serde_json::{Value, json};

use crate::backend::{
    BackendBrowser, BackendCodegen, BackendHarPlayer, BackendRecorder, BackendRoute, BackendTab,
    ContextUseSpec, ensure_backend_available,
};
use crate::protocol::{BackendKind, EngineCommand, IdentityGate, TabSummary};

fn truncate_chars(text: &mut String, max_chars: usize) -> bool {
    if text.chars().count() <= max_chars {
        return false;
    }
    *text = text.chars().take(max_chars).collect();
    true
}

pub struct BrowserState {
    backend: BackendKind,
    browser: BackendBrowser,
    tabs: Vec<TabSlot>,
    active: Option<u64>,
    active_context: Option<String>,
    next_tab_id: u64,
}

struct TabSlot {
    id: u64,
    tab: BackendTab,
    context: Option<String>,
    routes: Vec<BackendRoute>,
    recorder: Option<(PathBuf, BackendRecorder)>,
    codegen: Option<BackendCodegen>,
    har_player: Option<BackendHarPlayer>,
}

pub struct EngineResult {
    pub data: Value,
    pub stop: bool,
}

impl BrowserState {
    pub async fn launch(
        backend: BackendKind,
        headless: bool,
        user_data_dir: Option<PathBuf>,
    ) -> Result<Self> {
        ensure_backend_available(backend)?;
        let browser = BackendBrowser::launch(backend, headless, user_data_dir).await?;
        let mut state = Self {
            backend,
            browser,
            tabs: Vec::new(),
            active: None,
            active_context: None,
            next_tab_id: 1,
        };
        let tab = state.browser.new_tab(None).await?;
        state.insert_tab(tab, None);
        Ok(state)
    }

    pub async fn execute(&mut self, command: EngineCommand) -> Result<EngineResult> {
        let data = match command {
            EngineCommand::Status => self.status().await?,
            EngineCommand::Stop => {
                let tabs = self.tabs.len();
                self.browser.quit().await?;
                return Ok(EngineResult {
                    data: json!({ "stopping": true, "tabs": tabs }),
                    stop: true,
                });
            }
            EngineCommand::Open { url, context } => {
                let context = context.or_else(|| self.active_context.clone());
                let tab = self.open_tab(Some(&url), context.as_deref()).await?;
                let id = self.insert_tab(tab, context.clone());
                json!({
                    "tabId": id,
                    "url": url,
                    "active": true,
                    "context": context,
                })
            }
            EngineCommand::Tabs => json!({ "tabs": self.tab_summaries().await? }),
            EngineCommand::UseTab { tab_id } => {
                self.find_slot(tab_id)?;
                self.active = Some(tab_id);
                json!({ "tabId": tab_id, "active": true })
            }
            EngineCommand::Close { tab_id } => {
                let id = tab_id
                    .or(self.active)
                    .ok_or_else(|| anyhow!("no active tab"))?;
                let idx = self
                    .tabs
                    .iter()
                    .position(|slot| slot.id == id)
                    .ok_or_else(|| anyhow!("tab {id} not found"))?;
                let slot = self.tabs.remove(idx);
                slot.tab.close().await?;
                if self.active == Some(id) {
                    self.active = self.tabs.last().map(|slot| slot.id);
                }
                json!({ "closed": id, "activeTabId": self.active })
            }
            EngineCommand::Ax { format } => {
                // Keep daemon responsive on automation-hostile pages.
                match tokio::time::timeout(Duration::from_secs(12), self.active_tab()?.ax(format))
                    .await
                {
                    Ok(Ok(v)) => v,
                    Ok(Err(e)) => return Err(e),
                    Err(_) => {
                        return Err(anyhow!(
                            "accessibility snapshot timed out after 12s; try `drs snapshot` instead"
                        ));
                    }
                }
            }
            EngineCommand::Snapshot {
                budget_ms,
                max_items,
                max_text_chars,
                max_markdown_chars,
            } => {
                self.snapshot(budget_ms, max_items, max_text_chars, max_markdown_chars)
                    .await?
            }
            EngineCommand::Markdown { max_chars } => self.page_markdown(max_chars).await?,
            EngineCommand::Html { max_chars } => {
                let html_fut = self.active_tab()?.html();
                let mut html = match tokio::time::timeout(Duration::from_secs(15), html_fut).await {
                    Ok(Ok(v)) => v,
                    Ok(Err(e)) => return Err(e),
                    Err(_) => {
                        return Err(anyhow!(
                            "html timed out after 15s; prefer `drs snapshot` for agents"
                        ));
                    }
                };
                let limit = max_chars.unwrap_or(200_000);
                let truncated = truncate_chars(&mut html, limit);
                json!({ "html": html, "truncated": truncated, "maxChars": limit })
            }
            EngineCommand::Text { selector } => {
                let selector = selector.map(|s| drission::ai_snapshot::resolve_selector(&s));
                json!({ "text": self.active_tab()?.text(selector.as_deref()).await? })
            }
            EngineCommand::Eval { js } => json!({ "value": self.active_tab()?.eval(&js).await? }),
            EngineCommand::Screenshot { out, full, inline } => {
                self.active_tab()?.screenshot(out, full, inline).await?
            }
            EngineCommand::Click { selector } => {
                self.active_tab()?.click(&selector).await?;
                json!({ "clicked": selector })
            }
            EngineCommand::Type { selector, text } => {
                self.active_tab()?.type_text(&selector, &text).await?;
                json!({ "typed": selector, "chars": text.chars().count() })
            }
            EngineCommand::Press { key, selector } => {
                self.active_tab()?.press(&key, selector.as_deref()).await?;
                json!({ "pressed": key, "selector": selector })
            }
            EngineCommand::Wait {
                selector,
                timeout_ms,
            } => {
                let ok = self
                    .active_tab()?
                    .wait(&selector, timeout_ms.map(Duration::from_millis))
                    .await?;
                json!({ "selector": selector, "displayed": ok })
            }
            EngineCommand::SaveState { path } => {
                self.active_tab()?.save_state(&path).await?;
                json!({ "saved": path })
            }
            EngineCommand::LoadState { path } => {
                self.active_tab()?.load_state(&path).await?;
                json!({ "loaded": path })
            }
            EngineCommand::Ocr { selector } => {
                #[cfg(feature = "ocr")]
                {
                    let text = self.active_tab()?.ocr(&selector).await?;
                    json!({ "selector": selector, "text": text })
                }
                #[cfg(not(feature = "ocr"))]
                {
                    let _ = selector;
                    anyhow::bail!("此 drs 未启用 ocr feature;请安装/构建带 --features ocr 的 drs")
                }
            }
            EngineCommand::SolveSlider { preset, index } => {
                #[cfg(feature = "slider")]
                {
                    self.active_tab()?.solve_slider(&preset, index).await?
                }
                #[cfg(not(feature = "slider"))]
                {
                    let _ = (preset, index);
                    anyhow::bail!(
                        "此 drs 未启用 slider feature;请安装/构建带 --features slider 的 drs"
                    )
                }
            }
            EngineCommand::ListenStart {
                keywords,
                xhr_only,
                methods,
            } => {
                self.active_tab()?
                    .listen_start(&keywords, xhr_only, &methods)
                    .await?
            }
            EngineCommand::ContextList => self.browser.list_contexts()?,
            EngineCommand::ContextUse {
                name,
                proxy,
                user_agent,
                locale,
                timezone,
                viewport,
            } => {
                let spec = context_spec(
                    &name,
                    proxy.as_deref(),
                    user_agent.as_deref(),
                    locale.as_deref(),
                    timezone.as_deref(),
                    viewport.as_deref(),
                )?;
                let data = self.browser.ensure_context(spec).await?;
                self.active_context = Some(name);
                data
            }
            EngineCommand::ContextClose { name } => {
                self.drop_tabs_in_context(&name);
                self.browser.close_context(&name).await?;
                if self.active_context.as_deref() == Some(name.as_str()) {
                    self.active_context = None;
                }
                json!({ "closed": name, "activeContext": self.active_context })
            }
            EngineCommand::NetworkBlock { keywords, methods } => {
                let route = self
                    .active_tab()?
                    .network_block(&keywords, &methods)
                    .await?;
                self.active_slot_mut()?.routes.push(route);
                json!({ "blocked": keywords, "methods": methods })
            }
            EngineCommand::NetworkMock {
                keywords,
                methods,
                status,
                body,
                headers,
            } => {
                let route = self
                    .active_tab()?
                    .network_mock(&keywords, &methods, status, headers, body)
                    .await?;
                self.active_slot_mut()?.routes.push(route);
                json!({ "mocked": keywords, "status": status })
            }
            EngineCommand::NetworkRewrite {
                keywords,
                methods,
                set_headers,
                url,
                method,
                post_data,
            } => {
                let mut opts = drission::net::ResumeOptions::new();
                if !set_headers.is_empty() {
                    opts = opts.headers(set_headers);
                }
                if let Some(url) = url {
                    opts = opts.url(url);
                }
                if let Some(method) = method {
                    opts = opts.method(method);
                }
                if let Some(post_data) = post_data {
                    opts = opts.post_data(post_data);
                }
                let route = self
                    .active_tab()?
                    .network_rewrite(&keywords, &methods, opts)
                    .await?;
                self.active_slot_mut()?.routes.push(route);
                json!({ "rewritten": keywords, "methods": methods })
            }
            EngineCommand::NetworkRecord { out } => {
                let path = match out {
                    Some(p) => PathBuf::from(p),
                    None => default_har_path()?,
                };
                let recorder = self.active_tab()?.network_record().await?;
                self.active_slot_mut()?.recorder = Some((path.clone(), recorder));
                json!({ "recording": true, "out": path })
            }
            EngineCommand::NetworkInspect { limit } => {
                self.active_tab()?.listen_inspect(limit).await?
            }
            EngineCommand::NetworkReplay {
                path,
                ignore_query,
                fallback,
            } => {
                if let Some(player) = self.active_slot_mut()?.har_player.take() {
                    player.stop().await?;
                }
                let player = self
                    .active_tab()?
                    .network_replay(&path, ignore_query, fallback)
                    .await?;
                self.active_slot_mut()?.har_player = Some(player);
                json!({
                    "replaying": true,
                    "har": path,
                    "ignoreQuery": ignore_query,
                    "fallback": fallback,
                })
            }
            EngineCommand::NetworkStop => self.stop_network().await?,
            EngineCommand::Pdf {
                out,
                landscape,
                print_background,
            } => {
                self.active_tab()?
                    .save_pdf(out, landscape, print_background)
                    .await?
            }
            EngineCommand::Inspect {
                include_network,
                include_console,
                max_packets,
            } => {
                self.inspect_page(include_network, include_console, max_packets)
                    .await?
            }
            EngineCommand::RecordStart => {
                if let Some(old) = self.active_slot_mut()?.codegen.take() {
                    let _ = old.stop(None).await;
                }
                let rec = self.active_tab()?.codegen_start().await?;
                let status = rec.status().await?;
                self.active_slot_mut()?.codegen = Some(rec);
                status
            }
            EngineCommand::RecordStop { out } => {
                let rec =
                    self.active_slot_mut()?.codegen.take().ok_or_else(|| {
                        anyhow!("没有正在进行的 codegen 录制;先 `drs record start`")
                    })?;
                rec.stop(out.map(PathBuf::from)).await?
            }
            EngineCommand::RecordStatus => match &self.active_slot_mut()?.codegen {
                Some(rec) => rec.status().await?,
                None => json!({ "recording": false, "actions": 0 }),
            },
            EngineCommand::ConsoleStart { level, text } => {
                self.active_tab()?.console_start(level, text).await?
            }
            EngineCommand::ConsoleWait { timeout_ms } => {
                self.active_tab()?
                    .console_wait(timeout_ms.map(Duration::from_millis))
                    .await?
            }
            EngineCommand::ConsoleList => self.active_tab()?.console_list().await?,
            EngineCommand::ConsoleStop => self.active_tab()?.console_stop().await?,
            EngineCommand::DownloadStart { dir } => {
                self.active_tab()?.download_start(dir.as_deref()).await?
            }
            EngineCommand::DownloadList => self.active_tab()?.download_list().await?,
            EngineCommand::DownloadWait { timeout_ms, count } => {
                self.active_tab()?
                    .download_wait(timeout_ms.map(Duration::from_millis), count)
                    .await?
            }
            EngineCommand::DownloadStop => self.active_tab()?.download_stop().await?,
            EngineCommand::ListenWait { count, timeout_ms } => {
                self.active_tab()?
                    .listen_wait(count, timeout_ms.map(Duration::from_millis))
                    .await?
            }
            EngineCommand::ListenStop => {
                self.active_tab()?.listen_stop().await?;
                json!({ "listening": false })
            }
            EngineCommand::PassCf { timeout_ms } => {
                let passed = self
                    .active_tab()?
                    .pass_cf(Duration::from_millis(timeout_ms.unwrap_or(30_000)))
                    .await?;
                json!({ "passed": passed })
            }
            EngineCommand::Identity { pool, gate } => {
                if pool {
                    self.identity_pool(gate).await?
                } else {
                    self.identity_active_tab(gate).await?
                }
            }
            EngineCommand::Title => json!({ "title": self.active_tab()?.title().await? }),
            EngineCommand::Url => json!({ "url": self.active_tab()?.url().await? }),
            EngineCommand::Extract {
                url,
                wait_selector,
                timeout_ms,
                pass_cf,
                include_html,
                include_ax_json,
                include_markdown,
                max_text_chars,
                max_markdown_chars,
                screenshot_out,
                full_screenshot,
            } => {
                self.extract(
                    url.as_deref(),
                    wait_selector.as_deref(),
                    timeout_ms,
                    pass_cf,
                    include_html,
                    include_ax_json,
                    include_markdown,
                    max_text_chars,
                    max_markdown_chars,
                    screenshot_out,
                    full_screenshot,
                )
                .await?
            }
        };
        Ok(EngineResult { data, stop: false })
    }

    async fn snapshot(
        &mut self,
        budget_ms: Option<u64>,
        max_items: Option<usize>,
        max_text_chars: Option<usize>,
        max_markdown_chars: Option<usize>,
    ) -> Result<Value> {
        let tab_id = self.active.ok_or_else(|| anyhow!("no active tab"))?;
        let mut snap = {
            let tab = self.active_tab()?;
            let title = tab.title().await?;
            let url = tab.url().await?;
            let snap_fut = tab.ai_snapshot(budget_ms, max_items, max_text_chars);
            let mut snap = match tokio::time::timeout(Duration::from_secs(15), snap_fut).await {
                Ok(Ok(v)) => v,
                Ok(Err(e)) => return Err(e),
                Err(_) => {
                    return Err(anyhow!(
                        "AI snapshot timed out after 15s (page may be automation-hostile)"
                    ));
                }
            };
            if let Some(obj) = snap.as_object_mut() {
                obj.insert("tabId".into(), json!(tab_id));
                obj.insert("title".into(), json!(title));
                obj.insert("url".into(), json!(url));
            }
            snap
        };
        // max_markdown_chars=Some(0) skips conversion (agents that only want refs).
        if max_markdown_chars != Some(0) {
            self.attach_markdown(&mut snap, max_markdown_chars).await;
        }
        Ok(snap)
    }

    async fn page_markdown(&self, max_chars: Option<usize>) -> Result<Value> {
        let tab_id = self.active.ok_or_else(|| anyhow!("no active tab"))?;
        let tab = self.active_tab()?;
        let title = tab.title().await?;
        let url = tab.url().await?;
        let html = match tokio::time::timeout(Duration::from_secs(12), tab.body_html()).await {
            Ok(Ok(h)) => h,
            Ok(Err(e)) => return Err(e),
            Err(_) => return Err(anyhow!("body html timed out after 12s")),
        };
        let mut data = drission::html_md::markdown_fields(&html, max_chars);
        if let Some(obj) = data.as_object_mut() {
            obj.insert("tabId".into(), json!(tab_id));
            obj.insert("title".into(), json!(title));
            obj.insert("url".into(), json!(url));
        }
        Ok(data)
    }

    async fn attach_markdown(&self, data: &mut Value, max_chars: Option<usize>) {
        let Ok(tab) = self.active_tab() else {
            data["markdownError"] = Value::String("no active tab".into());
            return;
        };
        let html = match tokio::time::timeout(Duration::from_secs(12), tab.body_html()).await {
            Ok(Ok(h)) => h,
            Ok(Err(e)) => {
                data["markdownError"] = Value::String(e.to_string());
                return;
            }
            Err(_) => {
                data["markdownError"] = Value::String("body html timed out after 12s".into());
                return;
            }
        };
        if let Some(fields) = drission::html_md::markdown_fields(&html, max_chars).as_object() {
            if let Some(obj) = data.as_object_mut() {
                for (k, v) in fields {
                    obj.insert(k.clone(), v.clone());
                }
            }
        }
    }

    async fn extract(
        &mut self,
        url: Option<&str>,
        wait_selector: Option<&str>,
        timeout_ms: Option<u64>,
        pass_cf: bool,
        include_html: bool,
        include_ax_json: bool,
        include_markdown: bool,
        max_text_chars: Option<usize>,
        max_markdown_chars: Option<usize>,
        screenshot_out: Option<PathBuf>,
        full_screenshot: bool,
    ) -> Result<Value> {
        let tab_id = if let Some(url) = url {
            let context = self.active_context.clone();
            let tab = self.open_tab(Some(url), context.as_deref()).await?;
            let id = self.insert_tab(tab, context);
            self.active = Some(id);
            id
        } else {
            self.active
                .ok_or_else(|| anyhow!("no active tab; pass a URL or run `drs open` first"))?
        };

        if pass_cf {
            let _ = self
                .active_tab()?
                .pass_cf(Duration::from_millis(timeout_ms.unwrap_or(30_000)))
                .await?;
        }

        if let Some(selector) = wait_selector {
            let _ = self
                .active_tab()?
                .wait(selector, timeout_ms.map(Duration::from_millis))
                .await?;
        }

        let mut data = {
            let tab = self.active_tab()?;
            let title = tab.title().await?;
            let current_url = tab.url().await?;
            let mut text = tab.text(None).await?;
            let text_truncated = truncate_chars(&mut text, max_text_chars.unwrap_or(50_000));

            // Accessibility outline can hang on automation-hostile pages; soft-fail
            // so extract still returns title/url/text for agents.
            let ax_timeout = Duration::from_millis(timeout_ms.unwrap_or(8_000).min(15_000));
            let (outline, outline_error) =
                match tokio::time::timeout(ax_timeout, tab.ax(crate::protocol::AxFormat::Outline))
                    .await
                {
                    Ok(Ok(v)) => (
                        v.get("outline")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        None,
                    ),
                    Ok(Err(e)) => (String::new(), Some(e.to_string())),
                    Err(_) => (
                        String::new(),
                        Some(format!(
                            "ax outline timed out after {}ms",
                            ax_timeout.as_millis()
                        )),
                    ),
                };

            let mut data = json!({
                "tabId": tab_id,
                "url": current_url,
                "title": title,
                "text": text,
                "outline": outline,
                "textTruncated": text_truncated,
            });
            if let Some(err) = outline_error {
                data["outlineError"] = Value::String(err);
            }

            if include_html {
                let mut html = tab.html().await?;
                let html_truncated = truncate_chars(&mut html, max_text_chars.unwrap_or(200_000));
                data["html"] = Value::String(html);
                data["htmlTruncated"] = Value::Bool(html_truncated);
            }

            if include_ax_json {
                match tokio::time::timeout(ax_timeout, tab.ax(crate::protocol::AxFormat::Json))
                    .await
                {
                    Ok(Ok(v)) => data["ax"] = v,
                    Ok(Err(e)) => data["axError"] = Value::String(e.to_string()),
                    Err(_) => {
                        data["axError"] = Value::String(format!(
                            "ax json timed out after {}ms",
                            ax_timeout.as_millis()
                        ));
                    }
                }
            }

            if let Some(path) = screenshot_out {
                let shot = tab.screenshot(Some(path), full_screenshot, false).await?;
                if let Some(obj) = shot.as_object() {
                    for (key, value) in obj {
                        data[key.clone()] = value.clone();
                    }
                }
            }
            data
        };

        if include_markdown && max_markdown_chars != Some(0) {
            self.attach_markdown(&mut data, max_markdown_chars).await;
        }

        Ok(data)
    }

    async fn status(&self) -> Result<Value> {
        let contexts = self
            .browser
            .list_contexts()
            .unwrap_or_else(|_| json!({ "contexts": [] }));
        Ok(json!({
            "pid": std::process::id(),
            "backend": self.backend,
            "activeTabId": self.active,
            "activeContext": self.active_context,
            "tabs": self.tab_summaries().await?,
            "contexts": contexts.get("contexts").cloned().unwrap_or_else(|| json!([])),
        }))
    }

    async fn identity_active_tab(&self, gate: IdentityGate) -> Result<Value> {
        let id = self.active.ok_or_else(|| anyhow!("no active tab"))?;
        let report = self.active_tab()?.identity_report().await?;
        let gate = gate.evaluate_identity_report(&report);
        Ok(json!({
            "scope": "tab",
            "tabId": id,
            "gate": gate,
            "report": report,
        }))
    }

    async fn identity_pool(&self, gate: IdentityGate) -> Result<Value> {
        let tabs = self.tab_summaries().await?;
        let mut snapshots = Vec::with_capacity(self.tabs.len());
        for slot in &self.tabs {
            snapshots.push(slot.tab.fingerprint_snapshot().await?);
        }
        let report = drission::fingerprint::IdentityPoolReport::analyze(&snapshots);
        let clusters = report.risk_clusters();
        let offenders = report.risk_offenders();
        let quarantine = report.quarantine_plan();
        let admission = report.admission_plan();
        let gate = gate.evaluate_pool_report(&report);
        let tabs: Vec<_> = tabs
            .into_iter()
            .enumerate()
            .map(|(index, tab)| {
                json!({
                    "index": index,
                    "tabId": tab.id,
                    "active": tab.active,
                    "title": tab.title,
                    "url": tab.url,
                })
            })
            .collect();
        Ok(json!({
            "scope": "pool",
            "tabs": tabs,
            "gate": gate,
            "clusters": clusters,
            "offenders": offenders,
            "quarantine": quarantine,
            "admission": admission,
            "report": report,
        }))
    }

    async fn open_tab(&self, url: Option<&str>, context: Option<&str>) -> Result<BackendTab> {
        if let Some(name) = context {
            self.browser
                .new_tab_in_context(
                    ContextUseSpec {
                        name,
                        proxy: None,
                        user_agent: None,
                        locale: None,
                        timezone: None,
                        viewport: None,
                    },
                    url,
                )
                .await
        } else {
            self.browser.new_tab(url).await
        }
    }

    fn insert_tab(&mut self, tab: BackendTab, context: Option<String>) -> u64 {
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        self.tabs.push(TabSlot {
            id,
            tab,
            context,
            routes: Vec::new(),
            recorder: None,
            codegen: None,
            har_player: None,
        });
        self.active = Some(id);
        id
    }

    fn active_tab(&self) -> Result<&BackendTab> {
        let id = self.active.ok_or_else(|| anyhow!("no active tab"))?;
        Ok(&self.find_slot(id)?.tab)
    }

    fn active_slot_mut(&mut self) -> Result<&mut TabSlot> {
        let id = self.active.ok_or_else(|| anyhow!("no active tab"))?;
        self.tabs
            .iter_mut()
            .find(|slot| slot.id == id)
            .ok_or_else(|| anyhow!("tab {id} not found"))
    }

    fn find_slot(&self, id: u64) -> Result<&TabSlot> {
        self.tabs
            .iter()
            .find(|slot| slot.id == id)
            .ok_or_else(|| anyhow!("tab {id} not found"))
    }

    fn drop_tabs_in_context(&mut self, name: &str) {
        self.tabs
            .retain(|slot| slot.context.as_deref() != Some(name));
        if !self.tabs.iter().any(|slot| Some(slot.id) == self.active) {
            self.active = self.tabs.last().map(|slot| slot.id);
        }
    }

    async fn inspect_page(
        &mut self,
        include_network: bool,
        include_console: bool,
        max_packets: Option<usize>,
    ) -> Result<Value> {
        let tab_id = self.active.ok_or_else(|| anyhow!("no active tab"))?;
        let mut data = self
            .snapshot(Some(1500), Some(80), Some(2000), Some(0))
            .await?;
        if include_network {
            data["network"] = self.active_tab()?.listen_inspect(max_packets).await?;
        }
        if include_console {
            data["console"] = self.active_tab()?.console_list().await?;
        }
        if let Some(obj) = data.as_object_mut() {
            obj.insert("tabId".into(), json!(tab_id));
            obj.insert("kind".into(), json!("inspect"));
        }
        Ok(data)
    }

    async fn stop_network(&mut self) -> Result<Value> {
        let slot = self.active_slot_mut()?;
        let routes = std::mem::take(&mut slot.routes);
        let recorder = slot.recorder.take();
        let player = slot.har_player.take();
        for route in routes {
            route.stop().await?;
        }
        let mut saved = None;
        if let Some((path, recorder)) = recorder {
            saved = Some(recorder.save(&path).await?);
        }
        if let Some(player) = player {
            player.stop().await?;
        }
        self.active_tab()?.listen_stop().await?;
        Ok(json!({
            "listening": false,
            "har": saved,
            "replaying": false,
        }))
    }

    async fn tab_summaries(&self) -> Result<Vec<TabSummary>> {
        let mut out = Vec::with_capacity(self.tabs.len());
        for slot in &self.tabs {
            out.push(TabSummary {
                id: slot.id,
                active: self.active == Some(slot.id),
                title: slot.tab.title().await.unwrap_or_default(),
                url: slot.tab.url().await.unwrap_or_default(),
                context: slot.context.clone(),
            });
        }
        Ok(out)
    }
}

fn context_spec<'a>(
    name: &'a str,
    proxy: Option<&'a str>,
    user_agent: Option<&'a str>,
    locale: Option<&'a str>,
    timezone: Option<&'a str>,
    viewport: Option<&'a str>,
) -> Result<ContextUseSpec<'a>> {
    Ok(ContextUseSpec {
        name,
        proxy,
        user_agent,
        locale,
        timezone,
        viewport: viewport.map(parse_viewport).transpose()?,
    })
}

fn parse_viewport(raw: &str) -> Result<(u32, u32)> {
    let (w, h) = raw
        .split_once('x')
        .or_else(|| raw.split_once('X'))
        .ok_or_else(|| anyhow!("viewport 用 WIDTHxHEIGHT，例如 1280x720"))?;
    Ok((
        w.trim()
            .parse()
            .map_err(|_| anyhow!("viewport 宽度无效: {w}"))?,
        h.trim()
            .parse()
            .map_err(|_| anyhow!("viewport 高度无效: {h}"))?,
    ))
}

fn default_har_path() -> Result<PathBuf> {
    let dir = crate::paths::cli_dir()?.join("har");
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join(format!(
        "session-{}.har",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    )))
}

pub fn validate_backend_or_bail(backend: BackendKind) -> Result<()> {
    ensure_backend_available(backend).map_err(|e| {
        anyhow!(
            "{}. Current build features: cdp={}, camoufox={}",
            e,
            cfg!(feature = "cdp"),
            cfg!(feature = "camoufox")
        )
    })?;
    Ok(())
}
