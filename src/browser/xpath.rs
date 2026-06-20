//! 一个**面向静态 HTML 的 XPath 1.0 子集**求值器(纯 Rust,基于 `scraper`/`ego-tree` 的 DOM)。
//!
//! 用于 [`StaticElement`](crate::browser::StaticElement) 的 `xpath:` 查询(实时 `tab.ele` 走浏览器
//! 原生 `document.evaluate`,不经此模块)。覆盖 DP 生成的写法与绝大多数手写 XPath:
//!
//! - 路径:`//`(后代或自身)、`/`(子)、相对路径;节点测试 `*` 或标签名(大小写不敏感)。
//! - 谓词:`[n]`、`[last()]`、`[position()=n]`、`[@a]`、`[@a="v"]`/`[@a='v']`、`[@a!='v']`、
//!   `[contains(@a,"v")]`、`[contains(text(),"v")]`、`[contains(.,"v")]`、
//!   `[contains(normalize-space(.),"v")]`、`[text()="v"]`、`[.="v"]`、`[starts-with(@a,"v")]`、
//!   以及 `and`/`or`/`not(...)`/括号组合。
//! - 位置谓词按 **同一父节点下的同名兄弟** 计数(与 XPath `//li[1]`=每个列表首个 li 一致)。
//!
//! 不支持的轴(`..`/`following-sibling::` 等)或语法会返回 [`Error`],提示改用实时 `tab.ele`。

use std::collections::{HashMap, HashSet};

use ego_tree::{NodeId, NodeRef};
use scraper::Node;

use crate::{Error, Result};

/// 在 `root`(及其子树)上求值 XPath 子集,返回命中元素的 NodeId(文档顺序)。
pub(crate) fn eval(root: NodeRef<'_, Node>, expr: &str) -> Result<Vec<NodeId>> {
    let steps = parse_path(expr)?;
    let mut ctx: Vec<NodeRef<Node>> = vec![root];
    for step in &steps {
        ctx = apply_step(&ctx, step)?;
    }
    Ok(ctx.iter().map(|n| n.id()).collect())
}

// ---------------- 路径解析 ----------------

struct Step {
    descendant: bool,
    name: Option<String>, // None = '*'
    preds: Vec<Expr>,
}

impl Step {
    fn name_matches(&self, tag: &str) -> bool {
        match &self.name {
            None => true,
            Some(n) => n.eq_ignore_ascii_case(tag),
        }
    }
}

fn parse_path(s: &str) -> Result<Vec<Step>> {
    let chars: Vec<char> = s.trim().chars().collect();
    let n = chars.len();
    if n == 0 {
        return Err(Error::Other("空 XPath".into()));
    }
    let mut i = 0usize;
    // 首步轴:`//`/`/`/相对 一律按"后代或自身"处理(对静态查询更直觉)。
    let mut pending_desc = true;
    if chars[i] == '/' {
        i += 1;
        if i < n && chars[i] == '/' {
            i += 1;
        }
        pending_desc = true;
    }

    let mut steps = Vec::new();
    while i < n {
        // 读节点测试
        skip_ws(&chars, &mut i);
        let mut name = String::new();
        if i < n && chars[i] == '*' {
            name.push('*');
            i += 1;
        } else {
            while i < n && is_name_char(chars[i]) {
                name.push(chars[i]);
                i += 1;
            }
        }
        if name.is_empty() {
            return Err(Error::Other(format!(
                "XPath 解析失败:步缺少节点测试 @ {s:?}"
            )));
        }
        // 显式轴(`axis::node`)与父轴(`..`)暂不支持。
        if name.contains("::") || name == ".." {
            return Err(Error::Other(format!(
                "XPath:暂不支持的轴/步 {name:?},请改用实时 tab.ele()"
            )));
        }
        // 读谓词
        let mut preds = Vec::new();
        loop {
            skip_ws(&chars, &mut i);
            if i < n && chars[i] == '[' {
                let inner = read_bracket(&chars, &mut i, s)?;
                preds.push(parse_predicate(&inner)?);
            } else {
                break;
            }
        }
        steps.push(Step {
            descendant: pending_desc,
            name: if name == "*" { None } else { Some(name) },
            preds,
        });
        // 读分隔符决定下一步轴
        skip_ws(&chars, &mut i);
        if i >= n {
            break;
        }
        if chars[i] == '/' {
            i += 1;
            if i < n && chars[i] == '/' {
                pending_desc = true;
                i += 1;
            } else {
                pending_desc = false;
            }
        } else {
            return Err(Error::Other(format!(
                "XPath 解析失败:非法字符 {:?} @ {s:?}",
                chars[i]
            )));
        }
    }
    Ok(steps)
}

