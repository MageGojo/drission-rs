//! 自包含「补环境 + 纯算签名」运行器(内嵌 QuickJS,零安装、无 Node、无浏览器)。
//!
//! 这是 `tab.dump_env()` 吐环境能力的**最后一公里**:把吐出的补环境 `env.js` 与录制种子 `seed.json`
//! 用 `include_str!` **编进二进制**,再内嵌 [`rquickjs`](https://crates.io/crates/rquickjs)(QuickJS)
//! 引擎在进程内执行。于是签名脚本要用到的浏览器环境(navigator / screen / location / document /
//! canvas / webgl / audio 指纹回放)在一个**单文件可执行程序**里就绪——
//! 用户拿到二进制直接跑,不必再装 Node、不必开浏览器、不必 `npm i`。
//!
//! 它做两件事:
//!   1. **自检补环境对不对**:在内嵌 QuickJS 里回放 `env.js`,逐字段对比 `seed.json` 录制值
//!      (这是“测你补的环境对不对”的离线、可重复版本,不依赖 Node 的 `verify.js`)。
//!   2. **纯算签名(可选)**:给一个目录参数,把其中(或其 `signer/` 子目录)的站点签名脚本
//!      加载进同一个补环境,然后枚举可疑签名全局;设 `SIGN_CALL` 环境变量即在补环境里调用它得到签名。
//!
//! 编译(产出单个二进制):
//! ```bash
//! cargo build --release --example env_signer --features signer
//! # => target/release/examples/env_signer
//! ```
//! 运行:
//! ```bash
//! ./target/release/examples/env_signer                 # 仅自检补环境(零依赖、零安装)
//! ./target/release/examples/env_signer ./douyin-env     # 再加载 signer/ 下脚本纯算签名
//! SIGN_CALL='window.byted_acrawler && window.byted_acrawler.sign({url:"..."})' \
//!   ./target/release/examples/env_signer ./douyin-env   # 直接调用签名函数取值
//! ```

use rquickjs::{CatchResultExt, Context, Ctx, Runtime};
use serde_json::{Value, json};

/// 补环境模块(由 `dump_env` 导出;内含录制种子 `__SEED__` 与 `setup()` 回放逻辑)。编进二进制。
const ENV_JS: &str = include_str!("../douyin-env/env.js");
/// 录制种子(对比基准:浏览器真实采集到的环境/指纹)。编进二进制。
const SEED_JSON: &str = include_str!("../douyin-env/seed.json");

