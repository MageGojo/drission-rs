//! 🎓 drission 上手 + 进阶 Rust 知识（一份可反复运行的「活教材」）
//!
//! 运行(默认 CDP 后端,无需任何 feature,完全离线——不联网):
//!   cargo run --example learn_basics
//!   HL=0 cargo run --example learn_basics     # 想看见浏览器窗口就设 HL=0
//!
//! 这个示例故意「完全离线」:不访问任何外网,而是用 `tab.set_content(html)` 把一段
//! 内置 HTML 灌进页面,再用库去操作它。这样你每次跑结果都一样,便于对照学习。
//!
//! 它分成 9 「课」,每一课都在**真实使用你的库**的同时,讲清一个 Rust 关键概念:
//!   ① async/await 与 Result/? ② 所有权(谁拥有浏览器)③ 元素句柄与 Arc 共享所有权
//!   ④ Vec 与迭代(& 借用遍历)⑤ Option 处理 ⑥ 把函数参数写成「借用」⑦ 闭包 + move + 并发
//!   ⑧ 生命周期 'a(本库真实出现的地方)⑨ Send / !Send(Arc vs Rc)
//!
//! 每课结尾有「🧠 一句话记忆」,专治「学完就忘」。

use drission::prelude::*;

// 一段内置的练习页面(纯 ASCII 结构,含中文文本没问题;set_content 会把它设成整页文档)。
const PAGE: &str = r#"<!doctype html>
<html><head><meta charset="utf-8"><title>drission 练习页</title></head>
<body>
  <h1 id="title">你好 drission</h1>
  <ul id="list">
    <li class="item">苹果</li>
    <li class="item">香蕉</li>
    <li class="item">橙子</li>
  </ul>
  <input id="kw" type="text" />
  <a id="link" href="https://apizero.cn">官网</a>
</body></html>"#;

