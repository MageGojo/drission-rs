//! CDP 后端共享内核 [`CdpCore`]:被 [`ChromiumTab`](crate::cdp::ChromiumTab) /
//! [`ChromiumElement`](crate::cdp::ChromiumElement) / 监听 / 拦截共用的底层能力。
//!
//! 职责:在某个 page 会话(`sessionId`)上做 `Runtime.evaluate` / `Runtime.callFunctionOn` /
//! `Input.dispatch*`(原生可信鼠标/键盘)/ 节点数组展开 / 超时管理。与 Juggler 后端的 `TabCore`
//! 同位,但讲 **CDP 方法名**(`Input.*` 而非 `Page.dispatch*Event`、`callFunctionOn` 的 `this`
//! 绑定而非首参 `node`)。

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::Mutex;
use tokio::task::AbortHandle;

use std::path::PathBuf;

use crate::cdp::console::ConsoleShared;
use crate::cdp::download::DownloadShared;
use crate::cdp::interceptor::InterceptShared;
use crate::cdp::listener::ListenShared;
use crate::cdp::screencast::ScreencastShared;
use crate::cdp::websocket::WsShared;
use crate::protocol::Connection;
use crate::{Error, Result};

/// 通用的"后台监听共享状态":缓冲 + 运行标志 + 任务 abort 句柄(console/websocket 复用)。
pub(crate) struct EventBuf<T> {
    pub(crate) buf: Arc<Mutex<VecDeque<T>>>,
    pub(crate) running: bool,
    pub(crate) abort: Option<AbortHandle>,
}

impl<T> Default for EventBuf<T> {
    fn default() -> Self {
        Self {
            buf: Arc::new(Mutex::new(VecDeque::new())),
            running: false,
            abort: None,
        }
    }
}

/// CDP 标签的共享内核(`Arc` 持有,克隆代价低)。同一标签的 `Tab`/`Element`/监听/拦截句柄共享它。
pub(crate) struct CdpCore {
    pub(crate) conn: Connection,
    pub(crate) session_id: String,
    pub(crate) target_id: String,
    /// 该标签所属的独立 BrowserContext(仅 `new_tab_with` 带代理时为 `Some`);`close` 时一并销毁。
    pub(crate) browser_context_id: Option<String>,
    timeout_ms: AtomicU64,
    /// 最近一次 `get` 是否加载成功(供 `url_available` 同步读取,对齐 camoufox)。
    last_load_ok: AtomicBool,
    /// 下载目录(`ChromiumOptions::download_path` 或 `tab.set_download_path` 设;
    /// `tab.downloads().start()` 据此跟踪)。运行时可变,故用同步 `Mutex`。
    download_dir: std::sync::Mutex<Option<PathBuf>>,
    /// 网络监听共享状态(缓冲 + 运行标志 + 后台任务句柄)。
    pub(crate) listen: Mutex<ListenShared>,
    /// 请求拦截共享状态(运行标志 + 后台任务句柄 + 决策接收端)。
    pub(crate) intercept: Mutex<InterceptShared>,
    /// 控制台监听共享状态。
    pub(crate) console: Mutex<ConsoleShared>,
    /// WebSocket 帧监听共享状态。
    pub(crate) ws: Mutex<WsShared>,
    /// 录像共享状态。
    pub(crate) screencast: Mutex<ScreencastShared>,
    /// 下载跟踪共享状态(missions + 运行标志 + 后台任务句柄)。
    pub(crate) downloads: Mutex<DownloadShared>,
}

impl CdpCore {
    pub(crate) fn new(
        conn: Connection,
        session_id: String,
        target_id: String,
        download_dir: Option<PathBuf>,
        browser_context_id: Option<String>,
    ) -> Arc<Self> {
        Arc::new(Self {
            conn,
            session_id,
            target_id,
            browser_context_id,
            timeout_ms: AtomicU64::new(30_000),
            last_load_ok: AtomicBool::new(false),
            download_dir: std::sync::Mutex::new(download_dir),
            listen: Mutex::new(ListenShared::default()),
            intercept: Mutex::new(InterceptShared::default()),
            console: Mutex::new(ConsoleShared::default()),
            ws: Mutex::new(WsShared::default()),
            screencast: Mutex::new(ScreencastShared::default()),
            downloads: Mutex::new(DownloadShared::default()),
        })
    }

