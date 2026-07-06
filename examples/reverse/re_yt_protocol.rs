//! YouTube **纯协议 · 脱浏览器**:reqwest 抓 watch 页 + base.js → 内嵌 **QuickJS(`signer`)** 跑出
//! n/sig → 拼真实 `googlevideo` 直链 → 下载 + ffprobe 验真。**全程不开浏览器**。
//!
//! 这是 `re_yt_play`(浏览器 ① 断点抠 oracle)的「脱浏览器」版。难点在于现代 base.js 把 **sig 与 nsig
//! 统一进一个 opcode VM**(`g7` URL 类的 `.get("n")` 内部跑 `pV(8,6793,this)`,`y2` 里 `Wv(65,2902,…)`
//! 解 sig;`pV`/`O9`/`Wv` 是互递归的 opcode 派发器 + `c` 字符表)。逐 opcode 重写进 Rust 既随 base.js
//! 每几小时轮换而碎、又无收益(yt-dlp 同样是跑 JS、不重写 VM)。故走**「整份 base.js 喂进 JS 引擎」**:
//!
//!   1. **补环境**:base.js 是浏览器脚本(顶层 `var window=this` + 一堆 DOM/定时器初始化)。用
//!      `with(Proxy)` 把所有自由标识符引到一个沙箱:沙箱里放齐 QuickJS 标准内建 + document/navigator/
//!      location/console/atob… 桩;**未知全局经 Proxy `has:()=>true` 读成 `undefined` 而不抛 ReferenceError**
//!      (这是 headless 跑混淆脚本的关键)。把 IIFE 的 `var window=this` 改指沙箱,避免 `window.location` 缺失。
//!   2. **顶部注入惰性 oracle**:在 IIFE 顶部注入 `g.__ytUrl/__ytNsig`(函数体引用闭包局部 `y2`、命名空间
//!      `g.<g7>`,**call 时才解析**)+ `__ytStash(g)` 把命名空间存到沙箱——即便后续 DOM 初始化抛错,核心
//!      函数早已定义、oracle 已挂上,套 try/catch 仍可调用。
//!   3. **就地解析**:`y2` 名 / nsig 解扰类路径全从活 base.js 正则解析(名字随版本轮换,硬编码必挂)。
//!
//! 验真:reqwest `Range` 下载一段 + ffprobe(本地文件 + 直连 URL 权威)。
//!
//! 诚实边界:这不是「把 opcode VM 反汇编成纯 Rust 算法」——VM 执行仍交给 QuickJS(脱的是**浏览器**,
//! 不是 JS 引擎)。这与 yt-dlp 的思路一致,且对混淆轮换免疫;真要逐 opcode 纯 Rust 化是另一个量级的活。
//!
//! 运行:`cargo run --example re_yt_protocol --features signer`(`URL=` 换视频;`MB=` 下载 MiB 数)。

use std::process::Command;

use rquickjs::context::EvalOptions;
use rquickjs::{CatchResultExt, Context, Ctx, Runtime};
use serde_json::Value;

