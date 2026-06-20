//! Juggler 线格式编解码。
//!
//! Juggler(Firefox 的类 CDP 协议)在 fd3/fd4 管道上的帧格式是:
//! **UTF-8 JSON 字节 + 单个 `\0` 分隔符**。既不是按行分隔,也不是长度前缀。
//!
//! ```text
//! <UTF-8 JSON bytes>\0<UTF-8 JSON bytes>\0
//! ```
//!
//! 约定:
//! - 连续的两个 `\0`(空消息)会被静默忽略。
//! - 一条消息可能跨多次 `read`,需要累积缓冲直到出现 `\0`。
//! - 不做最大消息长度限制(与 Firefox 端实现一致)。

/// 帧分隔符。
pub const DELIMITER: u8 = 0;

/// 把一段 JSON 字节编码成可直接写入管道的一帧(追加 `\0`)。
pub fn encode_frame(json: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(json.len() + 1);
    out.extend_from_slice(json);
    out.push(DELIMITER);
    out
}

/// 增量帧解码器:不断 `push` 收到的原始字节,再用 `next_frame` 取出完整消息。
///
/// 内部维护一个累积缓冲区,只在出现 `\0` 时切出一帧;空帧自动跳过。
#[derive(Debug, Default)]
pub struct FrameDecoder {
    buf: Vec<u8>,
}

impl FrameDecoder {
    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }

    /// 追加一段从管道读到的原始字节。
    pub fn push(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }

    /// 取出下一条完整消息(不含分隔符)。若当前缓冲中没有完整帧,返回 `None`。
    ///
    /// 会自动跳过连续 `\0` 产生的空帧。
    pub fn next_frame(&mut self) -> Option<Vec<u8>> {
        loop {
            let pos = self.buf.iter().position(|&b| b == DELIMITER)?;
            // 切出 [0, pos) 作为一帧,并把分隔符之后的内容前移。
            let frame: Vec<u8> = self.buf.drain(..=pos).take(pos).collect();
            if frame.is_empty() {
                // 空帧(连续 \0)直接跳过,继续看后面有没有真正的帧。
                continue;
            }
            return Some(frame);
        }
    }

    /// 当前缓冲区中尚未成帧的字节数。
    pub fn pending_len(&self) -> usize {
        self.buf.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_appends_nul() {
        assert_eq!(encode_frame(b"{}"), vec![b'{', b'}', 0]);
    }

    #[test]
    fn decode_single_frame() {
        let mut d = FrameDecoder::new();
        d.push(b"{\"id\":1}\0");
        assert_eq!(d.next_frame().unwrap(), b"{\"id\":1}");
        assert!(d.next_frame().is_none());
    }

    #[test]
    fn decode_multiple_frames_in_one_push() {
        let mut d = FrameDecoder::new();
        d.push(b"abc\0def\0");
        assert_eq!(d.next_frame().unwrap(), b"abc");
        assert_eq!(d.next_frame().unwrap(), b"def");
        assert!(d.next_frame().is_none());
    }

    #[test]
    fn decode_frame_spanning_multiple_pushes() {
        let mut d = FrameDecoder::new();
        d.push(b"{\"par");
        assert!(d.next_frame().is_none());
        d.push(b"t\":true}\0rest");
        assert_eq!(d.next_frame().unwrap(), b"{\"part\":true}");
        assert!(d.next_frame().is_none());
        assert_eq!(d.pending_len(), 4); // "rest"
    }

    #[test]
    fn empty_frames_are_skipped() {
        let mut d = FrameDecoder::new();
        d.push(b"\0\0x\0\0");
        assert_eq!(d.next_frame().unwrap(), b"x");
        assert!(d.next_frame().is_none());
    }

    #[test]
    fn roundtrip() {
        let mut d = FrameDecoder::new();
        let f1 = encode_frame(b"hello");
        let f2 = encode_frame(b"world");
        d.push(&f1);
        d.push(&f2);
        assert_eq!(d.next_frame().unwrap(), b"hello");
        assert_eq!(d.next_frame().unwrap(), b"world");
    }
}
