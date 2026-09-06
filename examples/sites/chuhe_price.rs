//! 案例：用同一套 `Browser` / `Tab` API 听初和站报价。
//!
//! 这里不在页面里 `fetch`。库的职责是开浏览器、点页面、听 CDP 网络包；
//! 带 JiaMi 签名的请求由**站点自己**发出。换引擎只改一行：
//! `"engine": "chrome"` 或 `"engine": "lightpanda"`。
//!
//! ```bash
//! # 开发
//! cp examples/sites/chuhe.config.json examples/sites/chuhe.config.local.json
//! cargo run --example chuhe_price --features cdp,ocr
//!
//! # 自己跑（dist 里已内置 Lightpanda）
//! ./scripts/pack-chuhe-dist.sh
//! cd dist/chuhe && ./chuhe_price
//! ```
//!
//! 默认走框架内置的 Lightpanda。登录验证码走 `tab.ocr_code`（算数式先求值）。
//! 登录态用 `save_storage_state` 写在 `profile_dir/storage.json`，过期才再登。
//! 听完目录/详情就写 `data/chuhe/*.csv` 然后退出，不用空等。

use std::collections::{HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use drission::cdp::{OriginStorage, StorageState};
use drission::net::DataPacket;
use drission::prelude::*;
use serde_json::{Value, json};

const URL: &str = "https://chuhe.lk888.ai/apidoc#common/list_models";
const LISTEN: &[&str] = &["lingkeai.ai", "jiageliebiao", "moxingxiangqing"];

const BTN_START: &str = r#"xpath://*[self::button or self::a or self::span][contains(normalize-space(.),"开始使用")]"#;
const TAB_PASSWORD: &str = r#"xpath://*[self::button or self::a or self::span][contains(normalize-space(.),"密码登录")]"#;
const INPUT_USER: &str = r#"css:input[placeholder*="手机号或邮箱"]"#;
const INPUT_PASS: &str = "css:input[type=password]";
const BOX_AGREE: &str = "css:input.checkbox-input";
const BTN_LOGIN: &str = r#"xpath://button[normalize-space()="立即登录"]"#;
const BTN_PRICE: &str = r#"xpath://button[contains(normalize-space(.),"模型价格")]"#;
const INPUT_SEARCH: &str = r#"css:input[placeholder*="搜索模型名称"]"#;
const INPUT_CAPTCHA: &str = r#"css:input[placeholder*="图中"]"#;
const CAPTCHA_IMGS: &[&str] = &[
    r#"xpath://input[contains(@placeholder,"图中")]/following::img[1]"#,
    r#"xpath://input[contains(@placeholder,"算式")]/following::img[1]"#,
    r#"css:img[src*="yanzheng"]"#,
    r#"css:img[src*="captcha"]"#,
    r#"css:img[src*="verify"]"#,
    r#"css:img[src*="code"]"#,
];

#[derive(Debug, Clone)]
struct Cfg {
    url: String,
    username: String,
    password: String,
    engine: String,
    webhook: String,
    token: String,
    seconds: u64,
    reload_secs: u64,
    profile_dir: PathBuf,
}

impl Default for Cfg {
    fn default() -> Self {
        Self {
            url: URL.into(),
            username: String::new(),
            password: String::new(),
            engine: "lightpanda".into(),
            webhook: String::new(),
            token: String::new(),
            seconds: 0,
            reload_secs: 30,
            profile_dir: PathBuf::from("target/chuhe-profile"),
        }
    }
}

#[tokio::main]
async fn main() -> drission::Result<()> {
    let cfg = load_cfg();
    println!("[*] {}", cfg.engine);
    println!("[*] {}", cfg.url);
    if is_lightpanda(&cfg.engine) {
        if let Ok(p) = drission::cdp::locate_lightpanda() {
            println!("[*] lightpanda {}", p.display());
        } else {
            println!("[*] lightpanda （启动时自动定位 / 下载）");
        }
    }

    let browser = launch(&cfg).await?;
    let tab = browser.new_tab(None).await?;
    tab.listen().start(LISTEN).await?;

    if std::env::var("CHUHE_FORCE_LOGIN").is_ok() {
        println!("[*] CHUHE_FORCE_LOGIN：不复用登录态，走验证码登录");
        let home = cfg
            .url
            .split("/apidoc")
            .next()
            .unwrap_or("https://chuhe.lk888.ai")
            .trim_end_matches('/');
        let _ = tab.get(&format!("{home}/home")).await;
        let _ = tab.wait_ele(BTN_START, Some(Duration::from_secs(8))).await;
    } else {
        restore_session(&tab, &cfg).await;
    }
    login_if_needed(&tab, &cfg).await;
    if has_ele(&tab, BTN_PRICE).await {
        persist_session(&tab, &cfg).await;
    } else if tab.url().await.unwrap_or_default().contains("/zh-CN") {
        tab.get(&cfg.url).await.ok();
    }
    open_price_panel(&tab).await;

    let out_dir = work_dir().join("data/chuhe");
    fs::create_dir_all(&out_dir)?;
    let jsonl = work_dir().join("data/requests/chuhe.jsonl");
    fs::create_dir_all(jsonl.parent().unwrap())?;
    let mut seen = load_seen(&jsonl);
    let mut models: HashMap<u64, ModelRow> = HashMap::new();
    let mut prices: HashMap<(String, String), Quote> = HashMap::new();
    let webhook = if cfg.webhook.is_empty() {
        None
    } else {
        Some(reqwest::Client::new())
    };

    let once = cfg.seconds == 0;
    let max_secs = if once { 20 } else { cfg.seconds.max(8) };
    println!(
        "[*] {}（CSV → {}）",
        if once { "拿到报价就停" } else { "持续听价" },
        out_dir.display()
    );
    let deadline = Instant::now() + Duration::from_secs(max_secs);
    let mut packets = 0usize;

    while Instant::now() < deadline {
        match tab.listen().wait(Some(Duration::from_millis(400))).await? {
            Some(pkt) => {
                packets += 1;
                persist_packet(&jsonl, &mut seen, &pkt)?;
                ingest(&pkt, &mut models, &mut prices, &webhook, &cfg.webhook).await;
                if once && !prices.is_empty() {
                    while let Some(more) =
                        tab.listen().wait(Some(Duration::from_millis(250))).await?
                    {
                        packets += 1;
                        persist_packet(&jsonl, &mut seen, &more)?;
                        ingest(&more, &mut models, &mut prices, &webhook, &cfg.webhook).await;
                    }
                    break;
                }
            }
            None if once && !prices.is_empty() => break,
            None if !once && Instant::now() >= deadline => break,
            None => {}
        }
    }

    let models_csv = out_dir.join("models.csv");
    let prices_csv = out_dir.join("prices.csv");
    write_models_csv(&models_csv, &models)?;
    write_prices_csv(&prices_csv, &prices)?;
    tab.listen().stop().await.ok();
    browser.quit().await?;
    println!(
        "==== packets={packets} models={} prices={} ====",
        models.len(),
        prices.len()
    );
    println!("[*] {}", models_csv.display());
    println!("[*] {}", prices_csv.display());
    Ok(())
}

async fn launch(cfg: &Cfg) -> drission::Result<ChromiumBrowser> {
    let _ = fs::create_dir_all(&cfg.profile_dir);
    let mut opts = ChromiumOptions::new().user_data_dir(&cfg.profile_dir);
    if is_lightpanda(&cfg.engine) {
        opts = opts.lightpanda();
        if let Some(bin) = sidecar_browser() {
            opts = opts.binary_path(bin);
        }
    } else {
        let headed = !matches!(std::env::var("HL").ok().as_deref(), Some("1") | Some("true"));
        opts = opts.headless(!headed).window_size(1280, 800);
    }
    if let Ok(url) = std::env::var("CONNECT") {
        println!("[*] connect {url}");
        ChromiumBrowser::connect_with(&url, opts).await
    } else {
        ChromiumBrowser::launch(opts).await
    }
}

async fn has_ele(tab: &ChromiumTab, sel: &str) -> bool {
    tab.ele(sel).await.is_ok()
}

fn state_path(cfg: &Cfg) -> PathBuf {
    cfg.profile_dir.join("storage.json")
}

async fn restore_session(tab: &ChromiumTab, cfg: &Cfg) {
    let _ = tab.get(&cfg.url).await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    let path = state_path(cfg);
    if path.is_file() {
        match tab.load_storage_state(&path.to_string_lossy()).await {
            Ok(()) => {
                println!("[*] 已恢复 {}", path.display());
                let _ = tab.get(&cfg.url).await;
                tokio::time::sleep(Duration::from_millis(800)).await;
            }
            Err(e) => eprintln!("[!] load_storage_state {e}"),
        }
        return;
    }
    if cfg.token.is_empty() {
        return;
    }
    let st = StorageState {
        cookies: Vec::new(),
        origins: vec![OriginStorage {
            origin: "https://chuhe.lk888.ai".into(),
            local_storage: vec![("token".into(), cfg.token.clone())],
            session_storage: Vec::new(),
        }],
    };
    match tab.apply_storage_state(&st).await {
        Ok(()) => {
            println!("[*] 已灌回 token，刷新后看登录还在不在");
            let _ = tab.get(&cfg.url).await;
            tokio::time::sleep(Duration::from_millis(800)).await;
        }
        Err(e) => eprintln!("[!] apply_storage_state {e}"),
    }
}

async fn persist_session(tab: &ChromiumTab, cfg: &Cfg) {
    let path = state_path(cfg);
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    match tab.save_storage_state(&path.to_string_lossy()).await {
        Ok(()) => println!("[*] 登录态已存 {}", path.display()),
        Err(e) => eprintln!("[!] save_storage_state {e}"),
    }
}

/// 文档页已经能点「模型价格」就当登录还在；否则填表 + OCR 算数验证码。
async fn login_if_needed(tab: &ChromiumTab, cfg: &Cfg) {
    if has_ele(tab, BTN_PRICE).await && std::env::var("CHUHE_FORCE_LOGIN").is_err() {
        println!("[*] 登录还在，跳过登录框");
        return;
    }
    if cfg.username.is_empty() || cfg.password.is_empty() {
        println!("[*] 未填账号，且当前没登录");
        return;
    }
    if !has_ele(tab, INPUT_PASS).await && !has_ele(tab, INPUT_CAPTCHA).await {
        let _ = tab.wait_ele(BTN_START, Some(Duration::from_secs(6))).await;
        for gate in [
            BTN_START,
            r#"xpath://*[self::button or self::a][normalize-space()="登录"]"#,
            r#"xpath://*[self::button or self::a][contains(normalize-space(.),"注册/登录")]"#,
        ] {
            click_login_gate(tab, gate).await;
            if tab
                .wait_ele(INPUT_CAPTCHA, Some(Duration::from_secs(4)))
                .await
                .is_ok()
            {
                break;
            }
        }
    }
    click_login_gate(tab, TAB_PASSWORD).await;
    let _ = tab.wait_ele(INPUT_PASS, Some(Duration::from_secs(4))).await;
    dump_captcha_candidates(tab).await;
    if !has_ele(tab, INPUT_PASS).await {
        println!(
            "[*] 没有密码框 title={:?} url={:?}",
            tab.title().await.ok(),
            tab.url().await.ok()
        );
        return;
    }
    for attempt in 1..=5 {
        if tab.input(INPUT_USER, &cfg.username).await.is_err() {
            eprintln!("[!] 找不到账号框");
            return;
        }
        if tab.input(INPUT_PASS, &cfg.password).await.is_err() {
            eprintln!("[!] 找不到密码框");
            return;
        }
        ensure_agree(tab).await;
        if !fill_captcha(tab, attempt).await {
            if let Some(img) = find_captcha_img(tab).await {
                let _ = tab.click(img).await;
            }
            tokio::time::sleep(Duration::from_millis(600)).await;
            continue;
        }
        if let Err(e) = tab.click(BTN_LOGIN).await {
            eprintln!("[!] 点立即登录失败: {e}");
            return;
        }
        println!("[*] 已提交登录 #{attempt}");
        tokio::time::sleep(Duration::from_secs(1)).await;
        if has_login_token(tab).await {
            persist_session(tab, cfg).await;
            let _ = tab.get(&cfg.url).await;
            let _ = tab.wait_ele(BTN_PRICE, Some(Duration::from_secs(8))).await;
            return;
        }
        println!("[*] 登录未成功（验证码多半认错），换一张再试");
        if let Some(img) = find_captcha_img(tab).await {
            let _ = tab.click(img).await;
        }
        tokio::time::sleep(Duration::from_millis(600)).await;
    }
}

async fn dump_captcha_candidates(tab: &ChromiumTab) {
    if std::env::var("CHUHE_DUMP_CAPTCHA").is_err() {
        return;
    }
    if let Ok(v) = tab
        .run_js(
            "(()=>{const a=[...document.querySelectorAll('input,textarea')].map(e=>({t:e.type,p:e.placeholder||'',c:e.className}));return JSON.stringify({n:a.length,a:a.slice(0,20),frames:frames.length,text:(document.body&&document.body.innerText||'').slice(0,240)});})()",
        )
        .await
    {
        println!("[*] DOM {v}");
    }
    if let Ok(inputs) = tab.eles("tag:input").await {
        println!("[*] 页上 input ×{}", inputs.len());
        for (i, el) in inputs.iter().enumerate().take(16) {
            let ph = el.attr("placeholder").await.ok().flatten().unwrap_or_default();
            let ty = el.attr("type").await.ok().flatten().unwrap_or_default();
            println!("    input[{i}] type={ty:?} placeholder={ph:?}");
        }
    }
    if let Ok(imgs) = tab.eles("tag:img").await {
        println!("[*] 页上 img ×{}", imgs.len());
        for (i, img) in imgs.iter().enumerate().take(20) {
            let src = img.attr("src").await.ok().flatten().unwrap_or_default();
            let alt = img.attr("alt").await.ok().flatten().unwrap_or_default();
            println!("    img[{i}] alt={alt:?} src={src}");
        }
    }
    if let Some(sel) = find_captcha_img(tab).await
        && let Ok(el) = tab.ele(sel).await
        && let Ok(bytes) = el.image_bytes().await
    {
        let path = work_dir().join("data/chuhe/captcha.png");
        let _ = fs::create_dir_all(path.parent().unwrap());
        let _ = fs::write(&path, &bytes);
        println!("[*] 验证码图 {} ({}B)", path.display(), bytes.len());
    }
}

async fn ensure_agree(tab: &ChromiumTab) {
    if let Ok(el) = tab.ele(BOX_AGREE).await {
        let _ = el
            .run_js("if(this.checked===false){ this.click(); this.checked=true; }")
            .await;
    }
}

async fn has_login_token(tab: &ChromiumTab) -> bool {
    match tab.run_js("localStorage.getItem('token')||''").await {
        Ok(v) => v.as_str().is_some_and(|s| s.len() >= 16),
        Err(_) => false,
    }
}

async fn find_captcha_img(tab: &ChromiumTab) -> Option<&'static str> {
    for sel in CAPTCHA_IMGS {
        if has_ele(tab, sel).await {
            return Some(*sel);
        }
    }
    None
}

