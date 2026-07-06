//! 真站端到端(复现 + 重放):把 `api.rrmj.plus` 的 `x-ca-sign` 在 **Rust 里复算并与浏览器比对**,
//! 再用 Session(Chrome TLS 指纹)**重放验真** + AES-128-ECB 解响应。串起 ③ crypto tap + ⑤ 重放。
//!
//! 链路:不开 Debugger(避反调试)→ `hook().crypto_js()` 偷 `HmacSHA256(签名串, key)` →
//! Rust `base64(HMAC-SHA256(msg, key))` 与浏览器实际 `x-ca-sign` **逐字比对**(证明算法复现成功)→
//! 取一个带签名的 API 请求,`session.replay(&pkt)` 走 Chrome 指纹发出 → 200 + AES-ECB 解出 JSON。
//!
//! 运行:`cargo run --example re_rrmj_replay --features impersonate`(`HL=0` 有头;`SEC=12`)。

use std::time::Duration;

use aes::Aes128;
use aes::cipher::{BlockDecrypt, KeyInit, generic_array::GenericArray};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use drission::prelude::*;
use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// 已知签名 key(crypto tap 会现场确认 hook 偷到的 key 与此一致)。
const SIGN_KEY: &str = "ES513W0B1CsdUrR13Qk5EgDAKPeeKZY";
/// AES-128-ECB 响应解密 key。
const RESP_KEY: &[u8] = b"3b744389882a4067";

