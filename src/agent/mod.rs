//! Browser Agent Runtime:语义动作,不让 LLM 直接写 DOM 选择器。
//!
//! ```text
//! observe()  →  AX / snapshot（role + name + ref）
//! find()     →  Locator Resolver（AX → DOM/文本 → 截图空间 → OCR）
//! click / type_text / wait_for
//! ```
//!
//! 现有 `Tab` / `ele` / `wait` 全部保留。本模块是编排层。

mod visual;

use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::time::{Instant, sleep};

use crate::ai_snapshot::AiSnapshot;
use crate::cdp::{ChromiumBrowser, ChromiumElement, ChromiumTab};
use crate::locator::Locator;
use crate::{Error, Result};

/// 一次观察:大纲给人/LLM 读,actions 给下一步点按。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Observation {
    pub url: String,
    pub title: String,
    /// `role "name"` 大纲,比 HTML 小一个数量级。
    pub outline: String,
    pub actions: Vec<SemanticTarget>,
    pub partial: bool,
}

/// 语义目标。LLM 只应看见 role / name,不要自己拼 CSS。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticTarget {
    pub role: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(skip)]
    pub locator: Option<Locator>,
}

/// 解析后的可点元素。
pub struct AgentElement {
    ele: ChromiumElement,
    target: SemanticTarget,
}

impl AgentElement {
    pub fn target(&self) -> &SemanticTarget {
        &self.target
    }
    pub async fn click(self) -> Result<()> {
        self.ele.click().await
    }
    pub async fn fill(self, text: &str) -> Result<()> {
        self.ele.clear().await.ok();
        self.ele.input(text).await
    }
    pub fn inner(self) -> ChromiumElement {
        self.ele
    }
}

/// 包一层 [`ChromiumTab`]:observe / 语义查找 / 等待。
#[derive(Clone)]
pub struct AgentPage {
    tab: ChromiumTab,
}

impl AgentPage {
    pub fn new(tab: ChromiumTab) -> Self {
        Self { tab }
    }

    pub fn tab(&self) -> &ChromiumTab {
        &self.tab
    }

    /// 无障碍/交互大纲。不要拿这个去喂整页 HTML。
    pub async fn observe(&self) -> Result<Observation> {
        let url = self.tab.url().await.unwrap_or_default();
        let title = self.tab.title().await.unwrap_or_default();
        let snap = self.tab.ai_snapshot(None, None, None).await.ok();
        let ax = self.tab.ax_tree().await.ok();

        let outline = match ax {
            Some(ref tree) => {
                let o = tree.to_outline();
                if o.trim().is_empty() {
                    snap.as_ref()
                        .map(|s| s.snapshot.clone())
                        .unwrap_or_default()
                } else {
                    o
                }
            }
            None => snap
                .as_ref()
                .map(|s| s.snapshot.clone())
                .unwrap_or_default(),
        };

        Ok(Observation {
            url,
            title,
            outline,
            actions: actions_from_snapshot(snap.as_ref()),
            partial: snap.as_ref().map(|s| s.partial).unwrap_or(false),
        })
    }

    /// `find("登录按钮").click().await`。要元素本身用 [`locate`](Self::locate)。
    pub fn find(&self, hint: impl Into<Locator>) -> FindOp {
        FindOp {
            tab: self.tab.clone(),
            loc: hint.into(),
        }
    }

    pub async fn locate(&self, hint: impl Into<Locator>) -> Result<AgentElement> {
        resolve(&self.tab, hint.into()).await
    }

    pub async fn click(&self, hint: impl Into<Locator>) -> Result<()> {
        self.find(hint).click().await
    }

    pub async fn type_text(&self, field: impl Into<Locator>, text: &str) -> Result<()> {
        self.find(field).fill(text).await
    }

    /// 等标题、URL 或可见文本出现该子串。一次轮询里三项一起看,不要连等三次超时。
    pub async fn wait_for(&self, text: &str) -> Result<bool> {
        self.wait_for_timeout(text, Duration::from_secs(10)).await
    }