    /// 当前下载目录(供 `downloads().start()` 读取)。
    pub(crate) fn download_dir(&self) -> Option<PathBuf> {
        self.download_dir.lock().ok().and_then(|g| g.clone())
    }

    /// 设置下载目录(`set_download_path` 调用,使后续 `downloads().start()` 可用)。
    pub(crate) fn set_download_dir(&self, dir: PathBuf) {
        if let Ok(mut g) = self.download_dir.lock() {
            *g = Some(dir);
        }
    }

    /// 当前默认超时。
    pub(crate) fn timeout(&self) -> Duration {
        Duration::from_millis(self.timeout_ms.load(Ordering::Relaxed))
    }

    /// 设置默认超时(运行时可调)。
    pub(crate) fn set_timeout(&self, d: Duration) {
        self.timeout_ms
            .store(d.as_millis() as u64, Ordering::Relaxed);
    }

    /// 记录最近一次 `get` 是否加载成功。
    pub(crate) fn set_load_ok(&self, ok: bool) {
        self.last_load_ok.store(ok, Ordering::Relaxed);
    }

    /// 最近一次 `get` 是否加载成功(同步读)。
    pub(crate) fn load_ok(&self) -> bool {
        self.last_load_ok.load(Ordering::Relaxed)
    }

    /// 在本标签 page 会话上发一个 CDP 命令并等响应。
    pub(crate) async fn send(&self, method: &str, params: Value) -> Result<Value> {
        self.conn.send(method, params, Some(&self.session_id)).await
    }

    // ── Runtime:求值 / 调用 ───────────────────────────────────────────────

    /// `Runtime.evaluate` 取**值**(`returnByValue`,自动 await Promise)。
    pub(crate) async fn eval_value(&self, expression: &str) -> Result<Value> {
        let r = self
            .send(
                "Runtime.evaluate",
                json!({ "expression": expression, "returnByValue": true, "awaitPromise": true }),
            )
            .await?;
        check_exception(&r)?;
        Ok(r["result"]["value"].clone())
    }

    /// `Runtime.evaluate` 取**句柄**(`objectId`);结果为 `null`/`undefined` 时返回 `None`。
    pub(crate) async fn eval_handle(&self, expression: &str) -> Result<Option<String>> {
        let r = self
            .send(
                "Runtime.evaluate",
                json!({ "expression": expression, "returnByValue": false, "awaitPromise": true }),
            )
            .await?;
        check_exception(&r)?;
        Ok(r["result"]["objectId"].as_str().map(str::to_string))
    }

    /// 在 `object_id` 上 `Runtime.callFunctionOn` 取**值**。函数声明里用 `this` 指代该对象。
    pub(crate) async fn call_value(
        &self,
        object_id: &str,
        declaration: &str,
        args: Vec<Value>,
    ) -> Result<Value> {
        let r = self.call_raw(object_id, declaration, args, true).await?;
        Ok(r["result"]["value"].clone())
    }

    /// 在 `object_id` 上 `Runtime.callFunctionOn` 取**句柄**(`objectId`);`null` 返回 `None`。
    pub(crate) async fn call_handle(
        &self,
        object_id: &str,
        declaration: &str,
        args: Vec<Value>,
    ) -> Result<Option<String>> {
        let r = self.call_raw(object_id, declaration, args, false).await?;
        Ok(r["result"]["objectId"].as_str().map(str::to_string))
    }

    async fn call_raw(
        &self,
        object_id: &str,
        declaration: &str,
        args: Vec<Value>,
        by_value: bool,
    ) -> Result<Value> {
        let r = self
            .send(
                "Runtime.callFunctionOn",
                json!({
                    "objectId": object_id,
                    "functionDeclaration": declaration,
                    "arguments": args,
                    "returnByValue": by_value,
                    "awaitPromise": true,
                }),
            )
            .await?;
        check_exception(&r)?;
        Ok(r)
    }