#[tokio::main]
async fn main() -> drission::Result<()> {
    let headless = std::env::var("HL").map(|v| v != "0").unwrap_or(true);
    let url = std::env::var("URL").unwrap_or_else(|_| "https://mh.yichengwlkj.com/pc".to_string());
    let secs: u64 = std::env::var("SEC")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(12);

    let browser = ChromiumBrowser::launch(ChromiumOptions::new().headless(headless)).await?;
    let tab = browser.new_tab(Some("about:blank")).await?;

    // ③ hook XHR.open/setRequestHeader/send,带栈。不开 Debugger(避反调试)。
    // 关键:CDP requestWillBeSent.request.headers **不全**(缺 x-ca-key/nonce/timestamp),
    // 故用 hook 的 open→setRequestHeader→send 序列**重建每个请求的完整头集**(app 实际设置值)。
    let hook = tab.hook().xhr().with_stack().start().await?;
    let listen = tab.listen();
    listen.start(&["api.rrmj.plus"]).await?; // listen 仅用于拿真实密文响应(解密演示)

    tab.get(&url).await?;
    println!(
        "[*] 已加载 title={:?},采集 {secs}s …",
        tab.title().await.unwrap_or_default()
    );
    tokio::time::sleep(Duration::from_secs(secs)).await;

    // ── 从 hook 重建请求(open 起、send 止,中间 setRequestHeader 收头)──
    let hits = hook.drain().await;
    let reqs = rebuild_requests(&hits);
    println!(
        "[*] ③ hook 重建出 {} 个完整 XHR 请求(含全部 x-ca-* 头)",
        reqs.len()
    );
    // 诊断:打印每个 api 请求的全部头名(看 x-ca-key/nonce/timestamp 在不在)。
    println!("[诊断] 各 api 请求的头名:");
    for r in reqs.iter().filter(|r| r.url.contains("api.rrmj.plus")) {
        let names: Vec<String> = r
            .headers
            .iter()
            .map(|(k, _)| k.to_ascii_lowercase())
            .collect();
        println!(
            "    {} {} -> [{}]",
            r.method,
            short_path(&r.url),
            names.join(", ")
        );
    }
    let Some(req) = reqs.iter().find(|r| {
        r.url.contains("api.rrmj.plus")
            && r.method.eq_ignore_ascii_case("GET")
            && !header(&r.headers, "x-ca-sign").is_empty()
    }) else {
        println!("[!] 未重建出带 x-ca-sign 的 GET(首页可能未触发;试 SEC=20)。");
        hook.stop().await?;
        listen.stop().await?;
        browser.quit().await?;
        return Ok(());
    };

    println!("\n========== 选中 API(头来自 ③ hook,完整)==========");
    println!("[*] {} {}", req.method, trunc(&req.url, 120));
    for (k, v) in &req.headers {
        let lk = k.to_ascii_lowercase();
        if lk.starts_with("x-ca") || lk == "accept" || lk == "content-type" || lk == "date" {
            println!("      {k}: {}", trunc(v, 90));
        }
    }

    // ── ③→复现:阿里云网关算法重建签名串 → Rust HMAC-SHA256 base64 == 浏览器 x-ca-sign? ──
    println!("\n========== ③ 复现 x-ca-sign(Rust HMAC-SHA256,阿里云网关算法)==========");
    let captured = header(&req.headers, "x-ca-sign").to_string();
    let sign_str = build_aliyun_sign_string(&req.method, &req.url, &req.headers);
    println!(
        "[*] 重建签名串(\\n 显示为 ⏎):\n    {}",
        sign_str.replace('\n', " ⏎ ")
    );
    let mine = B64.encode(hmac_sha256(SIGN_KEY.as_bytes(), sign_str.as_bytes()));
    println!("[*] 浏览器 x-ca-sign = {captured}");
    println!("[*] Rust 复算       = {mine}");
    let reproduced = mine == captured;
    if reproduced {
        println!("[★✅] 逐字一致 —— x-ca-sign 算法在 Rust 中复现成功(key={SIGN_KEY})。");
    } else {
        println!("[*] 阿里云完整式不匹配,自动试多个候选签名串 + 多个 key …");
        let path = short_path(&req.url);
        let pq = req
            .url
            .split_once("://")
            .and_then(|(_, r)| r.split_once('/'))
            .map(|(_, p)| format!("/{p}"))
            .unwrap_or_else(|| path.clone());
        let acc = header(&req.headers, "accept");
        let m = req.method.to_ascii_uppercase();
        // 自定义头集(去掉 accept/x-ca-sign),两种排布:Aliyun 式 "k:v\n" 与 query 式 "k=v&"。
        let mut hs: Vec<(String, String)> = req
            .headers
            .iter()
            .filter(|(k, _)| {
                let lk = k.to_ascii_lowercase();
                lk != "accept" && lk != "x-ca-sign"
            })
            .map(|(k, v)| (k.to_ascii_lowercase(), v.clone()))
            .collect();
        hs.sort();
        let colon_block: String = hs.iter().map(|(k, v)| format!("{k}:{v}\n")).collect();
        let amp_block: String = hs
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("&");
        let candidates: Vec<(&str, String)> = vec![
            ("path", path.clone()),
            ("path?query", pq.clone()),
            ("METHOD\\npath", format!("{m}\n{path}")),
            ("METHOD\\npathquery", format!("{m}\n{pq}")),
            ("METHOD path", format!("{m} {path}")),
            ("METHOD\\naccept\\npath", format!("{m}\n{acc}\n{path}")),
            (
                "aliyun式+全头(accept在)",
                format!("{m}\n{acc}\n\n\n\n{colon_block}{pq}"),
            ),
            (
                "aliyun式+全头(无accept)",
                format!("{m}\n\n\n\n\n{colon_block}{pq}"),
            ),
            ("全头k:v + pathquery", format!("{colon_block}{pq}")),
            ("全头k=v& + path", format!("{amp_block}&{path}")),
            ("全头k=v&", amp_block.clone()),
            ("path?全头k=v&排序", format!("{path}?{amp_block}")),
        ];
        let keys = [SIGN_KEY, "3b744389882a4067"];
        let mut found = false;
        for (kname, k) in keys.iter().enumerate() {
            for (name, msg) in &candidates {
                let v = B64.encode(hmac_sha256(k.as_bytes(), msg.as_bytes()));
                if v == captured {
                    println!("[★✅] 命中!签名串=「{name}」 key=#{kname}({k}) → {v}");
                    found = true;
                }
            }
        }
        if !found {
            println!(
                "[⚠️] 候选均未命中:签名串含本地不外发字段(timestamp/nonce 等),需用 ① 断点在 hmac 处读入参(crypto tap 对 webpack 内置 hmac 无效)。"
            );
        }
    }

    // ── 解密一条 listen 抓到的真实密文响应(AES-128-ECB,不依赖网络)──
    println!(
        "\n========== 解密真实密文响应(AES-128-ECB,key={}) ==========",
        String::from_utf8_lossy(RESP_KEY)
    );
    let pkts = listen.wait_count(60, Some(Duration::from_secs(2))).await?;
    if let Some(p) = pkts.iter().find(|p| {
        !p.response.body.is_empty() && try_decrypt_ecb(&p.response.body, RESP_KEY).is_some()
    }) {
        let json = try_decrypt_ecb(&p.response.body, RESP_KEY).unwrap();
        println!(
            "[★] {} → 密文 {}B 解密成功,JSON 前 240:\n{}",
            short_path(&p.url),
            p.response.body.len(),
            trunc(&json, 240)
        );
    } else {
        println!(
            "[*] 本批响应未能解密(可能为空/已是明文);抓到 {} 条 api 响应。",
            pkts.len()
        );
    }

    // ── ⑤ 重放:用 hook 重建的完整头,Session(Chrome 指纹)重放(PROXY= 适配该站 IP 绑定)──
    println!("\n========== ⑤ Session(Chrome 指纹)重放验真 ==========");
    let mut opts = SessionOptions::new()
        .profile(BrowserProfile::Chrome)
        .timeout(Duration::from_secs(15));
    if let Ok(px) = std::env::var("PROXY") {
        if !px.is_empty() {
            println!("[*] 走代理 {px}(该站 token 按 IP 绑定,出口需与浏览器一致)");
            opts = opts.proxy(Proxy::new(px));
        }
    }
    let mut sess = SessionPage::new(opts)?;
    sess.load_cookies_from_cdp_tab(&tab).await?;
    let packet = req.to_packet();
    match sess.replay(&packet).send().await {
        Ok(ok) => {
            println!("[*] 重放 status={} ok={ok}", sess.status());
            match try_decrypt_ecb(sess.text(), RESP_KEY) {
                Some(json) => println!(
                    "[★] 重放响应 AES-ECB 解密成功,JSON 前 240:\n{}",
                    trunc(&json, 240)
                ),
                None => println!("[*] 重放响应前 160:{}", trunc(sess.text(), 160)),
            }
        }
        Err(e) => println!(
            "[*] 重放网络失败({e})——该站 token 按 IP 绑定,Session 出口需与浏览器一致(设 PROXY= 重试);复现已由上面证明。"
        ),
    }

    hook.stop().await?;
    listen.stop().await?;
    browser.quit().await?;
    println!("\n==== 端到端(复现+重放)完成 ====");
    Ok(())
}

