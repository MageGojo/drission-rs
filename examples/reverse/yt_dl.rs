//! `yt_dl` —— YouTube **通用命令行下载器**(脱浏览器纯算 sig/n;选清晰度 / 合并高清音视频 / 进度条 / pot 透传)。
//!
//! 复用 [`yt_engine`](自带模块):reqwest 抓 watch 页 + base.js → InnerTube(默认 **TVHTML5** 客户端,免 pot
//! 即给全部 adaptive 含 4K)→ 内嵌 QuickJS 跑整份 base.js 自算每路直链 → 进度条下载 → ffmpeg 合并。
//!
//! 用法:
//! ```bash
//! cargo run --example yt_dl --features signer,cdp -- <URL|videoId> [选项]
//!   -q, --quality best|2160|1440|1080|720|480|360|audio|itag:N   (默认 best)
//!   -o, --output  <输出名(不含扩展名)>                          (默认 视频标题/ID)
//!       --list        只列出可用格式,不下载
//!   -g, --get-url     只算并打印直链(不下载;分轨打印视频+音频两行;过程信息走 stderr)
//!   -x, --proxy <URL> 出口代理(铸链+下载全程同一出口)——直链 ip= 即该出口,可在该网络复用
//!                     支持 http://、https://、socks5://、socks5h://(也读 YT_PROXY 环境变量)
//!       --client tv|web                                          (默认 tv;web=进度/兼容)
//!       --audio-only  只下音频
//!       --no-merge    视频/音频分开存,不合并
//!       --pot <TOKEN> 手动透传 po_token
//!       --browser-pot 用无头 Chrome 抓播放器真实 pot+visitor(走 WEB 客户端)——给受限视频兜底
//! ```
//!
//! ## 出口 IP 与「换出口」(直链为何会失效、怎么解决)
//!
//! googlevideo 直链把**铸链时所见的客户端出口 IP** 签进 `ip=`(并入受签名保护的 `sparams`),
//! YouTube 边缘节点会校验「下载者 IP == 直链 ip=」。所以:
//!
//! - **改不了**已铸链的 `ip=`(被签名锁死,改一个字符就 403);换设备/换网络打开必然失效。
//! - 要让某出口 X 能用的直链,**只能用出口 X 去铸链**:本工具全流程共用一个 client,`--proxy X`
//!   会让 watch/base.js/InnerTube(铸链)与 googlevideo(下载)全程同一出口 → `ip=` 即 X。
//!
//! 三种用法:
//!
//! 1. 解决本机 WARP/轮换代理的间歇 403 = 用 sticky/固定出口代理把两端钉在同一 IP。
//! 2. 给别的设备/网络用 = 用「那台设备也会经过的代理」铸链,把代理给它(或它走同一代理)即可复用。
//! 3. 为指定地区/服务器出口铸链 = `--proxy` 指向落在该出口的代理。
//!
//! 无法做到「一条 URL 任意 IP 通用」(那是 YouTube 反盗链设计,yt-dlp 同此限制)。
//!
//! 例:`cargo run --example yt_dl --features signer,cdp -- dQw4w9WgXcQ -q 1080`

#[path = "yt_engine.rs"]
mod yt_engine;

use yt_engine::{Client, Format, Kind, Plan, Solver};

struct Args {
    url: String,
    quality: String,
    output: Option<String>,
    list: bool,
    get_url: bool,
    client: Client,
    audio_only: bool,
    no_merge: bool,
    pot: String,
    browser_pot: bool,
    proxy: Option<String>,
}