async fn fill_captcha(tab: &ChromiumTab, attempt: u32) -> bool {
    let Some(sel) = find_captcha_img(tab).await else {
        if has_ele(tab, INPUT_CAPTCHA).await {
            eprintln!("[!] 有验证码框但找不到图");
            return false;
        }
        println!("[*] 没有验证码图，直接登");
        return true;
    };
    #[cfg(feature = "ocr")]
    {
        match tab.ocr_code(sel).await {
            Ok(ans) => {
                println!("[*] OCR#{attempt} {sel} → {ans}");
                if let Ok(el) = tab.ele(sel).await
                    && let Ok(bytes) = el.image_bytes().await
                    && std::env::var("CHUHE_DUMP_CAPTCHA").is_ok()
                {
                    let path = work_dir().join("data/chuhe/captcha.png");
                    let _ = fs::create_dir_all(path.parent().unwrap());
                    let _ = fs::write(&path, bytes);
                    println!("[*] 验证码图 {}", path.display());
                }
                if let Ok(box_) = tab.ele(INPUT_CAPTCHA).await {
                    let _ = box_.clear().await;
                    if box_.input(&ans).await.is_ok() {
                        return true;
                    }
                }
                eprintln!("[!] 找不到验证码输入框");
                false
            }
            Err(e) => {
                eprintln!("[!] OCR#{attempt} {e}");
                false
            }
        }
    }
    #[cfg(not(feature = "ocr"))]
    {
        let _ = (tab, attempt, sel);
        eprintln!("[!] 本示例要用算数验证码，请加 --features ocr 再编");
        false
    }
}

