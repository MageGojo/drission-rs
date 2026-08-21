//! DrissionPage 风格的元素定位语法解析。
//!
//! 把 DP 那一套简洁的字符串选择器解析成最终用于浏览器查询的策略
//! ([`Query`]:CSS 选择器或 XPath)。支持的前缀:
//!
//! | DP 写法 | 含义 | 结果 |
//! |---|---|---|
//! | `#id` | id(CSS 简写) | CSS |
//! | `.class` | class(CSS 简写) | CSS |
//! | `css:` / `c:` | CSS 选择器 | CSS |
//! | `xpath:` / `x:` | XPath | XPath |
//! | `tag:name` / `t:name` | 标签名 | CSS |
//! | `@attr:val` / `@attr=val` | 按属性 | XPath |
//! | `@text():val` | 按文本(属性式写法) | XPath |
//! | `text:val` | 文本包含 | XPath |
//! | 其它无前缀 | 默认按文本包含 | XPath |
//!
//! 例:
//! ```
//! use drission::locator::{parse, Query};
//! assert!(matches!(parse("#kw"), Query::Css(_)));
//! assert!(matches!(parse("@id:kw"), Query::Xpath(_)));
//! assert!(matches!(parse("登录"), Query::Xpath(_)));
//! ```

/// 解析后的查询策略。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Query {
    /// CSS 选择器(交给 `querySelector`/`querySelectorAll`)。
    Css(String),
    /// XPath 表达式(交给 `document.evaluate`)。
    Xpath(String),
}

impl Query {
    /// 返回底层查询字符串。
    pub fn as_str(&self) -> &str {
        match self {
            Query::Css(s) | Query::Xpath(s) => s,
        }
    }

    /// 是否为 XPath。
    pub fn is_xpath(&self) -> bool {
        matches!(self, Query::Xpath(_))
    }
}

/// 把一个 XPath 字符串值安全地转成 XPath 字面量,正确处理其中的单/双引号。
///
/// - 不含双引号:用双引号包裹。
/// - 含双引号但不含单引号:用单引号包裹。
/// - 同时含两种引号:用 `concat(...)` 拼接。
pub fn xpath_literal(s: &str) -> String {
    if !s.contains('"') {
        format!("\"{s}\"")
    } else if !s.contains('\'') {
        format!("'{s}'")
    } else {
        let parts: Vec<String> = s.split('"').map(|p| format!("\"{p}\"")).collect();
        format!("concat({})", parts.join(", '\"', "))
    }
}

/// 一条前缀规则:`(前缀, 把余下部分构造成 [`Query`] 的函数)`。
type PrefixRule = (&'static str, fn(&str) -> Query);

/// 解析 DP 风格选择器为 [`Query`]。
pub fn parse(selector: &str) -> Query {
    let sel = selector.trim();

    // 显式前缀(大小写不敏感)→ 构造策略 的规则表,自上而下命中即返回。
    // `tag:`/`t:` 额外 trim 余下部分(容忍 `tag: div` 这种写法)。
    const PREFIX_RULES: &[PrefixRule] = &[
        ("xpath:", |r| Query::Xpath(r.to_string())),
        ("x:", |r| Query::Xpath(r.to_string())),
        ("css:", |r| Query::Css(r.to_string())),
        ("c:", |r| Query::Css(r.to_string())),
        ("tag:", |r| Query::Css(r.trim().to_string())),
        ("t:", |r| Query::Css(r.trim().to_string())),
        ("text:", text_contains_xpath),
        ("role:", |r| Locator::role(r.trim()).to_query()),
        ("label:", |r| Locator::label(r.trim()).to_query()),
        ("placeholder:", |r| {
            Locator::placeholder(r.trim()).to_query()
        }),
    ];
    for &(prefix, make) in PREFIX_RULES {
        if let Some(rest) = strip_prefix_ci(sel, prefix) {
            return make(rest);
        }
    }

    // 属性式:@attr:val 或 @attr=val,以及特例 @text()。
    if let Some(rest) = sel.strip_prefix('@') {
        return parse_attribute(rest);
    }

    // CSS 简写:#id / .class。
    if sel.starts_with('#') || sel.starts_with('.') {
        return Query::Css(sel.to_string());
    }

    // 默认:按文本包含查找。
    text_contains_xpath(sel)
}

/// 解析 `@` 之后的属性表达式。支持 `attr:val`、`attr=val`、`text()`、`tag()`。
fn parse_attribute(rest: &str) -> Query {
    // 分隔符可能是第一个 ':' 或第一个 '='。
    let (name, value) = match rest.find([':', '=']) {
        Some(i) => (&rest[..i], &rest[i + 1..]),
        // 只有属性名,没有值 → 匹配“存在该属性”。
        None => (rest, ""),
    };

    let name = name.trim();

    // @text():xxx → 文本包含
    if name.eq_ignore_ascii_case("text()") || name.eq_ignore_ascii_case("text") {
        return text_contains_xpath(value);
    }

    if value.is_empty() {
        return Query::Xpath(format!("//*[@{name}]"));
    }
    Query::Xpath(format!("//*[@{}={}]", name, xpath_literal(value)))
}

/// 生成“文本包含”的 XPath。使用 `normalize-space(.)` 以匹配跨子节点的可见文本。
fn text_contains_xpath(text: &str) -> Query {
    let t = text.trim();
    Query::Xpath(format!(
        "//*[contains(normalize-space(.), {})]",
        xpath_literal(t)
    ))
}

/// 面向**静态 HTML**(离线解析,无 XPath 引擎)的查询策略。
///
/// 与 [`Query`] 的区别:静态解析端没有浏览器的 `document.evaluate`,故把 DP 常见写法
/// (`@attr`、`text:`)直接表达为可在已解析 DOM 上匹配的结构;只有**显式 `xpath:`**
/// 无法在静态端支持(用 [`StaticQuery::Xpath`] 标记,由调用方拒绝并建议改用实时 `ele`)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StaticQuery {
    /// CSS 选择器(`querySelectorAll` 等价)。
    Css(String),
    /// 属性等值:等价 `[name="value"]`(精确匹配,与 XPath `@name="value"` 一致)。
    AttrEq { name: String, value: String },
    /// 属性存在:等价 `[name]`。
    AttrPresent(String),
    /// 文本包含(对元素 `normalize-space` 后 `contains`)。
    TextContains(String),
    /// 原始 XPath:静态解析不支持,调用方应报错。
    Xpath(String),
}

