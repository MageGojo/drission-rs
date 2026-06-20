//! 键名常量 [`Keys`] 与按键序列输入 [`KeyInput`](对标 DrissionPage 的 `Keys` 与 `ele.input` 序列)。
//!
//! **后端无关**:Camoufox(Juggler)与 Chromium(CDP)两后端共用,故住在 crate 顶层、始终编译。
//!
//! - [`Keys`][]:常用特殊键名常量,值即 DOM 的 `key` 名;普通字符直接用字符串即可。
//!   用于 `Tab::press_key` / [`KeyInput::key`]。
//! - [`KeyInput`][]:把"文本 + 特殊键"混排成一个序列,交给 `Element::input_keys`
//!   (对应 DP `ele.input(['abc', Keys.ENTER])`)。
//!
//! ```ignore
//! use drission::prelude::*;
//! tab.press_key(Keys::ENTER).await?;
//! ele.input_keys(&[KeyInput::text("hello"), KeyInput::key(Keys::ENTER)]).await?;
//! ```
//!
//! **修饰组合键 / 热键**:
//! - **CDP / Chromium 后端**:**已支持**。`tab.key_combo(&[Keys::CONTROL, "a"])` /
//!   `ele.shortcut(&[Keys::META, "a"])`——CDP 原生 `modifiers` 位掩码下发(页面读得到 `e.ctrlKey`/
//!   `metaKey` 等为 `true`),并对常见编辑快捷键(Ctrl/Cmd + A/C/X/V/Z/Y)带上 CDP `commands` 让
//!   浏览器**真正执行**编辑动作(selectAll/copy…),无头下也生效。
//! - **平台限制(Camoufox 后端)**:Camoufox 当前的 Juggler `dispatchKeyEvent` **没有 `modifiers` 字段**,
//!   也不会跨调用跟踪"修饰键按下态"(合成的主键事件 `e.ctrlKey` 仍为 false)→ **修饰组合键的原生
//!   效果无法合成**(非库缺陷)。需要"全选"等可用 JS:如 `ele.run_js("node.select()")`(输入框)
//!   或 `tab.run_js("document.execCommand('selectAll')")`。

/// 常用特殊键名常量(值即 DOM 的 `key` 名)。
pub struct Keys;

impl Keys {
    pub const ENTER: &'static str = "Enter";
    pub const TAB: &'static str = "Tab";
    pub const ESCAPE: &'static str = "Escape";
    pub const BACKSPACE: &'static str = "Backspace";
    pub const DELETE: &'static str = "Delete";
    pub const INSERT: &'static str = "Insert";
    pub const SPACE: &'static str = "Space";
    pub const HOME: &'static str = "Home";
    pub const END: &'static str = "End";
    pub const PAGE_UP: &'static str = "PageUp";
    pub const PAGE_DOWN: &'static str = "PageDown";
    pub const ARROW_UP: &'static str = "ArrowUp";
    pub const ARROW_DOWN: &'static str = "ArrowDown";
    pub const ARROW_LEFT: &'static str = "ArrowLeft";
    pub const ARROW_RIGHT: &'static str = "ArrowRight";
    pub const CONTROL: &'static str = "Control";
    pub const SHIFT: &'static str = "Shift";
    pub const ALT: &'static str = "Alt";
    pub const META: &'static str = "Meta";
}

/// 按键序列的一项:文本 或 特殊键(对应 DP `ele.input(['abc', Keys.ENTER])`)。
#[derive(Debug, Clone)]
pub enum KeyInput {
    /// 直接插入的文本片段。
    Text(String),
    /// 一个特殊键(键名见 [`Keys`])。
    Key(String),
}

impl KeyInput {
    /// 构造一个文本片段项。
    pub fn text(s: impl Into<String>) -> Self {
        KeyInput::Text(s.into())
    }

    /// 构造一个特殊键项。
    pub fn key(s: impl Into<String>) -> Self {
        KeyInput::Key(s.into())
    }
}

impl From<&str> for KeyInput {
    fn from(s: &str) -> Self {
        KeyInput::Text(s.to_string())
    }
}

impl From<String> for KeyInput {
    fn from(s: String) -> Self {
        KeyInput::Text(s)
    }
}
