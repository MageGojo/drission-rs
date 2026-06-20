//! 通用吐环境(dump browser env)**后端无关核心**。
//!
//! 面向 JS 逆向"补环境":在浏览器里针对指定签名参数(query/header/cookie 的某个 key)采集其生成所依赖的
//! 浏览器环境,导出可被 Node 直接 `require` 的补环境 `env.js`,并能同构双跑自验证。
//!
//! 这里只放**纯逻辑 + JS 配方/模板 + 类型**;两个后端(Camoufox/Juggler 与 Chromium/CDP)各自实现极薄的
//! [`EnvBackend`](trait)(导航前注入脚本 + 在页面求值),即可共用同一套探针 / `env.js` / 导出工程 / 验证。
//!
//! - [`EnvDumper`]/[`EnvProbe`] 泛型于 [`EnvBackend`];`tab.dump_env()`(两后端)返回它。
//! - [`EnvDump`] 是后端无关的结果(自包含 `Value`),其 [`verify`](EnvDump::verify) 泛型于后端。
//! - 资产模板(探针 / 指纹配方 / env 模板 / 导出工程)经 `include_str!` 内联,**录制==回放**单一来源。

use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::Result;

/// 探针模板:`__DUMP_CFG__`(运行配置)与 `__FP_RECIPES__`(指纹配方)会被替换后注入页面。
const PROBE_TEMPLATE: &str = include_str!("../browser/assets/dump_env_probe.js");
/// 指纹采集配方(canvas/webgl/audio…),探针与 verify 快照共用,保证录制==回放配方。
const FP_RECIPES: &str = include_str!("../browser/assets/fp_recipes.js");
/// 补环境回放运行时模板:`__SEED_JSON__` 替换为种子后即生成的 `env.js`。
const ENV_TEMPLATE: &str = include_str!("../browser/assets/env_template.js");
/// 一键导出工程模板(零依赖 Node 工程)。
const PROJ_PACKAGE_JSON: &str = include_str!("../browser/assets/project/package.json");
const PROJ_INDEX_JS: &str = include_str!("../browser/assets/project/index.js");
const PROJ_DEMO_JS: &str = include_str!("../browser/assets/project/demo.js");
const PROJ_VERIFY_JS: &str = include_str!("../browser/assets/project/verify.js");
const PROJ_README_MD: &str = include_str!("../browser/assets/project/README.md");

/// 默认追加的常见签名参数关键词(用于探针识别"哪个请求是签名请求"并记录其 writer)。
const DEFAULT_SIG: &[&str] = &[
    "a_bogus",
    "X-Bogus",
    "x-bogus",
    "msToken",
    "_signature",
    "verifyFp",
    "mssdk",
    "webid",
];

/// 吐环境的**后端适配**:导航前注入脚本 + 在页面求值。Camoufox `Tab` 与 CDP `ChromiumTab` 各实现一份。
pub trait EnvBackend {
    /// 导航前注入脚本(每个新文档最早期执行),并尽量在当前文档也执行一次。
    fn add_init_script(&self, script: &str) -> impl Future<Output = Result<()>>;
    /// 在页面执行 JS 表达式,返回结果值。
    fn run_js(&self, expr: &str) -> impl Future<Output = Result<Value>>;
}

/// 吐环境目标:定位"要针对哪个签名参数吐环境"。
#[derive(Debug, Clone)]
pub enum EnvTarget {
    /// URL query 参数,如 `a_bogus`。
    Query(String),
    /// 请求头,如 `x-bogus`(大小写不敏感匹配)。
    Header(String),
    /// cookie 字段,如 `msToken`。
    Cookie(String),
}

impl EnvTarget {
    fn kind(&self) -> &'static str {
        match self {
            EnvTarget::Query(_) => "query",
            EnvTarget::Header(_) => "header",
            EnvTarget::Cookie(_) => "cookie",
        }
    }

    fn key(&self) -> &str {
        match self {
            EnvTarget::Query(k) | EnvTarget::Header(k) | EnvTarget::Cookie(k) => k,
        }
    }

    fn to_json(&self) -> Value {
        json!({ "kind": self.kind(), "key": self.key() })
    }
}

/// `env.js` 裁剪范围。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvScope {
    /// 全量种子(吐全):navigator/screen/document/windowMetrics/storage 等。
    Full,
    /// 仅目标算法**实际读取**过的关键字段(只吐关键)。需先开 [`EnvDumper::proxy`] 追踪到 `access`。
    Accessed,
}

/// 吐环境构建器(泛型于后端)。由 `tab.dump_env()` 创建,链式配置后 [`start`](Self::start)。
pub struct EnvDumper<B: EnvBackend> {
    backend: B,
    targets: Vec<EnvTarget>,
    url_keywords: Vec<String>,
    watch: Vec<String>,
    proxy: bool,
}

impl<B: EnvBackend> EnvDumper<B> {
    /// 由后端(标签)创建一个吐环境构建器。
    pub fn new(backend: B) -> Self {
        Self {
            backend,
            targets: Vec::new(),
            url_keywords: Vec::new(),
            watch: vec!["navigator".into(), "screen".into()],
            proxy: false,
        }
    }

    /// 追加一个目标参数(可多次调用,累积多个目标)。
    pub fn target(mut self, t: EnvTarget) -> Self {
        self.targets.push(t);
        self
    }

    /// 追加一个 query 目标参数。
    pub fn target_query(self, key: &str) -> Self {
        self.target(EnvTarget::Query(key.to_string()))
    }

    /// 追加一个请求头目标参数(大小写不敏感)。
    pub fn target_header(self, key: &str) -> Self {
        self.target(EnvTarget::Header(key.to_string()))
    }

