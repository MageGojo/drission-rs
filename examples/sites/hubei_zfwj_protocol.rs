//! 湖北省人民政府「政府文件」纯协议采集示例:
//! 全程不启动浏览器。遇到站点时效 JS 挑战时,先用 `SessionPage` 抓 412 挑战页和外链 JS,
//! 再用本地 JS 引擎补最小 DOM 环境,收集 `document.cookie` 写入并回灌 `SessionPage`。
//! 默认使用 Node/V8(不启动浏览器,但需本机有 `node` 命令),因为该站会校验 V8 行为;也可设置
//! `HUBEI_JS_ENGINE=quickjs` 改用内嵌 QuickJS 做诊断。随后列表、翻页、详情页继续走 HTTP 协议请求,
//! 抽取详情正文和正文内链接后导出 CSV。
//!
//! 运行:
//!   cargo run --example hubei_zfwj_protocol --no-default-features --features camoufox,signer
//!   cargo run --example hubei_zfwj_protocol --no-default-features --features camoufox,signer -- https://www.hubei.gov.cn/zfwj/list1.shtml 5 target/hubei_zfwj_protocol/zfwj.csv
//!
//! 参数:
//!   1. 起始列表页,默认 https://www.hubei.gov.cn/zfwj/list1.shtml
//!   2. 最多抓多少页,默认 all/全部(分页 URL 从页面里的“下一页”动态发现)
//!   3. CSV 输出路径,默认 target/hubei_zfwj_protocol/zfwj.csv

use std::collections::HashSet;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use drission::browser::CookieParam as SessionCookieParam;
use drission::prelude::*;
use rquickjs::context::EvalOptions;
use rquickjs::{CatchResultExt, Context, Ctx, Runtime};
use scraper::{ElementRef, Html, Selector};
use serde::Serialize;
use tokio::time::sleep;

const DEFAULT_START: &str = "https://www.hubei.gov.cn/zfwj/list1.shtml";
const DEFAULT_OUT: &str = "target/hubei_zfwj_protocol/zfwj.csv";
const CHROME_UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
                         (KHTML, like Gecko) Chrome/149.0.0.0 Safari/537.36";

#[derive(Debug, Clone)]
struct GovFile {
    list_page: usize,
    list_title: String,
    list_date: String,
    url: String,
    detail_title: String,
    publish_date: String,
    source: String,
    document_no: String,
    content: String,
    content_links: String,
}

#[derive(Debug, Clone)]
struct ListPage {
    records: Vec<ListRecord>,
    next_url: Option<String>,
}

#[derive(Debug, Clone)]
struct ListRecord {
    title: String,
    date: String,
    url: String,
}

#[derive(Debug, Clone, Default)]
struct DetailData {
    title: String,
    publish_date: String,
    source: String,
    document_no: String,
    content: String,
    content_links: String,
}

#[derive(Debug, Serialize)]
struct ContentLink {
    text: String,
    url: String,
}

#[tokio::main]
async fn main() -> drission::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("warn,drission::protocol::connection=off")
        .init();

    let mut args = std::env::args().skip(1);
    let start_url = args.next().unwrap_or_else(|| DEFAULT_START.to_string());
    let max_pages = args.next().and_then(|s| parse_max_pages(&s));
    let out = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_OUT));

    println!(
        "==== 湖北省政府文件纯协议采集 ====\n起始页 = {start_url}\n最多页 = {}\n输出   = {}\n策略   = SessionPage HTTP + 本地 JS 引擎解挑战,全程不启动浏览器\n",
        max_pages
            .map(|n| n.to_string())
            .unwrap_or_else(|| "全部".to_string()),
        out.display()
    );

    let mut session = new_protocol_session()?;

    let mut all = Vec::new();
    let mut seen = HashSet::new();
    let mut current_url = Some(start_url.clone());
    let mut page_no = 1usize;

    while let Some(url) = current_url.clone() {
        if max_pages.is_some_and(|limit| page_no > limit) {
            break;
        }
        let page_progress = max_pages
            .map(|limit| format!("{page_no}/{limit}"))
            .unwrap_or_else(|| format!("{page_no}/全部"));
        println!("[{page_progress}] 协议 GET 列表 {url}");

        let html = fetch_html(&mut session, &url).await?;
        if std::env::var("HUBEI_SAVE_HTML").ok().as_deref() == Some("1") {
            write_debug_html("list", page_no, &html).await?;
        }
        let list_page = parse_list_page(&html, &url)?;
        if list_page.records.is_empty() {
            write_debug_html("list", page_no, &html).await?;
            println!("  未抽到列表记录,停止。");
            break;
        }

        let before = all.len();
        let total = list_page.records.len();
        for (idx, r) in list_page.records.into_iter().enumerate() {
            let key = format!("{}|{}", r.title, r.url);
            if !seen.insert(key) {
                continue;
            }

            println!(
                "  协议详情 {}/{} {}",
                idx + 1,
                total,
                truncate_for_log(&r.title, 42)
            );
            let detail_html = fetch_html(&mut session, &r.url).await?;
            let detail = parse_detail(&detail_html, &r.url)?;
            all.push(GovFile {
                list_page: page_no,
                list_title: r.title,
                list_date: r.date,
                url: r.url,
                detail_title: detail.title,
                publish_date: detail.publish_date,
                source: detail.source,
                document_no: detail.document_no,
                content: detail.content,
                content_links: detail.content_links,
            });
            sleep(Duration::from_millis(90)).await;
        }
        println!("  本页新增 {} 条,累计 {} 条", all.len() - before, all.len());

        current_url = list_page.next_url;
        page_no += 1;
        sleep(Duration::from_millis(260)).await;
    }

    let rows = to_csv_rows(&all);
    drission::scrape::write_csv(&out, &rows).await?;
    println!("\nCSV 已写出:{} ({} 条记录)", out.display(), all.len());
    Ok(())
}

fn new_protocol_session() -> drission::Result<SessionPage> {
    let opts = SessionOptions::new()
        .user_agent(CHROME_UA)
        .timeout(Duration::from_secs(25))
        .header(
            "Accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,\
             image/apng,*/*;q=0.8,application/signed-exchange;v=b3;q=0.7",
        )
        .header("Accept-Language", "zh-CN,zh;q=0.9")
        .header("Cache-Control", "no-cache")
        .header("Pragma", "no-cache")
        .header("Upgrade-Insecure-Requests", "1");
    SessionPage::new(opts)
}

async fn fetch_html(session: &mut SessionPage, url: &str) -> drission::Result<String> {
    for attempt in 0..3 {
        let ok = session.get(url).await?;
        let status = session.status();
        let html = session.html().to_string();
        if ok && !looks_like_js_challenge(&html) {
            return Ok(html);
        }

        if looks_like_js_challenge(&html) && attempt < 2 {
            println!(
                "  [challenge] 协议请求遇到 JS 挑战(status={status}),本地 JS 引擎计算 cookie 后重试"
            );
            let cookie_names = solve_challenge_into_session(session, url, &html).await?;
            println!("  [challenge] 已写入 cookie: {}", cookie_names.join(", "));
            sleep(Duration::from_millis(260)).await;
            continue;
        }

        write_debug_html("challenge", 0, &html).await?;
        return Err(drission::Error::msg(format!(
            "协议请求失败或仍是挑战页: status={status} url={url}"
        )));
    }
    unreachable!()
}