/// 加载站点签名脚本前注入的「浏览器 IO/事件壳」补丁:补齐 env.js(只回放数据型环境)未覆盖的
/// 行为型 API(定时器/事件/fetch/XHR/crypto/TextEncoder…)。它们只是让风控/上报代码**初始化时不崩**,
/// 不参与签名计算本身;签名所需的真实数据(navigator/canvas/webgl/audio 指纹)仍由 env.js 提供。
const SHIMS_JS: &str = r#"(function (g) {
  function noop() {}
  function ret0() { return 0; }
  // 定时器:no-op(避免自重排导致死循环);签名主路径通常是同步计算。
  if (typeof g.setTimeout !== "function") g.setTimeout = ret0;
  if (typeof g.clearTimeout !== "function") g.clearTimeout = noop;
  if (typeof g.setInterval !== "function") g.setInterval = ret0;
  if (typeof g.clearInterval !== "function") g.clearInterval = noop;
  if (typeof g.requestAnimationFrame !== "function") g.requestAnimationFrame = ret0;
  if (typeof g.cancelAnimationFrame !== "function") g.cancelAnimationFrame = noop;
  if (typeof g.requestIdleCallback !== "function") g.requestIdleCallback = ret0;
  if (typeof g.queueMicrotask !== "function") g.queueMicrotask = function (f) { try { Promise.resolve().then(f); } catch (e) {} };
  // 事件:window/document 都补上(env.js 只给了 document 一部分)。
  ["addEventListener", "removeEventListener"].forEach(function (m) { if (typeof g[m] !== "function") g[m] = noop; });
  if (typeof g.dispatchEvent !== "function") g.dispatchEvent = function () { return true; };
  if (g.document) { ["addEventListener", "removeEventListener"].forEach(function (m) { if (typeof g.document[m] !== "function") g.document[m] = noop; }); g.document.dispatchEvent = g.document.dispatchEvent || function () { return true; }; }
  function Evt(t) { this.type = t; this.bubbles = false; this.target = null; }
  if (typeof g.Event !== "function") g.Event = Evt;
  if (typeof g.CustomEvent !== "function") g.CustomEvent = function (t, o) { Evt.call(this, t); this.detail = (o && o.detail) || null; };
  if (typeof g.EventTarget !== "function") { g.EventTarget = function () {}; g.EventTarget.prototype.addEventListener = noop; g.EventTarget.prototype.removeEventListener = noop; g.EventTarget.prototype.dispatchEvent = function () { return true; }; }
  if (typeof g.MutationObserver !== "function") g.MutationObserver = function () { return { observe: noop, disconnect: noop, takeRecords: function () { return []; } }; };
  // 网络:fetch/XHR/sendBeacon —— 风控上报用,返回空响应即可。
  if (typeof g.fetch !== "function") g.fetch = function () { return Promise.resolve({ ok: true, status: 200, headers: { get: function () { return null; } }, json: function () { return Promise.resolve({}); }, text: function () { return Promise.resolve(""); }, arrayBuffer: function () { return Promise.resolve(new ArrayBuffer(0)); } }); };
  if (typeof g.Headers !== "function") g.Headers = function () { var m = {}; this.append = function (k, v) { m[String(k).toLowerCase()] = v; }; this.set = this.append; this.get = function (k) { return m[String(k).toLowerCase()] != null ? m[String(k).toLowerCase()] : null; }; };
  if (typeof g.Request !== "function") g.Request = function (u, o) { this.url = u; this.init = o || {}; };
  if (typeof g.Response !== "function") g.Response = function (b, o) { this.body = b; this.status = (o && o.status) || 200; this.ok = this.status < 400; };
  if (typeof g.XMLHttpRequest !== "function") g.XMLHttpRequest = function () { return { open: noop, send: noop, setRequestHeader: noop, abort: noop, addEventListener: noop, getAllResponseHeaders: function () { return ""; }, readyState: 0, status: 0, responseText: "" }; };
  if (g.navigator && typeof g.navigator.sendBeacon !== "function") { try { g.navigator.sendBeacon = function () { return true; }; } catch (e) {} }
  // 计时/随机/编码:指纹与签名常用。
  if (typeof g.performance !== "object" || !g.performance) g.performance = {};
  if (typeof g.performance.now !== "function") g.performance.now = function () { return 0; };
  if (g.performance.timeOrigin === undefined) g.performance.timeOrigin = 0;
  if (typeof g.crypto !== "object" || !g.crypto) g.crypto = {};
  if (typeof g.crypto.getRandomValues !== "function") g.crypto.getRandomValues = function (a) { for (var i = 0; i < (a ? a.length : 0); i++) a[i] = (i * 1103515245 + 12345) & 0xff; return a; };
  if (typeof g.crypto.randomUUID !== "function") g.crypto.randomUUID = function () { return "00000000-0000-4000-8000-000000000000"; };
  if (typeof g.TextEncoder !== "function") g.TextEncoder = function () { this.encode = function (s) { s = String(s == null ? "" : s); var a = []; for (var i = 0; i < s.length; i++) { var c = s.charCodeAt(i); if (c < 128) a.push(c); else if (c < 2048) { a.push(192 | (c >> 6), 128 | (c & 63)); } else { a.push(224 | (c >> 12), 128 | ((c >> 6) & 63), 128 | (c & 63)); } } return new Uint8Array(a); }; };
  if (typeof g.TextDecoder !== "function") g.TextDecoder = function () { this.decode = function (b) { b = b || []; var s = ""; for (var i = 0; i < b.length; i++) s += String.fromCharCode(b[i]); return s; }; };
  if (typeof g.matchMedia !== "function") g.matchMedia = function () { return { matches: false, media: "", addListener: noop, removeListener: noop, addEventListener: noop, removeEventListener: noop }; };
  if (typeof g.getComputedStyle !== "function") g.getComputedStyle = function () { return { getPropertyValue: function () { return ""; } }; };
})(globalThis);"#;

