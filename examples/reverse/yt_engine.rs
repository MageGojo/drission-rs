//! YouTube 纯协议引擎(脱浏览器):被 `re_yt_protocol` 与 `yt_dl` 共用(`#[path]` mod 引入)。
//!
//! 职责:① reqwest 抓 watch 页(拿 InnerTube key/版本/visitor + base.js URL)与 base.js;
//! ② InnerTube `/youtubei/v1/player` 取**完整分轨**(默认 `TVHTML5` 客户端——免 pot 即给全部 adaptive,
//! 含 4K,均带 signatureCipher);③ 内嵌 QuickJS 跑整份 base.js,把 `y2`(sig+nsig 统一 VM 的直链构建器)
//! 抠成 oracle,自算每个格式的终极直链;④ 进度条下载 + ffmpeg 合并 + pot 透传。
//!
//! VM 不逐 opcode 重写(随 base.js 轮换而碎、无收益,yt-dlp 同理),而是「整份喂进 QuickJS」。

#![allow(dead_code)]

use std::io::Write as _;
use std::process::Command;

use futures_util::StreamExt;
use rquickjs::context::EvalOptions;
use rquickjs::{CatchResultExt, Context, Ctx, Runtime};
use serde_json::{Value, json};

pub const UA_WEB: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Safari/537.36";
/// TV(TVHTML5)客户端版本——免 pot 拿全分轨的关键;YouTube 偶尔更新,失效时调高这里(或设 YT_TV_VER)。
pub const TV_CLIENT_VERSION: &str = "7.20250120.19.00";
pub const TV_UA: &str =
    "Mozilla/5.0 (PlayStation; PlayStation 4/12.00) AppleWebKit/605.1.15 (KHTML, like Gecko)";

// ════════════════════════════════════════════════════════════════════════════
// 抓取 watch 页 / base.js / InnerTube
// ════════════════════════════════════════════════════════════════════════════

pub struct WatchMeta {
    pub api_key: String,
    pub web_client_version: String,
    pub visitor_data: String,
    pub base_js_url: String,
}

pub async fn fetch_text(client: &reqwest::Client, url: &str, ua: &str) -> Option<String> {
    client
        .get(url)
        .header("User-Agent", ua)
        .header("Accept-Language", "en-US,en;q=0.9")
        .send()
        .await
        .ok()?
        .text()
        .await
        .ok()
}

pub fn parse_watch_meta(html: &str) -> Option<WatchMeta> {
    let api_key = json_str_field(html, "INNERTUBE_API_KEY")
        .or_else(|| std::env::var("YT_INNERTUBE_API_KEY").ok())
        .unwrap_or_default();
    let web_client_version = json_str_field(html, "INNERTUBE_CONTEXT_CLIENT_VERSION")
        .unwrap_or_else(|| "2.20240101.00.00".to_string());
    let visitor_data = json_str_field(html, "VISITOR_DATA").unwrap_or_default();
    let base_js_url = extract_base_js_url(html)?;
    Some(WatchMeta {
        api_key,
        web_client_version,
        visitor_data,
        base_js_url,
    })
}

/// 从 base.js 抓 `signatureTimestamp`(InnerTube 取密文格式必需)。
pub fn parse_sts(base_js: &str) -> Option<u64> {
    let i = base_js.find("signatureTimestamp:")?;
    let rest = &base_js[i + "signatureTimestamp:".len()..];
    let num: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    num.parse().ok()
}

/// InnerTube 客户端(默认 TV:免 pot 给全分轨)。
#[derive(Clone, Copy, PartialEq)]
pub enum Client {
    Tv,
    Web,
}