const DEFAULT_URL: &str = "https://www.youtube.com/watch?v=dQw4w9WgXcQ";
const UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Safari/537.36";

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let url = std::env::var("URL").unwrap_or_else(|_| DEFAULT_URL.to_string());
    let range_bytes: u64 = std::env::var("MB")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(4)
        * 1024
        * 1024;
    let client = reqwest::Client::builder().build().expect("reqwest client");

    // 1) 浏览器无关抓取:watch 页 + base.js。
    println!("[1] 抓 watch 页 + base.js(reqwest,无浏览器)…");
    let html = match http_get_text(&client, &url).await {
        Some(h) => h,
        None => {
            eprintln!("[!] 抓 watch 页失败");
            return;
        }
    };
    let Some(base_url) = extract_base_js_url(&html) else {
        eprintln!("[!] 未从 watch 页定位 base.js URL");
        return;
    };
    println!("    base.js = {base_url}");
    let Some(base_js) = http_get_text(&client, &base_url).await else {
        eprintln!("[!] 抓 base.js 失败");
        return;
    };
    let Some(pr_raw) = extract_balanced_json(&html, "ytInitialPlayerResponse") else {
        eprintln!("[!] 未从 watch 页取到 ytInitialPlayerResponse");
        return;
    };
    let pr: Value = serde_json::from_str(&pr_raw).unwrap_or(Value::Null);
    println!(
        "    watch页 {} 字节 | base.js {} 字节 | playerResponse {}",
        html.len(),
        base_js.len(),
        if pr.is_null() { "解析失败" } else { "OK" }
    );

    // 2) 就地解析 y2 名 + nsig 解扰类路径(版本轮换无关)。
    let Some(y2) = parse_url_builder_name(&base_js) else {
        eprintln!("[!] 未定位直链构建器 y2(set(\"alr\",\"yes\") 锚点失配)");
        return;
    };
    let Some(nsig_class) = parse_nsig_class(&base_js) else {
        eprintln!("[!] 未定位 nsig 解扰类");
        return;
    };
    println!("[2] 就地解析:y2={y2}  nsig解扰类={nsig_class}");

    // 3) 选格式(progressive 优先)。
    let Some(f) = pick_format(&pr) else {
        eprintln!("[!] streamingData 无带 url/signatureCipher 的格式");
        return;
    };
    println!(
        "[3] 格式 itag={} mime={} cipher?={}",
        f.itag,
        trunc(&f.mime, 42),
        yesno(!f.s.is_empty())
    );

    // 4) 内嵌 QuickJS 跑整份 base.js,算 n/sig、拼直链(全在一个同步块内,不跨 await)。
    println!("[4] 内嵌 QuickJS:补环境跑整份 base.js + 注入惰性 oracle → 算 n/sig …");
    let computed = run_quickjs(&base_js, &y2, &nsig_class, &f);
    let Some(c) = computed else {
        eprintln!("[!] QuickJS 计算失败");
        return;
    };
    if !c.yterr.is_empty() {
        println!(
            "    (base.js 顶层抛错但已 stash,属正常:{})",
            trunc(&c.yterr, 80)
        );
    }
    println!("    [n] 乱序 {} → 解扰 {}", f.scrambled_n, c.clean_n);
    if c.final_url.is_empty() || c.final_url.starts_with("ERR") {
        eprintln!("[!] 拼链失败:{}", c.final_url);
        return;
    }
    let n_applied = !c.clean_n.is_empty()
        && !c.clean_n.starts_with("ERR")
        && param_of(&c.final_url, "n").as_deref() == Some(c.clean_n.as_str());
    let sig_applied = f.s.is_empty() || param_of(&c.final_url, &f.sp).is_some();
    println!("    [直链] {}", trunc(&c.final_url, 150));
    println!(
        "    [校验] 确定性={} | n已套用={} | sig已set={}",
        yesno(c.deterministic),
        yesno(n_applied),
        yesno(sig_applied)
    );

    // 5) 下载 + ffprobe 验真。
    println!("[5] 下载一段 + ffprobe 验真 …");
    let out = format!("/tmp/yt_proto_{}.{}", f.itag, ext_of(&f.mime));
    let (status, ct, body) = download_range(&client, &c.final_url, range_bytes).await;
    println!(
        "    [下载] HTTP {status}  content-type={ct}  {} bytes",
        body.len()
    );
    let ok_http = (status == 200 || status == 206) && body.len() > 1024 && !ct.contains("text/");
    if ok_http {
        let _ = std::fs::write(&out, &body);
        println!("    [落盘] {out}");
        println!("    [ffprobe·本地]\n{}", indent(&ffprobe_file(&out)));
    }
    println!(
        "    [ffprobe·直连URL(权威)]\n{}",
        indent(&ffprobe_url(&c.final_url))
    );

    println!("\n========== 结论 ==========");
    let reproduced = c.deterministic && (n_applied || c.clean_n == f.scrambled_n) && sig_applied;
    if reproduced && ok_http {
        println!(
            "[✅] 脱浏览器纯算端到端:reqwest 抓 base.js → QuickJS 补环境跑整份 → 自算 n/sig 直链 → HTTP {status} 下载 + ffprobe 验真。"
        );
    } else if reproduced {
        println!(
            "[◑] 纯算复现 OK(确定性+套 n+set sig),但本次下载未达 2xx(多为 pot/IP/区域;itag18 通常无需 pot)。"
        );
    } else {
        println!("[!] 未完全闭环(见上各项)。");
    }
}

