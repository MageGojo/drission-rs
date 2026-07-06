//! YouTube **端到端**:base.js 签名(sig)+ n 参数解扰 → 拼真实 `googlevideo` 直链 → 下载 + ffprobe 验真。
//!
//! 现代 base.js(2024+)把 **sig 与 nsig 统一进同一个 opcode VM**——经典 `a.split("")…a.join("")` 已不存在。
//! 看清机器后发现一个「万能直链构建器」:
//! ```text
//! y2=function(m,Z="",J=""){m=new g.g7(m,!0);m.set("alr","yes");
//!     J&&(J=Wv(65,2902,Wv(3,4369,J)),m[c[12]](Z,$8(21,6101,J)));return m};
//! ```
//! 即 `y2(url, sp, s)` 返回一个 URL 对象:**n 已被 `g.g7(...,!0)` 解扰、签名 `s` 已被 `Wv` 解密并 `set`**,
//! `.JK()`(序列化方法,名字会轮换)即终极直链。opcode 常量(65,2902,…)闭包在 y2 体内,**捕获 y2 即自带**。
//!
//! 复用的「主动逆向」能力:
//!   - **① 断点 + `expose_as_global`**:`y2`/`g7` 都是模块闭包里的私有件(全局取不到)。在 base.js 求值时
//!     断在 `cs6` 赋值处(列 0,必命中),用 `expose_as_global` 在**断点帧作用域**里造一个闭包函数挂到
//!     `window`——它闭包住模块作用域里的 `y2`,resume 后仍可随处调用(call 时才解析,无顺序/版本依赖)。
//!   - **③/② 干净环境 + 就地解析**:`tab.scripts()` 定位时 `list()` 会 `setSkipAllPauses(true)` 屏蔽断点,
//!     下断点前必须 `set_skip_all_pauses(false)`;`y2`/解扰类/命名空间别名/行号**全从活 base.js 就地解析**
//!     (base.js 每几小时轮换,硬编码必挂)。
//!
//! 流程:
//!   1) ② 在活 base.js 里就地定位 `cs6` 的 nsig 站点(行号 + 解扰类路径 `g.g7`)、解析「直链构建器 y2」当前名;
//!   2) ① 干净环境 → 断 `cs6=function` 赋值处 → reload 触发 base.js 重求值 → 必命中;
//!   3) 断点帧暴露 `window.__ytUrl(url,sp,s)`(闭包 y2,n+sig 一把梭)+ `window.__ytNsig(url)`(闭包 g,展示 n);
//!   4) 从 `ytInitialPlayerResponse` 取 progressive 格式(带 `signatureCipher`)→ `__ytUrl` 拼终极直链;
//!   5) reqwest `Range` 下载一段落盘 → ffprobe 验真(本地文件 + 直连 URL 权威);403 则透传播放器 `pot` 重试。
//!
//! 运行:`cargo run --example re_yt_play --features cdp`(`HL=0` 有头;`URL=` 换视频;`MB=` 下载 MiB 数)。

use std::process::Command;
use std::time::Duration;

use drission::prelude::*;
use serde_json::Value;

const DEFAULT_URL: &str = "https://www.youtube.com/watch?v=7349tcyyE-c";