/// 调 InnerTube `/youtubei/v1/player`,返回完整 player response(含 streamingData)。
/// `pot` 非空时放进 `serviceIntegrityDimensions.poToken`——WEB 客户端有了它才会**返回** adaptive URL。
pub async fn innertube_player(
    client: &reqwest::Client,
    video_id: &str,
    kind: Client,
    meta: &WatchMeta,
    sts: u64,
    pot: &str,
) -> Option<Value> {
    if meta.api_key.trim().is_empty() {
        return None;
    }
    let (cname, cver, ua) = match kind {
        Client::Tv => (
            "TVHTML5",
            std::env::var("YT_TV_VER").unwrap_or_else(|_| TV_CLIENT_VERSION.to_string()),
            TV_UA,
        ),
        Client::Web => ("WEB", meta.web_client_version.clone(), UA_WEB),
    };
    let mut ctx_client =
        json!({"clientName": cname, "clientVersion": cver, "hl": "en", "gl": "US"});
    if !meta.visitor_data.is_empty() {
        ctx_client["visitorData"] = json!(meta.visitor_data);
    }
    let mut body = json!({
        "videoId": video_id,
        "context": {"client": ctx_client, "thirdParty": {"embedUrl": "https://www.youtube.com/"}},
        "playbackContext": {"contentPlaybackContext": {"signatureTimestamp": sts, "html5Preference": "HTML5_PREF_WANTS"}},
        "contentCheckOk": true,
        "racyCheckOk": true
    });
    if !pot.is_empty() {
        body["serviceIntegrityDimensions"] = json!({ "poToken": pot });
    }
    let url = format!(
        "https://www.youtube.com/youtubei/v1/player?key={}",
        meta.api_key
    );
    let resp = client
        .post(&url)
        .header("User-Agent", ua)
        .header("Content-Type", "application/json")
        .header("X-Goog-Visitor-Id", &meta.visitor_data)
        .json(&body)
        .send()
        .await
        .ok()?;
    resp.json::<Value>().await.ok()
}

// ════════════════════════════════════════════════════════════════════════════
// 格式模型 / 选择
// ════════════════════════════════════════════════════════════════════════════

#[derive(Clone)]
pub struct Format {
    pub itag: i64,
    pub mime: String,
    pub codec: String,
    pub kind: Kind,
    pub quality: String, // qualityLabel(视频)或 audioQuality
    pub width: i64,
    pub height: i64,
    pub fps: i64,
    pub bitrate: i64,
    pub content_length: u64,
    pub raw_url: String,
    pub sp: String,
    pub s: String,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    Muxed,
    Video,
    Audio,
}

impl Format {
    pub fn ext(&self) -> &'static str {
        if self.mime.contains("webm") {
            "webm"
        } else if self.mime.contains("audio/mp4") {
            "m4a"
        } else {
            "mp4"
        }
    }
    pub fn human_size(&self) -> String {
        human_bytes(self.content_length)
    }
    /// 编码短名(avc1→h264 / av01→av1 / vp9 / mp4a→aac / opus…)。
    pub fn codec_short(&self) -> String {
        let c = self.codec.to_lowercase();
        if c.starts_with("avc1") || c.starts_with("h264") {
            "h264".into()
        } else if c.starts_with("av01") {
            "av1".into()
        } else if c.starts_with("vp9") || c.starts_with("vp09") {
            "vp9".into()
        } else if c.starts_with("vp8") {
            "vp8".into()
        } else if c.starts_with("mp4a") {
            "aac".into()
        } else if c.starts_with("opus") {
            "opus".into()
        } else if c.starts_with("ec-3") || c.starts_with("ac-3") {
            "ac3".into()
        } else {
            self.codec.split('.').next().unwrap_or("?").to_string()
        }
    }
}

pub fn list_formats(pr: &Value) -> Vec<Format> {
    let sd = match pr.get("streamingData") {
        Some(s) => s,
        None => return Vec::new(),
    };
    let mut out = Vec::new();
    for (key, muxed) in [("formats", true), ("adaptiveFormats", false)] {
        if let Some(arr) = sd.get(key).and_then(Value::as_array) {
            for f in arr {
                if let Some(fmt) = to_format(f, muxed) {
                    out.push(fmt);
                }
            }
        }
    }
    out
}

