//! 动作链 [`Actions`]:对应 DrissionPage 的 `tab.actions` / Selenium ActionChains。
//!
//! 把一串鼠标/键盘动作**链式**串起来,最后 `.perform().await` 一次顺序执行。最典型的用途是
//! **拖放**:移到源元素 → 按住 → 移到目标 → 释放。鼠标移动自带拟人轨迹(缓动 + 抖动),
//! **按住期间的移动自动是拖拽**(`buttons=1`)。
//!
//! ```ignore
//! tab.actions()
//!     .move_to_ele(&src)      // 移到源元素中心
//!     .hold()                 // 按住左键
//!     .move_to_ele(&dst)      // 拖到目标元素(按住中→即拖拽)
//!     .release()              // 释放
//!     .perform().await?;
//!
//! // 也可拖到绝对坐标 / 相对位移:
//! tab.actions().hold_on(&slider).move_by(120.0, 0.0, 0.6).release().perform().await?;
//! ```

use std::time::Duration;

use serde_json::json;
use tokio::time::sleep;

use crate::Result;
use crate::browser::element::Element;
use crate::browser::tab::Tab;

/// 鼠标键。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
}

impl MouseButton {
    /// `button` 字段(0=左/1=中/2=右)。
    fn id(self) -> i64 {
        match self {
            MouseButton::Left => 0,
            MouseButton::Middle => 1,
            MouseButton::Right => 2,
        }
    }
    /// `buttons` 位掩码(左=1/右=2/中=4)。
    fn bit(self) -> i64 {
        match self {
            MouseButton::Left => 1,
            MouseButton::Right => 2,
            MouseButton::Middle => 4,
        }
    }
}

/// 单个动作(在 [`Actions::perform`] 时按序执行)。
enum Act {
    MoveAbs(f64, f64, f64),               // x, y, duration
    MoveEle(Box<Element>, f64, f64, f64), // 元素, 偏移x, 偏移y, duration
    MoveBy(f64, f64, f64),                // dx, dy, duration
    Down(MouseButton),
    Up(MouseButton),
    Click(MouseButton, u32), // 按键, 连击次数(1/2)
    Scroll(f64, f64),        // dx, dy
    KeyDown(String),
    KeyUp(String),
    Type(String),
    Wait(f64),
}

/// 动作链句柄(`tab.actions()` 返回)。链式收集动作,`perform().await` 执行。
pub struct Actions {
    tab: Tab,
    steps: Vec<Act>,
    default_move_secs: f64,
}

impl Actions {
    pub(crate) fn new(tab: Tab) -> Self {
        Self {
            tab,
            steps: Vec::new(),
            default_move_secs: 0.4,
        }
    }

    // ---------------- 移动 ----------------

    /// 移动到元素中心(默认时长)。
    pub fn move_to_ele(self, ele: &Element) -> Self {
        let d = self.default_move_secs;
        self.move_to_ele_offset(ele, 0.0, 0.0, d)
    }

    /// 移动到元素中心 + 偏移,自定义时长(秒)。
    pub fn move_to_ele_offset(mut self, ele: &Element, ox: f64, oy: f64, duration: f64) -> Self {
        self.steps
            .push(Act::MoveEle(Box::new(ele.clone()), ox, oy, duration));
        self
    }

    /// 移动到视口绝对坐标 `(x, y)`,自定义时长。
    pub fn move_to(mut self, x: f64, y: f64, duration: f64) -> Self {
        self.steps.push(Act::MoveAbs(x, y, duration));
        self
    }

    /// 相对当前位置移动 `(dx, dy)`,自定义时长。
    pub fn move_by(mut self, dx: f64, dy: f64, duration: f64) -> Self {
        self.steps.push(Act::MoveBy(dx, dy, duration));
        self
    }

