//! CDP **JS Hook 工具箱** [`ChromiumHook`](`tab.hook()`):一键 hook 常见 sink,命中把
//! **参数 + 调用栈**回传 Rust;hook `crypto.subtle`/`CryptoJS` 即 **crypto tap —— 直接偷 key/iv/明文**。
//!
//! 很多站不必复现算法,**直接偷密钥**最快。本模块复用 `expose_function` 同款基建
//! (`Runtime.addBinding` + `Page.addScriptToEvaluateOnNewDocument` + `Runtime.bindingCalled`),
//! 注入的 wrapper **全程 try/catch、绝不弄坏页面**,且捕获原始 `JSON.stringify`/`btoa` 防自递归。
//!
//! ```no_run
//! use drission::prelude::*;
//! # async fn f(tab: ChromiumTab) -> drission::Result<()> {
//! // 导航前装好 hook(crypto + json + 调用栈),再触发页面逻辑:
//! let hook = tab.hook().crypto().json().with_stack().start().await?;
//! // ... 触发签名/加密 ...
//! while let Some(hit) = hook.wait(None).await? {
//!     println!("[{}] {} args={:?}", hit.sink, hit.func, hit.args);
//!     // crypto.subtle.encrypt 的明文在 args[2](bytes→base64),CryptoKey 元信息在 args[1]
//! }
//! # Ok(()) }
//! ```

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::Mutex;
use tokio::sync::broadcast::error::RecvError;
use tokio::task::AbortHandle;
use tokio::time::{Instant, sleep};

use super::core::CdpCore;
use crate::Result;

/// 回传 Rust 的绑定名(全局唯一,避免与页面/`expose_function` 冲突)。
const HOOK_BINDING: &str = "__drission_hook_emit";

/// 缓冲上限(超出丢最旧)。
const BUFFER_CAP: usize = 600;

/// 可 hook 的 sink 类别。
#[derive(Debug, Clone)]
enum HookSink {
    CryptoSubtle,
    CryptoJs,
    Json,
    Base64,
    Xhr,
    Fetch,
    Eval,
    FunctionCtor,
    Custom(String),
}

/// 一次 hook 命中(页面调用了被 hook 的函数)。
#[derive(Debug, Clone)]
pub struct HookHit {
    /// sink 类别(如 `crypto.subtle` / `CryptoJS.AES.encrypt` / `XHR.send` / `custom:app.sign`)。
    pub sink: String,
    /// 被调函数名。
    pub func: String,
    /// 调用实参(已安全序列化:`ArrayBuffer`/`TypedArray`→`{__t:"bytes",b64,len}`、
    /// `CryptoKey`→`{__t:"CryptoKey",algorithm,...}`、循环引用降级为字符串)。
    pub args: Vec<Value>,
    /// 调用栈(`with_stack` 开启时;`new Error().stack`)。
    pub stack: String,
    /// 命中时间戳(`Date.now()`,毫秒)。
    pub ts: f64,
}

impl HookHit {
    /// 取第 `i` 个实参。
    pub fn arg(&self, i: usize) -> Option<&Value> {
        self.args.get(i)
    }

    /// 取第 `i` 个实参的字符串形态(字符串原样;bytes 记录取 base64;其余 JSON 串)。
    pub fn arg_str(&self, i: usize) -> String {
        match self.args.get(i) {
            Some(Value::String(s)) => s.clone(),
            Some(v) if v.get("__t").and_then(|t| t.as_str()) == Some("bytes") => {
                v["b64"].as_str().unwrap_or_default().to_string()
            }
            Some(v) if v.get("__t").and_then(|t| t.as_str()) == Some("str") => {
                v["v"].as_str().unwrap_or_default().to_string()
            }
            Some(Value::Null) | None => String::new(),
            Some(v) => v.to_string(),
        }
    }

    /// 若第 `i` 个实参是 bytes 记录,返回其 base64;否则 `None`(crypto tap 取明文/密文用)。
    pub fn arg_bytes_b64(&self, i: usize) -> Option<String> {
        let v = self.args.get(i)?;
        if v.get("__t").and_then(|t| t.as_str()) == Some("bytes") {
            v["b64"].as_str().map(str::to_string)
        } else {
            None
        }
    }
}