/// 点文档页右上角「模型价格」，搜一条再点列表行，逼出详情接口。
async fn open_price_panel(tab: &ChromiumTab) {
    if tab.ele(BTN_PRICE).await.is_err() {
        println!(
            "[*] 还没到文档页 title={:?} url={:?}",
            tab.title().await.ok(),
            tab.url().await.ok()
        );
        return;
    }
    if let Err(e) = tab.click(BTN_PRICE).await {
        eprintln!("[!] 点模型价格: {e}");
        return;
    }
    println!("[*] 已点模型价格");
    if tab
        .wait_ele(INPUT_SEARCH, Some(Duration::from_secs(6)))
        .await
        .is_err()
    {
        let _ = tab.click(BTN_PRICE).await;
        if tab
            .wait_ele(INPUT_SEARCH, Some(Duration::from_secs(4)))
            .await
            .is_err()
        {
            println!("[*] 价格面板没打开（搜索框不在）");
            return;
        }
    }
    println!("[*] 价格面板已打开");
    tokio::time::sleep(Duration::from_millis(1200)).await;
    for name in ["Omni 1.1", "GEM 3.7", "TT Image 2"] {
        if let Ok(search) = tab.ele(INPUT_SEARCH).await {
            let _ = search.clear().await;
            if search.input(name).await.is_ok() {
                println!("[*] input {name}");
            }
        }
        tokio::time::sleep(Duration::from_millis(800)).await;
        click_model_row(tab, name).await;
        tokio::time::sleep(Duration::from_millis(800)).await;
    }
}

