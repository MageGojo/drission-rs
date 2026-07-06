//! 站点实战:抓【路飞学城(luffycity)播放页】的视频内容。
//!
//! 目标页是 Nuxt SPA(`https://www.luffycity.com/play/<id>`),视频走 **保利威 polyv**:
//! 播放器先请求 `player.polyv.net/secure/<vid>_d.json`(响应体是**加密 hex**,前端解密),
//! 解密后再去拉真正可播的 **HLS `.m3u8`** 与分片。直接 curl 壳页 / polyv 接口都拿不到明文。
//!
//! 本例的做法 = **让真实 Chrome 跑一遍,库只负责监听网络**(不碰 polyv 的解密):
//!   1) 导航前用 CDP 原生 `Network.*` 监听(`tab.listen().start`)只盯数据类请求;
//!   2) 打开播放页,等 SPA 调课程接口 + polyv secure JSON;
//!   3) 注入 JS 触发 `<video>` 静音播放 / 点播放按钮,逼播放器解密后去拉 `.m3u8`;
//!   4) 抽干缓冲,**先落盘**(`requests.jsonl` 去重)再抽取关键内容(标题 / vid / m3u8)。
//!
//! 通用能力都在库里:`tab.listen()`(CDP 原生网络监听抓响应体)、`run_js`、`cookies`、`title`;
//! 这里只有 luffycity/polyv 的业务(盯哪些 URL、怎么触发播放)。
//!
//! 运行(默认无头;`HL=0` 有头便于排障):
//!   cargo run --example luffycity_capture            # 默认 cdp,无需额外 feature
//!   cargo run --example luffycity_capture -- https://www.luffycity.com/play/35167

use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use drission::prelude::*;
use serde_json::{Value, json};

/// 触发播放:静音播放所有 `<video>`,并点常见的播放/海报按钮(polyv H5 / 各通用播放器)。
const PLAY_JS: &str = r#"(() => {
  const acted = [];
  document.querySelectorAll('video').forEach(v => {
    try { v.muted = true; v.volume = 0; const p = v.play && v.play(); if (p && p.catch) p.catch(()=>{}); acted.push('video.play'); } catch (e) {}
  });
  const sels = ['.plv-poster','.pv-poster','.plv-controls__play','.plv-controls-play',
    '.prism-big-play-btn','.vjs-big-play-button','.plyr__control--overlaid',
    '[aria-label="播放"]','[aria-label="play"]','.play-btn','.poster','.pv-mask','.player-mask'];
  for (const s of sels) { const e = document.querySelector(s); if (e) { try { e.click(); acted.push('click:'+s); } catch (_) {} } }
  return acted.join(',') || 'none';
})()"#;

