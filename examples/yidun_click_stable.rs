//! 易盾「文字点选」验证码 **稳定版**:在 `yidun_click` 的「监听取图 → det → 逐框 OCR → 全局指派 →
//! 拟人点击 → check 铁证」链路上,专修两个老毛病(用户实测踩坑):
//!
//! 1. **点选图不稳定弹出**(老版会「hover 掉」):老版触发后**不确认图是否真显示**就进入耗时几秒的
//!    OCR,等去点时面板早收起了(弹窗没真打开 / hover 态丢失 / 挑战在 iframe 里读不到 rect)→ 读到
//!    rect 宽≈0 → 误判。本版**先轮询确认点选图真显示**(顶层 + iframe 兜底,期间持续 hover 验证条**保活**、
//!    必要时**重新触发**),确认显示才往下走;OCR 之后、点击之前再做一次「仍显示 & 没换图」**同步校验**。
//!
//! 2. **误判「过不了易盾」**:老版只有「通过 / 未通过」二元结论,把「图根本没弹出来」「中途换了图」
//!    「没点在可点击层(check 没发)」全归成「过不了易盾」——**完全错误的结论**。本版用显式
//!    [`Outcome`] 分类,把「没出图(触发/环境问题)」「识别置信不足」「点击未被接收」与「点击已提交但
//!    被易盾判失败(真·没过)」**严格区分**,最终结论诚实——只有「点击已提交→被拒」才算真没过。
//!
//! **识别增强(字形样本库·自增长)**:启动时加载真样本字形库 `bank/{字}/*.png`(融合进 OCR 第二信号,
//! 见 `src/ocr/glyph.rs` 的 `SampleBank`);每**过一次盾**就把验证正确的字图自动落盘进同一个 bank
//! (`ClickWord::harvest_verified`,标签已验证、零人工)——库越跑越厚、识别越跑越准,破里程碑 59 的「数据墙」。
//!
//! 案例:https://dun.163.com/trial/picture-click。
//! 运行:`cargo run --example yidun_click_stable --features cdp,ocr`(默认有头;`HL=1` 无头)。
//! 攒样本(连续采样、过盾不停):`YIDUN_HARVEST=1 YIDUN_TRIES=50 HL=1 cargo run --example yidun_click_stable --features cdp,ocr`。
//! 产物写到 `target/yidun/`(cap.jpg / overlay_*.png / plan_*.png / result_*.png)。
//! 可调环境变量:`YIDUN_TRIES`(默认 3)、`YIDUN_MIN_CONF`(默认 0.30)、`YIDUN_OUT`、`YIDUN_PROFILE`、`YIDUN_DIAG`;
//! 样本库:`YIDUN_BANK`/`DRISSION_GLYPH_SAMPLES`(默认 `yidun_samples/bank`)、`YIDUN_HARVEST`(连续采样模式)。

use std::time::{Duration, Instant};

use drission::cdp::ChromiumTab;
use drission::ocr::ClickWord;
use drission::prelude::*;

const URL: &str = "https://dun.163.com/trial/picture-click";

// 触发验证码:点页面里像「验证 / 体验 / 点击按钮」的候选(易盾试用 demo 触发器)。
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

// 换图(换一题):点易盾刷新键(兜底 JS click)。
const REFRESH_JS: &str = r#"(() => {
  for (const s of ['.yidun_refresh', '.yidun-refresh', '.yidun_panel-refresh']) {
    const e = document.querySelector(s);
    if (e && e.offsetParent !== null) { e.click(); return 'refreshed:' + s; }
  }
  return 'no-refresh';
})()"#;

// 点击后读结果提示。
const RESULT_JS: &str = r#"(()=>{const e=document.querySelector('.yidun_tips__text');return e?e.innerText.trim():'';})()"#;

