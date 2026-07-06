//! YouTube `n` 参数解扰:用 ① 断点把「藏在模块闭包里的解扰器」抠成可复用 oracle(接 re_yt_sig 之后)。
//!
//! 现代 base.js(2024+)把 nsig 解扰**藏进了 URL 类**:`cs6` 里 `(new g.g7(m,!0)).get("n")` 返回的就是
//! **已解扰**的 n(第二参 `!0`=true 开启解扰),真正的变换是 opcode 派发的 VM(`pV(8,6793,this)`…),
//! 不是老式独立函数 → 纯静态正则抠不出干净件。故走**运行时抠取**。
//!
//! 踩坑实录(本版逐一已解):
//!   1) **硬编码行号会过期**:base.js 每隔几小时轮换、行号跟着变 → 每次运行 ② grep 活脚本重新定位。
//!   2) **不能硬编码混淆名**:解扰类(`g.g7`)、命名空间别名(`g`)、入参名(`m`)都随版本轮换 → 全部
//!      **从活源码就地解析**。
//!   3) **`scripts()` 会污染调试器**:定位用的 `tab.scripts()` 内部 `list()` 为防反调试卡收集会调
//!      `setSkipAllPauses(true)` —— 它让调试器**忽略一切暂停,连我们的断点也不触发**。故下断点前必须
//!      `set_skip_all_pauses(false)` 复位成「干净环境」。
//!   4) **断 cs6 函数体多半命不中**:cs6 只处理 `/n/` 路径形态的 URL(`m.match(/\/n\/.../)`),很多视频
//!      走 `?n=` 查询形态、根本不调 cs6 → 函数体断点永远等不到。**正解:断在 `cs6=function` 的赋值处
//!      (列 0)** —— base.js 一被求值就执行、**必命中**,且此刻**模块命名空间对象 `g` 已存在**(只是
//!      `g.g7` 还没挂上)。抓住 `g` 的引用到全局,resume 后 base.js 继续求值会把 `g7` 挂到**同一个 `g`
//!      对象**上 → `window.__ns.g7` 即解扰类,oracle 随处可调。这招绕开了「cs6 不被调用」的死结。
//!
//! 流程:
//!   1) ② grep 活的 base.js 唯一定位 `(new …,!0)).get("n")`,抠出 RAW 行 + 解扰类路径/别名/入参名;
//!   2) ① 在(行, 列0=赋值处)下断点 → reload 触发 base.js 重新求值 → 必命中;
//!   3) 命中帧里把模块命名空间对象 `g` 抓到 `window.__ytNsigNs`;resume;
//!   4) 等 base.js 求值完(`g7` 已挂到 `__ytNsigNs`)→ 干净上下文挂 `window.__ytNsig(u)` oracle;
//!   5) dump 解扰类源码 + opcode 派发器到 /tmp(供精读 VM);
//!   6) 从 `ytInitialPlayerResponse` 取真实乱序 n,oracle 解扰两次验真:确定性 + 乱序≠解扰。
//!
//! 运行:`cargo run --example re_yt_sig2 --features cdp`(`HL=0` 有头;`URL=` 换视频)。

use std::time::Duration;

use drission::prelude::*;

