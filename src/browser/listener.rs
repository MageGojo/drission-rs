//! 网络监听(对应 DrissionPage 的 `tab.listen`)。
//!
//! 实现方式:在页面注入 **fetch/XHR hook**(导航前用 `Page.setInitScripts`,并对当前文档
//! 立即注入一次),把匹配 URL 的请求连同**响应体**推入页面全局队列;Rust 侧轮询取回。
//!
//! 为什么不用 `Network.getResponseBody`:Camoufox/Juggler 对部分(尤其较大的)响应取体会报
//! `NS_ERROR_FAILURE [onDataAvailable]`,而页面层 hook 能稳定拿到**同源**及**带 CORS** 的
//! 响应体。局限:无 CORS 的跨域 opaque 响应,页面层读不到 body(此时 body 为空字符串)。

use std::collections::VecDeque;

use serde_json::Value;

/// 后端无关的网络数据类型(`DataPacket`/`RequestData`/`ResponseData`/`ListenFilter`)统一定义在
/// [`crate::net`];此处再导出,保持 `browser::listener::*` 老路径可用。
pub use crate::net::{DataPacket, ListenFilter, RequestData, ResponseData};

/// 监听缓冲(放在 `TabCore` 的 `Mutex` 中):尚未被 `listen_wait`/`listen_next` 消费的包。
pub(crate) type ListenBuffer = VecDeque<DataPacket>;

/// 取回并清空页面队列的 JS(返回数组,由 `evaluate` 的 returnByValue 取回)。
pub(crate) const DRAIN_JS: &str =
    "(function(){var q=window.__drission_net_q||[];window.__drission_net_q=[];return q;})()";

/// 停止 hook:清空队列并允许后续重新安装(无法完美还原被包裹的 fetch,但足够)。
pub(crate) const UNINSTALL_JS: &str =
    "(function(){window.__drission_net_q=[];window.__drission_net_installed=false;})()";

/// 生成注入页面的 fetch/XHR hook 脚本(按 URL 关键词过滤;空=全部)。
pub(crate) fn hook_script(filter: &ListenFilter) -> String {
    let kw = serde_json::to_string(&filter.url_keywords).unwrap_or_else(|_| "[]".to_string());
    HOOK_TEMPLATE.replace("__KW__", &kw)
}

/// 把页面队列项(JSON 数组)转成 [`DataPacket`] 列表。
pub(crate) fn parse_packets(v: &Value) -> Vec<DataPacket> {
    let Some(arr) = v.as_array() else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(arr.len());
    for it in arr {
        out.push(DataPacket {
            url: it["url"].as_str().unwrap_or_default().to_string(),
            method: it["method"].as_str().unwrap_or_default().to_string(),
            resource_type: it["type"].as_str().unwrap_or_default().to_string(),
            request: RequestData {
                headers: Vec::new(),
                post_data: it["reqBody"].as_str().map(str::to_string),
            },
            response: ResponseData {
                status: it["status"].as_u64().unwrap_or(0) as u16,
                status_text: it["statusText"].as_str().unwrap_or_default().to_string(),
                headers: parse_pairs(&it["headers"]),
                body: it["body"].as_str().unwrap_or_default().to_string(),
                body_base64: String::new(),
            },
        });
    }
    out
}