/// Hook 构建器(`tab.hook()`)。链式选 sink,`start()` 装载并返回 [`HookSession`]。
pub struct ChromiumHook {
    core: Arc<CdpCore>,
    sinks: Vec<HookSink>,
    with_stack: bool,
}

impl ChromiumHook {
    pub(crate) fn new(core: Arc<CdpCore>) -> Self {
        Self {
            core,
            sinks: Vec::new(),
            with_stack: false,
        }
    }

    /// hook `crypto.subtle.{encrypt,decrypt,sign,verify,digest}`(WebCrypto)。
    pub fn crypto_subtle(mut self) -> Self {
        self.sinks.push(HookSink::CryptoSubtle);
        self
    }
    /// hook `CryptoJS` 的哈希/HMAC/对称加密(`AES/DES/...` 的 encrypt/decrypt)。
    pub fn crypto_js(mut self) -> Self {
        self.sinks.push(HookSink::CryptoJs);
        self
    }
    /// crypto tap 便捷:同时 hook `crypto.subtle` 与 `CryptoJS`。
    pub fn crypto(self) -> Self {
        self.crypto_subtle().crypto_js()
    }
    /// hook `JSON.stringify` / `JSON.parse`(看签名串拼装/响应解析)。
    pub fn json(mut self) -> Self {
        self.sinks.push(HookSink::Json);
        self
    }
    /// hook `btoa` / `atob`(base64 编解码)。
    pub fn base64(mut self) -> Self {
        self.sinks.push(HookSink::Base64);
        self
    }
    /// hook `XMLHttpRequest.{open,send,setRequestHeader}`(看请求怎么拼、带了哪些签名头)。
    pub fn xhr(mut self) -> Self {
        self.sinks.push(HookSink::Xhr);
        self
    }
    /// hook `window.fetch`。
    pub fn fetch(mut self) -> Self {
        self.sinks.push(HookSink::Fetch);
        self
    }
    /// hook `window.eval`(看动态执行的代码)。
    pub fn eval(mut self) -> Self {
        self.sinks.push(HookSink::Eval);
        self
    }
    /// hook 全局 `Function` 构造器(**较激进**,个别站点会因此异常,按需开)。
    pub fn function_constructor(mut self) -> Self {
        self.sinks.push(HookSink::FunctionCtor);
        self
    }
    /// hook **任意全局路径**的函数,如 `"app.sign"` / `"window.X.encrypt"`(万能定位)。
    pub fn custom(mut self, path: impl Into<String>) -> Self {
        self.sinks.push(HookSink::Custom(path.into()));
        self
    }
    /// 常用全家桶:crypto + json + base64 + xhr + fetch。
    pub fn common(self) -> Self {
        self.crypto().json().base64().xhr().fetch()
    }
    /// 命中时一并回传 `new Error().stack` 调用栈(定位「哪一行调用的」)。
    pub fn with_stack(mut self) -> Self {
        self.with_stack = true;
        self
    }

    /// 装载 hook(`Runtime.enable` + `addBinding` + 导航前注入 + 当前文档即时注入),返回会话句柄。
    pub async fn start(self) -> Result<HookSession> {
        self.core.send("Runtime.enable", json!({})).await?;
        self.core
            .send("Runtime.addBinding", json!({ "name": HOOK_BINDING }))
            .await?;

        let source = build_hook_js(&self.sinks, self.with_stack);
        self.core
            .send(
                "Page.addScriptToEvaluateOnNewDocument",
                json!({ "source": source }),
            )
            .await?;
        let _ = self.core.eval_value(&source).await; // 当前文档也即时生效

        let buf: Arc<Mutex<VecDeque<HookHit>>> = Arc::new(Mutex::new(VecDeque::new()));
        let task = tokio::spawn(hook_pump(
            self.core.conn.clone(),
            self.core.session_id.clone(),
            buf.clone(),
        ));
        Ok(HookSession {
            core: self.core.clone(),
            buf,
            abort: task.abort_handle(),
        })
    }
}

/// Hook 会话([`ChromiumHook::start`] 返回)。`wait`/`wait_count` 拉命中;drop / `stop` 即停。
pub struct HookSession {
    core: Arc<CdpCore>,
    buf: Arc<Mutex<VecDeque<HookHit>>>,
    abort: AbortHandle,
}