    pub async fn wait_for_timeout(&self, text: &str, timeout: Duration) -> Result<bool> {
        let deadline = Instant::now() + timeout;
        loop {
            let title = self.tab.title().await.unwrap_or_default();
            let url = self.tab.url().await.unwrap_or_default();
            let body = self
                .tab
                .run_js("document.body ? document.body.innerText : ''")
                .await
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default();
            if title.contains(text) || url.contains(text) || body.contains(text) {
                return Ok(true);
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            sleep(Duration::from_millis(100)).await;
        }
    }
}

fn actions_from_snapshot(snap: Option<&AiSnapshot>) -> Vec<SemanticTarget> {
    let Some(snap) = snap else {
        return Vec::new();
    };
    snap.refs
        .iter()
        .map(|(_, r)| SemanticTarget {
            role: r.role.clone(),
            name: r.name.clone(),
            value: r.value.clone(),
            locator: Some(Locator::parse(&r.selector)),
        })
        .collect()
}

/// `find(hint)` 的后续动作。
pub struct FindOp {
    tab: ChromiumTab,
    loc: Locator,
}

impl FindOp {
    pub async fn click(self) -> Result<()> {
        resolve(&self.tab, self.loc).await?.click().await
    }
    pub async fn fill(self, text: &str) -> Result<()> {
        resolve(&self.tab, self.loc).await?.fill(text).await
    }
    pub async fn ele(self) -> Result<AgentElement> {
        resolve(&self.tab, self.loc).await
    }
}

/// AX 只用来确认目标;真正点的是 role/name 或最内层文本,不用宽 `contains` 点到 html/body。
async fn resolve(tab: &ChromiumTab, loc: Locator) -> Result<AgentElement> {
    let sel = loc.as_selector();
    if let Ok(ele) = tab.ele(&sel).await {
        if is_interactive(&ele).await {
            return Ok(wrap_ele(ele, &loc));
        }
    }
    if let Some((_, Some(name))) = loc.role_name() {
        let lit = crate::locator::xpath_literal(name);
        let inner = format!(
            "xpath://*[contains(normalize-space(.), {lit}) and not(.//*[contains(normalize-space(.), {lit})])]"
        );
        if let Ok(ele) = tab.ele(&inner).await {
            if is_interactive(&ele).await {
                return Ok(wrap_ele(ele, &loc));
            }
        }
    }
    if let Some(ele) = visual::resolve_near_text(tab, &loc).await? {
        return Ok(ele);
    }
    if let Some(ele) = visual::resolve_ocr(tab, &loc).await? {
        return Ok(ele);
    }
    Err(Error::ElementNotFound(format!(
        "semantic target not found: {sel}"
    )))
}

async fn is_interactive(ele: &ChromiumElement) -> bool {
    let tag = ele.tag().await.unwrap_or_default().to_ascii_lowercase();
    if matches!(
        tag.as_str(),
        "button" | "a" | "input" | "textarea" | "select"
    ) {
        return true;
    }
    matches!(
        ele.attr("role").await.ok().flatten().as_deref(),
        Some("button" | "link" | "textbox" | "searchbox")
    )
}

pub(crate) fn wrap_ele(ele: ChromiumElement, loc: &Locator) -> AgentElement {
    let (role, name) = match loc.role_name() {
        Some((r, n)) => (r.to_string(), n.unwrap_or("").to_string()),
        None => (String::new(), String::new()),
    };
    AgentElement {
        ele,
        target: SemanticTarget {
            role,
            name,
            value: None,
            locator: Some(loc.clone()),
        },
    }
}

/// 浏览器上的 Agent 入口。不拥有 Browser。
pub struct AgentBrowser<'a> {
    browser: &'a ChromiumBrowser,
}

impl<'a> AgentBrowser<'a> {
    pub fn new(browser: &'a ChromiumBrowser) -> Self {
        Self { browser }
    }

    pub fn inner(&self) -> &ChromiumBrowser {
        self.browser
    }

    pub async fn page(&self) -> Result<AgentPage> {
        Ok(AgentPage::new(self.browser.latest_tab().await?))
    }

    pub async fn observe(&self) -> Result<Observation> {
        self.page().await?.observe().await
    }
}

impl ChromiumTab {
    /// Agent 编排入口。底层仍是这个 Tab。
    pub fn agent(&self) -> AgentPage {
        AgentPage::new(self.clone())
    }
}

impl ChromiumBrowser {
    pub fn agent(&self) -> AgentBrowser<'_> {
        AgentBrowser::new(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semantic_locator_from_zh() {
        let loc = Locator::parse("登录按钮");
        assert_eq!(loc.role_name(), Some(("button", Some("登录"))));
    }
}
