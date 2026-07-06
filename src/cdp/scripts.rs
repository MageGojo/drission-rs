//! CDP **脚本源码** [`ChromiumScripts`](`tab.scripts()`):dump 全部 JS、全文搜索、美化。
//!
//! 逆向常见痛点:站点把签名逻辑藏在十几个混淆 chunk 里。本模块用 `Debugger` 域把**当前页解析过的
//! 全部脚本**(含动态 `eval`/内联)dump 落盘、`searchInContent` 直接告诉你「`x-ca-sign`」在哪个脚本
//! 第几行,并内置**纯 Rust 美化器**(字符串/模板串/正则/注释感知)让混淆代码可读。
//!
//! ```no_run
//! use drission::prelude::*;
//! # async fn f(tab: ChromiumTab) -> drission::Result<()> {
//! let sc = tab.scripts();
//! // 全文搜索签名字样,直达脚本与行:
//! for m in sc.grep("x-ca-sign").await? {
//!     println!("{} :{}  {}", m.url, m.line_number, m.snippet);
//! }
//! // dump 全部 JS(自动美化)到目录:
//! let files = sc.dump_all("out/scripts").await?;
//! println!("dumped {} 个脚本", files.len());
//! # Ok(()) }
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::broadcast::error::RecvError;
use tokio::time::{Instant, timeout};

use super::core::CdpCore;
use crate::Result;

/// 一个被解析的脚本(`Debugger.scriptParsed`)。
#[derive(Debug, Clone)]
pub struct ScriptInfo {
    /// 脚本 id(供 `source`/`searchInContent`)。
    pub script_id: String,
    /// 脚本 URL(动态 `eval`/匿名内联可能为空)。
    pub url: String,
    /// 源码字符长度(scriptParsed 报告值)。
    pub length: u32,
    /// 是否 WebAssembly(`scriptLanguage == "WebAssembly"`)。
    pub is_wasm: bool,
    /// sourceMap URL(有则便于还原原始源码)。
    pub source_map_url: String,
}

/// 一条搜索命中。
#[derive(Debug, Clone)]
pub struct ScriptMatch {
    /// 所在脚本 URL。
    pub url: String,
    /// 脚本 id。
    pub script_id: String,
    /// 行号(0 基;压缩成单行的 JS 恒为 0)。
    pub line_number: u32,
    /// 命中行(可能极长);`snippet` 是其在命中处 ±60 字符的截断,便于阅读。
    pub line_content: String,
    /// 命中处上下文片段(压缩代码也能看清)。
    pub snippet: String,
}

/// 脚本源码句柄(`tab.scripts()`)。
pub struct ChromiumScripts {
    core: Arc<CdpCore>,
}

impl ChromiumScripts {
    pub(crate) fn new(core: Arc<CdpCore>) -> Self {
        Self { core }
    }

    /// 列出当前页解析过的全部脚本。开 `Debugger.enable` 触发 `scriptParsed` 回灌(含已解析脚本),
    /// 收集到「静默窗口」(默认 ~400ms 无新脚本)或达标签超时为止。
    ///
    /// **鲁棒性**:若 `Debugger` 此前已被开过(重复 `enable` **不再重发** `scriptParsed`),首轮会收到 0 个 →
    /// 自动 `disable` 再 `enable` **强制重灌**一次(代价:清掉已设断点,仅在 0 命中的回退路径触发)。
    pub async fn list(&self) -> Result<Vec<ScriptInfo>> {
        let first = self.collect_scripts().await?;
        if !first.is_empty() {
            return Ok(first);
        }
        // 0 个 → 多半 Debugger 已开过(enable 不重发);disable 后再收一次强制回灌。
        let _ = self.core.send("Debugger.disable", json!({})).await;
        self.collect_scripts().await
    }

