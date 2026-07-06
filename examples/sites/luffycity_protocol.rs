//! 站点实战:【路飞学城 luffycity】polyv 加密课程视频 —— **纯协议、零浏览器**下载为 mp4。
//!
//! 与同目录 `luffycity_capture` / `luffycity_download` 互补:那两例靠真实 Chrome(让 hls.js 在浏览器里
//! 解密、再用 MSE hook 截获明文);**本例完全不开浏览器、不跑任何 JS/WASM**,而是直接逆向 polyv 的 DRM
//! (本视频是 v11)在 Rust 里纯算解密。链路全程只有标准 HTTP + 标准 crypto(MD5 / AES-128-CBC):
//!
//!   0) GET luffycity API            → auth_info(vid / token)
//!   1) GET player.polyv.net/secure/<vid>.json(hex 密文)
//!        key/iv = MD5(vid) 的 hex 前/后 16 字节 → AES-128-CBC → base64 解码 → 播放器配置 JSON
//!        (含 seed_const 与 hls 清单)
//!   2) GET <hls>.pdx(加密 m3u8 `{version,body:base64}`)
//!        key = MD5(固定常量 + seed_const) 的 hex[1..17] → AES-128-CBC → 真实 m3u8(#EXT-X-KEY URI/IV + .ts)
//!   3) GET playsafe `/v1104/...key`(32B 包裹密钥)
//!        v11 解包:MD5 加盐 + 凯撒位移 + 两次定长置换 + AES-128-CBC → 真实 16B AES key
//!   4) 并发下载每个 .ts → AES-128-CBC(key, IV) 解密 → 顺序拼接 → ffmpeg remux 成 mp4
//!
//! polyv v11 算法逆向参考 DevLARLEY/PolyVGet(C#),本例移植其 v11 路径到 Rust。v11 分片仅标准 AES;
//! 头部加密(DecryptHeader)与 H.264「mars」反混淆是 v12/v13 才有的,这里用不到。
//!
//! 运行(前置:本机有 ffmpeg;无 ffmpeg 时保留可直接播放的 .ts):
//!   cargo run --example luffycity_protocol                       # 默认 section 35167
//!   cargo run --example luffycity_protocol -- 35167              # 指定 section id
//!   QUALITY=0 cargo run --example luffycity_protocol -- 35167    # 指定清晰度档(默认最高档)
//!
//! 说明:示例针对免费试看小节(无需登录)。付费小节需在 API 请求带上已登录 Cookie 才能拿到 token。

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use aes::Aes128;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use cbc::cipher::{BlockDecryptMut, KeyIvInit, block_padding::Pkcs7};
use drission::{Error, Result};
use futures_util::stream::{self, StreamExt};
use md5::{Digest, Md5};
use serde_json::Value;

type Aes128CbcDec = cbc::Decryptor<Aes128>;

const UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/149.0.0.0 Safari/537.36";

// ── polyv v11 解密常量(逆向自 PolyVGet)──────────────────────────────────────
/// .pdx(加密 m3u8)解密:派生 AES key 用的固定常量(v11/v12)。
const HLS_CONST_V1112: &str = "NTQ1ZjhmY2QtMzk3OS00NWZhLTkxNjktYzk3NTlhNDNhNTQ4#";
/// .pdx 解密用的固定 IV(v11/v12)。
const HLS_IV_V1112: [u8; 16] = [1, 1, 2, 3, 5, 8, 13, 21, 34, 21, 13, 8, 5, 3, 2, 1];
/// v11 key 解包的 MD5 盐。
const MD5_SALT_V11: &str = "FgzVfucSJUWkSIPYgiua";
/// v11 key 解包内层 AES 的固定 IV。
const KEY_IV_V11: [u8; 16] = [1, 2, 3, 5, 7, 11, 13, 17, 19, 23, 29, 7, 5, 3, 2, 1];
/// v11 token 字符替换表(密文字符 → 明文字符)。
const CIPHER_CHARS: &[u8] = b"lpmkenjibhuvgycftxdrzsoawq0126783459";
const PLAIN_CHARS: &[u8] = b"abcdofghijklnmepqrstuvwxyz0123456789";
/// 32 字节 token 哈希的定长置换索引。
const HASH_IDX: [usize; 32] = [
    0, 6, 12, 18, 24, 30, 1, 5, 7, 11, 13, 17, 19, 23, 25, 29, 31, 2, 4, 8, 10, 14, 16, 20, 22, 26,
    28, 3, 9, 15, 21, 27,
];
/// 16 字节解密 key 的定长置换索引。
const KEY_IDX: [usize; 16] = [0, 4, 8, 12, 1, 5, 9, 13, 2, 6, 10, 14, 3, 7, 11, 15];