    /// 追加一个 cookie 目标参数。
    pub fn target_cookie(self, key: &str) -> Self {
        self.target(EnvTarget::Cookie(key.to_string()))
    }

    /// 限定"只针对包含该子串的请求"取目标值 / 记录 writer(可多次,任一命中即可)。
    pub fn match_url(mut self, keyword: &str) -> Self {
        self.url_keywords.push(keyword.to_string());
        self
    }

    /// 设置要 Proxy 追踪访问路径的顶层对象(默认 `["navigator","screen"]`)。仅 [`proxy(true)`](Self::proxy) 时生效。
    pub fn watch(mut self, objects: &[&str]) -> Self {
        self.watch = objects.iter().map(|s| s.to_string()).collect();
        self
    }

    /// 是否开启 Proxy 追踪(诊断"算法读了哪些环境字段",据此裁出 [`EnvScope::Accessed`])。
    ///
    /// 注意:对强检测站点替换 `navigator` 会被识破、导致不发签名请求,故默认 `false`(安全模式)。
    pub fn proxy(mut self, on: bool) -> Self {
        self.proxy = on;
        self
    }

    /// 注入探针(导航前 `add_init_script`),返回会话句柄 [`EnvProbe`]。**务必在 `tab.get` 之前调用**。
    pub async fn start(self) -> Result<EnvProbe<B>> {
        let probe_js = build_probe_js(&self.targets, &self.watch, &self.url_keywords, self.proxy);
        self.backend.add_init_script(&probe_js).await?;
        Ok(EnvProbe {
            backend: self.backend,
            targets: self.targets,
            proxy: self.proxy,
            seed: Value::Null,
            access: Value::Null,
            sinks: Vec::new(),
            hits: Vec::new(),
        })
    }
}

/// 已注入探针的会话句柄(泛型于后端)。多次导航时反复 [`collect`](Self::collect) 累积。
pub struct EnvProbe<B: EnvBackend> {
    backend: B,
    #[allow(dead_code)]
    targets: Vec<EnvTarget>,
    proxy: bool,
    seed: Value,
    access: Value,
    sinks: Vec<Value>,
    hits: Vec<Value>,
}

impl<B: EnvBackend> EnvProbe<B> {
    /// 抽取当前页面探针状态(seed/access/sinks/targets),累积去重,返回当前累积的 [`EnvDump`]。
    pub async fn collect(&mut self) -> Result<EnvDump> {
        if let Ok(s) = self
            .backend
            .run_js("window.__DUMP__ ? window.__DUMP__.collectSeed() : null")
            .await
            && s.is_object()
        {
            self.seed = s;
        }
        if let Ok(a) = self
            .backend
            .run_js(
                "window.__DUMP__ ? ({order: window.__DUMP__.accessOrder, count: window.__DUMP__.access}) : null",
            )
            .await
            && a.is_object()
        {
            self.access = a;
        }
        if let Ok(v) = self
            .backend
            .run_js("window.__DUMP__ ? window.__DUMP__.sinks : []")
            .await
            && let Some(arr) = v.as_array()
        {
            for it in arr {
                if !self.sinks.contains(it) {
                    self.sinks.push(it.clone());
                }
            }
        }
        if let Ok(v) = self
            .backend
            .run_js("window.__DUMP__ ? window.__DUMP__.targets : []")
            .await
            && let Some(arr) = v.as_array()
        {
            for it in arr {
                if !self.hits.contains(it) {
                    self.hits.push(it.clone());
                }
            }
        }
        Ok(self.dump())
    }

    /// 当前累积结果快照(不访问页面)。
    pub fn dump(&self) -> EnvDump {
        EnvDump {
            seed: self.seed.clone(),
            access: self.access.clone(),
            sinks: self.sinks.clone(),
            targets: self.hits.clone(),
            proxy: self.proxy,
        }
    }

    /// 记录一个外部命中的目标值(例如从监听到的真实上线值),去重累积。
    pub fn record_hit(&mut self, kind: &str, key: &str, value: &str, url: &str) {
        let item =
            json!({ "kind": kind, "key": key, "value": value, "url": url, "source": "listen" });
        let dup = self.hits.iter().any(|h| {
            h["kind"] == item["kind"] && h["key"] == item["key"] && h["value"] == item["value"]
        });
        if !dup {
            self.hits.push(item);
        }
    }

    /// 句柄持有的后端(标签),与外部 `tab` 是同一会话的克隆。
    pub fn backend(&self) -> &B {
        &self.backend
    }
}

/// 一次吐环境的结果(已从页面取回、自包含、后端无关)。
#[derive(Debug, Clone)]
pub struct EnvDump {
    /// 全量真实环境种子。
    pub seed: Value,
    /// Proxy 追踪到的访问路径 `{order:[...], count:{...}}`(未开 proxy 时为 `Null`)。
    pub access: Value,
    /// 命中签名参数的请求 writer(URL + 调用栈)。
    pub sinks: Vec<Value>,
    /// 命中的目标参数(kind/key/value/url[/stack])。
    pub targets: Vec<Value>,
    /// 本次是否开了 Proxy 追踪。
    pub proxy: bool,
}

impl EnvDump {
    /// 仅按 `access` 路径从种子裁出"关键、被实际读取"的精简种子(始终保留补环境必需骨架)。
    pub fn accessed_seed(&self) -> Value {
        prune_seed(&self.seed, &self.access)
    }

    /// 生成 Node 补环境 `env.js`。`Full` 用全量种子;`Accessed` 按访问路径裁剪(需先 `proxy` 追踪)。
    pub fn env_js(&self, scope: EnvScope) -> String {
        let seed = match scope {
            EnvScope::Full => self.seed.clone(),
            EnvScope::Accessed => self.accessed_seed(),
        };
        build_env_js(&seed)
    }

