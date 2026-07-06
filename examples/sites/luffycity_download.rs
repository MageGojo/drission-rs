//! 站点实战:把【路飞学城(luffycity)播放页】的 polyv 加密视频**下载成可播放 mp4**。
//!
//! 续作 `luffycity_capture`(那一例只「监听拿清单」,不解密)。polyv 的 `secure`(hex 密文)、
//! `.pdx`(加密 m3u8)、playsafe `.key`(被包裹的密钥)、`.ts`(AES-128 分片)是四层自定义 DRM,
//! **硬逆代价大且脆**。本例换思路 —— 仍然「让真实浏览器替你解,库只观测」,但把观测点下沉到
//! **MSE(Media Source Extensions)**:
//!
//!   播放器内部用 hls.js 把 `.ts` 解密 + 解复用后,会调用 `SourceBuffer.appendBuffer(明文 fMP4)`
//!   喂给 `<video>`。我们在**导航前**注入 hook 包住 `MediaSource.addSourceBuffer` /
//!   `SourceBuffer.appendBuffer`,把这些**已解密的 fMP4 分块**按轨道收集起来;再用**高倍速静音
//!   播放**把整条时间线「过」一遍逼 hls.js 把所有分片都加载(并 append)一遍;最后把每条轨道的
//!   字节拉回 Rust,交给 **ffmpeg 合并成 mp4(视频无损 copy + 音频重编码 AAC)**。
//!   全程**不碰 polyv 的任何密钥与算法**。
//!
//! 前提:本机装了 `ffmpeg`(remux 用)。导航前注入靠库的 `tab.add_init_script`
//! (底层 `Page.addScriptToEvaluateOnNewDocument`,对主页面与子帧都生效)。
//!
//! 运行(默认无头;`HL=0` 有头便于排障):
//!   cargo run --example luffycity_download
//!   cargo run --example luffycity_download -- https://www.luffycity.com/play/35167

use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use drission::envkit::EnvBackend; // CDP 的 ChromiumTab::add_init_script 经此 trait 提供(导航前注入)
use drission::prelude::*;
use serde_json::{Value, json};

