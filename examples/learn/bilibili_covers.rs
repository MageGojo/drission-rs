//! 🎓 实战第 ② 课:用 drission 爬「B 站热门视频的封面图」——边做业务边学 Rust
//!
//! 运行(默认 CDP 后端 / Google Chrome,无需任何 feature;这一课**要联网**访问 bilibili):
//!   cargo run --example bilibili_covers
//!   HL=0 cargo run --example bilibili_covers                 # 想看见浏览器窗口就设 HL=0
//!   cargo run --example bilibili_covers -- "https://www.bilibili.com/v/popular/all" 12
//!                                             └── 目标页 ──┘  └ 抓几张封面 ┘
//!
//! 产物:封面图 + 一份清单 manifest.json 落到  ./target/bilibili_covers/。
//!
//! 与「第 ① 课 learn_basics(完全离线)」不同,这一课是**真实业务**:打开 B 站热门页 →
//! 等页面渲染 → 滚动触发懒加载 → 用**库的元素 API** 把每张卡片的「标题 + 封面 URL」抓出来 →
//! 清洗 URL → **并发下载**图片 → 落盘 + 导出清单。整条链路只有「点哪、读哪个字段」是 B 站业务,
//! 其余全是库能力(`eles`/`attr`/`text`/`scroll`/`wait`/`fetch_image`)。
//!
//! 这一课新学的 Rust(每步结尾都有「🧠 一句话记忆」):
//!   ① 自定义 struct 承载业务数据 ② 迭代器 filter_map/enumerate 收成 Vec
//!   ③ Option/Result 双重「可能没有/可能失败」的实战处理 ④ HashSet 去重
//!   ⑤ &str 切片做 URL 清洗(零拷贝)⑥ tokio 并发下载(spawn + JoinHandle + 分批限流)
//!   ⑦ 闭包 move 把所有权搬进任务 ⑧ 文件 IO 与用 HashMap 导出 JSON 清单

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;

use drission::prelude::*;

// ─────────────────────────────────────────────────────────────────────────────
// 业务数据模型:一条「封面记录」。
// ─────────────────────────────────────────────────────────────────────────────
//
// `struct` 是 Rust 组织数据的基本手段(相当于其它语言的「类的字段部分」)。
// `#[derive(Debug, Clone)]` 自动生成两种能力:
//   - Debug:能用 `{:?}` 打印(调试友好);
//   - Clone:能 `.clone()` 复制一份(第 ⑦ 步把它 move 进并发任务时要用)。
#[derive(Debug, Clone)]
struct Cover {
    /// 在列表里的序号(第几个),从 1 开始,纯粹为了给文件命名 / 打印好看。
    rank: usize,
    /// 视频标题(可能抓不到 → 那就给个占位,见第 ③ 步)。
    title: String,
    /// 封面图的最终可下载 URL(已补好 https: 前缀、去掉 B 站的 @压缩后缀)。
    url: String,
}

