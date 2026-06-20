//! 易盾「文字点选」验证码:`det → 逐框 OCR → 按提示顺序可信点击`(CDP 后端)。
//!
//! 案例:https://dun.163.com/trial/picture-click(试用页需触发才弹验证码)。
//! 运行:`cargo run --example yidun_click --features ocr`(默认有头;`HL=1` 无头)。
//! 产物写到 `target/yidun/`(cap.png 验证码图、result.png 结果截图,不污染仓库根目录)。
//! 易盾另有**行为风控**,字找得准≠必过(与识别算法两件事)。

use std::time::Duration;

use drission::ocr::ClickWord;
use drission::prelude::*;

const URL: &str = "https://dun.163.com/trial/picture-click";

// 触发验证码:点页面里像“验证/体验/点击按钮”的候选(易盾试用 demo 触发器)。
const TRIGGER_JS: &str = r#"(() => {
  const want = /点击按钮进行验证|开始验证|立即体验|点击验证|验证码|体验|验证/;
  const els = [...document.querySelectorAll('button,a,div,span,input')]
    .filter(e => e.offsetParent !== null);
  for (const e of els) {
    const t = (e.innerText||e.value||e.placeholder||'').trim();
    if (want.test(t) && t.length <= 16) { e.click(); return 'clicked:'+t; }
  }
  // 退一步:点 yidun 智能按钮
  const y = document.querySelector('.yidun_intelli-icon,.yidun-intelli,.yidun');
  if (y) { y.click(); return 'clicked:yidun'; }
  return 'no-trigger';
})()"#;

// 找验证码背景图:易盾常用 .yidun_bg-img;兜底找 src 像验证码的 img。
const FIND_CAP_JS: &str = r#"(() => {
  const vis = e => e.offsetParent !== null;
  const r = e => { const b=e.getBoundingClientRect(); return {sel:'', x:Math.round(b.x),y:Math.round(b.y),w:Math.round(b.width),h:Math.round(b.height)}; };
  let el = document.querySelector('.yidun_bg-img, .yidun_bgimg, img.yidun_bg-img');
  if (el && vis(el)) return JSON.stringify({found:'yidun_bg-img', ...r(el), src:(el.src||'').slice(0,80)});
  // 兜底:可见 img 里尺寸像验证码(宽 250~360、高 120~200)且 src 含 captcha/nos/yidun
  const im = [...document.querySelectorAll('img')].filter(vis).find(e => {
    const b=e.getBoundingClientRect(); const s=(e.src||'');
    return b.width>=240 && b.width<=400 && b.height>=110 && b.height<=240 && /captcha|nos\.|yidun|dun\./i.test(s);
  });
  if (im) return JSON.stringify({found:'heuristic-img', ...r(im), src:(im.src||'').slice(0,80)});
  return JSON.stringify({found:''});
})()"#;

// 读提示文字(要依次点击哪些字)。易盾提示在 .yidun_tips__answer 文本里。
const TIPS_JS: &str = r#"(() => {
  const el = document.querySelector('.yidun_tips__answer, .yidun_tips, .yidun_tips__text');
  return el ? el.innerText.trim() : '';
})()"#;

// 换图(换一题):点易盾刷新键。逐个**具体**类名取第一个可见的点(避免宽泛 [class*=refresh] 命中隐藏元素)。
const REFRESH_JS: &str = r#"(() => {
  for (const s of ['.yidun_refresh', '.yidun-refresh', '.yidun_panel-refresh']) {
    const e = document.querySelector(s);
    if (e && e.offsetParent !== null) { e.click(); return 'refreshed:' + s; }
  }
  return 'no-refresh';
})()"#;