fn parse_args() -> Option<Args> {
    let mut a = Args {
        url: String::new(),
        quality: "best".into(),
        output: None,
        list: false,
        get_url: false,
        client: Client::Tv,
        audio_only: false,
        no_merge: false,
        pot: String::new(),
        browser_pot: false,
        // 未给 --proxy 时回退 YT_PROXY 环境变量(空串视为未设)。
        proxy: std::env::var("YT_PROXY")
            .ok()
            .filter(|s| !s.trim().is_empty()),
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-q" | "--quality" => a.quality = it.next()?,
            "-o" | "--output" => a.output = it.next(),
            "--list" => a.list = true,
            "-g" | "--get-url" | "--url-only" => a.get_url = true,
            "-x" | "--proxy" => a.proxy = it.next().filter(|s| !s.trim().is_empty()),
            "--client" => {
                a.client = if it.next().as_deref() == Some("web") {
                    Client::Web
                } else {
                    Client::Tv
                }
            }
            "--audio-only" => a.audio_only = true,
            "--no-merge" => a.no_merge = true,
            "--pot" => a.pot = it.next().unwrap_or_default(),
            "--browser-pot" => a.browser_pot = true,
            s if !s.starts_with('-') => a.url = s.to_string(),
            _ => {}
        }
    }
    if a.url.is_empty() { None } else { Some(a) }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let Some(mut args) = parse_args() else {
        eprintln!(
            "用法:cargo run --example yt_dl --features signer,cdp -- <URL|videoId> [-q best|1080|audio|itag:N] [-o name] [--list] [-g|--get-url] [-x|--proxy URL] [--client tv|web] [--audio-only] [--no-merge] [--pot TOKEN] [--browser-pot]"
        );
        std::process::exit(2);
    };
    if args.audio_only {
        args.quality = "audio".into();
    }
    let Some(video_id) = yt_engine::extract_video_id(&args.url) else {
        eprintln!("[!] 无法从输入解析 videoId: {}", args.url);
        std::process::exit(2);
    };
    // googlevideo 直链把「铸链时所见客户端出口 IP」写进签名参数 `ip=`(进 `sparams`),下载出口必须与之
    // 一致否则 403/限速。**不要**绑 local_address:本机常走 fake-ip TUN 代理(198.18.0.0/15),铸链
    // (watch/InnerTube)与下载(googlevideo)都得经同一 TUN 出口才一致;一旦 .local_address(...) 绑死源地址
    // 反而把连接从 TUN 上拽下来 → 出口与 `ip=` 不符 → 时而 403、放行也被限速。默认路由 = 与 curl 同路 = 一致。
    // 给了 `--proxy` 则用代理把「铸链 + 下载」钉在同一出口 → `ip=` 即该出口,可换出口/给别的设备复用
    //(见文件头「出口 IP 与换出口」)。
    let client = build_client(args.proxy.as_deref());
    let watch_url = format!("https://www.youtube.com/watch?v={video_id}");

    // 1) watch 页 → InnerTube key/版本/visitor + base.js;base.js → sts + Solver。
    eprintln!("[*] 抓取与解析(无浏览器)…");
    let Some(html) = yt_engine::fetch_text(&client, &watch_url, yt_engine::UA_WEB).await else {
        eprintln!("[!] 抓 watch 页失败");
        std::process::exit(1);
    };
    let Some(mut meta) = yt_engine::parse_watch_meta(&html) else {
        eprintln!("[!] 解析 watch 元数据失败");
        std::process::exit(1);
    };
    let Some(base_js) = yt_engine::fetch_text(&client, &meta.base_js_url, yt_engine::UA_WEB).await
    else {
        eprintln!("[!] 抓 base.js 失败");
        std::process::exit(1);
    };
    let sts = yt_engine::parse_sts(&base_js).unwrap_or(0);
    eprintln!(
        "    base.js={} | sts={sts}",
        meta.base_js_url.rsplit('/').nth(3).unwrap_or("?")
    );

    // 2) pot:--browser-pot 用 CDP 抓真实 pot+visitor 并切 WEB 客户端;否则用 --pot / 默认 TV。
    let mut kind = args.client;
    if args.browser_pot {
        match capture_browser_pot(&watch_url, args.proxy.as_deref()).await {
            Some((pot, visitor)) => {
                eprintln!(
                    "    [pot] 浏览器捕获 pot({}…) + visitor 透传,切 WEB 客户端",
                    &pot[..pot.len().min(12)]
                );
                args.pot = pot;
                if !visitor.is_empty() {
                    meta.visitor_data = visitor;
                }
                kind = Client::Web;
            }
            None => eprintln!("    [pot] 浏览器捕获失败,退回默认客户端"),
        }
    }

    // 3) InnerTube 取完整分轨(带 pot 时 WEB 才返回 adaptive URL)。
    // TVHTML5 偶发降级:只返回 progressive itag18,且其内容实测会**串到别的视频**(同 IP 频繁请求时
    // itag18 的 lmt 指向上一个视频)。正常响应应含 adaptive 分轨,故「无 adaptive video」时重试拿正常响应。
    let mut pr =
        match yt_engine::innertube_player(&client, &video_id, kind, &meta, sts, &args.pot).await {
            Some(p) => p,
            None => {
                eprintln!("[!] InnerTube player 请求失败");
                std::process::exit(1);
            }
        };
    let mut formats = yt_engine::list_formats(&pr);
    for attempt in 1..=4 {
        if args.quality == "audio" || formats.iter().any(|f| f.kind == yt_engine::Kind::Video) {
            break;
        }
        eprintln!(
            "    [~] InnerTube 降级(可用格式={},无 adaptive 分轨;TV progressive 可能串视频)→ 第 {attempt} 次重试…",
            formats.len()
        );
        tokio::time::sleep(std::time::Duration::from_millis(600)).await;
        if let Some(p2) =
            yt_engine::innertube_player(&client, &video_id, kind, &meta, sts, &args.pot).await
        {
            pr = p2;
            formats = yt_engine::list_formats(&pr);
        }
    }
    let status = pr
        .pointer("/playabilityStatus/status")
        .and_then(|v| v.as_str())
        .unwrap_or("?");
    let title = pr
        .pointer("/videoDetails/title")
        .and_then(|v| v.as_str())
        .unwrap_or("video")
        .to_string();
    eprintln!(
        "    playability={status} | 标题={} | 可用格式={}",
        trunc(&title, 48),
        formats.len()
    );
    if formats.is_empty() {
        eprintln!("[!] 无可用(带 url/cipher)格式——该视频可能需要 pot/登录;试 --browser-pot。");
        std::process::exit(1);
    }

    if args.list {
        print_table(&formats);
        return;
    }

    // 4) 选择 + QuickJS 解直链。
    let Some(plan) = yt_engine::select_plan(&formats, &args.quality) else {
        eprintln!("[!] 没有匹配清晰度 {} 的格式", args.quality);
        std::process::exit(1);
    };
    eprintln!("[*] 内嵌 QuickJS 跑 base.js,自算 sig/n 直链 …");
    let Some(solver) = Solver::new(&base_js) else {
        eprintln!("[!] QuickJS solver 初始化失败");
        std::process::exit(1);
    };
    if !solver.is_ready() {
        eprintln!("[!] solver 未就位(base.js 结构变化?)");
        std::process::exit(1);
    }

    // 只取直链(-g/--get-url):算好直链打印到 stdout(过程信息已走 stderr),不下载、不合并。
    // 脚本可 `… -g 2>/dev/null` 拿纯净 URL,直接喂播放器 / aria2c / ffmpeg。分轨则视频在前、音频在后。
    if args.get_url {
        match &plan {
            Plan::Single(f) => {
                let url = yt_engine::with_pot(&solver.url_for(f), &args.pot);
                if bad_url(&url) {
                    eprintln!("[!] 直链计算失败:{url}");
                    std::process::exit(1);
                }
                eprintln!(
                    "[*] itag={} {} {}(单条直链)",
                    f.itag,
                    kindlabel(f),
                    f.quality
                );
                show_bound_ip(&url);
                println!("{url}");
            }
            Plan::Mux(v, a) => {
                let vurl = yt_engine::with_pot(&solver.url_for(v), &args.pot);
                let aurl = yt_engine::with_pot(&solver.url_for(a), &args.pot);
                if bad_url(&vurl) || bad_url(&aurl) {
                    eprintln!(
                        "[!] 直链计算失败 v={} a={}",
                        trunc(&vurl, 40),
                        trunc(&aurl, 40)
                    );
                    std::process::exit(1);
                }
                show_bound_ip(&vurl);
                eprintln!(
                    "[*] 分轨直链:第1行=视频 itag={} {} {};第2行=音频 itag={} {}kbps {}(高清本就分轨;要单条可直接播放的直链用 -q 18)",
                    v.itag,
                    v.quality,
                    v.codec_short(),
                    a.itag,
                    a.bitrate / 1000,
                    a.codec_short()
                );
                println!("{vurl}");
                println!("{aurl}");
            }
        }
        return;
    }

    // 下载所需的「重新铸链」上下文(403 重试时复用):见 dl_retry 注释。
    let rm = Remint {
        video_id: video_id.clone(),
        kind,
        meta: &meta,
        sts,
        pot: args.pot.clone(),
    };

    let stem = args.output.unwrap_or_else(|| sanitize(&title));
    match plan {
        Plan::Single(f) => {
            let url = yt_engine::with_pot(&solver.url_for(&f), &args.pot);
            if bad_url(&url) {
                eprintln!("[!] 直链计算失败:{url}");
                std::process::exit(1);
            }
            show_bound_ip(&url);
            let out = format!("{stem}.{}", f.ext());
            println!(
                "[*] 下载 itag={} {} {} → {out}",
                f.itag,
                kindlabel(&f),
                f.quality
            );
            dl_retry(&client, &solver, &rm, f.itag, url, &out, "媒体").await;
            println!("[✅] 完成:{out}\n    {}", yt_engine::ffprobe_brief(&out));
        }
        Plan::Mux(v, a) => {
            let vurl = yt_engine::with_pot(&solver.url_for(&v), &args.pot);
            let aurl = yt_engine::with_pot(&solver.url_for(&a), &args.pot);
            if std::env::var("YT_DEBUG").is_ok() {
                println!("[debug] vurl={vurl}");
            }
            if bad_url(&vurl) || bad_url(&aurl) {
                eprintln!(
                    "[!] 直链计算失败 v={} a={}",
                    trunc(&vurl, 40),
                    trunc(&aurl, 40)
                );
                std::process::exit(1);
            }
            show_bound_ip(&vurl);
            let vtmp = format!("{stem}.v.{}", v.ext());
            let atmp = format!("{stem}.a.{}", a.ext());
            println!(
                "[*] 视频 itag={} {}{} ({})  +  音频 itag={} {}kbps ({})",
                v.itag,
                v.quality,
                if v.fps > 30 {
                    format!("{}fps", v.fps)
                } else {
                    String::new()
                },
                v.codec_short(),
                a.itag,
                a.bitrate / 1000,
                a.codec_short()
            );
            dl_retry(&client, &solver, &rm, v.itag, vurl, &vtmp, "视频").await;
            dl_retry(&client, &solver, &rm, a.itag, aurl, &atmp, "音频").await;
            if args.no_merge {
                println!("[✅] 已分开保存:{vtmp} + {atmp}(--no-merge)");
                return;
            }
            println!("[*] ffmpeg 合并(copy,不转码)…");
            match yt_engine::ffmpeg_merge(&vtmp, &atmp, &stem, &v, &a) {
                Ok(out) => {
                    let _ = std::fs::remove_file(&vtmp);
                    let _ = std::fs::remove_file(&atmp);
                    println!("[✅] 完成:{out}\n    {}", yt_engine::ffprobe_brief(&out));
                }
                Err(e) => eprintln!("[!] 合并失败:{e}(分轨保留:{vtmp} + {atmp})"),
            }
        }
    }
}