#[tokio::main]
async fn main() -> drission::Result<()> {
    let headless = std::env::var("HL").map(|v| v != "0").unwrap_or(true);
    let url = std::env::var("URL").unwrap_or_else(|_| DEFAULT_URL.to_string());
    let range_bytes: u64 = std::env::var("MB")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(4)
        .saturating_mul(1024 * 1024);

    let browser = ChromiumBrowser::launch(ChromiumOptions::new().headless(headless)).await?;
    let tab = browser.new_tab(Some("about:blank")).await?;

    // ⑤ 抓播放器真实 googlevideo 请求(为 pot 透传 / n 交叉印证),导航前开监听。
    let listener = tab.listen();
    listener.start(&["videoplayback"]).await?;

    tab.get(&url).await?;
    println!("[*] title={:?}", tab.title().await.unwrap_or_default());
    let ua = run_js_str(&tab, "navigator.userAgent").await;
    tokio::time::sleep(Duration::from_secs(6)).await; // 等 base.js 解析
    // 主动起播:促使 base.js 处理 streamingData,并产生真实 googlevideo 流量。
    let _ = tab
        .run_js("setTimeout(function(){try{var v=document.querySelector('video');if(v){v.muted=true;v.play&&v.play()}}catch(e){}},0);1")
        .await;
    tokio::time::sleep(Duration::from_secs(3)).await;

    // ② 就地定位:nsig 站点(cs6 的 `.get("n")` 行)+ 直链构建器 y2 当前名。
    let sc = tab.scripts();
    let Some(site) = locate_nsig_site(&sc).await else {
        println!("[!] base.js 未定位到 n 解扰站点(版本大改),先跑 re_yt_sig 重新侦察。");
        browser.quit().await?;
        return Ok(());
    };
    let Some(y2) = locate_url_builder(&sc).await else {
        println!(
            "[!] base.js 未定位到直链构建器 y2(`set(\"alr\",\"yes\")` 锚点失配),版本结构可能变化。"
        );
        browser.quit().await?;
        return Ok(());
    };
    println!(
        "[*] nsig 站点 base.js:{}  解扰类={}  | 直链构建器 y2 = {}",
        site.line, site.class_path, y2
    );

    let dbg = tab.debugger();
    // ③ 干净环境:上面 scripts() 定位时 list() 会 setSkipAllPauses(true),不复位则断点永不触发。
    dbg.set_skip_all_pauses(false).await?;

    // ① 断在 cs6 赋值处(列 0):base.js 一被重新求值即执行、必命中。触发=整页 reload。
    println!(
        "[*] 断点(base.js:{}:0)+ reload 触发 base.js 重求值 …",
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
            println!("[!] 未命中 base.js 求值(reload 可能被拦),试 HL=0 或重跑。");
            browser.quit().await?;
            return Ok(());
        }
    };
    println!(
        "[①命中] {}",
        stack.backtrace().lines().next().unwrap_or_default()
    );

    // 断点帧暴露两个 oracle(都闭包住模块作用域,resume 后随处可调):
    //   __ytUrl(url,sp,s):调 y2 → n+sig 一把梭;返回对象用运行时探测序列化(对 .JK() 名字轮换免疫)。
    //   __ytNsig(url):用就地解析的解扰类,单独展示/印证 n 变换。
    let url_oracle = build_url_oracle(&y2);
    let t_url = stack
        .expose_as_global(0, "__ytUrl", &url_oracle)
        .await
        .unwrap_or_default();
    let nsig_oracle = format!(
        "function(u){{try{{return (new {cls}(u,!0)).get('n')||''}}catch(e){{return 'ERR:'+e}}}}",
        cls = site.class_path
    );
    let t_n = stack
        .expose_as_global(0, "__ytNsig", &nsig_oracle)
        .await
        .unwrap_or_default();
    println!("[oracle] __ytUrl typeof={t_url} | __ytNsig typeof={t_n}");
    stack.resume().await?;
    tokio::time::sleep(Duration::from_secs(6)).await; // 等 base.js 求值完成(y2/g7 全部就位)

    if run_js_str(&tab, "typeof window.__ytUrl").await != "function" {
        println!("[!] __ytUrl 未在干净上下文就位(版本结构变化);中止。");
        browser.quit().await?;
        return Ok(());
    }

    // ④ 取格式:progressive 优先(formats[0],muxed 易 ffprobe),退而求带 url/cipher 的 adaptive。
    let fmt: Value =
        serde_json::from_str(&run_js_str(&tab, PICK_FORMAT_JS).await).unwrap_or(Value::Null);
    if let Some(err) = fmt.get("err").and_then(Value::as_str) {
        println!("[!] 取格式失败:{err}");
        browser.quit().await?;
        return Ok(());
    }
    let f_url = fmt["url"].as_str().unwrap_or_default().to_string();
    let sp = fmt["sp"].as_str().unwrap_or_default().to_string();
    let s = fmt["s"].as_str().unwrap_or_default().to_string();
    let itag = fmt["itag"].as_i64().unwrap_or(0);
    let mime = fmt["mime"].as_str().unwrap_or_default().to_string();
    let kind = fmt["kind"].as_str().unwrap_or_default().to_string();
    let scrambled_n = fmt["scrambledN"].as_str().unwrap_or_default().to_string();
    println!("\n========== ④ 目标格式 ==========");
    println!("[格式] kind={kind} itag={itag} mime={mime}");
    println!(
        "       密文? s={} sp={}",
        yesno(!s.is_empty()),
        if sp.is_empty() { "-" } else { &sp }
    );

    // n 变换展示(乱序 → 解扰)。
    let clean_n = run_js_str(&tab, &format!("window.__ytNsig({})", json_str(&f_url))).await;
    println!("[n] 乱序 {scrambled_n}  →  解扰 {clean_n}");

    // 拼终极直链(连拼两次验确定性)。
    let call = format!(
        "window.__ytUrl({},{},{})",
        json_str(&f_url),
        json_str(&sp),
        json_str(&s)
    );
    let final_url = run_js_str(&tab, &call).await;
    let final_url2 = run_js_str(&tab, &call).await;
    if final_url.is_empty() || final_url.starts_with("ERR") {
        println!("[!] __ytUrl 拼链失败:{final_url}");
        browser.quit().await?;
        return Ok(());
    }
    let deterministic = final_url == final_url2;
    let n_applied = !clean_n.is_empty()
        && !clean_n.starts_with("ERR")
        && param_of(&final_url, "n").as_deref() == Some(clean_n.as_str());
    let sig_applied = s.is_empty() || param_of(&final_url, &sp).is_some();
    println!("\n========== ⑤ 拼链 + 校验 ==========");
    println!("[直链] {}", trunc(&final_url, 150));
    println!(
        "[校验] 确定性={} | n 已套用={} | sig 已 set={}",
        yesno(deterministic),
        yesno(n_applied),
        yesno(sig_applied)
    );
    // 完整直链落盘(googlevideo 链有 expire 时效、且绑定本机出口 IP——换机/过期会 403)。
    let link_path = format!("/tmp/yt_direct_link_{itag}.txt");
    let _ = std::fs::write(&link_path, &final_url);
    println!("[完整直链] 已写 {link_path}");

    // pot 透传:若我们的链没带、播放器真实请求带了 → 借用(pot 是会话 token,同 cookie 透传,非「复现」)。
    let packets = listener
        .wait_count(12, Some(Duration::from_secs(3)))
        .await
        .unwrap_or_default();
    let player_pot = packets.iter().find_map(|p| param_of(&p.url, "pot"));
    let player_n = packets.iter().find_map(|p| param_of(&p.url, "n"));
    listener.stop().await.ok();
    if let Some(pn) = &player_n {
        println!("[交叉] 播放器真实请求 n 样本={pn}(我方解扰 n={clean_n})");
    }
    let mut dl_url = final_url.clone();
    if param_of(&dl_url, "pot").is_none() {
        if let Some(p) = &player_pot {
            dl_url = append_param(&dl_url, "pot", p);
            println!(
                "[pot] 我方链未带 pot,透传播放器 pot({}…)",
                &p[..p.len().min(12)]
            );
        }
    }

    // 下载一段落盘 + ffprobe 验真。
    println!("\n========== ⑥ 下载 + ffprobe 验真 ==========");
    let out_path = format!("/tmp/yt_clip_{itag}.{}", ext_of(&mime));
    let (mut status, mut ct, mut body) = download_range(&dl_url, &ua, range_bytes).await;
    // 若 403 且我方链原本没带 pot、之前也没取到 → 直接用 final_url(无 pot)重试一次(部分视频无需 pot)。
    if !(status == 200 || status == 206) && dl_url != final_url {
        println!("[下载] 带 pot HTTP {status},回退无 pot 直链重试 …");
        let r = download_range(&final_url, &ua, range_bytes).await;
        status = r.0;
        ct = r.1;
        body = r.2;
    }
    println!(
        "[下载] HTTP {status}  content-type={ct}  {} bytes",
        body.len()
    );
    let ok_http = (status == 200 || status == 206) && body.len() > 1024 && !ct.contains("text/");
    if ok_http {
        if let Err(e) = std::fs::write(&out_path, &body) {
            println!("[!] 落盘失败:{e}");
        } else {
            println!("[落盘] {out_path}");
            println!("[ffprobe·本地文件]\n{}", indent(&ffprobe_file(&out_path)));
        }
    }
    // 直连 URL 的 ffprobe 是权威验真(ffprobe 自己做 Range、能处理 moov 在文件尾的情况)。
    println!(
        "[ffprobe·直连URL(权威)]\n{}",
        indent(&ffprobe_url(&final_url, &ua))
    );

    println!("\n========== 结论 ==========");
    let reproduced = deterministic && (n_applied || clean_n == scrambled_n) && sig_applied;
    if reproduced && ok_http {
        println!(
            "[✅] 端到端打通:① 断点抠出 y2(sig+nsig 统一 VM)→ 自算终极直链 → 真实下载 HTTP {status} + ffprobe 验真。"
        );
    } else if reproduced {
        println!(
            "[◑] 签名/直链复现 OK(确定性+套用 n+set sig),但本次下载未达 2xx(见上,多为 pot/IP/区域限制)。"
        );
    } else {
        println!("[!] 未完全闭环(见上各项校验)。");
    }

    browser.quit().await?;
    Ok(())
}