    /// 向上移动 `pixel` 像素。
    pub fn up(self, pixel: f64) -> Self {
        let d = self.default_move_secs;
        self.move_by(0.0, -pixel, d)
    }
    /// 向下移动 `pixel` 像素。
    pub fn down(self, pixel: f64) -> Self {
        let d = self.default_move_secs;
        self.move_by(0.0, pixel, d)
    }
    /// 向左移动 `pixel` 像素。
    pub fn left(self, pixel: f64) -> Self {
        let d = self.default_move_secs;
        self.move_by(-pixel, 0.0, d)
    }
    /// 向右移动 `pixel` 像素。
    pub fn right(self, pixel: f64) -> Self {
        let d = self.default_move_secs;
        self.move_by(pixel, 0.0, d)
    }

    // ---------------- 按键(鼠标) ----------------

    /// 在当前位置**按住左键**(开始拖拽)。
    pub fn hold(mut self) -> Self {
        self.steps.push(Act::Down(MouseButton::Left));
        self
    }

    /// 先移到元素,再按住左键。
    pub fn hold_on(self, ele: &Element) -> Self {
        self.move_to_ele(ele).hold()
    }

    /// 在当前位置**释放左键**(结束拖拽)。
    pub fn release(mut self) -> Self {
        self.steps.push(Act::Up(MouseButton::Left));
        self
    }

    /// 先移到元素,再释放左键。
    pub fn release_on(self, ele: &Element) -> Self {
        self.move_to_ele(ele).release()
    }

    /// 左键单击(当前位置)。
    pub fn click(mut self) -> Self {
        self.steps.push(Act::Click(MouseButton::Left, 1));
        self
    }
    /// 左键双击。
    pub fn double_click(mut self) -> Self {
        self.steps.push(Act::Click(MouseButton::Left, 2));
        self
    }
    /// 右键单击。
    pub fn right_click(mut self) -> Self {
        self.steps.push(Act::Click(MouseButton::Right, 1));
        self
    }
    /// 中键单击。
    pub fn middle_click(mut self) -> Self {
        self.steps.push(Act::Click(MouseButton::Middle, 1));
        self
    }
    /// 自定义按键按下。
    pub fn mouse_down(mut self, button: MouseButton) -> Self {
        self.steps.push(Act::Down(button));
        self
    }
    /// 自定义按键松开。
    pub fn mouse_up(mut self, button: MouseButton) -> Self {
        self.steps.push(Act::Up(button));
        self
    }

    // ---------------- 滚轮 / 键盘 / 等待 ----------------

    /// 在当前位置滚动滚轮 `(dx, dy)`(`dy>0` 向下)。
    pub fn scroll(mut self, dx: f64, dy: f64) -> Self {
        self.steps.push(Act::Scroll(dx, dy));
        self
    }

    /// 按下一个键(如 `"Shift"`、`"Control"`、`"a"`)。
    pub fn key_down(mut self, key: &str) -> Self {
        self.steps.push(Act::KeyDown(key.to_string()));
        self
    }
    /// 松开一个键。
    pub fn key_up(mut self, key: &str) -> Self {
        self.steps.push(Act::KeyUp(key.to_string()));
        self
    }
    /// 输入一段文本(`Page.insertText`)。
    pub fn type_text(mut self, text: &str) -> Self {
        self.steps.push(Act::Type(text.to_string()));
        self
    }

    /// 等待 `secs` 秒。
    pub fn wait(mut self, secs: f64) -> Self {
        self.steps.push(Act::Wait(secs));
        self
    }

    // ---------------- 执行 ----------------