    /// 一轮收集:订阅 → `Debugger.enable`(触发 scriptParsed 回灌)→ `setSkipAllPauses(true)`(防反调试卡收集)
    /// → 收到静默窗口或超时。
    async fn collect_scripts(&self) -> Result<Vec<ScriptInfo>> {
        let mut events = self.core.conn.subscribe(); // 先订阅,避免漏掉 enable 后的回灌
        self.core.send("Debugger.enable", json!({})).await?;
        // 反调试站:开 Debugger 会触发「无限 debugger」把收集卡死;setSkipAllPauses(true) 让收集不被暂停打断。
        let _ = self
            .core
            .send("Debugger.setSkipAllPauses", json!({ "skip": true }))
            .await;
        let sid = self.core.session_id.clone();
        let quiet = Duration::from_millis(400);
        let hard_deadline = Instant::now() + self.core.timeout();

        let mut out: Vec<ScriptInfo> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        loop {
            let remain = hard_deadline.saturating_duration_since(Instant::now());
            if remain.is_zero() {
                break;
            }
            let wait_for = remain.min(quiet);
            match timeout(wait_for, events.recv()).await {
                Ok(Ok(ev)) => {
                    if ev.method != "Debugger.scriptParsed"
                        || ev.session_id.as_deref() != Some(sid.as_str())
                    {
                        continue;
                    }
                    let info = parse_script_parsed(&ev.params);
                    if !info.script_id.is_empty() && seen.insert(info.script_id.clone()) {
                        out.push(info);
                    }
                }
                Ok(Err(RecvError::Lagged(_))) => continue,
                Ok(Err(RecvError::Closed)) => break,
                Err(_) => break, // 静默窗口到 → 认为脚本已收齐
            }
        }
        Ok(out)
    }

    /// 取某脚本源码(`Debugger.getScriptSource`)。wasm 脚本走 `bytecode`,本方法只返回文本源码
    /// (wasm 请用 [`dump_wasm`](Self::dump_wasm))。
    pub async fn source(&self, script_id: &str) -> Result<String> {
        let r = self
            .core
            .send("Debugger.getScriptSource", json!({ "scriptId": script_id }))
            .await?;
        Ok(r["scriptSource"].as_str().unwrap_or_default().to_string())
    }

    /// 全文搜索:在所有脚本里找 `needle`(子串,大小写不敏感),返回命中(脚本 + 行号 + 片段)。
    /// 走 CDP 原生 `Debugger.searchInContent`(服务端搜索,快)。
    pub async fn grep(&self, needle: &str) -> Result<Vec<ScriptMatch>> {
        self.grep_with(needle, false, false).await
    }

    /// 同 [`grep`](Self::grep),可指定大小写敏感 / 正则。
    pub async fn grep_with(
        &self,
        query: &str,
        case_sensitive: bool,
        is_regex: bool,
    ) -> Result<Vec<ScriptMatch>> {
        let scripts = self.list().await?;
        let mut out = Vec::new();
        for s in &scripts {
            if s.is_wasm {
                continue;
            }
            let r = self
                .core
                .send(
                    "Debugger.searchInContent",
                    json!({
                        "scriptId": s.script_id,
                        "query": query,
                        "caseSensitive": case_sensitive,
                        "isRegex": is_regex,
                    }),
                )
                .await;
            let Ok(r) = r else { continue };
            if let Some(list) = r["result"].as_array() {
                for m in list {
                    let line_content = m["lineContent"].as_str().unwrap_or_default().to_string();
                    let snippet = snippet_around(&line_content, query, case_sensitive);
                    out.push(ScriptMatch {
                        url: s.url.clone(),
                        script_id: s.script_id.clone(),
                        line_number: m["lineNumber"].as_u64().unwrap_or(0) as u32,
                        line_content,
                        snippet,
                    });
                }
            }
        }
        Ok(out)
    }

    /// 仅列出 WebAssembly 模块(便捷过滤)。
    pub async fn list_wasm(&self) -> Result<Vec<ScriptInfo>> {
        Ok(self
            .list()
            .await?
            .into_iter()
            .filter(|s| s.is_wasm)
            .collect())
    }