/// 导航前注入:hook MSE,收集 hls.js 解密后喂给播放器的明文 fMP4 分块。
///
/// 设计要点:① 在**每个 frame**(含 polyv 可能用到的 iframe)各自 patch,本帧捕获;② 子帧把分块
/// 经 `postMessage` 中继到 **top frame** 汇总(top 也接收自身捕获)——这样无论播放器在主页面还是
/// iframe,读 top 的 `window.__mseStore` 都能拿到全部;③ 提供 `__mseInfo()` / `__mseSlice()`
/// 供 Rust 侧分片把字节(base64)拉回。幂等、异常吞掉,绝不影响页面本身播放。
const MSE_HOOK_JS: &str = r#"(function () {
  try {
    if (window.__mseHookInstalled) return;
    window.__mseHookInstalled = true;
    var IS_TOP = false;
    try { IS_TOP = (window === window.top); } catch (e) { IS_TOP = false; }
    var FRAME = IS_TOP ? 'top' : ('f' + Math.random().toString(36).slice(2));
    var store = { map: {}, order: [], log: [] };
    window.__mseStore = store;
    function tlog(m){ try { if (store.log.length < 200) store.log.push(String(m)); } catch (e) {} }
    function getTrack(key, mime){
      var t = store.map[key];
      if (!t){ t = { mime: mime || '', chunks: [], bytes: 0 }; store.map[key] = t; store.order.push(key); }
      if (mime && !t.mime) t.mime = mime;
      return t;
    }
    function toU8(d){
      try {
        if (!d) return null;
        if (d instanceof ArrayBuffer) return new Uint8Array(d);
        if (ArrayBuffer.isView(d)) return new Uint8Array(d.buffer, d.byteOffset, d.byteLength);
      } catch (e) {}
      return null;
    }
    if (IS_TOP) {
      window.addEventListener('message', function (ev){
        var d = ev && ev.data;
        if (!d || d.__mse !== 1) return;
        if (d.t === 'sb') { getTrack(d.key, d.mime); }
        else if (d.t === 'data') { var t = getTrack(d.key, d.mime); var u8 = new Uint8Array(d.buf); t.chunks.push(u8); t.bytes += u8.length; }
      }, false);
    }
    var seq = 0;
    function sbKey(sb){
      if (sb.__mseKey) return sb.__mseKey;
      var k = FRAME + ':' + (seq++);
      sb.__mseKey = k;
      if (!IS_TOP) { try { window.top.postMessage({ __mse: 1, t: 'sb', key: k, mime: sb.__mseMime || '' }, '*'); } catch (e) { tlog('sb relay ' + e); } }
      return k;
    }
    function capture(sb, data){
      var u8 = toU8(data);
      if (!u8 || u8.length === 0) return;
      var copy = new Uint8Array(u8.length); copy.set(u8);
      var key = sbKey(sb);
      if (IS_TOP) { var t = getTrack(key, sb.__mseMime); t.chunks.push(copy); t.bytes += copy.length; }
      else {
        try { window.top.postMessage({ __mse: 1, t: 'data', key: key, mime: sb.__mseMime || '', buf: copy.buffer }, '*', [copy.buffer]); }
        catch (e) { var t2 = getTrack(key, sb.__mseMime); t2.chunks.push(copy); t2.bytes += copy.length; tlog('data relay ' + e); }
      }
    }
    function patchMS(proto){
      if (!proto || proto.__msePatched) return;
      proto.__msePatched = true;
      var add = proto.addSourceBuffer;
      if (add) {
        proto.addSourceBuffer = function (mime){
          var sb = add.apply(this, arguments);
          try { sb.__mseMime = String(mime); sbKey(sb); tlog('addSB ' + mime); } catch (e) { tlog('addSB ' + e); }
          return sb;
        };
      }
    }
    try { patchMS(window.MediaSource && MediaSource.prototype); } catch (e) {}
    try { if (window.ManagedMediaSource) patchMS(ManagedMediaSource.prototype); } catch (e) {}
    try {
      if (window.SourceBuffer && !SourceBuffer.prototype.__mseAppendPatched) {
        SourceBuffer.prototype.__mseAppendPatched = true;
        var ap = SourceBuffer.prototype.appendBuffer;
        SourceBuffer.prototype.appendBuffer = function (data){ try { capture(this, data); } catch (e) { tlog('cap ' + e); } return ap.apply(this, arguments); };
      }
    } catch (e) { tlog('patch sb ' + e); }
    // 穿透 shadow DOM 找所有 <video>(polyv H5 播放器把 video 藏在 shadowRoot 里,
    // 普通 querySelectorAll 够不到 → 否则高倍速驱动/时长判定都失效)。
    window.__videos = function (){
      var acc = [];
      (function dig(r){
        try {
          var vs = r.querySelectorAll('video');
          for (var i = 0; i < vs.length; i++) acc.push(vs[i]);
          var all = r.querySelectorAll('*');
          for (var j = 0; j < all.length; j++) { if (all[j].shadowRoot) dig(all[j].shadowRoot); }
        } catch (e) {}
      })(document);
      return acc;
    };
    window.__mseInfo = function (){
      return JSON.stringify({
        frame: FRAME,
        tracks: store.order.map(function (k){ var t = store.map[k]; return { key: k, mime: t.mime, bytes: t.bytes, chunks: t.chunks.length }; }),
        log: store.log
      });
    };
    window.__mseSlice = function (i, start, len){
      var k = store.order[i]; if (!k) return '';
      var t = store.map[k]; if (!t) return '';
      var end = Math.min(start + len, t.bytes);
      if (start >= end) return '';
      var out = new Uint8Array(end - start);
      var pos = 0, oi = 0;
      for (var c = 0; c < t.chunks.length && pos < end; c++) {
        var ch = t.chunks[c]; var cs = pos, ce = pos + ch.length;
        if (ce > start && cs < end) {
          var s = Math.max(start, cs) - cs, e = Math.min(end, ce) - cs;
          out.set(ch.subarray(s, e), oi); oi += (e - s);
        }
        pos = ce;
      }
      var bin = '', CH = 0x8000;
      for (var p = 0; p < out.length; p += CH) { bin += String.fromCharCode.apply(null, out.subarray(p, p + CH)); }
      return btoa(bin);
    };
    tlog('installed ' + FRAME);
  } catch (e) { try { window.__mseErr = String(e); } catch (_) {} }
})()"#;