    /// 是否拿到了有效的访问集(开了 proxy 且非空)。
    pub fn has_access(&self) -> bool {
        self.access
            .get("order")
            .and_then(Value::as_array)
            .is_some_and(|a| !a.is_empty())
    }

    /// 把全部产物写入目录:`seed.json`/`access.json`/`sinks.json`/`targets.json`/`env.js`
    /// (有访问集时再吐精简 `env.accessed.js`)。
    pub fn write_to(&self, dir: impl AsRef<Path>) -> Result<()> {
        let dir = dir.as_ref();
        std::fs::create_dir_all(dir)?;
        write_json(&dir.join("seed.json"), &self.seed)?;
        write_json(&dir.join("access.json"), &self.access)?;
        write_json(&dir.join("sinks.json"), &json!(self.sinks))?;
        write_json(&dir.join("signers.json"), &json!(self.signers()))?;
        write_json(&dir.join("targets.json"), &json!(self.targets))?;
        std::fs::write(dir.join("env.js"), self.env_js(EnvScope::Full))?;
        if self.has_access() {
            std::fs::write(dir.join("env.accessed.js"), self.env_js(EnvScope::Accessed))?;
        }
        Ok(())
    }

    /// 用 Node **同构双跑**验证 `env.js` 是否忠实还原浏览器:基于种子动态生成快照,在【浏览器真实
    /// 环境】与【Node `vm` 沙箱 + `env.js`】各跑一次,逐字段对比。需要本机有 `node`;没有则返回 `{error}`。
    /// 泛型于后端:`backend` 即标签(`&Tab` / `&ChromiumTab`)。
    pub async fn verify<B: EnvBackend>(
        &self,
        backend: &B,
        dir: impl AsRef<Path>,
        scope: EnvScope,
    ) -> Result<Value> {
        let dir = dir.as_ref();
        let seed = match scope {
            EnvScope::Full => self.seed.clone(),
            EnvScope::Accessed => self.accessed_seed(),
        };
        let snapshot = gen_snapshot_js(&seed);
        let mut r_browser = backend
            .run_js(&format!("(function(){{ {snapshot} }})()"))
            .await?;
        // canvas/webgl/audio:浏览器侧改取录制值,与 Node 回放同源比对(见 fill_recorded_fp 说明)。
        fill_recorded_fp(&mut r_browser, &seed);

        std::fs::create_dir_all(dir)?;
        std::fs::write(dir.join("env.verify.js"), build_env_js(&seed))?;
        let snap_lit = serde_json::to_string(&snapshot).unwrap_or_else(|_| "\"\"".into());
        let verify_js = format!(
            "const vm = require('vm');\n\
             const env = require('./env.verify.js');\n\
             const sandbox = {{}};\n\
             env.setup(sandbox);\n\
             vm.createContext(sandbox);\n\
             const res = vm.runInContext('(function(){{ ' + {snap_lit} + ' }})()', sandbox);\n\
             const audioCode = \"(async function(){{ try {{ if (typeof OfflineAudioContext === 'undefined') return null; var ctx = new OfflineAudioContext(1,5000,44100); var buf = await ctx.startRendering(); var d = buf.getChannelData(0); var s = 0; for (var i=4500;i<5000;i++) s += Math.abs(d[i]); return Math.round(s*1e6)/1e6; }} catch(e){{ return null; }} }})()\";\n\
             Promise.resolve(vm.runInContext(audioCode, sandbox)).then(function (a) {{ if (a !== null && a !== undefined) res['audio.sum'] = a; process.stdout.write(JSON.stringify(res)); }}).catch(function () {{ process.stdout.write(JSON.stringify(res)); }});\n"
        );
        std::fs::write(dir.join("verify-run.js"), &verify_js)?;

        let output = match std::process::Command::new("node")
            .arg("verify-run.js")
            .current_dir(dir)
            .output()
        {
            Ok(o) => o,
            Err(e) => return Ok(json!({ "error": format!("无法运行 node: {e}") })),
        };
        if !output.status.success() {
            return Ok(
                json!({ "error": String::from_utf8_lossy(&output.stderr).trim().to_string() }),
            );
        }
        let r_node: Value = serde_json::from_slice(&output.stdout).unwrap_or(Value::Null);
        Ok(compare(&r_browser, &r_node))
    }

    /// 命中签名请求的 writer 调用栈通用化定位到的「签名脚本」列表(频次降序)。
    pub fn signers(&self) -> Vec<Value> {
        parse_signers(&self.sinks)
    }

    /// 一键导出**可直接 `node` 运行的补环境工程**(npm 包结构,零依赖)。返回工程目录。
    pub fn export_project(&self, dir: impl AsRef<Path>, scope: EnvScope) -> Result<PathBuf> {
        let dir = dir.as_ref();
        std::fs::create_dir_all(dir)?;
        let seed = match scope {
            EnvScope::Full => self.seed.clone(),
            EnvScope::Accessed => self.accessed_seed(),
        };
        let pkg_name = dir
            .file_name()
            .and_then(|s| s.to_str())
            .filter(|s| !s.is_empty())
            .unwrap_or("drission-env");
        std::fs::write(dir.join("env.js"), build_env_js(&seed))?;
        std::fs::write(dir.join("index.js"), PROJ_INDEX_JS)?;
        std::fs::write(dir.join("demo.js"), PROJ_DEMO_JS)?;
        std::fs::write(dir.join("verify.js"), PROJ_VERIFY_JS)?;
        std::fs::write(
            dir.join("package.json"),
            PROJ_PACKAGE_JSON.replace("__PKG_NAME__", pkg_name),
        )?;
        std::fs::write(
            dir.join("README.md"),
            PROJ_README_MD.replace("__PKG_NAME__", pkg_name),
        )?;
        write_json(&dir.join("seed.json"), &seed)?;
        write_json(&dir.join("signers.json"), &json!(self.signers()))?;
        write_json(&dir.join("targets.json"), &json!(self.targets))?;
        write_json(&dir.join("sinks.json"), &json!(self.sinks))?;
        std::fs::create_dir_all(dir.join("signer"))?;
        Ok(dir.to_path_buf())
    }
}

