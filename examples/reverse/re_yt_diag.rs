//! YouTube n 参数诊断:摸清 ① headless 能否起播 ② 流地址里 n 的形态(/n/ 路径 vs ?n= 查询)
//! ③ 能否直接从 `ytInitialPlayerResponse` 拿到乱序 n(那样无需起播即可解扰)。
//! 仅诊断、不改逻辑。运行:`cargo run --example re_yt_diag --features cdp`(`HL=0` 有头)。

use std::time::Duration;

use drission::prelude::*;

#[tokio::main]
async fn main() -> drission::Result<()> {
    let headless = std::env::var("HL").map(|v| v != "0").unwrap_or(true);
    let url = std::env::var("URL")
        .unwrap_or_else(|_| "https://www.youtube.com/watch?v=7349tcyyE-c".to_string());
    let browser = ChromiumBrowser::launch(ChromiumOptions::new().headless(headless)).await?;
    let tab = browser.new_tab(Some("about:blank")).await?;
    tab.get(&url).await?;
    tokio::time::sleep(Duration::from_secs(6)).await;

    let diag = r#"(function(){
      var out={};
      try{
        var r=window.ytInitialPlayerResponse;
        out.hasPR=!!r;
        if(r&&r.streamingData){
          var sd=r.streamingData;
          var af=sd.adaptiveFormats||[], f=sd.formats||[];
          out.adaptiveCount=af.length; out.formatCount=f.length;
          var all=af.concat(f);
          var withUrl=0, withCipher=0, sampleUrl='', sampleCipher='';
          for(var i=0;i<all.length;i++){
            if(all[i].url){withUrl++; if(!sampleUrl)sampleUrl=all[i].url;}
            if(all[i].signatureCipher){withCipher++; if(!sampleCipher)sampleCipher=all[i].signatureCipher;}
          }
          out.withUrl=withUrl; out.withCipher=withCipher;
          var u=sampleUrl|| (sampleCipher? (function(){try{var p=new URLSearchParams(sampleCipher);return p.get('url')||''}catch(e){return ''}})() : '');
          out.sampleUrlHead=u.slice(0,120);
          out.nPath=(u.match(/\/n\/([^/]+)/)||[])[1]||'';
          out.nQuery=(u.match(/[?&]n=([^&]+)/)||[])[1]||'';
        }
      }catch(e){out.prErr=String(e);}
      try{
        var v=document.querySelector('video');
        out.hasVideo=!!v;
        if(v){out.readyState=v.readyState; out.paused=v.paused; out.ct=v.currentTime; out.err=v.error?v.error.code:0; out.src=(v.currentSrc||'').slice(0,60);}
      }catch(e){out.vErr=String(e);}
      return JSON.stringify(out);
    })()"#;

    let before = run_str(&tab, diag).await;
    println!("== 起播前 ==\n{before}");

    // 主动起播,等 5s 再看。
    let _ = tab
        .run_js("setTimeout(function(){try{var v=document.querySelector('video');if(v){v.muted=true;v.play&&v.play()}}catch(e){}},0);1")
        .await;
    tokio::time::sleep(Duration::from_secs(6)).await;
    let after = run_str(&tab, diag).await;
    println!("\n== 起播后 ==\n{after}");

    // 看是否真有 googlevideo 流量(performance 资源条目)。
    let gv = run_str(
        &tab,
        "JSON.stringify((performance.getEntriesByType('resource')||[]).map(function(e){return e.name}).filter(function(n){return /googlevideo|videoplayback/.test(n)}).slice(0,3))",
    )
    .await;
    println!("\n== googlevideo 资源请求(前3)==\n{gv}");

    browser.quit().await?;
    Ok(())
}

async fn run_str(tab: &ChromiumTab, expr: &str) -> String {
    tab.run_js(expr)
        .await
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| "<非字符串/失败>".into())
}
