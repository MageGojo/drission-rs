//! 静态元素 [`StaticElement`]:对应 DrissionPage 的"静态元素"(`s_ele`/`s_eles`)。
//!
//! 与实时 [`Element`](crate::browser::Element) 不同,静态元素是把某一时刻的 **HTML 快照**
//! 离线解析后(基于 `scraper`/`html5ever`)在内存 DOM 上做查询与读取——**不再与浏览器通信**,
//! 因此读取大量字段时极快,适合"抓到页面后批量解析"的爬虫场景。
//!
//! 定位:DP 的 `@attr` / `text:` / `tag:` / CSS 等常见写法均支持;显式 `xpath:` 走内置
//! [XPath 1.0 子集求值器](crate::browser::xpath)(覆盖 `//`/`/`、`*`、`[n]`/`[last()]`、
//! `@a="v"`、`contains(...)`、`and`/`or`/`not` 等),不支持的轴/语法会报错并提示改用实时
//! [`Tab::ele`](crate::browser::Tab::ele)。
//!
//! ```ignore
//! tab.get("https://example.com").await?;
//! let h1 = tab.s_ele("tag:h1").await?;      // 解析当前 HTML,取第一个 h1
//! println!("{}", h1.text()?);
//! for a in tab.s_eles("tag:a").await? {      // 批量:所有链接
//!     println!("{:?} -> {:?}", a.text()?, a.attr("href")?);
//! }
//! // 也可离线解析任意 HTML 字符串(例如监听到的响应体):
//! let root = StaticElement::parse("<ul><li>a</li><li>b</li></ul>")?;
//! ```

use std::rc::Rc;

use ego_tree::NodeId;
use scraper::{ElementRef, Html, Selector};

use crate::locator::{self, StaticQuery};
use crate::{Error, Result};

/// 一个静态(离线解析)元素句柄。克隆代价低(共享同一份已解析文档)。
///
/// 注意:内部持有 `scraper` 的解析树(基于 `Rc` 的 `Tendril`),故 `StaticElement` **不是 `Send`**,
/// 不能跨线程移动(不要在 `tokio::spawn` 的任务之间传递);在单个任务内顺序使用没有任何问题。
/// 文档用 `Rc`(而非 `Arc`)共享:反正解析树不可跨线程,`Rc` 更轻量。
#[derive(Clone)]
pub struct StaticElement {
    doc: Rc<Html>,
    id: NodeId,
}

impl StaticElement {
    /// 离线解析一段 HTML(文档或片段均可),返回其根元素。
    pub fn parse(html: &str) -> Result<Self> {
        let doc = Rc::new(Html::parse_document(html));
        Self::root(doc)
    }

    /// 以已解析文档的根元素构造。
    pub(crate) fn root(doc: Rc<Html>) -> Result<Self> {
        let id = doc.root_element().id();
        Ok(Self { doc, id })
    }

    /// 取得当前节点的 `ElementRef`(每次按 id 重新定位,避免自借用生命周期问题)。
    fn element(&self) -> Result<ElementRef<'_>> {
        let node = self
            .doc
            .tree
            .get(self.id)
            .ok_or_else(|| Error::StaleElement("静态节点不存在".into()))?;
        ElementRef::wrap(node).ok_or_else(|| Error::StaleElement("静态节点不是元素".into()))
    }

    /// 标签名(小写)。
    pub fn tag(&self) -> Result<String> {
        Ok(self.element()?.value().name().to_ascii_lowercase())
    }

    /// 元素可见文本(所有后代文本拼接,首尾去空白)。
    pub fn text(&self) -> Result<String> {
        Ok(self.element()?.text().collect::<String>().trim().to_string())
    }

    /// 读取属性;不存在返回 `None`。
    pub fn attr(&self, name: &str) -> Result<Option<String>> {
        Ok(self
            .element()?
            .value()
            .attr(name)
            .map(|s| s.to_string()))
    }

    /// 全部属性(名, 值)。
    pub fn attrs(&self) -> Result<Vec<(String, String)>> {
        Ok(self
            .element()?
            .value()
            .attrs()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect())
    }

    /// 该元素的 outer HTML。
    pub fn html(&self) -> Result<String> {
        Ok(self.element()?.html())
    }

    /// 该元素的 inner HTML。
    pub fn inner_html(&self) -> Result<String> {
        Ok(self.element()?.inner_html())
    }

    /// 在本元素子树内查找单个元素(DP 定位语法)。找不到 → [`Error::ElementNotFound`]。
    pub fn ele(&self, selector: &str) -> Result<StaticElement> {
        let q = locator::parse_static(selector);
        let ids = collect_ids(self.element()?, &q)?;
        match ids.into_iter().next() {
            Some(id) => Ok(StaticElement {
                doc: self.doc.clone(),
                id,
            }),
            None => Err(Error::ElementNotFound(selector.to_string())),
        }
    }

    /// 在本元素子树内查找全部匹配元素(DP 定位语法)。
    pub fn eles(&self, selector: &str) -> Result<Vec<StaticElement>> {
        let q = locator::parse_static(selector);
        let ids = collect_ids(self.element()?, &q)?;
        Ok(ids
            .into_iter()
            .map(|id| StaticElement {
                doc: self.doc.clone(),
                id,
            })
            .collect())
    }
}