/// 从配置构造探针 JS(填充 `__FP_RECIPES__` 与 `__DUMP_CFG__`)。
fn build_probe_js(
    targets: &[EnvTarget],
    watch: &[String],
    url_keywords: &[String],
    proxy: bool,
) -> String {
    let mut sig: Vec<String> = targets.iter().map(|t| t.key().to_string()).collect();
    for d in DEFAULT_SIG {
        if !sig.iter().any(|s| s == d) {
            sig.push((*d).to_string());
        }
    }
    let cfg = json!({
        "proxy": proxy,
        "watch": watch,
        "sig": sig,
        "urlMatch": url_keywords,
        "targets": targets.iter().map(EnvTarget::to_json).collect::<Vec<_>>(),
    });
    PROBE_TEMPLATE
        .replace("__FP_RECIPES__", FP_RECIPES)
        .replace("__DUMP_CFG__", &cfg.to_string())
}

fn write_json(path: &Path, v: &Value) -> Result<()> {
    std::fs::write(path, serde_json::to_string_pretty(v)?)?;
    Ok(())
}

/// 基于种子生成 Node 补环境模块(`env.js`)。模板见 `assets/env_template.js`。
fn build_env_js(seed: &Value) -> String {
    let seed_pretty = serde_json::to_string_pretty(seed).unwrap_or_else(|_| "null".into());
    ENV_TEMPLATE.replace("__SEED_JSON__", &seed_pretty)
}

/// 按 `access.order` 路径从种子裁出精简种子,并强制保留补环境必需骨架。
fn prune_seed(seed: &Value, access: &Value) -> Value {
    let mut out = json!({});
    if let Some(order) = access.get("order").and_then(Value::as_array) {
        for p in order {
            let Some(raw) = p.as_str() else { continue };
            let path = raw.replace("(in)", "");
            let parts: Vec<&str> = path.split('.').filter(|s| !s.is_empty()).collect();
            if parts.len() < 2 {
                continue;
            }
            if let Some(v) = get_path(seed, &parts) {
                let v = v.clone();
                set_path(&mut out, &parts, v);
            }
        }
    }
    if let Some(loc) = seed.get("location") {
        out["location"] = loc.clone();
    }
    let cookie = seed
        .pointer("/document/cookie")
        .cloned()
        .unwrap_or_else(|| json!(""));
    if !out.get("document").map(Value::is_object).unwrap_or(false) {
        out["document"] = json!({});
    }
    out["document"]["cookie"] = cookie;
    if !out.get("navigator").map(Value::is_object).unwrap_or(false) {
        out["navigator"] = json!({});
    }
    if out["navigator"].get("userAgent").is_none()
        && let Some(ua) = seed.pointer("/navigator/userAgent")
    {
        out["navigator"]["userAgent"] = ua.clone();
    }
    if let Some(fp) = seed.get("fingerprint") {
        out["fingerprint"] = fp.clone();
    }
    if let Some(wm) = seed.get("windowMetrics") {
        out["windowMetrics"] = wm.clone();
    }
    out
}

fn get_path<'a>(v: &'a Value, parts: &[&str]) -> Option<&'a Value> {
    let mut cur = v;
    for p in parts {
        cur = cur.get(p)?;
    }
    Some(cur)
}

fn set_path(root: &mut Value, parts: &[&str], val: Value) {
    let Some((last, prefix)) = parts.split_last() else {
        return;
    };
    let mut cur = root;
    for p in prefix {
        if !cur.get(*p).map(Value::is_object).unwrap_or(false) {
            cur[*p] = json!({});
        }
        cur = &mut cur[*p];
    }
    cur[*last] = val;
}