fn main() {
    println!("==== drission 补环境运行器(内嵌 QuickJS · 无 Node · 无浏览器)====");

    let seed: Value = serde_json::from_str(SEED_JSON).expect("内置 seed.json 解析失败");
    let rt = Runtime::new().expect("QuickJS runtime");
    let ctx = Context::full(&rt).expect("QuickJS context");

    // 1) 装载补环境:eval env.js -> setup(globalThis) 把浏览器环境注入沙箱。
    ctx.with(|ctx| {
        if let Err(e) = js_run(&ctx, ENV_JS) {
            eprintln!("加载 env.js 失败: {e}");
            std::process::exit(1);
        }
    });

    // 2) 触发 audio 指纹回放(OfflineAudioContext 渲染是异步 Promise),结果写入 __AUDIO__。
    ctx.with(|ctx| {
        let _ = js_run(
            &ctx,
            r#"globalThis.__AUDIO__ = "__pending__";
               (function () { try {
                 var c = new OfflineAudioContext(1, 5000, 44100);
                 c.startRendering().then(function (buf) {
                   var d = buf.getChannelData(0), s = 0;
                   for (var i = 4500; i < 5000; i++) s += Math.abs(d[i]);
                   globalThis.__AUDIO__ = Math.round(s * 1e6) / 1e6;
                 });
               } catch (e) { globalThis.__AUDIO__ = null; } })();"#,
        );
    });
    pump_jobs(&rt); // 把微任务(Promise.then)跑完,__AUDIO__ 就绪

    // 3) 逐字段对比:env.js 回放值 vs seed.json 录制值。
    let checks = ctx.with(|ctx| collect_checks(&ctx, &seed));
    let (mut pass, mut fail) = (0usize, 0usize);
    let mut bad: Vec<(String, Value, Value)> = Vec::new();
    for (field, got, want) in checks {
        if values_match(&got, &want) {
            pass += 1;
        } else {
            fail += 1;
            bad.push((field, got, want));
        }
    }

    println!(
        "\n[自检] 补环境回放 vs 录制种子:{pass}/{} 字段一致",
        pass + fail
    );
    if fail == 0 {
        println!(
            "  ✅ 全部一致 —— 二进制里补出的环境忠实还原了浏览器(canvas/webgl/audio 指纹均回放正确)。"
        );
    } else {
        println!("  ⚠ {fail} 个字段不一致:");
        for (f, got, want) in &bad {
            println!("    {f} : env={got} | seed={want}");
        }
    }

    // 4) 可选:加载站点签名脚本,在补环境里纯算签名。
    if let Some(dir) = std::env::args().nth(1) {
        load_and_sign(&rt, &ctx, &dir);
    } else {
        println!(
            "\n提示:传入工程目录(如 ./douyin-env)可加载 signer/ 下签名脚本纯算签名;设 SIGN_CALL 直接调用签名函数。"
        );
    }

    std::process::exit(if fail == 0 { 0 } else { 1 });
}

/// 在补环境里加载 `dir`(或 `dir/signer`)下所有 `*.js` 签名脚本,枚举可疑签名全局,
/// 并在设置了 `SIGN_CALL` 时调用它得到签名值。
fn load_and_sign(rt: &Runtime, ctx: &Context, dir: &str) {
    let base = std::path::Path::new(dir);
    let scan = if base.join("signer").is_dir() {
        base.join("signer")
    } else {
        base.to_path_buf()
    };
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(&scan)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("js"))
        .collect();
    files.sort();

    if files.is_empty() {
        println!(
            "\n[signer] {} 下没有 *.js —— 把 signers.json 里的签名脚本下载到该目录后重跑即可纯算签名。",
            scan.display()
        );
        return;
    }

    // 先注入浏览器 IO/事件壳,让风控/上报代码初始化时不因缺 setTimeout/addEventListener/fetch 等而崩。
    ctx.with(|ctx| {
        if let Err(e) = js_run(&ctx, SHIMS_JS) {
            eprintln!("  [shims] 注入失败: {e}");
        }
    });

    println!("\n[signer] 加载签名脚本到补环境:");
    for f in &files {
        let name = f.file_name().and_then(|s| s.to_str()).unwrap_or("?");
        let code = std::fs::read_to_string(f).unwrap_or_default();
        let r = ctx.with(|ctx| js_run(&ctx, &code));
        pump_jobs(rt);
        match r {
            Ok(()) => println!("  - {name} ✓"),
            Err(e) => println!("  - {name} ✗ {e}"),
        }
    }

    let suspicious = ctx.with(|ctx| {
        js_eval_json(
            &ctx,
            "Object.keys(globalThis).filter(function(k){return /sign|bogus|acrawler|bdms|secsdk|byted|token/i.test(k);})",
        )
    });
    println!("  可疑签名全局:{suspicious}");

    if let Ok(expr) = std::env::var("SIGN_CALL") {
        let out = ctx.with(|ctx| js_eval_json(&ctx, &expr));
        pump_jobs(rt);
        println!("  SIGN_CALL 结果:{out}");
    }
}