/// 建 reqwest client。给了代理就让**铸链 + 下载全程同一出口** —— 这是直链 `ip=` 对得上、且能
/// 「换出口 IP / 给别的设备复用」的关键(见文件头说明)。代理覆盖所有协议(`Proxy::all`):
/// 支持 `http://`、`https://`、`socks5://`、`socks5h://`(后者远端解析 DNS,代理后建议用它)。
fn build_client(proxy: Option<&str>) -> reqwest::Client {
    // 自定义 DNS 解析:只返回 IPv4(A)地址。WARP 这类网络 IPv6 出口**每连接抖动**、IPv4 出口稳定,
    // 只有都走 IPv4 才能把铸链与下载钉在同一出口,使直链 `ip=` 与下载出口一致(根治 adaptive 403)。
    // (`local_address` 只 bind 本地、挡不住 happy-eyeballs 选 IPv6,故必须从 DNS 层只给 A 记录。)
    struct Ipv4Only;
    impl reqwest::dns::Resolve for Ipv4Only {
        fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
            Box::pin(async move {
                let host = format!("{}:0", name.as_str());
                let addrs: Vec<std::net::SocketAddr> = tokio::net::lookup_host(host)
                    .await?
                    .filter(std::net::SocketAddr::is_ipv4)
                    .collect();
                Ok(Box::new(addrs.into_iter()) as reqwest::dns::Addrs)
            })
        }
    }
    let mut b = reqwest::Client::builder();
    // WARP 等代理常给 IPv6 分配「每连接不同的临时出口」(后缀抖动)→ 铸链(youtube)与下载
    // (googlevideo)落到不同 IPv6 出口 → 直链 `ip=` 对不上 → adaptive 间歇 403;而其 IPv4 出口
    // 通常是单一稳定地址。故 `YT_IPV4=1` 时只解析 A 记录,把铸链与下载钉在同一稳定 IPv4 出口。
    if std::env::var("YT_IPV4")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .is_some()
    {
        b = b.dns_resolver(std::sync::Arc::new(Ipv4Only));
        eprintln!(
            "[*] 强制 IPv4 出站(YT_IPV4):只解析 A 记录,治 WARP IPv6 出口抖动导致的 adaptive 403"
        );
    }
    if let Some(p) = proxy.filter(|s| !s.trim().is_empty()) {
        match reqwest::Proxy::all(p) {
            Ok(px) => {
                eprintln!("[*] 出口代理:{p}(铸链与下载全程同一出口 → 直链 ip= 即该出口)");
                b = b.proxy(px);
            }
            Err(e) => eprintln!("[!] 代理地址无效 {p}: {e} —— 忽略代理,走默认路由"),
        }
    }
    b.build().expect("reqwest")
}

