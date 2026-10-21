# Lightpanda：Zig 难度，以及能不能用 Rust 复现

> 调研日期：2026-09-07。对照仓库 [`lightpanda-io/browser`](https://github.com/lightpanda-io/browser) `main`（持续更新）、官方架构文档、CEO 选型文、第三方拆解。  
> 本仓库现状：已经把 Lightpanda 当 **CDP 引擎变体** 用（`BrowserOptions::new().lightpanda()`），不是引擎本身。

## 结论（先看这个）

| 问题 | 答案 |
|---|---|
| Zig 难不难？ | **语言不难，浏览器难。** Zig 0.15 能学，难的是 DOM / Web API / V8 绑定 / 脚本时序 / CDP 子集。 |
| Rust 能不能复现？ | **能。** 底层积木比 Zig 还齐（html5ever 本来就是 Rust，V8 有 `rusty_v8`）。 |
| 该不该在 drission-rs 里复现？ | **现在不该。** 那是另开一家无头引擎公司的活，不是这个库的下一迭代。 |
| 对我们最值钱的用法？ | 继续 **拉官方二进制讲 CDP**。楚河那种「听接口、不靠真截图」已经对上了。 |

一句话：Lightpanda 不是「一个 Zig 写的 Playwright」，是「砍掉渲染管线的浏览器内核」。把内核用 Rust 再写一遍，语言换了，工作量几乎不减。

## Lightpanda 到底是什么

不是 Chromium fork，也不是 WebKit 补丁。单进程、永远无头、不画像素。

```
页面 HTML
   │
   ▼
libcurl 拉字节
   │
   ▼
html5ever（Rust）解析 ──► Zig 自己的 DOM（zigdom）
   │
   ▼
V8 跑 <script>，Web API 对象是 Zig 实现（document / fetch / XHR …）
   │
   ▼
lightpanda serve 对外讲 CDP（WebSocket :9222）
```

入口有好几层，内核是同一套：

| 命令 | 干什么 |
|---|---|
| `fetch` | 开一页，吐 HTML / Markdown / JSON，退出 |
| `serve` | CDP 服务器。Playwright / Puppeteer / **我们的 `ChromiumTab`** 都走这条 |
| `mcp` | 进程内 MCP，不经过 CDP |
| `agent` | 模型循环也塞进同一个进程 |

官方卖点（他们自己的数字，当量级看，不当实验室数据）：冷启动几十毫秒、单实例内存大约 Chrome headless 的 1/9、吞吐大约 9–11 倍。代价是 **没有排版、没有真截图、坐标点击不可靠**——这正好是我们文档里已经踩过的边界。

## 体量（为什么不是「翻译一晚上」）

对照 2026-09 的 GitHub 快照：

| 项 | 数 |
|---|---|
| 仓库 | `lightpanda-io/browser`，2023-02 开工，AGPL-3.0 |
| Star / 贡献 | ~3.5 万 star；核心提交人 karlseguin 3800+、krichprollsch 2100+、arrufat 1200+，后面还有一串百级提交 |
| 语言字节 | Zig **8.1 MB**，HTML 测试夹具 1.9 MB，Rust **135 KB**（html5ever FFI） |
| 源文件 | `src/` 下大约 **570 个 `.zig`** |
| Web API | `src/browser/webapi/` **七十多个模块** |
| CDP | `src/cdp/domains/` **17–18 个域**（Page / DOM / Runtime / Network / Fetch / Input / Target / Accessibility …） |
| 最大单文件 | `Page.zig` ~136 KB（页生命周期 + DOM 根 + 脚本时序） |
| Zig 版本 | **0.15.2**（还没 1.0，升版本会断） |

粗算：Zig 源大约 **15–20 万行量级**（含空行注释）。Chrome 是千万行，所以它「小」；对一个人或一个自动化库来说，它「大」。

团队自己也写了：Web API 有几百个，无头浏览器也是无底洞，覆盖只能慢慢加，用 WPT 子集验收。

## Zig 难度拆开看

CEO 原文标题就是 [*Why We Built Lightpanda in Zig*](https://lightpanda.io/blog/posts/why-we-built-lightpanda-in-zig)：「不够聪明，才不用 C++ / Rust」。选型逻辑值得当真，不是段子。

### 语言本身：中等偏易

Zig 比 Rust / C++ 薄：没有生命周期、没有 trait 宇宙、allocator 显式传入。会 C 的人几周能读他们的 `Page.zig`。真正烦的是：

1. **还没 1.0**。0.11 → 0.13 → 0.15 都断过生态。跟官方 nightly 走，自己 fork 要跟着升。
2. **生态小**。网络、解析、V8 几乎全靠 C / 别人的 crate。他们自己已经在用 Rust 的 html5ever。
3. **C++ 的 V8 没有 C API**。Zig 不能直接吃 V8 C++。他们用 Deno `rusty_v8` 生成的 C 头，再包一层 `src/browser/js/`（二十多个文件，`Local.zig` 单独 56 KB）。

### 他们选 Zig 的三个真理由（也是 Rust 复刻的痛点）

| Zig 吃香的点 | 在浏览器里干什么 | Rust 怎么对 |
|---|---|---|
| **显式 Arena** | 一页一块内存，关页整块扔。DOM 节点不单独 free | 必须 `slotmap` / 分代 arena / `bumpalo`，或大量 `Rc<RefCell>`。后者慢、易环 |
| **comptime 反射** | `registerType(Point)` 自动生成 V8 绑定，少写宏 | 过程宏能做，但是另一套语言；`rusty_v8` 已有样板 |
| **C interop** | `@cImport` 直接拉 libcurl / BoringSSL | `bindgen` + `cc`，能做，啰嗦一些 |

浏览器 + GC 运行时是教科书级「跟 borrow checker 打架」的场景：节点互相指、事件监听挂在树上、V8 wrapper 和 native 对象寿命缠在一起。Zig 不装安全，debug allocator 抓 UAF；Rust 要么索引当指针，要么 `unsafe` 写到跟 Zig 一样。

**所以：Zig 难度 ≠ 复刻难度。**  
复刻难度在 **语义**，不在关键字。

### 真正难的五块（换 Rust 一块都少不了）

1. **DOM + 事件**  
   他们从 LibDOM 迁到自研 zigdom，花了大约半年兼职原型。Shadow DOM、Custom Elements、捕获/冒泡、`MutationObserver`、Range，全是规范坑。

2. **Web API 表面**  
   `fetch` / XHR / Storage / IndexedDB / Crypto / Observers / Canvas 2D（半成品）……站点一复杂就缺 API。这是持续几年的清单，不是一个 milestone。

3. **脚本时序**  
   sync / defer / async、`document.write`、微任务/宏任务、iframe、模块 import。SPA（Vue / React）能不能跑，看的是这块，不是「链上了 V8」。

4. **CDP 子集**  
   协议本身不难（JSON-RPC + WebSocket）。难的是每个命令背后要有真引擎：`Runtime.evaluate` 要 V8，`DOM.querySelector` 要选择器引擎，`Input.dispatchMouseEvent` 没有 layout 就只能 stub。我们自己已经见过：`Page.captureScreenshot` 是占位图，`Target.createTarget` 可能丢初始 url。

5. **站点兼容**  
   官方明确：坐标点击、CSS 盒模型、WebGL、真 PDF，都不做或不完整。楚河能听价，是因为业务走 XHR + DOM，不靠像素。换个强指纹、强 layout 的站，还是得回 Chrome。

## Rust 复现：技术上可行，路径也清楚

### 积木对照（Rust 其实更现成）

| 层 | Lightpanda 现在 | Rust 直接能用的 |
|---|---|---|
| HTML 解析 | **html5ever（已经是 Rust，Zig 反向 FFI）** | 同库，省掉一层 C ABI |
| JS | V8 + zig-v8-fork（头文件来自 rusty_v8） | [`rusty_v8`](https://github.com/denoland/rusty_v8) / `deno_core`（一等公民） |
| HTTP | libcurl + nghttp2 + BoringSSL | `hyper` + `rustls`，或 `curl` crate |
| CDP WebSocket | 自研 Zig | `tokio-tungstenite` / axum |
| DOM | 自研 zigdom | **没有现成「无头 DOM + V8 绑定」**。Servo 太重（有 layout），不能整仓库搬 |
| Web API | 73 个 Zig 模块 | 必须自己写。这是工期 |

html5ever 已经在 Lightpanda 里了——他们并不是「纯 Zig 洁癖」，该借 Rust 就借。反过来说：用 Rust 复刻，解析层是送的。

### 三档目标（别一上来对齐全功能）

| 档 | 交付 | 粗工期（1 个熟 Rust + 浏览器的人，或 2 人小团队） | 对 drission 的价值 |
|---|---|---|---|
| **A. 玩具** | 拉 HTML、html5ever 建树、V8 `1+2`、CSS 选择器、`click` 派 DOM 事件、CDP 四个域（Target / Page / Runtime / DOM） | 2–4 个月能 demo | 证明链路，站点会碎 |
| **B. 听接口引擎** | A + `fetch`/XHR + cookie + 脚本顺序 + Network/Fetch 拦截 + 我们 `ChromiumTab` 主路径（`ele` / `run_js` / `listen`） | **9–18 个月** 才跟现在 nightly Lightpanda 在「听接口」场景接近 | 去掉对官方二进制的依赖，可做 Windows 原生 |
| **C. 对等 Lightpanda** | B + 七十个 Web API + WPT 子集 + MCP/agent + 他们现在的坑都填过 | **2–4 年**，还要一直跟规范 | 等于再办一家 AGPL 引擎公司 |

Chrome 兼容（真截图、layout、指纹）不在表里。那是第三条产品线，Lightpanda 自己都没做。

### Rust 里必须先定的架构（否则写到一半会推倒）

1. **节点用索引，不用 `&T` 满天飞。** `slotmap::SlotMap<NodeId, Node>` + 页级 bump。这是 Zig arena 的 Rust 写法。
2. **V8 绑定跟页 Arena 寿命对齐。** wrapper 用 weak / embedder field，关页整 isolate 或整 context 扔。不要每个 DOM 节点一个 `Arc`。
3. **事件循环一个线程泵。** curl multi / hyper + V8 microtask + timer，对标他们的 `Runner.zig`。tokio 可以，但 isolate 不要跨线程乱跳。
4. **CDP 只实现 `ChromiumTab` 已经打的命令。** 先对 `lightpanda_demo` + 楚河，再谈 Playwright 全集。
5. **验收用 WPT 子集 + 本仓库站点案例**，不要用「能 open example.com」当完成。

### 许可证红线

Lightpanda 是 **AGPL-3.0**。

- **对照设计、自己重写**：可以，保持我们自己的许可证。
- **对着 Zig 一行行翻译 / 复制绑定表 / 搬测试夹具**：视为衍生，整份引擎要 AGPL，还会污染链到它的二进制分发。
- 我们现在 **下载官方 nightly 当子进程**，只走 CDP，不链他们的代码，这条是干净的。

复刻必须 **clean-room**：规范 + WPT + 我们自己的站点，不把他们的 `webapi/*.zig` 当源文件抄。

## 对 drission-rs 的产品判断

本仓库已经做完的是「适配」，不是「引擎」：

- `src/cdp/lightpanda.rs`：定位 / 下载 / `serve`
- 同一套 `ChromiumBrowser` / `ChromiumTab`
- 已知缺口写在 [`Lightpanda.md`](Lightpanda.md)：截图占位、无 Windows 原生、UA 不能带 `Mozilla`

再写一个 Rust Lightpanda，**解决不了库侧的问题**——库侧问题是站点兼容和 CDP 语义差，换语言内核还是要填同样的坑。它只解决：

| 想要的 | Rust 复刻能给 | 更便宜的做法 |
|---|---|---|
| 服务器少占内存 | 能，前提 B 档做完 | **继续用官方二进制**（已经在用） |
| Windows 原生无头 | 能 | WSL `connect`，或这种场景直接 Chrome |
| 进程内嵌、不当子进程 | 能 | 价值很小，CDP 已经够快 |
| 避开 AGPL 内核 | 能（clean-room） | 不嵌入、只 spawn，已经避开了 |
| 自己修 `captureScreenshot` | 能，但要 layout | **不要修**；继续 `ele.image_bytes()` |
| 跟 Chrome 一样过强反爬 | **不能**（和无头内核无关） | 默认 Chrome / Camoufox |

**决策：不立项「Rust Lightpanda」。**  
继续把 Lightpanda 当可替换的 CDP 后端；缺口用 Chrome 补。若以后官方停更、协议分叉、或我们被 AGPL / 无 Windows 卡死，再按 B 档从 `rusty_v8` + html5ever 另起仓库，不要塞进 `drission` 主 crate。

## Fork 改功能 vs Rust 重构（2026-09-07）

问的是：对着他们的库改我们要的能力，难，还是 Rust 重写难？

**对你们已经踩过的缺口：fork 便宜一个数量级。Rust 重构只有在「不要他们的内核、只要自己的许可证/语言」时才值得。**

### 你们实际想改的，不是「再做一个浏览器」

对照本仓库已经绕过或写进文档的缺口：

| 缺口 | 现在怎么混 | 改引擎才算修好 |
|---|---|---|
| `captureScreenshot` 占位图 | `ele.image_bytes()` | 要 layout + 光栅化（跟他们「不渲染」对着干） |
| `createTarget` 丢初始 url | 附着后再 `Page.navigate` | 改 CDP Target 域，小补丁 |
| UA 拒 `Mozilla` | 跳过并打日志 | 删/放宽一处校验，小补丁 |
| Vue 点击打不穿 | 元素自己的 `click()` | 事件派发 / 默认动作，中等 |
| 无原生 Windows | WSL `connect` 或改回 Chrome | 交叉编译 + 他们没维护的平台 |
| 强反爬 / 真窗口 | 默认 Chrome | **不该改 Lightpanda** |

前四条里，三条是「在现成内核上打补丁」；真截图和 Windows 才重。Rust 重写对这六条全都更贵，因为先要有内核才能打补丁。

### 按改动类型比工期

| 改动 | Fork（Zig，站在现成 DOM/V8/CDP 上） | Rust 重构 |
|---|---|---|
| 放宽 UA、`createTarget` 带 url、补一个 CDP 字段 | **几天到两周**（先学目录） | 先花 9–18 个月把 B 档做出来，再改同一处 |
| Vue/SPA 点击、补一个 Web API（`crypto.subtle` 之类） | **2–8 周**，对着他们的 `webapi/` + WPT 加 | 同上，还要先有绑定层 |
| Windows 原生 `serve` | **1–3 个月** 赌一把（他们没发资产，坑在构建/V8/curl） | 理论上 Rust 更好编，但前提是引擎已存在 |
| 真截图 / PDF / 坐标点击 | **很难**：等于加排版+绘图像素。fork 也要新子系统 | **一样难**，还没有现成 DOM 可挂 |
| 「听接口」引擎整体对等 nightly | 不用做，已经有 | **9–18 个月** |

经验规则：**功能能挂在现有 Page / WebAPI / CDP 上 → fork 完胜。功能需要新的渲染管线 → 两条路都难，fork 仍少重写 15 万行。**

### Fork 的真实成本（别只看「改两行」）

1. **要学 Zig 0.15 + 他们的 arena / comptime 绑定。** 会 Rust 的人读代码不难，改对生命周期要上手。入门按 2–4 周算，不是改第一行之前的税。
2. **上游很快。** 2026-09 还在日更，核心人单年几千 commit。长期 fork 要么勤 rebase（Zig 小版本还会断），要么冻结某 tag、自己养，最后变成「慢吞吞的私有引擎」。
3. **AGPL-3.0。** fork 改的二进制，分发时源码义务跟着走。`dist/chuhe` 塞自己改过的 `lightpanda`，要比现在「下载官方 nightly」重一档合规。只自己服务器跑、不对外发，压力小很多。
4. **补丁若逆着设计（真截图、伪装 Chrome UA 过风控），上游不会收，rebase 会反复打。**

### Rust 重构看起来香的点，多数是错觉

- 「我们只会 Rust」——省的是语言税（大约一个月），省不掉 DOM/WebAPI/脚本时序。
- 「Windows 更好编」——对，但 18 个月后才享受。
- 「避开 AGPL」——对，clean-room 才算；翻译他们的 Zig 仍是 AGPL。
- 「一次做对，以后好改」——只有活过 B 档才成立。死在半路是最贵的选项。

### 决策

| 场景 | 选 |
|---|---|
| 修 `createTarget` / UA / 某个点击 / 补一个 API | **Fork 或直接给上游提 PR**（能进 main 就别养 fork） |
| 楚河继续听价、验证码取原图 | **不改引擎**，保持现在的适配 |
| 要真截图、坐标点击、强指纹 | **换 Chrome**，不要 fork 也不要重写 |
| 官方停更、协议分叉、AGPL 卡住对外分发 | 再谈 Rust B 档，另起仓库 |

默认顺序：**能 PR 就 PR → 不能合再浅 fork 钉 tag → 绝不先 Rust 重写。**

## 若以后非做不可：最小切片（备忘，现在不动手）

独立仓库，MIT/Apache 或我们自己的许可证，clean-room：

1. `serve`：WebSocket + Target/Page/Runtime/DOM 四个域  
2. html5ever 建树 + `querySelector`  
3. `rusty_v8` isolate + snapshot  
4. `fetch` / XHR 接到 hyper  
5. 用 `cargo run --example lightpanda_demo` 当第一个客户：example.com 的 title / `h1` / `1+2`  
6. 第二个客户才是楚河听价  

在这六步绿之前，不要谈 MCP、agent、Canvas、Windows 安装包。

## 参考

- 仓库：https://github.com/lightpanda-io/browser  
- 架构：https://lightpanda.io/docs/core-concepts/architecture-overview  
- 为什么用 Zig：https://lightpanda.io/blog/posts/why-we-built-lightpanda-in-zig  
- zigdom：https://lightpanda.io/blog/posts/migrating-our-dom-to-zig  
- 本仓库适配：[`Lightpanda.md`](Lightpanda.md)  
- 本仓库用法：`src/cdp/lightpanda.rs`、`examples/cdp/lightpanda_demo.rs`