    /// 取某 **wasm** 脚本的原始字节(`Debugger.getScriptSource` 的 `bytecode` base64 解码)。
    /// 越来越多签名走 wasm,拿到字节后用 `wasm2wat`/wabt 反汇编成 `.wat` 阅读。
    pub async fn wasm_bytes(&self, script_id: &str) -> Result<Vec<u8>> {
        let r = self
            .core
            .send("Debugger.getScriptSource", json!({ "scriptId": script_id }))
            .await?;
        let b64 = r["bytecode"].as_str().unwrap_or_default();
        Ok(crate::util::base64_decode(b64).unwrap_or_default())
    }

    /// dump 全部 **WebAssembly** 模块字节到 `dir`(`.wasm`,按 URL 末段命名、去重),返回写出路径。
    /// 反汇编:`wasm2wat out/m.wasm -o m.wat`(wabt 工具集;全 wat 反汇编不进库,避免重依赖)。
    pub async fn dump_wasm(&self, dir: impl AsRef<Path>) -> Result<Vec<PathBuf>> {
        let dir = dir.as_ref();
        tokio::fs::create_dir_all(dir).await?;
        let scripts = self.list().await?;
        let mut written = Vec::new();
        let mut used: HashMap<String, u32> = HashMap::new();
        for s in &scripts {
            if !s.is_wasm {
                continue;
            }
            let bytes = match self.wasm_bytes(&s.script_id).await {
                Ok(b) if !b.is_empty() => b,
                _ => continue,
            };
            let fname = unique_filename(&s.url, &s.script_id, "wasm", &mut used);
            let path = dir.join(fname);
            tokio::fs::write(&path, &bytes).await?;
            written.push(path);
        }
        Ok(written)
    }

    /// dump 全部 JS 脚本到 `dir`(自动美化、按 URL 末段命名、去重),返回写出的文件路径。
    /// wasm 脚本跳过(请用 [`dump_wasm`](Self::dump_wasm))。
    pub async fn dump_all(&self, dir: impl AsRef<Path>) -> Result<Vec<PathBuf>> {
        self.dump_all_with(dir, true).await
    }

    /// 同 [`dump_all`](Self::dump_all),`beautify=false` 则保存原始(可能压缩)源码。
    pub async fn dump_all_with(
        &self,
        dir: impl AsRef<Path>,
        beautify: bool,
    ) -> Result<Vec<PathBuf>> {
        let dir = dir.as_ref();
        tokio::fs::create_dir_all(dir).await?;
        let scripts = self.list().await?;
        let mut written = Vec::new();
        let mut used: HashMap<String, u32> = HashMap::new();
        for s in &scripts {
            if s.is_wasm {
                continue;
            }
            let src = match self.source(&s.script_id).await {
                Ok(s) => s,
                Err(_) => continue,
            };
            if src.is_empty() {
                continue;
            }
            let content = if beautify { beautify_js(&src) } else { src };
            let fname = unique_filename(&s.url, &s.script_id, "js", &mut used);
            let path = dir.join(fname);
            tokio::fs::write(&path, content.as_bytes()).await?;
            written.push(path);
        }
        Ok(written)
    }
}

// ════════════════════════════════════════════════════════════════════════════
// 纯函数:scriptParsed 解析 / 文件名 / 片段 / 美化(均可单测,不触网)
// ════════════════════════════════════════════════════════════════════════════

/// 解析 `Debugger.scriptParsed` 事件参数。
fn parse_script_parsed(p: &Value) -> ScriptInfo {
    ScriptInfo {
        script_id: p["scriptId"].as_str().unwrap_or_default().to_string(),
        url: p["url"].as_str().unwrap_or_default().to_string(),
        length: p["length"].as_u64().unwrap_or(0) as u32,
        is_wasm: p["scriptLanguage"].as_str() == Some("WebAssembly"),
        source_map_url: p["sourceMapURL"].as_str().unwrap_or_default().to_string(),
    }
}