/// 打印直链绑定的出口 `ip=`,让用户直观确认「出口已切到代理 / 这条链只能从该出口用」。
fn show_bound_ip(url: &str) {
    if let Some(ip) = yt_engine::param_of(url, "ip") {
        eprintln!(
            "    [出口] 直链绑定 ip={ip} —— 仅该出口可用;换设备/网络须用同一出口(同一代理)重铸"
        );
    }
}

/// 「重新铸链」所需的上下文(给 [`dl_retry`] 在 403 时重算直链用)。
struct Remint<'a> {
    video_id: String,
    kind: Client,
    meta: &'a yt_engine::WatchMeta,
    sts: u64,
    pot: String,
}

/// 下载某 itag,**HTTP 403 时自动重新铸链重试**。
///
/// 为什么需要:googlevideo 直链把铸链时所见的出口 IP 签进 `ip=`,下载出口必须一致。直连时两者天然
/// 相同;但走 **WARP / 轮换代理**时,铸链(youtube.com)与下载(googlevideo)可能落到不同出口 IP →
/// `ip=` 不符 → 403(间歇性)。重试 = 重新 InnerTube 铸一条新 `ip=` 的直链再下,碰上出口一致即成。
/// 最稳的根治是固定代理出口或直连——这属环境,不是签名/管线问题(同链 curl 同样会 403)。
async fn dl_retry(
    client: &reqwest::Client,
    solver: &Solver,
    rm: &Remint<'_>,
    itag: i64,
    first_url: String,
    path: &str,
    label: &str,
) {
    const TRIES: usize = 4;
    let mut url = first_url;
    for attempt in 1..=TRIES {
        match yt_engine::download_with_progress(client, &url, path, label).await {
            Ok(_) => return,
            Err(e) if e.contains("403") && attempt < TRIES => {
                eprintln!(
                    "\n[~] {label} 第 {attempt} 次 HTTP 403(代理出口与铸链 ip= 不一致?WARP/轮换代理常见)→ 退避后重新铸链重试…"
                );
                // 退避:若代理出口是按时间轮换的,隔几秒更可能撞上「铸链与下载同一出口」的窗口
                // (若是按目标主机粘连地分流 youtube/googlevideo,则只能固定出口/直连,见函数注释)。
                tokio::time::sleep(std::time::Duration::from_secs(attempt as u64)).await;
                match solve_fresh_url(client, solver, rm, itag).await {
                    Some(u) => url = u,
                    None => {
                        eprintln!("[!] 重新铸链失败(InnerTube 无该 itag?)");
                        std::process::exit(1);
                    }
                }
            }
            Err(e) => {
                eprintln!(
                    "\n[!] 下载失败({label}):{e}\n    受限视频可试 --browser-pot;若走 WARP/轮换代理建议固定出口或直连(403 多因出口 IP 与直链 ip= 不符)"
                );
                std::process::exit(1);
            }
        }
    }
}