#[tokio::main]
async fn main() -> drission::Result<()> {
    let headless = matches!(
        std::env::var("HL").ok().as_deref(),
        Some("1") | Some("true")
    );
    // 运行产物统一写到 target/yidun/(不落在仓库根目录,避免误入库)。
    let out_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("yidun");
    std::fs::create_dir_all(&out_dir).ok();
    println!("[yidun] 加载 det+ocr 模型…");
    let cw = ClickWord::new().await?;
    println!("[yidun] 模型就绪 ✓");

    let browser = ChromiumBrowser::launch(
        ChromiumOptions::new()
            .headless(headless)
            .window_size(1200, 900),
    )
    .await?;
    let tab = browser.new_tab(Some(URL)).await?;
    tokio::time::sleep(Duration::from_secs(5)).await;

    // ① 点“在线体验”载入 demo 控件
    let r = tab
        .run_js(TRIGGER_JS)
        .await
        .ok()
        .and_then(|v| v.as_str().map(String::from));
    println!("[yidun] 触发 = {r:?}");
    tokio::time::sleep(Duration::from_secs(2)).await;
    // ② 可信点击易盾验证按钮(`.yidun_control`),弹出点选挑战(JS .click() 易盾不认,需 isTrusted)
    let mut clicked = false;
    for s in ["css:.yidun_control", "css:.yidun_tips", "css:.yidun"] {
        if let Ok(el) = tab.ele(s).await {
            if el.click().await.is_ok() {
                println!("[yidun] 可信点击验证按钮 {s}");
                clicked = true;
                break;
            }
        }
    }
    if !clicked {
        println!("[yidun] 未找到验证按钮");
    }
    tokio::time::sleep(Duration::from_secs(4)).await;
    let f = tab
        .run_js(FIND_CAP_JS)
        .await
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default();
    println!("[yidun] 验证码图 = {f}");
    // 诊断:列出所有 .yidun* 节点,定位挑战图/提示真实类名
    let yidun_dump = r#"(()=>{const out=[];for(const e of document.querySelectorAll('[class*=yidun]')){
      if(e.offsetParent===null)continue;const b=e.getBoundingClientRect();
      if(b.width<2&&b.height<2)continue;
      out.push({cls:e.className,tag:e.tagName,x:Math.round(b.x),y:Math.round(b.y),w:Math.round(b.width),h:Math.round(b.height),txt:(e.childElementCount?'':(e.textContent||'').trim().slice(0,20))});}
      return JSON.stringify(out.slice(0,40),null,1);})()"#;
    let yd = tab
        .run_js(yidun_dump)
        .await
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default();
    println!("[yidun] .yidun* 节点 =\n{yd}");

    // 截验证码图 rect + 按 PNG 宽算缩放(图内像素 → 页面 CSS 坐标)。
    // 多次尝试:每次 取图 → 解析提示 → 受约束求解(全局最优指派 + 置信度)→ 阈值门控 → 点击 / 换图重试。
    let tries: u32 = std::env::var("YIDUN_TRIES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(3);
    // 最低置信度阈值:某目标低于它多半是误识 → 不乱点,换图重试(可用 YIDUN_MIN_CONF 调)。
    let min_conf: f32 = std::env::var("YIDUN_MIN_CONF")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.30);
    let mut passed = false;

    for attempt in 1..=tries {
        println!("\n[yidun] ===== 第 {attempt}/{tries} 次 =====");

        // 视图(通用库能力 `tab.image_view`):读源图 <img> 的显示 rect + 自然尺寸 + src。
        // 取**干净源图**喂检测(源图无右上角工具栏图标 → Det 不会误检刷新/语音键);取不到退回元素截图。
        let view = tab
            .image_view(".yidun_bg-img, img.yidun_bg-img, .yidun_bgimg")
            .await
            .ok();
        let mut cap_png = Vec::new();
        let mut from_src = false;
        if let Some(v) = &view
            && v.src.starts_with("http")
            && let Ok(bytes) = fetch_image(&v.src).await
            && bytes.len() > 2000
        {
            cap_png = bytes;
            from_src = true;
            std::fs::write(out_dir.join("cap.jpg"), &cap_png).ok();
            println!(
                "[yidun] 源图 jpg {}… ({} bytes,无图标)",
                v.src.chars().take(54).collect::<String>(),
                cap_png.len()
            );
        }
        if cap_png.is_empty() {
            for s in ["css:.yidun_bg-img", "css:.yidun_bgimg"] {
                if let Ok(el) = tab.ele(s).await
                    && let Ok(b) = el.screenshot_bytes().await
                    && b.len() > 2000
                {
                    cap_png = b;
                    println!("[yidun] 源图取失败,退回截图 {s} ({} bytes)", cap_png.len());
                    break;
                }
            }
        }
        if cap_png.is_empty() {
            cap_png = tab.screenshot_bytes().await?;
            println!("[yidun] 未取到验证码图,整页兜底({} bytes)", cap_png.len());
        }

        // 提示:挑战态要依次点哪些字(DOM 渲染自 api 的 data.front)。
        let tips = tab
            .run_js(TIPS_JS)
            .await
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default();
        let targets: Vec<String> = tips
            .rsplit("点击")
            .next()
            .unwrap_or("")
            .chars()
            .filter(|c| ('\u{4e00}'..='\u{9fff}').contains(c))
            .map(|c| c.to_string())
            .collect();
        println!("[yidun] 提示「{tips}」→ 目标顺序 {targets:?}");

        // 坐标视图:源图用其 naturalWidth 映射;截图兜底则按解码尺寸构造(scale 由 ImageView 内部算)。
        let mut mv = view.clone().unwrap_or_default();
        if !from_src && let Ok(im) = image::load_from_memory(&cap_png) {
            mv.natural_w = im.width() as f64;
            mv.natural_h = im.height() as f64;
        }

        // 受约束求解:全局最优指派 + 每点置信度(替代旧的按目标序贪心)。
        let hits = cw.solve(&cap_png, &targets)?;
        for h in &hits {
            let tpl = h
                .template
                .map(|t| format!(" tpl={t:.2}"))
                .unwrap_or_default();
            println!(
                "[yidun]   「{}」 aff={:.2}{tpl} 图内点({},{})",
                h.target, h.affinity, h.point.0, h.point.1
            );
        }

        // 采集自训样本(阶段3):`YIDUN_DUMP=目录` 把每个字框裁剪图落盘 → 人工标注 → dddd_trainer 训练
        // → `DRISSION_OCR_MODEL/CHARSET` 热替换(见 examples/ocr_hotswap 与 docs/OCR模型热替换.md)。
        if let Ok(dir) = std::env::var("YIDUN_DUMP")
            && let Ok(crops) = cw.crops(&cap_png)
        {
            std::fs::create_dir_all(&dir).ok();
            for (i, (_b, png)) in crops.iter().enumerate() {
                std::fs::write(format!("{dir}/g_{attempt}_{i}.png"), png).ok();
            }
            println!(
                "[yidun] 采样:{} 个字框存到 {dir}/(提示字 {targets:?},标注后训练)",
                crops.len()
            );
        }
        // 综合置信度 = max(OCR 亲和度, 字形模板分):OCR 读不出时,真样本/字体模板高分也能驱动点击。
        let min_aff = hits
            .iter()
            .map(|h| h.affinity.max(h.template.unwrap_or(0.0)))
            .fold(f32::INFINITY, f32::min);
        let complete = !targets.is_empty() && hits.len() == targets.len();
        // 末次:即便低于阈值也按"最佳猜测"兜底点击(不白白浪费最后一次);非末次低置信则换图重试。
        let final_try = attempt == tries;
        let conf_ok = complete && (min_aff >= min_conf || final_try);
        println!(
            "[yidun] 命中 {}/{} · 最低置信度 {:.2}(阈值 {:.2})· rect 宽{:.0} scale={:.3} → {}",
            hits.len(),
            targets.len(),
            if hits.is_empty() { 0.0 } else { min_aff },
            min_conf,
            mv.w,
            mv.scale_x(),
            if conf_ok && mv.is_valid() {
                if min_aff >= min_conf {
                    "点击"
                } else {
                    "末次·按最佳猜测兜底点击"
                }
            } else {
                "置信不足/未集齐,换图重试"
            }
        );

        if conf_ok && mv.is_valid() {
            // 叠加图:把点击点(按顺序 红/绿/蓝/橙)画到验证码图上,直观看“点在哪几个字”(不改页面 DOM)。
            if let Ok(dimg) = image::load_from_memory(&cap_png) {
                let mut rgba = dimg.to_rgba8();
                let colors = [
                    [255, 0, 0, 255],
                    [0, 200, 0, 255],
                    [0, 90, 255, 255],
                    [255, 160, 0, 255],
                ];
                for (i, h) in hits.iter().enumerate() {
                    draw_marker(&mut rgba, h.point.0 as i32, h.point.1 as i32, colors[i % 4]);
                }
                rgba.save(out_dir.join(format!("overlay_{attempt}.png")))
                    .ok();
                println!("[yidun] 点击点叠加图 → target/yidun/overlay_{attempt}.png");
            }
            // 拟人轨迹点击(**通用库能力** `tab.human_click`):图内像素点 → `mv.map_u32` 映射到页面坐标 →
            // 点间走连续曲线 + minimum-jerk 变速 + 手抖,产生密集 mousemove/pointermove,对冲行为风控。
            let points: Vec<(f64, f64)> = hits.iter().map(|h| mv.map_u32(h.point)).collect();
            for (h, &(cx, cy)) in hits.iter().zip(&points) {
                println!("[yidun] 拟人点击「{}」→ ({cx:.0},{cy:.0})", h.target);
            }
            tab.human_click(&points).await?;
            tokio::time::sleep(Duration::from_secs(2)).await;
            let result = tab
                .run_js("(()=>{const e=document.querySelector('.yidun_tips__text');return e?e.innerText.trim():'';})()")
                .await
                .ok()
                .and_then(|v| v.as_str().map(String::from))
                .unwrap_or_default();
            let shot = tab.screenshot_bytes().await?;
            std::fs::write(out_dir.join(format!("result_{attempt}.png")), &shot).ok();
            println!("[yidun] 点击后提示=「{result}」,结果截图 target/yidun/result_{attempt}.png");
            if result.contains("成功") {
                passed = true;
                break;
            }
            println!("[yidun] 未通过(易盾另有行为风控:字点准也可能判失败,与识别算法两件事)");
        }

        // 换图重试(末次不再换)。优先**可信点击**易盾刷新按钮(它是真 `<button>刷新`),兜底走 JS click。
        if attempt < tries {
            let mut refreshed = None;
            for s in [
                "css:.yidun_refresh",
                "css:.yidun-refresh",
                "css:.yidun_panel-refresh",
            ] {
                if let Ok(el) = tab.ele(s).await
                    && el.click().await.is_ok()
                {
                    refreshed = Some(format!("可信点击 {s}"));
                    break;
                }
            }
            if refreshed.is_none() {
                refreshed = tab
                    .run_js(REFRESH_JS)
                    .await
                    .ok()
                    .and_then(|v| v.as_str().map(String::from));
            }
            println!("[yidun] 换图 = {refreshed:?}");
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }

    println!(
        "\n[yidun] 最终:{}",
        if passed {
            "通过 ✓"
        } else {
            "未通过(检测+识别+全局指派链路已验证;行为风控 / 艺术字 OCR 为已知局限)"
        }
    );

    if !headless {
        tokio::time::sleep(Duration::from_secs(60)).await;
    }
    browser.quit().await?;
    Ok(())
}

/// 在图上画一个空心圆环 + 中心点标记(用于点击点叠加图)。
fn draw_marker(img: &mut image::RgbaImage, cx: i32, cy: i32, color: [u8; 4]) {
    let (w, h) = (img.width() as i32, img.height() as i32);
    let put = |img: &mut image::RgbaImage, x: i32, y: i32| {
        if x >= 0 && y >= 0 && x < w && y < h {
            img.put_pixel(x as u32, y as u32, image::Rgba(color));
        }
    };
    let r = 13i32;
    // 圆环(2px 粗)。
    let steps = 720;
    for s in 0..steps {
        let t = (s as f32) * std::f32::consts::TAU / steps as f32;
        for rr in (r - 1)..=r {
            put(
                img,
                cx + (rr as f32 * t.cos()) as i32,
                cy + (rr as f32 * t.sin()) as i32,
            );
        }
    }
    // 中心 3x3 实心点。
    for dy in -1..=1 {
        for dx in -1..=1 {
            put(img, cx + dx, cy + dy);
        }
    }
}