#[tokio::main]
async fn main() -> drission::Result<()> {
    let headless = std::env::var("HL").map(|v| v != "0").unwrap_or(true);
    let url = std::env::var("URL")
        .unwrap_or_else(|_| "https://www.youtube.com/watch?v=7349tcyyE-c".to_string());

    let browser = ChromiumBrowser::launch(ChromiumOptions::new().headless(headless)).await?;
    let tab = browser.new_tab(Some("about:blank")).await?;
    tab.get(&url).await?;
    println!("[*] title={:?}", tab.title().await.unwrap_or_default());
    tokio::time::sleep(Duration::from_secs(6)).await; // 等 base.js 解析

    // ② 同一次运行内 grep 活的 base.js,唯一定位 cs6 的解扰调用并抠出名字(不硬编码行号/混淆名)。
    let sc = tab.scripts();
    let Some(site) = locate_nsig_site(&sc).await else {
        println!("[!] 未在 base.js 定位到 n 解扰调用(版本可能大改);先跑 re_yt_sig 重新侦察。");
        browser.quit().await?;
        return Ok(());
    };
    println!("[*] 定位 cs6 解扰调用:base.js RAW 行 {}(0 基)", site.line);
    println!("    解扰表达式:{}", trunc(&site.nsig_expr, 110));
    println!(
        "    解扰类={}  命名空间别名={}  入参名={}",
        site.class_path, site.alias, site.param
    );

    let dbg = tab.debugger();
    // ③ 干净环境:上面 `tab.scripts()` 定位时 list() 会设 setSkipAllPauses(true),不复位则断点永不触发。
    dbg.set_skip_all_pauses(false).await?;

    // ① 断在 cs6 的**赋值处(列 0)**:base.js 重新求值即执行、必命中(函数体断点对 ?n= 形态的视频命不中)。
    //    触发=整页 reload(重新求值 base.js)。setTimeout 包成立即返回:切勿 await 会命中断点的导航,否则死锁。
    println!(
        "[*] 断点(base.js:{}:0 赋值处)+ reload 触发 base.js 重新求值 …",
        site.line
    );
    let trigger = || async {
        let _ = tab
            .run_js("setTimeout(function(){try{location.reload()}catch(e){}},200);1")
            .await;
        Ok(())
    };
    let stack = match dbg
        .break_at_then(
            "base\\.js",
            site.line,
            Some(0),
            None,
            trigger,
            Some(Duration::from_secs(45)),
        )
        .await?
    {
        Some(s) => s,
        None => {
            println!("[!] 未命中 base.js 求值(reload 可能被拦);试 HL=0 或重跑。");
            browser.quit().await?;
            return Ok(());
        }
    };
    println!("\n========== ① 命中断点(base.js 求值,cs6 赋值处)==========");
    println!("reason={}", stack.reason());
    for l in stack.backtrace().lines().take(4) {
        println!("   {l}");
    }

    // 命中帧里:模块命名空间别名应为对象(此刻 g 已存在,g.g7 稍后才挂上)。
    let ns_type = stack
        .eval_str(0, &format!("typeof {}", site.alias))
        .await
        .unwrap_or_default();
    let cls_now = stack
        .eval_str(0, &format!("typeof {}", site.class_path))
        .await
        .unwrap_or_default();
    println!(
        "[现场] typeof {}={ns_type}  typeof {}={cls_now}(解扰类此刻未挂属正常)",
        site.alias, site.class_path
    );
    if ns_type != "object" && ns_type != "function" {
        println!(
            "[!] 命名空间别名 `{}` 不是对象(版本结构变化);先跑 re_yt_sig 重新侦察。",
            site.alias
        );
        stack.resume().await?;
        browser.quit().await?;
        return Ok(());
    }

    // ④ 把模块命名空间对象引用抓到全局:resume 后 base.js 把 g7 挂到**同一对象**上 → window.__ytNsigNs.g7 即解扰类。
    let cap = stack
        .expose_as_global(0, "__ytNsigNs", &site.alias)
        .await
        .unwrap_or_default();
    println!(
        "[抓取] window.__ytNsigNs = {}(模块命名空间),typeof={cap}",
        site.alias
    );
    stack.resume().await?;

    // 等 base.js 求值完成:g7 此时已挂到被抓住的命名空间对象上。
    tokio::time::sleep(Duration::from_secs(6)).await;
    let cls_after = run_js_str(&tab, &format!("typeof window.__ytNsigNs{}", site.rest)).await;
    println!("\n========== ④ resume 后:解扰类已就位 ==========");
    println!("[就位] typeof window.__ytNsigNs{} = {cls_after}", site.rest);
    if cls_after != "function" && cls_after != "object" {
        println!("[!] 解扰类未在命名空间上就位(版本结构变化);中止。");
        browser.quit().await?;
        return Ok(());
    }

    // 干净上下文挂 oracle:window.__ytNsig(url) → 解扰后的 n(用就地解析出的类路径,版本轮换也对)。
    let oracle = format!(
        "window.__ytNsig=function(u){{try{{return new window.__ytNsigNs{rest}(u,!0).get(\"n\")||''}}\
         catch(e){{return 'ERR:'+e}}}};typeof window.__ytNsig",
        rest = site.rest,
    );
    let otype = run_js_str(&tab, &oracle).await;
    println!("[oracle] window.__ytNsig 已挂载,typeof={otype}");

    // ⑤ dump 解扰类源码(供离线精读 VM)。派发器(pV/O9…)是模块闭包内的局部函数、不挂在命名空间上,
    //    故只列其名 + 指向完整 base.js(`re_yt_sig` 落盘的 /tmp/yt_base.js)里查其函数体。
    let cls_src = run_js_str(&tab, &format!("String(window.__ytNsigNs{})", site.rest)).await;
    if cls_src.len() > 20 {
        let _ = std::fs::write("/tmp/yt_nsig_class.js", beautify_js(&cls_src));
        println!(
            "[dump] 解扰类源码 → /tmp/yt_nsig_class.js({} 字符)",
            cls_src.len()
        );
        let disp = dispatcher_names(&cls_src);
        if !disp.is_empty() {
            println!(
                "[VM] opcode 派发器(闭包局部函数,见 /tmp/yt_base.js):{}",
                disp.join(", ")
            );
        }
    }

    // ⑥ 验真:从 player response 取真实乱序 n,干净上下文(无断点)用 oracle 解扰两次。
    println!("\n========== ⑥ 干净上下文验真 ==========");
    let n_scrambled = run_js_str(&tab, EXTRACT_SCRAMBLED_N_JS).await;
    println!("[乱序] n = {n_scrambled}");
    if n_scrambled.is_empty() || n_scrambled.starts_with("ERR") {
        println!(
            "[!] 未从 ytInitialPlayerResponse 取到乱序 n(可换 URL 重试);oracle 已就绪={}.",
            otype == "function"
        );
        browser.quit().await?;
        return Ok(());
    }
    let test_url = format!("https://rr.googlevideo.com/videoplayback?n={n_scrambled}&x=1");
    let call = format!("window.__ytNsig({})", json_str(&test_url));
    let again1 = run_js_str(&tab, &call).await;
    let again2 = run_js_str(&tab, &call).await;
    println!("[解扰] n = {again1}");
    println!("[解扰] n(复跑)= {again2}");
    let deterministic = !again1.is_empty() && !again1.starts_with("ERR") && again1 == again2;
    let changed = again1 != n_scrambled;
    println!("[结果] oracle可用(确定性)={deterministic} | 乱序≠解扰={changed}");
    if deterministic && changed {
        println!(
            "[✅] 端到端:① 断点抓模块命名空间 → 抠出解扰器 oracle → 干净上下文解扰一致,逻辑已可复用。"
        );
    } else {
        println!("[!] 未完全闭环(见上)。");
    }

    browser.quit().await?;
    Ok(())
}

