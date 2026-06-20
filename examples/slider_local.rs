//! 通用滑块库能力**离线自验证**:本地合成一个 **`<img>` 版**滑块(非极验,只有 bg+piece、无 fullbg),
//! 用 `tab.solve_slider(&SliderConfig)` 求解,断言通过——证明该能力**与厂商无关**。
//!
//! 覆盖:① img 图源读取 ② 拼图模板法(无 fullbg)③ 闭环拖动 + 把手:拼图比例标定(本例 0.9 非 1:1)
//! ④ `SuccessCheck::Js` 自定义判定。完全离线(`file://` 本地页,写在项目目录下避开 macOS 沙箱)。
//!
//! 运行:`cargo run --example slider_local --no-default-features --features slider`(默认 headless;`HL=0` 看界面)。末行 ALL CHECKS PASSED。

use std::path::Path;
use std::time::Duration;

use drission::prelude::*;
use tokio::time::sleep;

/// 合成滑块页:背景图(带暗色方形缺口 + 亮边)+ 拼图块(同形方块),把手拖动按 0.9 比例带动拼图,
/// 松手时拼图左缘对准缺口(±6px)即置 `window.__ok=true`。bg/piece 都用 `<img>`(data URL)。
const PAGE: &str = r##"<!doctype html><html><head><meta charset="utf-8"><style>
  body{font:14px monospace;padding:16px}
  #wrap{position:relative;width:300px;height:150px;outline:1px solid #ccc}
  #bg{position:absolute;left:0;top:0;width:300px;height:150px}
  #piece{position:absolute;top:55px;left:10px;width:40px;height:40px}
  #track{position:relative;margin-top:12px;width:300px;height:30px;background:#eee;border-radius:15px}
  #handle{position:absolute;left:0;top:0;width:30px;height:30px;background:#4a90e2;border-radius:15px;cursor:pointer}
  #status{margin-top:10px}
  .ok{color:green;font-weight:bold}
</style></head><body>
  <div id="wrap"><img id="bg"><img id="piece"></div>
  <div id="track"><div id="handle"></div></div>
  <div id="status">idle</div>
  <script>
    var W=300,H=150,PS=40,PIECE_START=10,RATIO=0.9;
    var gapX=110+Math.floor(Math.random()*70), gapY=55;     // 缺口左缘 110..179
    (function(){                                              // 背景图 + 缺口
      var c=document.createElement('canvas');c.width=W;c.height=H;var x=c.getContext('2d');
      var g=x.createLinearGradient(0,0,W,H);g.addColorStop(0,'#99aadd');g.addColorStop(1,'#55aa88');
      x.fillStyle=g;x.fillRect(0,0,W,H);
      for(var i=0;i<400;i++){x.fillStyle='rgba(255,255,255,'+(Math.random()*0.12)+')';x.fillRect(Math.random()*W|0,Math.random()*H|0,2,2);}
      x.fillStyle='rgba(0,0,0,0.45)';x.fillRect(gapX,gapY,PS,PS);
      x.strokeStyle='rgba(255,255,255,0.95)';x.lineWidth=2;x.strokeRect(gapX+1,gapY+1,PS-2,PS-2);
      document.getElementById('bg').src=c.toDataURL();
    })();
    (function(){                                              // 拼图块(同形方块)
      var c=document.createElement('canvas');c.width=PS;c.height=PS;var x=c.getContext('2d');
      x.fillStyle='#777777';x.fillRect(0,0,PS,PS);
      x.strokeStyle='rgba(255,255,255,0.95)';x.lineWidth=2;x.strokeRect(1,1,PS-2,PS-2);
      document.getElementById('piece').src=c.toDataURL();
    })();
    var handle=document.getElementById('handle'),piece=document.getElementById('piece'),status=document.getElementById('status');
    var dragging=false,sx=0,ht=0; window.__ok=false;
    document.addEventListener('mousedown',function(e){dragging=true;sx=e.clientX;status.textContent='drag';},true);
    document.addEventListener('mousemove',function(e){ if(!dragging)return; var dx=e.clientX-sx;
      ht=Math.max(0,Math.min(W-30,dx)); handle.style.transform='translateX('+ht+'px)'; piece.style.transform='translateX('+(ht*RATIO)+'px)'; },true);
    document.addEventListener('mouseup',function(e){ if(!dragging)return; dragging=false;
      var pl=PIECE_START+ht*RATIO;
      if(Math.abs(pl-gapX)<=6){window.__ok=true;status.textContent='OK';status.className='ok';}
      else{status.textContent='retry '+Math.round(pl-gapX);} },true);
  </script>
</body></html>"##;

#[tokio::main]
async fn main() -> drission::Result<()> {
    let headless = std::env::var("HL").map(|v| v != "0").unwrap_or(true);

    // 本地页写到项目目录下(避开 macOS 沙箱拒读 /var/folders 的 file://)。
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/slider-fixture");
    std::fs::create_dir_all(&dir).ok();
    let page = dir.join("page.html");
    std::fs::write(&page, PAGE).map_err(|e| drission::Error::msg(format!("写页面失败: {e}")))?;
    let url = format!("file://{}", page.display());

    let browser = Browser::launch(BrowserOptions::new().headless(headless)).await?;
    let tab = browser.latest_tab().await?;

    println!("[*] 打开本地合成滑块页 {url}");
    tab.get(&url).await?;
    tab.wait().ele_displayed("#handle", None).await?;
    sleep(Duration::from_millis(600)).await; // 等 data URL 图加载完

    // 通用配置:img 图源 bg+piece(无 fullbg → 拼图模板法),自定义 JS 判定,把手 0.9 比例闭环标定。
    let cfg = SliderConfig::new(ImageSource::img("#bg"), "#handle")
        .piece(ImageSource::img("#piece"))
        .success(SuccessCheck::Js("window.__ok===true".into()))
        .max_attempts(4);

    // 先看纯视觉算的距离。
    match tab.slider_gap(&cfg).await {
        Ok(g) => println!(
            "[*] slider_gap():拼图需移 {:.0}px(法={:?} 置信 {:.2})",
            g.displace, g.method, g.confidence
        ),
        Err(e) => println!("[!] slider_gap 失败: {e}"),
    }

    let r = tab.solve_slider(&cfg).await?;
    println!(
        "[*] solve_slider:passed={} 尝试={} 对齐误差={:.1}px",
        r.passed, r.attempts, r.align_error
    );

    let mut ok = true;
    if !r.passed {
        println!("[FAIL] 未通过本地合成滑块");
        ok = false;
    }
    if r.align_error.is_finite() && r.align_error.abs() > 6.0 {
        println!("[FAIL] 对齐误差过大: {:.1}px", r.align_error);
        ok = false;
    }

    if !headless {
        sleep(Duration::from_secs(2)).await;
    }
    browser.quit().await?;

    if ok {
        println!("\nALL CHECKS PASSED");
        Ok(())
    } else {
        Err(drission::Error::msg("本地合成滑块自验证失败"))
    }
}