/// 读取一个平衡的 `[...]`(尊重引号),返回内部字符串,游标移到 `]` 之后。
fn read_bracket(chars: &[char], i: &mut usize, src: &str) -> Result<String> {
    // chars[*i] == '['
    *i += 1;
    let mut depth = 1;
    let mut out = String::new();
    let mut quote: Option<char> = None;
    while *i < chars.len() {
        let c = chars[*i];
        if let Some(q) = quote {
            out.push(c);
            if c == q {
                quote = None;
            }
            *i += 1;
            continue;
        }
        match c {
            '\'' | '"' => {
                quote = Some(c);
                out.push(c);
            }
            '[' => {
                depth += 1;
                out.push(c);
            }
            ']' => {
                depth -= 1;
                if depth == 0 {
                    *i += 1;
                    return Ok(out);
                }
                out.push(c);
            }
            _ => out.push(c),
        }
        *i += 1;
    }
    Err(Error::Other(format!("XPath 谓词括号不匹配 @ {src:?}")))
}

fn skip_ws(chars: &[char], i: &mut usize) {
    while *i < chars.len() && chars[*i].is_whitespace() {
        *i += 1;
    }
}

fn is_name_char(c: char) -> bool {
    c.is_alphanumeric() || c == '-' || c == '_' || c == ':' || c == '.'
}

// ---------------- 谓词:词法 ----------------

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    At,
    Ident(String),
    Str(String),
    Num(f64),
    Dot,
    LParen,
    RParen,
    Comma,
    Eq,
    Ne,
}

fn tokenize(s: &str) -> Result<Vec<Tok>> {
    let chars: Vec<char> = s.chars().collect();
    let n = chars.len();
    let mut i = 0;
    let mut toks = Vec::new();
    while i < n {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
        } else if c == '@' {
            toks.push(Tok::At);
            i += 1;
        } else if c == '(' {
            toks.push(Tok::LParen);
            i += 1;
        } else if c == ')' {
            toks.push(Tok::RParen);
            i += 1;
        } else if c == ',' {
            toks.push(Tok::Comma);
            i += 1;
        } else if c == '=' {
            toks.push(Tok::Eq);
            i += 1;
        } else if c == '!' {
            if i + 1 < n && chars[i + 1] == '=' {
                toks.push(Tok::Ne);
                i += 2;
            } else {
                return Err(Error::Other("XPath 谓词:孤立的 '!'".into()));
            }
        } else if c == '\'' || c == '"' {
            let mut s2 = String::new();
            i += 1;
            while i < n && chars[i] != c {
                s2.push(chars[i]);
                i += 1;
            }
            if i >= n {
                return Err(Error::Other("XPath 谓词:字符串未闭合".into()));
            }
            i += 1; // 跳过结束引号
            toks.push(Tok::Str(s2));
        } else if c.is_ascii_digit() || (c == '.' && i + 1 < n && chars[i + 1].is_ascii_digit()) {
            let mut num = String::new();
            while i < n && (chars[i].is_ascii_digit() || chars[i] == '.') {
                num.push(chars[i]);
                i += 1;
            }
            let v: f64 = num
                .parse()
                .map_err(|_| Error::Other(format!("XPath 谓词:非法数字 {num:?}")))?;
            toks.push(Tok::Num(v));
        } else if c == '.' {
            toks.push(Tok::Dot);
            i += 1;
        } else if c.is_alphabetic() || c == '_' {
            let mut id = String::new();
            while i < n && (chars[i].is_alphanumeric() || chars[i] == '-' || chars[i] == '_') {
                id.push(chars[i]);
                i += 1;
            }
            toks.push(Tok::Ident(id));
        } else {
            return Err(Error::Other(format!("XPath 谓词:非法字符 {c:?}")));
        }
    }
    Ok(toks)
}

// ---------------- 谓词:AST ----------------

#[derive(Debug, Clone)]
enum Operand {
    Attr(String),
    Text,
    Dot,
    NormSpaceDot,
    NormSpaceText,
    Str(String),
    Num(f64),
    Position,
    Last,
}

#[derive(Debug, Clone, Copy)]
enum CmpOp {
    Eq,
    Ne,
}

