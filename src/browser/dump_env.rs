//! 通用吐环境 —— **Camoufox 后端胶水**。后端无关的全部逻辑(探针/env.js/导出工程/同构双跑验证 +
//! canvas/webgl/audio/字体/像素/WebRTC/plugins 指纹回放 + 反 hook + 签名 sink 定位)见 [`crate::envkit`]。
//!
//! 这里只把 Camoufox `Tab` 接到 [`EnvBackend`](crate::envkit::EnvBackend)(导航前 `add_init_script` +
//! `run_js`),并把泛型类型在 `Tab` 上特化为 `EnvDumper`/`EnvProbe`。`tab.dump_env()` 见 `super::tab::Tab`。
//!
//! ```ignore
//! let mut probe = tab.dump_env()
//!     .target_query("a_bogus").match_url("aweme/v1/web/aweme/detail")
//!     .start().await?;                                // 注入探针(必须在 get 之前)
//! tab.get(url).await?;
//! let dump = probe.collect().await?;                  // 采集
//! dump.write_to("./dump-env")?;                       // 吐 seed/env.js/sinks/signers/targets
//! dump.export_project("./douyin-env", EnvScope::Full)?;
//! let report = dump.verify(&tab, "./dump-env", EnvScope::Full).await?; // 同构双跑自验证
//! ```

use std::future::Future;

use serde_json::Value;

use super::tab::Tab;
use crate::Result;
use crate::envkit::EnvBackend;

pub use crate::envkit::{EnvDump, EnvScope, EnvTarget};

/// Camoufox 后端的吐环境构建器(泛型核心在 `Tab` 上的特化)。
pub type EnvDumper = crate::envkit::EnvDumper<Tab>;
/// Camoufox 后端的吐环境会话句柄。
pub type EnvProbe = crate::envkit::EnvProbe<Tab>;

impl EnvBackend for Tab {
    fn add_init_script(&self, script: &str) -> impl Future<Output = Result<()>> {
        Tab::add_init_script(self, script)
    }
    fn run_js(&self, expr: &str) -> impl Future<Output = Result<Value>> {
        Tab::run_js(self, expr)
    }
}
