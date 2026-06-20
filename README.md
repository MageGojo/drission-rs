# drission · Rust 反检测浏览器自动化 + 内置验证码识别(OCR / 滑块缺口距离)

[![crates.io](https://img.shields.io/crates/v/drission.svg)](https://crates.io/crates/drission)
[![docs.rs](https://docs.rs/drission/badge.svg)](https://docs.rs/drission)
[![Rust](https://img.shields.io/badge/rust-1.85%2B-orange.svg)](https://www.rust-lang.org)
[![platform](https://img.shields.io/badge/platform-macOS%20%7C%20Linux%20%7C%20Windows-blue.svg)](#-支持的平台与浏览器)

[English](README.en.md) · **简体中文**

> **drission 是一个用 Rust 编写的高性能反检测浏览器自动化库**:默认驱动 [Camoufox](https://github.com/daijro/camoufox)(Firefox 反检测内核),
> 可选 Chromium / CDP 后端,**内置字符验证码 OCR**(ddddocr 模型 · 纯 Rust 推理)与**图片滑块缺口距离识别**(极验 / 顶象),
> 支持 XHR 监听 / 拦截、自动过 Cloudflare 盾、高并发浏览器池,API 语法对齐 [DrissionPage](https://github.com/g1879/DrissionPage),面向高并发爬虫与自动化。
>
> *drission is a high-performance, anti-detect browser-automation library in Rust: Camoufox/Firefox by default (optional Chromium/CDP),
> with **built-in captcha OCR** and **image slider-gap recognition**, async high-concurrency crawling, XHR listen/intercept and
> Cloudflare bypass — with a DrissionPage-style API.*

本库由 **极数本源([apizero.cn](https://apizero.cn))** 出品与维护,是其自动化与数据采集技术栈的一部分。
如果你在找「**Rust 验证码识别 / 滑块缺口距离 / 反检测浏览器 / 高并发爬虫**」的一站式方案,这里是答案。

---

## 📖 这是什么(一句话看懂)

**drission = Rust 版的 DrissionPage + 内置打码(OCR / 滑块)+ 反检测过盾。** 用一套 `tokio` 异步 API 同时拿下:

- **浏览器自动化**:启动 / 接管反检测浏览器,像写 DrissionPage 一样定位元素、点击输入、抓包改包。
- **验证码识别**:字符验证码离线 OCR、滑块缺口距离计算 + 拟人轨迹,**不依赖第三方打码平台、无需联网**。
- **反检测与过盾**:指纹定制、`navigator.webdriver=false`、自动通过 Cloudflare Turnstile。
- **工程化采集**:高并发浏览器池、代理 / 指纹轮换、断点续抓、Session(HTTP)双模、CSV / JSON 导出。

> 典型场景:**Rust 爬虫 / 数据采集 / 自动化测试 / 风控与验证码对抗研究 / Web-JS 逆向补环境与纯算签名**。

---

## 🆕 最新版本 v0.1.1 新增

> 完整记录见 [CHANGELOG.md](CHANGELOG.md);以下为 `0.1.0` 之后累积的**向后兼容新增**,故按补丁号递增。

- **CDP / Chromium 后端**(`--features cdp`):新增 `ChromiumBrowser` / `ChromiumTab` / `ChromiumElement`,可驱动或接管 **Chrome / Edge / Brave / Electron**,含原生可信点击、拟人输入、`Network` 监听与 `Fetch` 拦截,数据类型与 Camoufox 后端共用。
- **Session(HTTP)双模 + `WebPage` 双模门面**:不开浏览器的纯 HTTP 会话,cookie 与浏览器双向互通,`change_mode` 自动同步登录态(对标 DrissionPage 的 Driver + Session,省内存、旧机友好)。
- **数据采集导出**:`scrape` 模块(`records_to_csv` / `records_to_json` / `write_csv` / `write_json`)、表格提取(`Element::table`)、自动翻页(`Tab::paginate`)。
- **代理池健康检查**:`ProxyHealth` / `ProxyGeo`,出口 IP 地理 ↔ 指纹自洽覆盖,住宅代理轮换。
- **纯算签名运行器**(`--features signer`):内嵌 QuickJS,把「吐环境」导出的 `env.js` 编进单个二进制,**无需 Node / 浏览器**即可回放补环境并纯算签名。
- **Cloudflare 自动过盾** `tab.pass_cloudflare()`、**顶象缺口通用算法** `GapMethod::ContentNcc`、**登录态持久化** `storageState`、**逐字符拟人输入** `ele.input_human`、**Shadow DOM**(`ShadowRoot`)、**下载管理** `tab.downloads()`、**请求拦截句柄** `tab.intercept()`。
- **工程化基建**:多平台 CI(fmt / clippy / test / feature 矩阵 / 跨平台 check / docs.rs 构建)、离线集成测试、criterion 基准、`CHANGELOG` / `CONTRIBUTING` / `SECURITY`。

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
- **Driver + Session 双模**:浏览器与纯 HTTP 会话双模、cookie 双向互通(省内存,旧机友好)。
- **截图与录像**:元素 / 整页 / 区域截图,视口录像合成 mp4。
- **吐环境(补环境)**:采集 canvas / webgl / audio 真实指纹 + 签名 sink 定位,一键导出可 `node` 运行的补环境工程;配合 `signer` 可编成无 Node 单二进制纯算签名。
- **接管浏览器**:`BrowserServer` 暴露 WebSocket 端点,`Browser::connect` 接管已运行的浏览器。
- **多后端**:默认 Camoufox / Juggler;`--features cdp` 起 Chromium / CDP 后端接管 Chrome / Edge / Brave / Electron。

---

## 🆚 对比:drission vs 其它方案

| 维度 | **drission**(Rust) | DrissionPage(Python) | Playwright / Selenium |
|---|---|---|---|
| 语言 / 运行时 | Rust · `tokio` 异步 · 可编单二进制 | Python | 多语言 |
| 默认反检测 | ✅ Camoufox(Firefox 反检测内核) | ⚠️ 需自行加固 | ❌ 默认易被识别 |
| 内置验证码 OCR | ✅ 离线纯 Rust 推理 | ❌ | ❌ |
| 滑块缺口距离识别 | ✅ 极验 / 顶象 | ❌ | ❌ |
| 自动过 Cloudflare | ✅ `pass_cloudflare()` | ⚠️ 部分 | ❌ |
| XHR 监听 / 抓响应体 | ✅ 内置 | ✅ | ⚠️ 需手写 |
| 高并发池 + 断点续抓 | ✅ `BrowserPool` 内置 | ⚠️ 需自建 | ❌ |
| 后端 | Camoufox + 可选 Chromium/CDP | Chromium | 多浏览器 |

> 一句话:**想要「DrissionPage 的顺手 + Rust 的性能 + 自带打码与反检测」,选 drission。**

---

## 📦 安装

```toml
[dependencies]
drission = "0.1"

# 按需开启验证码 / 后端能力(默认关,保持核心精简):
# drission = { version = "0.1", features = ["ocr", "slider", "cdp", "signer"] }
```

| feature | 能力 | 依赖 | 默认 |
|---|---|---|---|
| `camoufox` | Camoufox / Firefox(Juggler)后端 | 核心,始终编译 | **开** |
| `ocr` | 字符验证码识别(ddddocr + tract) | `image` + `tract-onnx` | 关 |
| `slider` | 图片滑块缺口距离识别(极验 / 顶象) | 纯 JS + std,零额外依赖 | 关 |
| `cdp` | Chromium 后端(Chrome / Edge / Brave / Electron) | std,无额外重依赖 | 关 |
| `signer` | 纯算签名运行器(内嵌 QuickJS,无需 Node) | `rquickjs` | 关 |

---

## 🚀 快速开始

```rust
use drission::prelude::*;

#[tokio::main]
async fn main() -> drission::Result<()> {
    // 留空 binary_path 即自动下载分发 Camoufox 到 ~/.cache/camoufox
    let browser = Browser::launch(BrowserOptions::new().headless(true)).await?;
    let tab = browser.latest_tab().await?;

    tab.listen_start(&["api/data"]).await?;        // 先开监听
    tab.get("https://example.com").await?;          // 再访问
    tab.ele("@id:kw").await?.input("drission").await?;
    tab.ele("#submit").await?.click().await?;

    let packet = tab.listen_wait().await?;          // 抓到目标 XHR(含响应体)
    println!("{}", packet.response.body);

    browser.quit().await?;
    Ok(())
}
```

示例:

```bash
cargo run --example quickstart                          # 最小闭环
cargo run --example pool_crawl                          # 高并发池 + 代理/指纹轮换 + 断点续抓
cargo run --example ocr_captcha   --features ocr        # 验证码 OCR
cargo run --example apizero_login --features ocr        # 端到端:填表 + OCR 验证码 + 登录
cargo run --example geetest_slide --features slider     # 极验滑块
cargo run --example dx_slide      --features slider      # 顶象滑块缺口识别(HL=0 看界面)
cargo run --example cdp_demo      --features cdp         # Chromium / CDP 后端
cargo run --example env_signer    --features signer      # 内嵌 QuickJS 纯算签名(无 Node)
```

---

## 🖥️ 支持的平台与浏览器

- **平台**:macOS(arm64,主力)· Linux · Windows(命名管道传输,已打通)。
- **浏览器**:[Camoufox](https://github.com/daijro/camoufox)(Firefox 反检测分支),首次运行**自动下载分发**;可选 Chromium 后端(Chrome / Edge / Brave / Electron,`--features cdp`)。
- **协议**:Firefox 的 **Juggler**(非 CDP)——Camoufox 仅支持 Juggler,本库自研 `tokio` 异步 Juggler 客户端;Chromium 后端走 **CDP**。
- **Rust**:≥ 1.85(edition 2024)。

---

## ❓ 常见问题(FAQ)

**Q:drission 和 DrissionPage 是什么关系?**
A:API 语法刻意对齐 DrissionPage,从 Python DP 迁移几乎零成本(见 [API 映射](docs/API映射.md));但 drission 是 **Rust 原生重写**,性能更高,且**内置验证码识别与反检测过盾**。

**Q:验证码识别要联网或调用打码平台吗?**
A:不需要。字符 OCR 用 ddddocr 预训练模型 + 纯 Rust 推理**离线**完成;滑块缺口距离是本地图像算法。首次仅自动下载一次模型到缓存。

**Q:只能用 Firefox 吗?支持 Chrome 吗?**
A:默认后端是 Camoufox(Firefox 反检测分支);开启 `--features cdp` 后可用 **Chromium / CDP** 后端驱动或接管 Chrome / Edge / Brave / Electron。

**Q:能过 Cloudflare 吗?**
A:可以。`tab.pass_cloudflare()` 支持交互式 Turnstile 可信点击与非交互式自动放行。

**Q:怎么做高并发采集?**
A:用 `BrowserPool` 浏览器池,内置代理 / 指纹轮换、失败重试与**断点续抓**;省内存场景可切 Session(HTTP)双模。

**Q:跨平台吗?需要什么 Rust 版本?**
A:macOS(主力)· Linux · Windows(命名管道传输已打通);Rust ≥ 1.85(edition 2024)。

---

## 📚 文档

- [文档总览 `docs/`](docs/) — 设计 · API 映射 · 并发池 · 长监听
- [**DrissionPage → drission API 映射**](docs/API映射.md) — 从 DP 迁移,按表把 Python 写法换成 Rust,几乎零成本
- [设计文档](docs/设计.md) — 分层架构 / Juggler 选型 / 并发模型 / 各能力接线
- [高并发池设计](docs/并发池.md) — `BrowserPool` / 代理池 / 指纹池 / 断点续抓
- [**示例索引(48+)**](examples/README.md) — 「能力 → 示例 → 运行命令」总览
- [API 参考(docs.rs)](https://docs.rs/drission) · [更新日志 CHANGELOG](CHANGELOG.md)

---

## 🗺️ 路线图(未来)

- 验证码:点选 / 文字点选、计算题、滑块行为轨迹模型化,OCR 自训模型接入(`dddd_trainer`)。
- 更多反检测深指纹注入与「吐环境」补全(字体枚举、像素级 canvas、WebRTC)。
- WS 接管多客户端多路复用、`wss://` TLS。
- 静态 XPath 子集扩展、更多厂商滑块 / 盾预设。
- 更完善的 Windows 进程生命周期(Job Object 兜底)与 Linux 实测矩阵。

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

<sub>关键词 / keywords: Rust 浏览器自动化 · 验证码识别 · OCR · 滑块缺口距离 · 极验 · 顶象 · 反检测 · 过 Cloudflare 盾 ·
高并发爬虫 · 代理轮换 · 指纹定制 · 补环境 · 纯算签名 · Chromium CDP · Camoufox · DrissionPage ·
browser automation · captcha solver · slider captcha · anti-detect · Cloudflare bypass · web scraping · proxy rotation ·
Camoufox · DrissionPage · 由 [极数本源 apizero.cn](https://apizero.cn) 出品。</sub>
