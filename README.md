# drission · Rust 反检测浏览器自动化 + 内置验证码识别(OCR / 滑块缺口距离)

[![crates.io](https://img.shields.io/crates/v/drission.svg)](https://crates.io/crates/drission)
[![docs.rs](https://docs.rs/drission/badge.svg)](https://docs.rs/drission)
[![Rust](https://img.shields.io/badge/rust-1.85%2B-orange.svg)](https://www.rust-lang.org)
[![platform](https://img.shields.io/badge/platform-macOS%20%7C%20Linux%20%7C%20Windows-blue.svg)](#-支持的平台与浏览器)
[![GitHub](https://img.shields.io/badge/GitHub-MageGojo%2Fdrission--rs-181717?logo=github)](https://github.com/MageGojo/drission-rs)
[![GitCode](https://img.shields.io/badge/GitCode-Roufsi%2Fdrission--rs-c71d23)](https://gitcode.com/Roufsi/drission-rs)

[English](README.en.md) · **简体中文** · 仓库:[GitHub](https://github.com/MageGojo/drission-rs) · [GitCode](https://gitcode.com/Roufsi/drission-rs)

> **drission 是一个用 Rust 编写的高性能浏览器自动化库**:**默认开箱驱动 Google Chrome**(Chromium / CDP,
> 也支持 Edge / Brave / Chromium / Electron),**一行开启** [Camoufox](https://github.com/daijro/camoufox)(Firefox 反检测内核)
> 及其全部高级能力——**内置字符验证码 OCR**(ddddocr 模型 · 纯 Rust 推理)、**图片滑块缺口距离识别**(极验 / 顶象)、
> 自动过 Cloudflare 盾、XHR 监听 / 拦截、高并发浏览器池。API 语法对齐 [DrissionPage](https://github.com/g1879/DrissionPage),面向高并发爬虫与自动化。
>
> *drission is a high-performance browser-automation library in Rust: drives **Google Chrome (CDP) by default**, with Camoufox/Firefox
> anti-detect available via one feature flag — **built-in captcha OCR**, **image slider-gap recognition**, async high-concurrency
> crawling, XHR listen/intercept and Cloudflare bypass — with a DrissionPage-style API.*

本库由 **极数本源([apizero.cn](https://apizero.cn))** 出品与维护,是其自动化与数据采集技术栈的一部分。
如果你在找「**Rust Chrome 自动化 / 验证码识别 / 滑块缺口距离 / 反检测浏览器 / 高并发爬虫**」的一站式方案,这里是答案。

> **与众不同之处**:Rust 生态里的浏览器自动化库(如 `zendriver-rs`、`rust_drission`、`stygian-browser` 等)验证码普遍依赖第三方打码服务
> (capsolver / 2captcha)。**drission 默认开箱即用驱动 Google Chrome,并内置离线验证码识别(ddddocr OCR + 图片滑块缺口距离)、
> 一行切换 Camoufox / Firefox 反检测内核**——无需联网打码,是「**Rust 版 DrissionPage**」里少有的自带打码方案。

> 🤖 **给 AI 编程助手**:若你是 AI,要**基于本库写代码**,请先阅读并严格遵循 [**`docs/SKILL.md`**](docs/SKILL.md)
> ——本库接口 / feature / 构建规则的**权威速查**,覆盖从最基础用法到**点选验证码点击**全流程,照它写即可一次编译通过、运行正确。
> 若你的环境不支持 "skill" 机制,可忽略此强制条款,但仍强烈建议据此使用本库。

---

## 📖 这是什么(一句话看懂)

**drission = Rust 版的 DrissionPage + 内置打码(OCR / 滑块)+ 反检测过盾。** 用一套 `tokio` 异步 API 同时拿下:

- **浏览器自动化**:启动 / 接管反检测浏览器,像写 DrissionPage 一样定位元素、点击输入、抓包改包。
- **验证码识别**:字符验证码离线 OCR、滑块缺口距离计算 + 拟人轨迹,**不依赖第三方打码平台、无需联网**。
- **反检测与过盾**:指纹定制、`navigator.webdriver=false`、自动通过 Cloudflare Turnstile。
- **工程化采集**:高并发浏览器池、代理 / 指纹轮换、断点续抓、Session(HTTP)双模、CSV / JSON 导出。

> 典型场景:**Rust 爬虫 / 数据采集 / 自动化测试 / 风控与验证码对抗研究 / Web-JS 逆向补环境与纯算签名**。

---

## 📦 没装 Rust 也能用(免编译上手)

🦀 **Python / TS 开发者友好**:不想手动装 Rust?用**一键环境配置脚本**(国内镜像加速、**双击即装**、带进度、自动验证):
见 [`install/`](install/) —— macOS 双击 `install-mac.command`、Windows 双击 `install-windows.bat`,装完即可 `cargo add drission`。
**运行前提**:本机已装 Chrome / Edge(可用环境变量 `CHROME_BIN` 指定路径);用到 OCR 的示例首次运行会自动下载模型到缓存。

## 🤖 `drs` CLI / MCP(AI Agent 入口)

本仓库内置 `drs` 命令行与 stdio MCP server,适合让 AI 以稳定 JSON 协议驱动浏览器:

```bash
cargo install drission-cli --bin drs
drs serve --backend cdp --headless
drs --json open https://example.com
drs ax --outline
drs mcp --backend cdp --headless
```

CLI 子包与核心库同仓库、独立依赖,不会污染普通 `drission` 库用户。完整说明见 [`docs/CLI.md`](docs/CLI.md)。

---

## 🆕 最新版本 v0.3.2 新增

> **v0.3.2** —— 对标 Playwright / Puppeteer / DrissionPage 的「标配补齐」+ 录制/无障碍 + AI Agent 入口:
>
> - **`drs` CLI / MCP(AI Agent 入口)**:新增同仓库 workspace 子包 `drission-cli`,二进制 `drs`。支持 `drs serve` 本地 daemon、`drs --json` 稳定 JSON 协议、页面观察/动作/监听/截图/过盾命令,以及 `drs mcp` stdio MCP server;CLI 依赖独立,不污染核心 `drission` 库用户。详见 [`docs/CLI.md`](docs/CLI.md)。
> - **录制 → 生成代码(codegen / recorder)**:`tab.recorder()` 录一遍页面操作 → 直接产**可运行 Rust**(DrissionPage 风格选择器),覆盖点击/输入/勾选/下拉/按键/悬停/拖拽/iframe/多标签。对标 Playwright `codegen`。
> - **无障碍快照(a11y)**:`tab.ax_tree()` / `ax_snapshot()` 把页面压成 `role "name"` 语义树,用于抗改版断言或**喂 LLM**(比整页 HTML 小一个数量级)。
> - **实时指纹快照读取(fingerprint)**:`tab` 一次性 dump UA/平台/时区/屏幕/WebGL/canvas 等,用于验证「指纹确实换了」。
> - **CDP 标配补齐**:PDF 导出 / MHTML / `set_content` / **HAR 录制+回放** / `expose_function` / 媒体·网络·CPU 模拟 / 移动端设备预设 / 运行时权限 / storage 便捷读写 / `wait().new_tab` 等。详见 [`docs/标配补齐.md`](docs/标配补齐.md)、[`docs/录制与无障碍.md`](docs/录制与无障碍.md)。
>
> 完整记录见 [CHANGELOG.md](CHANGELOG.md)。上一版 **v0.3.1** 聚焦 **Windows 实机点选 / 过盾精准度**与**无头反检测真实度**:
>
> - **Windows 高 DPI 点击对齐**:强制 `device-scale=1`,修复 125% / 150% 缩放下合成点击按物理像素偏移导致的 Cloudflare Turnstile / 易盾点选「点不中」。
> - **无头 GPU 自适应**:有真实 GPU 走硬件 ANGLE、无 GPU(VM / RDP)退 D3D11 WARP,WebGL 真实可用(避开 SwiftShader 软渲染破绽)。
> - **反检测身份一致**:伪装 Chrome UA 时必定补回一致的高熵 Client Hints / `userAgentMetadata`(消除空 `fullVersionList`、Edge 品牌矛盾)。
> - **Cloudflare 内嵌 Turnstile 过盾**:三级定位(含闭合 shadow DOM)+ 以 token 产出判过,支持表单内嵌 Turnstile。
> - **CDP 隔离上下文 cookie 修复**;发布产物体积优化(`opt-level=z` + LTO + strip)。
>
> **v0.3.0** 起的能力:本版**双协议统一接口全面对齐**、新增 Session TLS 指纹、每浏览器指纹与 AI 编程技能文档。

- **CDP 后端全面对齐 Camoufox(同一份代码切 feature 即换后端)**:补齐 iframe / Shadow DOM / 动作链 / 控制台 / WebSocket 监听 / 截图录像 / 上传 / 对话框 / **吐环境 `dump_env`** / **高并发池 `ChromiumPool`** / **修饰组合键**(无头也真执行 Ctrl+A/C/V 编辑命令)/ **Windows 进程树兜底(Job Object)**。
- **Session 浏览器 TLS / JA3 / JA4 + HTTP2 指纹伪装(`--features impersonate`)**:给纯 HTTP 双模套**真实浏览器握手指纹**(`wreq` + BoringSSL,`BrowserProfile::Chrome/Firefox/Safari/Edge`),让「浏览器过盾 → HTTP 接力」不再被现代 WAF(Akamai / CF / DataDome)凭 TLS 指纹拦下;Windows(含 mingw 交叉编译)已实测产出 `.exe`。
- **每浏览器不同指纹 `CdpFingerprint` / `CdpFingerprintPool`**(对标 Camoufox 指纹池):并发起 N 个浏览器各套一份**连贯指纹**(UA / 平台 / 语言 / 时区 / 屏幕 / 硬件 / WebGL / canvas·audio 噪声),同 OS 变体保真(Turnstile 友好)、跨 OS persona 完整伪装。
- **🤖 AI 编程技能 [`docs/SKILL.md`](docs/SKILL.md)(AI 必读)**:从基础到**点选验证码点击**全流程的接口 / feature / 构建规则权威速查;README 顶部声明「AI 基于本库开发须遵循此 skill」。
- **示例全部可复制运行**:默认后端翻为 cdp 后,修正 ~45 个 Camoufox / slider / ocr 示例头注释的运行命令(Camoufox 系须 `--no-default-features --features camoufox`)。

> 早期版本能力(`0.1.x` / `0.2.x`):默认 CDP / Google Chrome 驱动与自动下载、Windows 稳定支持 + Chrome 路径探测、验证码 OCR、图片滑块、**点选验证码真过盾**、Session / `WebPage` 双模、纯算签名运行器、Cloudflare 过盾、代理池健康、登录态持久化、Shadow DOM、下载管理 —— 详见 [CHANGELOG.md](CHANGELOG.md)。

---

## ✨ 核心亮点(重点)

### 1. 内置验证码 OCR(字符型,`feature = "ocr"`)

字母 / 数字 / 扭曲粘连验证码**离线识别**,**无需调用第三方打码平台、无需联网**:
采用 [ddddocr](https://github.com/sml2h3/ddddocr) 预训练模型 + **纯 Rust 推理引擎 [tract](https://github.com/sonos/tract)**
(不依赖原生 onnxruntime,跨平台编译干净)。流水线:灰度 → 等比缩放高 64 → 归一化 → CNN-LSTM → CTC 解码。

```rust
use drission::prelude::*;

#[tokio::main]
async fn main() -> drission::Result<()> {
    let browser = Browser::launch(BrowserOptions::new().headless(true)).await?;
    let tab = browser.latest_tab().await?;
    tab.get("https://apizero.cn/login").await?;

    // 一步:定位验证码 <img> → 取图 → 识别(首次自动下载模型到缓存)
    let code = tab.ocr_image("xpath://form//button/img").await?;
    println!("验证码 = {code}");                       // 例:"P38W"
    Ok(())
}
```

> **端到端实测**(`examples/apizero_login`):用本库打开 [apizero.cn](https://apizero.cn) 登录页 → 自动填表
> → OCR 识别验证码并填入 → 点登录,站点只回「账号或密码错误」而非「验证码错误」,即**验证码识别准确**
> (有头 / 无头各 **5/5、4/4** 通过)。

### 2. 图片滑块缺口距离识别(`feature = "slider"`)

把「拼图要移动多远」算准 + 拟人轨迹拖到位,**与厂商无关**的通用能力,内置极验 / 顶象预设:

```rust
use drission::prelude::*;

# async fn demo(tab: &Tab) -> drission::Result<()> {
// 极验 v4:三图模板匹配,缺口距离 + 闭环纠偏
let r = tab.solve_geetest_slide().await?;

// 顶象(Dingxiang):拼图跨域 taint 不可读 → 截图 + 绿环掩膜 + 彩色内容 NCC + 暗度/描边
let gap = tab.dingxiang_slide_gap(4).await?;   // 缺口位移(CSS 像素)+ 置信
println!("需移动 {:.0}px,置信 {:.2}", gap.displace, gap.confidence);
# Ok(()) }
```

- **极验 v4**:`canvas` 三图(bg / fullbg / slice)模板匹配,对齐误差 ≤1px,headless 实测过码。
- **顶象 popup**:繁杂实拍图 + 同形诱饵 + 重度压暗,用**彩色内容 NCC + 暗度门控 + 描边对齐**,
  离线标定缺口命中 6/6;算法已沉淀为库能力(`GapMethod::ContentNcc`)。
- 通用配置 `SliderConfig` + `tab.slider_gap()` / `tab.solve_slider()`,换厂商只换配置。

---

## 🧰 还支持

- **反检测过盾**:`navigator.webdriver=false`、canvas / webgl / audio 指纹定制、`block_webrtc`;**自动通过 Cloudflare 盾**(Turnstile 复选框可信点击)。
- **元素与交互**:DrissionPage 风格定位(`@id:` / `css:` / `xpath:` / `text:`)、点击 / 输入 / 逐字符拟人输入、动作链、拖拽、下拉 / 单选 / 多选填表、文件上传、iframe、JS 对话框。
- **网络**:XHR / Fetch **监听抓响应体**、**请求拦截改写**(fulfill / abort / resume)、WebSocket 帧监听、控制台监听。
- **多标签与高并发**:每标签独立 cookie 隔离、`BrowserPool` 浏览器池(代理 / 指纹轮换 + 失败重试 + **断点续抓**)。
- **Driver + Session 双模**:浏览器与纯 HTTP 会话双模、cookie 双向互通(省内存,旧机友好);Session 可选 **`--features impersonate` 套浏览器 TLS / JA3 / JA4 + HTTP2 指纹**(`wreq` + BoringSSL),让"浏览器过盾 → HTTP 接力"不被现代 WAF 凭 TLS 指纹拦下。
- **截图与录像**:元素 / 整页 / 区域截图,视口录像合成 mp4。
- **吐环境(补环境)**:采集 canvas / webgl / audio 真实指纹 + 签名 sink 定位,一键导出可 `node` 运行的补环境工程;配合 `signer` 可编成无 Node 单二进制纯算签名。
- **接管浏览器**:`BrowserServer` 暴露 WebSocket 端点,`Browser::connect` 接管已运行的浏览器。
- **多后端**:**默认 Chromium / CDP**(驱动 / 接管 Chrome / Edge / Brave / Chromium / Electron);`--features camoufox` 起 Camoufox / Firefox(Juggler)反检测后端及其全部高级能力。

---

## 🆚 对比:drission vs 其它方案

| 维度 | **drission**(Rust) | DrissionPage(Python) | Playwright / Selenium |
|---|---|---|---|
| 语言 / 运行时 | Rust · `tokio` 异步 · 可编单二进制 | Python | 多语言 |
| 默认后端 | ✅ Google Chrome(CDP),一行切 Camoufox 反检测 | Chromium | 多浏览器 |
| 内置反检测内核 | ✅ Camoufox(`--features camoufox`) | ⚠️ 需自行加固 | ❌ 默认易被识别 |
| 内置验证码 OCR | ✅ 离线纯 Rust 推理 | ❌ | ❌ |
| 滑块缺口距离识别 | ✅ 极验 / 顶象 | ❌ | ❌ |
| 自动过 Cloudflare | ✅ `pass_cloudflare()` | ⚠️ 部分 | ❌ |
| XHR 监听 / 抓响应体 | ✅ 内置 | ✅ | ⚠️ 需手写 |
| 高并发池 + 断点续抓 | ✅ `BrowserPool` 内置 | ⚠️ 需自建 | ❌ |
| 后端 | Chromium / CDP(默认)+ 可选 Camoufox | Chromium | 多浏览器 |

> 一句话:**想要「DrissionPage 的顺手 + Rust 的性能 + 自带打码与反检测」,选 drission。**

---

## 📦 安装

```toml
[dependencies]
drission = "0.3"                                         # 默认 = Chromium / CDP(Google Chrome)

# 要 Camoufox 反检测内核 + 全部高级能力(吐环境 / 过盾 / 池 / 滑块…),关默认 cdp 后开 camoufox:
# drission = { version = "0.3", default-features = false, features = ["camoufox", "ocr", "slider", "signer", "impersonate"] }
#
# 只给默认 CDP 叠加 OCR / signer:
# drission = { version = "0.3", features = ["ocr", "signer"] }
```

| feature | 能力 | 依赖 | 默认 |
|---|---|---|---|
| `cdp` | Chromium / CDP 后端(Chrome / Edge / Brave / Chromium / Electron) | std,无额外重依赖 | **开** |
| `camoufox` | Camoufox / Firefox(Juggler)反检测后端 + 全部高级能力 | std,自动下载 Camoufox | 关 |
| `ocr` | 字符验证码识别(ddddocr + tract) | `image` + `tract-onnx` | 关 |
| `slider` | 图片滑块缺口距离识别(极验 / 顶象) | 纯 JS + std,自动带入 `camoufox` | 关 |
| `signer` | 纯算签名运行器(内嵌 QuickJS,无需 Node) | `rquickjs` | 关 |
| `impersonate` | **Session 浏览器 TLS / JA3 / JA4 + HTTP2 指纹**(双模过 WAF) | `wreq` + BoringSSL(需 `cmake`+`nasm`;Windows 见下),自动带入 `camoufox` | 关 |

> **`impersonate` 的 Windows 构建**:原生 Windows 走 **MSVC**(VS Build Tools + `nasm`,BoringSSL 一等公民)或 mingw(+`nasm`),`cargo build --features impersonate` 即可。从 macOS / Linux **交叉编译到 `x86_64-pc-windows-gnu` 已实测跑通**:`brew install mingw-w64 nasm cmake` 后用 `scripts/win-cross-build.sh build --features impersonate`(脚本自动给 BoringSSL 的 bindgen 喂 mingw sysroot,产出真 `.exe`)。

---

## 🚀 快速开始

**默认后端 = Google Chrome(CDP)**,无需任何 feature,自动探测本机 Chrome(Windows 含注册表 / 用户级安装):

```rust
use drission::prelude::*;

#[tokio::main]
async fn main() -> drission::Result<()> {
    // 自动定位 Google Chrome(CHROME_BIN/DRISSION_CHROME → 安装路径 → Windows 注册表 → PATH);
    // 找不到则自动下载 Chrome for Testing 到 ~/.cache/drission/chrome/(对标 CloakBrowser)。
    // 要指定浏览器:ChromiumBrowser::launch_with("C:\\...\\chrome.exe", true)
    let browser = ChromiumBrowser::launch(ChromiumOptions::new().headless(true)).await?; // 无头;有头零配置用 launch_default()
    let tab = browser.new_tab(Some("https://example.com")).await?;

    println!("title = {:?}", tab.title().await?);
    println!("h1    = {:?}", tab.ele_text("h1").await?);

    browser.quit().await?;
    Ok(())
}
```

**Camoufox 反检测内核**(`--no-default-features --features camoufox`)—— 自动下载分发,带过盾 / 吐环境 / 池 / 滑块等全部高级能力:

```rust
use drission::prelude::*;

#[tokio::main]
async fn main() -> drission::Result<()> {
    let browser = Browser::launch(BrowserOptions::new().headless(true)).await?;
    let tab = browser.latest_tab().await?;

    tab.listen_start(&["api/data"]).await?;        // 先开监听
    tab.get("https://example.com").await?;          // 再访问
    tab.ele("@id:kw").await?.input("drission").await?;

    let packet = tab.listen_wait().await?;          // 抓到目标 XHR(含响应体)
    println!("{}", packet.response.body);

    browser.quit().await?;
    Ok(())
}
```

示例(Camoufox 系示例需 `--no-default-features` 单后端构建):

```bash
cargo run --example cdp_demo                                  # 默认 Chromium / CDP 后端(Google Chrome)
cargo run --example cdp_fetch                                 # 自动下载 Chrome for Testing 并驱动(对标 CloakBrowser)
cargo run --example quickstart    --no-default-features --features camoufox      # Camoufox 最小闭环
cargo run --example pool_crawl    --no-default-features --features camoufox      # 高并发池 + 代理/指纹轮换 + 断点续抓
cargo run --example ocr_captcha   --no-default-features --features camoufox,ocr  # 验证码 OCR
cargo run --example geetest_slide --no-default-features --features slider        # 极验滑块(slider 自动带入 camoufox)
cargo run --example dx_slide      --no-default-features --features slider        # 顶象滑块缺口识别(HL=0 看界面)
cargo run --example env_signer    --features signer           # 内嵌 QuickJS 纯算签名(无 Node)
```

---

## 🖥️ 支持的平台与浏览器

- **平台**:macOS(arm64,主力)· Linux · **Windows(稳定支持)**——CDP 直接启动本机浏览器;Camoufox 后端用命名管道传输。
- **浏览器**:**默认 Google Chrome**(及 Edge / Brave / Chromium / Electron,CDP);Chrome 路径智能探测(Windows 注册表 `App Paths` + 用户级 `%LOCALAPPDATA%` + `PATH`,对标 DrissionPage)。可选 [Camoufox](https://github.com/daijro/camoufox)(Firefox 反检测分支,依赖中用 `default-features=false, features=["camoufox"]`,首次运行**自动下载分发**)。
- **协议**:Chromium 后端走 **CDP**(Chrome DevTools Protocol);Camoufox 走 Firefox 的 **Juggler**(本库自研 `tokio` 异步 Juggler 客户端)。
- **Rust**:≥ 1.85(edition 2024)。
- **🐧 部署到「没界面(无 UI)的 Linux 服务器」**:开无头即可,服务器无需桌面/显示器;库已在 Linux 自动补 `--no-sandbox`/`--disable-dev-shm-usage`(root/容器不崩)。**最省心:用根目录 [`Dockerfile`](Dockerfile) 出镜像,`docker build -t drission . && docker run --rm --shm-size=1g drission` 即用**(已打包 Chrome + 中日韩字体 + 全部系统依赖)。完整指南(含裸机 musl 静态、各发行版依赖、Xvfb 兜底、崩溃速查)见 [**`docs/服务器部署.md`**](docs/服务器部署.md)。

---

## ❓ 常见问题(FAQ)

**Q:drission 和 DrissionPage 是什么关系?**
A:API 语法刻意对齐 DrissionPage,从 Python DP 迁移几乎零成本(见 [API 映射](docs/API映射.md));但 drission 是 **Rust 原生重写**,性能更高,且**内置验证码识别与反检测过盾**。

**Q:验证码识别要联网或调用打码平台吗?**
A:不需要。字符 OCR 用 ddddocr 预训练模型 + 纯 Rust 推理**离线**完成;滑块缺口距离是本地图像算法。首次仅自动下载一次模型到缓存。

**Q:支持 Chrome 吗?默认用哪个浏览器?**
A:**默认就是 Google Chrome**(Chromium / CDP 后端,开箱即用,也支持 Edge / Brave / Chromium / Electron)。本机 Chrome 路径自动探测(`CHROME_BIN` / `DRISSION_CHROME` → 安装路径 → **Windows 注册表 `App Paths`** → `PATH`,对标 DrissionPage)。**找不到系统 Chrome 时自动从官方 [Chrome for Testing](https://googlechromelabs.github.io/chrome-for-testing/) 下载并缓存**(对标 CloakBrowser 首次运行自动下载,三平台 mac / win / linux),也可用 `ChromiumBrowser::launch_with(path, headless)` 指定。要 Firefox 反检测内核则关默认 cdp 后开 `camoufox`。

**Q:没装 Chrome 也能用吗?**
A:能。`ChromiumBrowser::launch(headless)` 找不到系统浏览器会**自动下载 Chrome for Testing** 到 `~/.cache/drission/chrome/`;也可显式 `ChromiumBrowser::download_chrome()`(返回路径、不启动),或 `drission::cdp::download_chrome_for("win64", "Stable")` **跨平台预取**(如在 mac 上为分发下载 Windows 版)。

**Q:服务器没有界面(无 UI / 无显示器)能跑吗?怎么部署?**
A:能。**你的程序和它驱动的浏览器都不需要界面——开无头 `ChromiumOptions::new().headless(true)` 即可**。库已在 Linux 自动补 `--no-sandbox`/`--disable-dev-shm-usage`,所以 root / Docker 容器里也不会崩。**最省心**:用根目录 [`Dockerfile`](Dockerfile)(已打包 Chrome + 中日韩字体 + 全部系统依赖)`docker build -t drission . && docker run --rm --shm-size=1g drission`,目标服务器零配置。想直接 scp 二进制就用 musl 静态(`scripts/linux-musl-build.sh`)+ 在服务器装浏览器依赖。完整指南见 [**`docs/服务器部署.md`**](docs/服务器部署.md)。

**Q:能过 Cloudflare 吗?**
A:可以。`tab.pass_cloudflare()` 支持交互式 Turnstile 可信点击与非交互式自动放行。

**Q:怎么做高并发采集?**
A:用 `BrowserPool` 浏览器池,内置代理 / 指纹轮换、失败重试与**断点续抓**;省内存场景可切 Session(HTTP)双模。

**Q:跨平台吗?需要什么 Rust 版本?**
A:macOS(主力)· Linux · Windows(命名管道传输已打通);Rust ≥ 1.85(edition 2024)。

---

## 📚 文档

- [🤖 **编程技能 SKILL(AI 必读)**](docs/SKILL.md) — 接口 / feature / 构建规则权威速查,基础 → 点选验证码全流程,照写即对
- [文档总览 `docs/`](docs/) — 设计 · API 映射 · 并发池 · 长监听
- [**DrissionPage → drission API 映射**](docs/API映射.md) — 从 DP 迁移,按表把 Python 写法换成 Rust,几乎零成本
- [设计文档](docs/设计.md) — 分层架构 / Juggler 选型 / 并发模型 / 各能力接线
- [高并发池设计](docs/并发池.md) — `BrowserPool` / 代理池 / 指纹池 / 断点续抓
- [**示例索引(48+)**](examples/README.md) — 「能力 → 示例 → 运行命令」总览
- [API 参考(docs.rs)](https://docs.rs/drission) · [更新日志 CHANGELOG](CHANGELOG.md)

---

## 🗺️ 已完成与后续方向

已落地能力:

- 点选 / 文字点选链路:检测框(`Det`)→逐框 OCR→字形模板第二信号→全局最优指派→可信点击;易盾点选含采集、侦察、稳定版示例。
- OCR 自训模型运行时接入:支持 `dddd_trainer` 产物的 onnx + `charsets.json` 加载、热替换与文档化流程。
- 滑块通用缺口识别与拟人轨迹:GeeTest v4 / 顶象示例、最小 jerk 轨迹、闭环微调与可信鼠标事件。
- 反检测与「吐环境」补全:CDP/Camoufox 指纹覆盖、字体枚举、像素级 canvas、WebRTC、plugins/mimeTypes、WebGL/audio 等录制回放。
- WS 接管浏览器: `BrowserServer` + `Browser::connect` 已支持单活动客户端接管、断线重连、token 校验。
- 静态 XPath 1.0 常用子集、Windows Job Object 进程树兜底、Linux Docker/musl/CI 构建矩阵。

下一步增强:

- 计算题验证码、更多点选/文字点选厂商模板与样本库。
- 滑块 / 点选行为轨迹模型化,把行为风控从启发式推进到可复用模型。
- WS 接管的真正多客户端多路复用与 `wss://` TLS。
- 静态 XPath 更多 axes/functions,以及更多厂商滑块 / 盾预设。
- 更多真实云厂商、发行版、桌面/无头环境的 Linux 实测矩阵。

---

## ⚠️ 免责声明

本项目仅供学习与合法、非盈利用途。使用者须遵守目标站点的 `robots` 协议与当地法律法规,
**禁止**用于任何违法、侵害他人利益、攻击骚扰或采集受保护数据的行为。
使用 drission 产生的一切行为及后果均由使用者自行承担,与版权持有人(极数本源)无关;
版权持有人不对本项目可能存在的缺陷导致的任何损失负责。

**未经授权,禁止将本项目(无论是否修改)作为商品出售、转售、倒卖或作为付费产品/服务的核心牟利。**
详见 [`LICENSE`](LICENSE)。

---

## 🙏 致谢

- [DrissionPage](https://github.com/g1879/DrissionPage):API 设计灵感来源。
- [Camoufox](https://github.com/daijro/camoufox):默认浏览器内核。
- [ddddocr](https://github.com/sml2h3/ddddocr):验证码 OCR 预训练模型。
- [tract](https://github.com/sonos/tract):纯 Rust ONNX 推理引擎。

## 📄 许可证

自定义许可(源代码可用 · 仅限个人学习与合法非盈利 · 禁止未授权商业用途与转售),见 [`LICENSE`](LICENSE)。

---

<sub>关键词 / keywords: Rust 浏览器自动化 · 验证码识别 · ddddocr · OCR · 滑块缺口距离 · 极验 GeeTest · 顶象 Dingxiang ·
反检测 · 过 Cloudflare 盾 · 高并发爬虫 · 代理轮换 · 指纹定制 · 补环境 · 纯算签名 · Camoufox · Firefox Juggler · Chromium CDP ·
DrissionPage · Rust 版 DrissionPage · browser automation · captcha solver · captcha OCR · slider captcha · GeeTest · anti-detect ·
undetectable · stealth · Cloudflare bypass · web scraping · crawler · proxy rotation · alternative to rust_drission / zendriver-rs ·
由 [极数本源 apizero.cn](https://apizero.cn) 出品。</sub>