async fn solve_challenge_into_session(
    session: &mut SessionPage,
    url: &str,
    html: &str,
) -> drission::Result<Vec<String>> {
    let challenge = parse_challenge(html, url)?;
    let ok = session.get(&challenge.script_url).await?;
    let status = session.status();
    let challenge_js = session.text().to_string();
    if !ok || challenge_js.trim().is_empty() {
        return Err(drission::Error::msg(format!(
            "挑战 JS 下载失败: status={status} url={}",
            challenge.script_url
        )));
    }

    let cookie_writes = run_challenge_js(&challenge, &challenge_js)?;
    if std::env::var("HUBEI_DEBUG_COOKIE").ok().as_deref() == Some("1") {
        for c in &cookie_writes {
            println!("  [challenge-cookie] {c}");
        }
    }
    let cookies = cookie_writes
        .iter()
        .filter_map(|line| cookie_param_from_write(line, url))
        .collect::<Vec<_>>();
    if cookies.is_empty() {
        return Err(drission::Error::msg(
            "QuickJS 已执行,但没有收集到有效 cookie",
        ));
    }

    let names = cookies.iter().map(|c| c.name.clone()).collect::<Vec<_>>();
    session.set_cookies(cookies);
    Ok(names)
}

#[derive(Debug, Clone)]
struct ChallengeData {
    page_url: String,
    script_url: String,
    meta_content: String,
    inline_script: String,
    nsd: u64,
    cd: String,
}

fn parse_challenge(html: &str, page_url: &str) -> drission::Result<ChallengeData> {
    let nsd = extract_js_number(html, "$_ss.nsd=")
        .ok_or_else(|| drission::Error::msg("挑战页缺少 $_ss.nsd"))?;
    let cd = extract_js_string(html, "$_ss.cd=")
        .ok_or_else(|| drission::Error::msg("挑战页缺少 $_ss.cd"))?;
    let script_url = extract_challenge_script_url(html, page_url)
        .ok_or_else(|| drission::Error::msg("挑战页缺少外链 JS"))?;
    let meta_content = extract_challenge_meta_content(html).unwrap_or_default();
    let inline_script = extract_challenge_inline_script(html).unwrap_or_default();

    Ok(ChallengeData {
        page_url: page_url.to_string(),
        script_url,
        meta_content,
        inline_script,
        nsd,
        cd,
    })
}

fn extract_challenge_meta_content(html: &str) -> Option<String> {
    let doc = Html::parse_document(html);
    let sel = Selector::parse("meta[content]").ok()?;
    doc.select(&sel)
        .find(|m| m.attr("r") == Some("m"))
        .or_else(|| doc.select(&sel).find(|m| m.attr("content").is_some()))
        .and_then(|m| m.attr("content"))
        .map(str::to_string)
}

fn extract_challenge_inline_script(html: &str) -> Option<String> {
    let doc = Html::parse_document(html);
    let sel = Selector::parse("script:not([src])").ok()?;
    doc.select(&sel)
        .map(|s| s.inner_html())
        .find(|s| s.contains("$_ss.nsd") && s.contains("$_ss.cd"))
}

fn extract_challenge_script_url(html: &str, base_url: &str) -> Option<String> {
    let doc = Html::parse_document(html);
    let sel = Selector::parse("script[src]").ok()?;
    doc.select(&sel)
        .filter_map(|s| s.attr("src"))
        .filter(|src| src.contains(".js"))
        .find_map(|src| absolute_url(base_url, src))
}

fn run_challenge_js(
    challenge: &ChallengeData,
    challenge_js: &str,
) -> drission::Result<Vec<String>> {
    let setup = build_challenge_env(challenge);
    if std::env::var("HUBEI_DEBUG_SETUP").ok().as_deref() == Some("1") {
        let _ = std::fs::create_dir_all("target/hubei_zfwj_protocol");
        let _ = std::fs::write("target/hubei_zfwj_protocol/debug_setup.js", &setup);
        let _ = std::fs::write(
            "target/hubei_zfwj_protocol/debug_challenge.js",
            challenge_js,
        );
    }

    let engine = std::env::var("HUBEI_JS_ENGINE").unwrap_or_else(|_| "node".to_string());
    if !engine.eq_ignore_ascii_case("quickjs") {
        match run_challenge_js_node(&setup, challenge_js) {
            Ok(cookies) => return Ok(cookies),
            Err(e) if engine.eq_ignore_ascii_case("node") => return Err(e),
            Err(e) => println!("  [challenge] Node/V8 执行失败,回退 QuickJS: {e}"),
        }
    }

    run_challenge_js_quickjs(&setup, challenge_js)
}

fn run_challenge_js_quickjs(setup: &str, challenge_js: &str) -> drission::Result<Vec<String>> {
    let rt = Runtime::new().map_err(|e| drission::Error::msg(e.to_string()))?;
    let ctx = Context::full(&rt).map_err(|e| drission::Error::msg(e.to_string()))?;
    ctx.with(|ctx| {
        js_run(&ctx, &setup)?;
        let js_err = js_run(&ctx, challenge_js).err();
        let _ = js_run(
            &ctx,
            "try{ if(globalThis.$_ss && typeof globalThis.$_ss.lcd === 'function') globalThis.$_ss.lcd(); }catch(e){}",
        );
        if std::env::var("HUBEI_DEBUG_EVAL").ok().as_deref() == Some("1") {
            let eval_dump = js_eval_string(&ctx, "JSON.stringify(globalThis.__evalCodes||[])")?;
            let _ = std::fs::create_dir_all("target/hubei_zfwj_protocol");
            let _ = std::fs::write("target/hubei_zfwj_protocol/debug_eval_codes.json", eval_dump);
        }
        if std::env::var("HUBEI_DEBUG_DOM").ok().as_deref() == Some("1") {
            let dom_dump = js_eval_string(&ctx, "JSON.stringify(globalThis.__domLog||[])")?;
            let _ = std::fs::create_dir_all("target/hubei_zfwj_protocol");
            let _ = std::fs::write("target/hubei_zfwj_protocol/debug_dom_log.json", dom_dump);
        }
        let raw = js_eval_string(&ctx, "JSON.stringify(globalThis.__cookieWrites||[])")?;
        let cookies: Vec<String> = serde_json::from_str(&raw)
            .map_err(|e| drission::Error::msg(format!("cookie JSON 解析失败: {e}; raw={raw}")))?;
        if cookies.is_empty() {
            return Err(drission::Error::msg(format!(
                "挑战 JS 未写 cookie{}",
                js_err
                    .map(|e| format!("; 顶层异常: {e}"))
                    .unwrap_or_default()
            )));
        }
        Ok(cookies)
    })
}

const NODE_CHALLENGE_RUNNER: &str = r#"
const fs = require('fs');
const write = (s) => process.stdout.write(String(s));
const setupPath = process.argv[1];
const challengePath = process.argv[2];
let err = "";
try {
  const setup = fs.readFileSync(setupPath, "utf8");
  const challenge = fs.readFileSync(challengePath, "utf8");
  (0, eval)(setup);
  try { (0, eval)(challenge); } catch (e) { err = String(e && e.stack || e); }
  try {
    if (globalThis.$_ss && typeof globalThis.$_ss.lcd === "function") globalThis.$_ss.lcd();
  } catch (e) {
    err += "\nlcd " + String(e && e.stack || e);
  }
  const cookies = globalThis.__cookieWrites || [];
  write(JSON.stringify({ ok: cookies.length > 0, err, cookies }));
} catch (e) {
  write(JSON.stringify({ ok: false, err: String(e && e.stack || e), cookies: [] }));
}
"#;

