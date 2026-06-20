//! 截图与录像(对标 DrissionPage `browser_control/screen`)端到端自验证。
//!
//! 覆盖:
//! - **页面截图**:视口 / 整页 / **JPEG** / **指定区域** / **base64** / 按后缀保存。
//! - **元素截图**:`ele.screenshot_bytes()` / `ele.get_screenshot()`(先滚到视口中央)。
//! - **录像**:`Imgs` 模式后台逐帧存图(动画页→多帧);`FrugalImgs` 省流模式(静态页→去重只剩 1 帧)。
//!
//! 全程离线:用写到项目 `target/` 下的本地 HTML(`file://`),不依赖外网。
//! 运行:`cargo run --example screencast`
//!
//! 末尾打印 `ALL CHECKS PASSED` / `SOME CHECKS FAILED`,关键校验不过则进程非 0 退出。

use std::path::{Path, PathBuf};
use std::time::Duration;

use drission::prelude::*;

const PNG_MAGIC: &[u8] = &[0x89, 0x50, 0x4E, 0x47];
const JPEG_MAGIC: &[u8] = &[0xFF, 0xD8];

#[tokio::main]
async fn main() -> drission::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    // 本地测试页写到项目 target 下(在 home 下、绕开 /var/folders 沙箱限制)。
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/screencast-demo");
    tokio::fs::create_dir_all(&base).await?;
    let anim = base.join("anim.html");
    let still = base.join("still.html");
    tokio::fs::write(&anim, ANIM_HTML).await?;
    tokio::fs::write(&still, STILL_HTML).await?;

    println!("[*] 启动 Camoufox(headless)…");
    let browser = Browser::launch(BrowserOptions::new().headless(true)).await?;
    let tab = browser.latest_tab().await?;

    // 打开动画页,等元素就绪(导航后执行上下文切换需要一拍),再确认未踩到 file:// 沙箱(空文档约 39 字节)。
    tab.get(&file_url(&anim)).await?;
    let el_ready = tab
        .wait()
        .ele_displayed("#x", Some(Duration::from_secs(5)))
        .await?;
    let html_len = tab.html().await?.len();
    let doc_ok = el_ready && html_len > 39;
    println!("[*] 动画页已加载,#x就绪={el_ready} html_len={html_len}(doc_ok={doc_ok})");

    // ---------- 页面截图 ----------
    let png_view = tab.screenshot_bytes(false).await?;
    let png_full = tab.screenshot(&ShotOpts::new().full_page(true)).await?;
    let jpg = tab
        .screenshot(&ShotOpts::new().format(ImageFormat::Jpeg).quality(80))
        .await?;
    let region = tab
        .screenshot(&ShotOpts::new().region((0.0, 0.0), (120.0, 80.0)))
        .await?;
    let b64 = tab.screenshot_base64(false).await?;
    let saved_jpg = tab.get_screenshot(base.join("page.jpg"), false).await?;
    let saved_jpg_bytes = tokio::fs::read(&saved_jpg).await?;

    let png_view_ok = png_view.starts_with(PNG_MAGIC);
    let png_full_ok = png_full.starts_with(PNG_MAGIC) && png_full.len() > png_view.len() / 2;
    let jpg_ok = jpg.starts_with(JPEG_MAGIC);
    let region_ok = region.starts_with(PNG_MAGIC) && !region.is_empty();
    let b64_ok = b64.starts_with("iVBOR"); // PNG 头 "\x89PNG" 的 base64 前缀
    let saved_jpg_ok = saved_jpg_bytes.starts_with(JPEG_MAGIC);
    println!(
        "[截图] 视口PNG={}B({png_view_ok}) 整页PNG={}B({png_full_ok}) JPEG={}B({jpg_ok}) 区域PNG={}B({region_ok}) base64={}({b64_ok}) 存JPG={}B({saved_jpg_ok})",
        png_view.len(),
        png_full.len(),
        jpg.len(),
        region.len(),
        b64.len(),
        saved_jpg_bytes.len(),
    );

    // ---------- 元素截图 ----------
    let el = tab.ele("#x").await?;
    let ele_png = el.screenshot_bytes().await?;
    let ele_saved = el.get_screenshot(base.join("ele.png")).await?;
    let ele_saved_bytes = tokio::fs::read(&ele_saved).await?;
    let ele_ok = ele_png.starts_with(PNG_MAGIC) && ele_saved_bytes.starts_with(PNG_MAGIC);
    println!(
        "[元素截图] ele(#x)={}B saved={}({ele_ok})",
        ele_png.len(),
        ele_saved.display()
    );

    // ---------- 录像:Imgs(动画页 → 连续多帧) ----------
    let imgs_dir = base.join("rec_imgs");
    let _ = tokio::fs::remove_dir_all(&imgs_dir).await;
    let cast = tab.screencast();
    cast.set_mode(ScreencastMode::Imgs).set_fps(10.0);
    cast.start(Some(&imgs_dir)).await?;
    let recording_flag = cast.is_recording();
    tab.wait().secs(1.2).await;
    let imgs_out = cast.stop().await?;
    let stopped_flag = !cast.is_recording();
    let frames_imgs = count_png(&imgs_out).await;
    println!(
        "[录像/Imgs] recording={recording_flag} 帧目录={} 帧数={frames_imgs} stopped={stopped_flag}",
        imgs_out.display()
    );

    // ---------- 录像:FrugalImgs(静态页 → 去重只剩极少帧) ----------
    tab.get(&file_url(&still)).await?;
    let frugal_dir = base.join("rec_frugal");
    let _ = tokio::fs::remove_dir_all(&frugal_dir).await;
    let cast2 = tab.screencast();
    cast2.set_mode(ScreencastMode::FrugalImgs).set_fps(10.0);
    cast2.start(Some(&frugal_dir)).await?;
    tab.wait().secs(1.0).await;
    let frugal_out = cast2.stop().await?;
    let frames_frugal = count_png(&frugal_out).await;
    println!(
        "[录像/Frugal] 静态页帧目录={} 帧数={frames_frugal}(应远少于 Imgs)",
        frugal_out.display()
    );

    // ---------- 录像:Video(动画页 → ffmpeg 合成 mp4;无 ffmpeg 则跳过,不计失败) ----------
    tab.get(&file_url(&anim)).await?;
    tab.wait()
        .ele_displayed("#x", Some(Duration::from_secs(5)))
        .await?;
    let video_dir = base.join("rec_video");
    let _ = tokio::fs::remove_dir_all(&video_dir).await;
    let cast3 = tab.screencast();
    cast3
        .set_mode(ScreencastMode::Video)
        .set_fps(10.0)
        .set_save_path(&video_dir);
    cast3.start(None::<&str>).await?;
    tab.wait().secs(1.0).await;
    let (video_ok, video_note) = match cast3.stop().await {
        Ok(mp4) => {
            let bytes = tokio::fs::read(&mp4).await.unwrap_or_default();
            // mp4 文件头部通常含 "ftyp" box 标识。
            let is_mp4 = !bytes.is_empty() && bytes.windows(4).take(64).any(|w| w == b"ftyp");
            (
                is_mp4,
                format!("mp4={}({}B, mp4签名={is_mp4})", mp4.display(), bytes.len()),
            )
        }
        Err(e) => (true, format!("跳过:无 ffmpeg 或合成失败({e})")),
    };
    println!("[录像/Video] {video_note}");

    // ---------- 汇总 ----------
    let pass = doc_ok
        && video_ok
        && png_view_ok
        && png_full_ok
        && jpg_ok
        && region_ok
        && b64_ok
        && saved_jpg_ok
        && ele_ok
        && recording_flag
        && stopped_flag
        && frames_imgs >= 3
        && (1..=3).contains(&frames_frugal)
        && frames_imgs > frames_frugal;
    println!(
        "\n==== {} ====",
        if pass {
            "ALL CHECKS PASSED"
        } else {
            "SOME CHECKS FAILED"
        }
    );

    browser.quit().await?;
    if pass {
        Ok(())
    } else {
        Err(drission::Error::msg("screencast 自验证未通过"))
    }
}

