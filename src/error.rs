//! 统一错误类型。
//!
//! 整个 crate 对外只暴露 [`Error`] 与 [`Result`],方便上层用 `?` 串联。

use std::time::Duration;

/// crate 级别的 `Result` 别名。
pub type Result<T> = std::result::Result<T, Error>;

/// drission 的统一错误枚举。
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON 序列化/反序列化错误: {0}")]
    Json(#[from] serde_json::Error),

    #[error("HTTP 请求错误: {0}")]
    Http(#[from] reqwest::Error),

    #[error("解压错误: {0}")]
    Zip(#[from] zip::result::ZipError),

    /// 来自浏览器(Juggler 服务端)的协议错误,内容为 `error.message`。
    #[error("协议错误: {0}")]
    Protocol(String),

    /// 协议帧/会话路由相关的传输层错误。
    #[error("传输层错误: {0}")]
    Transport(String),

    #[error("操作超时(超过 {0:?})")]
    Timeout(Duration),

    #[error("未找到 Camoufox 浏览器: {0}")]
    BrowserNotFound(String),

    #[error("未找到元素: {0}")]
    ElementNotFound(String),

    #[error("元素已失效或脱离文档: {0}")]
    StaleElement(String),

    #[error("不支持的平台: {0}")]
    UnsupportedPlatform(String),

    #[error("功能尚未实现: {0}")]
    NotImplemented(&'static str),

    #[error("{0}")]
    Other(String),
}

impl Error {
    /// 便捷构造一个 `Other` 错误。
    pub fn msg(s: impl Into<String>) -> Self {
        Error::Other(s.into())
    }
}
