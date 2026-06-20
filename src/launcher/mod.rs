//! 浏览器启动器:选项配置 + Camoufox 自动下载分发 + 进程启动(进程启动见后续 `process`)。

pub mod fetch;
pub mod options;
pub mod process;

pub use fetch::{cache_root, ensure_camoufox, platform_tag};
pub use options::{BrowserOptions, Fingerprint, Geolocation, OsType, Proxy};
pub use process::{Launched, launch};