/// 命中处 ±`PAD` 字符的片段(压缩成单行的代码也能看清上下文)。
fn snippet_around(line: &str, needle: &str, case_sensitive: bool) -> String {
    const PAD: usize = 60;
    if needle.is_empty() {
        return line.chars().take(PAD * 2).collect();
    }
    let (hay, ndl) = if case_sensitive {
        (line.to_string(), needle.to_string())
    } else {
        (line.to_lowercase(), needle.to_lowercase())
    };
    let Some(byte_pos) = hay.find(&ndl) else {
        return line.chars().take(PAD * 2).collect();
    };
    // byte_pos 是小写串里的字节位;转成字符索引(小写化可能改变字节长度但字符数一致,ASCII 场景一致)。
    let char_pos = hay[..byte_pos].chars().count();
    let start = char_pos.saturating_sub(PAD);
    let chars: Vec<char> = line.chars().collect();
    let end = (char_pos + needle.chars().count() + PAD).min(chars.len());
    let mut s = String::new();
    if start > 0 {
        s.push('…');
    }
    s.extend(&chars[start..end]);
    if end < chars.len() {
        s.push('…');
    }
    s
}

/// 据 URL 末段 + 脚本 id 生成唯一文件名(无 URL → `inline_<id>`;冲突追加 `_n`)。
fn unique_filename(
    url: &str,
    script_id: &str,
    ext: &str,
    used: &mut HashMap<String, u32>,
) -> String {
    let base = filename_base(url, script_id);
    let dot_ext = format!(".{ext}");
    let stem = base.strip_suffix(&dot_ext).unwrap_or(&base).to_string();
    let mut name = format!("{stem}.{ext}");
    let count = used.entry(name.clone()).or_insert(0);
    if *count > 0 {
        let n = *count;
        name = format!("{stem}_{n}.{ext}");
    }
    *count += 1;
    name
}

/// URL → 安全文件名基:取 path 末段(去 query/hash),非法字符替换为 `_`;空则 `inline_<id>`。
fn filename_base(url: &str, script_id: &str) -> String {
    let sanitized_id: String = script_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    if url.is_empty() {
        return format!("inline_{sanitized_id}");
    }
    // 剥离 scheme+host,只取真正的 path(避免把 host 当文件名);无 scheme 则整串当 path。
    let path = if let Some((_, rest)) = url.split_once("://") {
        rest.split_once('/').map(|(_, p)| p).unwrap_or("")
    } else {
        url
    };
    let path = path.split(['?', '#']).next().unwrap_or("");
    let seg = path.trim_end_matches('/').rsplit('/').next().unwrap_or("");
    let safe: String = seg
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let safe = safe.trim_matches('_').to_string();
    if safe.is_empty() {
        format!("inline_{sanitized_id}")
    } else {
        safe
    }
}

