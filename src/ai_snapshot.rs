//! AI 友好的页面快照(后端无关)。
//!
//! 把当前页压成「可读大纲 + 可操作 ref」,供 LLM / MCP Agent 理解页面并点击/输入。
//! 对标 Playwright MCP 的 `browser_snapshot`,但走注入 JS,CDP / Camoufox 通用。
//!
//! - **interesting-only**:只收集交互控件 + 标题/地标,不全整页 DOM。
//! - **时间预算 + 节点上限**:重 SPA 上超时返回 `partial`,不卡死 Runtime.evaluate。
//! - **ref 标记**:给可操作节点写 `data-drs-ref="eN"`,可用选择器 `ref:eN` 点击/输入。

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 默认 JS 时间预算(毫秒)。
pub const DEFAULT_BUDGET_MS: u64 = 2_000;
/// 默认最多收录的大纲行数。
pub const DEFAULT_MAX_ITEMS: usize = 250;
/// 默认附带正文截断长度。
pub const DEFAULT_MAX_TEXT_CHARS: usize = 8_000;

/// 把 `ref:e1` 解析为可传给 `click`/`type` 的 DP 选择器;其它选择器原样返回。
pub fn resolve_selector(selector: &str) -> String {
    let sel = selector.trim();
    if let Some(rest) = sel.strip_prefix("ref:") {
        let id = rest.trim();
        if !id.is_empty()
            && id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return format!("css:[data-drs-ref=\"{id}\"]");
        }
    }
    sel.to_string()
}

/// 单个可操作 / 语义节点的元数据。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotRef {
    pub role: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub value: Option<String>,
    /// 已可直接喂给 `click`/`type` 的选择器(含 `css:` 前缀)。
    pub selector: String,
}

/// 解析后的 AI 快照结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiSnapshot {
    pub snapshot: String,
    pub refs: Vec<(String, SnapshotRef)>,
    pub text: String,
    pub partial: bool,
    pub truncated: bool,
    pub item_count: usize,
    pub elapsed_ms: u64,
}

impl AiSnapshot {
    /// 从页面 `run_js` 返回值解析。
    pub fn from_value(v: &Value) -> Option<Self> {
        let snapshot = v.get("snapshot")?.as_str()?.to_string();
        let text = v
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let partial = v.get("partial").and_then(Value::as_bool).unwrap_or(false);
        let truncated = v.get("truncated").and_then(Value::as_bool).unwrap_or(false);
        let item_count = v.get("itemCount").and_then(Value::as_u64).unwrap_or(0) as usize;
        let elapsed_ms = v.get("elapsedMs").and_then(Value::as_u64).unwrap_or(0);

        let mut refs = Vec::new();
        if let Some(arr) = v.get("refs").and_then(Value::as_array) {
            for item in arr {
                let id = item
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                if id.is_empty() {
                    continue;
                }
                let role = item
                    .get("role")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let name = item
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let value = item
                    .get("value")
                    .and_then(Value::as_str)
                    .map(|s| s.to_string())
                    .filter(|s| !s.is_empty());
                let selector = item
                    .get("selector")
                    .and_then(Value::as_str)
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| format!("css:[data-drs-ref=\"{id}\"]"));
                refs.push((
                    id,
                    SnapshotRef {
                        role,
                        name,
                        value,
                        selector,
                    },
                ));
            }
        }

        Some(Self {
            snapshot,
            refs,
            text,
            partial,
            truncated,
            item_count,
            elapsed_ms,
        })
    }

    /// 序列化为 CLI / MCP `data` 对象字段。
    pub fn into_json_fields(self) -> Value {
        let mut map = serde_json::Map::new();
        map.insert("snapshot".into(), Value::String(self.snapshot));
        map.insert("text".into(), Value::String(self.text));
        map.insert("partial".into(), Value::Bool(self.partial));
        map.insert("truncated".into(), Value::Bool(self.truncated));
        map.insert("itemCount".into(), json_usize(self.item_count));
        map.insert("elapsedMs".into(), Value::from(self.elapsed_ms));

        let mut refs_obj = serde_json::Map::new();
        for (id, r) in self.refs {
            let mut entry = serde_json::Map::new();
            entry.insert("role".into(), Value::String(r.role));
            entry.insert("name".into(), Value::String(r.name));
            if let Some(v) = r.value {
                entry.insert("value".into(), Value::String(v));
            }
            entry.insert("selector".into(), Value::String(r.selector));
            refs_obj.insert(id, Value::Object(entry));
        }
        map.insert("refs".into(), Value::Object(refs_obj));
        Value::Object(map)
    }
}

fn json_usize(n: usize) -> Value {
    Value::from(n as u64)
}