    /// 把一个"数组/类数组 RemoteObject"展开为其中各元素节点的 `objectId` 列表。
    /// 走 `Runtime.getProperties{ownProperties}`,只取数字下标且带 `objectId` 的项。
    pub(crate) async fn array_object_ids(&self, array_object_id: &str) -> Result<Vec<String>> {
        let props = self
            .send(
                "Runtime.getProperties",
                json!({ "objectId": array_object_id, "ownProperties": true }),
            )
            .await?;
        let mut out = Vec::new();
        if let Some(list) = props["result"].as_array() {
            for p in list {
                if p["name"].as_str().map(is_index).unwrap_or(false) {
                    if let Some(oid) = p["value"]["objectId"].as_str() {
                        out.push(oid.to_string());
                    }
                }
            }
        }
        Ok(out)
    }

    // ── Input:原生可信鼠标 / 键盘 ────────────────────────────────────────

    /// 派发一个鼠标事件(等响应)。`ty`:`mousePressed`/`mouseReleased`/`mouseMoved`;
    /// `button`:`none`/`left`/`middle`/`right`;`buttons`:当前按下位掩码(左=1/右=2/中=4)。
    pub(crate) async fn dispatch_mouse(
        &self,
        ty: &str,
        x: f64,
        y: f64,
        button: &str,
        buttons: i64,
        click_count: i64,
    ) -> Result<()> {
        self.send(
            "Input.dispatchMouseEvent",
            mouse_params(ty, x, y, button, buttons, click_count),
        )
        .await?;
        Ok(())
    }

    /// **不等响应**地派发鼠标事件(拟人轨迹密集移动用,节奏交给调用方 `sleep`)。
    pub(crate) fn dispatch_mouse_fire(
        &self,
        ty: &str,
        x: f64,
        y: f64,
        button: &str,
        buttons: i64,
        click_count: i64,
    ) -> Result<()> {
        self.conn.fire_session(
            "Input.dispatchMouseEvent",
            mouse_params(ty, x, y, button, buttons, click_count),
            Some(&self.session_id),
        )
    }

    /// 直接插入文本(等价 IME/粘贴,不产生按键事件;最快)。
    pub(crate) async fn insert_text(&self, text: &str) -> Result<()> {
        self.send("Input.insertText", json!({ "text": text }))
            .await?;
        Ok(())
    }

    /// 派发一个按键事件。`ty`:`keyDown`/`keyUp`/`rawKeyDown`/`char`;
    /// `modifiers` 为 CDP 修饰位掩码(Alt=1 / Ctrl=2 / Meta=4 / Shift=8),0 表示无。
    // 各参数都直接对应 CDP `Input.dispatchKeyEvent` 字段,内部热路径辅助,不拆结构体。
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn dispatch_key(
        &self,
        ty: &str,
        key: &str,
        code: &str,
        vk: i64,
        text: &str,
        modifiers: i64,
        commands: &[&str],
    ) -> Result<()> {
        let mut p = serde_json::Map::new();
        p.insert("type".into(), json!(ty));
        p.insert("key".into(), json!(key));
        if !code.is_empty() {
            p.insert("code".into(), json!(code));
        }
        if vk != 0 {
            p.insert("windowsVirtualKeyCode".into(), json!(vk));
            p.insert("nativeVirtualKeyCode".into(), json!(vk));
        }
        if modifiers != 0 {
            p.insert("modifiers".into(), json!(modifiers));
        }
        // 编辑命令(如 selectAll/copy/paste):浏览器据此**真正执行**编辑动作,弥补无头 Chrome
        // 不把合成快捷键自动翻译成编辑命令的限制(对齐 Puppeteer 的 `commands` 用法)。
        if !commands.is_empty() {
            p.insert("commands".into(), json!(commands));
        }
        if !text.is_empty() {
            p.insert("text".into(), json!(text));
        }
        self.send("Input.dispatchKeyEvent", Value::Object(p))
            .await?;
        Ok(())
    }