#[derive(Debug, Clone)]
enum Expr {
    Or(Box<Expr>, Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Not(Box<Expr>),
    Cmp(Operand, CmpOp, Operand),
    Contains(Operand, Operand),
    StartsWith(Operand, Operand),
    AttrExists(String),
    PosEq(usize),
    IsLast,
}

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
}

fn parse_predicate(s: &str) -> Result<Expr> {
    let toks = tokenize(s)?;
    let mut p = Parser { toks, pos: 0 };
    let e = p.parse_or()?;
    if p.pos != p.toks.len() {
        return Err(Error::Other(format!("XPath 谓词:多余 token @ {s:?}")));
    }
    Ok(e)
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }
    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).cloned();
        if t.is_some() {
            self.pos += 1;
        }
        t
    }
    fn expect(&mut self, t: &Tok) -> Result<()> {
        if self.peek() == Some(t) {
            self.pos += 1;
            Ok(())
        } else {
            Err(Error::Other(format!(
                "XPath 谓词:期望 {t:?},实际 {:?}",
                self.peek()
            )))
        }
    }
    fn is_kw(&self, kw: &str) -> bool {
        matches!(self.peek(), Some(Tok::Ident(s)) if s.eq_ignore_ascii_case(kw))
    }

    fn parse_or(&mut self) -> Result<Expr> {
        let mut left = self.parse_and()?;
        while self.is_kw("or") {
            self.pos += 1;
            let right = self.parse_and()?;
            left = Expr::Or(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<Expr> {
        let mut left = self.parse_unary()?;
        while self.is_kw("and") {
            self.pos += 1;
            let right = self.parse_unary()?;
            left = Expr::And(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<Expr> {
        if self.is_kw("not") {
            // not(...) 函数式
            self.pos += 1;
            self.expect(&Tok::LParen)?;
            let e = self.parse_or()?;
            self.expect(&Tok::RParen)?;
            return Ok(Expr::Not(Box::new(e)));
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Result<Expr> {
        if self.peek() == Some(&Tok::LParen) {
            self.pos += 1;
            let e = self.parse_or()?;
            self.expect(&Tok::RParen)?;
            return Ok(e);
        }
        // contains(...) / starts-with(...)
        if self.is_kw("contains") {
            self.pos += 1;
            self.expect(&Tok::LParen)?;
            let a = self.parse_operand()?;
            self.expect(&Tok::Comma)?;
            let b = self.parse_operand()?;
            self.expect(&Tok::RParen)?;
            return Ok(Expr::Contains(a, b));
        }
        if self.is_kw("starts-with") {
            self.pos += 1;
            self.expect(&Tok::LParen)?;
            let a = self.parse_operand()?;
            self.expect(&Tok::Comma)?;
            let b = self.parse_operand()?;
            self.expect(&Tok::RParen)?;
            return Ok(Expr::StartsWith(a, b));
        }

        // 普通操作数,后面可能跟 = / != 比较
        let op = self.parse_operand()?;
        match self.peek() {
            Some(Tok::Eq) => {
                self.pos += 1;
                let rhs = self.parse_operand()?;
                Ok(Expr::Cmp(op, CmpOp::Eq, rhs))
            }
            Some(Tok::Ne) => {
                self.pos += 1;
                let rhs = self.parse_operand()?;
                Ok(Expr::Cmp(op, CmpOp::Ne, rhs))
            }
            _ => {
                // 无比较:作为布尔基元
                match op {
                    Operand::Attr(n) => Ok(Expr::AttrExists(n)),
                    Operand::Num(v) => Ok(Expr::PosEq(v as usize)),
                    Operand::Last => Ok(Expr::IsLast),
                    Operand::Position => {
                        Err(Error::Other("XPath 谓词:position() 需与比较一起用".into()))
                    }
                    _ => Err(Error::Other("XPath 谓词:不支持的裸操作数".into())),
                }
            }
        }
    }

    fn parse_operand(&mut self) -> Result<Operand> {
        match self.next() {
            Some(Tok::At) => match self.next() {
                Some(Tok::Ident(name)) => Ok(Operand::Attr(name)),
                other => Err(Error::Other(format!(
                    "XPath 谓词:@ 后需属性名,实际 {other:?}"
                ))),
            },
            Some(Tok::Str(s)) => Ok(Operand::Str(s)),
            Some(Tok::Num(v)) => Ok(Operand::Num(v)),
            Some(Tok::Dot) => Ok(Operand::Dot),
            Some(Tok::Ident(id)) => {
                let low = id.to_ascii_lowercase();
                match low.as_str() {
                    "text" => {
                        self.expect(&Tok::LParen)?;
                        self.expect(&Tok::RParen)?;
                        Ok(Operand::Text)
                    }
                    "position" => {
                        self.expect(&Tok::LParen)?;
                        self.expect(&Tok::RParen)?;
                        Ok(Operand::Position)
                    }
                    "last" => {
                        self.expect(&Tok::LParen)?;
                        self.expect(&Tok::RParen)?;
                        Ok(Operand::Last)
                    }
                    "normalize-space" => {
                        self.expect(&Tok::LParen)?;
                        let inner = self.parse_operand()?;
                        self.expect(&Tok::RParen)?;
                        match inner {
                            Operand::Dot => Ok(Operand::NormSpaceDot),
                            Operand::Text => Ok(Operand::NormSpaceText),
                            _ => Err(Error::Other(
                                "XPath 谓词:normalize-space 仅支持 . 或 text()".into(),
                            )),
                        }
                    }
                    _ => Err(Error::Other(format!("XPath 谓词:未知函数/标识 {id:?}"))),
                }
            }
            other => Err(Error::Other(format!("XPath 谓词:非法操作数 {other:?}"))),
        }
    }
}

// ---------------- 求值 ----------------

fn apply_step<'a>(ctx: &[NodeRef<'a, Node>], step: &Step) -> Result<Vec<NodeRef<'a, Node>>> {
    let mut cands: Vec<NodeRef<'a, Node>> = Vec::new();
    let mut seen: HashSet<NodeId> = HashSet::new();
    for &c in ctx {
        if step.descendant {
            for nd in c.descendants() {
                push_if_match(nd, step, &mut cands, &mut seen);
            }
        } else {
            for nd in c.children() {
                push_if_match(nd, step, &mut cands, &mut seen);
            }
        }
    }
    for pred in &step.preds {
        cands = apply_predicate(cands, pred);
    }
    Ok(cands)
}

fn push_if_match<'a>(
    nd: NodeRef<'a, Node>,
    step: &Step,
    cands: &mut Vec<NodeRef<'a, Node>>,
    seen: &mut HashSet<NodeId>,
) {
    if let Some(el) = nd.value().as_element()
        && step.name_matches(el.name())
        && seen.insert(nd.id())
    {
        cands.push(nd);
    }
}

fn apply_predicate<'a>(cands: Vec<NodeRef<'a, Node>>, pred: &Expr) -> Vec<NodeRef<'a, Node>> {
    // 位置谓词按"同父分组"计数。
    let mut group_size: HashMap<Option<NodeId>, usize> = HashMap::new();
    for nd in &cands {
        *group_size.entry(nd.parent().map(|p| p.id())).or_insert(0) += 1;
    }
    let mut group_idx: HashMap<Option<NodeId>, usize> = HashMap::new();
    let mut out = Vec::new();
    for nd in cands {
        let pkey = nd.parent().map(|p| p.id());
        let idx = group_idx.entry(pkey).or_insert(0);
        *idx += 1;
        let pos = *idx;
        let size = *group_size.get(&pkey).unwrap_or(&0);
        if eval_expr(pred, nd, pos, size) {
            out.push(nd);
        }
    }
    out
}

enum OpVal {
    Str(String),
    Num(f64),
}

impl OpVal {
    fn to_text(&self) -> String {
        match self {
            OpVal::Str(s) => s.clone(),
            OpVal::Num(n) => {
                if n.fract() == 0.0 {
                    format!("{}", *n as i64)
                } else {
                    format!("{n}")
                }
            }
        }
    }
}

fn eval_expr(e: &Expr, nd: NodeRef<'_, Node>, pos: usize, size: usize) -> bool {
    match e {
        Expr::Or(a, b) => eval_expr(a, nd, pos, size) || eval_expr(b, nd, pos, size),
        Expr::And(a, b) => eval_expr(a, nd, pos, size) && eval_expr(b, nd, pos, size),
        Expr::Not(a) => !eval_expr(a, nd, pos, size),
        Expr::AttrExists(name) => attr(nd, name).is_some(),
        Expr::PosEq(n) => pos == *n,
        Expr::IsLast => pos == size,
        Expr::Contains(a, b) => operand(a, nd, pos, size)
            .to_text()
            .contains(&operand(b, nd, pos, size).to_text()),
        Expr::StartsWith(a, b) => operand(a, nd, pos, size)
            .to_text()
            .starts_with(&operand(b, nd, pos, size).to_text()),
        Expr::Cmp(a, op, b) => {
            let l = operand(a, nd, pos, size);
            let r = operand(b, nd, pos, size);
            let eq = match (&l, &r) {
                (OpVal::Num(x), OpVal::Num(y)) => x == y,
                _ => l.to_text() == r.to_text(),
            };
            match op {
                CmpOp::Eq => eq,
                CmpOp::Ne => !eq,
            }
        }
    }
}

fn operand(o: &Operand, nd: NodeRef<'_, Node>, pos: usize, size: usize) -> OpVal {
    match o {
        Operand::Attr(name) => OpVal::Str(attr(nd, name).unwrap_or_default()),
        Operand::Text => OpVal::Str(direct_text(nd)),
        Operand::Dot => OpVal::Str(string_value(nd)),
        Operand::NormSpaceDot => OpVal::Str(normalize_space(&string_value(nd))),
        Operand::NormSpaceText => OpVal::Str(normalize_space(&direct_text(nd))),
        Operand::Str(s) => OpVal::Str(s.clone()),
        Operand::Num(v) => OpVal::Num(*v),
        Operand::Position => OpVal::Num(pos as f64),
        Operand::Last => OpVal::Num(size as f64),
    }
}

fn attr(nd: NodeRef<'_, Node>, name: &str) -> Option<String> {
    nd.value()
        .as_element()
        .and_then(|e| e.attr(name))
        .map(|s| s.to_string())
}

/// 元素的直接子文本节点拼接。
fn direct_text(nd: NodeRef<'_, Node>) -> String {
    nd.children()
        .filter_map(|c| c.value().as_text().map(|t| t.text.to_string()))
        .collect()
}

/// 元素的字符串值(所有后代文本拼接)。
fn string_value(nd: NodeRef<'_, Node>) -> String {
    nd.descendants()
        .filter_map(|c| c.value().as_text().map(|t| t.text.to_string()))
        .collect()
}

fn normalize_space(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use scraper::Html;

    fn ids(html: &str, xp: &str) -> Vec<String> {
        let doc = Html::parse_document(html);
        let got = eval(*doc.root_element(), xp).unwrap();
        got.into_iter()
            .filter_map(|id| doc.tree.get(id))
            .filter_map(|nd| {
                let el = nd.value().as_element()?;
                Some(format!("{}#{}", el.name(), el.attr("id").unwrap_or("")))
            })
            .collect()
    }

    const H: &str = r#"<html><body>
      <div id="a" class="box"><a href="/x">link1</a><a href="/y">关于</a></div>
      <ul id="u1"><li>l1</li><li>l2</li></ul>
      <ul id="u2"><li>m1</li><li>m2</li></ul>
      <input id="i1" type="text"><input id="i2" type="file">
    </body></html>"#;

    #[test]
    fn attr_eq_and_star() {
        assert_eq!(ids(H, r#"//*[@id="a"]"#), vec!["div#a"]);
        assert_eq!(ids(H, r#"//div[@class="box"]"#), vec!["div#a"]);
        assert_eq!(ids(H, r#"//input[@type="file"]"#), vec!["input#i2"]);
    }

    #[test]
    fn descendant_and_child() {
        assert_eq!(ids(H, "//ul/li").len(), 4);
        assert_eq!(ids(H, "//div/a").len(), 2);
    }

    #[test]
    fn positional_per_parent() {
        // 每个 ul 的首个 li → 两个
        assert_eq!(ids(H, "//ul/li[1]").len(), 2);
        assert_eq!(ids(H, "//ul/li[last()]").len(), 2);
    }

    #[test]
    fn contains_and_text() {
        assert_eq!(ids(H, r#"//a[contains(@href,"x")]"#), vec!["a#"]);
        assert_eq!(ids(H, r#"//*[contains(text(),"关于")]"#), vec!["a#"]);
        assert_eq!(
            ids(H, r#"//a[contains(normalize-space(.),"关于")]"#),
            vec!["a#"]
        );
    }

    #[test]
    fn and_or_not() {
        assert_eq!(ids(H, r#"//input[@type="text" or @type="file"]"#).len(), 2);
        assert_eq!(
            ids(H, r#"//input[@type="file" and @id="i2"]"#),
            vec!["input#i2"]
        );
        assert_eq!(ids(H, r#"//input[not(@type="file")]"#), vec!["input#i1"]);
    }

    #[test]
    fn unsupported_axis_errors() {
        let doc = Html::parse_document(H);
        assert!(eval(*doc.root_element(), "//a/following-sibling::a").is_err());
    }
}
