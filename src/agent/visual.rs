//! Resolver 最后两档:可见字旁边的可点区域,以及(可选)元素截图 OCR。

use serde_json::json;

use crate::Result;
use crate::cdp::ChromiumTab;
use crate::locator::Locator;

use super::{AgentElement, wrap_ele};

const MARK: &str = "data-drs-visual";

/// 在可见文本附近找可点节点(不依赖 AX 名)。适合「字在旁边、按钮是空的」。
pub async fn resolve_near_text(tab: &ChromiumTab, loc: &Locator) -> Result<Option<AgentElement>> {
    let Some(hint) = loc.hint_text() else {
        return Ok(None);
    };
    if hint.is_empty() {
        return Ok(None);
    }
    let js = format!(
        "(function(hint){{\
           try{{document.querySelectorAll('[{MARK}]').forEach(function(e){{e.removeAttribute('{MARK}');}});}}catch(e){{}}\
           function vis(el){{\
             var r=el.getBoundingClientRect(); var s=getComputedStyle(el);\
             return r.width>2&&r.height>2&&s.visibility!=='hidden'&&s.display!=='none';\
           }}\
           var sel='button,a,[role=button],input[type=submit],input[type=button],[onclick],img,canvas';\
           function mark(el,via){{ el.setAttribute('{MARK}','1'); return {{ok:true,via:via}}; }}\
           var it=document.createNodeIterator(document.body, NodeFilter.SHOW_TEXT);\
           var n;\
           while(n=it.nextNode()){{\
             if(!(n.nodeValue||'').includes(hint)) continue;\
             var el=n.parentElement;\
             var climb=el;\
             while(climb&&!(climb.matches&&climb.matches(sel))) climb=climb.parentElement;\
             if(climb&&vis(climb)) return mark(climb,'ancestor');\
             var sibs=[el&&el.nextElementSibling, el&&el.previousElementSibling, el&&el.parentElement&&el.parentElement.nextElementSibling];\
             for(var i=0;i<sibs.length;i++){{\
               var s=sibs[i];\
               if(s&&s.matches&&s.matches(sel)&&vis(s)) return mark(s,'sibling');\
             }}\
             var range=document.createRange(); range.selectNodeContents(n);\
             var r=range.getBoundingClientRect();\
             var cx=r.left+r.width/2, cy=r.top+r.height/2;\
             var deltas=[[0,0],[36,0],[-36,0],[0,28],[0,-28],[80,0],[-80,0]];\
             for(var j=0;j<deltas.length;j++){{\
               var hit=document.elementFromPoint(cx+deltas[j][0], cy+deltas[j][1]);\
               if(!hit) continue;\
               var c=hit.closest?hit.closest(sel):null;\
               if(!c&&hit.matches&&hit.matches(sel)) c=hit;\
               if(c&&vis(c)) return mark(c,'point');\
             }}\
           }}\
           return {{ok:false}};\
         }})({})",
        json!(hint)
    );
    let v = tab.run_js(&js).await?;
    if !v.get("ok").and_then(|x| x.as_bool()).unwrap_or(false) {
        return Ok(None);
    }
    match tab.ele(&format!("css:[{MARK}=\"1\"]")).await {
        Ok(ele) => Ok(Some(wrap_ele(ele, loc))),
        Err(_) => Ok(None),
    }
}

/// 对无字的可点节点截图做 OCR。需 `--features ocr`。
#[cfg(feature = "ocr")]
pub async fn resolve_ocr(tab: &ChromiumTab, loc: &Locator) -> Result<Option<AgentElement>> {
    let Some(hint) = loc.hint_text() else {
        return Ok(None);
    };
    if hint.is_empty() {
        return Ok(None);
    }
    let marked = tab
        .run_js(
            "(function(){\
               var sel='button,a,[role=button],img,canvas,input[type=image]';\
               var els=[].slice.call(document.querySelectorAll(sel));\
               var n=0;\
               els.forEach(function(el,i){\
                 var t=(el.innerText||el.value||el.alt||el.title||'').trim();\
                 var r=el.getBoundingClientRect();\
                 if(t.length>0||r.width<8||r.height<8||r.width>800||r.height>400) return;\
                 el.setAttribute('data-drs-ocr', String(n++));\
               });\
               return n;\
             })()",
        )
        .await?;
    let n = marked.as_u64().unwrap_or(0) as usize;
    let needle = hint.to_lowercase();
    for i in 0..n.min(12) {
        let sel = format!("css:[data-drs-ocr=\"{i}\"]");
        let Ok(ele) = tab.ele(&sel).await else {
            continue;
        };
        let Ok(png) = ele.screenshot_bytes().await else {
            continue;
        };
        let Ok(text) = crate::ocr::recognize_shared(&png).await else {
            continue;
        };
        if text.to_lowercase().contains(&needle) {
            return Ok(Some(wrap_ele(ele, loc)));
        }
    }
    Ok(None)
}

#[cfg(not(feature = "ocr"))]
pub async fn resolve_ocr(_tab: &ChromiumTab, _loc: &Locator) -> Result<Option<AgentElement>> {
    Ok(None)
}