// ════════════════════════════════════════════════════════════════════════════
// QuickJS:整份 base.js + with(Proxy) 补环境 + 注入惰性 oracle
// ════════════════════════════════════════════════════════════════════════════

struct Computed {
    clean_n: String,
    final_url: String,
    deterministic: bool,
    yterr: String,
}

fn run_quickjs(base_js: &str, y2: &str, nsig_class: &str, f: &Fmt) -> Option<Computed> {
    // 顶部注入:stash 命名空间 + 惰性 oracle(闭包引用 y2 / g.<g7>,call 时解析)。
    let url_oracle = build_url_oracle(y2);
    let nsig_oracle = format!(
        "function(u){{try{{return new {nsig}(u,!0).get(\"n\")||\"\"}}catch(e){{return \"ERR:\"+e}}}}",
        nsig = nsig_class
    );
    let inject = format!(
        ";try{{__ytStash(g)}}catch(e){{}};try{{g.__ytUrl={url_oracle};g.__ytNsig={nsig_oracle};}}catch(e){{}}"
    );
    // 在 IIFE 顶部 `var window=this;`(sloppy 全局求值下 this=globalThis,已被补环境)后注入。
    let patched = base_js.replacen("var window=this;", &format!("var window=this;{inject}"), 1);
    if patched.len() == base_js.len() {
        eprintln!("[!] 注入锚点 `var window=this;` 未命中");
        return None;
    }
    // 补环境直接装到 globalThis(QuickJS 已自带标准内建,只补 DOM/定时器/编码等桩);避免 with(QuickJS
    // 对 with 有编译器 bug)。base.js 以 sloppy 全局求值:`var window=this`→globalThis,`var _yt_player`→全局。
    let setup = format!(
        "(function(){{var __t=globalThis;{ENV_STUBS}__t.window=__t;__t.self=__t;__t.top=__t;__t.parent=__t;__t.frames=__t;__t.frameElement=null;__t.__ytStash=function(ns){{__t.__ytns=ns;}};}})();void 0;"
    );

    let rt = Runtime::new().ok()?;
    let ctx = Context::full(&rt).ok()?;
    ctx.with(|ctx| {
        if let Err(e) = js_run(&ctx, &setup) {
            eprintln!("[!] 补环境装载异常: {e}");
            return None;
        }
        // 跑整份 base.js;即便 DOM 初始化抛错,顶部注入的 oracle/stash 已就位,故吞错继续。
        let yterr = match js_run(&ctx, &patched) {
            Ok(()) => String::new(),
            Err(e) => e,
        };
        // 命名空间:全局 _yt_player 或 stash 的 __ytns。
        let _ = js_run(
            &ctx,
            "globalThis.__NS=globalThis._yt_player||globalThis.__ytns||null;void 0;",
        );
        let ready = js_eval_str(&ctx, "typeof (globalThis.__NS&&globalThis.__NS.__ytUrl)");
        if ready != "function" {
            eprintln!(
                "[!] __ytUrl 未就位(typeof={ready});base.js err={}",
                trunc(&yterr, 100)
            );
            return None;
        }
        let clean_n = js_eval_str(
            &ctx,
            &format!("globalThis.__NS.__ytNsig({})", json_lit(&f.url)),
        );
        let call = format!(
            "globalThis.__NS.__ytUrl({},{},{})",
            json_lit(&f.url),
            json_lit(&f.sp),
            json_lit(&f.s)
        );
        let final_url = js_eval_str(&ctx, &call);
        let final_url2 = js_eval_str(&ctx, &call);
        Some(Computed {
            clean_n,
            deterministic: final_url == final_url2,
            final_url,
            yterr,
        })
    })
}

/// 终极直链 oracle(同 re_yt_play):y2(url,sp,s) → 运行时探测原型无参方法取 http 串(对 .JK() 名字轮换免疫)。
fn build_url_oracle(y2: &str) -> String {
    const T: &str = r#"function(u,sp,s){try{var o=__Y2__(u,(sp==null?"":sp),(s==null?"":s));if(o==null)return"";if(typeof o==="string")return o;var t="";try{t=o.toString()}catch(e){}if(typeof t==="string"&&/^https?:\/\//.test(t))return t;var pr=Object.getPrototypeOf(o)||{},ns=Object.getOwnPropertyNames(pr);for(var i=0;i<ns.length;i++){var nm=ns[i];if(nm==="constructor")continue;try{var fn=o[nm];if(typeof fn==="function"&&fn.length===0){var r=fn.call(o);if(typeof r==="string"&&/^https?:\/\//.test(r))return r}}catch(e){}}return"ERRSER:"+t}catch(e){return"ERR:"+e}}"#;
    T.replace("__Y2__", y2)
}