    /// 完整敲一个键(`keyDown`+`keyUp`;可打印键带 `text` 以同时插入字符并触发监听)。
    pub(crate) async fn press_key(&self, key: &str) -> Result<()> {
        let (k, code, vk, text) = cdp_key_descriptor(key);
        self.dispatch_key("keyDown", &k, &code, vk, &text, 0, &[])
            .await?;
        self.dispatch_key("keyUp", &k, &code, vk, "", 0, &[])
            .await?;
        Ok(())
    }

    /// **修饰组合键 / 热键**(如 Ctrl+A、Cmd+C):`keys` 最后一项为主键,其余为修饰键
    /// (`Control`/`Ctrl`、`Shift`、`Alt`、`Meta`/`Cmd`)。CDP 原生 `modifiers` 位掩码下发,
    /// 页面能读到 `e.ctrlKey`/`metaKey` 等为 `true`(真组合键,非逐键)。组合键不插入字符
    /// (主键 `text` 留空),故 Ctrl+A 触发"全选"而不会键入 "a"。
    ///
    /// 对常见**编辑快捷键**(Ctrl/Cmd + A/C/X/V/Z/Y)额外带上 CDP `commands`(selectAll/copy…),
    /// 让浏览器**真正执行**该编辑动作(无头 Chrome 不会自动把合成快捷键翻译成编辑命令)。
    pub(crate) async fn key_combo(&self, keys: &[&str]) -> Result<()> {
        if keys.is_empty() {
            return Ok(());
        }
        let (mods, main) = keys.split_at(keys.len() - 1);
        let main = main[0];
        let full: i64 = mods.iter().map(|k| modifier_bit(k)).fold(0, |a, b| a | b);
        let cmds = editing_commands(full, main);

        // 逐个按下修饰键(modifiers 累积包含自身)。
        let mut acc = 0i64;
        for k in mods {
            acc |= modifier_bit(k);
            let (kk, code, vk, _) = cdp_key_descriptor(k);
            self.dispatch_key("rawKeyDown", &kk, &code, vk, "", acc, &[])
                .await?;
        }
        // 主键 down+up(带完整修饰位 + 编辑命令;text 留空 → 组合键不键入字符)。
        let (mk, mcode, mvk, _) = cdp_key_descriptor(main);
        self.dispatch_key("rawKeyDown", &mk, &mcode, mvk, "", full, &cmds)
            .await?;
        self.dispatch_key("keyUp", &mk, &mcode, mvk, "", full, &[])
            .await?;
        // 反序松开修饰键。
        for k in mods.iter().rev() {
            let (kk, code, vk, _) = cdp_key_descriptor(k);
            acc &= !modifier_bit(k);
            self.dispatch_key("keyUp", &kk, &code, vk, "", acc, &[])
                .await?;
        }
        Ok(())
    }
}

/// 键名 → CDP 修饰位掩码(Alt=1 / Ctrl=2 / Meta=4 / Shift=8;非修饰键为 0)。
pub(crate) fn modifier_bit(key: &str) -> i64 {
    match key {
        "Alt" | "Option" => 1,
        "Control" | "Ctrl" => 2,
        "Meta" | "Command" | "Cmd" => 4,
        "Shift" => 8,
        _ => 0,
    }
}

/// 把"主修饰键(Ctrl/Cmd)+ 字母"映射到 CDP 编辑命令名(空 = 无对应编辑命令)。
/// 让 `key_combo` 对编辑类快捷键能在无头下**真正执行**(selectAll/copy/cut/paste/undo/redo)。
fn editing_commands(mask: i64, main: &str) -> Vec<&'static str> {
    let primary = mask & 0b0110 != 0; // Ctrl(2) 或 Meta(4)
    if !primary {
        return Vec::new();
    }
    let shift = mask & 8 != 0;
    match main.to_ascii_lowercase().as_str() {
        "a" => vec!["selectAll"],
        "c" => vec!["copy"],
        "x" => vec!["cut"],
        "v" => vec!["paste"],
        "z" => vec![if shift { "redo" } else { "undo" }],
        "y" => vec!["redo"],
        _ => Vec::new(),
    }
}