impl HookSession {
    /// 等**一个**命中(`timeout=None` 用标签默认超时);超时返回 `None`。
    pub async fn wait(&self, timeout: Option<Duration>) -> Result<Option<HookHit>> {
        let deadline = Instant::now() + timeout.unwrap_or_else(|| self.core.timeout());
        loop {
            if let Some(h) = self.buf.lock().await.pop_front() {
                return Ok(Some(h));
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            sleep(Duration::from_millis(40)).await;
        }
    }

    /// 在总超时内尽量收集 `n` 个命中(不足返回已收到的)。
    pub async fn wait_count(&self, n: usize, timeout: Option<Duration>) -> Result<Vec<HookHit>> {
        let deadline = Instant::now() + timeout.unwrap_or_else(|| self.core.timeout());
        let mut out = Vec::with_capacity(n);
        while out.len() < n {
            if let Some(h) = self.buf.lock().await.pop_front() {
                out.push(h);
                continue;
            }
            if Instant::now() >= deadline {
                break;
            }
            sleep(Duration::from_millis(40)).await;
        }
        Ok(out)
    }

    /// 取走当前已缓冲的全部命中(不等待)。
    pub async fn drain(&self) -> Vec<HookHit> {
        self.buf.lock().await.drain(..).collect()
    }

    /// 停止 hook 会话(中止泵 + 移除绑定)。注入的页面 wrapper 仍在(无害,会调一个空绑定;
    /// 可刷新页面彻底清除)。
    pub async fn stop(self) -> Result<()> {
        self.abort.abort();
        let _ = self
            .core
            .send("Runtime.removeBinding", json!({ "name": HOOK_BINDING }))
            .await;
        Ok(())
    }
}

impl Drop for HookSession {
    fn drop(&mut self) {
        self.abort.abort();
    }
}

/// 后台泵:订阅 `Runtime.bindingCalled`,解析回传 JSON 成 [`HookHit`] 推入缓冲。
async fn hook_pump(
    conn: crate::protocol::Connection,
    session_id: String,
    buf: Arc<Mutex<VecDeque<HookHit>>>,
) {
    let mut events = conn.subscribe();
    loop {
        let ev = match events.recv().await {
            Ok(ev) => ev,
            Err(RecvError::Lagged(_)) => continue,
            Err(RecvError::Closed) => break,
        };
        if ev.session_id.as_deref() != Some(session_id.as_str()) {
            continue;
        }
        if ev.method != "Runtime.bindingCalled" {
            continue;
        }
        if ev.params["name"].as_str() != Some(HOOK_BINDING) {
            continue;
        }
        let payload = ev.params["payload"].as_str().unwrap_or_default();
        if let Some(hit) = parse_hit(payload) {
            let mut g = buf.lock().await;
            if g.len() >= BUFFER_CAP {
                g.pop_front();
            }
            g.push_back(hit);
        }
    }
}

// ════════════════════════════════════════════════════════════════════════════
// 纯函数:命中解析 / 注入脚本构建(可单测,不触网)
// ════════════════════════════════════════════════════════════════════════════

/// 解析页面回传的一条记录 JSON。
fn parse_hit(payload: &str) -> Option<HookHit> {
    let v: Value = serde_json::from_str(payload).ok()?;
    Some(HookHit {
        sink: v["sink"].as_str().unwrap_or_default().to_string(),
        func: v["func"].as_str().unwrap_or_default().to_string(),
        ts: v["ts"].as_f64().unwrap_or(0.0),
        args: v["args"].as_array().cloned().unwrap_or_default(),
        stack: v["stack"].as_str().unwrap_or_default().to_string(),
    })
}

/// 据所选 sink 生成注入脚本(占位符替换,避免 `format!` 海量花括号转义)。
fn build_hook_js(sinks: &[HookSink], with_stack: bool) -> String {
    let (mut crypto_subtle, mut crypto_js, mut js, mut base64) = (false, false, false, false);
    let (mut xhr, mut fetch, mut eval, mut fn_ctor) = (false, false, false, false);
    let mut custom: Vec<&str> = Vec::new();
    for s in sinks {
        match s {
            HookSink::CryptoSubtle => crypto_subtle = true,
            HookSink::CryptoJs => crypto_js = true,
            HookSink::Json => js = true,
            HookSink::Base64 => base64 = true,
            HookSink::Xhr => xhr = true,
            HookSink::Fetch => fetch = true,
            HookSink::Eval => eval = true,
            HookSink::FunctionCtor => fn_ctor = true,
            HookSink::Custom(p) => custom.push(p.as_str()),
        }
    }
    let flags = json!({
        "cryptoSubtle": crypto_subtle,
        "cryptoJs": crypto_js,
        "json": js,
        "base64": base64,
        "xhr": xhr,
        "fetch": fetch,
        "eval": eval,
        "fnCtor": fn_ctor,
    });
    HOOK_JS_TEMPLATE
        .replace("__BIND__", HOOK_BINDING)
        .replace("__STACK__", if with_stack { "true" } else { "false" })
        .replace("__FLAGS__", &flags.to_string())
        .replace("__CUSTOM__", &json!(custom).to_string())
}

/// 注入脚本模板。占位符:`__BIND__`(绑定名)/ `__STACK__`(是否抓栈)/ `__FLAGS__`(sink 开关 JSON)/
/// `__CUSTOM__`(自定义路径 JSON 数组)。**关键防递归**:`emit`/`rep` 用启动时捕获的原始
/// `JSON.stringify`/`btoa`,故即便 hook 了它们也不会自调用。
const HOOK_JS_TEMPLATE: &str = r#"(function(){
  try{
    var BIND="__BIND__"; var WITH_STACK=__STACK__; var F=__FLAGS__; var CUSTOM=__CUSTOM__;
    if(typeof window[BIND]!=='function') return;
    var _stringify=JSON.stringify, _parse=JSON.parse;
    var _btoa=(typeof btoa!=='undefined')?btoa:function(s){return '';};
    var _Error=Error;
    function ab2b64(u8){ try{var s='';for(var i=0;i<u8.length;i++)s+=String.fromCharCode(u8[i]);return _btoa(s);}catch(e){return '';} }
    function rep(a){
      try{
        if(a===undefined) return {__t:'undefined'};
        if(a===null) return null;
        var t=typeof a;
        if(t==='string'||t==='number'||t==='boolean') return a;
        if(t==='function') return {__t:'fn',src:String(a).slice(0,300)};
        if(a instanceof ArrayBuffer) return {__t:'bytes',b64:ab2b64(new Uint8Array(a)),len:a.byteLength};
        if(ArrayBuffer.isView&&ArrayBuffer.isView(a)) return {__t:'bytes',b64:ab2b64(new Uint8Array(a.buffer,a.byteOffset,a.byteLength)),len:a.byteLength};
        if(typeof CryptoKey!=='undefined'&&a instanceof CryptoKey) return {__t:'CryptoKey',algorithm:a.algorithm,type:a.type,extractable:a.extractable,usages:a.usages};
        return _parse(_stringify(a,function(k,v){
          if(v instanceof ArrayBuffer) return {__t:'bytes',b64:ab2b64(new Uint8Array(v)),len:v.byteLength};
          if(ArrayBuffer.isView&&ArrayBuffer.isView(v)) return {__t:'bytes',b64:ab2b64(new Uint8Array(v.buffer,v.byteOffset,v.byteLength)),len:v.byteLength};
          return v;
        }));
      }catch(e){ try{return {__t:'str',v:String(a).slice(0,2000)};}catch(e2){return {__t:'err'};} }
    }
    function emit(sink,func,args){
      try{
        var rec={sink:sink,func:func,ts:Date.now(),args:[]};
        for(var i=0;i<args.length;i++) rec.args.push(rep(args[i]));
        if(WITH_STACK){ try{rec.stack=(new _Error()).stack||'';}catch(e){} }
        window[BIND](_stringify(rec));
      }catch(e){}
    }
    function wrap(obj,name,sink){
      try{
        if(!obj) return;
        var orig=obj[name];
        if(typeof orig!=='function'||orig.__drission_wrapped) return;
        var w=function(){ emit(sink,name,Array.prototype.slice.call(arguments)); return orig.apply(this,arguments); };
        try{w.__drission_wrapped=true;}catch(e){}
        try{w.toString=function(){return orig.toString();};}catch(e){}
        obj[name]=w;
      }catch(e){}
    }
    if(F.cryptoSubtle&&window.crypto&&crypto.subtle){ ['encrypt','decrypt','sign','verify','digest'].forEach(function(m){wrap(crypto.subtle,m,'crypto.subtle');}); }
    if(F.cryptoJs&&window.CryptoJS){ var C=window.CryptoJS;
      ['MD5','SHA1','SHA256','SHA512','SHA3','HmacMD5','HmacSHA1','HmacSHA256','HmacSHA512'].forEach(function(m){ if(typeof C[m]==='function') wrap(C,m,'CryptoJS.'+m); });
      ['AES','DES','TripleDES','RC4','Rabbit','RabbitLegacy'].forEach(function(m){ if(C[m]){ wrap(C[m],'encrypt','CryptoJS.'+m+'.encrypt'); wrap(C[m],'decrypt','CryptoJS.'+m+'.decrypt'); } }); }
    if(F.json){ wrap(JSON,'stringify','JSON.stringify'); wrap(JSON,'parse','JSON.parse'); }
    if(F.base64){ wrap(window,'btoa','btoa'); wrap(window,'atob','atob'); }
    if(F.xhr&&window.XMLHttpRequest){ var P=XMLHttpRequest.prototype; wrap(P,'open','XHR.open'); wrap(P,'send','XHR.send'); wrap(P,'setRequestHeader','XHR.setRequestHeader'); }
    if(F.fetch&&window.fetch){ wrap(window,'fetch','fetch'); }
    if(F.eval){ wrap(window,'eval','eval'); }
    if(F.fnCtor&&window.Function){ wrap(window,'Function','Function'); }
    if(CUSTOM&&CUSTOM.length){ CUSTOM.forEach(function(p){ try{ var parts=p.replace(/^window\./,'').split('.'); var nm=parts.pop(); var o=window; for(var i=0;i<parts.length;i++){o=o[parts[i]]; if(!o) return;} wrap(o,nm,'custom:'+p); }catch(e){} }); }
  }catch(e){}
})();"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_hit_full_record() {
        let h = parse_hit(
            r#"{"sink":"crypto.subtle","func":"encrypt","ts":123.0,
               "args":[{"name":"AES-CBC"},{"__t":"CryptoKey","type":"secret"},
                       {"__t":"bytes","b64":"aGk=","len":2}],"stack":"at sign (app.js:1)"}"#,
        )
        .unwrap();
        assert_eq!(h.sink, "crypto.subtle");
        assert_eq!(h.func, "encrypt");
        assert_eq!(h.ts, 123.0);
        assert_eq!(h.args.len(), 3);
        assert!(h.stack.contains("app.js"));
        // 明文 bytes 取 base64。
        assert_eq!(h.arg_bytes_b64(2).as_deref(), Some("aGk="));
        assert_eq!(h.arg_str(2), "aGk=");
        assert!(h.arg_bytes_b64(0).is_none());
    }

    #[test]
    fn parse_hit_bad_json_is_none() {
        assert!(parse_hit("not json").is_none());
    }

    #[test]
    fn build_js_inlines_flags_and_binding() {
        let js = build_hook_js(&[HookSink::CryptoSubtle, HookSink::Json], true);
        // 绑定名替换。
        assert!(js.contains(HOOK_BINDING));
        assert!(!js.contains("__BIND__"));
        // 选中的开关为 true、未选的为 false。
        assert!(js.contains("\"cryptoSubtle\":true"));
        assert!(js.contains("\"json\":true"));
        assert!(js.contains("\"fetch\":false"));
        // with_stack。
        assert!(js.contains("var WITH_STACK=true"));
        // 占位符全部替换干净。
        assert!(
            !js.contains("__STACK__") && !js.contains("__FLAGS__") && !js.contains("__CUSTOM__")
        );
    }

    #[test]
    fn build_js_custom_paths() {
        let js = build_hook_js(&[HookSink::Custom("app.sign".into())], false);
        assert!(js.contains("\"app.sign\""));
        assert!(js.contains("var WITH_STACK=false"));
    }

    #[test]
    fn hit_arg_helpers() {
        let h = HookHit {
            sink: "x".into(),
            func: "f".into(),
            args: vec![json!("hello"), json!({"__t":"str","v":"big"}), json!(42)],
            stack: String::new(),
            ts: 0.0,
        };
        assert_eq!(h.arg_str(0), "hello");
        assert_eq!(h.arg_str(1), "big");
        assert_eq!(h.arg_str(2), "42");
        assert_eq!(h.arg_str(9), "");
        assert!(h.arg(0).is_some());
        assert!(h.arg(9).is_none());
    }
}