// ════════════════════════════════════════════════════════════════════════════
// nsig 站点定位(从活 base.js 就地解析,版本无关;同 re_yt_sig2)
// ════════════════════════════════════════════════════════════════════════════

/// n 解扰站点:cs6 里 `(new g.g7(m,!0)).get("n")` 所在行 + 解扰类路径。
struct NsigSite {
    line: u32,
    class_path: String,
}

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

fn parse_nsig_site(m: &ScriptMatch) -> Option<NsigSite> {
    let lc = &m.line_content;
    let get = r#".get("n")"#;
    let mut from = 0usize;
    while let Some(rel) = lc[from..].find(get) {
        let gi = from + rel;
        if let Some(np) = lc[..gi].rfind("(new") {
            if let Some(class_path) = parse_new_class(&lc[np..gi]) {
                return Some(NsigSite {
                    line: m.line_number,
                    class_path,
                });
            }
        }
        from = gi + get.len();
    }
    None
}

/// 解析 `(new <类>(<参>,…))` → 类表达式(如 `g.g7`)。
fn parse_new_class(s: &str) -> Option<String> {
    let s = s.trim_start().strip_prefix('(')?.trim_start();
    let s = s.strip_prefix("new")?.trim_start();
    let lp = s.find('(')?;
    let class_path = s[..lp].trim().to_string();
    let clean = !class_path.is_empty()
        && class_path
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'$'));
    clean.then_some(class_path)
}