/// 解析 hook 上报的 `[[name,value], ...]` 头数组。
fn parse_pairs(v: &Value) -> Vec<(String, String)> {
    v.as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|p| {
                    let pair = p.as_array()?;
                    Some((
                        pair.first()?.as_str()?.to_string(),
                        pair.get(1)?.as_str().unwrap_or_default().to_string(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// 把 Juggler 的 `[{name,value}, ...]` 头数组解析成键值对(供请求拦截复用)。
pub(crate) fn parse_headers(v: &Value) -> Vec<(String, String)> {
    v.as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|h| {
                    Some((
                        h["name"].as_str()?.to_string(),
                        h["value"].as_str().unwrap_or_default().to_string(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// fetch/XHR hook 模板。`__KW__` 会被替换为关键词 JSON 数组。
const HOOK_TEMPLATE: &str = r#"(function(){
  if (window.__drission_net_installed) { window.__drission_net_kw = __KW__; return; }
  window.__drission_net_installed = true;
  window.__drission_net_kw = __KW__;
  window.__drission_net_q = window.__drission_net_q || [];
  function kw(){ return window.__drission_net_kw || []; }
  function match(u){ if(!u) return false; var k=kw(); if(k.length===0) return true; for(var i=0;i<k.length;i++){ if(String(u).indexOf(k[i])>=0) return true; } return false; }
  function push(p){ try{ var q=window.__drission_net_q; if(q.length<300) q.push(p); }catch(e){} }
  try {
    if (window.fetch){
      var of = window.fetch;
      window.fetch = function(input, init){
        var url=''; try{ url=(typeof input==='string')?input:((input&&input.url)||''); }catch(e){}
        var method='GET'; try{ method=(init&&init.method)||(input&&input.method)||'GET'; }catch(e){}
        var pr = of.apply(this, arguments);
        try {
          if (match(url)){
            pr.then(function(resp){
              try{
                var hs=[]; try{ resp.headers.forEach(function(v,k){ hs.push([k,v]); }); }catch(e){}
                resp.clone().text().then(function(body){
                  push({url:url, method:method, status:resp.status, statusText:resp.statusText||'', headers:hs, body:body||'', type:'fetch'});
                }).catch(function(){ push({url:url, method:method, status:resp.status, statusText:resp.statusText||'', headers:hs, body:'', type:'fetch'}); });
              }catch(e){}
              return resp;
            }).catch(function(){});
          }
        } catch(e){}
        return pr;
      };
    }
  } catch(e){}
  try {
    var oo = XMLHttpRequest.prototype.open;
    var os = XMLHttpRequest.prototype.send;
    XMLHttpRequest.prototype.open = function(m,u){ try{ this.__d_url=u; this.__d_m=m; }catch(e){} return oo.apply(this, arguments); };
    XMLHttpRequest.prototype.send = function(b){
      var self=this;
      try {
        self.addEventListener('load', function(){
          try{
            if(!match(self.__d_url)) return;
            var body=''; try{ var rt=self.responseType; if(!rt||rt==='text'){ body=self.responseText; } else if(rt==='json'){ body=JSON.stringify(self.response); } }catch(e){ body=''; }
            var hs=[]; try{ var raw=self.getAllResponseHeaders()||''; raw.trim().split(/[\r\n]+/).forEach(function(line){ var i=line.indexOf(':'); if(i>0) hs.push([line.slice(0,i).trim(), line.slice(i+1).trim()]); }); }catch(e){}
            push({url:self.__d_url, method:self.__d_m||'GET', status:self.status, statusText:self.statusText||'', headers:hs, body:body||'', reqBody:(typeof b==='string'?b:null), type:'xhr'});
          }catch(e){}
        });
      } catch(e){}
      return os.apply(this, arguments);
    };
  } catch(e){}
})()"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn hook_script_embeds_keywords() {
        let s = hook_script(&ListenFilter {
            url_keywords: vec!["aweme/detail".into()],
            xhr_only: false,
        });
        assert!(s.contains("aweme/detail"));
        assert!(!s.contains("__KW__"));
    }

    #[test]
    fn parse_packets_basic() {
        let v = json!([{
            "url":"https://x.com/api","method":"GET","status":200,"statusText":"OK",
            "headers":[["content-type","application/json"]],"body":"{\"a\":1}","type":"fetch"
        }]);
        let ps = parse_packets(&v);
        assert_eq!(ps.len(), 1);
        assert_eq!(ps[0].url, "https://x.com/api");
        assert_eq!(ps[0].response.status, 200);
        assert_eq!(ps[0].response.body, "{\"a\":1}");
        assert_eq!(ps[0].response.headers[0].0, "content-type");
    }
}
