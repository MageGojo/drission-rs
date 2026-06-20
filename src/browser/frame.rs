//! 子框架 [`Frame`]:对应 DrissionPage 的 iframe/frame 对象。
//!
//! 通过 [`Tab::get_frame`](crate::browser::Tab::get_frame) 或
//! [`Element::content_frame`](crate::browser::Element::content_frame) 获得。`Frame` 在**该帧自己的
//! 执行上下文**里查元素 / 执行 JS——返回的 [`Element`] 也归属该帧,后续 `click`/`text`/`input` 等都正确。
//!
//! ```ignore
//! let frame = tab.get_frame("tag:iframe").await?;   // 或 tab.ele("#ifr").await?.content_frame().await?
//! let btn = frame.ele("text:提交").await?;           // 在 iframe 内查找
//! btn.click().await?;                                 // 点击 iframe 内元素
//! println!("{}", frame.html().await?);
//! ```

use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tokio::time::{Instant, sleep};

use crate::browser::element::Element;
use crate::browser::static_element::StaticElement;
use crate::browser::tab::{TabCore, multi_query_expr, single_query_expr};
use crate::locator;
use crate::{Error, Result};

/// 一个子框架(iframe/frame)句柄。克隆代价低(共享 Tab 内核)。
#[derive(Clone)]
pub struct Frame {
    core: Arc<TabCore>,
    frame_id: String,
}

impl Frame {
    pub(crate) fn new(core: Arc<TabCore>, frame_id: String) -> Self {
        Self { core, frame_id }
    }

    /// 该帧的 frameId。
    pub fn frame_id(&self) -> &str {
        &self.frame_id
    }

    /// 在该帧上下文执行 JS 表达式,返回其值。
    pub async fn run_js(&self, script: &str) -> Result<Value> {
        self.core.evaluate_in(&self.frame_id, script, true).await
    }

    /// 该帧文档的 HTML。
    pub async fn html(&self) -> Result<String> {
        Ok(self
            .core
            .evaluate_in(&self.frame_id, "document.documentElement.outerHTML", true)
            .await?
            .as_str()
            .unwrap_or_default()
            .to_string())
    }

    /// 该帧的 URL。
    pub async fn url(&self) -> Result<String> {
        Ok(self
            .core
            .evaluate_in(&self.frame_id, "location.href", true)
            .await?
            .as_str()
            .unwrap_or_default()
            .to_string())
    }

    /// 在该帧内查找单个元素(DP 定位语法),超时内轮询等待。
    pub async fn ele(&self, selector: &str) -> Result<Element> {
        let deadline = Instant::now() + self.core.timeout();
        loop {
            if let Some(el) = self.find_once(selector).await? {
                return Ok(el);
            }
            if Instant::now() >= deadline {
                return Err(Error::ElementNotFound(selector.to_string()));
            }
            sleep(Duration::from_millis(100)).await;
        }
    }

    /// 在该帧内查找多个元素(立即返回,不等待)。
    pub async fn eles(&self, selector: &str) -> Result<Vec<Element>> {
        let query = locator::parse(selector);
        let expr = multi_query_expr(&query);
        let result = self.core.evaluate_in(&self.frame_id, &expr, false).await?;
        let Some(array_object_id) = result.get("objectId").and_then(|v| v.as_str()) else {
            return Ok(Vec::new());
        };
        let oids = self
            .core
            .node_array_object_ids(&self.frame_id, array_object_id)
            .await?;
        Ok(oids
            .into_iter()
            .map(|oid| Element::new_in_frame(self.core.clone(), oid, self.frame_id.clone()))
            .collect())
    }

    /// 单次查找(不等待)。
    async fn find_once(&self, selector: &str) -> Result<Option<Element>> {
        let query = locator::parse(selector);
        let expr = single_query_expr(&query);
        let result = self.core.evaluate_in(&self.frame_id, &expr, false).await?;
        Ok(result
            .get("objectId")
            .and_then(|v| v.as_str())
            .map(|oid| {
                Element::new_in_frame(self.core.clone(), oid.to_string(), self.frame_id.clone())
            }))
    }

    /// 解析该帧 HTML,取第一个匹配的**静态元素**。
    pub async fn s_ele(&self, selector: &str) -> Result<StaticElement> {
        let html = self.html().await?;
        StaticElement::parse(&html)?.ele(selector)
    }

    /// 解析该帧 HTML,取全部匹配的**静态元素**。
    pub async fn s_eles(&self, selector: &str) -> Result<Vec<StaticElement>> {
        let html = self.html().await?;
        StaticElement::parse(&html)?.eles(selector)
    }
}