/// 重新 InnerTube 铸链,取指定 itag 的新直链(`ip=`/`expire`/服务器都会换新)。
async fn solve_fresh_url(
    client: &reqwest::Client,
    solver: &Solver,
    rm: &Remint<'_>,
    itag: i64,
) -> Option<String> {
    let pr = yt_engine::innertube_player(client, &rm.video_id, rm.kind, rm.meta, rm.sts, &rm.pot)
        .await?;
    let f = yt_engine::list_formats(&pr)
        .into_iter()
        .find(|f| f.itag == itag)?;
    let u = yt_engine::with_pot(&solver.url_for(&f), &rm.pot);
    u.starts_with("http").then_some(u)
}

/// `--browser-pot`:无头 Chrome 打开视频,抓播放器真实 googlevideo 请求里的 `pot` + 页面 visitor_data。
#[cfg(feature = "cdp")]
async fn capture_browser_pot(watch_url: &str, proxy: Option<&str>) -> Option<(String, String)> {
    use drission::prelude::*;
    // 浏览器也走同一出口代理,保证 pot/visitor 与后续 InnerTube 铸链同源(出口一致)。
    let mut opts = ChromiumOptions::new().headless(true);
    if let Some(p) = proxy.filter(|s| !s.trim().is_empty()) {
        opts = opts.proxy(p);
    }
    let browser = ChromiumBrowser::launch(opts).await.ok()?;
    let tab = browser.new_tab(Some("about:blank")).await.ok()?;
    let listener = tab.listen();
    listener.start(&["videoplayback"]).await.ok()?;
    tab.get(watch_url).await.ok()?;
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    let _ = tab
        .run_js("setTimeout(function(){try{var v=document.querySelector('video');if(v){v.muted=true;v.play&&v.play()}}catch(e){}},0);1")
        .await;
    let visitor = tab
        .run_js("(window.ytcfg&&ytcfg.get&&ytcfg.get('VISITOR_DATA'))||(window.ytInitialPlayerResponse&&ytInitialPlayerResponse.responseContext&&ytInitialPlayerResponse.responseContext.visitorData)||''")
        .await
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default();
    let mut pot = String::new();
    for _ in 0..6 {
        let packets = listener
            .wait_count(10, Some(std::time::Duration::from_secs(2)))
            .await
            .unwrap_or_default();
        if let Some(p) = packets
            .iter()
            .find_map(|p| yt_engine::param_of(&p.url, "pot"))
        {
            pot = p;
            break;
        }
    }
    let _ = browser.quit().await;
    if pot.is_empty() {
        None
    } else {
        Some((pot, visitor))
    }
}