/// 收集所有对比项 `(字段名, env.js 回放值, seed.json 录制值)`(对齐 dump_env 的 `verify.js`)。
fn collect_checks<'js>(ctx: &Ctx<'js>, seed: &Value) -> Vec<(String, Value, Value)> {
    let mut v = Vec::new();

    // navigator 标量(对象/数组如 userAgentData/languages 跳过,与 verify.js 一致)。
    if let Some(nav) = seed.get("navigator").and_then(Value::as_object) {
        for (k, want) in nav {
            if want.is_object() || want.is_array() {
                continue;
            }
            v.push((
                format!("navigator.{k}"),
                js_eval_json(ctx, &format!("navigator.{k}")),
                want.clone(),
            ));
        }
    }
    // screen 全字段。
    if let Some(scr) = seed.get("screen").and_then(Value::as_object) {
        for (k, want) in scr {
            v.push((
                format!("screen.{k}"),
                js_eval_json(ctx, &format!("screen.{k}")),
                want.clone(),
            ));
        }
    }
    // location host/origin。
    if let Some(loc) = seed.get("location") {
        for k in ["host", "origin"] {
            if let Some(want) = loc.get(k) {
                v.push((
                    format!("location.{k}"),
                    js_eval_json(ctx, &format!("location.{k}")),
                    want.clone(),
                ));
            }
        }
    }

    let fp = seed.get("fingerprint");
    let supported = |obj: &str| {
        fp.and_then(|f| f.pointer(&format!("/{obj}/supported")))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    };
    // canvas 2D 指纹回放。
    if supported("canvas") {
        v.push((
            "canvas.dataURL".into(),
            js_eval_json(ctx, "document.createElement('canvas').toDataURL()"),
            fp.and_then(|f| f.pointer("/canvas/dataURL"))
                .cloned()
                .unwrap_or(Value::Null),
        ));
    }
    // WebGL 指纹回放(getExtension + getParameter + getSupportedExtensions)。
    if supported("webgl") {
        v.push((
            "webgl.unmaskedVendor".into(),
            js_eval_json(
                ctx,
                "(function(){var g=document.createElement('canvas').getContext('webgl');var e=g.getExtension('WEBGL_debug_renderer_info');return e?g.getParameter(e.UNMASKED_VENDOR_WEBGL):null;})()",
            ),
            fp.and_then(|f| f.pointer("/webgl/unmaskedVendor"))
                .cloned()
                .unwrap_or(Value::Null),
        ));
        v.push((
            "webgl.unmaskedRenderer".into(),
            js_eval_json(
                ctx,
                "(function(){var g=document.createElement('canvas').getContext('webgl');var e=g.getExtension('WEBGL_debug_renderer_info');return e?g.getParameter(e.UNMASKED_RENDERER_WEBGL):null;})()",
            ),
            fp.and_then(|f| f.pointer("/webgl/unmaskedRenderer"))
                .cloned()
                .unwrap_or(Value::Null),
        ));
        let extc = fp
            .and_then(|f| f.pointer("/webgl/extensions"))
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0);
        v.push((
            "webgl.extCount".into(),
            js_eval_json(
                ctx,
                "(document.createElement('canvas').getContext('webgl').getSupportedExtensions()||[]).length",
            ),
            json!(extc),
        ));
    }
    // audio 指纹回放(已 pump_jobs,结果在 __AUDIO__)。
    if supported("audio") {
        let want = fp
            .and_then(|f| f.pointer("/audio/sum"))
            .and_then(Value::as_f64)
            .map(round6)
            .map(|x| json!(x))
            .unwrap_or(Value::Null);
        v.push((
            "audio.sum".into(),
            js_eval_json(ctx, "globalThis.__AUDIO__"),
            want,
        ));
    }
    v
}

/// 在沙箱里执行一段脚本(全局作用域,保留顶层声明);追加 `;void 0;` 保证完成值为 undefined,便于以 `()` 接收。
fn js_run<'js>(ctx: &Ctx<'js>, code: &str) -> Result<(), String> {
    ctx.eval::<(), _>(format!("{code}\n;void 0;"))
        .catch(ctx)
        .map_err(|e| e.to_string())
}

/// 在沙箱里求值一个表达式并以 JSON 取回为 `serde_json::Value`(出错返回 `Null`/`"<ERR:..>"`)。
fn js_eval_json<'js>(ctx: &Ctx<'js>, expr: &str) -> Value {
    let code = format!(
        "(function(){{ try {{ var v=({expr}); return JSON.stringify(v===undefined?null:v); }} catch(e){{ return JSON.stringify('<ERR:'+(e&&e.message)+'>'); }} }})()"
    );
    match ctx.eval::<String, _>(code) {
        Ok(s) => serde_json::from_str(&s).unwrap_or(Value::Null),
        Err(_) => Value::Null,
    }
}

/// 跑完所有待执行的微任务/作业(Promise 回调),让异步回放(audio)就绪。
fn pump_jobs(rt: &Runtime) {
    while rt.is_job_pending() {
        if rt.execute_pending_job().is_err() {
            break;
        }
    }
}

/// 数值容差比较(整数精确、浮点 1e-6);其余按 JSON 值严格相等。
fn values_match(a: &Value, b: &Value) -> bool {
    match (a.as_f64(), b.as_f64()) {
        (Some(x), Some(y)) if a.is_number() && b.is_number() => (x - y).abs() < 1e-6,
        _ => a == b,
    }
}

fn round6(x: f64) -> f64 {
    (x * 1e6).round() / 1e6
}