#[tokio::main]
async fn main() -> drission::Result<()> {
    // 只把库内部的告警打出来,免得日志刷屏(和长监听示例一致的习惯)。
    tracing_subscriber::fmt().with_env_filter("warn").init();

    // ── 读命令行参数:目标页 + 想抓几张 ──────────────────────────────────────
    // `std::env::args()` 第 0 个是程序名,`.skip(1)` 跳过它。这里演示 Option 的组合子:
    //   `.next()`            → Option<String>(可能没传)
    //   `.unwrap_or_else(..)` → 没传就用默认值(闭包里现造,惰性求值)
    //   `.and_then(|s| s.parse().ok())` → 传了但解析失败也当没传
    let mut args = std::env::args().skip(1);
    let target = args
        .next()
        .unwrap_or_else(|| "https://www.bilibili.com/v/popular/all".to_string());
    let want: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(10);

    // HL=0 → 有头(能看见窗口);默认无头。map/unwrap_or 是 Option 常用组合。
    let headless = std::env::var("HL").map(|v| v != "0").unwrap_or(true);
    println!(
        "==== 实战第 ② 课:B 站封面爬取 ====\n目标页 = {target}\n想抓 = {want} 张  headless = {headless}\n"
    );

    // ─────────────────────────────────────────────────────────────────────────
    // 第 ① 步:启动浏览器 + 开一个标签页导航过去
    // ─────────────────────────────────────────────────────────────────────────
    //
    // builder 链式配置:设成中文/东八区,让 B 站给我们「国内正常版」页面。
    // `browser` 拥有这个浏览器(所有权);main 结束时它的 Drop 自动收尾(RAII)。
    let browser = ChromiumBrowser::launch(
        ChromiumOptions::new()
            .headless(headless)
            .window_size(1440, 900)
            .locale("zh-CN")
            .timezone("Asia/Shanghai"),
    )
    .await?;

    // `new_tab` 借用 `&browser`,返回一个我们**拥有**的标签页。
    let tab = browser.new_tab(Some(&target)).await?;

    // `get` 返回 `bool`(超时为 false,不报错)——这是本库刻意的设计:导航「没在超时内 load 完」
    // 常常还能继续干活,不该直接 Err 打断你。我们只打印一下,不因 false 就退出。
    let loaded = tab.get(&target).await?;
    println!("[①] 打开页面完成 loaded={loaded}");

    // ─────────────────────────────────────────────────────────────────────────
    // 第 ② 步:等首屏卡片渲染 + 滚动触发「懒加载」
    // ─────────────────────────────────────────────────────────────────────────
    //
    // B 站是 SPA:HTML 骨架先到,视频卡片是 JS 异步渲染进来的;而且封面图**懒加载**——
    // 不滚到视口附近就不给你把真实地址塞进 <img src>。所以两件事必须做:
    //   1) 先 `wait().ele_displayed` 等某个卡片真的出现(别一 get 完就抓,会抓到空);
    //   2) 循环往下滚几屏,把更多封面「喂」进加载区。
    //
    // `wait().ele_displayed(sel, Some(超时))` 返回 bool:出现了就 true。这里用「多选择器兜底」:
    // B 站不同页面/改版卡片类名不一样,`.bili-video-card` 是常见的一种。
    let appeared = tab
        .wait()
        .ele_displayed(
            ".video-card, .bili-video-card",
            Some(Duration::from_secs(20)),
        )
        .await?;
    println!("[②] 首屏卡片出现 = {appeared}");

    // 滚动是「控制流」练习:固定滚 6 屏,每屏之间 sleep 让图片有时间加载。
    // CDP 后端没有 camoufox 的 `wait().secs()`,等待就用 `tokio::time::sleep`(见 SKILL gotcha #7)。
    for round in 1..=6 {
        // `scroll().by(dx, dy)` 相对滚动;这里每次往下滚将近一屏。
        tab.scroll().by(0.0, 1400.0).await?;
        tokio::time::sleep(Duration::from_millis(700)).await;
        print!("\r[②] 滚动加载中… {round}/6");
        use std::io::Write;
        let _ = std::io::stdout().flush();
    }
    println!();
    // 🧠 一句话记忆:SPA + 懒加载 → 先 `wait().ele_displayed` 等渲染,再滚动喂图,别急着抓。

    // ─────────────────────────────────────────────────────────────────────────
    // 第 ③ 步:用「库的元素 API」把每张卡片的标题 + 封面抓出来(主力)
    // ─────────────────────────────────────────────────────────────────────────
    //
    // `eles` 返回 `Vec<ChromiumElement>`——一批我们拥有的元素句柄(内部是 Arc 共享所有权,见第 ① 课③)。
    // `.video-card` 是热门页(/v/popular/all)每张视频卡片的类;`.bili-video-card` 是首页版式,兜底。
    let cards = tab.eles(".video-card, .bili-video-card").await?;
    println!("[③] 定位到 {} 张卡片,开始逐张读取…", cards.len());

    // 用一个可增长的 Vec 承载结果;HashSet 给封面 URL 去重(B 站同一封面可能出现在多个卡片)。
    let mut covers: Vec<Cover> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    // `for (i, card) in cards.iter().enumerate()`:
    //   - `cards.iter()` 是**借用**遍历(循环后 cards 还能用,见第 ① 课④);
    //   - `enumerate()` 给出 `(下标, &元素)`,下标从 0 起。
    for (i, card) in cards.iter().enumerate() {
        // 一张卡片可能没有封面 <img>(占位卡/广告位)→ 用 `extract_cover` 返回 Option,None 就跳过。
        // 这一步同时体现「Result(可能失败)」与「Option(可能没有)」两层:元素查询会失败(? 传播),
        // 而「有没有 src」是 Option。我们在 helper 里把二者收敛成一个 Option<String>。
        let Some(raw) = extract_cover(card).await else {
            continue; // let-else:拿不到就跳过这张卡,继续下一张(优雅早退)
        };

        // 清洗 URL(见 helper):`//i0.hdslb.com/...jpg@672w` → `https://i0.hdslb.com/...jpg`。
        let url = clean_cover_url(&raw);

        // 去重:HashSet::insert 返回 bool(true=首次见到)。已见过就跳过。
        if !seen.insert(url.clone()) {
            continue;
        }

        // 标题:抓不到不致命,给个占位(Option → unwrap_or 一个默认串)。
        let title = extract_title(card)
            .await
            .unwrap_or_else(|| format!("视频_{}", i + 1));

        covers.push(Cover {
            rank: covers.len() + 1,
            title,
            url,
        });
        if covers.len() >= want {
            break; // 够了就停,不用把整页读完
        }
    }

    // 库 API 抓不到(比如 B 站又改版了类名)→ JS 兜底扫一遍全页 <img>。
    // 这是真实工程里的稳健写法:**结构化 API 为主,JS 兜底为辅**。
    if covers.is_empty() {
        println!("[③] 元素 API 没抓到,启用 JS 兜底扫描全页封面…");
        covers = extract_covers_by_js(&tab, want).await?;
    }

    println!("[③] 最终得到 {} 张待下载封面:", covers.len());
    for c in &covers {
        // {:>2} 右对齐占 2 位;title 太长截断,避免刷屏。
        let short: String = c.title.chars().take(24).collect();
        println!("   #{:>2}  {short}", c.rank);
    }
    if covers.is_empty() {
        println!("没抓到封面。可能是网络/改版;试试加 HL=0 看页面,或换 -- <URL> <数量>。");
        browser.quit().await?;
        return Ok(());
    }
    // 🧠 一句话记忆:`for x in v.iter().enumerate()` 借用遍历带下标;单条失败用 `let-else`/`continue` 跳过,别让一条脏数据搞崩整批。

    // ─────────────────────────────────────────────────────────────────────────
    // 第 ④ 步:并发下载封面(tokio::spawn + 分批限流)
    // ─────────────────────────────────────────────────────────────────────────
    //
    // 串行下载 N 张会很慢(每张都在等网络)。用 tokio 并发:把每张的下载丢进一个 task 同时跑。
    // 但也不能一次性全放出去(几百张会打爆对端 / 本地),所以**分批**:每批最多 CONCURRENCY 个,
    // 一批 join 完再放下一批。这就是最朴素的「限流并发」。
    let out_dir = PathBuf::from("target/bilibili_covers");
    tokio::fs::create_dir_all(&out_dir).await?; // 目录不存在就建(父目录一起建)
    const CONCURRENCY: usize = 4;

    let mut ok = 0usize;
    // `covers.chunks(CONCURRENCY)` 把 Vec 切成一段段 `&[Cover]`,每段并发跑。
    for batch in covers.chunks(CONCURRENCY) {
        // 每批:为每条造一个 task,收集它们的 JoinHandle。
        let mut handles = Vec::new();
        for cover in batch {
            // 关键:task 可能比这轮循环活得久,所以要把数据**拥有**进 task。
            //   - `cover.clone()`:复制这条记录(struct 派生了 Clone);
            //   - `out_dir.clone()`:复制路径。
            // `async move { .. }` 里的 `move` 把这些克隆的所有权**搬进**闭包(见第 ① 课⑦)。
            let cover = cover.clone();
            let dir = out_dir.clone();
            handles.push(tokio::spawn(async move {
                // `fetch_image` 是**后端无关**的自由函数(prelude 里就有):服务端直拉图片字节,
                // 不走浏览器、不受跨域 taint 影响——正适合把封面原图拉下来。
                let bytes = fetch_image(&cover.url).await?;
                let path = dir.join(file_name_for(&cover));
                tokio::fs::write(&path, &bytes).await?; // io::Error 会经 `?` 自动转成 drission::Error
                // 显式标注返回类型,spawn 的闭包才知道 Ok/Err 各是什么。
                drission::Result::Ok((path, bytes.len()))
            }));
        }
        // join 这一批:`handle.await` 得到 `Result<任务返回, JoinError>`,任务返回本身又是 `Result`。
        // 于是要「剥两层」:外层 JoinError(任务 panic 了?),内层业务 Result(下载/写盘成功?)。
        for h in handles {
            match h.await {
                Ok(Ok((path, n))) => {
                    ok += 1;
                    println!("   ✓ {} ({} 字节)", path.display(), n);
                }
                Ok(Err(e)) => println!("   ✗ 下载/写盘失败: {e}"),
                Err(e) => println!("   ✗ 任务异常: {e}"),
            }
        }
    }
    println!("\n[④] 下载完成:{ok}/{} 成功。", covers.len());
    // 🧠 一句话记忆:并发 = clone 数据 + `async move` 搬进 `tokio::spawn`;`chunks(N)` 做限流;`h.await` 要剥「JoinError」和「业务 Result」两层。

    // ─────────────────────────────────────────────────────────────────────────
    // 第 ⑤ 步:导出清单 manifest.json(用库自带的 scrape 导出)
    // ─────────────────────────────────────────────────────────────────────────
    //
    // `drission::scrape::write_json` 接收 `&[HashMap<String,String>]`:一条记录 = 一个 map。
    // 我们把每个 Cover 变成一个 map(iterator map + collect 是 Rust 里最常见的「变形」写法)。
    let records: Vec<HashMap<String, String>> = covers
        .iter()
        .map(|c| {
            let mut m = HashMap::new();
            m.insert("rank".to_string(), c.rank.to_string());
            m.insert("title".to_string(), c.title.clone());
            m.insert("cover_url".to_string(), c.url.clone());
            m // 闭包最后一个表达式即返回值(无分号)
        })
        .collect();

    let manifest = out_dir.join("manifest.json");
    drission::scrape::write_json(&manifest, &records).await?;
    println!("[⑤] 清单已写出:{}", manifest.display());

    // 收尾:显式关浏览器更干净(Drop 也会兜底)。
    browser.quit().await?;
    println!(
        "\n==== 实战第 ② 课完成:封面 + 清单都在 {} ====",
        out_dir.display()
    );
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// 辅助函数们:参数都收 `&ChromiumElement` / `&str`(借用,不夺所有权,见第 ① 课⑥)
// ─────────────────────────────────────────────────────────────────────────────

/// 从一张卡片里取封面图 URL。
///
/// 返回 `Option<String>`:抓不到(没有 img / 没有可用 src)就 `None`,让调用方 `continue` 跳过。
/// 内部把「查询失败(Result::Err)」也收敛成 `None`——因为对本业务来说,「这张卡没有封面」
/// 和「查询这张卡的 img 出错」都只意味着「跳过它」,不必区分。用 `.ok()?` 一步搞定。
async fn extract_cover(card: &ChromiumElement) -> Option<String> {
    // 卡片内相对查询第一个 <img>;`.ok()?` = Err→None 直接返回、Ok→取出继续。
    let img = card.ele("css:img").await.ok()?;
    // B 站封面可能落在 src,也可能懒加载放在 data-src;依次尝试。
    // `attr` 返回 `Result<Option<String>>`:先 `.ok()?` 剥掉 Result,再看 Option。
    for name in ["src", "data-src", "data-original"] {
        if let Ok(Some(v)) = img.attr(name).await {
            let v = v.trim();
            // 懒加载占位图(base64/透明 gif)不要;要真实的 http(s)/协议相对(//)地址。
            if !v.is_empty()
                && !v.starts_with("data:")
                && (v.starts_with("http") || v.starts_with("//"))
            {
                return Some(v.to_string());
            }
        }
    }
    None
}

/// 从一张卡片里取标题:先试标题元素的文本,再退而求其次找 `title`/`alt` 属性。
async fn extract_title(card: &ChromiumElement) -> Option<String> {
    // 常见标题容器;不同版式类名不同,给几个候选。text() 拿可见文本。
    // `.video-name` 是热门页(/v/popular/all)的标题类;`.bili-video-card__info__tit` 是首页版式。
    for sel in [
        ".video-name",
        ".bili-video-card__info__tit",
        ".title",
        "h3",
        "a[title]",
    ] {
        if let Ok(el) = card.ele(sel).await {
            if let Ok(t) = el.text().await {
                let t = t.trim();
                if !t.is_empty() {
                    return Some(t.to_string());
                }
            }
            // 文本空就试它的 title 属性(B 站标题常挂在链接的 title 上)。
            if let Ok(Some(t)) = el.attr("title").await {
                if !t.trim().is_empty() {
                    return Some(t.trim().to_string());
                }
            }
        }
    }
    None
}

/// 清洗封面 URL:补协议 + 去掉 B 站的 `@` 压缩后缀,拿到原图地址。
///
/// 演示「用 &str 切片做零拷贝处理」:`split_once`/`starts_with` 都不复制原串,
/// 只在最后要「拥有一个 String」时才 `to_string()`。
fn clean_cover_url(raw: &str) -> String {
    // ① 协议相对地址 `//i0.hdslb.com/...` → 补上 `https:`。
    let with_scheme: String = if let Some(rest) = raw.strip_prefix("//") {
        format!("https://{rest}")
    } else {
        raw.to_string()
    };
    // ② B 站图片 CDN 用 `@` 后缀表示裁剪/压缩参数(如 `xxx.jpg@672w_378h_1c.webp`);
    //    截到 `@` 之前就是原图(通常是 .jpg)。`split_once('@')` 返回 Option<(前, 后)>。
    match with_scheme.split_once('@') {
        Some((base, _params)) => base.to_string(),
        None => with_scheme,
    }
}

/// 给封面生成一个安全的文件名:`001_标题片段.jpg`。
///
/// 文件名不能带 `/`、`?`、空格等,否则写盘会出错或路径歧义——所以做「白名单清洗」:
/// 只保留中日韩汉字、字母、数字,其余一律换成 `_`。
fn file_name_for(c: &Cover) -> String {
    let safe: String = c
        .title
        .chars()
        .take(20) // 标题太长只取前 20 字
        .map(|ch| if ch.is_alphanumeric() { ch } else { '_' })
        .collect();
    // 从 URL 末尾猜扩展名(默认 jpg)。
    let ext = if c.url.ends_with(".png") {
        "png"
    } else if c.url.ends_with(".webp") {
        "webp"
    } else {
        "jpg"
    };
    format!("{:03}_{safe}.{ext}", c.rank)
}

/// JS 兜底提取器:当库的元素 API 抓不到时(改版/类名变了),直接在页面里扫一遍。
///
/// 这里用 `run_js` 执行一段 JS,让它 `JSON.stringify(...)` 返回**字符串**,我们再用 `serde_json`
/// 解析——这和库内部 `image_view` 的套路一致(不依赖 CDP 是否按值序列化对象,最稳)。
/// 教学点:`run_js` 返回 `serde_json::Value`;取字段用 `v["k"].as_str()` 这种「按需取值」写法。
async fn extract_covers_by_js(tab: &ChromiumTab, want: usize) -> drission::Result<Vec<Cover>> {
    // 扫全页图片:凡是 B 站封面 CDN(hdslb.com 且路径含 /bfs/)的 <img>,连同最近的标题一起收集。
    let js = r#"
        (() => {
          const out = [];
          const seen = new Set();
          for (const img of document.querySelectorAll('img')) {
            let src = img.currentSrc || img.src || img.getAttribute('data-src') || '';
            if (!src || src.startsWith('data:')) src = img.getAttribute('data-src') || '';
            if (!src || src.startsWith('data:')) continue;
            if (!src.includes('hdslb.com') || !src.includes('/bfs/archive/')) continue; // 只要视频封面(排除导航/头图)
            if (seen.has(src)) continue; seen.add(src);
            // 就近找标题:img 往上找卡片容器,再取里面的标题/链接 title/alt。
            const card = img.closest('.bili-video-card, .video-card, .card-box') || img.parentElement;
            let title = '';
            if (card) {
              const t = card.querySelector('.bili-video-card__info__tit, .title, h3, a[title]');
              title = (t && (t.getAttribute('title') || t.textContent || '')).trim();
            }
            title = title || img.alt || '';
            out.push({ src, title });
          }
          return JSON.stringify(out);
        })()
    "#;
    let value = tab.run_js(js).await?;
    // run_js 给回的是一个「内容为 JSON 文本」的字符串值;取出来再解析成数组。
    let text = value.as_str().unwrap_or("[]");
    let arr: serde_json::Value = serde_json::from_str(text).unwrap_or(serde_json::Value::Null);

    let mut covers = Vec::new();
    if let Some(items) = arr.as_array() {
        for (i, it) in items.iter().enumerate() {
            let Some(src) = it["src"].as_str() else {
                continue;
            };
            let title = it["title"].as_str().unwrap_or("").trim();
            let title = if title.is_empty() {
                format!("视频_{}", i + 1)
            } else {
                title.to_string()
            };
            covers.push(Cover {
                rank: covers.len() + 1,
                title,
                url: clean_cover_url(src),
            });
            if covers.len() >= want {
                break;
            }
        }
    }
    Ok(covers)
}