/// n 解扰调用点(全部从活源码解析,版本无关)。
struct NsigSite {
    /// base.js RAW 行(0 基);列固定用 0(断 `cs6=function` 赋值处)。
    line: u32,
    /// 整段解扰表达式,如 `(new g.g7(m,!0)).get("n")`(仅打印展示)。
    nsig_expr: String,
    /// 解扰类完整路径,如 `g.g7`。
    class_path: String,
    /// 命名空间别名(类路径首段),如 `g`;断点处抓它到全局。
    alias: String,
    /// 类路径去掉别名后的剩余,如 `.g7`;拼 `window.__ytNsigNs.g7`。
    rest: String,
    /// 入参变量名,如 `m`(仅打印展示)。
    param: String,
}

/// ② 在活的 base.js 里唯一定位 cs6 的解扰调用 `(new …,!0)).get("n")`。
/// 锚点从强到弱(带 `!0` 解扰旗标最特异),命中后逐条尝试精确解析,取第一条解析成功的。
async fn locate_nsig_site(sc: &ChromiumScripts) -> Option<NsigSite> {
    for pat in [
        r#",!0\)\)\.get\("n"\)"#,
        r#"\)\)\.get\("n"\)"#,
        r#"\.get\("n"\)"#,
    ] {
        let hits = sc.grep_with(pat, true, true).await.unwrap_or_default();
        for m in hits.iter().filter(|m| m.url.contains("/base.js")) {
            if let Some(site) = parse_nsig_site(m) {
                return Some(site);
            }
        }
    }
    None
}