/// 触发播放:静音播放所有 `<video>`(穿透 shadow DOM)+ 点常见播放/海报按钮。
const PLAY_JS: &str = r#"(() => {
  const acted = [];
  const vs = (window.__videos ? window.__videos() : Array.from(document.querySelectorAll('video')));
  vs.forEach(v => {
    try { v.muted = true; v.volume = 0; const p = v.play && v.play(); if (p && p.catch) p.catch(()=>{}); acted.push('video.play'); } catch (e) {}
  });
  const sels = ['.plv-poster','.pv-poster','.plv-controls__play','.plv-controls-play',
    '.prism-big-play-btn','.vjs-big-play-button','.plyr__control--overlaid',
    '[aria-label="播放"]','[aria-label="play"]','.play-btn','.poster','.pv-mask','.player-mask'];
  for (const s of sels) { const e = document.querySelector(s); if (e) { try { e.click(); acted.push('click:'+s); } catch (_) {} } }
  return acted.join(',') || 'none';
})()"#;

/// 每拍:对所有 video(穿透 shadow DOM)保活高倍速静音播放 + 回报播放/缓冲/已采集进度(JSON)。
const POLL_JS: &str = r#"(function (){
  var vs = (window.__videos ? window.__videos() : Array.from(document.querySelectorAll('video')));
  var v = null;
  for (var i = 0; i < vs.length; i++) { if (vs[i].duration && isFinite(vs[i].duration) && vs[i].duration > 0) { v = vs[i]; break; } }
  if (!v && vs.length) v = vs[0];
  // 对所有 video 都保活(真正在播的那个可能不是 v)。
  for (var k = 0; k < vs.length; k++) {
    try { vs[k].muted = true; vs[k].volume = 0; if (vs[k].playbackRate < 8) vs[k].playbackRate = 16; var p = vs[k].play(); if (p && p.catch) p.catch(function (){}); } catch (e) {}
  }
  var info = (window.__mseInfo ? JSON.parse(window.__mseInfo()) : { tracks: [] });
  var bytes = 0; (info.tracks || []).forEach(function (t){ bytes += t.bytes; });
  if (!v) return JSON.stringify({ video: false, tracks: (info.tracks || []).length, bytes: bytes });
  var bend = 0; try { if (v.buffered && v.buffered.length) bend = v.buffered.end(v.buffered.length - 1); } catch (e) {}
  return JSON.stringify({
    video: true, ct: v.currentTime || 0, dur: (isFinite(v.duration) ? v.duration : 0),
    bend: bend, ended: !!v.ended, paused: !!v.paused, rate: v.playbackRate,
    tracks: (info.tracks || []).length, bytes: bytes
  });
})()"#;

