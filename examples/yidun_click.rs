//! 易盾「文字点选」验证码:**监听接口取图取序** → det → 逐框 OCR → 按序可信点击(CDP 后端)。
//!
//! 案例:https://dun.163.com/trial/picture-click(试用页需触发才弹验证码)。
//! 运行:`cargo run --example yidun_click --features cdp,ocr`(默认有头;`HL=1` 无头)。
//! 产物写到 `target/yidun/`(cap.jpg 点选图、overlay_*.png 点击叠加、result_*.png 结果截图)。
//!
//! 关键认知(为何要"监听"而不是"截图"):易盾点选图**不在 `<img>.src`** 上——它由接口
//! `c.dun.163.com/api/v3/get` 以 **JSONP** 下发 `data.bg[0]`(干净 JPEG,无右上角刷新/语音工具栏)
//! 与 `data.front`(**要依次点击的字 + 顺序**)。截图会把工具栏拍进去且易缩放错位 → 点不中。
//! 故本例**监听 `c.dun.163.com/api/*`**:`api/get` 取干净图 + 点击顺序;点完再读 `api/check`
//! 的响应作为"点击是否被易盾接收/是否通过"的**铁证**。易盾另有行为风控,字点准≠必过。

use std::time::{Duration, Instant};

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
  const y = document.querySelector('.yidun_intelli-icon,.yidun-intelli,.yidun');
  if (y) { y.click(); return 'clicked:yidun'; }
  return 'no-trigger';
})()"#;

// 读 DOM 提示文字(仅用于和接口 front 交叉核对;点击顺序以接口 front 为准)。
const TIPS_JS: &str = r#"(() => {
  const el = document.querySelector('.yidun_tips__answer, .yidun_tips, .yidun_tips__text');
  return el ? el.innerText.trim() : '';
})()"#;

// 点击后读结果提示。
const RESULT_JS: &str = r#"(()=>{const e=document.querySelector('.yidun_tips__text');return e?e.innerText.trim():'';})()"#;

// 换图(换一题):点易盾刷新键(兜底 JS click)。
const REFRESH_JS: &str = r#"(() => {
  for (const s of ['.yidun_refresh', '.yidun-refresh', '.yidun_panel-refresh']) {
    const e = document.querySelector(s);
    if (e && e.offsetParent !== null) { e.click(); return 'refreshed:' + s; }
  }
  return 'no-refresh';
})()"#;

// 诊断:点击前看关键 yidun 节点的位置 + 是否真可见(offsetParent / visibility / opacity)。
const DUMP_VIS_JS: &str = r#"(()=>{const sel=['.yidun_bg-img','.yidun_cover-frame','.yidun_panel','.yidun_control'];const out=[];
for(const s of sel){const e=document.querySelector(s);if(!e){out.push(s+':无');continue;}
const b=e.getBoundingClientRect();const st=getComputedStyle(e);
const vis=e.offsetParent!==null&&st.visibility!=='hidden'&&parseFloat(st.opacity)>0.01;
out.push(`${s}:[${Math.round(b.x)},${Math.round(b.y)},${Math.round(b.width)},${Math.round(b.height)}]vis=${vis}`);}
return out.join('  ');})()"#;

// 诊断:点击后读提示文字 + 已点标记数(易盾点中会落 .yidun_point* 标记)。
const MARKERS_JS: &str = r#"(()=>{const m=document.querySelectorAll('[class*=yidun_point]');
const tip=document.querySelector('.yidun_tips__text');
return JSON.stringify({point_nodes:m.length,tip:tip?tip.innerText.trim():''});})()"#;

// 点选目标图选择器(读**实时** rect 用)。
const BG_SEL: &str = ".yidun_bg-img, img.yidun_bg-img, .yidun_bgimg";

// 在页面上画"将点击处"的红点(position:fixed ⇒ 直接用视口坐标;pointer-events:none 不挡真正的点击)。
// 配合截图 plan_*.png:一眼看出计划点击点是否压在要点的字上(排查偏移的铁证)。
const MARK_JS: &str = r#"((pts)=>{document.querySelectorAll('.__ymk').forEach(e=>e.remove());
for(const p of pts){const d=document.createElement('div');d.className='__ymk';
d.style.cssText='position:fixed;left:'+(p[0]-7)+'px;top:'+(p[1]-7)+'px;width:14px;height:14px;border-radius:50%;'
+'background:rgba(255,0,0,.55);border:2px solid #fff;box-shadow:0 0 4px #000;z-index:2147483647;pointer-events:none';
document.body.appendChild(d);}return pts.length;})"#;