/// 补环境桩(QuickJS 不自带的:console/document/navigator/location/atob/TextEncoder/定时器/XHR…)。
/// 以 `__t`(= globalThis)为目标;由 `run_quickjs` 的 setup 包成 IIFE 注入。
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

// ════════════════════════════════════════════════════════════════════════════
// QuickJS 小工具
// ════════════════════════════════════════════════════════════════════════════

fn js_run(ctx: &Ctx, code: &str) -> Result<(), String> {
    // strict=false:base.js 顶层用 `with(Proxy)` 补环境,strict 模式禁 `with`,必须关。
    let mut opts = EvalOptions::default();
    opts.strict = false;
    opts.global = true;
    ctx.eval_with_options::<(), _>(code, opts)
        .catch(ctx)
        .map_err(|e| e.to_string())
}

/// 求值表达式取字符串(出错/非串返回 "ERR:.." 或空)。
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
// base.js 就地解析(y2 名 / nsig 解扰类)
// ════════════════════════════════════════════════════════════════════════════

/// 从 base.js 抠出直链构建器名:`NAME=function(a,b="",c=""){a=new g.X(a,!0);a.set("alr","yes")…`。
fn parse_url_builder_name(src: &str) -> Option<String> {
    let ai = src.find(r#"set("alr","yes")"#)?;
    // 在该锚点前找最近的 `=function(`,要求其后到锚点之间含 `,!0)` 与至少两个 `=""` 默认参。
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

/// 从 `(new g.X(m,!0)).get("n")` 抠出解扰类路径 `g.X`。
fn parse_nsig_class(src: &str) -> Option<String> {
    let gi = src.find(r#".get("n")"#)?;
    // 这里第一处 `.get("n")` 通常就是 cs6;回溯最近的 `(new`。
    let np = src[..gi].rfind("(new")?;
    let seg = &src[np..gi];
    let s = seg
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

fn is_ident(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$'
}

// ════════════════════════════════════════════════════════════════════════════
// 抓取 / 解析 / 下载 / ffprobe
// ════════════════════════════════════════════════════════════════════════════

async fn http_get_text(client: &reqwest::Client, url: &str) -> Option<String> {
    client
        .get(url)
        .header("User-Agent", UA)
        .header("Accept-Language", "en-US,en;q=0.9")
        .send()
        .await
        .ok()?
        .text()
        .await
        .ok()
}

/// 从 watch 页定位 player base.js 的绝对 URL。
fn extract_base_js_url(html: &str) -> Option<String> {
    let i = html.find("/s/player/")?;
    let rest = &html[i..];
    let end = rest.find("base.js")? + "base.js".len();
    let path = &rest[..end];
    if path
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-'))
    {
        Some(format!("https://www.youtube.com{path}"))
    } else {
        None
    }
}

/// 从 html 取 `<marker> = { … }` 的平衡 JSON(字符串/转义感知)。
fn extract_balanced_json(html: &str, marker: &str) -> Option<String> {
    let mi = html.find(marker)?;
    let after = &html[mi + marker.len()..];
    let eq = after.find('=')?;
    let brace_rel = after[eq..].find('{')?;
    let start = mi + marker.len() + eq + brace_rel;
    let bytes = html.as_bytes();
    let (mut depth, mut i) = (0i32, start);
    let (mut in_str, mut esc, mut quote) = (false, false, 0u8);
    while i < bytes.len() {
        let c = bytes[i];
        if in_str {
            if esc {
                esc = false;
            } else if c == b'\\' {
                esc = true;
            } else if c == quote {
                in_str = false;
            }
        } else {
            match c {
                b'"' | b'\'' => {
                    in_str = true;
                    quote = c;
                }
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(html[start..=i].to_string());
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }
    None
}

struct Fmt {
    itag: i64,
    mime: String,
    url: String,
    sp: String,
    s: String,
    scrambled_n: String,
}

fn pick_format(pr: &Value) -> Option<Fmt> {
    let sd = pr.get("streamingData")?;
    let take = |key: &str| -> Option<Fmt> {
        for f in sd.get(key)?.as_array()? {
            if f.get("url").is_some() || f.get("signatureCipher").is_some() {
                return Some(to_fmt(f));
            }
        }
        None
    };
    take("formats").or_else(|| take("adaptiveFormats"))
}

fn to_fmt(f: &Value) -> Fmt {
    let itag = f.get("itag").and_then(Value::as_i64).unwrap_or(0);
    let mime = f
        .get("mimeType")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let (url, sp, s) = if let Some(u) = f.get("url").and_then(Value::as_str) {
        (u.to_string(), String::new(), String::new())
    } else {
        let cipher = f
            .get("signatureCipher")
            .and_then(Value::as_str)
            .unwrap_or_default();
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
    };
    let scrambled_n = param_of(&url, "n").unwrap_or_default();
    Fmt {
        itag,
        mime,
        url,
        sp,
        s,
        scrambled_n,
    }
}

async fn download_range(client: &reqwest::Client, url: &str, max: u64) -> (u16, String, Vec<u8>) {
    let resp = client
        .get(url)
        .header("User-Agent", UA)
        .header("Referer", "https://www.youtube.com/")
        .header("Origin", "https://www.youtube.com")
        .header("Range", format!("bytes=0-{}", max.saturating_sub(1)))
        .send()
        .await;
    match resp {
        Ok(r) => {
            let status = r.status().as_u16();
            let ct = r
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_string();
            let body = r.bytes().await.map(|b| b.to_vec()).unwrap_or_default();
            (status, ct, body)
        }
        Err(e) => {
            eprintln!("[!] 下载错误: {e}");
            (0, String::new(), Vec::new())
        }
    }
}

fn ffprobe_file(path: &str) -> String {
    run_ffprobe(&[
        "-v",
        "error",
        "-show_entries",
        "format=format_name,duration,size:stream=codec_type,codec_name,width,height,sample_rate",
        "-of",
        "default=noprint_wrappers=1",
        path,
    ])
}

fn ffprobe_url(url: &str) -> String {
    run_ffprobe(&[
        "-v",
        "error",
        "-user_agent",
        UA,
        "-headers",
        "Referer: https://www.youtube.com/\r\nOrigin: https://www.youtube.com\r\n",
        "-show_entries",
        "format=format_name,duration:stream=codec_type,codec_name,width,height,sample_rate",
        "-of",
        "default=noprint_wrappers=1",
        url,
    ])
}

fn run_ffprobe(args: &[&str]) -> String {
    match Command::new("ffprobe").args(args).output() {
        Ok(o) => {
            let out = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if out.is_empty() {
                format!(
                    "(无可识别媒体流;stderr: {})",
                    String::from_utf8_lossy(&o.stderr).trim()
                )
            } else {
                out
            }
        }
        Err(e) => format!("ffprobe 调用失败: {e}"),
    }
}

// ════════════════════════════════════════════════════════════════════════════
// 杂项
// ════════════════════════════════════════════════════════════════════════════

fn param_of(url: &str, key: &str) -> Option<String> {
    let q = url.split_once('?')?.1;
    for kv in q.split('&') {
        let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
        if k == key {
            return Some(url_decode(v));
        }
    }
    None
}

fn url_decode(s: &str) -> String {
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

fn ext_of(mime: &str) -> &'static str {
    if mime.contains("audio/mp4") {
        "m4a"
    } else if mime.contains("webm") {
        "webm"
    } else if mime.contains("mp4") {
        "mp4"
    } else {
        "bin"
    }
}

fn yesno(b: bool) -> &'static str {
    if b { "是" } else { "否" }
}

fn trunc(s: &str, n: usize) -> String {
    let t: String = s.chars().take(n).collect();
    if s.chars().count() > n {
        format!("{t}…")
    } else {
        t
    }
}

fn indent(s: &str) -> String {
    s.lines()
        .map(|l| format!("   {l}"))
        .collect::<Vec<_>>()
        .join("\n")
}
