//! CDP 后端的 **open Shadow DOM** 查找句柄 [`ChromiumShadowRoot`](对齐 camoufox `ShadowRoot`)。
//!
//! `ele.shadow_root()` 取 `this.shadowRoot`(仅 open),在其内部查找元素/读 HTML/跑 JS。
//! shadow 内**仅支持 CSS 系定位**(无 `document.evaluate`,`xpath:` 报错)。

use std::sync::Arc;

use serde_json::{Value, json};

use crate::cdp::core::CdpCore;
use crate::cdp::element::ChromiumElement;
use crate::locator::{self, Query};
use crate::{Error, Result};

/// 一个 open shadow root 的查找上下文(由 [`ChromiumElement::shadow_root`] 返回)。
pub struct ChromiumShadowRoot {
    core: Arc<CdpCore>,
    /// shadowRoot 节点的 objectId。
    object_id: String,
}

impl ChromiumShadowRoot {
    pub(crate) fn new(core: Arc<CdpCore>, object_id: String) -> Self {
        Self { core, object_id }
    }

    /// 在 shadow 内查找第一个匹配元素(**仅 CSS**)。
    pub async fn ele(&self, selector: &str) -> Result<ChromiumElement> {
        let css = css_only(selector)?;
        match self
            .core
            .call_handle(
                &self.object_id,
                "function(s){ return this.querySelector(s); }",
                vec![json!({ "value": css })],
            )
            .await?
        {
            Some(oid) => Ok(ChromiumElement::new(self.core.clone(), oid)),
            None => Err(Error::ElementNotFound(selector.to_string())),
        }
    }

    /// 在 shadow 内查找所有匹配元素(**仅 CSS**)。
    pub async fn eles(&self, selector: &str) -> Result<Vec<ChromiumElement>> {
        let css = css_only(selector)?;
        let Some(arr) = self
            .core
            .call_handle(
                &self.object_id,
                "function(s){ return Array.from(this.querySelectorAll(s)); }",
                vec![json!({ "value": css })],
            )
            .await?
        else {
            return Ok(Vec::new());
        };
        let oids = self.core.array_object_ids(&arr).await?;
        Ok(oids
            .into_iter()
            .map(|oid| ChromiumElement::new(self.core.clone(), oid))
            .collect())
    }

    /// shadow 内 HTML(`innerHTML`)。
    pub async fn html(&self) -> Result<String> {
        let v = self
            .core
            .call_value(
                &self.object_id,
                "function(){ return this.innerHTML ?? ''; }",
                vec![],
            )
            .await?;
        Ok(v.as_str().unwrap_or_default().to_string())
    }

    /// 在 shadow root 上跑 JS(`this` 即 shadowRoot)。
    pub async fn run_js(&self, body: &str) -> Result<Value> {
        self.core
            .call_value(&self.object_id, &format!("function(){{ {body} }}"), vec![])
            .await
    }

    /// 静态查找(解析 shadow 的 HTML 快照)。
    pub async fn s_ele(&self, selector: &str) -> Result<crate::static_element::StaticElement> {
        crate::static_element::StaticElement::parse(&self.html().await?)?.ele(selector)
    }

    /// 静态查找所有(解析 shadow 的 HTML 快照)。
    pub async fn s_eles(
        &self,
        selector: &str,
    ) -> Result<Vec<crate::static_element::StaticElement>> {
        crate::static_element::StaticElement::parse(&self.html().await?)?.eles(selector)
    }
}

/// shadow 内只接受 CSS 定位;`xpath:` 报错(shadow 树无 `document.evaluate`)。
fn css_only(selector: &str) -> Result<String> {
    match locator::parse(selector) {
        Query::Css(s) => Ok(s),
        Query::Xpath(_) => Err(Error::msg("Shadow DOM 内不支持 xpath,请用 CSS 定位")),
    }
}