// ★ 点选图「是否真显示」探针(顶层文档):返回 rect / 自然尺寸 / src / 提示文字 / iframe 数。
//   vis = offsetParent 在 + 非 hidden + opacity>0.01 + 宽>2(综合判定「图真画出来了」而非仅存在于 DOM)。
const BG_PROBE_JS: &str = r#"(()=>{
  const SEL='.yidun_bg-img,img.yidun_bg-img,.yidun_bgimg';
  const e=document.querySelector(SEL);
  const tipEl=document.querySelector('.yidun_tips__text,.yidun_tips__answer,.yidun_tips');
  const tip=tipEl?tipEl.innerText.trim():'';
  const ifr=document.querySelectorAll('iframe').length;
  const out={top:false,vis:false,x:0,y:0,w:0,h:0,nw:0,nh:0,src:'',tip,iframes:ifr};
  if(e){const b=e.getBoundingClientRect();const s=getComputedStyle(e);
    out.top=true;
    out.vis=e.offsetParent!==null&&s.visibility!=='hidden'&&parseFloat(s.opacity)>0.01&&b.width>2;
    out.x=b.x;out.y=b.y;out.w=b.width;out.h=b.height;
    out.nw=e.naturalWidth||0;out.nh=e.naturalHeight||0;
    out.src=(e.currentSrc||e.src||'').slice(-80);}
  return JSON.stringify(out);
})()"#;

// 同上,但在 **iframe 内**执行(只返回元素的帧内 rect / 自然尺寸 / src);绝对坐标 = iframe 偏移 + 帧内 rect。
const IN_FRAME_BG_JS: &str = r#"(()=>{
  const SEL='.yidun_bg-img,img.yidun_bg-img,.yidun_bgimg';
  const e=document.querySelector(SEL);
  const out={vis:false,x:0,y:0,w:0,h:0,nw:0,nh:0,src:''};
  if(e){const b=e.getBoundingClientRect();const s=getComputedStyle(e);
    out.vis=e.offsetParent!==null&&s.visibility!=='hidden'&&parseFloat(s.opacity)>0.01&&b.width>2;
    out.x=b.x;out.y=b.y;out.w=b.width;out.h=b.height;
    out.nw=e.naturalWidth||0;out.nh=e.naturalHeight||0;
    out.src=(e.currentSrc||e.src||'').slice(-80);}
  return JSON.stringify(out);
})()"#;

// 环境诊断(没出图时排根因):聚焦 / 可见性 / webdriver / 各 yidun 节点可见性 / bg 加载状态。
const DIAG_JS: &str = r#"(()=>{
  const vis=(s)=>{const e=document.querySelector(s);if(!e)return 'none';
    const b=e.getBoundingClientRect();const st=getComputedStyle(e);
    const v=e.offsetParent!==null&&st.visibility!=='hidden'&&parseFloat(st.opacity)>0.01;
    return (v?'vis':'hid')+'['+Math.round(b.width)+'x'+Math.round(b.height)+']';};
  return JSON.stringify({
    focus:document.hasFocus(), visState:document.visibilityState, wd:navigator.webdriver,
    iframes:document.querySelectorAll('iframe').length,
    control:vis('.yidun_control'), tips:vis('.yidun_tips'), bg:vis('.yidun_bg-img'),
    popup:vis('.yidun_popup'), modal:vis('.yidun_modal'), intelli:vis('.yidun_intelli-icon')
  });
})()"#;

// 在页面上画「将点击处」红点(position:fixed ⇒ 视口坐标;pointer-events:none 不挡真点击),配合 plan_*.png 排查偏移。
const MARK_JS: &str = r#"((pts)=>{document.querySelectorAll('.__ymk').forEach(e=>e.remove());
for(const p of pts){const d=document.createElement('div');d.className='__ymk';
d.style.cssText='position:fixed;left:'+(p[0]-7)+'px;top:'+(p[1]-7)+'px;width:14px;height:14px;border-radius:50%;'
+'background:rgba(255,0,0,.55);border:2px solid #fff;box-shadow:0 0 4px #000;z-index:2147483647;pointer-events:none';
document.body.appendChild(d);}return pts.length;})"#;

/// 单轮(或最终)的**诚实分类**——把「没过」拆开,杜绝把「图没弹出来」误报成「过不了易盾」。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Outcome {
    /// 通过:check 回 result:true / 提示「成功」(含未出图就智能通过)。
    Passed,
    /// **真·没过**:点击已提交(check 已发),但被易盾判失败(行为风控 / 识别错)。
    Rejected,
    /// 点了但没触发 check:没点在可点击层(面板收起 / 坐标 / iframe 边界)——**不等于过不了易盾**。
    ClickNotSubmitted,
    /// 识别置信不足、不敢下点——识别问题,**不等于过不了易盾**。
    LowConfidence,
    /// 点选图弹了但不稳定(中途换图 / rect 失效)——**不等于过不了易盾**。
    ImageUnstable,
    /// 点选图**始终没弹出来**(触发 / 焦点 / 环境问题)——**绝不等于过不了易盾**。
    ImageNeverShown,
}