// ════════════════════════════════════════════════════════════════════════════
// 直链构建器 y2 名解析(锚点:YouTube 专有的 `set("alr","yes")` + 双默认参 + `,!0)`)
// ════════════════════════════════════════════════════════════════════════════

/// 在活 base.js 里定位「万能直链构建器」`y2=function(m,Z="",J=""){m=new g.g7(m,!0);m.set("alr","yes");…}`
/// 的当前函数名(`set("alr","yes")` 是 YouTube 极稳的专有标记;名字会轮换故就地解析)。
async fn locate_url_builder(sc: &ChromiumScripts) -> Option<String> {
    let hits = sc
        .grep_with(r#"set("alr","yes")"#, true, false)
        .await
        .unwrap_or_default();
    for m in hits.iter().filter(|m| m.url.contains("/base.js")) {
        if let Some(name) = parse_url_builder_name(&m.line_content) {
            return Some(name);
        }
    }
    None
}

/// 从含 `set("alr","yes")` 的行抠出形如 `NAME=function(a,b="",c="")` 的 NAME。
/// 校验:`=function(` 与 `alr` 之间须含 `,!0)`(URL 解扰类构造)且至少两个 `=""` 默认参——排除其它用到
/// `set("alr","yes")` 的方法(g.Rq / async $r 等)。
fn parse_url_builder_name(lc: &str) -> Option<String> {
    let ai = lc.find(r#"set("alr","yes")"#)?;
    let fi = lc[..ai].rfind("=function(")?;
    let between = &lc[fi..ai];
    if !between.contains(",!0)") || between.matches("=\"\"").count() < 2 {
        return None;
    }
    let bytes = lc.as_bytes();
    let mut start = fi; // fi 指向 '='
    while start > 0 && is_ident_part(bytes[start - 1]) {
        start -= 1;
    }
    let name = &lc[start..fi];
    (!name.is_empty() && is_ident_start(name.as_bytes()[0])).then(|| name.to_string())
}

/// 终极直链 oracle:`y2(url,sp,s)` → 返回 URL 对象,再**运行时探测**其原型上的无参方法、取返回
/// `http(s)://…` 的那个序列化(对 `.JK()` 之类的名字轮换免疫)。`__Y2__` 占位符注入实际函数名。
fn build_url_oracle(y2: &str) -> String {
    const T: &str = r#"function(u,sp,s){try{var o=__Y2__(u,(sp==null?"":sp),(s==null?"":s));if(o==null)return"";if(typeof o==="string")return o;var t="";try{t=o.toString()}catch(e){}if(typeof t==="string"&&/^https?:\/\//.test(t))return t;var pr=Object.getPrototypeOf(o)||{},ns=Object.getOwnPropertyNames(pr);for(var i=0;i<ns.length;i++){var nm=ns[i];if(nm==="constructor")continue;try{var fn=o[nm];if(typeof fn==="function"&&fn.length===0){var r=fn.call(o);if(typeof r==="string"&&/^https?:\/\//.test(r))return r}}catch(e){}}return"ERRSER:"+t}catch(e){return"ERR:"+e}}"#;
    T.replace("__Y2__", y2)
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_' || b == b'$'
}
fn is_ident_part(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$'
}

// ════════════════════════════════════════════════════════════════════════════
// 取 player response 格式
// ════════════════════════════════════════════════════════════════════════════

/// 从 `ytInitialPlayerResponse` 取一个带 url/signatureCipher 的格式(progressive 优先)。
const PICK_FORMAT_JS: &str = r#"(function(){try{
  var r=window.ytInitialPlayerResponse,sd=r&&r.streamingData;
  if(!sd)return JSON.stringify({err:'no streamingData'});
  function take(list,kind){for(var i=0;i<(list||[]).length;i++){var f=list[i];if(f.url||f.signatureCipher)return {kind:kind,f:f}}return null}
  var pick=take(sd.formats,'progressive')||take(sd.adaptiveFormats,'adaptive');
  if(!pick)return JSON.stringify({err:'no format carries url/signatureCipher'});
  var f=pick.f,out={kind:pick.kind,itag:f.itag,mime:f.mimeType||'',clen:f.contentLength||''};
  if(f.url){out.url=f.url;out.sp='';out.s=''}
  else{var p=new URLSearchParams(f.signatureCipher);out.url=p.get('url')||'';out.sp=p.get('sp')||'sig';out.s=p.get('s')||''}
  var mm=out.url.match(/[?&]n=([^&]+)/);out.scrambledN=mm?decodeURIComponent(mm[1]):'';
  return JSON.stringify(out)
}catch(e){return JSON.stringify({err:String(e)})}})()"#;

