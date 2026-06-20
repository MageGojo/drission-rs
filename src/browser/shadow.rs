//! Shadow DOM 根 [`ShadowRoot`]:对应 DrissionPage 的 `ele.shadow_root`。
//!
//! 由 [`Element::shadow_root`](crate::browser::Element::shadow_root) 获得。它是一个**查找上下文**
//! (并非元素本身,故没有 `click`/`text` 等元素操作),用于在 shadow 内继续 `ele`/`eles`。
//!
//! ```ignore
//! let host = tab.ele("#my-widget").await?;     // 挂着 open shadow root 的宿主元素
//! let root = host.shadow_root().await?;
//! let btn = root.ele("button.submit").await?;   // 在 shadow 内查找
//! btn.click().await?;
//! ```
//!
//! 说明:shadow 内**不支持** `document.evaluate`(它不进 shadow,且 `ShadowRoot` 不是 `document`),
//! 故定位仅支持 **CSS 系**(`#`/`.`/`tag:`/`@attr`/`css:`);传 `xpath:` 会报错。只有
//! `mode:'open'` 的 shadow root 可被脚本访问(`closed` 的 `el.shadowRoot` 为 `null`)。

use std::sync::Arc;

use serde_json::{Value, json};

use crate::browser::element::Element;
use crate::browser::static_element::StaticElement;
use crate::browser::tab::TabCore;
use crate::locator::{self, Query};
use crate::{Error, Result};

/// 一个 open shadow root 句柄(shadow 内的查找上下文)。克隆代价低(共享 Tab 内核)。
#[derive(Clone)]
pub struct ShadowRoot {
    core: Arc<TabCore>,
    object_id: String,
    /// shadow root 所属 frame;`None` 表示主帧。与宿主元素一致(shadow 不创建新执行上下文)。
    frame_id: Option<String>,
}

impl ShadowRoot {
    pub(crate) fn new(core: Arc<TabCore>, object_id: String, frame_id: Option<String>) -> Self {
        Self {
            core,
            object_id,
            frame_id,
        }
    }

    /// 底层 objectId(调试用)。
    pub fn object_id(&self) -> &str {
        &self.object_id
    }

    fn frame_id_ref(&self) -> &str {
        self.frame_id.as_deref().unwrap_or(&self.core.main_frame_id)
    }

    /// 在该 shadow root 上调用函数声明(第一个参数为 shadow root 本身),在其所属 frame 的上下文执行。
    async fn call(&self, declaration: &str, extra: Vec<Value>, by_value: bool) -> Result<Value> {
        let mut args = vec![json!({ "objectId": self.object_id })];
        args.extend(extra);
        match &self.frame_id {
            Some(fid) => {
                self.core
                    .call_function_in(fid, declaration, args, by_value)
                    .await
            }
            None => self.core.call_function(declaration, args, by_value).await,
        }
    }

    /// 把 DP 定位语法解析成 shadow 内可用的 CSS;`xpath:` 报错(shadow 内无 `document.evaluate`)。
    fn css_of(selector: &str) -> Result<String> {
        match locator::parse(selector) {
            Query::Css(sel) => Ok(sel),
            Query::Xpath(_) => Err(Error::Other(
                "shadow root 内不支持 xpath 定位(document.evaluate 不进 shadow);请用 CSS / tag: / @attr".into(),
            )),
        }
    }

    fn make_element(&self, object_id: String) -> Element {
        match &self.frame_id {
            Some(fid) => Element::new_in_frame(self.core.clone(), object_id, fid.clone()),
            None => Element::new(self.core.clone(), object_id),
        }
    }

    /// 在 shadow 内查找单个元素(CSS 系定位);找不到返回 [`Error::ElementNotFound`]。
    pub async fn ele(&self, selector: &str) -> Result<Element> {
        let css = Self::css_of(selector)?;
        let result = self
            .call(
                "(root, sel) => root.querySelector(sel)",
                vec![json!({ "value": css })],
                false,
            )
            .await?;
        match result.get("objectId").and_then(|v| v.as_str()) {
            Some(oid) => Ok(self.make_element(oid.to_string())),
            None => Err(Error::ElementNotFound(selector.to_string())),
        }
    }

    /// 在 shadow 内查找多个元素(CSS 系定位;立即返回,不等待)。
    pub async fn eles(&self, selector: &str) -> Result<Vec<Element>> {
        let css = Self::css_of(selector)?;
        let result = self
            .call(
                "(root, sel) => Array.from(root.querySelectorAll(sel))",
                vec![json!({ "value": css })],
                false,
            )
            .await?;
        let Some(array_object_id) = result.get("objectId").and_then(|v| v.as_str()) else {
            return Ok(Vec::new());
        };
        let oids = self
            .core
            .node_array_object_ids(self.frame_id_ref(), array_object_id)
            .await?;
        Ok(oids.into_iter().map(|oid| self.make_element(oid)).collect())
    }

    /// shadow 内的 HTML(`innerHTML`)。
    pub async fn html(&self) -> Result<String> {
        let v = self
            .call("root => root.innerHTML ?? ''", vec![], true)
            .await?;
        Ok(v.as_str().unwrap_or_default().to_string())
    }

    /// 在该 shadow root 上执行 JS(函数体内 `root` 即本 shadow root)。返回其值。
    pub async fn run_js(&self, body: &str) -> Result<Value> {
        let decl = format!("(root) => {{ {body} }}");
        self.call(&decl, vec![], true).await
    }

    /// 解析 shadow 内 HTML,取第一个匹配的**静态元素**。
    pub async fn s_ele(&self, selector: &str) -> Result<StaticElement> {
        StaticElement::parse(&self.html().await?)?.ele(selector)
    }

    /// 解析 shadow 内 HTML,取全部匹配的**静态元素**。
    pub async fn s_eles(&self, selector: &str) -> Result<Vec<StaticElement>> {
        StaticElement::parse(&self.html().await?)?.eles(selector)
    }
}

#[cfg(test)]
mod tests {
    use super::ShadowRoot;

    #[test]
    fn css_of_accepts_css_rejects_xpath() {
        // CSS 系定位(#/./tag:/css:)可在 shadow 内用 querySelector。
        assert_eq!(ShadowRoot::css_of("#x").unwrap(), "#x");
        assert_eq!(ShadowRoot::css_of(".cls").unwrap(), ".cls");
        assert_eq!(ShadowRoot::css_of("tag:button").unwrap(), "button");
        assert_eq!(ShadowRoot::css_of("css:div.box").unwrap(), "div.box");
        // xpath(显式 xpath: 或 @attr/text 这类落到 XPath 的)在 shadow 内不支持,须报错。
        assert!(ShadowRoot::css_of("xpath://button").is_err());
        assert!(ShadowRoot::css_of("@id:kw").is_err());
        assert!(ShadowRoot::css_of("text:提交").is_err());
    }
}