/// 解析 DP 风格选择器为 [`StaticQuery`],用于离线静态 HTML 查询(`tab.s_ele`/`StaticElement`)。
///
/// 前缀与 [`parse`] 完全一致,只是结果落到无需 XPath 引擎的策略上。
pub fn parse_static(selector: &str) -> StaticQuery {
    let sel = selector.trim();

    // 与 [`parse`] 同构的前缀规则表(结果落到无 XPath 引擎的 [`StaticQuery`])。
    type StaticPrefixRule = (&'static str, fn(&str) -> StaticQuery);
    const PREFIX_RULES: &[StaticPrefixRule] = &[
        ("xpath:", |r| StaticQuery::Xpath(r.to_string())),
        ("x:", |r| StaticQuery::Xpath(r.to_string())),
        ("css:", |r| StaticQuery::Css(r.to_string())),
        ("c:", |r| StaticQuery::Css(r.to_string())),
        ("tag:", |r| StaticQuery::Css(r.trim().to_string())),
        ("t:", |r| StaticQuery::Css(r.trim().to_string())),
        ("text:", |r| StaticQuery::TextContains(r.trim().to_string())),
    ];
    for &(prefix, make) in PREFIX_RULES {
        if let Some(rest) = strip_prefix_ci(sel, prefix) {
            return make(rest);
        }
    }

    if let Some(rest) = sel.strip_prefix('@') {
        return parse_attribute_static(rest);
    }
    if sel.starts_with('#') || sel.starts_with('.') {
        return StaticQuery::Css(sel.to_string());
    }
    StaticQuery::TextContains(sel.to_string())
}

/// 解析 `@` 之后的属性表达式为 [`StaticQuery`](与 [`parse_attribute`] 语义一致)。
fn parse_attribute_static(rest: &str) -> StaticQuery {
    let (name, value) = match rest.find([':', '=']) {
        Some(i) => (&rest[..i], &rest[i + 1..]),
        None => (rest, ""),
    };
    let name = name.trim();

    if name.eq_ignore_ascii_case("text()") || name.eq_ignore_ascii_case("text") {
        return StaticQuery::TextContains(value.trim().to_string());
    }
    if value.is_empty() {
        return StaticQuery::AttrPresent(name.to_string());
    }
    StaticQuery::AttrEq {
        name: name.to_string(),
        value: value.to_string(),
    }
}

/// 统一多策略定位。最终都变成 [`Query`],给 `tab.ele` / Agent resolver 用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Locator {
    inner: LocatorKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum LocatorKind {
    Query(Query),
    Role { role: String, name: Option<String> },
    Label(String),
    Placeholder(String),
}

impl Locator {
    pub fn css(sel: impl Into<String>) -> Self {
        Self {
            inner: LocatorKind::Query(Query::Css(sel.into())),
        }
    }
    pub fn xpath(sel: impl Into<String>) -> Self {
        Self {
            inner: LocatorKind::Query(Query::Xpath(sel.into())),
        }
    }
    pub fn id(id: impl Into<String>) -> Self {
        Self::css(format!("#{}", id.into()))
    }
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            inner: LocatorKind::Query(text_contains_xpath(&text.into())),
        }
    }
    pub fn role(role: impl Into<String>) -> Self {
        Self {
            inner: LocatorKind::Role {
                role: role.into(),
                name: None,
            },
        }
    }
    pub fn name(mut self, name: impl Into<String>) -> Self {
        if let LocatorKind::Role { name: slot, .. } = &mut self.inner {
            *slot = Some(name.into());
        }
        self
    }
    pub fn label(text: impl Into<String>) -> Self {
        Self {
            inner: LocatorKind::Label(text.into()),
        }
    }
    pub fn placeholder(text: impl Into<String>) -> Self {
        Self {
            inner: LocatorKind::Placeholder(text.into()),
        }
    }
    /// DP 字符串或中文语义(「登录按钮」→ role=button + name=登录)。
    pub fn parse(raw: impl AsRef<str>) -> Self {
        let raw = raw.as_ref().trim();
        if let Some(loc) = semantic_zh(raw) {
            return loc;
        }
        Self {
            inner: LocatorKind::Query(parse(raw)),
        }
    }

    pub fn role_name(&self) -> Option<(&str, Option<&str>)> {
        match &self.inner {
            LocatorKind::Role { role, name } => Some((role.as_str(), name.as_deref())),
            _ => None,
        }
    }

    /// 给 resolver / OCR 用的可见名。CSS 选择器没有名字。
    pub fn hint_text(&self) -> Option<String> {
        match &self.inner {
            LocatorKind::Role { name, .. } => name.clone(),
            LocatorKind::Label(t) | LocatorKind::Placeholder(t) => Some(t.clone()),
            LocatorKind::Query(Query::Xpath(s)) => extract_contained_literal(s),
            LocatorKind::Query(Query::Css(_)) => None,
        }
    }

    pub fn to_query(&self) -> Query {
        match &self.inner {
            LocatorKind::Query(q) => q.clone(),
            LocatorKind::Role { role, name } => role_query(role, name.as_deref()),
            LocatorKind::Label(t) => Query::Xpath(format!(
                "//*[@aria-label={0}] | //label[contains(normalize-space(.), {0})]",
                xpath_literal(t)
            )),
            LocatorKind::Placeholder(t) => {
                Query::Xpath(format!("//*[@placeholder={}]", xpath_literal(t)))
            }
        }
    }

    /// 可直接交给 `tab.ele` 的 DP 选择器。
    pub fn as_selector(&self) -> String {
        match self.to_query() {
            Query::Css(s) => {
                if s.starts_with('#') || s.starts_with('.') || s.starts_with("css:") {
                    s
                } else {
                    format!("css:{s}")
                }
            }
            Query::Xpath(s) => {
                if s.starts_with("xpath:") || s.starts_with("x:") {
                    s
                } else {
                    format!("xpath:{s}")
                }
            }
        }
    }
}