/// **JS 美化器**(纯 Rust,零依赖):在 `{` `}` `;` 处换行 + 两空格缩进,**字符串/模板串/正则/注释
/// 感知**(其内部的 `{};` 绝不当结构符)。best-effort:目标是把压缩成一行的混淆代码变得**可读可搜**,
/// 不追求 100% 还原格式(不动逗号/运算符换行,避免破坏语义)。
pub fn beautify_js(src: &str) -> String {
    let b = src.as_bytes();
    let n = b.len();
    let mut out: Vec<u8> = Vec::with_capacity(n * 2);
    let mut indent: usize = 0;
    let mut last_sig: u8 = 0; // 上一个有意义(非空白/注释)字节,用于判定 `/` 是正则还是除号
    let mut i = 0;

    while i < n {
        let c = b[i];
        match c {
            b'"' | b'\'' => {
                let q = c;
                out.push(c);
                i += 1;
                while i < n {
                    let d = b[i];
                    out.push(d);
                    i += 1;
                    if d == b'\\' {
                        if i < n {
                            out.push(b[i]);
                            i += 1;
                        }
                        continue;
                    }
                    if d == q {
                        break;
                    }
                }
                last_sig = q;
            }
            b'`' => {
                out.push(b'`');
                i += 1;
                while i < n {
                    let d = b[i];
                    out.push(d);
                    i += 1;
                    if d == b'\\' {
                        if i < n {
                            out.push(b[i]);
                            i += 1;
                        }
                        continue;
                    }
                    if d == b'`' {
                        break;
                    }
                }
                last_sig = b'`';
            }
            b'/' if i + 1 < n && b[i + 1] == b'/' => {
                while i < n && b[i] != b'\n' {
                    out.push(b[i]);
                    i += 1;
                }
            }
            b'/' if i + 1 < n && b[i + 1] == b'*' => {
                out.push(b'/');
                out.push(b'*');
                i += 2;
                while i < n {
                    if b[i] == b'*' && i + 1 < n && b[i + 1] == b'/' {
                        out.push(b'*');
                        out.push(b'/');
                        i += 2;
                        break;
                    }
                    out.push(b[i]);
                    i += 1;
                }
            }
            b'/' if is_regex_pos(last_sig) => {
                out.push(b'/');
                i += 1;
                let mut in_class = false;
                while i < n {
                    let d = b[i];
                    out.push(d);
                    i += 1;
                    if d == b'\\' {
                        if i < n {
                            out.push(b[i]);
                            i += 1;
                        }
                        continue;
                    }
                    match d {
                        b'[' => in_class = true,
                        b']' => in_class = false,
                        b'/' if !in_class => break,
                        _ => {}
                    }
                }
                while i < n && b[i].is_ascii_alphabetic() {
                    out.push(b[i]);
                    i += 1;
                }
                last_sig = b'/';
            }
            b'{' => {
                out.push(b'{');
                indent += 1;
                newline_indent(&mut out, indent);
                last_sig = b'{';
                i += 1;
                while i < n && (b[i] == b' ' || b[i] == b'\t') {
                    i += 1;
                }
            }
            b'}' => {
                trim_trailing_ws(&mut out);
                indent = indent.saturating_sub(1);
                newline_indent(&mut out, indent);
                out.push(b'}');
                last_sig = b'}';
                i += 1;
            }
            b';' => {
                out.push(b';');
                newline_indent(&mut out, indent);
                last_sig = b';';
                i += 1;
                while i < n && (b[i] == b' ' || b[i] == b'\t') {
                    i += 1;
                }
            }
            b'\n' | b'\r' => {
                i += 1; // 原换行丢弃,由本美化器统一管理
            }
            b' ' | b'\t' => {
                if !matches!(out.last(), Some(b' ') | Some(b'\n') | None) {
                    out.push(b' ');
                }
                i += 1;
            }
            _ => {
                out.push(c);
                if !c.is_ascii_whitespace() {
                    last_sig = c;
                }
                i += 1;
            }
        }
    }

    String::from_utf8_lossy(&out).into_owned()
}

/// `/` 处是否「期望表达式」(从而是正则字面量而非除号)。单字符启发式:够覆盖压缩代码常见写法。
fn is_regex_pos(last_sig: u8) -> bool {
    matches!(
        last_sig,
        0 | b'('
            | b','
            | b'='
            | b':'
            | b'['
            | b'!'
            | b'&'
            | b'|'
            | b'?'
            | b'{'
            | b'}'
            | b';'
            | b'~'
            | b'+'
            | b'-'
            | b'*'
            | b'%'
            | b'<'
            | b'>'
            | b'^'
            | b'\n'
    )
}

/// 追加换行 + `indent` 级(每级两空格)。
fn newline_indent(out: &mut Vec<u8>, indent: usize) {
    out.push(b'\n');
    for _ in 0..indent {
        out.push(b' ');
        out.push(b' ');
    }
}

