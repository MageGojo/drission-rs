//! 动作链 `tab.actions()` 端到端演示:**拖放**(移到源 → 按住 → 拖到目标 → 释放)。
//!
//! 用本地拖放页(指针式:mousedown 跟随 mousemove 移动方块,mouseup 判断是否落入目标框),
//! 全程离线、确定性强。`HL=0 cargo run --example actions_drag` 可开窗口观看。
//!
//! 运行:`cargo run --example actions_drag`(默认 headless;`HL=0` 看界面)
//! 末行打印 `ALL CHECKS PASSED` / `SOME CHECKS FAILED`。

use std::time::Duration;

use drission::prelude::*;

const PAGE: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>drag</title>
<style>
  body{font-family:sans-serif;margin:40px;background:#fafafa}
  #area{position:relative;width:620px;height:300px;border:1px solid #ddd;background:#fff;border-radius:10px}
  #drag{position:absolute;left:24px;top:118px;width:90px;height:64px;background:#4f8cff;color:#fff;
        display:flex;align-items:center;justify-content:center;cursor:grab;user-select:none;border-radius:10px;font-size:16px}
  #drop{position:absolute;left:440px;top:90px;width:150px;height:120px;border:2px dashed #bbb;border-radius:10px;
        display:flex;align-items:center;justify-content:center;color:#999}
  #drop.over{border-color:#4f8cff;color:#4f8cff}
  #drop.done{background:#e7ffe9;border-color:#2ecc71;color:#2ecc71}
  #status{margin-top:18px;font-size:18px;color:#333}
</style></head><body>
<h2>动作链拖放测试</h2>
<div id="area"><div id="drag">拖我</div><div id="drop">放这里</div></div>
<div id="status">idle</div>
<script>
(function(){
  var drag=document.getElementById('drag'), drop=document.getElementById('drop'),
      status=document.getElementById('status'), area=document.getElementById('area');
  var on=false, ox=0, oy=0;
  drag.addEventListener('mousedown', function(e){ on=true; var r=drag.getBoundingClientRect();
    ox=e.clientX-r.left; oy=e.clientY-r.top; status.textContent='dragging'; e.preventDefault(); });
  document.addEventListener('mousemove', function(e){ if(!on) return; var ar=area.getBoundingClientRect();
    drag.style.left=(e.clientX-ar.left-ox)+'px'; drag.style.top=(e.clientY-ar.top-oy)+'px';
    var dr=drop.getBoundingClientRect();
    drop.className=(e.clientX>dr.left&&e.clientX<dr.right&&e.clientY>dr.top&&e.clientY<dr.bottom)?'over':''; });
  document.addEventListener('mouseup', function(){ if(!on) return; on=false;
    var dr=drop.getBoundingClientRect(), dc=drag.getBoundingClientRect();
    var cx=dc.left+dc.width/2, cy=dc.top+dc.height/2;
    if(cx>dr.left&&cx<dr.right&&cy>dr.top&&cy<dr.bottom){ drop.className='done'; drop.textContent='完成'; status.textContent='DROPPED'; }
    else status.textContent='missed'; });
})();
</script></body></html>"#;

#[tokio::main]
async fn main() -> drission::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .init();

    // 写到项目目录(home 下,避开 macOS 沙箱对 /var/folders 的 file:// 拒读)。
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("drission-actions");
    tokio::fs::create_dir_all(&dir).await?;
    let page = dir.join("drag.html");
    tokio::fs::write(&page, PAGE).await?;
    let url = format!("file://{}", page.display());

    let headless = std::env::var("HL").map(|v| v != "0").unwrap_or(true);
    println!("[*] 启动 Camoufox(headless={headless})…");
    let browser = Browser::launch(BrowserOptions::new().headless(headless)).await?;
    let tab = browser.latest_tab().await?;
    tab.get(&url).await?;

    let src = tab.ele("#drag").await?;
    let dst = tab.ele("#drop").await?;

    let status0 = status_text(&tab).await;
    println!("[*] 初始 status={status0:?}");

    // ---- 动作链:移到源 → 按住 → 拖到目标 → 释放 ----
    println!("[*] 执行动作链:move_to_ele(drag) → hold → move_to_ele(drop) → release");
    tab.actions()
        .move_to_ele(&src)
        .hold()
        .move_to_ele_offset(&dst, 0.0, 0.0, 0.7) // 慢一点,便于 HL=0 观看
        .wait(0.2)
        .release()
        .perform()
        .await?;

    tokio::time::sleep(Duration::from_millis(300)).await;
    let status1 = status_text(&tab).await;
    println!("[*] 拖放后 status={status1:?}");

    let pass = status1 == "DROPPED";
    println!(
        "\n==== {} ====",
        if pass { "ALL CHECKS PASSED" } else { "SOME CHECKS FAILED" }
    );

    if !headless {
        tokio::time::sleep(Duration::from_secs(3)).await; // 留时间看界面
    }
    browser.quit().await?;
    if pass {
        Ok(())
    } else {
        Err(drission::Error::msg("actions_drag 自验证未通过"))
    }
}

async fn status_text(tab: &Tab) -> String {
    tab.run_js("document.getElementById('status').textContent")
        .await
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}
