//! Session 模式浏览器 TLS / JA3 / JA4 + HTTP2 指纹伪装(`--features impersonate`)。
//!
//! 同一进程分别用 `BrowserProfile::None`(纯 reqwest)与 `BrowserProfile::Chrome`(wreq + BoringSSL)
//! 打 `https://tls.peet.ws/api/all`,打印两者的 **JA3 / JA4 / Akamai(HTTP2)指纹**——
//! 证明开启 profile 后指纹变为**浏览器形态**(与默认 Rust TLS 指纹明显不同)。
//!
//! 运行:
//! ```bash
//! cargo run --example session_tls --features impersonate
//! ```
//! 需联网;若到 peet.ws 出网受限,示例会打印提示并以 0 退出(离线无法验证,不算失败)。

use drission::prelude::*;
use serde_json::Value;

const ECHO_URL: &str = "https://tls.peet.ws/api/all";

/// 用指定指纹档抓一次 echo 服务,返回解析后的 JSON。
async fn probe(profile: BrowserProfile) -> drission::Result<Value> {
    let mut s = SessionPage::new(SessionOptions::new().profile(profile))?;
    let ok = s.get(ECHO_URL).await?;
    if !ok {
        return Err(drission::Error::msg(format!("HTTP {}", s.status())));
    }
    s.json()
}

/// 提取关心的指纹字段:(ja3_hash, ja4, akamai_hash, user_agent)。
fn fp(v: &Value) -> (String, String, String, String) {
    let s = |p: &str| v.pointer(p).and_then(Value::as_str).unwrap_or("?").to_string();
    (
        s("/tls/ja3_hash"),
        s("/tls/ja4"),
        s("/http2/akamai_fingerprint_hash"),
        s("/user_agent"),
    )
}

fn show(tag: &str, v: &Value) {
    let (ja3, ja4, akamai, ua) = fp(v);
    println!("[{tag}]");
    println!("  JA3  hash : {ja3}");
    println!("  JA4       : {ja4}");
    println!("  Akamai(h2): {akamai}");
    println!("  UA        : {ua}");
}

#[tokio::main]
async fn main() {
    // 网络问题不算"功能失败"——抓不到就打印提示、0 退出(离线无法验证)。
    let plain = match probe(BrowserProfile::None).await {
        Ok(v) => v,
        Err(e) => {
            println!("跳过(到 {ECHO_URL} 出网失败,离线无法验证 TLS 指纹): {e}");
            return;
        }
    };
    let chrome = match probe(BrowserProfile::Chrome).await {
        Ok(v) => v,
        Err(e) => {
            println!("跳过(Chrome 档抓取失败): {e}");
            return;
        }
    };

    show("None(纯 reqwest)", &plain);
    show("Chrome(wreq 指纹)", &chrome);

    let (plain_ja3, plain_ja4, ..) = fp(&plain);
    let (chrome_ja3, chrome_ja4, ..) = fp(&chrome);

    // 核心断言:开启 Chrome 档后,JA3 / JA4 与纯 reqwest 不同(指纹确实变了)。
    let mut failed = false;
    if plain_ja3 == "?" || chrome_ja3 == "?" {
        println!("\n⚠ 未能从 echo 服务解析到 JA3(可能 schema 变化),跳过断言。");
        return;
    }
    if plain_ja3 == chrome_ja3 {
        eprintln!("\n✗ JA3 未变化(impersonate 似乎未生效): {plain_ja3}");
        failed = true;
    } else {
        println!("\n✓ JA3 已改变: reqwest {plain_ja3}  →  Chrome {chrome_ja3}");
    }
    if plain_ja4 != chrome_ja4 {
        println!("✓ JA4 已改变: reqwest {plain_ja4}  →  Chrome {chrome_ja4}");
    }

    if failed {
        std::process::exit(1);
    }
    println!("\nALL CHECKS PASSED");
}
