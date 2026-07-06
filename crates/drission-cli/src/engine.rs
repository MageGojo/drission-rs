use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Result, anyhow};
use serde_json::{Value, json};

use crate::backend::{BackendBrowser, BackendTab, ensure_backend_available};
use crate::protocol::{BackendKind, EngineCommand, TabSummary};

pub struct BrowserState {
    backend: BackendKind,
    browser: BackendBrowser,
    tabs: Vec<TabSlot>,
    active: Option<u64>,
    next_tab_id: u64,
}

struct TabSlot {
    id: u64,
    tab: BackendTab,
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
            next_tab_id: 1,
        };
        let tab = state.browser.new_tab(None).await?;
        state.insert_tab(tab);
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
            EngineCommand::Open { url } => {
                let tab = self.browser.new_tab(Some(&url)).await?;
                let id = self.insert_tab(tab);
                json!({ "tabId": id, "url": url, "active": true })
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
            EngineCommand::Ax { format } => self.active_tab()?.ax(format).await?,
            EngineCommand::Html => json!({ "html": self.active_tab()?.html().await? }),
            EngineCommand::Text { selector } => {
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
            EngineCommand::ListenStart { keywords, xhr_only } => {
                self.active_tab()?.listen_start(&keywords, xhr_only).await?
            }
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
        };
        Ok(EngineResult { data, stop: false })
    }

    async fn status(&self) -> Result<Value> {
        Ok(json!({
            "pid": std::process::id(),
            "backend": self.backend,
            "activeTabId": self.active,
            "tabs": self.tab_summaries().await?,
        }))
    }

    fn insert_tab(&mut self, tab: BackendTab) -> u64 {
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        self.tabs.push(TabSlot { id, tab });
        self.active = Some(id);
        id
    }

    fn active_tab(&self) -> Result<&BackendTab> {
        let id = self.active.ok_or_else(|| anyhow!("no active tab"))?;
        Ok(&self.find_slot(id)?.tab)
    }

    fn find_slot(&self, id: u64) -> Result<&TabSlot> {
        self.tabs
            .iter()
            .find(|slot| slot.id == id)
            .ok_or_else(|| anyhow!("tab {id} not found"))
    }

    async fn tab_summaries(&self) -> Result<Vec<TabSummary>> {
        let mut out = Vec::with_capacity(self.tabs.len());
        for slot in &self.tabs {
            out.push(TabSummary {
                id: slot.id,
                active: self.active == Some(slot.id),
                title: slot.tab.title().await.unwrap_or_default(),
                url: slot.tab.url().await.unwrap_or_default(),
            });
        }
        Ok(out)
    }
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