fn to_format(f: &Value, muxed: bool) -> Option<Format> {
    let mime_full = f
        .get("mimeType")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let mime = mime_full.split(';').next().unwrap_or("").trim().to_string();
    let codec = mime_full
        .split("codecs=")
        .nth(1)
        .map(|s| s.trim_matches('"').to_string())
        .unwrap_or_default();
    let has_v = f.get("width").is_some() || mime.starts_with("video");
    let has_a = mime.starts_with("audio") || f.get("audioQuality").is_some();
    let kind = if muxed
        || (mime.starts_with("video")
            && f.get("audioChannels").is_some()
            && f.get("width").is_some()
            && key_present(f, "audioQuality"))
    {
        Kind::Muxed
    } else if mime.starts_with("video") || has_v && !has_a {
        Kind::Video
    } else {
        Kind::Audio
    };
    // 取 url 或拆 signatureCipher。
    let (raw_url, sp, s) = if let Some(u) = f.get("url").and_then(Value::as_str) {
        (u.to_string(), String::new(), String::new())
    } else if let Some(cipher) = f.get("signatureCipher").and_then(Value::as_str) {
        let (mut url, mut sp, mut s) = (String::new(), "sig".to_string(), String::new());
        for kv in cipher.split('&') {
            if let Some((k, v)) = kv.split_once('=') {
                let v = url_decode(v);
                match k {
                    "url" => url = v,
                    "sp" => sp = v,
                    "s" => s = v,
                    _ => {}
                }
            }
        }
        (url, sp, s)
    } else {
        return None; // 无 url/cipher(被 pot 剥离)→ 跳过
    };
    Some(Format {
        itag: f.get("itag").and_then(Value::as_i64).unwrap_or(0),
        mime,
        codec,
        kind,
        quality: f
            .get("qualityLabel")
            .or_else(|| f.get("audioQuality"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        width: f.get("width").and_then(Value::as_i64).unwrap_or(0),
        height: f.get("height").and_then(Value::as_i64).unwrap_or(0),
        fps: f.get("fps").and_then(Value::as_i64).unwrap_or(0),
        bitrate: f.get("bitrate").and_then(Value::as_i64).unwrap_or(0),
        content_length: f
            .get("contentLength")
            .and_then(Value::as_str)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0),
        raw_url,
        sp,
        s,
    })
}

fn key_present(f: &Value, k: &str) -> bool {
    f.get(k).is_some()
}

/// 选择计划:要么单一 muxed/progressive,要么 视频+音频 双轨。
pub enum Plan {
    Single(Format),
    Mux(Format, Format),
}

/// 按清晰度选择。`q`:`best`/`1080`/`720`/`480`/`360`/`audio`/`itag:N`。
pub fn select_plan(formats: &[Format], q: &str) -> Option<Plan> {
    let videos: Vec<&Format> = formats.iter().filter(|f| f.kind == Kind::Video).collect();
    let audios: Vec<&Format> = formats.iter().filter(|f| f.kind == Kind::Audio).collect();
    let muxed: Vec<&Format> = formats.iter().filter(|f| f.kind == Kind::Muxed).collect();

    if let Some(rest) = q.strip_prefix("itag:") {
        let want: i64 = rest.parse().ok()?;
        let f = formats.iter().find(|f| f.itag == want)?.clone();
        return Some(Plan::Single(f));
    }
    if q == "audio" {
        let a = best_audio(&audios)?;
        return Some(Plan::Single(a.clone()));
    }
    let best_a = best_audio(&audios);
    // 目标高度。
    let target: Option<i64> = match q {
        "best" => None,
        _ => q.trim_end_matches('p').parse().ok(),
    };
    let pick_v = |vs: &[&Format]| -> Option<Format> {
        let mut cand: Vec<&&Format> = vs.iter().collect();
        cand.sort_by(|a, b| (b.height, b.fps, b.bitrate).cmp(&(a.height, a.fps, a.bitrate)));
        match target {
            None => cand.first().map(|f| (**f).clone()),
            Some(h) => cand
                .iter()
                .find(|f| f.height <= h)
                .or_else(|| cand.last())
                .map(|f| (**f).clone()),
        }
    };
    if let (Some(v), Some(a)) = (pick_v(&videos), best_a) {
        return Some(Plan::Mux(v, a.clone()));
    }
    // 退化:没有分轨视频 → 用 progressive。
    if let Some(m) = muxed.iter().max_by_key(|f| f.height).map(|f| (*f).clone()) {
        return Some(Plan::Single(m));
    }
    None
}

fn best_audio<'a>(audios: &'a [&'a Format]) -> Option<&'a Format> {
    audios.iter().max_by_key(|f| f.bitrate).copied()
}

// ════════════════════════════════════════════════════════════════════════════
// QuickJS Solver:整份 base.js 进引擎 + 注入惰性 oracle,自算直链
// ════════════════════════════════════════════════════════════════════════════

pub struct Solver {
    ctx: Context,
    _rt: Runtime,
    ready: bool,
}

impl Solver {
    /// 解析 base.js 的 y2/解扰类,跑整份 base.js,挂好 oracle。
    pub fn new(base_js: &str) -> Option<Solver> {
        let y2 = parse_url_builder_name(base_js)?;
        let nsig_class = parse_nsig_class(base_js)?;
        let url_oracle = build_url_oracle(&y2);
        let nsig_oracle = format!(
            "function(u){{try{{return new {nsig}(u,!0).get(\"n\")||\"\"}}catch(e){{return \"ERR:\"+e}}}}",
            nsig = nsig_class
        );
        let inject = format!(
            ";try{{__ytStash(g)}}catch(e){{}};try{{g.__ytUrl={url_oracle};g.__ytNsig={nsig_oracle};}}catch(e){{}}"
        );
        let patched = base_js.replacen("var window=this;", &format!("var window=this;{inject}"), 1);
        if patched.len() == base_js.len() {
            return None;
        }
        let setup = format!(
            "(function(){{var __t=globalThis;{ENV_STUBS}__t.window=__t;__t.self=__t;__t.top=__t;__t.parent=__t;__t.frames=__t;__t.frameElement=null;__t.__ytStash=function(ns){{__t.__ytns=ns;}};}})();void 0;"
        );
        let rt = Runtime::new().ok()?;
        let ctx = Context::full(&rt).ok()?;
        let ready = ctx.with(|ctx| {
            if js_run(&ctx, &setup).is_err() {
                return false;
            }
            let _ = js_run(&ctx, &patched); // 顶层即便抛错,oracle 已 stash
            let _ = js_run(
                &ctx,
                "globalThis.__NS=globalThis._yt_player||globalThis.__ytns||null;void 0;",
            );
            js_eval_str(&ctx, "typeof (globalThis.__NS&&globalThis.__NS.__ytUrl)") == "function"
        });
        Some(Solver {
            ctx,
            _rt: rt,
            ready,
        })
    }

    pub fn is_ready(&self) -> bool {
        self.ready
    }

    /// 给 (raw_url, sp, s) 算终极直链(n 解扰 + sig 解密 set)。
    pub fn final_url(&self, raw_url: &str, sp: &str, s: &str) -> String {
        self.ctx.with(|ctx| {
            js_eval_str(
                &ctx,
                &format!(
                    "globalThis.__NS.__ytUrl({},{},{})",
                    json_lit(raw_url),
                    json_lit(sp),
                    json_lit(s)
                ),
            )
        })
    }

    pub fn nsig(&self, url: &str) -> String {
        self.ctx.with(|ctx| {
            js_eval_str(
                &ctx,
                &format!("globalThis.__NS.__ytNsig({})", json_lit(url)),
            )
        })
    }

    /// 给一个 Format 算可下载直链(progressive 直链也走 y2 解 n)。
    pub fn url_for(&self, f: &Format) -> String {
        self.final_url(&f.raw_url, &f.sp, &f.s)
    }
}

fn build_url_oracle(y2: &str) -> String {
    const T: &str = r#"function(u,sp,s){try{var o=__Y2__(u,(sp==null?"":sp),(s==null?"":s));if(o==null)return"";if(typeof o==="string")return o;var t="";try{t=o.toString()}catch(e){}if(typeof t==="string"&&/^https?:\/\//.test(t))return t;var pr=Object.getPrototypeOf(o)||{},ns=Object.getOwnPropertyNames(pr);for(var i=0;i<ns.length;i++){var nm=ns[i];if(nm==="constructor")continue;try{var fn=o[nm];if(typeof fn==="function"&&fn.length===0){var r=fn.call(o);if(typeof r==="string"&&/^https?:\/\//.test(r))return r}}catch(e){}}return"ERRSER:"+t}catch(e){return"ERR:"+e}}"#;
    T.replace("__Y2__", y2)
}

const ENV_STUBS: &str = r#"
var noop=function(){},ret0=function(){return 0};
function elStub(){return {style:{},setAttribute:noop,getAttribute:function(){return null},appendChild:noop,removeChild:noop,insertBefore:noop,getContext:function(){return null},addEventListener:noop,removeEventListener:noop,classList:{add:noop,remove:noop,contains:function(){return false},toggle:noop},dataset:{},children:[],childNodes:[],attributes:[],cloneNode:function(){return elStub()},querySelector:function(){return null},querySelectorAll:function(){return []},getBoundingClientRect:function(){return {top:0,left:0,width:0,height:0,right:0,bottom:0}}}}
__t.console={log:noop,warn:noop,error:noop,info:noop,debug:noop,trace:noop,group:noop,groupEnd:noop,table:noop,assert:noop};
__t.document={cookie:"",referrer:"",title:"",URL:"https://www.youtube.com/",readyState:"complete",visibilityState:"visible",hidden:false,documentElement:elStub(),body:elStub(),head:elStub(),createElement:elStub,createElementNS:elStub,createDocumentFragment:elStub,getElementById:function(){return null},getElementsByTagName:function(){return []},getElementsByClassName:function(){return []},getElementsByName:function(){return []},querySelector:function(){return null},querySelectorAll:function(){return []},addEventListener:noop,removeEventListener:noop,dispatchEvent:function(){return true},createEvent:function(){return {initEvent:noop}}};
__t.navigator={userAgent:"Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Safari/537.36",appVersion:"5.0",platform:"MacIntel",language:"en-US",languages:["en-US"],vendor:"Google Inc.",product:"Gecko",productSub:"20030107",hardwareConcurrency:8,deviceMemory:8,maxTouchPoints:0,onLine:true,cookieEnabled:true,doNotTrack:null,mimeTypes:{length:0},plugins:{length:0},sendBeacon:function(){return true},javaEnabled:function(){return false}};
__t.location={href:"https://www.youtube.com/watch?v=x",protocol:"https:",hostname:"www.youtube.com",host:"www.youtube.com",port:"",origin:"https://www.youtube.com",pathname:"/watch",search:"",hash:"",assign:noop,replace:noop,reload:noop,toString:function(){return this.href}};
__t.document.location=__t.location;
__t.screen={width:1920,height:1080,availWidth:1920,availHeight:1080,colorDepth:24,pixelDepth:24,orientation:{type:"landscape-primary",angle:0,addEventListener:noop}};
__t.history={length:1,state:null,pushState:noop,replaceState:noop,back:noop,forward:noop,go:noop};
function storageStub(){var m={};return {getItem:function(k){return m[k]!=null?m[k]:null},setItem:function(k,v){m[k]=String(v)},removeItem:function(k){delete m[k]},clear:function(){m={}},key:function(i){return Object.keys(m)[i]||null}}}
__t.localStorage=storageStub();__t.sessionStorage=storageStub();
__t.setTimeout=ret0;__t.clearTimeout=noop;__t.setInterval=ret0;__t.clearInterval=noop;__t.requestAnimationFrame=ret0;__t.cancelAnimationFrame=noop;__t.requestIdleCallback=ret0;__t.cancelIdleCallback=noop;__t.queueMicrotask=function(f){try{Promise.resolve().then(f)}catch(e){}};
__t.addEventListener=noop;__t.removeEventListener=noop;__t.dispatchEvent=function(){return true};
__t.performance={now:function(){return Date.now()},timeOrigin:Date.now(),timing:{},getEntriesByType:function(){return[]},mark:noop,measure:noop,clearMarks:noop,clearMeasures:noop};
if(typeof __t.btoa!=="function")__t.btoa=function(s){var b="ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";s=String(s);var o="";for(var i=0;i<s.length;){var c1=s.charCodeAt(i++),c2=s.charCodeAt(i++),c3=s.charCodeAt(i++),e1=c1>>2,e2=((c1&3)<<4)|(c2>>4),e3=((c2&15)<<2)|(c3>>6),e4=c3&63;if(isNaN(c2)){e3=e4=64}else if(isNaN(c3)){e4=64}o+=b.charAt(e1)+b.charAt(e2)+b.charAt(e3)+b.charAt(e4)}return o};
if(typeof __t.atob!=="function")__t.atob=function(s){var b="ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";s=String(s).replace(/[^A-Za-z0-9+\/]/g,"");var o="";for(var i=0;i<s.length;){var x1=b.indexOf(s.charAt(i++)),x2=b.indexOf(s.charAt(i++)),x3=b.indexOf(s.charAt(i++)),x4=b.indexOf(s.charAt(i++)),c1=(x1<<2)|(x2>>4),c2=((x2&15)<<4)|(x3>>2),c3=((x3&3)<<6)|x4;o+=String.fromCharCode(c1);if(x3!==64)o+=String.fromCharCode(c2);if(x4!==64)o+=String.fromCharCode(c3)}return o};
if(typeof __t.TextEncoder!=="function")__t.TextEncoder=function(){this.encode=function(s){s=String(s==null?"":s);var a=[];for(var i=0;i<s.length;i++){var c=s.charCodeAt(i);if(c<128)a.push(c);else if(c<2048)a.push(192|(c>>6),128|(c&63));else a.push(224|(c>>12),128|((c>>6)&63),128|(c&63))}return new Uint8Array(a)}};
if(typeof __t.TextDecoder!=="function")__t.TextDecoder=function(){this.decode=function(b){b=b||[];var s="";for(var i=0;i<(b.length||0);i++)s+=String.fromCharCode(b[i]);return s}};
__t.crypto={getRandomValues:function(a){for(var i=0;i<(a?a.length:0);i++)a[i]=(i*1103515245+12345)&0xff;return a},randomUUID:function(){return "00000000-0000-4000-8000-000000000000"},subtle:{}};
__t.XMLHttpRequest=function(){return {open:noop,send:noop,setRequestHeader:noop,abort:noop,addEventListener:noop,removeEventListener:noop,getAllResponseHeaders:function(){return ""},getResponseHeader:function(){return null},readyState:0,status:0,responseText:"",response:""}};
__t.fetch=function(){return Promise.resolve({ok:true,status:200,headers:{get:function(){return null}},json:function(){return Promise.resolve({})},text:function(){return Promise.resolve("")},arrayBuffer:function(){return Promise.resolve(new ArrayBuffer(0))}})};
__t.Headers=function(){var m={};this.append=function(k,v){m[String(k).toLowerCase()]=v};this.set=this.append;this.get=function(k){return m[String(k).toLowerCase()]!=null?m[String(k).toLowerCase()]:null};this.has=function(k){return m[String(k).toLowerCase()]!=null}};
__t.Request=function(u,o){this.url=u;this.init=o||{}};__t.Response=function(b,o){this.body=b;this.status=(o&&o.status)||200;this.ok=this.status<400};
function Evt(t){this.type=t;this.bubbles=false;this.cancelable=false;this.target=null;this.currentTarget=null;this.preventDefault=noop;this.stopPropagation=noop;this.stopImmediatePropagation=noop}
__t.Event=Evt;__t.CustomEvent=function(t,o){Evt.call(this,t);this.detail=(o&&o.detail)||null};
__t.EventTarget=function(){};__t.EventTarget.prototype.addEventListener=noop;__t.EventTarget.prototype.removeEventListener=noop;__t.EventTarget.prototype.dispatchEvent=function(){return true};
__t.MutationObserver=function(){return {observe:noop,disconnect:noop,takeRecords:function(){return[]}}};
__t.matchMedia=function(){return {matches:false,media:"",addListener:noop,removeListener:noop,addEventListener:noop,removeEventListener:noop}};
__t.getComputedStyle=function(){return {getPropertyValue:function(){return ""}}};
if(typeof __t.Intl==="undefined")__t.Intl=(function(){function mk(inst){var C=function(){return inst};C.supportedLocalesOf=function(l){return [].concat(l||[])};return C}return {DateTimeFormat:mk({format:function(){return ""},formatToParts:function(){return []},resolvedOptions:function(){return {timeZone:"UTC",locale:"en-US"}}}),NumberFormat:mk({format:function(x){return String(x)},formatToParts:function(){return []},resolvedOptions:function(){return {}}}),Collator:mk({compare:function(){return 0},resolvedOptions:function(){return {}}}),PluralRules:mk({select:function(){return "other"},resolvedOptions:function(){return {}}}),RelativeTimeFormat:mk({format:function(){return ""}}),ListFormat:mk({format:function(){return ""}}),Locale:function(o){return {toString:function(){return String(o||"en-US")}}},getCanonicalLocales:function(x){return [].concat(x||[])},Segmenter:mk({segment:function(){return []}})}})();
if(typeof __t.WebAssembly==="undefined")__t.WebAssembly={};
"#;

fn js_run(ctx: &Ctx, code: &str) -> Result<(), String> {
    let mut opts = EvalOptions::default();
    opts.strict = false;
    opts.global = true;
    ctx.eval_with_options::<(), _>(code, opts)
        .catch(ctx)
        .map_err(|e| e.to_string())
}

fn js_eval_str(ctx: &Ctx, expr: &str) -> String {
    let code = format!(
        "(function(){{try{{var v=({expr});return v==null?\"\":String(v)}}catch(e){{return \"ERR:\"+(e&&e.message)}}}})()"
    );
    ctx.eval::<String, _>(code).unwrap_or_default()
}

fn json_lit(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string())
}

// ════════════════════════════════════════════════════════════════════════════
// base.js / html 解析
// ════════════════════════════════════════════════════════════════════════════

pub fn parse_url_builder_name(src: &str) -> Option<String> {
    let ai = src.find(r#"set("alr","yes")"#)?;
    let fi = src[..ai].rfind("=function(")?;
    let between = &src[fi..ai];
    if !between.contains(",!0)") || between.matches("=\"\"").count() < 2 {
        return None;
    }
    let bytes = src.as_bytes();
    let mut start = fi;
    while start > 0 && is_ident(bytes[start - 1]) {
        start -= 1;
    }
    let name = &src[start..fi];
    (!name.is_empty() && !name.as_bytes()[0].is_ascii_digit()).then(|| name.to_string())
}

pub fn parse_nsig_class(src: &str) -> Option<String> {
    let gi = src.find(r#".get("n")"#)?;
    let np = src[..gi].rfind("(new")?;
    let s = src[np..gi]
        .trim_start()
        .strip_prefix('(')?
        .trim_start()
        .strip_prefix("new")?
        .trim_start();
    let lp = s.find('(')?;
    let class_path = s[..lp].trim().to_string();
    let clean = !class_path.is_empty()
        && class_path
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'$'));
    clean.then_some(class_path)
}

fn extract_base_js_url(html: &str) -> Option<String> {
    let i = html.find("/s/player/")?;
    let rest = &html[i..];
    let end = rest.find("base.js")? + "base.js".len();
    let path = &rest[..end];
    path.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-'))
        .then(|| format!("https://www.youtube.com{path}"))
}

/// 抓 `"<field>":"<value>"` 的字符串值(value 不含转义引号场景足够用)。
fn json_str_field(html: &str, field: &str) -> Option<String> {
    let marker = format!("\"{field}\":\"");
    let i = html.find(&marker)? + marker.len();
    let rest = &html[i..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// 从各种 YouTube URL/裸 id 抽取 11 位 videoId。
pub fn extract_video_id(input: &str) -> Option<String> {
    let valid = |s: &str| {
        s.len() == 11
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    };
    if valid(input) {
        return Some(input.to_string());
    }
    for marker in ["v=", "/shorts/", "youtu.be/", "/embed/", "/live/"] {
        if let Some(i) = input.find(marker) {
            let rest = &input[i + marker.len()..];
            let id: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
                .collect();
            if valid(&id) {
                return Some(id);
            }
        }
    }
    None
}

// ════════════════════════════════════════════════════════════════════════════
// 下载(进度条)/ 合并 / pot
// ════════════════════════════════════════════════════════════════════════════

/// 分块(HTTP Range)流式下载到 `path`,带进度条。返回写入字节数。
///
/// **为什么必须分块**:googlevideo 对「不带 `Range` 的整文件 GET」会把速率压到接近实时码率
/// (实测同一条直链:整发 GET 仅 ~130KB/s、HTTP 200;带 `Range` 则 ~9MB/s、HTTP 206)。
/// 这不是签名/n 的问题(同链同 n),纯粹是服务端对「单发整取」的节流。逐块 `Range: bytes=a-b`
/// 请求即可拿全速(yt-dlp 同理,默认 ~10MB/块)。
pub async fn download_with_progress(
    client: &reqwest::Client,
    url: &str,
    path: &str,
    label: &str,
) -> Result<u64, String> {
    // 块大小:默认 10MB,可用 CHUNK_MB 覆盖。
    let chunk: u64 = std::env::var("CHUNK_MB")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(10)
        .max(1)
        * 1024
        * 1024;
    let mut file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut downloaded: u64 = 0;
    let mut total: u64 = 0;
    let started = std::time::Instant::now();
    let mut last = std::time::Instant::now();
    let mut offset: u64 = 0;
    let mut cur_url = url.to_string();
    let mut alr_hops = 0u32;
    loop {
        let end = offset + chunk - 1;
        let resp = client
            .get(&cur_url)
            .header("User-Agent", UA_WEB)
            .header("Range", format!("bytes={offset}-{end}"))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        let status = resp.status();
        // 416 = 请求范围越界,视作正常 EOF(已知总大小为块整数倍时可能发生)。
        if status == reqwest::StatusCode::RANGE_NOT_SATISFIABLE && downloaded > 0 {
            break;
        }
        if !status.is_success() {
            return Err(format!("HTTP {}", status.as_u16()));
        }
        // `alr=yes`(googlevideo 给 TV/adaptive 直链常带):**首个请求会返回 200 + `text/plain`,
        // body 是一条新的重定向直链**(而非媒体)。必须取出该 URL 再请求媒体,否则会把这段
        // ~1KB 的 URL 文本当媒体写进文件 → 视频轨只有 1~2KB、ffmpeg 合并失败(本 bug 的根因)。
        if downloaded == 0 && alr_hops < 5 {
            let ct = resp
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_string();
            if ct.starts_with("text/plain") {
                let body = resp.text().await.map_err(|e| e.to_string())?;
                let body = body.trim();
                if body.starts_with("http") {
                    cur_url = body.to_string();
                    offset = 0;
                    total = 0;
                    alr_hops += 1;
                    continue;
                }
                return Err(format!(
                    "非媒体响应(text/plain): {}",
                    &body[..body.len().min(120)]
                ));
            }
        }
        // 第一块:从 `Content-Range: bytes a-b/TOTAL` 解析文件总大小(失败回退 Content-Length)。
        if total == 0 {
            total = resp
                .headers()
                .get(reqwest::header::CONTENT_RANGE)
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.rsplit('/').next())
                .and_then(|s| s.parse::<u64>().ok())
                .or_else(|| resp.content_length())
                .unwrap_or(0);
        }
        let mut got: u64 = 0;
        let mut stream = resp.bytes_stream();
        while let Some(item) = stream.next().await {
            let bytes = item.map_err(|e| e.to_string())?;
            file.write_all(&bytes).map_err(|e| e.to_string())?;
            downloaded += bytes.len() as u64;
            got += bytes.len() as u64;
            if last.elapsed().as_millis() >= 120 {
                print_progress(label, downloaded, total, started.elapsed().as_secs_f64());
                last = std::time::Instant::now();
            }
        }
        offset += got;
        if got == 0 {
            break; // 防御:空响应,避免死循环
        }
        if status == reqwest::StatusCode::OK {
            break; // 服务器没按 Range 给(整文件一次性发完)
        }
        if total > 0 {
            // 已知总大小:以 `downloaded >= total` 为唯一完成判据。**不要**因「本块不足 chunk」就 break:
            // 网络波动 / 服务器分段会让某块短于请求量,只要没到 total 就得继续请求下一段,否则视频被
            // 截断(实测现象:1080p 视频轨只下 24MB / 2734 帧、ffmpeg 解码 NAL 报错)。
            if downloaded >= total {
                break;
            }
        } else if got < chunk {
            break; // 总大小未知时:末块不足一块 = EOF 兜底
        }
    }
    print_progress(
        label,
        downloaded,
        total.max(downloaded),
        started.elapsed().as_secs_f64(),
    );
    println!();
    Ok(downloaded)
}

fn print_progress(label: &str, done: u64, total: u64, secs: f64) {
    let speed = if secs > 0.0 { done as f64 / secs } else { 0.0 };
    if total > 0 {
        let pct = (done as f64 / total as f64 * 100.0).min(100.0);
        let filled = (pct / 5.0) as usize;
        let bar: String = "█".repeat(filled) + &"░".repeat(20usize.saturating_sub(filled));
        print!(
            "\r  {label} [{bar}] {pct:5.1}%  {}/{}  {}/s   ",
            human_bytes(done),
            human_bytes(total),
            human_bytes(speed as u64)
        );
    } else {
        print!(
            "\r  {label}  {}  {}/s   ",
            human_bytes(done),
            human_bytes(speed as u64)
        );
    }
    let _ = std::io::stdout().flush();
}

/// ffmpeg 合并视频+音频(copy,不转码);按 codec 选容器。返回输出路径。
pub fn ffmpeg_merge(
    video: &str,
    audio: &str,
    out_stem: &str,
    v: &Format,
    a: &Format,
) -> Result<String, String> {
    let container = if v.mime.contains("mp4") && a.mime.contains("mp4") {
        "mp4"
    } else if v.mime.contains("webm") && a.mime.contains("webm") {
        "webm"
    } else {
        "mkv"
    };
    let out = format!("{out_stem}.{container}");
    let status = Command::new("ffmpeg")
        .args([
            "-y", "-i", video, "-i", audio, "-c", "copy", "-map", "0:v:0", "-map", "1:a:0", &out,
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|e| format!("ffmpeg 调用失败: {e}"))?;
    if status.success() {
        Ok(out)
    } else {
        Err(format!("ffmpeg 退出码 {:?}", status.code()))
    }
}

/// 把 pot(po_token)透传进直链(没带才加)。
pub fn with_pot(url: &str, pot: &str) -> String {
    if pot.is_empty() || url.contains("&pot=") || url.contains("?pot=") {
        return url.to_string();
    }
    let sep = if url.contains('?') { '&' } else { '?' };
    format!("{url}{sep}pot={}", url_encode(pot))
}

pub fn ffprobe_brief(path: &str) -> String {
    match Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=format_name,duration,size:stream=codec_type,codec_name,width,height",
            "-of",
            "default=noprint_wrappers=1",
            path,
        ])
        .output()
    {
        Ok(o) => {
            let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if s.is_empty() {
                String::from_utf8_lossy(&o.stderr).trim().to_string()
            } else {
                s
            }
        }
        Err(e) => format!("ffprobe 失败: {e}"),
    }
}

// ════════════════════════════════════════════════════════════════════════════
// 杂项
// ════════════════════════════════════════════════════════════════════════════

pub fn human_bytes(n: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut x = n as f64;
    let mut i = 0;
    while x >= 1024.0 && i < U.len() - 1 {
        x /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{n}{}", U[0])
    } else {
        format!("{x:.1}{}", U[i])
    }
}

pub fn url_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(byte) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(if b[i] == b'+' { b' ' } else { b[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn is_ident(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$'
}

pub fn param_of(url: &str, key: &str) -> Option<String> {
    let q = url.split_once('?')?.1;
    for kv in q.split('&') {
        let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
        if k == key {
            return Some(url_decode(v));
        }
    }
    None
}
