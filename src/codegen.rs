//! **录制 → 生成代码**(Recorder / Codegen)的**后端无关**核心。
//!
//! 把一次页面操作录制成一串 [`RecordedAction`](见 [`RecordedScript`]),再生成**可运行的 Rust 代码**
//! (DrissionPage 风格选择器,复制即跑)或 JSON 中间表示。对标 Playwright `codegen`。
//!
//! - 录制的**采集**(页面事件钩子 + 选择器计算)在各后端(当前 CDP:[`crate::cdp::ChromiumRecorder`])。
//! - 本模块只放**值类型与代码生成**(纯函数,始终编译、可单测、可跨后端复用)。
//!
//! 覆盖动作:导航 / 点击 / 输入 / 勾选 / 下拉 / 按键 / **悬停** / **拖拽(元素→元素)** / **新标签**;
//! 元素动作可带 **iframe 框选择器**(`frame`),`Navigate` 之外的动作在新标签打开后自动切到新标签变量。
//!
//! ```
//! use drission::codegen::{RecordedAction, RecordedScript};
//! let mut s = RecordedScript::new();
//! s.push(RecordedAction::Navigate { url: "https://example.com".into() });
//! s.push(RecordedAction::Fill { selector: "@name:q".into(), text: "rust".into(), frame: None });
//! s.push(RecordedAction::Click { selector: "#go".into(), frame: None });
//! let code = s.to_rust();
//! assert!(code.contains("tab.get(\"https://example.com\").await?;"));
//! assert!(code.contains("tab.input(\"@name:q\", \"rust\").await?;"));
//! ```

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 录制到的一个用户动作。覆盖能一一映射到本库 API、且稳定的动作;元素动作可带 `frame`
/// (所在 iframe 的选择器,`None` = 顶层文档)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RecordedAction {
    /// 主框架导航到某 URL → `tab.get(url)`。
    Navigate {
        /// 目标地址。
        url: String,
    },
    /// 点击(按钮 / 链接 / 可点击元素)→ `tab.click(selector)`。
    Click {
        /// DP 风格选择器。
        selector: String,
        /// 所在 iframe 选择器(`None` = 顶层)。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        frame: Option<String>,
    },
    /// 文本输入(提交 / 失焦时的最终值)→ `tab.input(selector, text)`。
    Fill {
        /// DP 风格选择器。
        selector: String,
        /// 输入的最终文本。
        text: String,
        /// 所在 iframe 选择器。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        frame: Option<String>,
    },
    /// 勾选 / 取消复选框、单选 → `ele.set_checked(checked)`。
    Check {
        /// DP 风格选择器。
        selector: String,
        /// 是否选中。
        checked: bool,
        /// 所在 iframe 选择器。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        frame: Option<String>,
    },
    /// 下拉选择 `<select>` → `ele.select_value(value)`。
    Select {
        /// DP 风格选择器。
        selector: String,
        /// 选中项的 value。
        value: String,
        /// 所在 iframe 选择器。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        frame: Option<String>,
    },
    /// 在输入框上按下特殊键(如回车)→ `ele.input_keys(&[KeyInput::key(..)])`。
    Press {
        /// DP 风格选择器。
        selector: String,
        /// DOM 键名(如 `Enter`)。
        key: String,
        /// 所在 iframe 选择器。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        frame: Option<String>,
    },
    /// 悬停(鼠标移入,常用于触发菜单 / 提示)→ `ele.hover()`。
    Hover {
        /// DP 风格选择器。
        selector: String,
        /// 所在 iframe 选择器。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        frame: Option<String>,
    },
    /// 拖拽(元素 → 元素,HTML5 DnD 或按住移动)→ 动作链 `move_to_ele(from).hold().move_to_ele(to).release()`。
    Drag {
        /// 源元素选择器。
        from: String,
        /// 目标元素选择器。
        to: String,
        /// 两元素所在 iframe 选择器(同一帧)。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        frame: Option<String>,
    },
    /// 打开了新标签 / 弹窗 → 后续动作切到新标签(`tab.wait().new_tab()`)。
    NewTab,
}