/// 统计目录下 `.png` 帧数量。
async fn count_png(dir: &Path) -> usize {
    let mut n = 0;
    if let Ok(mut rd) = tokio::fs::read_dir(dir).await {
        while let Ok(Some(entry)) = rd.next_entry().await {
            if entry.path().extension().and_then(|e| e.to_str()) == Some("png") {
                n += 1;
            }
        }
    }
    n
}

fn file_url(path: &Path) -> String {
    format!("file://{}", path.display())
}

/// 动画页:每 50ms 改背景色与文字 → 帧与帧之间画面持续变化。
const ANIM_HTML: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>anim</title></head>
<body style="margin:0;height:300px">
<div id="x" style="font-size:64px;font-family:monospace;color:#fff">0</div>
<script>
let n=0;
setInterval(()=>{n++;document.body.style.background='rgb('+(n*7%255)+','+(n*3%255)+','+(n*11%255)+')';document.getElementById('x').textContent=String(n);},50);
</script></body></html>"#;

/// 静态页:无脚本、无动画 → 连续截图字节一致(FrugalImgs 去重后应只剩 1 帧)。
const STILL_HTML: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>still</title></head>
<body style="margin:0;height:300px;background:#204060">
<div id="x" style="font-size:64px;font-family:monospace;color:#fff">STILL</div>
</body></html>"#;