#[tokio::main]
async fn main() -> drission::Result<()> {
    let headless = matches!(
        std::env::var("HL").ok().as_deref(),
        Some("1") | Some("true")
    );
    // 输出目录:发布二进制(如 Windows 验证包)用 `YIDUN_OUT` 指向可写目录(`CARGO_MANIFEST_DIR`
    // 是**编译期**路径,在别的机器上不存在 → 截图/叠加图会静默写不出);本地 `cargo run` 不设则回退 target/yidun。
    let out_dir = std::env::var_os("YIDUN_OUT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("target")
                .join("yidun")
        });
    std::fs::create_dir_all(&out_dir).ok();
    println!("[yidun] 加载 det+ocr 模型…");
    let cw = ClickWord::new().await?;
    println!("[yidun] 模型就绪 ✓");

    // 可选持久 profile(`YIDUN_PROFILE`=目录):有头本地跑时设它 —— Edge/Chrome 的首启/「使用 xxx 身份」
    // 账户提示只需手动关一次,profile 记住后续不再弹(临时 profile 每次都会重弹挡住首次验证点击)。
    // ★ 跨平台一致的关键:**强制设备像素比 = 1**。Windows 显示缩放(125%/150%)有头时,
    //   `getBoundingClientRect`(CSS px)与 CDP `Input.dispatchMouseEvent` 的像素口径可能不一致 →
    //   点击整体偏移(mac retina 能点中、Win 缩放下点不到字的根因)。`--force-device-scale-factor=1`
    //   令 CSS px == 设备 px,读到的 rect 与点击坐标一一对应,Win/mac 行为一致。
    let mut opts = ChromiumOptions::new()
        .headless(headless)
        .window_size(1200, 900)
        .add_arg("--force-device-scale-factor=1")
        .add_arg("--high-dpi-support=1");
    if let Some(dir) = std::env::var_os("YIDUN_PROFILE") {
        opts = opts.user_data_dir(std::path::PathBuf::from(dir));
    }
    let browser = ChromiumBrowser::launch(opts).await?;
    let tab = browser.new_tab(Some(URL)).await?;
    tokio::time::sleep(Duration::from_secs(5)).await;
    if let Ok(v) = tab
        .run_js("({dpr:devicePixelRatio,iw:innerWidth,ih:innerHeight})")
        .await
    {
        println!(
            "[yidun] 视口 dpr={} {:.0}x{:.0}(已强制 device-scale=1 ⇒ CSS px == 设备 px)",
            v["dpr"].as_f64().unwrap_or(0.0),
            v["iw"].as_f64().unwrap_or(0.0),
            v["ih"].as_f64().unwrap_or(0.0)
        );
    }

    // ★ 监听验证码接口(在触发前开,确保抓到第一题)。只过滤 `c.dun.163.com/api/*`(=get/check),
    //   不抓 `/v4/j/up`(上报)与 `/node/api/check-guardian`(不同路径),噪声最小。
    tab.listen().start(&["dun.163.com/api"]).await?;
    println!("[yidun] 监听 c.dun.163.com/api/*(api/get 取 bg+front,api/check 验结果)");

    // ① 触发 demo 控件 → ② 可信点击验证按钮(JS .click 易盾不认,需 isTrusted)弹出挑战。
    let r = tab
        .run_js(TRIGGER_JS)
        .await
        .ok()
        .and_then(|v| v.as_str().map(String::from));
    println!("[yidun] 触发 = {r:?}");
    tokio::time::sleep(Duration::from_secs(2)).await;
    for s in ["css:.yidun_control", "css:.yidun_tips", "css:.yidun"] {
        if let Ok(el) = tab.ele(s).await
            && el.click().await.is_ok()
        {
            println!("[yidun] 可信点击验证按钮 {s}");
            break;
        }
    }
    tokio::time::sleep(Duration::from_secs(3)).await;

    let tries: u32 = std::env::var("YIDUN_TRIES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(3);
    let min_conf: f32 = std::env::var("YIDUN_MIN_CONF")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.30);
    let mut passed = false;

    for attempt in 1..=tries {
        println!("\n[yidun] ===== 第 {attempt}/{tries} 次 =====");

        // ① 监听取这一题:bg 干净图 URL + front 点击顺序(等到最新一题的 api/get)。
        let Some((bg_url, front)) = wait_challenge(&tab, Duration::from_secs(12)).await else {
            // 点选图没弹出来:首次"点击验证"那一下常被浏览器弹窗(如 Edge「使用 xxx 身份」账户提示)
            // 或窗口失焦挡掉 → 验证框压根没开(此时没有「换图」按钮)。**检测到没弹就再点一次验证按钮**
            // 重新唤起挑战;若验证框其实开着(只是这题没拿到),才退回「换图」。
            println!("[yidun] 未监听到 api/get(点选图未弹出);重新点击「点击验证」再触发…");
            if attempt < tries {
                if !retrigger_verify(&tab).await {
                    trusted_refresh(&tab).await;
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            continue;
        };
        let targets: Vec<String> = front
            .chars()
            .filter(|c| ('\u{4e00}'..='\u{9fff}').contains(c))
            .map(|c| c.to_string())
            .collect();
        let tips = tab
            .run_js(TIPS_JS)
            .await
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default();
        println!("[yidun] 接口 front=「{front}」→ 目标顺序 {targets:?}(DOM 提示「{tips}」核对)");
        println!("[yidun] 干净点选图 = {}", short(&bg_url, 78));

        // ② 服务端直拉干净图(避开浏览器跨域,无 UI 叠加)。
        let cap = match fetch_image(&bg_url).await {
            Ok(b) if b.len() > 1000 => b,
            other => {
                println!(
                    "[yidun] 拉取 bg 失败({:?});换图重试",
                    other.map(|b| b.len())
                );
                if attempt < tries {
                    trusted_refresh(&tab).await;
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
                continue;
            }
        };
        std::fs::write(out_dir.join("cap.jpg"), &cap).ok();

        // ④ 检测 + 逐框 OCR + 全局最优指派(干净图,无需排除工具栏)。显示 rect 稍后 hover 面板再读。
        let hits = cw.solve(&cap, &targets)?;
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

        let min_aff = hits
            .iter()
            .map(|h| h.affinity.max(h.template.unwrap_or(0.0)))
            .fold(f32::INFINITY, f32::min);
        let complete = !targets.is_empty() && hits.len() == targets.len();
        let final_try = attempt == tries;
        let want_click = complete && (min_aff >= min_conf || final_try);

        // ③ 触发式面板是 hover 触发:先 hover 验证条让面板**保持可见**,再读显示 rect(隐藏态 rect 不可靠)。
        let bar = tab
            .image_view(".yidun_control, .yidun_tips")
            .await
            .ok()
            .map(|b| (b.x + b.w / 2.0, b.y + b.h / 2.0));
        if let Some(b) = bar {
            tab.mouse_move(b.0, b.1).await.ok();
            tokio::time::sleep(Duration::from_millis(450)).await;
        }
        let view = tab.image_view(BG_SEL).await.ok();
        let mut mv = view.clone().unwrap_or_default();
        if let Ok(im) = image::load_from_memory(&cap) {
            mv.natural_w = im.width() as f64;
            mv.natural_h = im.height() as f64;
        }
        if let Ok(v) = tab.run_js(DUMP_VIS_JS).await
            && let Some(s) = v.as_str()
        {
            println!("[yidun] 点前可见性 = {s}");
        }
        println!(
            "[yidun] 命中 {}/{} · 最低置信度 {:.2}(阈值 {:.2})· rect 宽{:.0} 自然{:.0}x{:.0} scale={:.3} → {}",
            hits.len(),
            targets.len(),
            if hits.is_empty() { 0.0 } else { min_aff },
            min_conf,
            mv.w,
            mv.natural_w,
            mv.natural_h,
            mv.scale_x(),
            if want_click && mv.is_valid() {
                if min_aff >= min_conf {
                    "点击"
                } else {
                    "末次·按最佳猜测兜底点击"
                }
            } else {
                "置信不足/未集齐/面板不可见,换图重试"
            }
        );

        if want_click && mv.is_valid() {
            // 叠加图:把点击点(顺序 红/绿/蓝/橙)画到干净图上,直观看点在哪几个字。
            if let Ok(dimg) = image::load_from_memory(&cap) {
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
            // 图内像素 → **像素分数**(与平台/DPI/缩放无关)。点击时再用 `.yidun_bg-img` 的**实时 rect**
            // 还原页面坐标(rect.x + frac·rect.w),避开"一次性 rect 过期 / Win 缩放下 CSS↔设备 px 偏移"。
            let cap_w = mv.natural_w.max(1.0);
            let cap_h = mv.natural_h.max(1.0);
            let fracs: Vec<(f64, f64)> = hits
                .iter()
                .map(|h| (h.point.0 as f64 / cap_w, h.point.1 as f64 / cap_h))
                .collect();
            // 先 hover 验证条让 hover 面板出现/保持可见,再读**实时** rect(隐藏态 rect 不可靠)。
            let bar_pt = bar.unwrap_or((mv.x + mv.w / 2.0, mv.y + mv.h + 40.0));
            tab.mouse_move(bar_pt.0, bar_pt.1).await.ok();
            tokio::time::sleep(Duration::from_millis(450)).await;
            let points = live_points(&tab, BG_SEL, &fracs).await;
            if points.len() != hits.len() {
                println!("[yidun] 实时 rect 不可用(面板未展开?);换图重试");
                if attempt < tries {
                    trusted_refresh(&tab).await;
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
                continue;
            }
            for (h, &(cx, cy)) in hits.iter().zip(&points) {
                println!("[yidun] 拟人点击「{}」→ 页面({cx:.0},{cy:.0})", h.target);
            }
            // ★ 页面内可视红点 + 截图:直观确认"计划点击点"是否压在字上(排查偏移的铁证,留档 plan_*.png)。
            let arr: Vec<[f64; 2]> = points.iter().map(|&(x, y)| [x, y]).collect();
            let _ = tab
                .run_js(&format!(
                    "({MARK_JS})({})",
                    serde_json::to_string(&arr).unwrap_or_default()
                ))
                .await;
            if let Ok(shot) = tab.screenshot_bytes().await {
                std::fs::write(out_dir.join(format!("plan_{attempt}.png")), &shot).ok();
                println!("[yidun] 计划点击点截图(红点=将点击处)→ target/yidun/plan_{attempt}.png");
            }
            // 从验证条**连续移入**面板逐字点击,全程不离开控件(避免 hover 面板被收起 → 点到隐藏层不生效)。
            hover_click(&tab, bar_pt, &points).await?;
            tokio::time::sleep(Duration::from_secs(1)).await;
            if let Ok(v) = tab.run_js(MARKERS_JS).await
                && let Some(s) = v.as_str()
            {
                println!("[yidun] 点后状态 = {s}");
            }

            // ⑤ 铁证:监听 api/check 响应——点击若被易盾接收,会发起 check;否则根本不发。
            let chk = wait_check(&tab, Duration::from_secs(6)).await;
            match &chk {
                Some(b) => println!("[yidun] ✓ 捕获 check 响应(点击已被接收)= {}", short(b, 160)),
                None => println!("[yidun] ✗ 未捕获 check(点击未触发提交——可能没点在可点击层上)"),
            }
            let result_tip = tab
                .run_js(RESULT_JS)
                .await
                .ok()
                .and_then(|v| v.as_str().map(String::from))
                .unwrap_or_default();
            let shot = tab.screenshot_bytes().await?;
            std::fs::write(out_dir.join(format!("result_{attempt}.png")), &shot).ok();
            println!(
                "[yidun] 点击后提示=「{result_tip}」,结果截图 target/yidun/result_{attempt}.png"
            );

            let ok = result_tip.contains("成功")
                || chk
                    .as_deref()
                    .map(|b| b.contains("\"result\":true") || b.contains("验证成功"))
                    .unwrap_or(false);
            if ok {
                passed = true;
                break;
            }
            println!("[yidun] 未通过(易盾行为风控:字点准也可能判失败,与识别两件事)");
        }

        if attempt < tries {
            trusted_refresh(&tab).await;
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }

    tab.listen().stop().await.ok();
    println!(
        "\n[yidun] 最终:{}",
        if passed {
            "通过 ✓"
        } else {
            "未通过(监听取图 + 识别 + 全局指派链路已验证;行为风控 / 艺术字 OCR 为已知局限)"
        }
    );

    if !headless {
        tokio::time::sleep(Duration::from_secs(60)).await;
    }
    browser.quit().await?;
    Ok(())
}

/// 监听排空,取**最新一题**的 `api/get`:解析 JSONP → `(bg[0] URL, front 点击顺序)`。
/// 持续 `wait` 直到缓冲暂空且已拿到一题,或超时。换图后调用即拿到新题。
async fn wait_challenge(
    tab: &drission::cdp::ChromiumTab,
    timeout: Duration,
) -> Option<(String, String)> {
    let deadline = Instant::now() + timeout;
    let mut latest: Option<(String, String)> = None;
    loop {
        match tab.listen().wait(Some(Duration::from_millis(300))).await {
            Ok(Some(p)) => {
                if p.url.contains("/get") {
                    if let Some(c) = parse_yidun_get(&p.response.body) {
                        latest = Some(c); // 保留最新一题
                    }
                }
            }
            Ok(None) => {
                if latest.is_some() || Instant::now() >= deadline {
                    return latest;
                }
            }
            Err(_) => return latest,
        }
        if Instant::now() >= deadline {
            return latest;
        }
    }
}

/// 监听排空,取点击后 `api/check` 的响应体(点击被接收才会发起)。超时返回 `None`。
async fn wait_check(tab: &drission::cdp::ChromiumTab, timeout: Duration) -> Option<String> {
    let deadline = Instant::now() + timeout;
    loop {
        match tab.listen().wait(Some(Duration::from_millis(300))).await {
            Ok(Some(p)) => {
                if p.url.contains("/check") {
                    return Some(p.response.body.clone());
                }
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    return None;
                }
            }
            Err(_) => return None,
        }
        if Instant::now() >= deadline {
            return None;
        }
    }
}

/// **重新唤起挑战**(点选图没弹出时):先跑一遍 demo 触发器(验证条可能整个没出来),
/// 再**可信点击**验证按钮(「点击按钮进行验证」`.yidun_control` / `.yidun_intelli-icon` 等)。
/// 应对首次点击被浏览器弹窗(Edge 账户提示等)或窗口失焦挡掉 → 挑战没弹的情况。返回是否点到了按钮。
async fn retrigger_verify(tab: &drission::cdp::ChromiumTab) -> bool {
    // 验证条可能根本没出来:先重跑 demo 触发器把它唤出。
    let _ = tab.run_js(TRIGGER_JS).await;
    tokio::time::sleep(Duration::from_millis(600)).await;
    for s in [
        "css:.yidun_control",
        "css:.yidun_tips",
        "css:.yidun_intelli-icon",
        "css:.yidun",
    ] {
        if let Ok(el) = tab.ele(s).await
            && el.click().await.is_ok()
        {
            println!("[yidun] 重新点击验证按钮 {s}(再触发点选图)");
            return true;
        }
    }
    println!("[yidun] 未找到可点的验证按钮(验证条未加载?)");
    false
}

/// 换图:优先**可信点击**易盾刷新键(真 `<button>`),兜底 JS click。
async fn trusted_refresh(tab: &drission::cdp::ChromiumTab) {
    for s in [
        "css:.yidun_refresh",
        "css:.yidun-refresh",
        "css:.yidun_panel-refresh",
    ] {
        if let Ok(el) = tab.ele(s).await
            && el.click().await.is_ok()
        {
            println!("[yidun] 换图(可信点击 {s})");
            return;
        }
    }
    let r = tab
        .run_js(REFRESH_JS)
        .await
        .ok()
        .and_then(|v| v.as_str().map(String::from));
    println!("[yidun] 换图(JS)= {r:?}");
}

/// std-only xorshift,给 hover 轨迹加手抖/变速。
fn nextf(seed: &mut u64) -> f64 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 7;
    *seed ^= *seed << 17;
    (*seed >> 11) as f64 / (1u64 << 53) as f64
}

/// 触发式点选:`bar` = 验证条中心(hover 触发面板),`points` = 各字页面坐标。
/// 先 hover 验证条让面板出现/保持,再**从验证条连续移入**面板逐字点击(minimum-jerk 变速 + 手抖),
/// 全程不离开控件——否则一旦 mouseleave,hover 面板收起,点击落到隐藏层不被易盾接收。
async fn hover_click(
    tab: &drission::cdp::ChromiumTab,
    bar: (f64, f64),
    points: &[(f64, f64)],
) -> drission::Result<()> {
    let mut seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9E37_79B9_7F4A_7C15)
        | 1;
    // hover 验证条 → 面板出现/保持可见。
    tab.mouse_move(bar.0, bar.1).await?;
    tokio::time::sleep(Duration::from_millis(450)).await;
    let mut cur = bar;
    for &p in points {
        let steps = 30;
        for i in 1..=steps {
            let t = i as f64 / steps as f64;
            let s = 10.0 * t.powi(3) - 15.0 * t.powi(4) + 6.0 * t.powi(5); // minimum-jerk
            let x = cur.0 + (p.0 - cur.0) * s + (nextf(&mut seed) - 0.5) * 1.2;
            let y = cur.1 + (p.1 - cur.1) * s + (nextf(&mut seed) - 0.5) * 1.2;
            // fire 快路径(不等往返)→ 密集采样,逼近真人;节奏交给 sleep。
            tab.mouse_move_fast(x, y)?;
            tokio::time::sleep(Duration::from_millis(8 + (nextf(&mut seed) * 8.0) as u64)).await;
        }
        tokio::time::sleep(Duration::from_millis(70 + (nextf(&mut seed) * 60.0) as u64)).await;
        tab.mouse_down(p.0, p.1).await?;
        tokio::time::sleep(Duration::from_millis(45 + (nextf(&mut seed) * 45.0) as u64)).await;
        tab.mouse_up(p.0, p.1).await?;
        cur = p;
        tokio::time::sleep(Duration::from_millis(
            180 + (nextf(&mut seed) * 160.0) as u64,
        ))
        .await;
    }
    Ok(())
}

/// 用图内像素**分数** × 元素**实时** rect 求页面点(避开一次性 rect 过期 / Win 缩放下 CSS↔设备 px 偏移)。
/// `fracs` 为各点的 `(x/自然宽, y/自然高)`;读不到有效 rect(宽 ≤ 1)返回空切片。
async fn live_points(
    tab: &drission::cdp::ChromiumTab,
    sel: &str,
    fracs: &[(f64, f64)],
) -> Vec<(f64, f64)> {
    let Ok(view) = tab.image_view(sel).await else {
        return Vec::new();
    };
    if view.w <= 1.0 {
        return Vec::new();
    }
    fracs
        .iter()
        .map(|&(fx, fy)| view.clamp_point((view.x + fx * view.w, view.y + fy * view.h)))
        .collect()
}

/// 解析易盾 `api/get` 的 JSONP 响应体 → `(bg 图 URL, front 点击顺序文本)`。
/// 形如 `__JSONP_xxx({"data":{"bg":["https://…jpg", …],"front":"全验体",…},…});`。
fn parse_yidun_get(body: &str) -> Option<(String, String)> {
    let js = json_slice(body)?;
    let v: serde_json::Value = serde_json::from_str(js).ok()?;
    let data = &v["data"];
    let bg = data["bg"]
        .get(0)
        .and_then(|x| x.as_str())
        .or_else(|| data["bg"].as_str())?;
    if bg.is_empty() {
        return None;
    }
    let front = data["front"].as_str().unwrap_or("").to_string();
    Some((bg.to_string(), front))
}

/// 从 JSONP/含包裹的文本里截出第一个 `{` 到最后一个 `}` 的 JSON 子串。
fn json_slice(s: &str) -> Option<&str> {
    let a = s.find('{')?;
    let b = s.rfind('}')?;
    (b >= a).then(|| &s[a..=b])
}

/// 截断长字符串(URL/响应体)便于打印。
fn short(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// 在图上画空心圆环 + 中心点(点击点叠加图用)。
fn draw_marker(img: &mut image::RgbaImage, cx: i32, cy: i32, color: [u8; 4]) {
    let (w, h) = (img.width() as i32, img.height() as i32);
    let put = |img: &mut image::RgbaImage, x: i32, y: i32| {
        if x >= 0 && y >= 0 && x < w && y < h {
            img.put_pixel(x as u32, y as u32, image::Rgba(color));
        }
    };
    let r = 13i32;
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
    for dy in -1..=1 {
        for dx in -1..=1 {
            put(img, cx + dx, cy + dy);
        }
    }
}
