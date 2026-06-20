//! 通用吐环境 —— **CDP 后端胶水**。后端无关逻辑(探针/env.js/导出工程/同构双跑验证 + 指纹回放)见
//! [`crate::envkit`]。这里把 `ChromiumTab` 接到 [`EnvBackend`](crate::envkit::EnvBackend):导航前注入用
//! `Page.addScriptToEvaluateOnNewDocument`、求值用 `Runtime.evaluate`(`run_js`),并在 `ChromiumTab` 上
//! 特化泛型类型为 [`ChromiumEnvDumper`]/[`ChromiumEnvProbe`]。`tab.dump_env()` 见 [`ChromiumTab`]。

use std::future::Future;

use serde_json::{Value, json};

use crate::Result;
use crate::cdp::ChromiumTab;
use crate::envkit::EnvBackend;

/// CDP 后端的吐环境构建器(泛型核心在 `ChromiumTab` 上的特化)。
pub type ChromiumEnvDumper = crate::envkit::EnvDumper<ChromiumTab>;
/// CDP 后端的吐环境会话句柄。
pub type ChromiumEnvProbe = crate::envkit::EnvProbe<ChromiumTab>;

impl EnvBackend for ChromiumTab {
    fn add_init_script(&self, script: &str) -> impl Future<Output = Result<()>> {
        let src = script.to_string();
        async move {
            // 导航前对每个新文档注入(CDP 原生累积);当前文档也执行一次(best-effort)。
            self.core
                .send(
                    "Page.addScriptToEvaluateOnNewDocument",
                    json!({ "source": src }),
                )
                .await?;
            let _ = ChromiumTab::run_js(self, &src).await;
            Ok(())
        }
    }

    fn run_js(&self, expr: &str) -> impl Future<Output = Result<Value>> {
        ChromiumTab::run_js(self, expr)
    }
}
