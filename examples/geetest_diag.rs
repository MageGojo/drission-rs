//! GeeTest 缺口检测**诊断**:把 bg/fullbg/slice 三张 canvas 落盘成 PNG,打印逐列 diff 剖面,
//! 再跑一遍当前 `geetest_slide` 的缺口算法,最后截图——用来确认"缺口距离到底算对没有"。
//!
//! 产物在 `target/geetest-diag/`:`bg.png` `fullbg.png` `slice.png` `before.png` `after.png`。
//!
//! 运行:`cargo run --example geetest_diag --no-default-features --features camoufox`(默认 headless;`HL=0` 看界面)

use std::path::Path;
use std::time::Duration;

use drission::prelude::*;
use tokio::time::sleep;

const DEFAULT_URL: &str = "https://demos.geetest.com/slide-float.html";

/// 取三张 canvas 的尺寸/位置/dataURL。
const CANVAS_INFO: &str = r#"(function(){
  function info(sel){
    var c=document.querySelector(sel);
    if(!c) return {found:false, sel:sel};
    var r=c.getBoundingClientRect();
    var o={found:true, sel:sel, w:c.width, h:c.height,
           cssW:Math.round(r.width*100)/100, cssH:Math.round(r.height*100)/100,
           left:Math.round(r.left*100)/100, top:Math.round(r.top*100)/100,
           display:getComputedStyle(c).display, opacity:getComputedStyle(c).opacity};
    try { o.dataURL = c.toDataURL('image/png'); } catch(e){ o.dataURL=null; o.err=String(e); }
    return o;
  }
  return JSON.stringify({
    bg: info('.geetest_canvas_bg'),
    full: info('.geetest_canvas_fullbg'),
    slice: info('.geetest_canvas_slice')
  });
})()"#;

/// 逐列 diff 剖面 + 各图均值(判空)+ slice 逐列 alpha 剖面。
const PROFILE_JS: &str = r#"(function(){
  var bg=document.querySelector('.geetest_canvas_bg');
  var full=document.querySelector('.geetest_canvas_fullbg');
  var slice=document.querySelector('.geetest_canvas_slice');
  if(!bg||!full) return JSON.stringify({ok:false, reason:'no bg/full'});
  var W=bg.width,H=bg.height;
  try{
    var b=bg.getContext('2d').getImageData(0,0,W,H).data;
    var f=full.getContext('2d').getImageData(0,0,W,H).data;
    function mean(a){var s=0;for(var k=0;k<a.length;k+=4){s+=a[k]+a[k+1]+a[k+2];}return Math.round(s/(a.length/4)/3);}
    var prof=[];
    for(var x=0;x<W;x++){ var d=0;
      for(var y=0;y<H;y++){ var i=(y*W+x)*4;
        if(Math.abs(b[i]-f[i])+Math.abs(b[i+1]-f[i+1])+Math.abs(b[i+2]-f[i+2])>60) d++; }
      prof.push(d);
    }
    var salpha=[];
    if(slice){ var sW=slice.width,sH=slice.height;
      var s=slice.getContext('2d').getImageData(0,0,sW,sH).data;
      for(var x2=0;x2<sW;x2++){ var a=0;
        for(var y2=0;y2<sH;y2++){ if(s[(y2*sW+x2)*4+3]>10) a++; }
        salpha.push(a);
      }
    }
    return JSON.stringify({ok:true, W:W, H:H, bgMean:mean(b), fullMean:mean(f), prof:prof, salpha:salpha});
  }catch(e){ return JSON.stringify({ok:false, reason:String(e)}); }
})()"#;