#[tokio::main]
async fn main() -> drission::Result<()> {
    tracing_subscriber::fmt().with_env_filter("warn").init();

    let url = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "https://www.luffycity.com/play/35167".into());
    let headless = !matches!(
        std::env::var("HL").ok().as_deref(),
        Some("0") | Some("false")
    );

    let out = std::env::current_dir()?.join("captures").join("luffycity");
    std::fs::create_dir_all(&out)?;
    println!("目标播放页 : {url}");
    println!("无头模式   : {headless}（HL=0 切有头）");
    println!("产物目录   : {}", out.display());

    // 反检测默认开;关站点隔离让 polyv 若用 iframe 也与主页面同进程/同会话(监听才抓得到其请求);
    // 放开自动播放 + 静音,逼播放器在无头下也去解密拉 m3u8。
    let browser = ChromiumBrowser::launch(
        ChromiumOptions::new()
            .headless(headless)
            .window_size(1280, 800)
            .add_arg("--autoplay-policy=no-user-gesture-required")
            .add_arg("--mute-audio")
            .add_arg("--disable-features=IsolateOrigins,site-per-process"),
    )
    .await?;
    let tab = browser.new_tab(None).await?;

    // 导航前开监听。盯数据类 URL:课程接口 / polyv secure / HLS 播放列表(polyv 用 .pdx 不是 .m3u8!)/
    // 鉴权 key / 分片 ts / 统计;避开 nuxt 包与 player.js。
    tab.listen()
        .start(&[
            "luffycity.com/api",
            "/api/v",
            "/secure/",
            "_d.json",
            ".pdx",
            ".m3u8",
            "playsafe",
            ".key",
            ".ts?",
            "prtas",
            "/qos",
        ])
        .await?;

    tab.get(&url).await?;
    // 等 SPA 起来、课程接口 + polyv secure JSON 发出。
    tokio::time::sleep(Duration::from_secs(6)).await;

    // 逼播放:解密后播放器才会去拉 .m3u8(真正可播的内容)。重试几次,等异步播放器挂载。
    for _ in 0..3 {
        let acted = tab.run_js(PLAY_JS).await.unwrap_or(Value::Null);
        println!("触发播放   : {acted}");
        tokio::time::sleep(Duration::from_secs(3)).await;
    }

    // 抽干缓冲(最多 300 条,总超时 15s)。
    let packets = tab
        .listen()
        .wait_count(300, Some(Duration::from_secs(15)))
        .await?;
    tab.listen().stop().await?;
    println!("\n抓到数据类请求 {} 条", packets.len());

    // ── 先落盘(save-first):全部请求去重写入 JSONL ──────────────────────────
    let jsonl = out.join("requests.jsonl");
    let mut seen = load_seen(&jsonl);
    let mut appended = 0usize;
    let mut lines = String::new();
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    for p in &packets {
        let is_ts = p.url.contains(".ts?") || p.path().ends_with(".ts");
        // ts 分片体积大且是加密二进制,只留 URL/状态不存 body;二进制(key 等)存 base64。
        let body = if is_ts { "" } else { p.response.body.as_str() };
        let fp = fingerprint(&p.method, &p.url, body);
        if !seen.insert(fp) {
            continue;
        }
        let rec = json!({
            "ts": now_ms,
            "method": p.method,
            "url": p.url,
            "type": p.resource_type,
            "status": p.response.status,
            "req_headers": pairs(&p.request.headers),
            "req_body": p.request.post_data,
            "resp_headers": pairs(&p.response.headers),
            "resp_body": body,
            // 仅对小二进制(如 16B 的 AES key)存 base64;ts 分片连 base64 也不存(避免 JSONL 爆量)。
            "resp_body_base64": if !is_ts && body.is_empty() && !p.response.body_base64.is_empty() {
                Value::String(p.response.body_base64.clone())
            } else {
                Value::Null
            },
        });
        lines.push_str(&serde_json::to_string(&rec).unwrap_or_default());
        lines.push('\n');
        appended += 1;
    }
    if !lines.is_empty() {
        append_file(&jsonl, &lines)?;
    }
    println!("落盘        : +{appended} 条 → {}", jsonl.display());

    // ── 抽取关键内容(高亮) ──────────────────────────────────────────────
    let mut polyv_secure_url = None::<String>;
    let mut polyv_vid = None::<String>;
    let mut playlist_urls: Vec<String> = Vec::new(); // polyv .pdx(= 加密 m3u8)
    let mut key_url = None::<String>; // playsafe AES key
    let mut ts_urls: Vec<String> = Vec::new(); // 视频分片
    let mut course_api: Vec<Value> = Vec::new();
    let mut lesson_name = String::new();
    let mut course_name = String::new();
    let mut auth_info = Value::Null;
    let mut course_outline = Value::Null;

    for p in &packets {
        let path = p.path();
        if p.url.contains("/secure/") && p.url.contains("_d.json") {
            polyv_secure_url.get_or_insert_with(|| p.url.clone());
            polyv_vid.get_or_insert_with(|| extract_vid(&p.url));
            let _ = std::fs::write(out.join("polyv_secure.json"), &p.response.body);
        } else if p.url.contains(".pdx") || p.url.contains(".m3u8") {
            if !playlist_urls.contains(&p.url) {
                playlist_urls.push(p.url.clone());
            }
            let n = playlist_urls.len();
            let _ = std::fs::write(out.join(format!("playlist_{n}.pdx")), &p.response.body);
        } else if p.url.contains("playsafe") || path.ends_with(".key") {
            key_url.get_or_insert_with(|| p.url.clone());
            if !p.response.body_base64.is_empty() {
                let _ = std::fs::write(out.join("playsafe.key.b64"), &p.response.body_base64);
            }
        } else if p.url.contains(".ts?") || path.ends_with(".ts") {
            ts_urls.push(p.url.clone());
        }

        // luffycity 课程接口:抽课程大纲 / 本节标题 / polyv 鉴权令牌(auth_info)。
        if (p.url.contains("luffycity.com/api") || p.url.contains("/api/v"))
            && p.method != "OPTIONS"
            && let Some(j) = p.json()
        {
            let d = &j["data"];
            if p.url.contains("/play/sections/") {
                course_outline = json!({
                    "course_name": d["name"], "course_id": d["id"],
                    "chapter_count": d["chapter_count"], "section_count": d["section_count"],
                    "total_video_time": d["video_time"], "is_buy": d["is_buy"],
                });
                course_name = d["name"].as_str().unwrap_or_default().to_string();
            } else if p.url.contains("/play/") && d["auth_info"].is_object() {
                lesson_name = d["name"].as_str().unwrap_or_default().to_string();
                if course_name.is_empty() {
                    course_name = d["course_name"].as_str().unwrap_or_default().to_string();
                }
                auth_info = d["auth_info"].clone();
            }
            course_api.push(json!({ "url": p.url, "json": j }));
        }
    }

    let page_title = tab.title().await.unwrap_or_default();
    let cookies = tab.cookies().await.unwrap_or_default();
    let cookie_str = cookies
        .iter()
        .map(|c| format!("{}={}", c.name, c.value))
        .collect::<Vec<_>>()
        .join("; ");

    let summary = json!({
        "play_url": url,
        "page_title": page_title.clone(),
        "course_name": course_name.clone(),
        "lesson_name": lesson_name.clone(),
        "course_outline": course_outline.clone(),
        "polyv_vid": polyv_vid.clone(),
        "polyv_secure_url": polyv_secure_url.clone(),
        "polyv_auth_info": auth_info.clone(),
        "playlist_urls": playlist_urls.clone(),
        "playsafe_key_url": key_url.clone(),
        "ts_segment_count": ts_urls.len(),
        "ts_sample": ts_urls.iter().take(3).collect::<Vec<_>>(),
        "course_api_urls": course_api.iter().map(|c| c["url"].clone()).collect::<Vec<_>>(),
        "captured_total": packets.len(),
        "cookie": cookie_str,
        "captured_at_ms": now_ms,
    });
    std::fs::write(
        out.join("summary.json"),
        serde_json::to_string_pretty(&summary)?,
    )?;
    std::fs::write(
        out.join("course_outline.json"),
        serde_json::to_string_pretty(&Value::Array(course_api))?,
    )?;

    // ── 控制台高亮汇报 ────────────────────────────────────────────────────
    println!("\n================= 抓取结果(高亮) =================");
    println!("页面标题   : {page_title}");
    println!("课程        : {course_name}");
    println!("本节        : {lesson_name}");
    if let Value::Object(_) = &course_outline {
        println!(
            "课程大纲   : {} 章 / {} 节,总时长 {}",
            course_outline["chapter_count"],
            course_outline["section_count"],
            course_outline["total_video_time"].as_str().unwrap_or("?")
        );
    }
    match (&polyv_vid, &polyv_secure_url) {
        (Some(vid), Some(_)) => {
            println!("polyv vid  : {vid}（secure JSON 加密体 → polyv_secure.json）")
        }
        _ => println!("polyv vid  : ❌ 未抓到 secure JSON"),
    }
    if auth_info.is_object() {
        println!(
            "鉴权令牌   : token={} sign={}",
            auth_info["token"].as_str().unwrap_or("?"),
            auth_info["sign"].as_str().unwrap_or("?")
        );
    }
    if playlist_urls.is_empty() {
        println!("播放列表   : ❌ 未抓到 .pdx/.m3u8（视频未起播）");
    } else {
        println!(
            "播放列表   : ✅ {} 个 polyv .pdx(= 加密 m3u8)",
            playlist_urls.len()
        );
        for u in playlist_urls.iter().take(3) {
            println!("            {}", u.chars().take(130).collect::<String>());
        }
    }
    match &key_url {
        Some(_) => println!("HLS 密钥   : ✅ playsafe .key 已抓(16B → playsafe.key.b64)"),
        None => println!("HLS 密钥   : ❌ 未抓到"),
    }
    println!("视频分片   : {} 个 .ts(加密,URL 已记录)", ts_urls.len());
    println!("\n产物       : summary.json / course_outline.json / playlist_*.pdx / requests.jsonl");
    println!("说明       : polyv 的 .pdx 与 .ts 仍是 polyv 加密;浏览器内 hls.js 已自解播放。");
    println!(
        "            纯监听即拿到【课程全内容 + 完整可播清单(列表/密钥/分片)】,无需自己解密。"
    );
    println!("==================================================");

    browser.quit().await?;
    Ok(())
}