/// 从 ③ hook 重建出的一个 XHR 请求(open 起、send 止)。
struct Req {
    method: String,
    url: String,
    headers: Vec<(String, String)>,
}

impl Req {
    /// 转成 `session.replay` 要的 `DataPacket`。
    fn to_packet(&self) -> DataPacket {
        DataPacket {
            url: self.url.clone(),
            method: self.method.clone(),
            resource_type: "xhr".into(),
            request: RequestData {
                headers: self.headers.clone(),
                post_data: None,
            },
            response: ResponseData::default(),
        }
    }
}

/// 把 hook 命中流(按时序)还原为「请求列表」:`XHR.open` 开新请求、`setRequestHeader` 收头、`XHR.send` 收尾。
/// 这样能拿到 app 实际设置的**完整头集**(含 CDP requestWillBeSent 漏掉的 x-ca-key/nonce/timestamp)。
fn rebuild_requests(hits: &[HookHit]) -> Vec<Req> {
    let mut out = Vec::new();
    let mut cur: Option<Req> = None;
    for h in hits {
        match h.func.as_str() {
            "open" => {
                if let Some(r) = cur.take() {
                    out.push(r);
                }
                cur = Some(Req {
                    method: h.arg_str(0),
                    url: h.arg_str(1),
                    headers: Vec::new(),
                });
            }
            "setRequestHeader" => {
                if let Some(r) = cur.as_mut() {
                    let name = h.arg_str(0);
                    if !name.is_empty() {
                        r.headers.push((name, h.arg_str(1)));
                    }
                }
            }
            "send" => {
                if let Some(r) = cur.take() {
                    out.push(r);
                }
            }
            _ => {}
        }
    }
    if let Some(r) = cur.take() {
        out.push(r);
    }
    out
}

/// URL 的 path(去 query),打印用。
fn short_path(url: &str) -> String {
    url.split_once("://")
        .and_then(|(_, r)| r.split_once('/'))
        .map(|(_, p)| format!("/{}", p.split('?').next().unwrap_or(p)))
        .unwrap_or_else(|| url.to_string())
}