/// 在价格面板里点最短的那条展示名（避开文档正文里同名大段文字）。
async fn click_model_row(tab: &ChromiumTab, name: &str) {
    let sel = format!("@text():{name}");
    let Ok(els) = tab.eles(&sel).await else {
        return;
    };
    let mut best: Option<(usize, ChromiumElement)> = None;
    for el in els {
        let n = el.text().await.unwrap_or_default().chars().count();
        if n == 0 || n > 80 {
            continue;
        }
        if best.as_ref().is_none_or(|(cur, _)| n < *cur) {
            best = Some((n, el));
        }
    }
    let Some((n, el)) = best else {
        println!("[*] 列表里没有 {name}");
        return;
    };
    match el.click().await {
        Ok(()) => println!("[*] click row {name} ({n}字)"),
        Err(e) => eprintln!("[!] click row {name}: {e}"),
    }
}

/// Lightpanda 的鼠标点击经常打不穿 Vue，登录入口再用元素自己的 `click()`。
async fn click_login_gate(tab: &ChromiumTab, sel: &str) {
    let Ok(el) = tab.ele(sel).await else {
        println!("[*] 没找到 {sel}");
        return;
    };
    if std::env::var("CHUHE_DUMP_CAPTCHA").is_ok() {
        if let Ok(info) = el
            .run_js("return JSON.stringify({tag:this.tagName,href:this.getAttribute('href'),cls:this.className,t:(this.innerText||'').slice(0,40)});")
            .await
        {
            println!("[*] gate {sel} {info}");
        }
    }
    let _ = el.click().await;
    let _ = el
        .run_js(
            "this.click(); var a=this.closest('a')||this; if(a.href) location.assign(a.href);",
        )
        .await;
    println!("[*] click {sel}");
}