#[cfg(not(feature = "cdp"))]
async fn capture_browser_pot(_watch_url: &str, _proxy: Option<&str>) -> Option<(String, String)> {
    eprintln!("    [pot] --browser-pot 需要 cdp feature(--features signer,cdp)");
    None
}

fn print_table(formats: &[Format]) {
    println!("\n  itag  类型   清晰度        编码         码率      体积");
    println!("  ----  -----  ------------  -----------  --------  ---------");
    let mut fs: Vec<&Format> = formats.iter().collect();
    fs.sort_by_key(|f| std::cmp::Reverse((f.height, f.bitrate)));
    for f in fs {
        println!(
            "  {:<4}  {:<5}  {:<12}  {:<11}  {:>7}k  {:>9}",
            f.itag,
            kindlabel(f),
            if f.quality.is_empty() {
                format!("{}p", f.height)
            } else {
                f.quality.clone()
            },
            f.codec_short(),
            f.bitrate / 1000,
            f.human_size()
        );
    }
}

fn kindlabel(f: &Format) -> &'static str {
    match f.kind {
        Kind::Muxed => "音视",
        Kind::Video => "视频",
        Kind::Audio => "音频",
    }
}

fn bad_url(u: &str) -> bool {
    u.is_empty() || u.starts_with("ERR") || !u.starts_with("http")
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim()
        .replace(' ', "_")
        .chars()
        .take(60)
        .collect()
}

fn trunc(s: &str, n: usize) -> String {
    let t: String = s.chars().take(n).collect();
    if s.chars().count() > n {
        format!("{t}…")
    } else {
        t
    }
}
