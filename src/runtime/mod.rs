//! 四层运行时再导出:避免把所有能力继续堆进巨大的 `Browser` 类。
//!
//! ```text
//! Browser Runtime   HTTP Runtime    AI Runtime
//!      CDP            reqwest           MCP
//!   Camoufox        impersonate       JSONL
//!           ╰────────────┬────────────╯
//!                    Task Runtime
//!              Pool / Profile / Checkpoint
//! ```
//!
//! 设计见 [`docs/运行时分层.md`](../../docs/运行时分层.md)。实现仍按后端分目录,
//! 本模块只做分层入口。

/// 浏览器运行时:进程、Context、Tab、元素、CDP / Camoufox。
pub mod browser {
    #[cfg(feature = "cdp")]
    pub use crate::cdp::{
        ChromiumBrowser as Browser, ChromiumBrowserContext as BrowserContext,
        ChromiumOptions as BrowserOptions, ChromiumTab as Tab, ContextSpec, PoolLease,
    };
}

/// HTTP 运行时:统一请求/响应形状;无浏览器 Session(camoufox)与 TLS 指纹。
pub mod http {
    pub use crate::net::{
        DataPacket, ListenFilter, NetRequest, NetResponse, RequestData, ResponseData,
        ResumeOptions,
    };
    #[cfg(feature = "camoufox")]
    pub use crate::session::{BrowserProfile, SessionOptions, SessionPage};
}

/// AI 运行时:页面快照、Markdown、MCP/JSONL 读页。
pub mod ai {
    pub use crate::ai_snapshot::{AiSnapshot, SnapshotRef, ai_snapshot_js, resolve_selector};
    pub use crate::html_md::{html_to_markdown, html_to_markdown_truncated, markdown_fields};
}

/// 任务运行时:池、重试、代理、断点、磁盘 Profile。
pub mod task {
    #[cfg(feature = "cdp")]
    pub use crate::cdp::{
        ChromiumPool as Pool, ChromiumPoolOptions as PoolOptions, ManagedProfile, ProfileLease,
        ProfileManager, ProfileManifest,
    };
    #[cfg(any(feature = "camoufox", feature = "cdp"))]
    pub use crate::pool::{Checkpoint, RetryPolicy, RotateStrategy};
}