async fn ingest(
    pkt: &DataPacket,
    models: &mut HashMap<u64, ModelRow>,
    prices: &mut HashMap<(String, String), Quote>,
    http: &Option<reqwest::Client>,
    webhook: &str,
) {
    let json = pkt.json().unwrap_or(Value::Null);
    let added_models = extract_models(&json);
    let added_n = added_models.len();
    for row in added_models {
        models.insert(row.id, row);
    }
    let quotes = extract_quotes(&json);
    if quotes.is_empty() && added_n == 0 {
        println!(
            "[.] {} {} {} ({}B)",
            pkt.method,
            pkt.response.status,
            pkt.url.split('?').next().unwrap_or(&pkt.url),
            pkt.response.body.len()
        );
        return;
    }
    if added_n > 0 {
        println!("[.] 目录 +{added_n} 累计 {}", models.len());
    }
    for q in quotes {
        let key = (q.model.clone(), q.field.clone());
        if prices.get(&key).is_some_and(|old| (old.price - q.price).abs() < 1e-9) {
            continue;
        }
        println!("[+] {} {} = {}", q.model, q.field, q.price);
        if let (Some(client), true) = (http.as_ref(), !webhook.is_empty()) {
            let tick = json!({
                "model": q.model,
                "field": q.field,
                "price": q.price,
                "url": pkt.url,
            });
            if let Err(e) = client.post(webhook).json(&tick).send().await {
                eprintln!("[!] webhook {e}");
            }
        }
        prices.insert(key, q);
    }
}