impl Drop for CdpCore {
    /// 最后一个持有者析构时,中止监听/拦截后台任务(它们只持 `Connection`,不持 `CdpCore`,
    /// 故此处能可靠 `abort`,不形成自环)。
    fn drop(&mut self) {
        if let Ok(g) = self.listen.try_lock() {
            if let Some(a) = &g.abort {
                a.abort();
            }
        }
        if let Ok(g) = self.intercept.try_lock() {
            if let Some(a) = &g.abort {
                a.abort();
            }
        }
        if let Ok(g) = self.console.try_lock() {
            if let Some(a) = &g.abort {
                a.abort();
            }
        }
        if let Ok(g) = self.ws.try_lock() {
            if let Some(a) = &g.abort {
                a.abort();
            }
        }
        if let Ok(g) = self.screencast.try_lock() {
            if let Some(a) = &g.abort {
                a.abort();
            }
        }
        if let Ok(g) = self.downloads.try_lock() {
            if let Some(a) = &g.abort {
                a.abort();
            }
        }
    }
}

/// 组装 `Input.dispatchMouseEvent` 参数。
fn mouse_params(ty: &str, x: f64, y: f64, button: &str, buttons: i64, click_count: i64) -> Value {
    json!({
        "type": ty,
        "x": x,
        "y": y,
        "button": button,
        "buttons": buttons,
        "clickCount": click_count,
        "modifiers": 0,
    })
}

/// `Runtime.evaluate`/`callFunctionOn` 的异常检查:有 `exceptionDetails` 即报协议错误。
fn check_exception(r: &Value) -> Result<()> {
    if let Some(exc) = r.get("exceptionDetails") {
        let msg = exc["exception"]["description"]
            .as_str()
            .or_else(|| exc["text"].as_str())
            .unwrap_or("JS 异常");
        return Err(Error::Protocol(format!("CDP JS 异常: {msg}")));
    }
    Ok(())
}

/// 是否是非负整数下标字符串(用于从对象属性里挑数组元素)。
fn is_index(name: &str) -> bool {
    !name.is_empty() && name.bytes().all(|b| b.is_ascii_digit())
}

/// 把 DP/DOM 键名翻译成 CDP `Input.dispatchKeyEvent` 的 `(key, code, windowsVirtualKeyCode, text)`。
///
/// 特殊键 `text` 为空(只触发按键事件、不插入字符);可打印单字符 `text` 即字符本身
/// (CDP 的 `keyDown` 带 `text` 会同时产生 keypress/input,故可一次性敲入)。
pub(crate) fn cdp_key_descriptor(key: &str) -> (String, String, i64, String) {
    let owned =
        |k: &str, c: &str, vk: i64, t: &str| (k.to_string(), c.to_string(), vk, t.to_string());
    match key {
        "Enter" | "\n" | "\r" => owned("Enter", "Enter", 13, "\r"),
        "Tab" => owned("Tab", "Tab", 9, ""),
        "Backspace" => owned("Backspace", "Backspace", 8, ""),
        "Delete" | "Del" => owned("Delete", "Delete", 46, ""),
        "Escape" | "Esc" => owned("Escape", "Escape", 27, ""),
        "Insert" => owned("Insert", "Insert", 45, ""),
        "Home" => owned("Home", "Home", 36, ""),
        "End" => owned("End", "End", 35, ""),
        "PageUp" => owned("PageUp", "PageUp", 33, ""),
        "PageDown" => owned("PageDown", "PageDown", 34, ""),
        "ArrowUp" | "Up" => owned("ArrowUp", "ArrowUp", 38, ""),
        "ArrowDown" | "Down" => owned("ArrowDown", "ArrowDown", 40, ""),
        "ArrowLeft" | "Left" => owned("ArrowLeft", "ArrowLeft", 37, ""),
        "ArrowRight" | "Right" => owned("ArrowRight", "ArrowRight", 39, ""),
        " " | "Space" => owned(" ", "Space", 32, " "),
        "Control" | "Ctrl" => owned("Control", "ControlLeft", 17, ""),
        "Shift" => owned("Shift", "ShiftLeft", 16, ""),
        "Alt" => owned("Alt", "AltLeft", 18, ""),
        "Meta" | "Command" | "Cmd" => owned("Meta", "MetaLeft", 91, ""),
        other => {
            let mut chars = other.chars();
            match (chars.next(), chars.next()) {
                // 单个字符:推导 code 与虚拟键码,text 即字符本身。
                (Some(ch), None) => {
                    let (code, vk) = char_code(ch);
                    (ch.to_string(), code, vk, ch.to_string())
                }
                // 多字符串(非已知键名):当作要插入的文本,无按键码。
                _ => owned(other, "", 0, other),
            }
        }
    }
}

