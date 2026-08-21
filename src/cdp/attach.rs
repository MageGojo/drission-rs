//! 可克隆的「附着能力」:Context / Browser 共用同一套 attach 参数,避免把 Browser 做成巨大上帝对象。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde_json::json;

use crate::cdp::core::CdpCore;
use crate::cdp::stealth;
use crate::cdp::tab::ChromiumTab;
use crate::protocol::Connection;
use crate::{Error, Result};

/// 从浏览器进程附着一个 page target 所需的会话级参数(Clone 廉价)。
#[derive(Clone)]
pub(crate) struct CdpAttach {
    pub conn: Connection,
    pub download_dir: Option<PathBuf>,
    pub stealth: bool,
    pub headless: bool,
    pub full_ua_metadata: bool,
    pub ua_override: Option<String>,
    pub ua_full_version: Option<String>,
    pub platform_version: Option<String>,
    pub init_scripts: Vec<String>,
    pub container_js: Option<String>,
    /// 命名 Context 注册表:`name → browserContextId`。
    pub named_contexts: Arc<Mutex<HashMap<String, String>>>,
}

impl CdpAttach {
    pub(crate) fn empty_registry() -> Arc<Mutex<HashMap<String, String>>> {
        Arc::new(Mutex::new(HashMap::new()))
    }

    /// 附着到 target,并记录所属 BrowserContext。
    ///
    /// `dispose_context_on_close=true` 时 `Tab::close` 会顺带 `disposeBrowserContext`
    /// (池里 `new_tab_with` 的一次性隔离 context);命名 Context 开的标签传 `false`。
    pub(crate) async fn attach_in_context(
        &self,
        target_id: String,
        browser_context_id: Option<String>,
        dispose_context_on_close: bool,
    ) -> Result<ChromiumTab> {
        let a = self
            .conn
            .send(
                "Target.attachToTarget",
                json!({ "targetId": target_id, "flatten": true }),
                None,
            )
            .await?;
        let session_id = a["sessionId"]
            .as_str()
            .ok_or_else(|| Error::msg("CDP: 附着无 sessionId"))?
            .to_string();
        let core = CdpCore::new(
            self.conn.clone(),
            session_id,
            target_id,
            self.download_dir.clone(),
            browser_context_id,
        );
        if !dispose_context_on_close {
            core.keep_context_on_close();
        }
        let _ = core.send("Page.enable", json!({})).await;
        if let Some(dir) = &self.download_dir {
            let _ = std::fs::create_dir_all(dir);
            let _ = core
                .send(
                    "Browser.setDownloadBehavior",
                    json!({
                        "behavior": "allow",
                        "downloadPath": dir.display().to_string(),
                        "eventsEnabled": true
                    }),
                )
                .await;
        }
        // 反检测关键点:**绝不调用 `Runtime.enable`**。
        if self.stealth {
            let _ = core
                .send(
                    "Page.addScriptToEvaluateOnNewDocument",
                    json!({ "source": stealth::STEALTH_JS }),
                )
                .await;
            if self.headless {
                let _ = core
                    .send(
                        "Page.addScriptToEvaluateOnNewDocument",
                        json!({ "source": stealth::headless_screen_js() }),
                    )
                    .await;
            }
            if let Some(js) = &self.container_js {
                let _ = core
                    .send(
                        "Page.addScriptToEvaluateOnNewDocument",
                        json!({ "source": js }),
                    )
                    .await;
            }
        }
        for src in &self.init_scripts {
            let _ = core
                .send(
                    "Page.addScriptToEvaluateOnNewDocument",
                    json!({ "source": src }),
                )
                .await;
        }
        if self.full_ua_metadata {
            if let (Some(ua), Some(full)) = (&self.ua_override, &self.ua_full_version) {
                crate::cdp::browser::apply_ua_metadata(
                    &core,
                    ua,
                    full,
                    self.platform_version.as_deref(),
                )
                .await;
            }
        }
        Ok(ChromiumTab::new(core))
    }

    pub(crate) fn lookup_named(&self, name: &str) -> Option<String> {
        self.named_contexts
            .lock()
            .ok()
            .and_then(|g| g.get(name).cloned())
    }

    pub(crate) fn register_named(&self, name: &str, id: &str) {
        if let Ok(mut g) = self.named_contexts.lock() {
            g.insert(name.to_string(), id.to_string());
        }
    }

    pub(crate) fn unregister_named(&self, name: &str) {
        if let Ok(mut g) = self.named_contexts.lock() {
            g.remove(name);
        }
    }
}