/// 从 polyv secure URL 里取 vid:`.../secure/<vid>_d.json`。
fn extract_vid(url: &str) -> String {
    url.rsplit('/')
        .next()
        .and_then(|f| f.split('?').next())
        .map(|f| {
            f.trim_end_matches("_d.json")
                .trim_end_matches(".json")
                .to_string()
        })
        .unwrap_or_default()
}

/// 去重指纹:method + 规范化 URL(剔除时间戳/nonce 类易变参数) + 响应体哈希。
fn fingerprint(method: &str, url: &str, body: &str) -> u64 {
    let norm = normalize_url(url);
    let mut h = std::collections::hash_map::DefaultHasher::new();
    method.hash(&mut h);
    norm.hash(&mut h);
    body.len().hash(&mut h);
    body.hash(&mut h);
    h.finish()
}

/// 规范化 URL:去掉易变 query(ran/t/_/timestamp/ts/nonce/rand/r),稳定去重。
fn normalize_url(url: &str) -> String {
    const VOLATILE: [&str; 8] = ["ran", "t", "_", "timestamp", "ts", "nonce", "rand", "r"];
    let Some((base, q)) = url.split_once('?') else {
        return url.to_string();
    };
    let kept: Vec<&str> = q
        .split('&')
        .filter(|kv| {
            let k = kv.split('=').next().unwrap_or(kv);
            !VOLATILE.contains(&k)
        })
        .collect();
    if kept.is_empty() {
        base.to_string()
    } else {
        format!("{base}?{}", kept.join("&"))
    }
}

/// 读取已存在 JSONL 的指纹集合(跨运行去重)。
fn load_seen(path: &PathBuf) -> HashSet<u64> {
    let mut set = HashSet::new();
    if let Ok(txt) = std::fs::read_to_string(path) {
        for ln in txt.lines() {
            if let Ok(v) = serde_json::from_str::<Value>(ln) {
                let m = v["method"].as_str().unwrap_or("");
                let u = v["url"].as_str().unwrap_or("");
                let b = v["resp_body"].as_str().unwrap_or("");
                set.insert(fingerprint(m, u, b));
            }
        }
    }
    set
}

fn append_file(path: &PathBuf, data: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    f.write_all(data.as_bytes())
}

fn pairs(h: &[(String, String)]) -> Value {
    Value::Array(h.iter().map(|(k, v)| json!([k, v])).collect())
}