/// 注入页面采集 AI 快照的 JS(返回 JSON 对象)。
///
/// 参数由 Rust 侧插值:`budget_ms` / `max_items` / `max_text_chars`。
pub fn ai_snapshot_js(budget_ms: u64, max_items: usize, max_text_chars: usize) -> String {
    format!(
        r#"(()=>{{
  const BUDGET={budget_ms}, MAX={max_items}, MAX_TEXT={max_text_chars};
  const t0=Date.now();
  const timedOut=()=>Date.now()-t0>BUDGET;
  try{{
    document.querySelectorAll('[data-drs-ref]').forEach(el=>el.removeAttribute('data-drs-ref'));
  }}catch(_e){{}}
  const INTERACTIVE='a[href],button,input:not([type=hidden]),select,textarea,summary,[role=button],[role=link],[role=textbox],[role=searchbox],[role=checkbox],[role=radio],[role=combobox],[role=menuitem],[role=tab],[role=switch],[role=option],[contenteditable=""],[contenteditable=true]';
  const LANDMARK='h1,h2,h3,h4,h5,h6,main,nav,header,footer,dialog,[role=heading],[role=main],[role=navigation],[role=banner],[role=contentinfo],[role=dialog],[role=alert],[role=status]';
  const SKIP_TAG={{SCRIPT:1,STYLE:1,NOSCRIPT:1,TEMPLATE:1,META:1,LINK:1,BR:1,SVG:1,PATH:1}};
  const INPUT_ROLE={{checkbox:'checkbox',radio:'radio',button:'button',submit:'button',reset:'button',range:'slider',search:'searchbox',email:'textbox',tel:'textbox',url:'textbox',number:'spinbutton',password:'textbox',text:'textbox',file:'button'}};
  const roleOf=el=>{{
    const ex=el.getAttribute('role'); if(ex) return ex.trim().split(/\s+/)[0];
    const tag=el.tagName;
    if(tag==='A') return el.hasAttribute('href')?'link':'generic';
    if(tag==='INPUT'){{const t=(el.getAttribute('type')||'text').toLowerCase();return INPUT_ROLE[t]||'textbox';}}
    if(tag==='BUTTON'||tag==='SUMMARY') return 'button';
    if(tag==='SELECT') return 'combobox';
    if(tag==='TEXTAREA') return 'textbox';
    if(/^H[1-6]$/.test(tag)) return 'heading';
    if(tag==='MAIN') return 'main';
    if(tag==='NAV') return 'navigation';
    if(tag==='HEADER') return 'banner';
    if(tag==='FOOTER') return 'contentinfo';
    if(tag==='DIALOG') return 'dialog';
    if(el.isContentEditable) return 'textbox';
    return 'generic';
  }};
  const ownText=el=>{{let s='';for(const n of el.childNodes){{if(n.nodeType===3)s+=n.textContent;}}return s.replace(/\s+/g,' ').trim();}};
  const nameOf=el=>{{
    const al=el.getAttribute('aria-label'); if(al&&al.trim()) return al.trim().slice(0,100);
    const lb=el.getAttribute('aria-labelledby');
    if(lb){{const r=lb.split(/\s+/).map(id=>{{const n=document.getElementById(id);return n?((n.innerText||'').trim()):'';}}).join(' ').trim();if(r)return r.slice(0,100);}}
    const tag=el.tagName;
    if(tag==='IMG') return (el.getAttribute('alt')||'').trim().slice(0,100);
    if(tag==='INPUT'){{
      const t=(el.getAttribute('type')||'text').toLowerCase();
      if(t==='submit'||t==='button'||t==='reset') return (el.value||'').trim().slice(0,100);
      const lab=el.labels&&el.labels[0]?(el.labels[0].innerText||'').trim():'';
      if(lab) return lab.slice(0,100);
      return (el.getAttribute('placeholder')||el.getAttribute('name')||el.getAttribute('aria-placeholder')||'').trim().slice(0,100);
    }}
    if(tag==='TEXTAREA'||tag==='SELECT'){{
      const lab=el.labels&&el.labels[0]?(el.labels[0].innerText||'').trim():'';
      if(lab) return lab.slice(0,100);
      return (el.getAttribute('placeholder')||el.getAttribute('name')||'').trim().slice(0,100);
    }}
    const o=ownText(el); if(o) return o.slice(0,100);
    try{{return ((el.innerText||'').trim()).slice(0,100);}}catch(_e){{return '';}}
  }};
  const roughVisible=el=>{{
    if(!el||el.nodeType!==1) return false;
    if(SKIP_TAG[el.tagName]) return false;
    if(el.hasAttribute('hidden')||el.getAttribute('aria-hidden')==='true') return false;
    const st=el.getAttribute('style')||'';
    if(/display\s*:\s*none/i.test(st)||/visibility\s*:\s*hidden/i.test(st)) return false;
    try{{
      const r=el.getBoundingClientRect();
      if(r.width===0&&r.height===0&&el.tagName!=='INPUT') return false;
    }}catch(_e){{}}
    return true;
  }};
  const valueOf=el=>{{
    const tag=el.tagName;
    if(tag==='INPUT'){{
      const t=(el.getAttribute('type')||'text').toLowerCase();
      if(t==='checkbox'||t==='radio') return el.checked?'checked':'';
      if(t==='password') return el.value?'••••':'';
      return (el.value||'').slice(0,80);
    }}
    if(tag==='TEXTAREA') return (el.value||'').slice(0,80);
    if(tag==='SELECT'){{
      const opt=el.selectedOptions&&el.selectedOptions[0];
      return opt?((opt.textContent||'').trim().slice(0,80)):'';
    }}
    if(el.isContentEditable) return ((el.innerText||'').trim()).slice(0,80);
    return '';
  }};
  const propsOf=el=>{{
    const p=[];
    if(el.disabled||el.getAttribute('aria-disabled')==='true') p.push('disabled');
    if(el.tagName==='INPUT'&&(el.type==='checkbox'||el.type==='radio')&&el.checked) p.push('checked');
    const ae=el.getAttribute('aria-expanded'); if(ae) p.push('expanded='+ae);
    if(/^H[1-6]$/.test(el.tagName)) p.push('level='+el.tagName[1]);
    const lvl=el.getAttribute('aria-level'); if(lvl) p.push('level='+lvl);
    return p;
  }};
  const isInteractive=role=>['button','link','textbox','searchbox','checkbox','radio','combobox','slider','spinbutton','menuitem','tab','switch','option'].includes(role);
  const collect=(sel,into,seen)=>{{
    let list; try{{list=document.querySelectorAll(sel);}}catch(_e){{return;}}
    for(const el of list){{
      if(timedOut()||into.length>=MAX) return;
      if(seen.has(el)||!roughVisible(el)) continue;
      seen.add(el);
      into.push(el);
    }}
  }};
  const seen=new Set();
  const els=[];
  collect(LANDMARK,els,seen);
  collect(INTERACTIVE,els,seen);
  // document order
  els.sort((a,b)=>{{
    const p=a.compareDocumentPosition(b);
    if(p&Node.DOCUMENT_POSITION_FOLLOWING) return -1;
    if(p&Node.DOCUMENT_POSITION_PRECEDING) return 1;
    return 0;
  }});
  const lines=[];
  const refs=[];
  let refN=0;
  let partial=false;
  for(const el of els){{
    if(timedOut()){{partial=true;break;}}
    if(lines.length>=MAX){{partial=true;break;}}
    const role=roleOf(el);
    const name=nameOf(el);
    const val=valueOf(el);
    const props=propsOf(el);
    let line='- '+role;
    if(name) line+=' "'+name.replace(/"/g,"'")+'"';
    let id=null;
    if(isInteractive(role)){{
      refN+=1; id='e'+refN;
      try{{el.setAttribute('data-drs-ref',id);}}catch(_e){{}}
      line+=' [ref='+id+']';
      refs.push({{id,role,name,value:val||undefined,selector:'css:[data-drs-ref="'+id+'"]'}});
    }}
    if(props.length) line+=' ['+props.join(', ')+']';
    if(val&&(role==='textbox'||role==='searchbox'||role==='spinbutton'||role==='combobox')) line+=' value="'+String(val).replace(/"/g,"'")+'"';
    if(val==='checked') line+=' [checked]';
    lines.push(line);
  }}
  let text='';
  let truncated=false;
  if(!timedOut()){{
    try{{
      // Prefer textContent when budget is nearly spent — innerText forces layout.
      const remain=BUDGET-(Date.now()-t0);
      const raw=remain<400
        ? ((document.body&&document.body.textContent)||'')
        : ((document.body&&document.body.innerText)||'');
      text=String(raw).replace(/\s+\n/g,'\n').replace(/\n{{3,}}/g,'\n\n').trim();
      if(text.length>MAX_TEXT){{text=text.slice(0,MAX_TEXT);truncated=true;}}
    }}catch(_e){{text='';}}
  }}else{{ partial=true; }}
  return {{
    snapshot:lines.join('\n'),
    refs,
    text,
    partial,
    truncated,
    itemCount:lines.length,
    elapsedMs:Date.now()-t0
  }};
}})()"#,
        budget_ms = budget_ms,
        max_items = max_items,
        max_text_chars = max_text_chars
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn resolve_ref_selector() {
        assert_eq!(resolve_selector("ref:e12"), "css:[data-drs-ref=\"e12\"]");
        assert_eq!(resolve_selector("css:button.ok"), "css:button.ok");
        assert_eq!(resolve_selector("ref:../evil"), "ref:../evil");
    }

    #[test]
    fn parse_snapshot_value() {
        let v = json!({
            "snapshot": "- button \"OK\" [ref=e1]",
            "refs": [{"id":"e1","role":"button","name":"OK","selector":"css:[data-drs-ref=\"e1\"]"}],
            "text": "hello",
            "partial": false,
            "truncated": true,
            "itemCount": 1,
            "elapsedMs": 12
        });
        let snap = AiSnapshot::from_value(&v).unwrap();
        assert_eq!(snap.refs.len(), 1);
        assert_eq!(snap.refs[0].0, "e1");
        assert!(snap.truncated);
        let fields = snap.into_json_fields();
        assert_eq!(fields["refs"]["e1"]["role"], "button");
    }

    #[test]
    fn js_contains_budget_and_ref_attr() {
        let js = ai_snapshot_js(1500, 100, 4000);
        assert!(js.contains("BUDGET=1500"));
        assert!(js.contains("MAX=100"));
        assert!(js.contains("data-drs-ref"));
        assert!(js.contains("interesting") || js.contains("INTERACTIVE"));
    }
}