fn run_challenge_js_node(setup: &str, challenge_js: &str) -> drission::Result<Vec<String>> {
    let dir = PathBuf::from("target/hubei_zfwj_protocol/node_runner");
    std::fs::create_dir_all(&dir)?;
    let nonce = format!(
        "{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    let setup_path = dir.join(format!("setup_{nonce}.js"));
    let challenge_path = dir.join(format!("challenge_{nonce}.js"));
    std::fs::write(&setup_path, setup)?;
    std::fs::write(&challenge_path, challenge_js)?;

    let output = Command::new("node")
        .arg("-e")
        .arg(NODE_CHALLENGE_RUNNER)
        .arg(&setup_path)
        .arg(&challenge_path)
        .output()
        .map_err(|e| {
            drission::Error::msg(format!(
                "Node/V8 不可用: {e}; 可设置 HUBEI_JS_ENGINE=quickjs 使用内嵌 QuickJS,但该站当前会校验 V8 行为"
            ))
        })?;
    let _ = std::fs::remove_file(&setup_path);
    let _ = std::fs::remove_file(&challenge_path);

    let stdout = String::from_utf8_lossy(&output.stdout);
    if !output.status.success() {
        return Err(drission::Error::msg(format!(
            "Node/V8 执行失败(status={}): {}{}",
            output.status,
            stdout,
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    let value: serde_json::Value = serde_json::from_str(stdout.trim()).map_err(|e| {
        drission::Error::msg(format!("Node/V8 输出不是 JSON: {e}; stdout={stdout}"))
    })?;
    let cookies = value
        .get("cookies")
        .and_then(|v| serde_json::from_value::<Vec<String>>(v.clone()).ok())
        .unwrap_or_default();
    if cookies.is_empty() {
        return Err(drission::Error::msg(format!(
            "Node/V8 未写 cookie: {}",
            value
                .get("err")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
        )));
    }
    Ok(cookies)
}

const HUBEI_CHALLENGE_ENV: &str = r#"
(function(){
  var g = globalThis;
  g.__domLog = [];
  function logAccess(x){ try { g.__domLog.push(String(x)); } catch(e) {} }
  var __nativeToString = new WeakMap();
  var __realFunctionToString = Function.prototype.toString;
  function markNative(fn, name){
    if (typeof fn === "function") {
      __nativeToString.set(fn, "function " + (name || fn.name || "") + "() { [native code] }");
    }
    return fn;
  }
  Function.prototype.toString = function(){
    if (__nativeToString.has(this)) return __nativeToString.get(this);
    var s = __realFunctionToString.call(this);
    if (s.indexOf("[native code]") >= 0) {
      var n = this && this.name ? " " + this.name : "";
      return "function" + n + "() { [native code] }";
    }
    return s;
  };
  markNative(Function.prototype.toString, "toString");
  function noop(){}
  function ret1(){ return 1; }
  function storageStub(){ var m={}; return {getItem:function(k){return Object.prototype.hasOwnProperty.call(m,k)?m[k]:null},setItem:function(k,v){m[String(k)]=String(v)},removeItem:function(k){delete m[String(k)]},clear:function(){m={}},key:function(i){return Object.keys(m)[i]||null},get length(){return Object.keys(m).length}}; }
  function anyStub(){
    var fn = function(){ return anyStub(); };
    return new Proxy(fn, {
      get:function(t,p){
        if (p === Symbol.toPrimitive) return function(){ return ""; };
        if (p === "toString") return function(){ return ""; };
        if (p === "valueOf") return function(){ return 0; };
        if (p === "length") return 0;
        if (!(p in t)) t[p] = anyStub();
        return t[p];
      },
      set:function(t,p,v){ t[p]=v; return true; },
      has:function(t,p){ return p in t; },
      apply:function(){ return anyStub(); },
      construct:function(){ return anyStub(); }
    });
  }
  function cssStyleStub(){
    var s = {
      cssText: "",
      length: 0,
      WebkitAppearance: "",
      webkitAppearance: "",
      WebkitTransform: "",
      webkitTransform: "",
      WebkitTransition: "",
      webkitTransition: "",
      WebkitUserSelect: "",
      webkitUserSelect: "",
      transform: "",
      transition: "",
      appearance: "",
      getPropertyValue: function(){ return ""; },
      setProperty: noop,
      removeProperty: function(){ return ""; },
      item: function(){ return ""; }
    };
    try { Object.defineProperty(s, Symbol.toStringTag, {value:"CSSStyleDeclaration"}); } catch(e) {}
    return new Proxy(s, {
      get:function(t,p){ if (typeof p !== "symbol") logAccess("style."+String(p)); return t[p]; },
      set:function(t,p,v){ t[p]=v; return true; },
      has:function(t,p){ logAccess("style.has:"+String(p)); return p in t; }
    });
  }
  function makeEl(tag){
    tag = String(tag || "div").toUpperCase();
    var e = {};
    e.tagName = tag;
    e.nodeName = tag;
    e.nodeType = 1;
    e.style = cssStyleStub();
    e.dataset = {};
    e.children = [];
    e.childNodes = [];
    e.attributes = [];
    e.parentNode = {appendChild:function(x){return x},removeChild:function(x){return x},insertBefore:function(x){return x}};
    e.parentElement = e.parentNode;
    e.ownerDocument = null;
    e.innerHTML = "";
    e.textContent = "";
    e.className = "";
    e.id = "";
    e.name = "";
    e.src = "";
    e.href = "";
    e.type = "";
    e.value = "";
    e.checked = false;
    e.disabled = false;
    e.setAttribute = function(k,v){ logAccess("el("+tag+").setAttribute:"+k+"="+v); this[String(k)] = String(v); };
    e.getAttribute = function(k){ logAccess("el("+tag+").getAttribute:"+k); var v = this[String(k)]; return v == null ? null : String(v); };
    e.hasAttribute = function(k){ logAccess("el("+tag+").hasAttribute:"+k); return this[String(k)] != null; };
    e.removeAttribute = function(k){ delete this[String(k)]; };
    e.appendChild = function(x){ logAccess("el("+tag+").appendChild:"+(x&&x.tagName)); if (x) { x.parentNode = this; x.parentElement = this; } this.childNodes.push(x); return x; };
    e.removeChild = function(x){ return x; };
    e.insertBefore = function(x){ return this.appendChild(x); };
    e.cloneNode = function(){ return makeEl(tag); };
    e.getElementsByTagName = function(){ return []; };
    e.getElementsByClassName = function(){ return []; };
    e.querySelector = function(){ return null; };
    e.querySelectorAll = function(){ return []; };
    e.addEventListener = noop;
    e.removeEventListener = noop;
    e.dispatchEvent = function(){ return true; };
    e.click = noop;
    e.focus = noop;
    e.blur = noop;
    e.submit = noop;
    e.getBoundingClientRect = function(){ return {x:0,y:0,top:0,left:0,right:0,bottom:0,width:0,height:0}; };
    e.getClientRects = function(){ return []; };
    e.getContext = function(){ return null; };
    e.classList = {add:noop,remove:noop,toggle:function(){return false},contains:function(){return false}};
    try { Object.defineProperty(e, Symbol.toStringTag, {value:"HTML" + tag.charAt(0) + tag.slice(1).toLowerCase() + "Element"}); } catch(e) {}
    return new Proxy(e, {
      get:function(t,p){ if (typeof p !== "symbol") logAccess("el("+tag+")."+String(p)); return t[p]; },
      set:function(t,p,v){ t[p]=v; return true; },
      has:function(t,p){ return p in t; }
    });
  }

  var href = __HREF__;
  var protocol = __PROTOCOL__;
  var host = __HOST__;
  var hostname = __HOSTNAME__;
  var origin = __ORIGIN__;
  var pathname = __PATHNAME__;
  var search = __SEARCH__;
  var metaContent = __META_CONTENT__;
  var scriptSrc = __SCRIPT_SRC__;
  var inlineScriptText = __INLINE_SCRIPT__;
  var webdriverMode = __WEBDRIVER_MODE__;
  var webkitStorageMode = __WEBKIT_STORAGE_MODE__;
  var mimeMode = __MIME_MODE__;
  var captureEval = __CAPTURE_EVAL__;
  var locationObj = {
    href: href, protocol: protocol, host: host, hostname: hostname, origin: origin,
    pathname: pathname, search: search, hash: "", port: "",
    assign: noop, replace: noop, reload: noop,
    toString: function(){ return this.href; }
  };
  try { Object.defineProperty(locationObj, Symbol.toStringTag, {value:"Location"}); } catch(e) {}
  var head = makeEl("head"), body = makeEl("body"), docEl = makeEl("html");
  var meta = makeEl("meta");
  meta.content = metaContent;
  meta.r = "m";
  meta.setAttribute("content", metaContent);
  meta.setAttribute("r", "m");
  var inlineScript = makeEl("script");
  inlineScript.type = "text/javascript";
  inlineScript.r = "m";
  inlineScript.text = inlineScriptText;
  inlineScript.textContent = inlineScriptText;
  inlineScript.innerHTML = inlineScriptText;
  inlineScript.setAttribute("type", "text/javascript");
  inlineScript.setAttribute("r", "m");
  var script = makeEl("script");
  script.src = scriptSrc;
  script.charset = "utf-8";
  script.type = "text/javascript";
  script.r = "m";
  script.setAttribute("src", scriptSrc);
  script.setAttribute("charset", "utf-8");
  script.setAttribute("type", "text/javascript");
  script.setAttribute("r", "m");
  script.parentNode = head;
  inlineScript.parentNode = head;
  meta.parentNode = head;
  head.children = [meta, inlineScript, script];
  head.childNodes = [meta, inlineScript, script];
  var doc = {
    nodeType: 9,
    URL: href,
    documentURI: href,
    compatMode: "CSS1Compat",
    referrer: "",
    title: "",
    readyState: "complete",
    visibilityState: "visible",
    hidden: false,
    location: locationObj,
    defaultView: g,
    parentWindow: g,
    documentElement: docEl,
    head: head,
    body: body,
    currentScript: script,
    forms: [],
    scripts: [inlineScript, script],
    images: [],
    links: [],
    createElement: makeEl,
    createElementNS: function(ns, tag){ return makeEl(tag); },
    createDocumentFragment: function(){ var f = makeEl("fragment"); f.nodeType = 11; return f; },
    createTextNode: function(t){ return {nodeType:3,nodeValue:String(t),textContent:String(t)}; },
    createComment: function(t){ return {nodeType:8,nodeValue:String(t),textContent:String(t)}; },
    createEvent: function(){ return {initEvent:noop,initMouseEvent:noop}; },
    getElementById: function(){ return null; },
    getElementsByName: function(){ return []; },
    getElementsByClassName: function(){ return []; },
    getElementsByTagName: function(name){
      logAccess("document.getElementsByTagName:"+name);
      name = String(name || "").toLowerCase();
      if (name === "head") return [head];
      if (name === "body") return [body];
      if (name === "html") return [docEl];
      if (name === "meta") return [meta];
      if (name === "script") return [inlineScript, script];
      return [];
    },
    querySelector: function(sel){
      logAccess("document.querySelector:"+sel);
      sel = String(sel || "").toLowerCase();
      if (sel.indexOf("meta") >= 0) return meta;
      if (sel.indexOf("script") >= 0) return inlineScript;
      return null;
    },
    querySelectorAll: function(sel){
      logAccess("document.querySelectorAll:"+sel);
      sel = String(sel || "").toLowerCase();
      if (sel.indexOf("meta") >= 0) return [meta];
      if (sel.indexOf("script") >= 0) return [inlineScript, script];
      return [];
    },
    addEventListener: noop,
    removeEventListener: noop,
    dispatchEvent: function(){ return true; },
    write: noop,
    writeln: noop,
    open: noop,
    close: noop
  };
  try { Object.defineProperty(doc, Symbol.toStringTag, {value:"HTMLDocument"}); } catch(e) {}
  head.ownerDocument = body.ownerDocument = docEl.ownerDocument = meta.ownerDocument = inlineScript.ownerDocument = script.ownerDocument = doc;

  var cookieStore = {};
  g.__cookieWrites = [];
  Object.defineProperty(doc, "cookie", {
    configurable: true,
    get: function(){
      logAccess("document.cookie:get");
      return Object.keys(cookieStore).map(function(k){ return k + "=" + cookieStore[k]; }).join("; ");
    },
    set: function(v){
      v = String(v == null ? "" : v);
      logAccess("document.cookie:set:"+v);
      g.__cookieWrites.push(v);
      var first = v.split(";")[0];
      var eq = first.indexOf("=");
      if (eq > 0) cookieStore[first.slice(0, eq).trim()] = first.slice(eq + 1).trim();
      return v;
    }
  });

  g.window = g;
  g.self = g;
  g.top = g;
  g.parent = g;
  g.frames = g;
  g.frameElement = null;
  g.document = doc;
  g.location = locationObj;
  var pdfPlugin = {name:"PDF Viewer", filename:"internal-pdf-viewer", description:"Portable Document Format"};
  var chromePdfPlugin = {name:"Chrome PDF Viewer", filename:"internal-pdf-viewer", description:"Portable Document Format"};
  var chromiumPdfPlugin = {name:"Chromium PDF Viewer", filename:"internal-pdf-viewer", description:"Portable Document Format"};
  var edgePdfPlugin = {name:"Microsoft Edge PDF Viewer", filename:"internal-pdf-viewer", description:"Portable Document Format"};
  var webkitPdfPlugin = {name:"WebKit built-in PDF", filename:"internal-pdf-viewer", description:"Portable Document Format"};
  var pluginsObj = {
    0: pdfPlugin, 1: chromePdfPlugin, 2: chromiumPdfPlugin, 3: edgePdfPlugin, 4: webkitPdfPlugin,
    length: 5,
    item: function(i){ return this[i] || null; },
    namedItem: function(n){ for(var i=0;i<this.length;i++){ if(this[i] && this[i].name===n) return this[i]; } return null; },
    refresh: noop
  };
  try { Object.defineProperty(pluginsObj, Symbol.toStringTag, {value:"PluginArray"}); } catch(e) {}
  pluginsObj[pdfPlugin.name] = pdfPlugin;
  pluginsObj[chromePdfPlugin.name] = chromePdfPlugin;
  pluginsObj[chromiumPdfPlugin.name] = chromiumPdfPlugin;
  pluginsObj[edgePdfPlugin.name] = edgePdfPlugin;
  pluginsObj[webkitPdfPlugin.name] = webkitPdfPlugin;
  var mimePdf = {type:"application/pdf", suffixes:"pdf", description:"Portable Document Format", enabledPlugin: pdfPlugin};
  var mimeTextPdf = {type:"text/pdf", suffixes:"pdf", description:"Portable Document Format", enabledPlugin: pdfPlugin};
  var mimeTypesObj = {
    0: mimePdf, 1: mimeTextPdf, length: 2,
    item: function(i){ return this[i] || null; },
    namedItem: function(n){ return this[n] || null; }
  };
  pluginsObj = new Proxy(pluginsObj, {
    get:function(t,p){ if (typeof p !== "symbol") logAccess("plugins."+String(p)); return t[p]; },
    set:function(t,p,v){ t[p]=v; return true; },
    has:function(t,p){ logAccess("plugins.has:"+String(p)); return p in t; }
  });
  mimeTypesObj = new Proxy(mimeTypesObj, {
    get:function(t,p){ if (typeof p !== "symbol") logAccess("mimeTypes."+String(p)); return t[p]; },
    set:function(t,p,v){ t[p]=v; return true; },
    has:function(t,p){ logAccess("mimeTypes.has:"+String(p)); return p in t; }
  });
  try {
    Object.defineProperty(pdfPlugin, Symbol.toStringTag, {value:"Plugin"});
    Object.defineProperty(chromePdfPlugin, Symbol.toStringTag, {value:"Plugin"});
    Object.defineProperty(chromiumPdfPlugin, Symbol.toStringTag, {value:"Plugin"});
    Object.defineProperty(edgePdfPlugin, Symbol.toStringTag, {value:"Plugin"});
    Object.defineProperty(webkitPdfPlugin, Symbol.toStringTag, {value:"Plugin"});
    Object.defineProperty(mimePdf, Symbol.toStringTag, {value:"MimeType"});
    Object.defineProperty(mimeTextPdf, Symbol.toStringTag, {value:"MimeType"});
    Object.defineProperty(mimeTypesObj, Symbol.toStringTag, {value:"MimeTypeArray"});
  } catch(e) {}
  mimeTypesObj[mimePdf.type] = mimePdf;
  mimeTypesObj[mimeTextPdf.type] = mimeTextPdf;
  var storageQuota = {
    queryUsageAndQuota: function(cb){ if (typeof cb === "function") cb(0, 0); },
    requestQuota: function(bytes, cb){ if (typeof cb === "function") cb(bytes || 0); }
  };
  try { Object.defineProperty(storageQuota, Symbol.toStringTag, {value:"DeprecatedStorageQuota"}); } catch(e) {}
  var navigatorObj = {
    userAgent: __UA__,
    appVersion: "5.0 (Macintosh; Intel Mac OS X 10_15_7)",
    platform: "MacIntel",
    language: "zh-CN",
    languages: ["zh-CN","zh"],
    vendor: "Google Inc.",
    product: "Gecko",
    productSub: "20030107",
    cookieEnabled: true,
    webdriver: false,
    onLine: true,
    hardwareConcurrency: 8,
    deviceMemory: 8,
    maxTouchPoints: 0,
    plugins: pluginsObj,
    mimeTypes: mimeTypesObj,
    webkitPersistentStorage: storageQuota,
    webkitTemporaryStorage: storageQuota,
    javaEnabled: function(){ return false; },
    sendBeacon: function(){ return true; }
  };
  if (webdriverMode === "undefined") delete navigatorObj.webdriver;
  else navigatorObj.webdriver = webdriverMode === "true";
  if (webkitStorageMode === "undefined") {
    delete navigatorObj.webkitPersistentStorage;
    delete navigatorObj.webkitTemporaryStorage;
  }
  if (mimeMode === "empty") {
    navigatorObj.plugins = {length:0,item:function(){return null},namedItem:function(){return null},refresh:noop};
    navigatorObj.mimeTypes = {length:0,item:function(){return null},namedItem:function(){return null}};
  }
  (function(){
    var values = {};
    Object.keys(navigatorObj).forEach(function(k){ values[k] = navigatorObj[k]; });
    var proto = {};
    Object.keys(values).forEach(function(k){
      Object.defineProperty(proto, k, {
        configurable: true,
        enumerable: true,
        get: function(){ return values[k]; }
      });
      delete navigatorObj[k];
    });
    Object.setPrototypeOf(navigatorObj, proto);
  })();
  try { Object.defineProperty(navigatorObj, Symbol.toStringTag, {value:"Navigator"}); } catch(e) {}
  g.navigator = new Proxy(navigatorObj, {get:function(t,p){ logAccess("navigator."+String(p)); return t[p]; }, set:function(t,p,v){ t[p]=v; return true; }, has:function(t,p){return p in t;}});
  var screenObj = {width:1920,height:1080,availWidth:1920,availHeight:1040,colorDepth:24,pixelDepth:24,orientation:{type:"landscape-primary",angle:0,addEventListener:noop,removeEventListener:noop}};
  try { Object.defineProperty(screenObj, Symbol.toStringTag, {value:"Screen"}); } catch(e) {}
  g.screen = new Proxy(screenObj, {get:function(t,p){ logAccess("screen."+String(p)); return t[p]; }, set:function(t,p,v){ t[p]=v; return true; }, has:function(t,p){return p in t;}});
  g.history = {length:1,state:null,pushState:noop,replaceState:noop,back:noop,forward:noop,go:noop};
  g.localStorage = storageStub();
  g.sessionStorage = storageStub();
  g.console = {log:noop,warn:noop,error:noop,info:noop,debug:noop,trace:noop,group:noop,groupEnd:noop,table:noop,assert:noop};
  g.setTimeout = ret1;
  g.setInterval = ret1;
  g.clearTimeout = noop;
  g.clearInterval = noop;
  g.requestAnimationFrame = ret1;
  g.cancelAnimationFrame = noop;
  g.requestIdleCallback = ret1;
  g.cancelIdleCallback = noop;
  g.queueMicrotask = function(f){ try { Promise.resolve().then(f); } catch(e) {} };
  g.addEventListener = noop;
  g.removeEventListener = noop;
  g.dispatchEvent = function(){ return true; };
  g.getComputedStyle = function(){ return {getPropertyValue:function(){return "";}}; };
  g.matchMedia = function(){ return {matches:false,media:"",addListener:noop,removeListener:noop,addEventListener:noop,removeEventListener:noop}; };
  g.Event = function(t){ this.type=t; this.bubbles=false; this.cancelable=false; };
  g.CustomEvent = function(t,o){ this.type=t; this.detail=o&&o.detail; };
  g.EventTarget = function(){};
  g.EventTarget.prototype.addEventListener = noop;
  g.EventTarget.prototype.removeEventListener = noop;
  g.EventTarget.prototype.dispatchEvent = function(){ return true; };
  g.MutationObserver = function(){ return {observe:noop,disconnect:noop,takeRecords:function(){return [];}}; };
  g.Image = function(){ return makeEl("img"); };
  g.Option = function(){ return makeEl("option"); };
  g.HTMLElement = function(){};
  g.HTMLInputElement = function(){};
  g.HTMLCanvasElement = function(){};
  g.XMLHttpRequest = function(){ return {open:noop,send:noop,setRequestHeader:noop,abort:noop,addEventListener:noop,removeEventListener:noop,getAllResponseHeaders:function(){return "";},getResponseHeader:function(){return null},readyState:0,status:0,responseText:"",response:""}; };
  g.fetch = function(){ return Promise.resolve({ok:true,status:200,headers:{get:function(){return null}},text:function(){return Promise.resolve("");},json:function(){return Promise.resolve({});},arrayBuffer:function(){return Promise.resolve(new ArrayBuffer(0));}}); };
  g.Headers = function(){};
  g.Request = function(u,o){ this.url=u; this.init=o||{}; };
  g.Response = function(b,o){ this.body=b; this.status=o&&o.status||200; this.ok=this.status<400; };
  g.crypto = {getRandomValues:function(a){ for(var i=0;i<(a?a.length:0);i++) a[i]=(i*1103515245+12345)&255; return a; }, randomUUID:function(){ return "00000000-0000-4000-8000-000000000000"; }, subtle:{}};
  var __nativeEval = (0, eval);
  g.__evalCodes = [];
  if (captureEval) {
    g.eval = function(s){ s = String(s); g.__evalCodes.push(s); return __nativeEval(s); };
  } else {
    g.eval = __nativeEval;
  }
  g.execScript = undefined;
  g.$_ss = {nsd: __NSD__, cd: __CD__};
  g.window["$_ss"] = g.$_ss;
  (function(){
    var seen = [];
    function seenHas(o){ for(var i=0;i<seen.length;i++){ if(seen[i]===o) return true; } return false; }
    function visit(o, depth){
      if (!o || depth > 4 || seenHas(o)) return;
      seen.push(o);
      var names = [];
      try { names = Object.getOwnPropertyNames(o); } catch(e) { return; }
      for (var i=0;i<names.length;i++) {
        var k = names[i], v;
        try { v = o[k]; } catch(e) { continue; }
        if (typeof v === "function") markNative(v, k);
        else if (v && typeof v === "object") visit(v, depth + 1);
      }
      try { visit(Object.getPrototypeOf(o), depth + 1); } catch(e) {}
    }
    [
      g.document, g.location, g.navigator, g.screen, g.history,
      g.localStorage, g.sessionStorage, g.EventTarget && g.EventTarget.prototype
    ].forEach(function(o){ visit(o, 0); });
    [
      "setTimeout","setInterval","clearTimeout","clearInterval","requestAnimationFrame",
      "cancelAnimationFrame","requestIdleCallback","cancelIdleCallback","addEventListener",
      "removeEventListener","dispatchEvent","getComputedStyle","matchMedia","Image","Option",
      "XMLHttpRequest","fetch","Headers","Request","Response"
    ].forEach(function(k){ markNative(g[k], k); });
  })();
})();
"#;

fn build_challenge_env(challenge: &ChallengeData) -> String {
    let parsed = reqwest::Url::parse(&challenge.page_url).ok();
    let href = parsed
        .as_ref()
        .map(|u| u.to_string())
        .unwrap_or_else(|| challenge.page_url.clone());
    let protocol = parsed
        .as_ref()
        .map(|u| format!("{}:", u.scheme()))
        .unwrap_or_else(|| "https:".to_string());
    let host = parsed
        .as_ref()
        .and_then(|u| {
            u.host_str().map(|h| match u.port() {
                Some(port) => format!("{h}:{port}"),
                None => h.to_string(),
            })
        })
        .unwrap_or_default();
    let hostname = parsed
        .as_ref()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_default();
    let origin = parsed
        .as_ref()
        .map(|u| {
            let port = u.port().map(|p| format!(":{p}")).unwrap_or_default();
            format!(
                "{}://{}{}",
                u.scheme(),
                u.host_str().unwrap_or_default(),
                port
            )
        })
        .unwrap_or_default();
    let pathname = parsed
        .as_ref()
        .map(|u| u.path().to_string())
        .unwrap_or_else(|| "/".to_string());
    let search = parsed
        .as_ref()
        .and_then(|u| u.query().map(|q| format!("?{q}")))
        .unwrap_or_default();

    HUBEI_CHALLENGE_ENV
        .replace("__HREF__", &json_lit(&href))
        .replace("__PROTOCOL__", &json_lit(&protocol))
        .replace("__HOST__", &json_lit(&host))
        .replace("__HOSTNAME__", &json_lit(&hostname))
        .replace("__ORIGIN__", &json_lit(&origin))
        .replace("__PATHNAME__", &json_lit(&pathname))
        .replace("__SEARCH__", &json_lit(&search))
        .replace("__META_CONTENT__", &json_lit(&challenge.meta_content))
        .replace("__SCRIPT_SRC__", &json_lit(&challenge.script_url))
        .replace("__INLINE_SCRIPT__", &json_lit(&challenge.inline_script))
        .replace(
            "__WEBDRIVER_MODE__",
            &json_lit(&std::env::var("HUBEI_WEBDRIVER").unwrap_or_else(|_| "false".to_string())),
        )
        .replace(
            "__WEBKIT_STORAGE_MODE__",
            &json_lit(
                &std::env::var("HUBEI_WEBKIT_STORAGE").unwrap_or_else(|_| "object".to_string()),
            ),
        )
        .replace(
            "__MIME_MODE__",
            &json_lit(&std::env::var("HUBEI_MIME_MODE").unwrap_or_else(|_| "chrome".to_string())),
        )
        .replace(
            "__CAPTURE_EVAL__",
            if std::env::var("HUBEI_DEBUG_EVAL").ok().as_deref() == Some("1") {
                "true"
            } else {
                "false"
            },
        )
        .replace("__UA__", &json_lit(CHROME_UA))
        .replace("__NSD__", &challenge.nsd.to_string())
        .replace("__CD__", &json_lit(&challenge.cd))
}

fn extract_js_number(src: &str, marker: &str) -> Option<u64> {
    let mut rest = &src[src.find(marker)? + marker.len()..];
    rest = rest.trim_start();
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

fn extract_js_string(src: &str, marker: &str) -> Option<String> {
    let rest = src[src.find(marker)? + marker.len()..].trim_start();
    let quote = rest.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let mut out = String::new();
    let mut chars = rest[quote.len_utf8()..].chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == quote {
            return Some(out);
        }
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        let Some(esc) = chars.next() else {
            return None;
        };
        match esc {
            '"' => out.push('"'),
            '\'' => out.push('\''),
            '\\' => out.push('\\'),
            '/' => out.push('/'),
            'b' => out.push('\u{0008}'),
            'f' => out.push('\u{000C}'),
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            't' => out.push('\t'),
            'x' => {
                let hex = take_n_chars(&mut chars, 2)?;
                out.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?);
            }
            'u' => {
                let hex = take_n_chars(&mut chars, 4)?;
                out.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?);
            }
            other => out.push(other),
        }
    }
    None
}

fn take_n_chars<I>(chars: &mut std::iter::Peekable<I>, n: usize) -> Option<String>
where
    I: Iterator<Item = char>,
{
    let mut out = String::with_capacity(n);
    for _ in 0..n {
        out.push(chars.next()?);
    }
    Some(out)
}

fn cookie_param_from_write(line: &str, url: &str) -> Option<SessionCookieParam> {
    let mut parts = line.split(';');
    let first = parts.next()?.trim();
    let (name, value) = first.split_once('=')?;
    let name = name.trim();
    if name.is_empty() {
        return None;
    }

    let mut cookie = SessionCookieParam {
        name: name.to_string(),
        value: value.trim().to_string(),
        url: Some(url.to_string()),
        domain: None,
        path: Some("/".to_string()),
        secure: None,
        http_only: None,
        expires: None,
    };
    for attr in parts {
        let attr = attr.trim();
        let (key, val) = attr.split_once('=').unwrap_or((attr, ""));
        match key.trim().to_ascii_lowercase().as_str() {
            "domain" if !val.trim().is_empty() => {
                cookie.domain = Some(val.trim().trim_start_matches('.').to_string());
            }
            "path" if !val.trim().is_empty() => cookie.path = Some(val.trim().to_string()),
            "secure" => cookie.secure = Some(true),
            "httponly" => cookie.http_only = Some(true),
            _ => {}
        }
    }
    Some(cookie)
}

fn js_run(ctx: &Ctx<'_>, code: &str) -> drission::Result<()> {
    let mut opts = EvalOptions::default();
    opts.strict = false;
    opts.global = true;
    ctx.eval_with_options::<(), _>(code, opts)
        .catch(ctx)
        .map_err(|e| drission::Error::msg(e.to_string()))
}

fn js_eval_string(ctx: &Ctx<'_>, expr: &str) -> drission::Result<String> {
    let code = format!(
        "(function(){{try{{var v=({expr});return v==null?\"\":String(v)}}catch(e){{return \"ERR:\"+(e&&e.stack||e)}}}})()"
    );
    ctx.eval::<String, _>(code)
        .catch(ctx)
        .map_err(|e| drission::Error::msg(e.to_string()))
}

fn json_lit(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string())
}

fn parse_list_page(html: &str, base_url: &str) -> drission::Result<ListPage> {
    let doc = Html::parse_document(html);
    let li_sel = selector(".hbgov-newslist-itemheight-18px > li, .hbgov-list-block li")?;
    let a_sel = selector("a[href]")?;
    let mut records = Vec::new();
    let mut seen = HashSet::new();

    for li in doc.select(&li_sel) {
        let scope_text = text_of(li);
        let date = find_date(&scope_text).unwrap_or_default();
        if date.is_empty() {
            continue;
        }

        let Some(a) = li.select(&a_sel).find(|link| {
            let text = text_of(*link);
            let href = link.attr("href").unwrap_or_default().trim();
            text.chars().count() >= 5
                && !is_bad_title(&text)
                && !is_bad_file_link(&text)
                && is_real_href(href)
        }) else {
            continue;
        };

        let text = text_of(a);
        let title = clean(a.attr("title").unwrap_or_default());
        let title = if title.is_empty() { text } else { title };
        if title.chars().count() < 5 || is_bad_title(&title) || is_bad_file_link(&title) {
            continue;
        }
        let Some(url) = absolute_url(base_url, a.attr("href").unwrap_or_default()) else {
            continue;
        };

        let key = format!("{title}|{url}");
        if seen.insert(key) {
            records.push(ListRecord { title, date, url });
        }
    }

    let next_url = parse_next_url(&doc, html, base_url)?;
    Ok(ListPage { records, next_url })
}

fn parse_next_url(doc: &Html, html: &str, base_url: &str) -> drission::Result<Option<String>> {
    let nav_sel = selector("#pages-nav, .hbgov-pagination")?;
    let active_sel = selector("li.active")?;
    let a_sel = selector("a[href]")?;

    for nav in doc.select(&nav_sel) {
        if let Some(active) = nav.select(&active_sel).next() {
            for sibling in active.next_siblings() {
                let Some(li) = ElementRef::wrap(sibling) else {
                    continue;
                };
                if !li.value().name().eq_ignore_ascii_case("li") {
                    continue;
                }
                if let Some(a) = li.select(&a_sel).next()
                    && let Some(url) = absolute_url(base_url, a.attr("href").unwrap_or_default())
                {
                    return Ok(Some(url));
                }
                break;
            }
        }

        for a in nav.select(&a_sel) {
            let label = clean(a.attr("aria-label").unwrap_or_default());
            let text = text_of(a);
            if (label.eq_ignore_ascii_case("next") || text.contains("下一"))
                && let Some(url) = absolute_url(base_url, a.attr("href").unwrap_or_default())
            {
                return Ok(Some(url));
            }
        }
    }
    Ok(parse_page_control_next(html, base_url))
}

fn parse_detail(html: &str, base_url: &str) -> drission::Result<DetailData> {
    let doc = Html::parse_document(html);
    let body_text = text_of(doc.root_element());

    let title = pick_meta(&doc, &["ArticleTitle", "title", "og:title"])?
        .or_else(|| {
            first_text(
                &doc,
                "h1, .hbgov-article-title, .article-title, .detail-title, .title",
            )
            .ok()
        })
        .unwrap_or_else(|| {
            pick_title_tag(&doc)
                .unwrap_or_default()
                .replace("- 湖北省人民政府门户网站", "")
                .trim()
                .to_string()
        });

    let publish_date = pick_meta(&doc, &["PubDate", "publishdate", "publishDate"])?
        .and_then(|s| find_date(&s))
        .or_else(|| find_date(&body_text))
        .unwrap_or_default();

    let source = pick_meta(&doc, &["ContentSource", "source", "Source"])?
        .or_else(|| extract_after_label(&body_text, &["来源", "信息来源"]))
        .unwrap_or_default();
    let document_no = find_document_no(&body_text).unwrap_or_default();

    let content_selectors = [
        "#zoom",
        ".TRS_Editor",
        ".hbgov-article-content",
        ".hbgov-detail-content",
        ".article-content",
        ".detail-content",
        ".content",
        "article",
    ];
    let mut content_el = None;
    let mut content = String::new();
    for sel in content_selectors {
        for el in doc.select(&selector(sel)?) {
            let text = clean_content(&text_of(el));
            if text.chars().count() > content.chars().count() {
                content = text;
                content_el = Some(el);
            }
        }
    }

    if content.is_empty() {
        let fallback_sel = selector("main, .container, .hbgov-bfc-block, section, div")?;
        let body_len = body_text.chars().count();
        for el in doc.select(&fallback_sel) {
            let text = clean_content(&text_of(el));
            let len = text.chars().count();
            if len > content.chars().count() && len < body_len.saturating_mul(9) / 10 {
                content = text;
                content_el = Some(el);
            }
        }
    }
    if !title.is_empty() && content.starts_with(&title) {
        content = clean_content(&content[title.len()..]);
    }

    let link_scope = content_el.unwrap_or_else(|| doc.root_element());
    let links = collect_links(link_scope, base_url)?;
    Ok(DetailData {
        title,
        publish_date,
        source,
        document_no,
        content,
        content_links: serde_json::to_string(&links)?,
    })
}

fn collect_links(scope: ElementRef<'_>, base_url: &str) -> drission::Result<Vec<ContentLink>> {
    let a_sel = selector("a[href]")?;
    let mut links = Vec::new();
    let mut seen = HashSet::new();
    for a in scope.select(&a_sel) {
        let href = a.attr("href").unwrap_or_default();
        if !is_real_href(href) {
            continue;
        }
        let Some(url) = absolute_url(base_url, href) else {
            continue;
        };
        let text = {
            let t = text_of(a);
            if t.is_empty() {
                clean(a.attr("title").unwrap_or_default())
            } else {
                t
            }
        };
        let key = format!("{text}|{url}");
        if seen.insert(key) {
            links.push(ContentLink { text, url });
        }
    }
    Ok(links)
}

fn pick_meta(doc: &Html, names: &[&str]) -> drission::Result<Option<String>> {
    let meta_sel = selector("meta")?;
    for name in names {
        for meta in doc.select(&meta_sel) {
            let key = meta.attr("name").or_else(|| meta.attr("property"));
            if key == Some(*name) {
                let val = clean(meta.attr("content").unwrap_or_default());
                if !val.is_empty() {
                    return Ok(Some(val));
                }
            }
        }
    }
    Ok(None)
}

fn first_text(doc: &Html, css: &str) -> drission::Result<String> {
    let sel = selector(css)?;
    Ok(doc
        .select(&sel)
        .map(text_of)
        .find(|s| !s.is_empty())
        .unwrap_or_default())
}

fn pick_title_tag(doc: &Html) -> Option<String> {
    let sel = selector("title").ok()?;
    doc.select(&sel)
        .map(text_of)
        .find(|title| !title.is_empty())
}

fn to_csv_rows(records: &[GovFile]) -> Vec<Vec<String>> {
    let mut rows = Vec::with_capacity(records.len() + 1);
    rows.push(vec![
        "list_page".to_string(),
        "list_date".to_string(),
        "list_title".to_string(),
        "url".to_string(),
        "detail_title".to_string(),
        "publish_date".to_string(),
        "source".to_string(),
        "document_no".to_string(),
        "content".to_string(),
        "content_links".to_string(),
    ]);
    rows.extend(records.iter().map(|r| {
        vec![
            r.list_page.to_string(),
            r.list_date.clone(),
            r.list_title.clone(),
            r.url.clone(),
            r.detail_title.clone(),
            r.publish_date.clone(),
            r.source.clone(),
            r.document_no.clone(),
            r.content.clone(),
            r.content_links.clone(),
        ]
    }));
    rows
}

async fn write_debug_html(kind: &str, page_no: usize, html: &str) -> drission::Result<()> {
    let dir = PathBuf::from("target/hubei_zfwj_protocol");
    tokio::fs::create_dir_all(&dir).await?;
    let path = dir.join(format!("debug_{kind}_{page_no}.html"));
    tokio::fs::write(&path, html).await?;
    println!("  诊断 HTML 已写出:{}", path.display());
    Ok(())
}

fn selector(css: &str) -> drission::Result<Selector> {
    Selector::parse(css)
        .map_err(|e| drission::Error::msg(format!("非法 CSS 选择器 {css:?}: {e:?}")))
}

fn text_of(el: ElementRef<'_>) -> String {
    clean(&el.text().collect::<String>())
}

fn clean(s: &str) -> String {
    let no_zero_width = s
        .chars()
        .filter(|c| !matches!(c, '\u{200B}'..='\u{200D}' | '\u{FEFF}'))
        .collect::<String>();
    no_zero_width
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn clean_content(s: &str) -> String {
    clean(s)
        .replace("分享到 微信", "")
        .replace("扫一扫在手机上查看当前页面", "")
        .replace("关闭打印", "")
        .trim()
        .to_string()
}

fn looks_like_js_challenge(html: &str) -> bool {
    html.contains("$_ss") || html.contains("nsd=") || html.contains("Precondition Failed")
}

fn is_real_href(href: &str) -> bool {
    let h = href.trim();
    !h.is_empty() && !h.starts_with('#') && !h.to_ascii_lowercase().starts_with("javascript:")
}

fn absolute_url(base_url: &str, href: &str) -> Option<String> {
    if !is_real_href(href) {
        return None;
    }
    reqwest::Url::parse(base_url)
        .ok()?
        .join(href.trim())
        .ok()
        .map(|u| u.to_string())
}

fn is_bad_title(s: &str) -> bool {
    matches!(
        clean(s).as_str(),
        "首页"
            | "政府信息公开"
            | "政策"
            | "文件"
            | "搜索"
            | "上一页"
            | "下一页"
            | "末页"
            | "尾页"
            | "更多"
            | "返回"
    )
}

fn is_bad_file_link(s: &str) -> bool {
    let t = clean(s)
        .trim_matches(|c| matches!(c, '[' | ']' | '【' | '】'))
        .to_string();
    ["解读", "图解", "附件"]
        .iter()
        .any(|prefix| t.starts_with(prefix))
}

fn parse_max_pages(s: &str) -> Option<usize> {
    let trimmed = s.trim();
    if trimmed.is_empty()
        || trimmed.eq_ignore_ascii_case("all")
        || trimmed.eq_ignore_ascii_case("全部")
    {
        return None;
    }
    trimmed.parse::<usize>().ok().filter(|n| *n > 0)
}

fn truncate_for_log(s: &str, max_chars: usize) -> String {
    let mut out: String = s.chars().take(max_chars).collect();
    if s.chars().count() > max_chars {
        out.push_str("...");
    }
    out
}

fn find_date(s: &str) -> Option<String> {
    let chars = s.chars().collect::<Vec<_>>();
    for i in 0..chars.len().saturating_sub(7) {
        if !chars[i..i + 4].iter().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let year = chars[i..i + 4].iter().collect::<String>();
        let mut pos = i + 4;
        let Some(sep) = chars.get(pos).copied() else {
            continue;
        };
        if !matches!(sep, '-' | '/' | '.' | '年') {
            continue;
        }
        pos += 1;
        let Some((month, next)) = parse_number(&chars, pos, 1, 2) else {
            continue;
        };
        pos = next;
        if sep == '年' {
            if chars.get(pos) != Some(&'月') {
                continue;
            }
        } else if !matches!(chars.get(pos), Some('-' | '/' | '.')) {
            continue;
        }
        pos += 1;
        let Some((day, _)) = parse_number(&chars, pos, 1, 2) else {
            continue;
        };
        if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
            continue;
        }
        return Some(format!("{year}-{month:02}-{day:02}"));
    }
    None
}

fn parse_page_control_next(html: &str, base_url: &str) -> Option<String> {
    let marker = "pageControl(";
    let start = html.find(marker)? + marker.len();
    let args_src = html[start..].split_once(");")?.0;
    let args = split_js_args(args_src);
    if args.len() < 4 {
        return None;
    }

    let total = args[0].trim().parse::<usize>().ok()?;
    let current = args[1].trim().parse::<usize>().ok()?;
    if current + 1 >= total {
        return None;
    }

    let prefix = unquote_js_arg(&args[2]);
    let ext = unquote_js_arg(&args[3]);
    if prefix.is_empty() || ext.is_empty() {
        return None;
    }

    let next = current + 1;
    let href = if next == 0 {
        format!("{prefix}.{ext}")
    } else {
        format!("{prefix}_{next}.{ext}")
    };
    absolute_url(base_url, &href)
}

fn split_js_args(src: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut cur = String::new();
    let mut quote = None;
    let mut escaped = false;
    for ch in src.chars() {
        if escaped {
            cur.push(ch);
            escaped = false;
            continue;
        }
        if ch == '\\' {
            cur.push(ch);
            escaped = true;
            continue;
        }
        if let Some(q) = quote {
            cur.push(ch);
            if ch == q {
                quote = None;
            }
            continue;
        }
        match ch {
            '\'' | '"' => {
                quote = Some(ch);
                cur.push(ch);
            }
            ',' => {
                args.push(cur.trim().to_string());
                cur.clear();
            }
            _ => cur.push(ch),
        }
    }
    if !cur.trim().is_empty() {
        args.push(cur.trim().to_string());
    }
    args
}

fn unquote_js_arg(s: &str) -> String {
    s.trim().trim_matches(|c| c == '\'' || c == '"').to_string()
}

fn parse_number(chars: &[char], pos: usize, min: usize, max: usize) -> Option<(u32, usize)> {
    let mut end = pos;
    while end < chars.len() && end - pos < max && chars[end].is_ascii_digit() {
        end += 1;
    }
    if end - pos < min {
        return None;
    }
    let n = chars[pos..end].iter().collect::<String>().parse().ok()?;
    Some((n, end))
}

fn extract_after_label(text: &str, labels: &[&str]) -> Option<String> {
    for label in labels {
        let Some(idx) = text.find(label) else {
            continue;
        };
        let tail = text[idx + label.len()..]
            .trim_start_matches(|c: char| c == '：' || c == ':' || c.is_whitespace());
        let val = tail
            .chars()
            .take_while(|c| !c.is_whitespace() && *c != '|' && *c != '　')
            .collect::<String>();
        if !val.is_empty() {
            return Some(val);
        }
    }
    None
}

fn find_document_no(text: &str) -> Option<String> {
    let chars = text.chars().collect::<Vec<_>>();
    for end in 0..chars.len() {
        if chars[end] != '号' {
            continue;
        }
        let start = end.saturating_sub(50);
        let Some(lb) = chars[start..=end].iter().position(|c| *c == '〔') else {
            continue;
        };
        let lb = start + lb;
        let Some(rb) = chars[lb..=end].iter().position(|c| *c == '〕') else {
            continue;
        };
        let rb = lb + rb;
        if rb >= end {
            continue;
        }
        let Some(pol) = chars[start..lb].iter().rposition(|c| *c == '政') else {
            continue;
        };
        let no_start = start + pol.saturating_sub(2);
        let candidate = chars[no_start..=end].iter().collect::<String>();
        if candidate.chars().count() <= 28 && candidate.chars().any(|c| c.is_ascii_digit()) {
            return Some(candidate.trim_matches(is_trim_punct).to_string());
        }
    }
    None
}

fn is_trim_punct(c: char) -> bool {
    c.is_whitespace() || matches!(c, '，' | '。' | '、' | ':' | '：' | '-' | '—' | '_' | '|')
}