impl From<&str> for Locator {
    fn from(s: &str) -> Self {
        Self::parse(s)
    }
}

impl From<String> for Locator {
    fn from(s: String) -> Self {
        Self::parse(s)
    }
}

fn role_query(role: &str, name: Option<&str>) -> Query {
    let tag = match role {
        "button" => "button",
        "link" => "a",
        "textbox" | "searchbox" => "input",
        "heading" => "h1 | //h2 | //h3 | //h4",
        "checkbox" => "input[@type='checkbox']",
        _ => "",
    };
    let named = |expr: String| match name {
        Some(n) => format!("{expr}[contains(normalize-space(.), {})]", xpath_literal(n)),
        None => expr,
    };
    let mut parts = vec![named(format!("//*[@role={}]", xpath_literal(role)))];
    if !tag.is_empty() && !tag.contains('|') {
        parts.push(named(format!("//{tag}")));
    }
    Query::Xpath(parts.join(" | "))
}

fn extract_contained_literal(xpath: &str) -> Option<String> {
    let key = "normalize-space(.), ";
    let i = xpath.find(key)?;
    let rest = xpath.get(i + key.len()..)?;
    if let Some(body) = rest.strip_prefix('"') {
        let end = body.find('"')?;
        let s = &body[..end];
        return (!s.is_empty()).then(|| s.to_string());
    }
    None
}

fn semantic_zh(raw: &str) -> Option<Locator> {
    const PAIRS: &[(&str, &str)] = &[
        ("按钮", "button"),
        ("链接", "link"),
        ("输入框", "textbox"),
        ("文本框", "textbox"),
        ("复选框", "checkbox"),
        ("标题", "heading"),
    ];
    for (suffix, role) in PAIRS {
        if let Some(name) = raw.strip_suffix(suffix) {
            let name = name.trim();
            if !name.is_empty() {
                return Some(Locator::role(*role).name(name));
            }
            return Some(Locator::role(*role));
        }
    }
    None
}