/// 在 `root` 子树内按 [`StaticQuery`] 收集匹配元素的 NodeId(文档顺序)。
fn collect_ids(root: ElementRef<'_>, q: &StaticQuery) -> Result<Vec<NodeId>> {
    match q {
        StaticQuery::Css(sel) => {
            let selector = Selector::parse(sel)
                .map_err(|e| Error::Other(format!("非法 CSS 选择器 {sel:?}: {e:?}")))?;
            Ok(root.select(&selector).map(|e| e.id()).collect())
        }
        StaticQuery::AttrEq { name, value } => {
            let uni = universal()?;
            Ok(root
                .select(&uni)
                .filter(|e| e.value().attr(name) == Some(value.as_str()))
                .map(|e| e.id())
                .collect())
        }
        StaticQuery::AttrPresent(name) => {
            let uni = universal()?;
            Ok(root
                .select(&uni)
                .filter(|e| e.value().attr(name).is_some())
                .map(|e| e.id())
                .collect())
        }
        StaticQuery::TextContains(t) => {
            // 按元素**自身直接文本**(直接子文本节点)匹配,对齐 DP `text:` 的 `contains(text(), …)`
            // 语义——只命中真正承载该文本的元素,而非其所有祖先(后者会让 body/div 等也命中)。
            let needle = normalize_space(t);
            let uni = universal()?;
            Ok(root
                .select(&uni)
                .filter(|e| {
                    let direct: String = e
                        .children()
                        .filter_map(|c| c.value().as_text().map(|t| t.text.as_ref()))
                        .collect();
                    normalize_space(&direct).contains(&needle)
                })
                .map(|e| e.id())
                .collect())
        }
        StaticQuery::Xpath(xp) => crate::browser::xpath::eval(*root, xp),
    }
}

/// 通配选择器 `*`(用于属性/文本过滤时遍历全部后代元素)。
fn universal() -> Result<Selector> {
    Selector::parse("*").map_err(|_| Error::Other("内部错误:通配选择器解析失败".into()))
}

/// `normalize-space` 语义:折叠连续空白为单个空格并去首尾。
fn normalize_space(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const HTML: &str = r#"<html><body>
        <div id="main" class="box wrap">
          <a href="/a" class="link">首页</a>
          <a href="/b" class="link">关于我们</a>
          <span data-x="1">hello world</span>
        </div>
      </body></html>"#;

    #[test]
    fn css_and_tag() {
        let root = StaticElement::parse(HTML).unwrap();
        assert_eq!(root.ele("#main").unwrap().attr("class").unwrap().as_deref(), Some("box wrap"));
        assert_eq!(root.eles("tag:a").unwrap().len(), 2);
        assert_eq!(root.ele("css:a.link").unwrap().attr("href").unwrap().as_deref(), Some("/a"));
    }

    #[test]
    fn attr_eq_and_present() {
        let root = StaticElement::parse(HTML).unwrap();
        assert_eq!(root.ele("@id:main").unwrap().tag().unwrap(), "div");
        assert_eq!(root.ele("@data-x:1").unwrap().text().unwrap(), "hello world");
        assert_eq!(root.eles("@href").unwrap().len(), 2);
    }

    #[test]
    fn text_contains() {
        let root = StaticElement::parse(HTML).unwrap();
        let e = root.ele("text:关于").unwrap();
        assert_eq!(e.tag().unwrap(), "a");
        assert_eq!(e.attr("href").unwrap().as_deref(), Some("/b"));
    }

    #[test]
    fn nested_and_xpath() {
        let root = StaticElement::parse(HTML).unwrap();
        let main = root.ele("#main").unwrap();
        assert_eq!(main.eles("tag:a").unwrap().len(), 2);
        // 显式 xpath: 现已支持(内置子集求值器)
        assert_eq!(root.eles("xpath://a").unwrap().len(), 2);
        assert_eq!(
            root.ele(r#"xpath://*[@id="main"]"#).unwrap().tag().unwrap(),
            "div"
        );
        // 不支持的轴仍报错
        assert!(root.ele("xpath://a/following-sibling::a").is_err());
    }
}