/// 从一条命中行里解析解扰点:行内找 `.get("n")`、回溯到它前面的 `(new <类>(<参>,…))`,抠出 类路径/别名/参数名。
fn parse_nsig_site(m: &ScriptMatch) -> Option<NsigSite> {
    let lc = &m.line_content;
    let get = r#".get("n")"#;
    let mut from = 0usize;
    while let Some(rel) = lc[from..].find(get) {
        let gi = from + rel;
        if let Some(np) = lc[..gi].rfind("(new") {
            if let Some((class_path, param)) = parse_new_call(&lc[np..gi]) {
                let alias = class_path.split('.').next().unwrap_or("").to_string();
                let rest = class_path.strip_prefix(&alias).unwrap_or("").to_string();
                return Some(NsigSite {
                    line: m.line_number,
                    nsig_expr: lc[np..gi + get.len()].to_string(),
                    class_path,
                    alias,
                    rest,
                    param,
                });
            }
        }
        from = gi + get.len();
    }
    None
}

/// 解析 `(new <类>(<参>,…))` → (类表达式, 第一个参数名)。名字随版本轮换,故就地抠并校验是干净标识符。
fn parse_new_call(s: &str) -> Option<(String, String)> {
    let s = s.trim_start().strip_prefix('(')?.trim_start();
    let s = s.strip_prefix("new")?.trim_start(); // 形如 "g.g7(m,!0))"
    let lp = s.find('(')?;
    let class_path = s[..lp].trim().to_string();
    let rest = &s[lp + 1..];
    let end = rest.find([',', ')']).unwrap_or(rest.len());
    let param = rest[..end].trim().to_string();
    let clean_class = !class_path.is_empty()
        && class_path
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'$'));
    let clean_param = !param.is_empty()
        && param
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'$'));
    (clean_class && clean_param).then_some((class_path, param))
}

/// 从解扰类源码里抠出 opcode 派发器的函数名:形如 `pV(8,6793,this)` / `O9(20,1951,this)`。
fn dispatcher_names(src: &str) -> Vec<String> {
    let bytes = src.as_bytes();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        if is_ident_start(bytes[i]) {
            let start = i;
            while i < bytes.len() && is_ident_part(bytes[i]) {
                i += 1;
            }
            let name = &src[start..i];
            if looks_like_dispatch_call(&src[i..]) && !out.iter().any(|n| n == name) {
                out.push(name.to_string());
            }
        } else {
            i += 1;
        }
    }
    out
}

/// `(数字,数字,this)` 形态判定(派发器调用特征)。
fn looks_like_dispatch_call(s: &str) -> bool {
    let s = s.as_bytes();
    if s.first() != Some(&b'(') {
        return false;
    }
    let mut i = 1;
    let mut nums = 0;
    while nums < 2 {
        let d0 = i;
        while i < s.len() && s[i].is_ascii_digit() {
            i += 1;
        }
        if i == d0 {
            return false;
        }
        nums += 1;
        if i < s.len() && s[i] == b',' {
            i += 1;
        }
    }
    s[i..].starts_with(b"this)")
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_' || b == b'$'
}
fn is_ident_part(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$'
}

fn trunc(s: &str, n: usize) -> String {
    let t: String = s.chars().take(n).collect();
    if s.chars().count() > n {
        format!("{t}…")
    } else {
        t
    }
}

/// 把字符串包成 JS 字面量(供 run_js 内联)。
fn json_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string())
}

/// 跑一段 JS,取其字符串结果(非字符串/失败 → 空串)。
async fn run_js_str(tab: &ChromiumTab, expr: &str) -> String {
    tab.run_js(expr)
        .await
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// 从 `ytInitialPlayerResponse` 的 adaptiveFormats/formats 里取一个真实乱序 n(`?n=` 查询形态)。
const EXTRACT_SCRAMBLED_N_JS: &str = r#"(function(){try{
  var r=window.ytInitialPlayerResponse;var sd=r&&r.streamingData;if(!sd)return '';
  var all=(sd.adaptiveFormats||[]).concat(sd.formats||[]);
  for(var i=0;i<all.length;i++){
    var u=all[i].url;
    if(!u&&all[i].signatureCipher){try{u=new URLSearchParams(all[i].signatureCipher).get('url')||''}catch(e){}}
    if(u){var mm=u.match(/[?&]n=([^&]+)/);if(mm)return mm[1]}
  }
  return ''
}catch(e){return 'ERR:'+e}})()"#;