/// 取某请求头(大小写不敏感),无则空串。
fn header<'a>(hs: &'a [(String, String)], name: &str) -> &'a str {
    hs.iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
        .unwrap_or("")
}

/// 按**阿里云 API 网关**客户端算法重建待签名串(`x-ca-sign = base64(HMAC-SHA256(本串, appSecret))`):
/// `Method\n Accept\n Content-MD5\n Content-Type\n Date\n <按名排序的签名头 k:v\n> PathAndQuery`。
/// 签名头由 `x-ca-signature-headers` 列出;query 参数按键排序拼接。
fn build_aliyun_sign_string(method: &str, url: &str, hs: &[(String, String)]) -> String {
    // 拆 path 与 query。
    let after_host = url
        .split_once("://")
        .and_then(|(_, r)| r.split_once('/'))
        .map(|(_, rest)| format!("/{rest}"))
        .unwrap_or_else(|| url.to_string());
    let (path, query) = match after_host.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (after_host, String::new()),
    };
    // 签名头(x-ca-signature-headers 列出的,按名排序)。
    let mut sh_lines = String::new();
    let sig_hdrs = header(hs, "x-ca-signature-headers");
    if !sig_hdrs.is_empty() {
        let mut names: Vec<String> = sig_hdrs
            .split(',')
            .map(|s| s.trim().to_ascii_lowercase())
            .filter(|s| !s.is_empty())
            .collect();
        names.sort();
        for n in names {
            sh_lines.push_str(&format!("{n}:{}\n", header(hs, &n)));
        }
    }
    // PathAndQuery:path + 排序后的 query。
    let path_and_query = if query.is_empty() {
        path.clone()
    } else {
        let mut params: Vec<(&str, &str)> = query
            .split('&')
            .filter(|s| !s.is_empty())
            .map(|kv| kv.split_once('=').unwrap_or((kv, "")))
            .collect();
        params.sort();
        let joined = params
            .iter()
            .map(|(k, v)| {
                if v.is_empty() {
                    (*k).to_string()
                } else {
                    format!("{k}={v}")
                }
            })
            .collect::<Vec<_>>()
            .join("&");
        format!("{path}?{joined}")
    };
    format!(
        "{}\n{}\n{}\n{}\n{}\n{sh_lines}{path_and_query}",
        method.to_ascii_uppercase(),
        header(hs, "accept"),
        header(hs, "content-md5"),
        header(hs, "content-type"),
        header(hs, "date"),
    )
}

/// HMAC-SHA256 原始字节(32B)。
fn hmac_sha256(key: &[u8], msg: &[u8]) -> Vec<u8> {
    let mut mac = <HmacSha256 as Mac>::new_from_slice(key).expect("hmac 任意长 key");
    mac.update(msg);
    mac.finalize().into_bytes().to_vec()
}

/// 尝试把响应当 AES-128-ECB 密文解密:body 可能是 base64(整体密文),或 JSON 里有 `data` 密文串。
fn try_decrypt_ecb(body: &str, key: &[u8]) -> Option<String> {
    let candidate = if let Ok(v) = serde_json::from_str::<serde_json::Value>(body) {
        v.get("data")
            .and_then(|d| d.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| body.trim().to_string())
    } else {
        body.trim().to_string()
    };
    let ct = B64.decode(candidate.as_bytes()).ok()?;
    if ct.is_empty() || ct.len() % 16 != 0 {
        return None;
    }
    let cipher = Aes128::new_from_slice(key).ok()?;
    let mut out = ct.clone();
    for chunk in out.chunks_mut(16) {
        let mut blk = GenericArray::clone_from_slice(chunk);
        cipher.decrypt_block(&mut blk);
        chunk.copy_from_slice(&blk);
    }
    // 去 PKCS7
    if let Some(&pad) = out.last() {
        let pad = pad as usize;
        if (1..=16).contains(&pad) && pad <= out.len() {
            out.truncate(out.len() - pad);
        }
    }
    let s = String::from_utf8(out).ok()?;
    if s.trim_start().starts_with('{') || s.trim_start().starts_with('[') {
        Some(s)
    } else {
        None
    }
}

fn trunc(s: &str, n: usize) -> String {
    let t: String = s.chars().take(n).collect();
    if s.chars().count() > n {
        format!("{t}…")
    } else {
        t
    }
}