fn is_lightpanda(engine: &str) -> bool {
    matches!(engine.to_ascii_lowercase().as_str(), "lightpanda" | "lp" | "panda")
}

fn load_cfg() -> Cfg {
    let mut cfg = Cfg::default();
    let path = config_path();
    if let Some(path) = path {
        println!("[*] {}", path.display());
        if let Ok(raw) = fs::read_to_string(&path)
            && let Ok(v) = serde_json::from_str::<Value>(&raw)
        {
            take_str(&v, "url", &mut cfg.url);
            take_str(&v, "username", &mut cfg.username);
            take_str(&v, "password", &mut cfg.password);
            take_str(&v, "engine", &mut cfg.engine);
            take_str(&v, "webhook", &mut cfg.webhook);
            take_str(&v, "token", &mut cfg.token);
            if let Some(n) = v.get("seconds").and_then(Value::as_u64) {
                cfg.seconds = n;
            }
            if let Some(n) = v.get("reload_secs").and_then(Value::as_u64) {
                cfg.reload_secs = n;
            }
            if let Some(p) = v.get("profile_dir").and_then(Value::as_str).filter(|s| !s.is_empty()) {
                cfg.profile_dir = PathBuf::from(p);
            }
        }
    }
    if let Ok(x) = std::env::var("CHUHE_URL") {
        cfg.url = x;
    }
    if let Ok(x) = std::env::var("CHUHE_USER") {
        cfg.username = x;
    }
    if let Ok(x) = std::env::var("CHUHE_PASS") {
        cfg.password = x;
    }
    if let Ok(x) = std::env::var("CHUHE_ENGINE") {
        cfg.engine = x;
    }
    if let Ok(x) = std::env::var("CHUHE_WEBHOOK") {
        cfg.webhook = x;
    }
    if let Ok(x) = std::env::var("CHUHE_SECONDS")
        && let Ok(n) = x.parse()
    {
        cfg.seconds = n;
    }
    cfg.username = cfg.username.trim().trim_end_matches(['·', '.', '。']).into();
    if cfg.profile_dir.is_relative() {
        cfg.profile_dir = work_dir().join(&cfg.profile_dir);
    }
    cfg
}

fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn sidecar_browser() -> Option<PathBuf> {
    let dir = exe_dir();
    let name = if cfg!(windows) {
        "lightpanda.exe"
    } else {
        "lightpanda"
    };
    [dir.join(name), dir.join("bin").join(name)]
        .into_iter()
        .find(|p| p.is_file())
}

/// dist 打包目录（旁边有 lightpanda）优先；否则当前工作目录（cargo run 写仓库 `data/`）。
fn work_dir() -> PathBuf {
    if sidecar_browser().is_some() {
        exe_dir()
    } else {
        PathBuf::from(".")
    }
}

fn config_path() -> Option<PathBuf> {
    let mut v = Vec::new();
    if let Ok(p) = std::env::var("CHUHE_CONFIG") {
        v.push(PathBuf::from(p));
    }
    let here = exe_dir();
    v.push(here.join("chuhe.config.local.json"));
    v.push(here.join("chuhe.config.json"));
    v.push(PathBuf::from("chuhe.config.local.json"));
    v.push(PathBuf::from("chuhe.config.json"));
    if let Ok(root) = std::env::var("CARGO_MANIFEST_DIR") {
        let root = PathBuf::from(root);
        v.push(root.join("examples/sites/chuhe.config.local.json"));
        v.push(root.join("examples/sites/chuhe.config.json"));
    }
    v.push(PathBuf::from("examples/sites/chuhe.config.local.json"));
    v.into_iter().find(|p| p.is_file())
}

fn take_str(v: &Value, key: &str, dest: &mut String) {
    if let Some(s) = v.get(key).and_then(Value::as_str) {
        *dest = s.to_string();
    }
}

#[derive(Debug, Clone)]
struct ModelRow {
    id: u64,
    display: String,
    name: String,
    kind: String,
}

#[derive(Debug, Clone)]
struct Quote {
    model: String,
    field: String,
    price: f64,
}

fn extract_models(v: &Value) -> Vec<ModelRow> {
    let mut rows = Vec::new();
    if let Some(list) = v.pointer("/data/list").and_then(Value::as_array) {
        rows.extend(list.iter().filter_map(model_from_row));
    }
    if let Some(map) = v.get("data").and_then(Value::as_object) {
        for (kind, val) in map {
            if let Some(list) = val.as_array() {
                for row in list {
                    if let Some(mut m) = model_from_row(row) {
                        if m.kind.is_empty() {
                            m.kind = kind.clone();
                        }
                        rows.push(m);
                    }
                }
            }
        }
    }
    rows
}

fn model_from_row(row: &Value) -> Option<ModelRow> {
    let id = row.get("id").and_then(Value::as_u64)?;
    let display = ["展示名称", "zhanshi_mingcheng", "display_name"]
        .into_iter()
        .find_map(|k| row.get(k).and_then(Value::as_str))
        .unwrap_or("")
        .to_string();
    let name = ["模型名称", "mingcheng", "name"]
        .into_iter()
        .find_map(|k| row.get(k).and_then(Value::as_str))
        .unwrap_or("")
        .to_string();
    let kind = ["模型类型", "leixing", "type"]
        .into_iter()
        .find_map(|k| row.get(k).and_then(Value::as_str))
        .unwrap_or("")
        .to_string();
    Some(ModelRow {
        id,
        display,
        name,
        kind,
    })
}