// ─────────────────────────────────────────────────────────────────────────────
// 第 ① 课:程序入口 —— async / await 与 Result / ?
// ─────────────────────────────────────────────────────────────────────────────
//
// - `#[tokio::main]`:本库所有 IO 都是**异步**的(不会卡住线程)。异步函数返回的是一个
//   「future(未来的值)」,必须放进一个「运行时」里才会真正执行。`#[tokio::main]`
//   就是帮你把 `async fn main` 包进 tokio 运行时。
// - 返回值 `drission::Result<()>`:等价于 `Result<(), drission::Error>`。main 返回 Result,
//   才能在 main 里用 `?`。`?` 的意思是:「成功就取出里面的值继续;失败就直接 return 这个错误」。
#[tokio::main]
async fn main() -> drission::Result<()> {
    // 读环境变量决定有头/无头。map/unwrap_or 是 Option 的常用组合子(见第 ⑤ 课)。
    let headless = std::env::var("HL").map(|v| v != "0").unwrap_or(true);
    println!("==== drission 教学示例开始(headless={headless})====\n");

    // ─────────────────────────────────────────────────────────────────────────
    // 第 ② 课:所有权 —— 谁「拥有」这个浏览器?
    // ─────────────────────────────────────────────────────────────────────────
    //
    // `ChromiumOptions::new().headless(headless)` 是「builder(建造者)链式调用」:
    //   每个方法都是 `fn headless(mut self, ..) -> Self`,吃掉自己再吐出自己,所以能一直点下去。
    //
    // `let browser = ...launch(...).await?;` 之后,变量 `browser` 就**拥有(own)**了这个浏览器。
    //   Rust 的规则:一个值同一时刻只有一个「主人」。`browser` 就是主人。
    //   当 `browser` 在 main 结束时离开作用域,它的 `Drop` 会自动收尾(这就是 RAII)。
    let browser = ChromiumBrowser::launch(ChromiumOptions::new().headless(headless)).await?;

    // `new_tab` 借用 `&self`(不夺走所有权),返回一个我们**拥有**的 `ChromiumTab`。
    let tab = browser.new_tab(Some("about:blank")).await?;

    // 用内置 HTML 灌满页面(离线,不联网)。`&str` 是「字符串切片(借用)」,PAGE 的所有权还在常量里。
    tab.set_content(PAGE).await?;

    // 读页面标题。注意每个 IO 方法都要 `.await?`。
    let title = tab.title().await?;
    println!("[②] 页面标题 = {title:?}");
    // 🧠 一句话记忆:`let x = 值;` 就是「x 成为这个值的主人」;`&x` 是借出去、不转让。

    // ─────────────────────────────────────────────────────────────────────────
    // 第 ③ 课:元素句柄 —— 本库最关键的设计:用 Arc「共享所有权」,而不是生命周期借用
    // ─────────────────────────────────────────────────────────────────────────
    //
    // `tab.ele("#title")` 返回的是一个**你拥有的** `ChromiumElement`(值,不是 `&`借用)。
    // 你可能会疑惑:元素明明「属于」某个标签页,为什么它不是 `&'a` 借用 tab 呢?
    //
    // 看你库里的定义(src/cdp/element.rs 与 tab.rs):
    //     struct ChromiumTab     { core: Arc<CdpCore> }
    //     struct ChromiumElement { core: Arc<CdpCore>, object_id: String }
    //
    // 关键点:Tab 和 Element **各自持有一个 `Arc<CdpCore>`**。`Arc` = 原子引用计数的共享所有权:
    // clone 一下只是把计数 +1(很便宜),大家共享同一个底层内核 `CdpCore`,谁都不「借用」谁。
    // 好处:`el` 可以脱离 `tab` 独立存活、可以 clone、可以跨 `.await`、可以塞进别的任务(第 ⑦ 课)。
    // 代价:比纯借用多一次原子加减。对浏览器自动化这种「IO 才是大头」的场景,这点代价完全不值一提。
    let title_el = tab.ele("#title").await?;
    println!("[③] #title 文本 = {:?}", title_el.text().await?);
    println!("[③] #title 标签 = {:?}", title_el.tag().await?);

    // 因为是共享所有权,克隆一个句柄毫无心理负担(底层同一个 CdpCore,只是计数 +1):
    let title_el2 = title_el.clone();
    println!(
        "[③] 克隆的句柄仍指向同一元素,文本 = {:?}",
        title_el2.text().await?
    );
    // 🧠 一句话记忆:**异步世界里,本库用 `Arc` 共享所有权,不用生命周期借用**——所以句柄能到处传。

    // ─────────────────────────────────────────────────────────────────────────
    // 第 ④ 课:Vec 与迭代 —— `&` 借用遍历 vs 消费遍历
    // ─────────────────────────────────────────────────────────────────────────
    //
    // `eles` 返回 `Vec<ChromiumElement>`(一批你拥有的句柄)。
    let items = tab.eles("css:.item").await?;
    println!("[④] 找到 {} 个 .item", items.len());

    // `for it in &items`:用 `&items` 遍历是「借用」每个元素,循环结束后 `items` 还能继续用。
    //   若写 `for it in items`(没有 &),会「消费(move)」掉 items,之后就不能再用 items 了。
    for it in &items {
        // it 的类型是 &ChromiumElement;调方法时 Rust 会自动帮你处理 & 的层数(auto-ref/deref)。
        println!("    - {:?}", it.text().await?);
    }
    println!(
        "[④] 遍历用的是 &items,所以这里还能再用 items.len() = {}",
        items.len()
    );
    // 🧠 一句话记忆:能不夺走就别夺走 —— 优先 `for x in &v`,需要「用完就扔」时才 `for x in v`。

    // ─────────────────────────────────────────────────────────────────────────
    // 第 ⑤ 课:Option 处理 —— 「可能没有」这件事,类型系统逼你面对
    // ─────────────────────────────────────────────────────────────────────────
    //
    // `attr("href")` 返回 `Result<Option<String>>`:调用本身可能失败(Result),
    // 就算成功,该属性也「可能不存在」(Option)。别 unwrap!用 match / if let 处理两种情况。
    let link = tab.ele("#link").await?;
    match link.attr("href").await? {
        Some(href) => println!("[⑤] #link 的 href = {href}"),
        None => println!("[⑤] #link 没有 href 属性"),
    }

    // `ele_text` 是便捷方法:找不到元素时返回 `Ok(None)` 而不是报错,适合「有就用、没有就算」。
    if let Some(t) = tab.ele_text("#not-exist").await? {
        println!("[⑤] 不该出现:{t}");
    } else {
        println!("[⑤] #not-exist 不存在 → 得到 None(优雅,不 panic)");
    }
    // 🧠 一句话记忆:`Option` = 可能没有,`Result` = 可能失败;别 `unwrap()`,用 `match`/`if let`/`?`。

    // ─────────────────────────────────────────────────────────────────────────
    // 第 ⑥ 课:把「读取逻辑」抽成函数 —— 参数写成借用 `&`
    // ─────────────────────────────────────────────────────────────────────────
    //
    // 见下方 `dump_list` 的签名:`async fn dump_list(tab: &ChromiumTab, ...)`。
    // 它只是「用一下」tab,不需要拥有,所以收 `&ChromiumTab`(借用)。调用方 `&tab` 借出去,
    // 函数返回后 tab 依然是 main 的。这就是「借用让同一个东西能被多处安全地用」。
    let count = dump_list(&tab, "css:.item").await?;
    println!("[⑥] dump_list 通过借用 &tab 读到了 {count} 个 item(tab 还在 main 手里)");
    // 🧠 一句话记忆:函数「只想看/用一下」→ 收 `&T`;「要接管/存起来」→ 才收 `T`。

    // ─────────────────────────────────────────────────────────────────────────
    // 第 ⑦ 课:闭包 + move + 并发 —— 为什么「Arc 共享所有权」让并发变简单
    // ─────────────────────────────────────────────────────────────────────────
    //
    // 因为 `ChromiumTab` 是 `Clone`(内部就是 `Arc`)且可跨线程(Send,见第 ⑨ 课),
    // 我们可以 clone 一个句柄、用 `move` 把它「搬进」一个后台任务里并发执行。
    //   - 闭包 `async move { ... }`:`move` 表示把用到的变量(这里是 tab_bg)的**所有权移进闭包**。
    //   - 若不写 move,闭包只是借用外部变量,但后台任务可能比外部活得久 → 借用会失效,编译不过。
    //   - 这里能轻松 move,正是因为第 ③ 课说的:句柄是 Arc 共享所有权,clone 很便宜。
    let tab_bg = tab.clone();
    let handle = tokio::spawn(async move {
        // 这个块在另一个任务里跑;它「拥有」tab_bg。返回 drission::Result<String>。
        let title = tab_bg.title().await?;
        drission::Result::Ok(title)
    });
    // 主任务这边同时干别的活……然后 await 后台任务的结果(join)。
    let bg_title = handle.await.expect("后台任务不该 panic")?;
    println!("[⑦] 后台任务(move 进闭包的 tab 克隆)读到标题 = {bg_title:?}");
    // 🧠 一句话记忆:`move` 把变量所有权搬进闭包;有了 `Arc`(Clone+Send),并发只是「clone 再 move」。

    // ─────────────────────────────────────────────────────────────────────────
    // 第 ⑧ 课:生命周期 'a —— 你还没学的那个!它其实很朴素
    // ─────────────────────────────────────────────────────────────────────────
    //
    // 你翻遍上面的异步代码,几乎看不到 `<'a>`。这不是巧合:**本库在异步层刻意用 `Arc` 代替借用,
    // 就是为了让你(和它自己)不用跟生命周期缠斗**。生命周期真正出场,是在「同步 + 返回引用」的地方。
    //
    // 生命周期只回答一个问题:「这个返回的引用,借的是哪个输入?能活多久?」
    // 看下方 `first_word` 的签名:`fn first_word<'a>(s: &'a str) -> &'a str`。
    //   读作:返回的 &str,和参数 s **同寿**('a 把两者绑在一起)。所以只要 s 还在,返回值就有效。
    // 你库里就有一模一样的真实例子 —— src/locator.rs 的:
    //   fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str>
    //   注意:返回值绑的是 `s` 的 'a,而不是 `prefix` —— 因为切片是从 s 里切出来的。
    let sentence = "drission 很好用";
    let w = first_word(sentence);
    println!(
        "[⑧] first_word({sentence:?}) = {w:?}(返回值借用了 sentence,所以 sentence 得比它活得久)"
    );

    // 顺便看看库里那套 DP 选择器语法是怎么被解析的(parse_locator 来自 prelude):
    for sel in ["#kw", "@id:kw", "登录", "xpath://a"] {
        // Query 派生了 Debug,可直接 {:?} 打印。它把 "#kw" 解析成 Css、把 "登录" 解析成按文本的 Xpath。
        let q: Query = parse_locator(sel);
        println!("    parse_locator({sel:?}) = {q:?}");
    }
    // 🧠 一句话记忆:`'a` 不是「生命的意义」,只是「返回的引用 = 借了哪个输入、跟谁同寿」的标签。

    // ─────────────────────────────────────────────────────────────────────────
    // 第 ⑨ 课:Send / !Send —— Arc 能跨线程,Rc 不能(本库两处真实对照)
    // ─────────────────────────────────────────────────────────────────────────
    //
    // 「静态元素」`StaticElement` 是把某一刻的 HTML 快照**离线**解析后在内存里查询(不再连浏览器,极快)。
    // 它内部持有 `Rc<Html>`(见 src/static_element.rs)。`Rc` 是「单线程引用计数」,比 `Arc` 轻,
    // 但**不能跨线程**(它是 `!Send`)。所以:StaticElement 绝不要塞进 `tokio::spawn` 跨任务传,
    // 在**同一个任务里顺序用**完全没问题(下面就是)。
    let doc = StaticElement::parse(PAGE)?; // 同步!没有 .await —— 因为它根本不碰浏览器
    let li = doc.eles("css:.item")?; // 同步返回 Vec<StaticElement>
    println!("[⑨] StaticElement 离线解析到 {} 个 .item:", li.len());
    for e in &li {
        println!("    - {}", e.text()?); // 同步 text(),无 .await
    }
    //
    // 对照记忆:
    //   Tab / Element  → `Arc<CdpCore>` → 共享所有权 + Send(可跨任务)→ 异步、要 .await、连着浏览器
    //   StaticElement  → `Rc<Html>`     → 共享所有权 + !Send(不可跨线程)→ 同步、无 .await、纯内存
    // 🧠 一句话记忆:要跨线程就 `Arc`(Send);只在单线程图轻量就 `Rc`(!Send)。

    // 收尾:关闭浏览器。`browser` 的 Drop 也会兜底清理,但显式 quit 更干净、更可控。
    browser.quit().await?;
    println!("\n==== 全部 9 课完成,你已经把这个库跑通了一遍 ====");
    Ok(())
}