fn md5_hex(data: &[u8]) -> String {
    let mut h = Md5::new();
    h.update(data);
    hex::encode(h.finalize())
}

/// AES-128-CBC + PKCS7 解密。
fn aes_cbc_dec(key: &[u8], iv: &[u8], data: &[u8]) -> Result<Vec<u8>> {
    let dec = Aes128CbcDec::new_from_slices(key, iv)
        .map_err(|e| Error::msg(format!("AES 初始化失败: {e}")))?;
    dec.decrypt_padded_vec_mut::<Pkcs7>(data)
        .map_err(|e| Error::msg(format!("AES 解密(去填充)失败: {e}")))
}

/// 凯撒位移:对每个字母按 mh 在其大小写区间内循环位移(逆向自 PolyVGet.CaesarShift)。
fn caesar_shift(input: &[u8], mh: i64) -> Vec<u8> {
    input
        .iter()
        .map(|&b| {
            let base: i64 = if !(65..=90).contains(&b) { 97 } else { 65 };
            let shifted = (((mh + b as i64 - base) % 26) + 26) % 26;
            (base + shifted) as u8
        })
        .collect()
}

/// token 字符替换(查表替换;表外字符原样保留)。
fn unshuffle_token(s: &str) -> String {
    s.bytes()
        .map(|c| match CIPHER_CHARS.iter().position(|&x| x == c) {
            Some(i) => PLAIN_CHARS[i] as char,
            None => c as char,
        })
        .collect()
}

fn permute(buf: &[u8], idx: &[usize]) -> Vec<u8> {
    idx.iter().map(|&i| buf[i]).collect()
}

/// v11 playsafe key 解包:32B 包裹密钥 → 真实 16B AES key。
fn decrypt_key_v11(key32: &[u8], mh: i64, token_id: &str) -> Result<Vec<u8>> {
    let mut salt_mh = MD5_SALT_V11.as_bytes().to_vec();
    salt_mh.extend_from_slice(mh.to_string().as_bytes());
    let shifted = caesar_shift(md5_hex(&salt_mh).as_bytes(), mh);

    let token_hash = md5_hex(unshuffle_token(token_id).as_bytes());
    let un_token_hash = permute(token_hash.as_bytes(), &HASH_IDX);

    let mut buf = MD5_SALT_V11.as_bytes().to_vec();
    buf.extend_from_slice(&shifted);
    buf.extend_from_slice(&un_token_hash);
    let key_hash = md5_hex(&buf);

    let dec = aes_cbc_dec(&key_hash.as_bytes()[7..23], &KEY_IV_V11, key32)?;
    Ok(permute(&dec, &KEY_IDX))
}

/// 生成 polyv 客户端 pid:`<毫秒时间戳>X<随机数>`。
fn gen_pid() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let rand = 1_000_000 + (now.subsec_nanos() as u64 % 1_000_000);
    format!("{}X{}", now.as_millis(), rand)
}

