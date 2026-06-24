//! CDP 无障碍快照接线:把 [`crate::a11y`] 的后端无关能力挂到 [`ChromiumTab`]。
//!
//! - [`ax_tree`](ChromiumTab::ax_tree):**CDP 原生** `Accessibility.getFullAXTree`(最准)。
//! - [`ax_snapshot`](ChromiumTab::ax_snapshot):**DOM 派生**(注入 [`AX_SNAPSHOT_JS`],跨后端一致)。
//! - [`ax_find`](ChromiumTab::ax_find):在原生树里按角色检索(便捷)。

use serde_json::json;

use super::ChromiumTab;
use crate::Result;
use crate::a11y::{self, AX_SNAPSHOT_JS, AxNode, AxTree};

impl ChromiumTab {
    /// **CDP 原生**无障碍树(`Accessibility.getFullAXTree`):角色/可见名来自 Chrome 无障碍引擎,最准确。
    ///
    /// 用于抗改版断言(按角色+名定位)或喂 LLM(语义树比整页 HTML 小一个数量级)。
    pub async fn ax_tree(&self) -> Result<AxTree> {
        // 部分 Chrome 版本需先 enable;失败忽略,直接取全树。
        let _ = self.core.send("Accessibility.enable", json!({})).await;
        let r = self
            .core
            .send("Accessibility.getFullAXTree", json!({}))
            .await?;
        Ok(a11y::build_from_cdp(&r))
    }

    /// **DOM 派生**的近似无障碍快照(注入 [`AX_SNAPSHOT_JS`] 按 ARIA 规则算):跨后端一致、
    /// 无需 `Accessibility` 域。准确度略低于 [`ax_tree`](Self::ax_tree),但任何 `run_js` 能跑处都可用。
    pub async fn ax_snapshot(&self) -> Result<AxTree> {
        let v = self.core.eval_value(AX_SNAPSHOT_JS).await?;
        Ok(a11y::build_from_snapshot(&v))
    }

    /// 便捷:在**原生**无障碍树里按角色找节点(返回克隆,避免借用整树)。
    pub async fn ax_find(&self, role: &str) -> Result<Vec<AxNode>> {
        let tree = self.ax_tree().await?;
        Ok(tree.find_by_role(role).into_iter().cloned().collect())
    }
}