/// 基于种子动态生成"对齐用快照"JS(被包进 `(function(){ <这里> })()` 执行)。
fn gen_snapshot_js(seed: &Value) -> String {
    let mut items: Vec<String> = Vec::new();
    if let Some(map) = seed.get("navigator").and_then(Value::as_object) {
        for (k, val) in map {
            match val {
                Value::Object(_) => {}
                Value::Array(_) => {
                    if k == "languages" {
                        items.push(
                            "  \"navigator.languages\": g(function () { return (navigator.languages || []).join(\",\"); })".into(),
                        );
                    }
                }
                _ => items.push(format!(
                    "  \"navigator.{k}\": g(function () {{ return navigator.{k}; }})"
                )),
            }
        }
    }
    if let Some(map) = seed.get("screen").and_then(Value::as_object) {
        for k in map.keys() {
            items.push(format!(
                "  \"screen.{k}\": g(function () {{ return screen.{k}; }})"
            ));
        }
    }
    for k in ["host", "origin"] {
        items.push(format!(
            "  \"location.{k}\": g(function () {{ return location.{k}; }})"
        ));
    }
    let fp = seed.get("fingerprint");
    let supported = |obj: &str| {
        fp.and_then(|f| f.get(obj))
            .and_then(|c| c.get("supported"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    };
    if supported("canvas") {
        items.push("  \"canvas.dataURL\": g(function () { return __C.dataURL; })".into());
    }
    if supported("webgl") {
        for (key, expr) in [
            ("webgl.unmaskedVendor", "__W.unmaskedVendor"),
            ("webgl.unmaskedRenderer", "__W.unmaskedRenderer"),
            ("webgl.vendor", "__W.parameters && __W.parameters[7936]"),
            ("webgl.renderer", "__W.parameters && __W.parameters[7937]"),
            ("webgl.version", "__W.parameters && __W.parameters[7938]"),
            ("webgl.glsl", "__W.parameters && __W.parameters[35724]"),
            (
                "webgl.maxTextureSize",
                "__W.parameters && __W.parameters[3379]",
            ),
            ("webgl.extCount", "(__W.extensions || []).length"),
        ] {
            items.push(format!("  \"{key}\": g(function () {{ return {expr}; }})"));
        }
    }
    if supported("fonts") {
        items.push("  \"fonts.hash\": g(function () { return __F.hash; })".into());
        items.push(
            "  \"fonts.count\": g(function () { return (__F.detected || []).length; })".into(),
        );
    }
    if supported("canvasPixels") {
        items.push("  \"canvasPixels.hash\": g(function () { return __P.hash; })".into());
    }
    items.push("  \"rtc.supported\": g(function () { return !!__R.supported; })".into());
    items.push(
        "  \"navigator.pluginsCount\": g(function () { return navigator.plugins ? navigator.plugins.length : 0; })"
            .into(),
    );
    items.push(
        "  \"navigator.plugins0\": g(function () { return (navigator.plugins && navigator.plugins[0]) ? navigator.plugins[0].name : null; })"
            .into(),
    );
    items.push(
        "  \"navigator.mimeTypesCount\": g(function () { return navigator.mimeTypes ? navigator.mimeTypes.length : 0; })"
            .into(),
    );
    format!(
        "{recipes}\nfunction g(f) {{ try {{ var v = f(); return v === undefined ? null : v; }} catch (e) {{ return \"<ERR:\" + (e && e.message) + \">\"; }} }}\nvar __C = (function () {{ try {{ return __fpCanvas(); }} catch (e) {{ return {{}}; }} }})();\nvar __W = (function () {{ try {{ return __fpWebGL(); }} catch (e) {{ return {{}}; }} }})();\nvar __F = (function () {{ try {{ return __fpFonts(); }} catch (e) {{ return {{}}; }} }})();\nvar __P = (function () {{ try {{ return __fpCanvasPixels(); }} catch (e) {{ return {{}}; }} }})();\nvar __R = (function () {{ try {{ return __fpRtc(); }} catch (e) {{ return {{}}; }} }})();\nreturn {{\n{items}\n}};\n",
        recipes = FP_RECIPES,
        items = items.join(",\n")
    )
}

/// 把浏览器快照里的 canvas/webgl/audio 字段改为 seed 录制值,使其与 Node 侧回放值同源对齐。
fn fill_recorded_fp(browser: &mut Value, seed: &Value) {
    let Some(obj) = browser.as_object_mut() else {
        return;
    };
    let fp = seed.get("fingerprint");
    let sup = |o: &str| {
        fp.and_then(|f| f.get(o))
            .and_then(|c| c.get("supported"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    };
    if sup("canvas")
        && let Some(d) = fp.and_then(|f| f.pointer("/canvas/dataURL"))
    {
        obj.insert("canvas.dataURL".into(), d.clone());
    }
    if sup("webgl") {
        let w = fp.and_then(|f| f.get("webgl"));
        let getp = |k: &str| {
            w.and_then(|w| w.pointer(&format!("/parameters/{k}")))
                .cloned()
                .unwrap_or(Value::Null)
        };
        let get = |k: &str| w.and_then(|w| w.get(k)).cloned().unwrap_or(Value::Null);
        obj.insert("webgl.unmaskedVendor".into(), get("unmaskedVendor"));
        obj.insert("webgl.unmaskedRenderer".into(), get("unmaskedRenderer"));
        obj.insert("webgl.vendor".into(), getp("7936"));
        obj.insert("webgl.renderer".into(), getp("7937"));
        obj.insert("webgl.version".into(), getp("7938"));
        obj.insert("webgl.glsl".into(), getp("35724"));
        obj.insert("webgl.maxTextureSize".into(), getp("3379"));
        let extc = w
            .and_then(|w| w.get("extensions"))
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0);
        obj.insert("webgl.extCount".into(), json!(extc));
    }
    if sup("audio")
        && let Some(sum) = fp
            .and_then(|f| f.pointer("/audio/sum"))
            .and_then(Value::as_f64)
    {
        obj.insert("audio.sum".into(), json!(round6(sum)));
    }
    if sup("fonts") {
        let f = fp.and_then(|f| f.get("fonts"));
        if let Some(h) = f.and_then(|f| f.get("hash")) {
            obj.insert("fonts.hash".into(), h.clone());
        }
        let cnt = f
            .and_then(|f| f.get("detected"))
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0);
        obj.insert("fonts.count".into(), json!(cnt));
    }
    if sup("canvasPixels")
        && let Some(h) = fp.and_then(|f| f.pointer("/canvasPixels/hash"))
    {
        obj.insert("canvasPixels.hash".into(), h.clone());
    }
    let rtc_sup = fp
        .and_then(|f| f.pointer("/rtc/supported"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    obj.insert("rtc.supported".into(), json!(rtc_sup));
    if let Some(nav) = seed.get("navigator") {
        let pcount = nav
            .get("plugins")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0);
        obj.insert("navigator.pluginsCount".into(), json!(pcount));
        obj.insert(
            "navigator.plugins0".into(),
            nav.pointer("/plugins/0/name")
                .cloned()
                .unwrap_or(Value::Null),
        );
        let mcount = nav
            .get("mimeTypes")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0);
        obj.insert("navigator.mimeTypesCount".into(), json!(mcount));
    }
}

fn round6(x: f64) -> f64 {
    (x * 1e6).round() / 1e6
}

/// 从 sink 调用栈通用化定位「签名脚本」(按文件聚合计数,频次降序)。
fn parse_signers(sinks: &[Value]) -> Vec<Value> {
    let mut map: HashMap<String, (u64, u64, u64, String)> = HashMap::new();
    for s in sinks {
        let stack = s.get("stack").and_then(Value::as_str).unwrap_or("");
        let req = s
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if let Some((url, line, col)) = first_http_frame(stack) {
            let e = map.entry(url).or_insert((0, line, col, req));
            e.0 += 1;
        }
    }
    let mut v: Vec<Value> = map
        .into_iter()
        .map(|(url, (count, line, col, req))| {
            json!({ "url": url, "line": line, "col": col, "count": count, "sample_request": req })
        })
        .collect();
    v.sort_by(|a, b| {
        b["count"]
            .as_u64()
            .cmp(&a["count"].as_u64())
            .then_with(|| a["url"].as_str().cmp(&b["url"].as_str()))
    });
    v
}

/// 取栈字符串里第一个(最靠上)http(s) 帧,返回 `(脚本URL, 行, 列)`。兼容 FF 与 Chrome 两种栈格式。
fn first_http_frame(stack: &str) -> Option<(String, u64, u64)> {
    let pos = ["https://", "http://"]
        .iter()
        .filter_map(|m| stack.find(m))
        .min()?;
    let rest = &stack[pos..];
    let end = rest
        .find(|c: char| c.is_whitespace() || c == ')' || c == '\'' || c == '"')
        .unwrap_or(rest.len());
    let token = &rest[..end];
    let mut it = token.rsplitn(3, ':');
    let col = it.next();
    let line = it.next();
    let url = it.next();
    match (url, line, col) {
        (Some(u), Some(l), Some(c)) => match (l.parse::<u64>(), c.parse::<u64>()) {
            (Ok(l), Ok(c)) => Some((u.to_string(), l, c)),
            _ => Some((token.to_string(), 0, 0)),
        },
        _ => Some((token.to_string(), 0, 0)),
    }
}

/// 逐字段对比浏览器快照与 Node 快照,生成报告。
fn compare(browser: &Value, node: &Value) -> Value {
    let mut fields = Vec::new();
    let (mut pass, mut fail) = (0usize, 0usize);
    if let Some(map) = browser.as_object() {
        for (k, bv) in map {
            let nv = node.get(k).cloned().unwrap_or(Value::Null);
            let ok = &nv == bv;
            if ok {
                pass += 1;
            } else {
                fail += 1;
            }
            fields.push(json!({ "field": k, "ok": ok, "browser": bv, "node": nv }));
        }
    }
    json!({
        "pass": pass, "fail": fail, "total": pass + fail,
        "browser": browser, "node": node, "fields": fields,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_template_replaces_all_placeholders() {
        assert_eq!(
            PROBE_TEMPLATE.matches("__DUMP_CFG__").count(),
            1,
            "__DUMP_CFG__ 必须恰好出现 1 次(在 `var CFG =` 处)"
        );
        assert_eq!(
            PROBE_TEMPLATE.matches("__FP_RECIPES__").count(),
            1,
            "__FP_RECIPES__ 必须恰好出现 1 次"
        );
        let replaced = PROBE_TEMPLATE
            .replace("__FP_RECIPES__", FP_RECIPES)
            .replace("__DUMP_CFG__", "{\"proxy\":false}");
        assert!(
            !replaced.contains("__DUMP_CFG__"),
            "替换后不应再有配置占位符"
        );
        assert!(
            !replaced.contains("__FP_RECIPES__"),
            "替换后不应再有配方占位符"
        );
        assert!(replaced.contains("var CFG = {\"proxy\":false};"));
        assert!(replaced.contains("function __fpCanvas()"));
        assert!(replaced.contains("function __fpWebGL()"));
        assert!(replaced.contains("function __fpAudioAsync("));
        assert!(replaced.contains("function __fpFonts()"));
        assert!(replaced.contains("function __fpCanvasPixels()"));
        assert!(replaced.contains("function __fpRtc()"));
        assert!(replaced.contains("function __fpHash("));
        assert!(replaced.contains("function __fpHashBytes("));
        assert!(replaced.contains("canvasPixels: sc("));
        assert!(replaced.contains("rtc: sc("));
        assert!(replaced.contains("plugins: sc("));
        assert!(replaced.contains("mimeTypes: sc("));
    }

    #[test]
    fn env_template_has_single_seed_placeholder() {
        assert_eq!(
            ENV_TEMPLATE.matches("__SEED_JSON__").count(),
            1,
            "__SEED_JSON__ 必须恰好出现 1 次(在 `var __SEED__ =` 处)"
        );
        let seed = json!({ "navigator": { "userAgent": "UA" }, "location": { "host": "x.com" } });
        let env = build_env_js(&seed);
        assert!(!env.contains("__SEED_JSON__"), "替换后不应再有种子占位符");
        assert!(env.contains("\"userAgent\": \"UA\""));
        assert!(env.contains("function setup("));
        assert!(env.contains("module.exports"));
        assert!(env.contains("function installWebRtc("));
        assert!(env.contains("function installNavigatorPlugins("));
        assert!(env.contains("function arrayLike("));
        assert!(env.contains("function pixelBytes("));
    }

    #[test]
    fn first_http_frame_firefox_and_chrome() {
        let ff = "sign@https://site.com/static/bdms.js:5:120\n@https://site.com/app.js:1:2";
        assert_eq!(
            first_http_frame(ff),
            Some(("https://site.com/static/bdms.js".to_string(), 5, 120))
        );
        let cr = "Error\n    at sign (https://site.com/static/bdms.js:5:120)\n    at h (https://site.com/app.js:1:2)";
        assert_eq!(
            first_http_frame(cr),
            Some(("https://site.com/static/bdms.js".to_string(), 5, 120))
        );
        assert_eq!(first_http_frame("no url here"), None);
    }

    #[test]
    fn parse_signers_aggregates_by_file() {
        let sinks = json!([
            { "type": "xhr", "url": "https://api/detail?a_bogus=1", "stack": "sign@https://site/static/bdms.js:5:1\n@https://site/app.js:1:1" },
            { "type": "xhr", "url": "https://api/detail?a_bogus=2", "stack": "sign@https://site/static/bdms.js:6:1\n@https://site/app.js:1:1" },
            { "type": "fetch", "url": "https://api/related?a_bogus=3", "stack": "w@https://site/app.js:9:1" },
        ]);
        let signers = parse_signers(sinks.as_array().unwrap());
        assert_eq!(signers.len(), 2);
        assert_eq!(signers[0]["url"], json!("https://site/static/bdms.js"));
        assert_eq!(signers[0]["count"], json!(2));
        assert_eq!(signers[1]["url"], json!("https://site/app.js"));
        assert_eq!(signers[1]["count"], json!(1));
    }

    #[test]
    fn prune_keeps_fingerprint_skeleton() {
        let seed = json!({
            "navigator": { "userAgent": "UA", "platform": "MacIntel" },
            "screen": { "width": 1920 },
            "location": { "host": "x.com" },
            "document": { "cookie": "a=1" },
            "windowMetrics": { "devicePixelRatio": 2 },
            "fingerprint": { "canvas": { "supported": true, "dataURL": "data:img" }, "webgl": { "supported": true }, "audio": { "supported": false } }
        });
        let access = json!({ "order": ["navigator.platform"], "count": {} });
        let pruned = prune_seed(&seed, &access);
        assert_eq!(
            pruned.pointer("/fingerprint/canvas/dataURL"),
            Some(&json!("data:img"))
        );
        assert_eq!(
            pruned.pointer("/windowMetrics/devicePixelRatio"),
            Some(&json!(2))
        );
    }

    #[test]
    fn snapshot_includes_fingerprint_when_supported() {
        let seed = json!({
            "navigator": { "userAgent": "UA" },
            "screen": { "width": 1920 },
            "fingerprint": {
                "canvas": { "supported": true }, "webgl": { "supported": true }, "audio": { "supported": true },
                "fonts": { "supported": true }, "canvasPixels": { "supported": true }, "rtc": { "supported": true }
            }
        });
        let js = gen_snapshot_js(&seed);
        assert!(js.contains("function __fpCanvas()"));
        assert!(js.contains("function __fpFonts()"));
        assert!(js.contains("function __fpCanvasPixels()"));
        assert!(js.contains("function __fpRtc()"));
        assert!(js.contains("\"canvas.dataURL\""));
        assert!(js.contains("\"webgl.unmaskedVendor\""));
        assert!(js.contains("\"webgl.extCount\""));
        assert!(js.contains("\"fonts.hash\""));
        assert!(js.contains("\"fonts.count\""));
        assert!(js.contains("\"canvasPixels.hash\""));
        assert!(js.contains("\"rtc.supported\""));
        assert!(js.contains("\"navigator.pluginsCount\""));
        assert!(js.contains("\"navigator.plugins0\""));
        assert!(js.contains("\"navigator.mimeTypesCount\""));
        let seed2 = json!({ "navigator": { "userAgent": "UA" }, "fingerprint": { "canvas": { "supported": false } } });
        let js2 = gen_snapshot_js(&seed2);
        assert!(!js2.contains("\"canvas.dataURL\""));
        assert!(!js2.contains("\"fonts.hash\""));
        assert!(!js2.contains("\"canvasPixels.hash\""));
        assert!(js2.contains("\"rtc.supported\""));
        assert!(js2.contains("\"navigator.pluginsCount\""));
    }

    #[test]
    fn fill_recorded_fp_overwrites_from_seed() {
        let seed = json!({
            "fingerprint": {
                "canvas": { "supported": true, "dataURL": "data:REC" },
                "webgl": { "supported": true, "unmaskedVendor": "Acme", "unmaskedRenderer": "GPU-9", "parameters": { "7936": "Moz", "3379": 16384 }, "extensions": ["A", "B"] },
                "audio": { "supported": true, "sum": 124.0434752751607 }
            }
        });
        let mut browser = json!({ "canvas.dataURL": "data:LIVE", "webgl.unmaskedVendor": "live" });
        fill_recorded_fp(&mut browser, &seed);
        assert_eq!(browser["canvas.dataURL"], json!("data:REC"));
        assert_eq!(browser["webgl.unmaskedVendor"], json!("Acme"));
        assert_eq!(browser["webgl.vendor"], json!("Moz"));
        assert_eq!(browser["webgl.maxTextureSize"], json!(16384));
        assert_eq!(browser["webgl.extCount"], json!(2));
        assert_eq!(browser["audio.sum"], json!(round6(124.0434752751607)));
    }

    #[test]
    fn fill_recorded_fp_fills_new_fingerprints() {
        let seed = json!({
            "navigator": {
                "userAgent": "UA",
                "plugins": [
                    { "name": "PDF Viewer", "filename": "internal-pdf-viewer", "mimeTypes": [{ "type": "application/pdf" }] },
                    { "name": "Chrome PDF Viewer", "filename": "internal-pdf-viewer", "mimeTypes": [] }
                ],
                "mimeTypes": [{ "type": "application/pdf" }, { "type": "text/pdf" }]
            },
            "fingerprint": {
                "fonts": { "supported": true, "hash": "deadbeef", "detected": ["Arial", "Verdana", "Tahoma"] },
                "canvasPixels": { "supported": true, "hash": "cafe1234", "width": 96, "height": 32 },
                "rtc": { "supported": true, "codecsHash": "abcd0001" }
            }
        });
        let mut browser = json!({
            "fonts.hash": "LIVE", "fonts.count": 0, "canvasPixels.hash": "LIVE",
            "rtc.supported": false, "navigator.pluginsCount": 0, "navigator.plugins0": null,
            "navigator.mimeTypesCount": 0
        });
        fill_recorded_fp(&mut browser, &seed);
        assert_eq!(browser["fonts.hash"], json!("deadbeef"));
        assert_eq!(browser["fonts.count"], json!(3));
        assert_eq!(browser["canvasPixels.hash"], json!("cafe1234"));
        assert_eq!(browser["rtc.supported"], json!(true));
        assert_eq!(browser["navigator.pluginsCount"], json!(2));
        assert_eq!(browser["navigator.plugins0"], json!("PDF Viewer"));
        assert_eq!(browser["navigator.mimeTypesCount"], json!(2));
    }

    #[test]
    fn fill_recorded_fp_handles_missing_new_fingerprints() {
        let seed = json!({ "navigator": { "userAgent": "UA" }, "fingerprint": {} });
        let mut browser = json!({ "rtc.supported": true });
        fill_recorded_fp(&mut browser, &seed);
        assert_eq!(browser["rtc.supported"], json!(false));
        assert_eq!(browser["navigator.pluginsCount"], json!(0));
        assert_eq!(browser["navigator.plugins0"], Value::Null);
        assert!(browser.get("fonts.hash").is_none());
        assert!(browser.get("canvasPixels.hash").is_none());
    }

    #[test]
    fn target_to_json() {
        assert_eq!(
            EnvTarget::Query("a_bogus".into()).to_json(),
            json!({ "kind": "query", "key": "a_bogus" })
        );
        assert_eq!(
            EnvTarget::Header("x-bogus".into()).to_json(),
            json!({ "kind": "header", "key": "x-bogus" })
        );
        assert_eq!(EnvTarget::Cookie("msToken".into()).key(), "msToken");
    }

    #[test]
    fn set_and_get_path() {
        let mut o = json!({});
        set_path(&mut o, &["navigator", "userAgent"], json!("UA"));
        set_path(
            &mut o,
            &["navigator", "userAgentData", "platform"],
            json!("macOS"),
        );
        assert_eq!(
            get_path(&o, &["navigator", "userAgent"]),
            Some(&json!("UA"))
        );
        assert_eq!(
            get_path(&o, &["navigator", "userAgentData", "platform"]),
            Some(&json!("macOS"))
        );
        assert_eq!(get_path(&o, &["missing"]), None);
    }

    #[test]
    fn prune_keeps_accessed_and_skeleton() {
        let seed = json!({
            "navigator": { "userAgent": "UA", "platform": "MacIntel", "vendor": "Google", "hardwareConcurrency": 10 },
            "screen": { "width": 1920, "height": 1080 },
            "location": { "host": "x.com", "origin": "https://x.com" },
            "document": { "cookie": "a=1", "title": "T" },
            "localStorage": { "k": "v" }
        });
        let access = json!({
            "order": ["navigator.platform", "navigator.hardwareConcurrency", "screen.width"],
            "count": {}
        });
        let pruned = prune_seed(&seed, &access);
        assert_eq!(
            pruned.pointer("/navigator/platform"),
            Some(&json!("MacIntel"))
        );
        assert_eq!(
            pruned.pointer("/navigator/hardwareConcurrency"),
            Some(&json!(10))
        );
        assert_eq!(pruned.pointer("/screen/width"), Some(&json!(1920)));
        assert!(pruned.get("localStorage").is_none());
        assert!(pruned.pointer("/screen/height").is_none());
        assert_eq!(pruned.pointer("/navigator/userAgent"), Some(&json!("UA")));
        assert_eq!(pruned.pointer("/location/host"), Some(&json!("x.com")));
        assert_eq!(pruned.pointer("/document/cookie"), Some(&json!("a=1")));
    }

    #[test]
    fn snapshot_covers_scalars() {
        let seed = json!({
            "navigator": { "userAgent": "UA", "languages": ["zh-CN", "en"], "userAgentData": { "mobile": false } },
            "screen": { "width": 1920 }
        });
        let js = gen_snapshot_js(&seed);
        assert!(js.contains("\"navigator.userAgent\""));
        assert!(js.contains("(navigator.languages || []).join"));
        assert!(!js.contains("\"navigator.userAgentData\""));
        assert!(js.contains("\"screen.width\""));
        assert!(js.contains("\"location.host\""));
    }

    #[test]
    fn compare_counts() {
        let b = json!({ "navigator.userAgent": "UA", "screen.width": 1920 });
        let n = json!({ "navigator.userAgent": "UA", "screen.width": 1366 });
        let r = compare(&b, &n);
        assert_eq!(r["pass"], 1);
        assert_eq!(r["fail"], 1);
        assert_eq!(r["total"], 2);
    }
}