/// 两种稳健位移估计 + 当前算法位移,各出一张叠加验证图(slice 右移 D 叠到 bg,D 对则盖住缺口)。
///  - D_a: 拼图形状(alpha)在 bg-vs-full 的 diff 幅度图上滑动,最大化重叠。
///  - D_b: 拼图真实颜色与 fullbg 在落点处的颜色做最小绝对差(把拼图放回原图它该长的样子)。
///  - D_cur: 复刻当前 geetest_slide 算法(diff>阈值、从 x=55 起首个连续≥6列块左缘 - 拼图最左非透明列)。
const MATCH_JS: &str = r#"(function(){
  var bg=document.querySelector('.geetest_canvas_bg');
  var full=document.querySelector('.geetest_canvas_fullbg');
  var slice=document.querySelector('.geetest_canvas_slice');
  if(!bg||!full||!slice) return JSON.stringify({ok:false, reason:'missing canvas'});
  var W=bg.width,H=bg.height;
  try{
    var b=bg.getContext('2d').getImageData(0,0,W,H).data;
    var f=full.getContext('2d').getImageData(0,0,W,H).data;
    var s=slice.getContext('2d').getImageData(0,0,W,H).data;
    var dm=new Float64Array(W*H);
    for(var y=0;y<H;y++) for(var x=0;x<W;x++){ var i=(y*W+x)*4;
      dm[y*W+x]=Math.abs(b[i]-f[i])+Math.abs(b[i+1]-f[i+1])+Math.abs(b[i+2]-f[i+2]); }
    var pts=[]; var px0=W, px1=0, alphaLeft=-1;
    for(var y2=0;y2<H;y2++) for(var x2=0;x2<W;x2++){ var a=s[(y2*W+x2)*4+3];
      if(a>30){ pts.push([x2,y2,a, s[(y2*W+x2)*4],s[(y2*W+x2)*4+1],s[(y2*W+x2)*4+2]]);
        if(x2<px0)px0=x2; if(x2>px1)px1=x2; } }
    for(var xx0=0;xx0<W&&alphaLeft<0;xx0++){ for(var yy0=0;yy0<H;yy0++){ if(s[(yy0*W+xx0)*4+3]>10){alphaLeft=xx0;break;} } }
    var maxD=W-px1-1;
    // D_a: 形状 vs diff,最大化。
    var Da=-1,bestA=-1, curveA=[];
    for(var D=0;D<=maxD;D++){ var sc=0;
      for(var k=0;k<pts.length;k++){ var p=pts[k]; var x=p[0]+D; sc+=dm[p[1]*W+x]*p[2]; }
      curveA.push(Math.round(sc/1000)); if(sc>bestA){bestA=sc;Da=D;}
    }
    // D_b: 颜色 vs fullbg,最小化绝对差(只在 px0+D 落入图内的范围)。
    var Db=-1,bestB=1e18, curveB=[];
    for(var D2=0;D2<=maxD;D2++){ var er=0,cnt=0;
      for(var k2=0;k2<pts.length;k2++){ var q=pts[k2]; var x2c=q[0]+D2; var j=(q[1]*W+x2c)*4;
        er+=Math.abs(q[3]-f[j])+Math.abs(q[4]-f[j+1])+Math.abs(q[5]-f[j+2]); cnt++; }
      var avg=cnt?er/cnt:1e9; curveB.push(Math.round(avg));
      if(avg<bestB){bestB=avg;Db=D2;}
    }
    // D_cur: 复刻当前算法。
    function diffRows(x){var d=0;for(var y=0;y<H;y++){var i=(y*W+x)*4;
      if(Math.abs(b[i]-f[i])+Math.abs(b[i+1]-f[i+1])+Math.abs(b[i+2]-f[i+2])>60)d++;}return d;}
    var gapX=-1, run=0;
    for(var xc=55;xc<W;xc++){ if(diffRows(xc)>20){ run++; if(run>=6){ gapX=xc-run+1; break; } } else run=0; }
    var Dcur = gapX>=0 ? (gapX-alphaLeft) : -1;
    function overlay(D){ if(D<0)return null;
      var t=document.createElement('canvas'); t.width=W;t.height=H;
      var c=t.getContext('2d'); c.drawImage(bg,0,0); c.drawImage(slice,D,0);
      try{return t.toDataURL('image/png');}catch(e){return null;}
    }
    return JSON.stringify({ok:true, W:W, H:H, px0:px0, px1:px1, alphaLeft:alphaLeft,
      Da:Da, Db:Db, Dcur:Dcur, gapX:gapX, bestBerr:Math.round(bestB),
      curveA:curveA, curveB:curveB,
      overlayA:overlay(Da), overlayB:overlay(Db), overlayCur:overlay(Dcur)});
  }catch(e){ return JSON.stringify({ok:false, reason:String(e)}); }
})()"#;