impl RecordedAction {
    /// 从录制脚本回传的事件 JSON 解析为动作。无法生成代码(如选择器为空)时返回 `None`。
    ///
    /// 字段约定:`{type, selector?, from?, to?, url?, value?, checked?, key?, frame?}`。
    pub fn from_event(ev: &Value) -> Option<Self> {
        let ty = ev.get("type")?.as_str()?;
        let s = |k: &str| ev.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
        let frame = {
            let f = s("frame");
            if f.is_empty() { None } else { Some(f) }
        };
        let sel = s("selector");
        let need = |v: String| if v.is_empty() { None } else { Some(v) };
        match ty {
            "navigate" => need(s("url")).map(|url| RecordedAction::Navigate { url }),
            "click" => need(sel).map(|selector| RecordedAction::Click { selector, frame }),
            "fill" => need(sel).map(|selector| RecordedAction::Fill {
                selector,
                text: s("value"),
                frame,
            }),
            "check" => need(sel).map(|selector| RecordedAction::Check {
                selector,
                checked: ev.get("checked").and_then(|v| v.as_bool()).unwrap_or(true),
                frame,
            }),
            "select" => need(sel).map(|selector| RecordedAction::Select {
                selector,
                value: s("value"),
                frame,
            }),
            "press" => need(sel).map(|selector| RecordedAction::Press {
                selector,
                key: {
                    let k = s("key");
                    if k.is_empty() { "Enter".into() } else { k }
                },
                frame,
            }),
            "hover" => need(sel).map(|selector| RecordedAction::Hover { selector, frame }),
            "drag" => {
                let (from, to) = (s("from"), s("to"));
                if from.is_empty() || to.is_empty() {
                    None
                } else {
                    Some(RecordedAction::Drag { from, to, frame })
                }
            }
            "newtab" => Some(RecordedAction::NewTab),
            _ => None,
        }
    }

    /// 生成该动作的一行 Rust 语句(`tabvar` = 当前标签变量名)。`NewTab` 由 [`RecordedScript`]
    /// 统一处理(需分配新变量名),此处返回提示注释。
    pub fn to_rust_line(&self, tabvar: &str) -> String {
        match self {
            RecordedAction::Navigate { url } => format!("{tabvar}.get({}).await?;", lit(url)),
            RecordedAction::Click { selector, frame } => match frame {
                None => format!("{tabvar}.click({}).await?;", lit(selector)),
                Some(_) => format!("{}.click().await?;", ele_expr(tabvar, selector, frame)),
            },
            RecordedAction::Fill { selector, text, frame } => match frame {
                None => format!("{tabvar}.input({}, {}).await?;", lit(selector), lit(text)),
                Some(_) => {
                    format!("{}.input({}).await?;", ele_expr(tabvar, selector, frame), lit(text))
                }
            },
            RecordedAction::Check { selector, checked, frame } => {
                format!("{}.set_checked({}).await?;", ele_expr(tabvar, selector, frame), checked)
            }
            RecordedAction::Select { selector, value, frame } => format!(
                "{}.select_value({}).await?;",
                ele_expr(tabvar, selector, frame),
                lit(value)
            ),
            RecordedAction::Press { selector, key, frame } => format!(
                "{}.input_keys(&[KeyInput::key({})]).await?;",
                ele_expr(tabvar, selector, frame),
                key_expr(key)
            ),
            RecordedAction::Hover { selector, frame } => {
                format!("{}.hover().await?;", ele_expr(tabvar, selector, frame))
            }
            RecordedAction::Drag { from, to, frame } => format!(
                "{{ let _from = {}; let _to = {}; \
                 {tabvar}.actions().move_to_ele(&_from, 0.2).hold()\
                 .move_to_ele(&_to, 0.4).release().perform().await?; }}",
                ele_expr(tabvar, from, frame),
                ele_expr(tabvar, to, frame)
            ),
            RecordedAction::NewTab => "// (新标签:由 RecordedScript 统一生成)".to_string(),
        }
    }
}