/// 确认点选图是否真显示的结果。
enum Confirm {
    Shown,
    SilentPass,
    NotShown,
}

/// 点选图的**绝对页面视图**(无论顶层还是 iframe 内,坐标已折算成视口坐标,可直接喂 CDP 点击)。
#[derive(Clone, Default)]
struct BgView {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    src: String,
    visible: bool,
    in_iframe: bool,
}

impl BgView {
    /// 图内像素**分数**(0–1)→ 绝对页面坐标,并钳制到本图 rect 内(绝不点到图外工具栏)。
    fn map_frac(&self, fx: f64, fy: f64) -> (f64, f64) {
        let x = (self.x + fx * self.w).clamp(self.x, self.x + self.w.max(0.0));
        let y = (self.y + fy * self.h).clamp(self.y, self.y + self.h.max(0.0));
        (x, y)
    }
}

#[tokio::main]
async fn main() -> drission::Result<()> {
    let headless = matches!(std::env::var("HL").ok().as_deref(), Some("1") | Some("true"));
    let out_dir = std::env::var_os("YIDUN_OUT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("target")
                .join("yidun")
        });
    std::fs::create_dir_all(&out_dir).ok();

    println!("[yidun] 加载 det+ocr 模型…");
    let mut cw = ClickWord::new().await?;
    println!("[yidun] 模型就绪 ✓");

    // ★ 识别增强·字形样本库:统一「使用 + 采样」到同一个 bank 目录(优先级 YIDUN_BANK >
    //   DRISSION_GLYPH_SAMPLES > 默认 yidun_samples/bank)。过盾时把验证正确的字图追加进来,越跑越厚。
    let bank_dir = std::env::var_os("YIDUN_BANK")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("DRISSION_GLYPH_SAMPLES").map(std::path::PathBuf::from))
        .unwrap_or_else(|| {
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("yidun_samples")
                .join("bank")
        });
    cw.reload_sample_bank(&bank_dir);
    let harvest_mode = std::env::var("YIDUN_HARVEST").is_ok();
    println!(
        "[yidun] 字形样本库 = {}(现 {} 张{}){}",
        bank_dir.display(),
        cw.bank.as_ref().map(|b| b.len()).unwrap_or(0),
        if cw.bank.is_some() {
            ""
        } else {
            ",空/未建,退化渲染字体+纯OCR"
        },
        if harvest_mode {
            " · HARVEST 采样模式(过盾不停、持续攒样本)"
        } else {
            " · 过盾即采样(自增长)"
        }
    );
    let mut harvested_total = 0usize;

    // 强制 device-scale=1:令 CSS px == 设备 px,读到的 rect 与点击坐标一一对应(跨 mac/Win 一致)。
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

    // ★ 触发前就开监听(确保抓到第一题)。只过滤 `c.dun.163.com/api/*`(get/check),噪声最小。
    tab.listen().start(&["dun.163.com/api"]).await?;
    println!("[yidun] 监听 c.dun.163.com/api/*(get 取 bg+front、check 验结果)");

    // 首次触发(后续每轮在 open_and_confirm 内按需重触发)。
    let r = tab
        .run_js(TRIGGER_JS)
        .await
        .ok()
        .and_then(|v| v.as_str().map(String::from));
    println!("[yidun] 首次触发 = {r:?}");
    tokio::time::sleep(Duration::from_secs(1)).await;

    let tries: u32 = env_u32("YIDUN_TRIES", 3);
    let min_conf: f32 = std::env::var("YIDUN_MIN_CONF")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.30);

    let mut outcomes: Vec<Outcome> = Vec::new();

    for attempt in 1..=tries {
        println!("\n[yidun] ===== 第 {attempt}/{tries} 次 =====");

        // ① 打开并**确认点选图真显示**(带 hover 保活 + 重触发);确认到了才往下走。
        match open_and_confirm(&tab, attempt == 1).await {
            Confirm::SilentPass => {
                println!("[yidun] ✓ 智能通过(未弹点选图即过——这算过,不是没出图)");
                outcomes.push(Outcome::Passed);
                break;
            }
            Confirm::NotShown => {
                println!(
                    "[yidun] ⚠ 点选图始终未弹出(触发/焦点/环境问题)——记为「未出图」,**不是**「过不了易盾」"
                );
                dump_diag(&tab, &format!("第{attempt}次未出图")).await;
                outcomes.push(Outcome::ImageNeverShown);
                if attempt < tries {
                    trusted_refresh(&tab).await;
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
                continue;
            }
            Confirm::Shown => {}
        }

        // ② 取本轮 api/get(bg 干净图 URL + front 点击顺序)。
        //    若没抓到:多半是「页面加载即渲染的首图,其 get 早于监听启动」→ 此刻面板已开,**当场换一次图**
        //    逼出一条可抓的 get,再等一次(就地兜住,不白白消耗一次尝试)。
        let challenge = match wait_challenge(&tab, Duration::from_secs(8)).await {
            Some(c) => Some(c),
            None => {
                println!("[yidun] 图在但未抓到 api/get(首图 get 早于监听?)——当场换图逼出可抓的 get,再等一次…");
                trusted_refresh(&tab).await;
                tokio::time::sleep(Duration::from_millis(800)).await;
                let _ = open_and_confirm(&tab, false).await;
                wait_challenge(&tab, Duration::from_secs(8)).await
            }
        };
        let Some((bg_url, front)) = challenge else {
            println!("[yidun] 换图后仍未抓到 api/get——记为「不稳定」,刷新重试(**不是**过不了易盾)");
            outcomes.push(Outcome::ImageUnstable);
            if attempt < tries {
                trusted_refresh(&tab).await;
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            continue;
        };
        let targets: Vec<String> = front
            .chars()
            .filter(|c| ('\u{4e00}'..='\u{9fff}').contains(c))
            .map(|c| c.to_string())
            .collect();
        println!("[yidun] 接口 front=「{front}」→ 目标顺序 {targets:?}");
        println!("[yidun] 干净点选图 = {}", short(&bg_url, 78));

        // 记录「确认显示时」的图源指纹(src),供点击前比对是否中途换了图。
        let v_confirm = resolve_bg_view(&tab).await;
        let src_at_ocr = v_confirm.src.clone();

        // ③ 服务端直拉干净图(避开浏览器跨域、无 UI 叠加)。
        let cap = match fetch_image(&bg_url).await {
            Ok(b) if b.len() > 1000 => b,
            other => {
                println!("[yidun] 拉取 bg 失败({:?})——记为「不稳定」,刷新重试", other.map(|b| b.len()));
                outcomes.push(Outcome::ImageUnstable);
                if attempt < tries {
                    trusted_refresh(&tab).await;
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
                continue;
            }
        };
        std::fs::write(out_dir.join("cap.jpg"), &cap).ok();
        let (cap_w, cap_h) = match image::load_from_memory(&cap) {
            Ok(im) => (im.width() as f64, im.height() as f64),
            Err(_) => (1.0, 1.0),
        };

        // ④ 识别:OCR 前把鼠标**停在验证条上保活**(OCR 是阻塞 CPU,期间鼠标不动 ⇒ :hover 不丢、面板不收起)。
        let control_pt = control_point(&tab).await;
        if let Some(c) = control_pt {
            tab.mouse_move(c.0, c.1).await.ok();
        }
        // ★ 排除右上角刷新/语音/反馈工具栏区域(按 SKILL 5.3:`solve_excluding`)。干净源图本身没有该
        //   工具栏,但**真机点击坐标会落在浏览器叠加的工具栏上**——一旦把某目标误指派到右上角,点下去
        //   就是「刷新换图 / 切语音」(用户实测踩坑)。从源头丢弃落在该区的检测框,绝不点到刷新。
        let exclude = vec![BBox {
            x1: (cap_w * 0.66) as u32,
            y1: 0,
            x2: cap_w as u32,
            y2: (cap_h * 0.22) as u32,
            score: 0.0,
        }];
        let hits = cw.solve_excluding(&cap, &targets, &exclude)?;
        for h in &hits {
            let tpl = h.template.map(|t| format!(" tpl={t:.2}")).unwrap_or_default();
            println!("[yidun]   「{}」 aff={:.2}{tpl} 图内点({},{})", h.target, h.affinity, h.point.0, h.point.1);
        }
        let min_aff = hits
            .iter()
            .map(|h| h.affinity.max(h.template.unwrap_or(0.0)))
            .fold(f32::INFINITY, f32::min);
        let complete = !targets.is_empty() && hits.len() == targets.len();
        let final_try = attempt == tries;

        // 置信度门:没集齐 / 置信太低且非末次 → 记「识别不足」(识别问题,非过不了易盾),换图重试。
        if !complete || (min_aff < min_conf && !final_try) {
            println!(
                "[yidun] 命中 {}/{} · 最低置信 {:.2}(阈值 {:.2})→ 识别不足,换图重试(**不是**过不了易盾)",
                hits.len(),
                targets.len(),
                if hits.is_empty() { 0.0 } else { min_aff },
                min_conf
            );
            outcomes.push(Outcome::LowConfidence);
            if attempt < tries {
                trusted_refresh(&tab).await;
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            continue;
        }

        // ⑤ **点击前同步校验**:重新 hover 保活 + 读实时 rect;图收起就重开,换了图就别点旧坐标。
        if let Some(c) = control_pt {
            tab.mouse_move(c.0, c.1).await.ok();
            tokio::time::sleep(Duration::from_millis(450)).await;
        }
        let mut view = resolve_bg_view(&tab).await;
        if !view.visible {
            // 收起了:再 hover 一次尝试重开。
            if let Some(c) = control_pt {
                tab.mouse_move(c.0, c.1).await.ok();
                tokio::time::sleep(Duration::from_millis(600)).await;
            }
            view = resolve_bg_view(&tab).await;
        }
        if !view.visible {
            println!("[yidun] 点击前点选图已收起且无法稳定重开——记为「不稳定」,刷新重试");
            outcomes.push(Outcome::ImageUnstable);
            if attempt < tries {
                trusted_refresh(&tab).await;
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            continue;
        }
        if !src_at_ocr.is_empty() && !view.src.is_empty() && view.src != src_at_ocr {
            println!("[yidun] 点击前发现图已被换掉(src 变了)——绝不点旧坐标,记「不稳定」刷新重试");
            outcomes.push(Outcome::ImageUnstable);
            if attempt < tries {
                trusted_refresh(&tab).await;
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            continue;
        }
        println!(
            "[yidun] 点击前确认:图仍显示{} rect[{:.0},{:.0} {:.0}x{:.0}]",
            if view.in_iframe { "(iframe 内)" } else { "" },
            view.x,
            view.y,
            view.w,
            view.h
        );

        // ⑥ 图内像素分数 → 绝对页面点(用实时 rect,跨平台/缩放一致)。
        let fracs: Vec<(f64, f64)> = hits
            .iter()
            .map(|h| (h.point.0 as f64 / cap_w.max(1.0), h.point.1 as f64 / cap_h.max(1.0)))
            .collect();
        let points: Vec<(f64, f64)> = fracs.iter().map(|&(fx, fy)| view.map_frac(fx, fy)).collect();
        for (h, &(cx, cy)) in hits.iter().zip(&points) {
            println!("[yidun] 拟人点击「{}」→ 页面({cx:.0},{cy:.0})", h.target);
        }

        // 叠加图 + 计划点截图(留档,排查偏移)。
        save_overlay(&cap, &hits, &out_dir, attempt);
        let arr: Vec<[f64; 2]> = points.iter().map(|&(x, y)| [x, y]).collect();
        let _ = tab
            .run_js(&format!("({MARK_JS})({})", serde_json::to_string(&arr).unwrap_or_default()))
            .await;
        if let Ok(shot) = tab.screenshot_bytes().await {
            std::fs::write(out_dir.join(format!("plan_{attempt}.png")), &shot).ok();
        }

        // ⑦ **拟人轨迹点击**:用库自带 `Humanize::human_click`(点间走二次贝塞尔 + minimum-jerk 变速
        //    + 手抖 + 落点微停,产生密集 mousemove,击穿行为风控)。先 hover 验证条让(触发式)面板维持,
        //    再走轨迹——轨迹全程落在图区内,不会经过/点到右上角刷新键。
        if let Some(c) = control_pt {
            tab.mouse_move(c.0, c.1).await.ok();
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
        tab.human_click(&points).await?;
        tokio::time::sleep(Duration::from_secs(1)).await;

        // ⑧ 铁证:监听 api/check——点击被易盾接收才会发起 check;否则根本不发。
        let chk = wait_check(&tab, Duration::from_secs(6)).await;
        let result_tip = tab
            .run_js(RESULT_JS)
            .await
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default();
        if let Ok(shot) = tab.screenshot_bytes().await {
            std::fs::write(out_dir.join(format!("result_{attempt}.png")), &shot).ok();
        }

        match &chk {
            None => {
                println!("[yidun] ✗ 未捕获 check(点击未被接收——没点在可点击层)——**不是**「过不了易盾」");
                outcomes.push(Outcome::ClickNotSubmitted);
            }
            Some(b) => {
                let ok = result_tip.contains("成功")
                    || b.contains("\"result\":true")
                    || b.contains("验证成功");
                if ok {
                    println!("[yidun] ✓ check 通过 = {}", short(b, 140));
                    outcomes.push(Outcome::Passed);
                    // ★「过盾即验真」自动采样:check 回 true ⇒ 各命中框里的字就是其 target(点击序与
                    //   front 一致且被服务端判过),把这些字图按 {字}/ 落盘成**标签已验证**的真样本,
                    //   并即时重载样本库,后续题立刻用上——零人工破「数据墙」(里程碑 59)。
                    match cw.harvest_verified(&cap, &hits, &bank_dir) {
                        Ok(n) if n > 0 => {
                            harvested_total += n;
                            cw.reload_sample_bank(&bank_dir);
                            println!(
                                "[yidun] 采样 +{n} 张已验证真样本 → bank 现 {} 张(本轮累计 +{harvested_total})",
                                cw.bank.as_ref().map(|b| b.len()).unwrap_or(0)
                            );
                        }
                        Ok(_) => println!("[yidun] 采样:本题字图已在库中(去重跳过)"),
                        Err(e) => println!("[yidun] 采样失败(不影响过盾结论):{e}"),
                    }
                    // 采样模式:不停在首过,继续换图多采几轮;普通模式过了即收。
                    if harvest_mode {
                        if attempt < tries {
                            trusted_refresh(&tab).await;
                            tokio::time::sleep(Duration::from_secs(2)).await;
                        }
                        continue;
                    }
                    break;
                }
                println!(
                    "[yidun] ✗ 点击已提交但被易盾判失败(check={})——这才算**真·没过**(行为风控/识别)",
                    short(b, 140)
                );
                outcomes.push(Outcome::Rejected);
            }
        }

        if attempt < tries {
            trusted_refresh(&tab).await;
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }

    tab.listen().stop().await.ok();
    print_verdict(&outcomes);
    if harvested_total > 0 {
        println!(
            "[yidun] 本轮共采集 {harvested_total} 张已验证真样本 → {}(下次启动自动加载,识别越跑越准)",
            bank_dir.display()
        );
    }

    if !headless {
        tokio::time::sleep(Duration::from_secs(30)).await;
    }
    browser.quit().await?;
    Ok(())
}

/// **打开并确认点选图真显示**(本版核心修复):反复 hover 验证条**保活** + 必要时重触发,轮询
/// [`BG_PROBE_JS`] / iframe 探针,直到①图真显示(`Shown`)②或检测到智能通过(`SilentPass`)
/// ③或超时仍没出图(`NotShown`)。确认显示才返回 —— 杜绝「没确认就 OCR、点时面板已收起」。
async fn open_and_confirm(tab: &ChromiumTab, first: bool) -> Confirm {
    // 没出图时的重触发预算:首轮多给(页面刚加载),后续每轮也会重触发一次。
    let deadline = Instant::now() + Duration::from_secs(if first { 14 } else { 10 });
    let mut next_trigger = Instant::now();
    loop {
        // ★ 先判「已显示 / 已智能通过」——已弹出就**绝不再点验证条**(避免把已开的弹窗点关、
        //   或重发 api/get 把当前题冲掉)。这一步顺序很关键:老版先触发后判,会打断已开的挑战。
        if let Ok(v) = tab.run_js(BG_PROBE_JS).await
            && let Some(s) = v.as_str()
            && let Ok(j) = serde_json::from_str::<serde_json::Value>(s)
        {
            let tip = j["tip"].as_str().unwrap_or("");
            if tip.contains("成功") || tip.contains("通过") {
                return Confirm::SilentPass;
            }
        }
        let view = resolve_bg_view(tab).await;
        if view.visible {
            println!(
                "[yidun] 点选图已稳定显示{} rect[{:.0}x{:.0}]",
                if view.in_iframe { "(iframe 内)" } else { "" },
                view.w,
                view.h
            );
            return Confirm::Shown;
        }

        // 还没出图 → 周期性重新触发 + 可信点击验证条把挑战唤出来。
        if Instant::now() >= next_trigger {
            let _ = tab.run_js(TRIGGER_JS).await;
            tokio::time::sleep(Duration::from_millis(400)).await;
            for s in ["css:.yidun_control", "css:.yidun_tips", "css:.yidun_intelli-icon", "css:.yidun"] {
                if let Ok(el) = tab.ele(s).await
                    && el.click().await.is_ok()
                {
                    break;
                }
            }
            next_trigger = Instant::now() + Duration::from_secs(4);
        }

        // hover 验证条**保活**(触发式面板靠 hover 维持;弹窗式无害)。
        if let Some(c) = control_point(tab).await {
            tab.mouse_move(c.0, c.1).await.ok();
        }
        tokio::time::sleep(Duration::from_millis(350)).await;

        if Instant::now() >= deadline {
            return Confirm::NotShown;
        }
    }
}

/// 解析点选图的**绝对页面视图**:先查顶层文档([`BG_PROBE_JS`]);顶层没有再遍历 iframe,在帧内读
/// rect 后**叠加 iframe 元素的视口偏移**得到绝对坐标(CDP 点击用视口坐标,跨 iframe 边界有效)。
async fn resolve_bg_view(tab: &ChromiumTab) -> BgView {
    // 顶层文档。
    if let Ok(v) = tab.run_js(BG_PROBE_JS).await
        && let Some(s) = v.as_str()
        && let Ok(j) = serde_json::from_str::<serde_json::Value>(s)
        && j["top"].as_bool().unwrap_or(false)
        && j["vis"].as_bool().unwrap_or(false)
    {
        return BgView {
            x: jf(&j, "x"),
            y: jf(&j, "y"),
            w: jf(&j, "w"),
            h: jf(&j, "h"),
            src: j["src"].as_str().unwrap_or("").to_string(),
            visible: true,
            in_iframe: false,
        };
    }

    // iframe 兜底:挑战常被塞进 iframe,顶层 querySelector 读不到 → 老版误判「面板未展开」。
    for sel in [
        "css:iframe.yidun_iframe",
        "css:iframe[src*='dun.163']",
        "css:iframe[src*='captcha']",
        "css:iframe[src*='yidun']",
        "css:iframe",
    ] {
        let Ok(ifr) = tab.ele(sel).await else { continue };
        let Ok(rect) = ifr.rect().await else { continue };
        let Ok(frame) = ifr.content_frame().await else {
            continue;
        };
        if let Ok(v) = frame.run_js(IN_FRAME_BG_JS).await
            && let Some(s) = v.as_str()
            && let Ok(j) = serde_json::from_str::<serde_json::Value>(s)
            && j["vis"].as_bool().unwrap_or(false)
        {
            return BgView {
                x: rect.viewport_x + jf(&j, "x"),
                y: rect.viewport_y + jf(&j, "y"),
                w: jf(&j, "w"),
                h: jf(&j, "h"),
                src: j["src"].as_str().unwrap_or("").to_string(),
                visible: true,
                in_iframe: true,
            };
        }
    }

    BgView::default()
}

/// 验证条中心(hover 保活 / 点击起点用);取 `.yidun_control` 或 `.yidun_tips` 的显示 rect 中心。
async fn control_point(tab: &ChromiumTab) -> Option<(f64, f64)> {
    tab.image_view(".yidun_control, .yidun_tips")
        .await
        .ok()
        .filter(|b| b.w > 1.0)
        .map(|b| (b.x + b.w / 2.0, b.y + b.h / 2.0))
}

/// 监听排空,取**最新一题**的 `api/get` → `(bg[0] URL, front 点击顺序)`。
async fn wait_challenge(tab: &ChromiumTab, timeout: Duration) -> Option<(String, String)> {
    let deadline = Instant::now() + timeout;
    let mut latest: Option<(String, String)> = None;
    let diag = std::env::var("YIDUN_DIAG").is_ok();
    loop {
        match tab.listen().wait(Some(Duration::from_millis(300))).await {
            Ok(Some(p)) => {
                if diag {
                    println!("[yidun]   监听包 {} status={}", short(&p.url, 60), p.response.status);
                }
                if p.url.contains("/get")
                    && let Some(c) = parse_yidun_get(&p.response.body)
                {
                    latest = Some(c);
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
async fn wait_check(tab: &ChromiumTab, timeout: Duration) -> Option<String> {
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

/// 采样模式取「新题」:trial 控件**过一次会保持已验证**、不再下发挑战(刷新键也消失),只能**重载页面**
/// 拿一道全新的题。重载后重新触发 + 短等,让易盾重新下发 `api/get`(监听跨导航仍有效)。
async fn fresh_challenge(tab: &ChromiumTab) {
    let _ = tab.get(URL).await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    let _ = tab.run_js(TRIGGER_JS).await;
    tokio::time::sleep(Duration::from_secs(1)).await;
}

/// 换图:优先**可信点击**易盾刷新键(真按钮),兜底 JS click。
async fn trusted_refresh(tab: &ChromiumTab) {
    for s in ["css:.yidun_refresh", "css:.yidun-refresh", "css:.yidun_panel-refresh"] {
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

/// 打印点击点叠加图(顺序 红/绿/蓝/橙),直观看点在哪几个字上。
fn save_overlay(cap: &[u8], hits: &[ClickHit], out_dir: &std::path::Path, attempt: u32) {
    if let Ok(dimg) = image::load_from_memory(cap) {
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
        rgba.save(out_dir.join(format!("overlay_{attempt}.png"))).ok();
    }
}

/// 诚实的最终结论:把各轮 [`Outcome`] 汇总——**只有「点击已提交→被拒」才算真没过**,
/// 「没出图 / 不稳定 / 识别不足 / 点击未被接收」一律单独说明,绝不冒充「过不了易盾」。
fn print_verdict(outcomes: &[Outcome]) {
    use Outcome::*;
    let has = |o: Outcome| outcomes.contains(&o);
    println!("\n[yidun] 各轮结果 = {outcomes:?}");
    let verdict = if has(Passed) {
        "通过 ✓"
    } else if has(Rejected) {
        "真·未通过 ✗:点击已提交但被易盾判失败(行为风控/艺术字识别)——这才是「过不了易盾」"
    } else if has(ClickNotSubmitted) {
        "未走完:点击未被易盾接收(没点在可点击层/面板收起/iframe 边界)——**不是**「过不了易盾」,属链路问题"
    } else if has(LowConfidence) {
        "未走完:识别置信不足、未敢下点——**不是**「过不了易盾」,属识别问题(可训模型/调阈值)"
    } else if has(ImageUnstable) {
        "未走完:点选图弹了但不稳定(中途换图/rect 失效)——**不是**「过不了易盾」,属稳定性问题"
    } else if has(ImageNeverShown) {
        "未走完:点选图**始终没弹出来**(触发/焦点/环境问题)——**绝不是**「过不了易盾」,先解决出图"
    } else {
        "无有效尝试"
    };
    println!("[yidun] 最终:{verdict}");
}

/// 环境诊断打印(没出图时定位根因)。
async fn dump_diag(tab: &ChromiumTab, label: &str) {
    if let Ok(v) = tab.run_js(DIAG_JS).await
        && let Some(s) = v.as_str()
    {
        println!("[yidun] 诊断[{label}] = {s}");
    }
}

/// 解析易盾 `api/get` 的 JSONP → `(bg 图 URL, front 点击顺序文本)`。
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

/// 从 JSONP/含包裹文本里截出第一个 `{` 到最后一个 `}` 的 JSON 子串。
fn json_slice(s: &str) -> Option<&str> {
    let a = s.find('{')?;
    let b = s.rfind('}')?;
    (b >= a).then(|| &s[a..=b])
}

/// 读 JSON 数字字段(缺则 0)。
fn jf(j: &serde_json::Value, k: &str) -> f64 {
    j[k].as_f64().unwrap_or(0.0)
}

/// 读 u32 环境变量(缺/非法则默认)。
fn env_u32(key: &str, default: u32) -> u32 {
    std::env::var(key).ok().and_then(|s| s.parse().ok()).unwrap_or(default)
}

/// 截断长字符串便于打印。
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
            put(img, cx + (rr as f32 * t.cos()) as i32, cy + (rr as f32 * t.sin()) as i32);
        }
    }
    for dy in -1..=1 {
        for dx in -1..=1 {
            put(img, cx + dx, cy + dy);
        }
    }
}