// ── 第 ⑥ 课用:参数收 `&ChromiumTab`(借用),只读不夺所有权 ──────────────────────
/// 打印某个选择器命中的所有元素文本,返回命中数量。
///
/// 参数写成 `&ChromiumTab`:我们只是「借来用一下」,函数结束把 tab 还给调用方。
async fn dump_list(tab: &ChromiumTab, selector: &str) -> drission::Result<usize> {
    let eles = tab.eles(selector).await?;
    for (i, e) in eles.iter().enumerate() {
        // enumerate() 给出 (下标, 元素) 对;i 从 0 开始。
        println!("    [{i}] {:?}", e.text().await?);
    }
    Ok(eles.len())
}

// ── 第 ⑧ 课用:一个带显式生命周期的纯函数(和库里 locator::strip_prefix_ci 同款套路)──
/// 返回 `s` 的第一个「单词」(以空白分隔)。没有单词时返回空串。
///
/// `<'a>` 声明一个生命周期参数;`s: &'a str -> &'a str` 表示:
/// 返回的切片借用自 `s`,与 `s` 同寿 —— 编译器据此保证你不会拿到一个「悬垂引用」。
fn first_word<'a>(s: &'a str) -> &'a str {
    match s.trim_start().split_whitespace().next() {
        Some(w) => w,
        None => "",
    }
}