fn extract_quotes(v: &Value) -> Vec<Quote> {
    let mut out = Vec::new();
    walk(v, None, &mut out);
    out
}

fn walk(v: &Value, hint: Option<&str>, out: &mut Vec<Quote>) {
    match v {
        Value::Array(items) => items.iter().for_each(|i| walk(i, hint, out)),
        Value::Object(map) => {
            let model = ["展示名称", "模型名称", "分组名称", "name"]
                .into_iter()
                .find_map(|k| map.get(k).and_then(Value::as_str))
                .map(str::to_string)
                .or_else(|| hint.map(str::to_string));
            let option = map.get("选项名称").and_then(Value::as_str);
            for (k, val) in map {
                if let Some(n) = numberish(val) {
                    if n != 0.0 && is_price_key(k) {
                        let field = match option {
                            Some(opt) => format!("{k}:{opt}"),
                            None => k.clone(),
                        };
                        out.push(Quote {
                            model: model.clone().unwrap_or_else(|| "_".into()),
                            field,
                            price: n,
                        });
                    }
                } else if val.is_object() || val.is_array() {
                    walk(val, model.as_deref(), out);
                }
            }
        }
        _ => {}
    }
}

fn is_price_key(k: &str) -> bool {
    (k.contains("价格") || k.contains("单价"))
        && !k.contains("系数")
        && !k.contains("加成")
        && !k.contains("影响")
}

fn numberish(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn csv_cell(s: &str) -> String {
    if s.contains([',', '"', '\n']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

fn write_models_csv(path: &Path, models: &HashMap<u64, ModelRow>) -> std::io::Result<()> {
    let mut rows: Vec<_> = models.values().cloned().collect();
    rows.sort_by_key(|r| r.id);
    let mut f = fs::File::create(path)?;
    writeln!(f, "id,display,name,kind")?;
    for r in rows {
        writeln!(
            f,
            "{},{},{},{}",
            r.id,
            csv_cell(&r.display),
            csv_cell(&r.name),
            csv_cell(&r.kind)
        )?;
    }
    Ok(())
}

fn write_prices_csv(path: &Path, prices: &HashMap<(String, String), Quote>) -> std::io::Result<()> {
    let mut rows: Vec<_> = prices.values().cloned().collect();
    rows.sort_by(|a, b| a.model.cmp(&b.model).then(a.field.cmp(&b.field)));
    let mut f = fs::File::create(path)?;
    writeln!(f, "model,field,price")?;
    for q in rows {
        writeln!(f, "{},{},{}", csv_cell(&q.model), csv_cell(&q.field), q.price)?;
    }
    Ok(())
}

fn persist_packet(path: &Path, seen: &mut HashSet<u64>, pkt: &DataPacket) -> std::io::Result<()> {
    let body = &pkt.response.body;
    if body.trim_start().starts_with("<!") {
        return Ok(());
    }
    let mut h = std::collections::hash_map::DefaultHasher::new();
    pkt.method.hash(&mut h);
    pkt.url.hash(&mut h);
    body.hash(&mut h);
    let fp = h.finish();
    if !seen.insert(fp) {
        return Ok(());
    }
    let rec = json!({
        "ts": now_ms(),
        "method": pkt.method,
        "url": pkt.url,
        "status": pkt.response.status,
        "body": if body.len() > 200_000 { Value::Null } else { Value::String(body.clone()) },
        "fp": fp,
    });
    let mut f = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(f, "{}", serde_json::to_string(&rec).unwrap_or_default())?;
    Ok(())
}

fn load_seen(path: &Path) -> HashSet<u64> {
    let Ok(text) = fs::read_to_string(path) else {
        return HashSet::new();
    };
    text.lines()
        .filter_map(|ln| serde_json::from_str::<Value>(ln).ok())
        .filter_map(|v| v.get("fp").and_then(Value::as_u64))
        .collect()
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}