/// 大小写不敏感地剥离前缀;成功则返回去掉前缀并 `trim_start` 后的剩余部分。
///
/// 用 `str::get` 取头部,避免在多字节 UTF-8(如中文)上按字节切片导致 panic。
fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let head = s.get(..prefix.len())?;
    if head.eq_ignore_ascii_case(prefix) {
        Some(s[prefix.len()..].trim_start())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn css_shorthands() {
        assert_eq!(parse("#kw"), Query::Css("#kw".into()));
        assert_eq!(parse(".title.foo"), Query::Css(".title.foo".into()));
    }

    #[test]
    fn explicit_css_and_xpath() {
        assert_eq!(parse("css:div.box"), Query::Css("div.box".into()));
        assert_eq!(parse("c:div.box"), Query::Css("div.box".into()));
        assert_eq!(
            parse("xpath://div[@id='a']"),
            Query::Xpath("//div[@id='a']".into())
        );
        assert_eq!(parse("x://a"), Query::Xpath("//a".into()));
    }

    #[test]
    fn tag_prefix() {
        assert_eq!(parse("tag:li"), Query::Css("li".into()));
        assert_eq!(parse("t:h3"), Query::Css("h3".into()));
    }

    #[test]
    fn attribute_colon_and_eq() {
        assert_eq!(parse("@id:kw"), Query::Xpath(r#"//*[@id="kw"]"#.into()));
        assert_eq!(parse("@id=kw"), Query::Xpath(r#"//*[@id="kw"]"#.into()));
        assert_eq!(
            parse("@class=project list"),
            Query::Xpath(r#"//*[@class="project list"]"#.into())
        );
    }

    #[test]
    fn attribute_presence_only() {
        assert_eq!(parse("@disabled"), Query::Xpath("//*[@disabled]".into()));
    }

    #[test]
    fn attribute_text() {
        assert_eq!(
            parse("@text():登录"),
            Query::Xpath(r#"//*[contains(normalize-space(.), "登录")]"#.into())
        );
    }

    #[test]
    fn text_prefix_and_default() {
        assert_eq!(
            parse("text:提交"),
            Query::Xpath(r#"//*[contains(normalize-space(.), "提交")]"#.into())
        );
        assert_eq!(
            parse("提交"),
            Query::Xpath(r#"//*[contains(normalize-space(.), "提交")]"#.into())
        );
    }

    #[test]
    fn xpath_literal_quotes() {
        assert_eq!(xpath_literal("abc"), r#""abc""#);
        assert_eq!(xpath_literal(r#"say "hi""#), r#"'say "hi"'"#);
        // 同时含单双引号 → concat 拼接
        assert_eq!(xpath_literal("a\"b'c"), r#"concat("a", '"', "b'c")"#);
    }

    #[test]
    fn static_query_mapping() {
        assert_eq!(parse_static("#kw"), StaticQuery::Css("#kw".into()));
        assert_eq!(parse_static(".a.b"), StaticQuery::Css(".a.b".into()));
        assert_eq!(
            parse_static("css:div.box"),
            StaticQuery::Css("div.box".into())
        );
        assert_eq!(parse_static("tag:li"), StaticQuery::Css("li".into()));
        assert_eq!(
            parse_static("@id:kw"),
            StaticQuery::AttrEq {
                name: "id".into(),
                value: "kw".into()
            }
        );
        assert_eq!(
            parse_static("@disabled"),
            StaticQuery::AttrPresent("disabled".into())
        );
        assert_eq!(
            parse_static("text:登录"),
            StaticQuery::TextContains("登录".into())
        );
        assert_eq!(
            parse_static("提交"),
            StaticQuery::TextContains("提交".into())
        );
        assert_eq!(
            parse_static("@text():你好"),
            StaticQuery::TextContains("你好".into())
        );
        assert_eq!(parse_static("xpath://a"), StaticQuery::Xpath("//a".into()));
    }

    #[test]
    fn locator_role_and_semantic() {
        let loc = Locator::role("button").name("登录");
        let q = loc.to_query();
        assert!(q.as_str().contains("@role"));
        assert!(q.as_str().contains("登录"));
        let zh = Locator::parse("登录按钮");
        assert_eq!(zh.role_name(), Some(("button", Some("登录"))));
        assert_eq!(Locator::id("kw").as_selector(), "#kw");
        assert_eq!(Locator::text("提交").hint_text().as_deref(), Some("提交"));
        assert_eq!(
            Locator::parse("登录按钮").hint_text().as_deref(),
            Some("登录")
        );
    }
}