/// 在 URL 上设置/覆盖若干 query 参数(已存在的同名键被替换)。
fn set_query(base: &str, params: &[(&str, &str)]) -> Result<String> {
    let mut url =
        reqwest::Url::parse(base).map_err(|e| Error::msg(format!("URL 解析失败: {e}")))?;
    let keys: Vec<&str> = params.iter().map(|(k, _)| *k).collect();
    let kept: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(k, _)| !keys.contains(&k.as_ref()))
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    {
        let mut qp = url.query_pairs_mut();
        qp.clear();
        for (k, v) in &kept {
            qp.append_pair(k, v);
        }
        for (k, v) in params {
            qp.append_pair(k, v);
        }
    }
    Ok(url.to_string())
}

fn save(dir: &Path, name: &str, bytes: &[u8]) -> Result<()> {
    std::fs::write(dir.join(name), bytes)?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().with_env_filter("warn").init();

    let section = std::env::args().nth(1).unwrap_or_else(|| "35167".into());
    let quality_env = std::env::var("QUALITY")
        .ok()
        .and_then(|s| s.parse::<usize>().ok());

    let out = std::env::current_dir()?.join("captures").join("luffycity");
    let work = out.join(format!("protocol_{section}"));
    std::fs::create_dir_all(&work)?;
    println!("课程小节   : section {section}");
    println!("产物目录   : {}", out.display());
    println!("(纯协议·零浏览器:逆 polyv v11 DRM,标准 MD5/AES 解密)\n");

    let client = reqwest::Client::builder()
        .user_agent(UA)
        .build()
        .map_err(Error::from)?;
    let headers = |req: reqwest::RequestBuilder| {
        req.header("Referer", "https://www.luffycity.com/")
            .header("Origin", "https://www.luffycity.com")
    };
    let pid = gen_pid();

    // ── [0] luffycity API → auth_info(vid / token)────────────────────────────
    let play_api = format!("https://api.luffycity.com/api/v1/play/{section}/?play_id={section}");
    let api: Value = headers(client.get(&play_api)).send().await?.json().await?;
    let auth = &api["data"]["auth_info"];
    let vid = auth["vid"]
        .as_str()
        .ok_or_else(|| Error::msg("API 未返回 auth_info.vid(可能该小节需登录)"))?
        .to_string();
    let token = auth["token"]
        .as_str()
        .ok_or_else(|| Error::msg("API 未返回 auth_info.token"))?
        .to_string();
    let lesson = api["data"]["name"].as_str().unwrap_or("");
    println!("[0] auth    : vid={vid}");
    println!("    token   : {token}");
    println!("    小节     : {lesson}");

    // ── [1] secure JSON → 播放器配置 ────────────────────────────────────────────
    let secure_url = format!("https://player.polyv.net/secure/{vid}.json");
    let secure: Value = headers(client.get(&secure_url))
        .send()
        .await?
        .json()
        .await?;
    let body_hex = secure["body"]
        .as_str()
        .ok_or_else(|| Error::msg("secure 无 body"))?;
    let uri_hash = md5_hex(vid.as_bytes());
    let enc = hex::decode(body_hex).map_err(|e| Error::msg(format!("secure body 非 hex: {e}")))?;
    let dec_b64 = aes_cbc_dec(uri_hash[..16].as_bytes(), uri_hash[16..32].as_bytes(), &enc)?;
    let json_bytes = B64
        .decode(String::from_utf8_lossy(&dec_b64).trim())
        .map_err(|e| Error::msg(format!("secure 内层非 base64: {e}")))?;
    let vj: Value = serde_json::from_slice(&json_bytes)?;
    save(&work, "video_config.json", &json_bytes)?;

    let seed = vj["seed"].as_i64().unwrap_or(0);
    if seed == 0 {
        return Err(Error::msg(
            "该视频未加密(seed=0),直接取 mp4 即可,本例只处理 HLS 加密",
        ));
    }
    if !vj["hlsPrivate"].is_null() {
        return Err(Error::msg(format!(
            "该视频是 v{} (hlsPrivate={}),本例只实现 v11;v12/v13 还需头部解密+mars 反混淆",
            vj["hlsPrivate"].as_i64().unwrap_or(0) + 11,
            vj["hlsPrivate"]
        )));
    }
    let seed_const = vj["seed_const"]
        .as_i64()
        .ok_or_else(|| Error::msg("配置无 seed_const"))?;
    let title = vj["title"].as_str().unwrap_or(lesson);
    let hls302 = vj["hls302"].as_str().unwrap_or("");
    let hls_list = if hls302 == "1" {
        vj.get("hls2pc")
            .and_then(Value::as_array)
            .or_else(|| vj.get("hls2").and_then(Value::as_array))
    } else {
        vj.get("hls").and_then(Value::as_array)
    }
    .ok_or_else(|| Error::msg("配置无 hls 清单"))?;
    if hls_list.is_empty() {
        return Err(Error::msg("hls 清单为空"));
    }
    let qi = quality_env
        .unwrap_or(hls_list.len() - 1)
        .min(hls_list.len() - 1);
    let manifest_url = hls_list[qi]
        .as_str()
        .ok_or_else(|| Error::msg("hls 清单项非字符串"))?;
    println!(
        "\n[1] 配置    : 《{title}》 seed_const={seed_const} 清晰度档={}/{}",
        qi + 1,
        hls_list.len()
    );

    // ── [2] .pdx → 真实 m3u8 ────────────────────────────────────────────────────
    let pdx_key = md5_hex(format!("{HLS_CONST_V1112}{seed_const}").as_bytes());
    let pdx_url = set_query(
        &manifest_url.replace(".m3u8", ".pdx"),
        &[("pid", &pid), ("device", "desktop"), ("token", &token)],
    )?;
    let pdx: Value = headers(client.get(&pdx_url)).send().await?.json().await?;
    let pdx_ct = B64
        .decode(
            pdx["body"]
                .as_str()
                .ok_or_else(|| Error::msg(".pdx 无 body"))?,
        )
        .map_err(|e| Error::msg(format!(".pdx body 非 base64: {e}")))?;
    let m3u8 = aes_cbc_dec(pdx_key[1..17].as_bytes(), &HLS_IV_V1112, &pdx_ct)?;
    let m3u8 = String::from_utf8_lossy(&m3u8).into_owned();
    save(&work, "playlist.m3u8", m3u8.as_bytes())?;

    let mut key_uri = None;
    let mut iv: Option<Vec<u8>> = None;
    let mut ts_urls: Vec<String> = Vec::new();
    for line in m3u8.lines() {
        if let Some(rest) = line.strip_prefix("#EXT-X-KEY:") {
            for attr in rest.split(',') {
                if let Some(u) = attr.strip_prefix("URI=") {
                    key_uri = Some(u.trim_matches('"').to_string());
                } else if let Some(v) = attr
                    .strip_prefix("IV=0x")
                    .or_else(|| attr.strip_prefix("IV=0X"))
                {
                    iv = hex::decode(v.trim()).ok();
                }
            }
        } else if line.starts_with("http") {
            ts_urls.push(line.trim().to_string());
        }
    }
    let key_uri = key_uri.ok_or_else(|| Error::msg("m3u8 无 #EXT-X-KEY URI"))?;
    let iv = iv.ok_or_else(|| Error::msg("m3u8 无 IV"))?;
    println!(
        "[2] m3u8    : {} 个分片  IV={}",
        ts_urls.len(),
        hex::encode(&iv)
    );

    // ── [3] playsafe key(32B)→ 真 16B key ─────────────────────────────────────
    let mut ku = reqwest::Url::parse(&key_uri).map_err(|e| Error::msg(format!("key URI: {e}")))?;
    ku.set_path(&format!("/playsafe/v1104{}", ku.path()));
    let key_url = set_query(ku.as_str(), &[("pid", &pid), ("token", &token)])?;
    let key32 = headers(client.get(&key_url)).send().await?.bytes().await?;
    if key32.len() != 32 {
        return Err(Error::msg(format!(
            "playsafe key 长度 {} != 32(可能 token 过期或非 v11)",
            key32.len()
        )));
    }
    let token_id = token
        .rsplit('-')
        .next()
        .unwrap_or("")
        .get(1..)
        .unwrap_or("");
    let real_key = decrypt_key_v11(&key32, seed_const, token_id)?;
    save(&work, "key.bin", &real_key)?;
    println!(
        "[3] key     : 32B 包裹 → 真 16B key {}",
        hex::encode(&real_key)
    );

    // ── [4] 下载 + 解密所有 .ts(并发),顺序拼接 ─────────────────────────────────
    println!("\n[4] 下载解密 {} 个分片(并发 16)...", ts_urls.len());
    let total = ts_urls.len();
    let mut frags: Vec<(usize, Vec<u8>)> =
        stream::iter(ts_urls.into_iter().enumerate().map(|(i, u)| {
            let client = client.clone();
            let key = real_key.clone();
            let iv = iv.clone();
            async move {
                let enc = headers(client.get(&u)).send().await?.bytes().await?;
                let dec = aes_cbc_dec(&key, &iv, &enc)?;
                Ok::<(usize, Vec<u8>), Error>((i, dec))
            }
        }))
        .buffer_unordered(16)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>>>()?;
    frags.sort_by_key(|(i, _)| *i);

    let merged_ts = out.join(format!("{section}.protocol.ts"));
    {
        use std::io::Write;
        let mut f = std::fs::File::create(&merged_ts)?;
        for (_, d) in &frags {
            f.write_all(d)?;
        }
    }
    let ts_bytes: u64 = std::fs::metadata(&merged_ts).map(|m| m.len()).unwrap_or(0);
    println!(
        "    拼接     : {}/{} 分片 → {} ({:.2} MB)",
        frags.len(),
        total,
        merged_ts.display(),
        ts_bytes as f64 / 1e6
    );

    // ── ffmpeg remux 成 mp4(分片是原始 TS,-c copy 即可;无 ffmpeg 则保留 .ts)──────
    let final_mp4 = out.join(format!("{section}.protocol.mp4"));
    let ff = Command::new("ffmpeg")
        .args([
            "-y",
            "-hide_banner",
            "-loglevel",
            "warning",
            "-i",
            &merged_ts.to_string_lossy(),
            "-c",
            "copy",
            "-movflags",
            "+faststart",
            &final_mp4.to_string_lossy(),
        ])
        .status();

    println!("\n================= 下载结果(纯协议·零浏览器)=================");
    match ff {
        Ok(s) if s.success() && final_mp4.exists() => {
            let sz = std::fs::metadata(&final_mp4).map(|m| m.len()).unwrap_or(0);
            println!(
                "最终视频   : ✅ {} ({:.2} MB)",
                final_mp4.display(),
                sz as f64 / 1e6
            );
            let _ = std::fs::remove_file(&merged_ts);
            report_probe(&final_mp4);
        }
        _ => {
            println!(
                "最终视频   : ✅ {}(未找到 ffmpeg,保留可直接播放的 TS)",
                merged_ts.display()
            );
        }
    }
    println!(
        "中间产物   : {}/(video_config.json / playlist.m3u8 / key.bin)",
        work.display()
    );
    println!(
        "说明       : 全程无 Chrome、无 JS 引擎、无 WASM —— 直接逆 polyv v11 的 MD5/AES 链路解密。"
    );
    println!("===========================================================");
    Ok(())
}

/// 若本机有 ffprobe,打印时长/分辨率核对完整性。
fn report_probe(path: &PathBuf) {
    if let Ok(o) = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-show_entries",
            "stream=codec_type,codec_name,width,height",
            "-of",
            "default=noprint_wrappers=1",
            &path.to_string_lossy(),
        ])
        .output()
        && o.status.success()
    {
        for ln in String::from_utf8_lossy(&o.stdout).lines() {
            println!("            {ln}");
        }
    }
}
