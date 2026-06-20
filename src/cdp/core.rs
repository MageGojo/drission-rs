//! CDP 后端共享内核 [`CdpCore`]:被 [`ChromiumTab`](crate::cdp::ChromiumTab) /
//! [`ChromiumElement`](crate::cdp::ChromiumElement) / 监听 / 拦截共用的底层能力。
//!
//! 职责:在某个 page 会话(`sessionId`)上做 `Runtime.evaluate` / `Runtime.callFunctionOn` /
//! `Input.dispatch*`(原生可信鼠标/键盘)/ 节点数组展开 / 超时管理。与 Juggler 后端的 `TabCore`
//! 同位,但讲 **CDP 方法名**(`Input.*` 而非 `Page.dispatch*Event`、`callFunctionOn` 的 `this`
//! 绑定而非首参 `node`)。

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::cdp::interceptor::InterceptShared;
use crate::cdp::listener::ListenShared;
use crate::protocol::Connection;
use crate::{Error, Result};

/// CDP 标签的共享内核(`Arc` 持有,克隆代价低)。同一标签的 `Tab`/`Element`/监听/拦截句柄共享它。
pub(crate) struct CdpCore {
    pub(crate) conn: Connection,
    pub(crate) session_id: String,
    #[allow(dead_code)]
    pub(crate) target_id: String,
    timeout_ms: AtomicU64,
    /// 网络监听共享状态(缓冲 + 运行标志 + 后台任务句柄)。
    pub(crate) listen: Mutex<ListenShared>,
    /// 请求拦截共享状态(运行标志 + 后台任务句柄 + 决策接收端)。
    pub(crate) intercept: Mutex<InterceptShared>,
}

impl CdpCore {
    pub(crate) fn new(conn: Connection, session_id: String, target_id: String) -> Arc<Self> {
        Arc::new(Self {
            conn,
            session_id,
            target_id,
            timeout_ms: AtomicU64::new(30_000),
            listen: Mutex::new(ListenShared::default()),
            intercept: Mutex::new(InterceptShared::default()),
        })
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

    /// 派发一个按键事件。`ty`:`keyDown`/`keyUp`/`rawKeyDown`/`char`。
    pub(crate) async fn dispatch_key(
        &self,
        ty: &str,
        key: &str,
        code: &str,
        vk: i64,
        text: &str,
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
        self.dispatch_key("keyDown", &k, &code, vk, &text).await?;
        self.dispatch_key("keyUp", &k, &code, vk, "").await?;
        Ok(())
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
}