/// 单字符的 `(code, windowsVirtualKeyCode)`(覆盖字母/数字;其余给空 code、0 键码,靠 `text` 插入)。
fn char_code(ch: char) -> (String, i64) {
    if ch.is_ascii_alphabetic() {
        let up = ch.to_ascii_uppercase();
        (format!("Key{up}"), up as i64)
    } else if ch.is_ascii_digit() {
        (format!("Digit{ch}"), ch as i64)
    } else {
        (String::new(), 0)
    }
}

/// 极简 xorshift64(std-only),用于拟人输入/拖拽的随机停顿与抖动。
pub(crate) struct Xorshift(u64);

impl Xorshift {
    pub(crate) fn new(seed: u64) -> Self {
        Xorshift(seed | 1)
    }
    pub(crate) fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    pub(crate) fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
    pub(crate) fn range_ms(&mut self, lo: u64, hi: u64) -> u64 {
        if hi <= lo {
            lo
        } else {
            lo + self.next_u64() % (hi - lo)
        }
    }
}

/// 用系统时钟纳秒做种子。
pub(crate) fn seed_from_clock() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9E37_79B9_7F4A_7C15)
}

/// 生成一段拟人拖拽轨迹:`(相对起点 x 位移, y 位移, 该步后停顿毫秒)`。
///
/// 神经运动学 **minimum-jerk** 速度 `10t³-15t⁴+6t⁵`(钟形:慢起→中段最快→慢收)+ 时间驱动密集
/// 采样(~13ms/点,真人 60~120Hz)+ 手抖 + 纵向漂移 + 末段过冲回拉到精确目标。`duration_secs<=0`
/// 按距离自估。与 Juggler 后端 `human_drag_track` 同构(此处供 CDP `Input.dispatchMouseEvent` fire 路径用)。
pub(crate) fn human_drag_track(
    dx: f64,
    dy: f64,
    duration_secs: f64,
    seed: u64,
) -> Vec<(f64, f64, u64)> {
    let mut rng = Xorshift::new(seed);
    let dist = (dx * dx + dy * dy).sqrt();
    let dur_ms = if duration_secs > 0.0 {
        (duration_secs * 1000.0).round()
    } else {
        (dist * 3.5 + 320.0).clamp(350.0, 1600.0)
    };
    let n = ((dur_ms / 13.0).round() as usize).clamp(24, 160);
    let base = (dur_ms / n as f64).max(4.0);
    let overshoot = if dist > 40.0 {
        1.0 + 0.02 + rng.unit() * 0.04
    } else {
        1.0
    };
    let fwd = ((n as f64) * 0.82) as usize;
    let back = n.saturating_sub(fwd).max(3);
    let drift_y = (rng.unit() - 0.5) * 6.0;
    let mj = |t: f64| 10.0 * t.powi(3) - 15.0 * t.powi(4) + 6.0 * t.powi(5);
    let delay = |rng: &mut Xorshift| -> u64 {
        let jit = (rng.unit() - 0.5) * 0.8;
        ((base * (1.0 + jit)).round() as u64).max(3)
    };

    let mut out = Vec::with_capacity(n + 2);
    for i in 1..=fwd {
        let t = i as f64 / fwd as f64;
        let frac = overshoot * mj(t);
        out.push((
            dx * frac + (rng.unit() - 0.5),
            dy * frac + drift_y * mj(t) + (rng.unit() - 0.5) * 1.6,
            delay(&mut rng),
        ));
    }
    for i in 1..=back {
        let t = i as f64 / back as f64;
        let frac = overshoot - (overshoot - 1.0) * mj(t);
        out.push((
            dx * frac + (rng.unit() - 0.5) * 0.8,
            dy * frac + drift_y + (rng.unit() - 0.5) * 1.2,
            delay(&mut rng) + 2,
        ));
    }
    out.push((dx, dy, base.round() as u64));

    // 偶发迟疑(模拟人手中途微停)。
    let pauses = 1 + (rng.unit() * 2.0) as usize;
    for _ in 0..pauses {
        if fwd > 4 {
            let idx = 2 + (rng.unit() * (fwd as f64 - 4.0)) as usize;
            if let Some(p) = out.get_mut(idx) {
                p.2 += rng.range_ms(25, 75);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_descriptor_specials_and_chars() {
        let (k, c, vk, t) = cdp_key_descriptor("Enter");
        assert_eq!(
            (k.as_str(), c.as_str(), vk, t.as_str()),
            ("Enter", "Enter", 13, "\r")
        );
        let (k, c, vk, t) = cdp_key_descriptor("a");
        assert_eq!(
            (k.as_str(), c.as_str(), vk, t.as_str()),
            ("a", "KeyA", 65, "a")
        );
        let (k, c, vk, t) = cdp_key_descriptor("5");
        assert_eq!(
            (k.as_str(), c.as_str(), vk, t.as_str()),
            ("5", "Digit5", 53, "5")
        );
        // 多字符串非已知键名:当作文本插入。
        let (k, c, vk, t) = cdp_key_descriptor("hello");
        assert_eq!(
            (k.as_str(), c.as_str(), vk, t.as_str()),
            ("hello", "", 0, "hello")
        );
    }

    #[test]
    fn is_index_basic() {
        assert!(is_index("0"));
        assert!(is_index("12"));
        assert!(!is_index(""));
        assert!(!is_index("length"));
        assert!(!is_index("1a"));
    }

    #[test]
    fn modifier_bit_mapping() {
        // CDP 位掩码:Alt=1 / Ctrl=2 / Meta=4 / Shift=8。
        assert_eq!(modifier_bit("Alt"), 1);
        assert_eq!(modifier_bit("Control"), 2);
        assert_eq!(modifier_bit("Ctrl"), 2);
        assert_eq!(modifier_bit("Meta"), 4);
        assert_eq!(modifier_bit("Cmd"), 4);
        assert_eq!(modifier_bit("Shift"), 8);
        // 非修饰键(含主键)为 0。
        assert_eq!(modifier_bit("a"), 0);
        assert_eq!(modifier_bit("Enter"), 0);
        // 组合的完整位掩码:Ctrl+Shift = 2|8 = 10。
        let full = ["Control", "Shift"]
            .iter()
            .map(|k| modifier_bit(k))
            .fold(0, |a, b| a | b);
        assert_eq!(full, 10);
    }

    #[test]
    fn editing_commands_mapping() {
        // Ctrl(2) / Meta(4) + 字母 → 对应编辑命令。
        assert_eq!(editing_commands(2, "a"), vec!["selectAll"]);
        assert_eq!(editing_commands(4, "A"), vec!["selectAll"]); // 大小写不敏感
        assert_eq!(editing_commands(2, "c"), vec!["copy"]);
        assert_eq!(editing_commands(2, "v"), vec!["paste"]);
        assert_eq!(editing_commands(2, "x"), vec!["cut"]);
        assert_eq!(editing_commands(2, "z"), vec!["undo"]);
        // Ctrl+Shift+Z = redo(掩码 2|8=10)。
        assert_eq!(editing_commands(10, "z"), vec!["redo"]);
        assert_eq!(editing_commands(2, "y"), vec!["redo"]);
        // 无主修饰键(纯 Shift)→ 无编辑命令。
        assert!(editing_commands(8, "a").is_empty());
        // 非编辑字母 → 无命令。
        assert!(editing_commands(2, "b").is_empty());
    }
}