/// 一段录制脚本:有序的动作列表。`to_rust()` 生成可运行 Rust、`to_json()` 出 JSON 中间表示。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordedScript {
    /// 录制到的动作(按发生顺序)。
    pub actions: Vec<RecordedAction>,
}

impl RecordedScript {
    /// 空脚本。
    pub fn new() -> Self {
        Self::default()
    }

    /// 已录动作数。
    pub fn len(&self) -> usize {
        self.actions.len()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.actions.is_empty()
    }

    /// 追加一个动作,**就地去噪**:
    /// - 连续对同一选择器(同帧)的 `Fill` 只保留最后一次(逐次输入收敛成最终值)。
    /// - 连续导航到同一 URL 去重。
    /// - 连续重复的 `Hover` 同一元素去重。
    pub fn push(&mut self, action: RecordedAction) {
        if let Some(last) = self.actions.last_mut() {
            match (&last, &action) {
                (
                    RecordedAction::Fill { selector: a, frame: fa, .. },
                    RecordedAction::Fill { selector: b, frame: fb, .. },
                ) if a == b && fa == fb => {
                    *last = action;
                    return;
                }
                (RecordedAction::Navigate { url: a }, RecordedAction::Navigate { url: b })
                    if a == b =>
                {
                    return;
                }
                (
                    RecordedAction::Hover { selector: a, frame: fa },
                    RecordedAction::Hover { selector: b, frame: fb },
                ) if a == b && fa == fb => {
                    return;
                }
                _ => {}
            }
        }
        self.actions.push(action);
    }

    /// 仅生成**动作语句**(每行一条,带 4 空格缩进),用于嵌进已有代码。多标签自动切换标签变量。
    pub fn to_rust_body(&self) -> String {
        let mut lines: Vec<String> = Vec::new();
        let mut tab_idx = 1usize;
        let curvar = |i: usize| if i == 1 { "tab".to_string() } else { format!("tab_{i}") };
        for a in &self.actions {
            match a {
                RecordedAction::NewTab => {
                    let prev = curvar(tab_idx);
                    tab_idx += 1;
                    let new = curvar(tab_idx);
                    lines.push(format!(
                        "    let {new} = {prev}.wait().new_tab(None).await?\
                         .expect(\"未捕获到新标签(如失败请把打开动作与 new_tab 用 tokio::join! 并发)\");"
                    ));
                }
                other => lines.push(format!("    {}", other.to_rust_line(&curvar(tab_idx)))),
            }
        }
        lines.join("\n")
    }

    /// 生成**完整可运行**的 Rust 程序(`use drission::prelude::*;` + `#[tokio::main]` + 启动/动作/退出)。
    pub fn to_rust(&self) -> String {
        let body = if self.actions.is_empty() {
            "    // (未录制到任何动作)".to_string()
        } else {
            self.to_rust_body()
        };
        format!(
            "// 由 drission 录制生成(drission codegen)。\n\
             use drission::prelude::*;\n\
             \n\
             #[tokio::main]\n\
             async fn main() -> drission::Result<()> {{\n\
             \x20   let browser = Browser::launch(BrowserOptions::new().headless(false)).await?;\n\
             \x20   let tab = browser.new_tab(None).await?;\n\
             {body}\n\
             \x20   browser.quit().await?;\n\
             \x20   Ok(())\n\
             }}\n"
        )
    }

    /// 序列化为美化 JSON(动作中间表示,可二次加工 / 跨语言 codegen)。
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(&self.actions).unwrap_or_else(|_| "[]".into())
    }
}

/// 元素获取表达式:带 frame 走 `get_frame(..).ele(..)`,否则 `tab.ele(..)`(均以 `.await?` 结尾)。
fn ele_expr(tabvar: &str, selector: &str, frame: &Option<String>) -> String {
    match frame {
        Some(f) => format!(
            "{tabvar}.get_frame({}).await?.ele({}).await?",
            lit(f),
            lit(selector)
        ),
        None => format!("{tabvar}.ele({}).await?", lit(selector)),
    }
}