    /// 顺序执行已串好的全部动作。
    pub async fn perform(self) -> Result<()> {
        let core = &self.tab.core;
        let mut cur = (0.0_f64, 0.0_f64);
        let mut held = 0i64; // 当前按下的按键位掩码

        for act in self.steps {
            match act {
                Act::MoveAbs(x, y, d) => {
                    glide(&self.tab, cur, (x, y), d, held).await?;
                    cur = (x, y);
                }
                Act::MoveEle(ele, ox, oy, d) => {
                    ele.scroll_into_view().await?;
                    let (cx, cy) = ele.center_point().await?;
                    let to = (cx + ox, cy + oy);
                    glide(&self.tab, cur, to, d, held).await?;
                    cur = to;
                }
                Act::MoveBy(dx, dy, d) => {
                    let to = (cur.0 + dx, cur.1 + dy);
                    glide(&self.tab, cur, to, d, held).await?;
                    cur = to;
                }
                Act::Down(b) => {
                    held |= b.bit();
                    core.dispatch_mouse_ex("mousedown", cur.0, cur.1, b.id(), held, 1)
                        .await?;
                    sleep(Duration::from_millis(60)).await;
                }
                Act::Up(b) => {
                    held &= !b.bit();
                    core.dispatch_mouse_ex("mouseup", cur.0, cur.1, b.id(), held, 1)
                        .await?;
                    sleep(Duration::from_millis(30)).await;
                }
                Act::Click(b, count) => {
                    core.dispatch_mouse("mousemove", cur.0, cur.1, held).await?;
                    for n in 1..=count {
                        let down_buttons = held | b.bit();
                        core.dispatch_mouse_ex(
                            "mousedown",
                            cur.0,
                            cur.1,
                            b.id(),
                            down_buttons,
                            n as i64,
                        )
                        .await?;
                        sleep(Duration::from_millis(40)).await;
                        core.dispatch_mouse_ex("mouseup", cur.0, cur.1, b.id(), held, n as i64)
                            .await?;
                        sleep(Duration::from_millis(40)).await;
                    }
                }
                Act::Scroll(dx, dy) => {
                    core.send_page(
                        "Page.dispatchWheelEvent",
                        json!({ "x": cur.0.floor(), "y": cur.1.floor(),
                                "deltaX": dx, "deltaY": dy, "deltaZ": 0, "modifiers": 0 }),
                    )
                    .await?;
                }
                Act::KeyDown(k) => {
                    core.send_page(
                        "Page.dispatchKeyEvent",
                        json!({ "type": "keydown", "key": k }),
                    )
                    .await?;
                }
                Act::KeyUp(k) => {
                    core.send_page(
                        "Page.dispatchKeyEvent",
                        json!({ "type": "keyup", "key": k }),
                    )
                    .await?;
                }
                Act::Type(t) => {
                    core.send_page("Page.insertText", json!({ "text": t }))
                        .await?;
                }
                Act::Wait(s) => {
                    sleep(Duration::from_secs_f64(s.max(0.0))).await;
                }
            }
        }
        Ok(())
    }
}

/// 从 `from` 缓动滑到 `to`,`duration` 秒;`held_buttons` 非 0 时为按住拖拽(`buttons` 带位)。
async fn glide(
    tab: &Tab,
    from: (f64, f64),
    to: (f64, f64),
    duration: f64,
    held: i64,
) -> Result<()> {
    let dx = to.0 - from.0;
    let dy = to.1 - from.1;
    let dist = (dx * dx + dy * dy).sqrt();
    let steps = ((dist / 8.0).round() as usize).clamp(6, 50);
    let per = if duration > 0.0 {
        ((duration * 1000.0) / steps as f64) as u64
    } else {
        4
    };
    for i in 1..=steps {
        let t = i as f64 / steps as f64;
        // ease-in-out cubic
        let eased = if t < 0.5 {
            4.0 * t * t * t
        } else {
            1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
        };
        let jx = (i as f64 * 1.3).sin() * 0.8;
        let jy = (i as f64 * 1.7).cos() * 0.8;
        let x = from.0 + dx * eased + jx;
        let y = from.1 + dy * eased + jy;
        tab.core.dispatch_mouse("mousemove", x, y, held).await?;
        if per > 0 {
            sleep(Duration::from_millis(per)).await;
        }
    }
    // 末步精确落点。
    tab.core
        .dispatch_mouse("mousemove", to.0, to.1, held)
        .await?;
    Ok(())
}