// ════════════════════════════════════════════════════════════════════════════
// 下载 + ffprobe
// ════════════════════════════════════════════════════════════════════════════

/// 下载 `Range: bytes=0-(max-1)`,返回 (status, content-type, bytes)。带浏览器 UA + youtube Referer/Origin。
async fn download_range(url: &str, ua: &str, max: u64) -> (u16, String, Vec<u8>) {
    let client = match reqwest::Client::builder().build() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[!] reqwest client: {e}");
            return (0, String::new(), Vec::new());
        }
    };
    let resp = client
        .get(url)
        .header("User-Agent", ua)
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

fn ffprobe_url(url: &str, ua: &str) -> String {
    run_ffprobe(&[
        "-v",
        "error",
        "-user_agent",
        ua,
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
                let err = String::from_utf8_lossy(&o.stderr);
                format!("(无可识别媒体流;stderr: {})", err.trim())
            } else {
                out
            }
        }
        Err(e) => format!("ffprobe 调用失败: {e}"),
    }
}

// ════════════════════════════════════════════════════════════════════════════
// 小工具
// ════════════════════════════════════════════════════════════════════════════

fn ext_of(mime: &str) -> &'static str {
    if mime.contains("audio/mp4") {
        "m4a"
    } else if mime.contains("audio/webm") {
        "weba"
    } else if mime.contains("webm") {
        "webm"
    } else if mime.contains("mp4") {
        "mp4"
    } else {
        "bin"
    }
}

/// 取 URL 查询参数(url-decode 后)。
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

fn append_param(url: &str, key: &str, val: &str) -> String {
    let sep = if url.contains('?') { '&' } else { '?' };
    format!("{url}{sep}{key}={}", url_encode(val))
}

/// 极简 percent-decode(够用:n/pot 值不含 %xx 也安全)。
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

/// 极简 percent-encode(只对非 unreserved 编码,够拼 pot 这类 token)。
fn url_encode(s: &str) -> String {
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

fn json_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string())
}

async fn run_js_str(tab: &ChromiumTab, expr: &str) -> String {
    tab.run_js(expr)
        .await
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}
