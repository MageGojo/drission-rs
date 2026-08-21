//! HTML → Markdown(后端无关),供 AI / MCP 读页。
//!
//! 用 [`htmd`](https://crates.io/crates/htmd)(turndown.js 风格,仅依赖 html5ever)。
//! 比整页 HTML 更适合喂给 LLM:体积小、标题/链接/列表结构保留、默认跳过 script/style。

use std::sync::OnceLock;

use htmd::HtmlToMarkdown;
use serde_json::{Value, json};

/// 默认 Markdown 截断长度(Unicode scalars)。
pub const DEFAULT_MAX_MARKDOWN_CHARS: usize = 50_000;

fn converter() -> &'static HtmlToMarkdown {
    static CONV: OnceLock<HtmlToMarkdown> = OnceLock::new();
    CONV.get_or_init(|| {
        HtmlToMarkdown::builder()
            .skip_tags(vec![
                "script", "style", "noscript", "svg", "canvas", "iframe", "template",
            ])
            .build()
    })
}

/// 把 HTML 转成 Markdown。失败时返回可读错误字符串(不 panic)。
pub fn html_to_markdown(html: &str) -> Result<String, String> {
    if html.trim().is_empty() {
        return Ok(String::new());
    }
    converter()
        .convert(html)
        .map(|s| collapse_blank_lines(&s))
        .map_err(|e| e.to_string())
}

/// 转换并截断;返回 `(markdown, truncated)`。
pub fn html_to_markdown_truncated(html: &str, max_chars: usize) -> (String, bool) {
    match html_to_markdown(html) {
        Ok(mut md) => {
            let truncated = truncate_chars(&mut md, max_chars);
            (md, truncated)
        }
        Err(err) => (format!("<!-- html_to_markdown failed: {err} -->"), false),
    }
}

/// 组装进 CLI/MCP `data` 的字段。
pub fn markdown_fields(html: &str, max_chars: Option<usize>) -> Value {
    let limit = max_chars.unwrap_or(DEFAULT_MAX_MARKDOWN_CHARS);
    let (markdown, truncated) = html_to_markdown_truncated(html, limit);
    json!({
        "markdown": markdown,
        "markdownTruncated": truncated,
        "markdownMaxChars": limit,
    })
}

fn truncate_chars(text: &mut String, max_chars: usize) -> bool {
    if text.chars().count() <= max_chars {
        return false;
    }
    *text = text.chars().take(max_chars).collect();
    true
}

fn collapse_blank_lines(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut blank = 0u8;
    for line in s.lines() {
        if line.trim().is_empty() {
            blank = blank.saturating_add(1);
            if blank <= 2 {
                out.push('\n');
            }
        } else {
            blank = 0;
            out.push_str(line.trim_end());
            out.push('\n');
        }
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_heading_and_link() {
        let md =
            html_to_markdown("<h1>Hello</h1><p>Go <a href=\"https://example.com\">there</a>.</p>")
                .unwrap();
        assert!(md.contains("# Hello"), "{md}");
        assert!(md.contains("[there](https://example.com)"), "{md}");
    }

    #[test]
    fn skips_script_and_style() {
        let md =
            html_to_markdown("<p>ok</p><script>alert(1)</script><style>body{}</style><p>end</p>")
                .unwrap();
        assert!(!md.contains("alert"));
        assert!(!md.contains("body{}"));
        assert!(md.contains("ok"));
        assert!(md.contains("end"));
    }

    #[test]
    fn truncates_long_markdown() {
        let html = format!("<p>{}</p>", "字".repeat(100));
        let (md, truncated) = html_to_markdown_truncated(&html, 10);
        assert!(truncated);
        assert_eq!(md.chars().count(), 10);
    }
}