/// 去掉尾部的空格与换行(为 `}` 回退到上一行尾)。
fn trim_trailing_ws(out: &mut Vec<u8>) {
    while matches!(
        out.last(),
        Some(b' ') | Some(b'\n') | Some(b'\t') | Some(b'\r')
    ) {
        out.pop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_script_parsed_basic_and_wasm() {
        let js = json!({ "scriptId": "7", "url": "https://x/app.js", "length": 1234 });
        let s = parse_script_parsed(&js);
        assert_eq!(s.script_id, "7");
        assert_eq!(s.url, "https://x/app.js");
        assert_eq!(s.length, 1234);
        assert!(!s.is_wasm);

        let w =
            json!({ "scriptId": "8", "url": "https://x/m.wasm", "scriptLanguage": "WebAssembly" });
        assert!(parse_script_parsed(&w).is_wasm);
    }

    #[test]
    fn filename_from_url_and_inline() {
        assert_eq!(
            filename_base("https://x.com/a/b/app.min.js?v=1", "3"),
            "app.min.js"
        );
        assert_eq!(filename_base("https://x.com/", "3"), "inline_3");
        assert_eq!(filename_base("", "12"), "inline_12");
        // 非法字符替换。
        assert_eq!(filename_base("https://x.com/a b!c.js", "3"), "a_b_c.js");
    }

    #[test]
    fn unique_filename_dedupes() {
        let mut used = HashMap::new();
        assert_eq!(
            unique_filename("https://x/app.js", "1", "js", &mut used),
            "app.js"
        );
        assert_eq!(
            unique_filename("https://x/app.js", "2", "js", &mut used),
            "app_1.js"
        );
        assert_eq!(
            unique_filename("https://x/app.js", "3", "js", &mut used),
            "app_2.js"
        );
    }

    #[test]
    fn unique_filename_wasm_ext() {
        let mut used = HashMap::new();
        // 已带 .wasm 后缀不重复追加。
        assert_eq!(
            unique_filename("https://x/m.wasm", "1", "wasm", &mut used),
            "m.wasm"
        );
        // 无后缀补全 + 去重。
        assert_eq!(
            unique_filename("https://x/mod", "2", "wasm", &mut used),
            "mod.wasm"
        );
        assert_eq!(
            unique_filename("https://x/mod", "3", "wasm", &mut used),
            "mod_1.wasm"
        );
    }

    #[test]
    fn snippet_truncates_around_match() {
        let long = format!("{}x-ca-sign{}", "a".repeat(200), "b".repeat(200));
        let s = snippet_around(&long, "x-ca-sign", false);
        assert!(s.contains("x-ca-sign"));
        assert!(s.starts_with('…') && s.ends_with('…'));
        assert!(s.chars().count() < long.chars().count());
    }

    #[test]
    fn beautify_braces_and_semicolons() {
        let out = beautify_js("function f(){var a=1;if(a){b()}}");
        // 每个 { 后换行、; 后换行、} 单独成行。
        assert!(out.contains("{\n"));
        assert!(out.contains(";\n"));
        let lines: Vec<&str> = out.lines().collect();
        assert!(lines.len() >= 4, "应被拆成多行: {out:?}");
    }

    #[test]
    fn beautify_keeps_braces_inside_strings() {
        // 字符串里的 } ; { 不能被当结构符换行。
        let out = beautify_js(r#"var s="a{b};c";f();"#);
        assert!(
            out.contains(r#""a{b};c""#),
            "字符串字面量必须原样保留: {out:?}"
        );
    }

    #[test]
    fn beautify_keeps_template_and_regex() {
        // 模板串与正则里的特殊符不破坏。
        let out = beautify_js("var r=/a{2}\\/b/g;var t=`x${y};z`;");
        assert!(out.contains("/a{2}\\/b/g"), "正则应原样: {out:?}");
        assert!(out.contains("`x${y};z`"), "模板串应原样: {out:?}");
    }

    #[test]
    fn beautify_handles_division_not_regex() {
        // a/b/c 是除法,不应被吞成正则(标识符后的 / 是除号)。
        let out = beautify_js("var x=a/b/c;");
        assert!(out.contains("a/b/c"), "除法应原样: {out:?}");
    }

    #[test]
    fn beautify_is_utf8_safe() {
        // 字符串里的中文不被破坏。
        let out = beautify_js(r#"var s="签名密钥";f();"#);
        assert!(out.contains("签名密钥"));
    }
}