/// 把字符串转成 Rust 字符串字面量(用 `serde_json` 转义:正确处理 `"`/`\`/控制字符,保留 UTF-8)。
fn lit(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

/// 把 DOM 键名映射到 `Keys::*` 常量表达式;未知键回退为字符串字面量(`KeyInput::key("X")` 仍合法)。
fn key_expr(key: &str) -> String {
    let konst = match key {
        "Enter" => "ENTER",
        "Tab" => "TAB",
        "Escape" | "Esc" => "ESCAPE",
        "Backspace" => "BACKSPACE",
        "Delete" | "Del" => "DELETE",
        "ArrowUp" => "ARROW_UP",
        "ArrowDown" => "ARROW_DOWN",
        "ArrowLeft" => "ARROW_LEFT",
        "ArrowRight" => "ARROW_RIGHT",
        _ => return lit(key),
    };
    format!("Keys::{konst}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn each_action_to_rust_line() {
        assert_eq!(
            RecordedAction::Navigate { url: "https://x.test/a".into() }.to_rust_line("tab"),
            "tab.get(\"https://x.test/a\").await?;"
        );
        assert_eq!(
            RecordedAction::Click { selector: "#go".into(), frame: None }.to_rust_line("tab"),
            "tab.click(\"#go\").await?;"
        );
        assert_eq!(
            RecordedAction::Fill { selector: "@name:q".into(), text: "rust".into(), frame: None }
                .to_rust_line("tab"),
            "tab.input(\"@name:q\", \"rust\").await?;"
        );
        assert_eq!(
            RecordedAction::Check { selector: "#agree".into(), checked: true, frame: None }
                .to_rust_line("tab"),
            "tab.ele(\"#agree\").await?.set_checked(true).await?;"
        );
        assert_eq!(
            RecordedAction::Select { selector: "#lang".into(), value: "rs".into(), frame: None }
                .to_rust_line("tab"),
            "tab.ele(\"#lang\").await?.select_value(\"rs\").await?;"
        );
        assert_eq!(
            RecordedAction::Press { selector: "@name:q".into(), key: "Enter".into(), frame: None }
                .to_rust_line("tab"),
            "tab.ele(\"@name:q\").await?.input_keys(&[KeyInput::key(Keys::ENTER)]).await?;"
        );
        assert_eq!(
            RecordedAction::Hover { selector: "#menu".into(), frame: None }.to_rust_line("tab"),
            "tab.ele(\"#menu\").await?.hover().await?;"
        );
    }

    #[test]
    fn drag_emits_actions_chain() {
        let line = RecordedAction::Drag {
            from: "#a".into(),
            to: "#b".into(),
            frame: None,
        }
        .to_rust_line("tab");
        assert!(line.contains("let _from = tab.ele(\"#a\").await?;"));
        assert!(line.contains("let _to = tab.ele(\"#b\").await?;"));
        assert!(line.contains("move_to_ele(&_from, 0.2).hold()"));
        assert!(line.contains("move_to_ele(&_to, 0.4).release().perform().await?;"));
    }

    #[test]
    fn frame_qualified_actions() {
        assert_eq!(
            RecordedAction::Click { selector: "#in".into(), frame: Some("#ifr".into()) }
                .to_rust_line("tab"),
            "tab.get_frame(\"#ifr\").await?.ele(\"#in\").await?.click().await?;"
        );
        assert_eq!(
            RecordedAction::Fill {
                selector: "#in".into(),
                text: "x".into(),
                frame: Some("#ifr".into())
            }
            .to_rust_line("tab"),
            "tab.get_frame(\"#ifr\").await?.ele(\"#in\").await?.input(\"x\").await?;"
        );
    }

    #[test]
    fn unknown_key_falls_back_to_literal() {
        assert_eq!(
            RecordedAction::Press { selector: "#x".into(), key: "F5".into(), frame: None }
                .to_rust_line("tab"),
            "tab.ele(\"#x\").await?.input_keys(&[KeyInput::key(\"F5\")]).await?;"
        );
    }

    #[test]
    fn literal_escapes_quotes_and_keeps_utf8() {
        assert_eq!(lit(r#"a"b"#), "\"a\\\"b\"");
        assert_eq!(lit("登录"), "\"登录\"");
    }

    #[test]
    fn from_event_parses_each_type() {
        assert_eq!(
            RecordedAction::from_event(&json!({"type":"navigate","url":"https://x/"})),
            Some(RecordedAction::Navigate { url: "https://x/".into() })
        );
        assert_eq!(
            RecordedAction::from_event(&json!({"type":"fill","selector":"#q","value":"hi","frame":"#f"})),
            Some(RecordedAction::Fill { selector: "#q".into(), text: "hi".into(), frame: Some("#f".into()) })
        );
        assert_eq!(
            RecordedAction::from_event(&json!({"type":"hover","selector":"#m"})),
            Some(RecordedAction::Hover { selector: "#m".into(), frame: None })
        );
        assert_eq!(
            RecordedAction::from_event(&json!({"type":"drag","from":"#a","to":"#b"})),
            Some(RecordedAction::Drag { from: "#a".into(), to: "#b".into(), frame: None })
        );
        assert_eq!(
            RecordedAction::from_event(&json!({"type":"newtab"})),
            Some(RecordedAction::NewTab)
        );
        // drag 缺端点 → None。
        assert_eq!(RecordedAction::from_event(&json!({"type":"drag","from":"#a"})), None);
        assert_eq!(RecordedAction::from_event(&json!({"type":"scroll"})), None);
    }

    #[test]
    fn push_collapses_fill_navigate_hover() {
        let mut s = RecordedScript::new();
        s.push(RecordedAction::Navigate { url: "https://x/".into() });
        s.push(RecordedAction::Navigate { url: "https://x/".into() });
        s.push(RecordedAction::Fill { selector: "#q".into(), text: "r".into(), frame: None });
        s.push(RecordedAction::Fill { selector: "#q".into(), text: "rust".into(), frame: None });
        s.push(RecordedAction::Hover { selector: "#m".into(), frame: None });
        s.push(RecordedAction::Hover { selector: "#m".into(), frame: None });
        assert_eq!(s.len(), 3);
        assert_eq!(
            s.actions[1],
            RecordedAction::Fill { selector: "#q".into(), text: "rust".into(), frame: None }
        );
    }

    #[test]
    fn full_program_has_scaffold_and_body() {
        let mut s = RecordedScript::new();
        s.push(RecordedAction::Navigate { url: "https://x/".into() });
        s.push(RecordedAction::Click { selector: "#go".into(), frame: None });
        let code = s.to_rust();
        assert!(code.contains("use drission::prelude::*;"));
        assert!(code.contains("Browser::launch(BrowserOptions::new().headless(false))"));
        assert!(code.contains("    tab.get(\"https://x/\").await?;"));
        assert!(code.contains("    tab.click(\"#go\").await?;"));
        assert!(code.contains("browser.quit().await?;"));
    }

    #[test]
    fn multi_tab_switches_variable() {
        let mut s = RecordedScript::new();
        s.push(RecordedAction::Click { selector: "#open".into(), frame: None });
        s.push(RecordedAction::NewTab);
        s.push(RecordedAction::Click { selector: "#in-popup".into(), frame: None });
        let body = s.to_rust_body();
        assert!(body.contains("    tab.click(\"#open\").await?;"));
        assert!(body.contains("let tab_2 = tab.wait().new_tab(None).await?"));
        // 新标签后的动作切到 tab_2。
        assert!(body.contains("    tab_2.click(\"#in-popup\").await?;"));
    }

    #[test]
    fn empty_script_still_compiles_shape() {
        let code = RecordedScript::new().to_rust();
        assert!(code.contains("(未录制到任何动作)"));
        assert!(code.contains("Ok(())"));
    }

    #[test]
    fn json_roundtrip() {
        let mut s = RecordedScript::new();
        s.push(RecordedAction::Press { selector: "#q".into(), key: "Enter".into(), frame: None });
        s.push(RecordedAction::NewTab);
        let j = s.to_json();
        let back: Vec<RecordedAction> = serde_json::from_str(&j).unwrap();
        assert_eq!(back, s.actions);
        assert!(j.contains("\"type\": \"press\""));
        assert!(j.contains("\"type\": \"new_tab\""));
    }
}