fn b64_decode(s: &str) -> Vec<u8> {
    fn val(b: u8) -> Option<u32> {
        match b {
            b'A'..=b'Z' => Some((b - b'A') as u32),
            b'a'..=b'z' => Some((b - b'a' + 26) as u32),
            b'0'..=b'9' => Some((b - b'0' + 52) as u32),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let mut bits = 0u32;
    let mut nbits = 0u32;
    let mut out = Vec::new();
    for &b in s.as_bytes() {
        if b == b'=' {
            break;
        }
        let Some(v) = val(b) else { continue };
        bits = (bits << 6) | v;
        nbits += 6;
        if nbits >= 8 {
            nbits -= 8;
            out.push((bits >> nbits) as u8);
        }
    }
    out
}

fn save_data_url(dir: &Path, name: &str, data_url: &str) {
    if let Some(idx) = data_url.find("base64,") {
        let bytes = b64_decode(&data_url[idx + 7..]);
        let p = dir.join(name);
        if std::fs::write(&p, &bytes).is_ok() {
            println!("    saved {} ({} bytes)", p.display(), bytes.len());
        }
    } else {
        println!("    [!] {name}: no base64 payload");
    }
}

/// 把 0..max 的剖面画成一行 sparkline。
fn sparkline(prof: &[i64]) -> String {
    let blocks = [
        ' ', '\u{2581}', '\u{2582}', '\u{2583}', '\u{2584}', '\u{2585}', '\u{2586}', '\u{2587}',
        '\u{2588}',
    ];
    let max = prof.iter().copied().max().unwrap_or(1).max(1);
    prof.iter()
        .map(|&v| {
            let idx = ((v as f64 / max as f64) * 8.0).round() as usize;
            blocks[idx.min(8)]
        })
        .collect()
}

#[tokio::main]
async fn main() -> drission::Result<()> {
    let headless = std::env::var("HL").map(|v| v != "0").unwrap_or(true);
    let out_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/geetest-diag");
    std::fs::create_dir_all(&out_dir).ok();

    let browser = Browser::launch(BrowserOptions::new().headless(headless)).await?;
    let tab = browser.latest_tab().await?;
    tab.apply_pointer_stealth().await?;

    let url = std::env::var("URL").unwrap_or_else(|_| DEFAULT_URL.to_string());
    println!("[*] 打开 {url}");
    tab.get(&url).await?;
    sleep(Duration::from_secs(3)).await;

    println!("[*] 点雷达按钮弹滑块…");
    tab.ele("css:.geetest_radar_btn").await?.click().await?;
    let _ = tab.ele("css:.geetest_slider_button").await?;
    sleep(Duration::from_millis(1200)).await;

    // before 截图。
    if let Ok(p) = tab.get_screenshot(out_dir.join("before.png"), false).await {
        println!("[*] before 截图 {}", p.display());
    }

    // 三张 canvas 信息 + 落盘。
    let info = tab.run_js(CANVAS_INFO).await?;
    let iv: serde_json::Value =
        serde_json::from_str(info.as_str().unwrap_or("{}")).unwrap_or_default();
    println!("\n==== canvas 信息 ====");
    for key in ["bg", "full", "slice"] {
        let c = &iv[key];
        if c["found"].as_bool() != Some(true) {
            println!("  {key:<6} 未找到");
            continue;
        }
        println!(
            "  {key:<6} backing={}x{} css={}x{} left={} top={} display={} opacity={}",
            c["w"], c["h"], c["cssW"], c["cssH"], c["left"], c["top"], c["display"], c["opacity"]
        );
        if let Some(durl) = c["dataURL"].as_str() {
            save_data_url(&out_dir, &format!("{key}.png"), durl);
        } else {
            println!("    [!] dataURL 读取失败: {}", c["err"]);
        }
    }

    // diff 剖面。
    let prof = tab.run_js(PROFILE_JS).await?;
    let pv: serde_json::Value =
        serde_json::from_str(prof.as_str().unwrap_or("{}")).unwrap_or_default();
    if pv["ok"].as_bool() != Some(true) {
        println!("\n[!] 剖面读取失败: {}", pv["reason"]);
    } else {
        let w = pv["W"].as_i64().unwrap_or(0);
        let prof: Vec<i64> = pv["prof"]
            .as_array()
            .map(|a| a.iter().map(|v| v.as_i64().unwrap_or(0)).collect())
            .unwrap_or_default();
        let salpha: Vec<i64> = pv["salpha"]
            .as_array()
            .map(|a| a.iter().map(|v| v.as_i64().unwrap_or(0)).collect())
            .unwrap_or_default();
        println!(
            "\n==== diff 剖面 (W={w}, bgMean={}, fullMean={}) ====",
            pv["bgMean"], pv["fullMean"]
        );
        println!("  bg-vs-full 逐列高差异行数 (越高=该列差异越大):");
        println!("  [{}]", sparkline(&prof));
        // 列出所有高差异区间(连续 diff>20)。
        let thr = 20;
        let mut runs: Vec<(i64, i64, i64)> = vec![]; // (start, end, peak)
        let mut i = 0i64;
        while (i as usize) < prof.len() {
            if prof[i as usize] > thr {
                let start = i;
                let mut peak = 0;
                while (i as usize) < prof.len() && prof[i as usize] > thr {
                    peak = peak.max(prof[i as usize]);
                    i += 1;
                }
                runs.push((start, i - 1, peak));
            } else {
                i += 1;
            }
        }
        println!("  高差异区间(diff>{thr}): ");
        for (s, e, pk) in &runs {
            println!("    x=[{s:>3}..{e:>3}] 宽{:>3} 峰值{pk}", e - s + 1);
        }
        // 当前算法:从 x=55 起,首个连续 >=6 列的左缘。
        let mut gap_x = -1i64;
        let mut run = 0i64;
        for (x, &v) in prof.iter().enumerate().skip(55) {
            if v > thr {
                run += 1;
                if run >= 6 {
                    gap_x = x as i64 - run + 1;
                    break;
                }
            } else {
                run = 0;
            }
        }
        println!("  >> 当前算法 gapX = {gap_x} (从 x=55 起首个连续≥6列高差异块的左缘)");
        if !salpha.is_empty() {
            println!("\n  slice 逐列非透明像素数 (拼图块形状):");
            println!("  [{}]", sparkline(&salpha));
            let piece_left = salpha
                .iter()
                .position(|&a| a > 5)
                .map(|p| p as i64)
                .unwrap_or(-1);
            let piece_right = salpha
                .iter()
                .rposition(|&a| a > 5)
                .map(|p| p as i64)
                .unwrap_or(-1);
            println!(
                "  slice 非透明列: 左缘={piece_left} 右缘={piece_right} 宽={}",
                piece_right - piece_left + 1
            );
        }
    }

    // 三法位移对比 + 叠加验证图。
    let m = tab.run_js(MATCH_JS).await?;
    let mv: serde_json::Value =
        serde_json::from_str(m.as_str().unwrap_or("{}")).unwrap_or_default();
    if mv["ok"].as_bool() != Some(true) {
        println!("\n[!] 匹配失败: {}", mv["reason"]);
    } else {
        let da = mv["Da"].as_i64().unwrap_or(-1);
        let db = mv["Db"].as_i64().unwrap_or(-1);
        let dcur = mv["Dcur"].as_i64().unwrap_or(-1);
        println!("\n==== 位移三法对比(画布像素,1:1==CSS) ====");
        println!(
            "  px0={} alphaLeft={} gapX={}",
            mv["px0"], mv["alphaLeft"], mv["gapX"]
        );
        println!("  D_cur (当前算法: gapX - alphaLeft) = {dcur}");
        println!("  D_a   (拼图形状 vs diff,最大重叠)   = {da}");
        println!(
            "  D_b   (拼图颜色 vs fullbg,最小色差) = {db}  (平均残差 {}/765)",
            mv["bestBerr"]
        );
        println!(
            "  >> 三法差值: |D_a-D_b|={}  |D_cur-D_b|={}",
            (da - db).abs(),
            (dcur - db).abs()
        );
        if let Some(curve) = mv["curveB"].as_array() {
            let c: Vec<i64> = curve.iter().map(|v| v.as_i64().unwrap_or(0)).collect();
            // D_b 是最小化,取负画 sparkline 让谷=峰。
            let maxv = c.iter().copied().max().unwrap_or(1);
            let inv: Vec<i64> = c.iter().map(|&v| maxv - v).collect();
            println!("  D_b 残差曲线(谷=最佳,这里翻成峰):");
            println!("  [{}]", sparkline(&inv));
        }
        for (key, name) in [
            ("overlayCur", "overlay_cur.png"),
            ("overlayA", "overlay_a.png"),
            ("overlayB", "overlay_b.png"),
        ] {
            if let Some(durl) = mv[key].as_str() {
                save_data_url(&out_dir, name, durl);
            }
        }
        println!("  >> 看 overlay_b.png(应严丝合缝盖住缺口) vs overlay_cur.png(当前算法,可能偏移)");
    }

    if !headless {
        sleep(Duration::from_secs(3)).await;
    }
    browser.quit().await?;
    println!("\n[*] 诊断完成,产物见 {}", out_dir.display());
    Ok(())
}
