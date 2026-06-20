//! CDP 后端的 **iframe 内容帧** 查找句柄 [`ChromiumFrame`](对齐 camoufox `Frame`)。
//!
//! 关键:用 `Page.createIsolatedWorld{frameId}` 在该帧建一个**隔离世界**拿到 `executionContextId`,
//! 之后 `Runtime.evaluate{contextId}` 即在该帧上下文查元素/读 HTML/跑 JS —— **无需 `Runtime.enable`**
//! (不破坏反检测),且隔离世界与主世界共享 DOM,故 `document.querySelector` 命中帧内真实元素。
//! 跨域 iframe 同样可用(隔离世界建在该帧内、访问其同源 DOM)。

use std::sync::Arc;

use serde_json::{Value, json};

use crate::cdp::core::CdpCore;
use crate::cdp::element::ChromiumElement;
use crate::cdp::tab::doc_query_expr;
use crate::{Error, Result};

/// 一个 iframe 内容帧的查找上下文(由 [`Tab::get_frame`](crate::cdp::ChromiumTab) /
/// [`ChromiumElement::content_frame`](crate::cdp::ChromiumElement) 返回)。
pub struct ChromiumFrame {
    core: Arc<CdpCore>,
    context_id: i64,
}

impl ChromiumFrame {
    /// 由 `<iframe>` 元素 objectId 取其内容帧。
    pub(crate) async fn from_iframe(core: Arc<CdpCore>, iframe_object_id: &str) -> Result<Self> {
        let d = core
            .send("DOM.describeNode", json!({ "objectId": iframe_object_id }))
            .await?;
        let frame_id = d["node"]["frameId"]
            .as_str()
            .ok_or_else(|| Error::msg("CDP: 该元素不是 iframe 或无 frameId"))?
            .to_string();
        Self::from_frame_id(core, &frame_id).await
    }

    /// 由 frameId 建隔离世界拿执行上下文。
    pub(crate) async fn from_frame_id(core: Arc<CdpCore>, frame_id: &str) -> Result<Self> {
        let w = core
            .send(
                "Page.createIsolatedWorld",
                json!({ "frameId": frame_id, "worldName": "drission_frame" }),
            )
            .await?;
        let context_id = w["executionContextId"]
            .as_i64()
            .ok_or_else(|| Error::msg("CDP: createIsolatedWorld 无 executionContextId"))?;
        Ok(Self { core, context_id })
    }

    async fn eval_handle(&self, expr: &str) -> Result<Option<String>> {
        let r = self
            .core
            .send(
                "Runtime.evaluate",
                json!({ "expression": expr, "contextId": self.context_id, "returnByValue": false, "awaitPromise": true }),
            )
            .await?;
        Ok(r["result"]["objectId"].as_str().map(str::to_string))
    }

    async fn eval_value(&self, expr: &str) -> Result<Value> {
        let r = self
            .core
            .send(
                "Runtime.evaluate",
                json!({ "expression": expr, "contextId": self.context_id, "returnByValue": true, "awaitPromise": true }),
            )
            .await?;
        Ok(r["result"]["value"].clone())
    }

    /// 在帧内查找第一个匹配元素(DP 定位语法,CSS/xpath 均可)。
    pub async fn ele(&self, selector: &str) -> Result<ChromiumElement> {
        match self.eval_handle(&doc_query_expr(selector, true)).await? {
            Some(oid) => Ok(ChromiumElement::new(self.core.clone(), oid)),
            None => Err(Error::ElementNotFound(selector.to_string())),
        }
    }

    /// 在帧内查找所有匹配元素。
    pub async fn eles(&self, selector: &str) -> Result<Vec<ChromiumElement>> {
        let Some(arr) = self.eval_handle(&doc_query_expr(selector, false)).await? else {
            return Ok(Vec::new());
        };
        let oids = self.core.array_object_ids(&arr).await?;
        Ok(oids
            .into_iter()
            .map(|oid| ChromiumElement::new(self.core.clone(), oid))
            .collect())
    }

    /// 在帧上下文执行 JS 表达式。
    pub async fn run_js(&self, expression: &str) -> Result<Value> {
        self.eval_value(expression).await
    }

    /// 帧内 HTML。
    pub async fn html(&self) -> Result<String> {
        Ok(self
            .eval_value("document.documentElement.outerHTML")
            .await?
            .as_str()
            .unwrap_or_default()
            .to_string())
    }

    /// 帧内静态查找(解析帧 HTML 快照)。
    pub async fn s_ele(&self, selector: &str) -> Result<crate::static_element::StaticElement> {
        crate::static_element::StaticElement::parse(&self.html().await?)?.ele(selector)
    }

    /// 帧内静态查找所有。
    pub async fn s_eles(
        &self,
        selector: &str,
    ) -> Result<Vec<crate::static_element::StaticElement>> {
        crate::static_element::StaticElement::parse(&self.html().await?)?.eles(selector)
    }
}