/// 卡死兜底:把 currentTime 推到已缓冲末尾再播,逼 hls.js 继续加载(尽量少用,会触发 seek)。
const NUDGE_JS: &str = r#"(function (){
  var vs = (window.__videos ? window.__videos() : Array.from(document.querySelectorAll('video')));
  if (!vs.length) return false;
  vs.forEach(function (v){
    try {
      var b = 0; if (v.buffered && v.buffered.length) b = v.buffered.end(v.buffered.length - 1);
      v.currentTime = Math.max(v.currentTime, b) + 0.3;
      var pr = v.play(); if (pr && pr.catch) pr.catch(function (){});
    } catch (e) {}
  });
  return true;
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
    let id = url
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("video")
        .to_string();

    let out = std::env::current_dir()?.join("captures").join("luffycity");
    std::fs::create_dir_all(&out)?;
    println!("目标播放页 : {url}");
    println!("无头模式   : {headless}（HL=0 切有头）");
    println!("产物目录   : {}", out.display());

    // 反检测默认开;关站点隔离让 polyv 若用 iframe 也与主页面同进程(中继 postMessage 能到 top);
    // 放开自动播放 + 静音,逼无头下也起播并把整条时间线加载完。
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

    // ── 关键:导航【前】注入 MSE hook,确保早于 hls.js 创建 SourceBuffer ──
    tab.add_init_script(MSE_HOOK_JS).await?;

    tab.get(&url).await?;
    tokio::time::sleep(Duration::from_secs(6)).await;

    // 触发播放:解密后播放器才会 append。重试几次等异步播放器挂载。
    for _ in 0..3 {
        let acted = tab.run_js(PLAY_JS).await.unwrap_or(Value::Null);
        println!("触发播放   : {acted}");
        tokio::time::sleep(Duration::from_secs(2)).await;
    }

    // ── 高倍速「过」完整条时间线,驱动 hls.js 把所有分片加载 + append ──
    println!("\n开始驱动播放(16x 静音)抽取明文 fMP4 …");
    let deadline = Instant::now() + Duration::from_secs(240);
    let mut last_bytes = 0u64;
    let mut stall = 0u32;
    let mut nudges = 0u32;
    loop {
        let raw = tab.run_js(POLL_JS).await.unwrap_or(Value::Null);
        let st: Value =
            serde_json::from_str(raw.as_str().unwrap_or("{}")).unwrap_or_else(|_| json!({}));
        let dur = st["dur"].as_f64().unwrap_or(0.0);
        let bend = st["bend"].as_f64().unwrap_or(0.0);
        let ct = st["ct"].as_f64().unwrap_or(0.0);
        let ended = st["ended"].as_bool().unwrap_or(false);
        let bytes = st["bytes"].as_u64().unwrap_or(0);
        let tracks = st["tracks"].as_u64().unwrap_or(0);
        println!(
            "  进度 ct={ct:.1}s  buffered={bend:.1}/{dur:.1}s  轨道={tracks}  已采集={}",
            human(bytes)
        );

        if ended || (dur > 1.0 && bend >= dur - 0.6) {
            println!("  ✓ 已覆盖整条时间线");
            break;
        }
        if bytes == last_bytes {
            stall += 1;
        } else {
            stall = 0;
            last_bytes = bytes;
        }
        if stall >= 8 {
            if dur > 1.0 && bend < dur - 0.6 && nudges < 6 {
                let _ = tab.run_js(NUDGE_JS).await;
                nudges += 1;
                stall = 0;
                println!("  · 播放疑似卡住,nudge #{nudges} 续推");
            } else if bytes > 0 {
                println!("  ! 进度停滞,以已采集数据收尾");
                break;
            }
        }
        if Instant::now() > deadline {
            println!("  ! 总超时(240s),以已采集数据收尾");
            break;
        }
        tokio::time::sleep(Duration::from_millis(1500)).await;
    }

    // ── 提取:把每条轨道的 fMP4 字节分片拉回并落盘 ──
    let info_str = tab
        .run_js("window.__mseInfo ? window.__mseInfo() : '{}'")
        .await?
        .as_str()
        .unwrap_or("{}")
        .to_string();
    let info: Value = serde_json::from_str(&info_str).unwrap_or_else(|_| json!({}));
    let tracks = info["tracks"].as_array().cloned().unwrap_or_default();

    println!("\n抓到轨道 {} 条", tracks.len());
    if tracks.is_empty() {
        println!("❌ 未捕获到任何 MSE 数据。诊断日志:");
        if let Some(log) = info["log"].as_array() {
            for l in log {
                println!("   - {}", l.as_str().unwrap_or(""));
            }
        }
        println!("可能原因:播放未起播 / 需登录 / 播放器用 MSE-in-Worker。试 HL=0 有头排障。");
        browser.quit().await?;
        return Ok(());
    }

    const SLICE: u64 = 4 * 1024 * 1024;
    let mut track_files: Vec<PathBuf> = Vec::new();
    for (i, t) in tracks.iter().enumerate() {
        let bytes = t["bytes"].as_u64().unwrap_or(0);
        let mime = t["mime"].as_str().unwrap_or("");
        println!(
            "  轨道#{i}  {}  {}  分块={}",
            if mime.is_empty() {
                "(未知 mime)"
            } else {
                mime
            },
            human(bytes),
            t["chunks"].as_u64().unwrap_or(0)
        );
        if bytes == 0 {
            continue;
        }
        let mut buf: Vec<u8> = Vec::with_capacity(bytes as usize);
        let mut start = 0u64;
        while start < bytes {
            let b64 = tab
                .run_js(&format!("window.__mseSlice({i},{start},{SLICE})"))
                .await?;
            let b64 = b64.as_str().unwrap_or("");
            if b64.is_empty() {
                break;
            }
            buf.extend_from_slice(&b64_decode(b64));
            start += SLICE;
        }
        let path = out.join(format!("track{i}.mp4"));
        std::fs::write(&path, &buf)?;
        println!(
            "    → 落盘 {} ({})",
            path.display(),
            human(buf.len() as u64)
        );
        track_files.push(path);
    }

    if track_files.is_empty() {
        println!("❌ 轨道均为空,无法合成。");
        browser.quit().await?;
        return Ok(());
    }

    // ── ffmpeg 合并成最终 mp4:视频无损 copy + 音频重编码 AAC ──
    // 为何音频不 copy:MSE 截到的 fMP4 音频分片直接 `-c copy` 会被 ffmpeg 判成 0 packet 丢弃
    //（解码却正常,疑似 hls.js 音频 init 段的 edit-list/priming quirk);重编码 AAC 体积小、损失可忽略。
    let final_mp4 = out.join(format!("{id}.mp4"));
    let mut args: Vec<String> = vec![
        "-y".into(),
        "-hide_banner".into(),
        "-loglevel".into(),
        "warning".into(),
    ];
    for f in &track_files {
        args.push("-i".into());
        args.push(f.to_string_lossy().into_owned());
    }
    for idx in 0..track_files.len() {
        args.push("-map".into());
        args.push(idx.to_string());
    }
    args.extend([
        "-c:v".into(),
        "copy".into(),
        "-c:a".into(),
        "aac".into(),
        "-b:a".into(),
        "128k".into(),
    ]);
    args.push("-movflags".into());
    args.push("+faststart".into());
    args.push(final_mp4.to_string_lossy().into_owned());

    println!(
        "\nffmpeg 合并 {} 条轨道(视频 copy + 音频 aac)→ {}",
        track_files.len(),
        final_mp4.display()
    );
    let status = Command::new("ffmpeg").args(&args).status();
    let ok = matches!(status, Ok(s) if s.success());
    if !ok && track_files.len() > 1 {
        // 合并失败兜底:挑体积最大的轨道(通常是视频)单独 remux,至少保住主画面。
        let biggest = track_files
            .iter()
            .max_by_key(|p| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0))
            .cloned()
            .unwrap_or_else(|| track_files[0].clone());
        println!(
            "  多轨合并失败,退回只 remux 最大轨道 {} …",
            biggest.display()
        );
        let _ = Command::new("ffmpeg")
            .args([
                "-y",
                "-hide_banner",
                "-loglevel",
                "warning",
                "-i",
                &biggest.to_string_lossy(),
                "-c:v",
                "copy",
                "-c:a",
                "aac",
                "-b:a",
                "128k",
                "-movflags",
                "+faststart",
                &final_mp4.to_string_lossy(),
            ])
            .status();
    }

    // ── 高亮汇报 ──
    println!("\n================= 下载结果(高亮) =================");
    if final_mp4.exists() {
        let sz = std::fs::metadata(&final_mp4).map(|m| m.len()).unwrap_or(0);
        println!("最终视频   : ✅ {}  ({})", final_mp4.display(), human(sz));
        // 用 ffprobe(若有)报告时长/分辨率,核对完整性。
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
                &final_mp4.to_string_lossy(),
            ])
            .output()
        {
            if o.status.success() {
                for ln in String::from_utf8_lossy(&o.stdout).lines() {
                    println!("            {ln}");
                }
            }
        }
    } else {
        println!("最终视频   : ❌ 生成失败(轨道文件仍在 {})", out.display());
    }
    println!("中间产物   : track*.mp4(原始 fMP4,可删)");
    println!(
        "说明       : 全程未碰 polyv 密钥/算法 —— 浏览器内 hls.js 解密后,在 MSE append 处截获明文 fMP4。"
    );
    println!("==================================================");

    browser.quit().await?;
    Ok(())
}

/// 人类可读字节数。
fn human(n: u64) -> String {
    const U: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut f = n as f64;
    let mut i = 0;
    while f >= 1024.0 && i < U.len() - 1 {
        f /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{n} B")
    } else {
        format!("{f:.2} {}", U[i])
    }
}

/// 极小 base64 解码(标准字母表;跳过 '=' 与空白)。避免为一个示例引入 base64 依赖。
fn b64_decode(s: &str) -> Vec<u8> {
    fn val(c: u8) -> i16 {
        match c {
            b'A'..=b'Z' => (c - b'A') as i16,
            b'a'..=b'z' => (c - b'a' + 26) as i16,
            b'0'..=b'9' => (c - b'0' + 52) as i16,
            b'+' => 62,
            b'/' => 63,
            _ => -1,
        }
    }
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let mut buf = 0u32;
    let mut bits = 0u32;
    for &c in s.as_bytes() {
        let v = val(c);
        if v < 0 {
            continue;
        }
        buf = (buf << 6) | (v as u32);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
        }
    }
    out
}
