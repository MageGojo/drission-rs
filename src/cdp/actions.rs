//! CDP 后端的**动作链** [`ChromiumActions`](对齐 camoufox `Actions` / DP `ActionChains`)。
//!
//! 链式串起鼠标/键盘动作,`perform().await` 一次性顺序执行。按住左键期间移动自动成为**拖拽**
//! (`buttons=1`);移动走拟人密集采样(`dispatch_mouse_fire` 不等往返 + `sleep` 控节奏)。
//! 鼠标事件均为可信事件(`Input.dispatchMouseEvent`,`isTrusted=true`)。

use std::sync::Arc;
use std::time::Duration;

use tokio::time::sleep;

use crate::Result;
use crate::cdp::core::{CdpCore, Xorshift, seed_from_clock};
use crate::cdp::element::ChromiumElement;

enum Step {
    MoveTo(f64, f64, f64),
    MoveBy(f64, f64, f64),
    MoveToEle(ChromiumElement, f64),
    Down(&'static str),
    Up(&'static str),
    Click(&'static str, i64),
    KeyDown(String),
    KeyUp(String),
    Type(String),
    Scroll(f64, f64),
    Wait(f64),
}

/// 动作链(由 [`ChromiumTab::actions`](crate::cdp::ChromiumTab) 返回)。链式记录,`perform()` 执行。
pub struct ChromiumActions {
    core: Arc<CdpCore>,
    steps: Vec<Step>,
    x: f64,
    y: f64,
    left_held: bool,
}

impl ChromiumActions {
    pub(crate) fn new(core: Arc<CdpCore>) -> Self {
        Self {
            core,
            steps: Vec::new(),
            x: 0.0,
            y: 0.0,
            left_held: false,
        }
    }

    /// 移动到视口坐标 `(x, y)`,用 `duration` 秒(拟人)。
    pub fn move_to(mut self, x: f64, y: f64, duration: f64) -> Self {
        self.steps.push(Step::MoveTo(x, y, duration));
        self
    }

    /// 相对当前位置移动 `(dx, dy)`。
    pub fn move_by(mut self, dx: f64, dy: f64, duration: f64) -> Self {
        self.steps.push(Step::MoveBy(dx, dy, duration));
        self
    }

    /// 移动到元素中心。
    pub fn move_to_ele(mut self, ele: &ChromiumElement, duration: f64) -> Self {
        self.steps.push(Step::MoveToEle(ele.clone(), duration));
        self
    }

    /// 按下左键(按住,后续移动即拖拽)。
    pub fn hold(mut self) -> Self {
        self.steps.push(Step::Down("left"));
        self
    }
    /// 松开左键。
    pub fn release(mut self) -> Self {
        self.steps.push(Step::Up("left"));
        self
    }
    /// 左键单击(在当前位置)。
    pub fn click(mut self) -> Self {
        self.steps.push(Step::Click("left", 1));
        self
    }
    /// 右键单击。
    pub fn right_click(mut self) -> Self {
        self.steps.push(Step::Click("right", 1));
        self
    }
    /// 左键双击。
    pub fn double_click(mut self) -> Self {
        self.steps.push(Step::Click("left", 2));
        self
    }
    /// 按下某键(不松开,用于组合)。键名见 [`Keys`](crate::keys::Keys)。
    pub fn key_down(mut self, key: impl Into<String>) -> Self {
        self.steps.push(Step::KeyDown(key.into()));
        self
    }
    /// 松开某键。
    pub fn key_up(mut self, key: impl Into<String>) -> Self {
        self.steps.push(Step::KeyUp(key.into()));
        self
    }
    /// 输入一段文本(直接插入)。
    pub fn type_text(mut self, text: impl Into<String>) -> Self {
        self.steps.push(Step::Type(text.into()));
        self
    }
    /// 滚轮滚动 `(dx, dy)`(在当前位置)。
    pub fn scroll(mut self, dx: f64, dy: f64) -> Self {
        self.steps.push(Step::Scroll(dx, dy));
        self
    }
    /// 停顿 `secs` 秒。
    pub fn wait(mut self, secs: f64) -> Self {
        self.steps.push(Step::Wait(secs));
        self
    }

    /// 顺序执行所有动作。
    pub async fn perform(mut self) -> Result<()> {
        let steps = std::mem::take(&mut self.steps);
        for step in steps {
            match step {
                Step::MoveTo(x, y, d) => self.human_move(x, y, d).await?,
                Step::MoveBy(dx, dy, d) => self.human_move(self.x + dx, self.y + dy, d).await?,
                Step::MoveToEle(ele, d) => {
                    let r = ele.rect().await?;
                    let (cx, cy) = (r.viewport_x + r.width / 2.0, r.viewport_y + r.height / 2.0);
                    self.human_move(cx, cy, d).await?;
                }
                Step::Down(btn) => {
                    let buttons = btn_mask(btn);
                    self.core
                        .dispatch_mouse("mousePressed", self.x, self.y, btn, buttons, 1)
                        .await?;
                    if btn == "left" {
                        self.left_held = true;
                    }
                }
                Step::Up(btn) => {
                    self.core
                        .dispatch_mouse("mouseReleased", self.x, self.y, btn, 0, 1)
                        .await?;
                    if btn == "left" {
                        self.left_held = false;
                    }
                }
                Step::Click(btn, count) => {
                    let buttons = btn_mask(btn);
                    for i in 1..=count {
                        self.core
                            .dispatch_mouse("mousePressed", self.x, self.y, btn, buttons, i)
                            .await?;
                        self.core
                            .dispatch_mouse("mouseReleased", self.x, self.y, btn, 0, i)
                            .await?;
                    }
                }
                Step::KeyDown(k) => {
                    let (key, code, vk, text) = crate::cdp::core::cdp_key_descriptor(&k);
                    self.core
                        .dispatch_key("keyDown", &key, &code, vk, &text, 0, &[])
                        .await?;
                }
                Step::KeyUp(k) => {
                    let (key, code, vk, _) = crate::cdp::core::cdp_key_descriptor(&k);
                    self.core
                        .dispatch_key("keyUp", &key, &code, vk, "", 0, &[])
                        .await?;
                }
                Step::Type(t) => self.core.insert_text(&t).await?,
                Step::Scroll(dx, dy) => {
                    self.core
                        .send(
                            "Input.dispatchMouseEvent",
                            serde_json::json!({ "type": "mouseWheel", "x": self.x, "y": self.y, "deltaX": dx, "deltaY": dy }),
                        )
                        .await?;
                }
                Step::Wait(s) => sleep(Duration::from_secs_f64(s.max(0.0))).await,
            }
        }
        Ok(())
    }

    /// 从当前点拟人移动到 `(tx, ty)`:密集采样 + 轻微抖动;按住左键时为拖拽。
    async fn human_move(&mut self, tx: f64, ty: f64, duration: f64) -> Result<()> {
        let (sx, sy) = (self.x, self.y);
        let dist = ((tx - sx).powi(2) + (ty - sy).powi(2)).sqrt();
        let dur_ms = if duration > 0.0 {
            duration * 1000.0
        } else {
            (dist * 2.0 + 120.0).clamp(120.0, 1200.0)
        };
        let n = ((dur_ms / 14.0).round() as usize).clamp(8, 120);
        let mut rng = Xorshift::new(seed_from_clock());
        let buttons = if self.left_held { 1 } else { 0 };
        for i in 1..=n {
            let t = i as f64 / n as f64;
            let ease = 3.0 * t * t - 2.0 * t * t * t; // smoothstep
            let jx = (rng.unit() - 0.5) * 1.2;
            let jy = (rng.unit() - 0.5) * 1.2;
            let x = sx + (tx - sx) * ease + jx;
            let y = sy + (ty - sy) * ease + jy;
            self.core
                .dispatch_mouse_fire("mouseMoved", x, y, "none", buttons, 0)?;
            sleep(Duration::from_millis(rng.range_ms(8, 18))).await;
        }
        // 末步精确落点(等一次往返作屏障,确保位置生效)。
        self.core
            .dispatch_mouse("mouseMoved", tx, ty, "none", buttons, 0)
            .await?;
        self.x = tx;
        self.y = ty;
        Ok(())
    }
}

/// 鼠标按钮位掩码(左=1/右=2/中=4)。
fn btn_mask(btn: &str) -> i64 {
    match btn {
        "left" => 1,
        "right" => 2,
        "middle" => 4,
        _ => 0,
    }
}
