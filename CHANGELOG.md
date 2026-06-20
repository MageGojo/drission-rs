# 更新日志 / Changelog

本项目的所有重要变更都记录在此文件。

格式遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/),
版本号遵循 [语义化版本](https://semver.org/lang/zh-CN/)。

## [Unreleased]

## [0.2.0] - 2026-06-20

> 后端架构调整 + Windows 稳定支持 + **默认 Google Chrome**。
> 含一处**破坏性变更**(默认后端改为 CDP/Chromium),故升次版本号。

### 破坏性变更 Breaking

- **默认后端改为 Chromium / CDP**(`default = ["cdp"]`):开箱即用驱动/接管 **Google Chrome**
  (及 Edge / Brave / Chromium / Electron),最精简、无 Camoufox 重代码。
  原 Camoufox / Firefox 反检测后端及其全部高层能力(`Page` / `WebPage` / `SessionPage` / `Pool` /
  吐环境 / 过盾 / 滑块…)改为 **opt-in**:`--features camoufox`(`slider` 会自动带入)。
  升级方式:依赖处显式开启即可,`drission = { version = "0.2", features = ["camoufox"] }`。

### 新增 Added

- **Windows Chrome 路径探测强化(对标 DrissionPage `get_chrome_path`)**:新增 `src/cdp/locate.rs`,
  探测优先级 = `CHROME_BIN` / `DRISSION_CHROME` 环境变量 → 常见安装路径(Windows 覆盖**用户级**
  `%LOCALAPPDATA%` 与系统级 `%PROGRAMFILES%` / `%PROGRAMFILES(X86)%` / `%PROGRAMW6432%`)→
  **Windows 注册表** `App Paths\chrome.exe`(`HKEY_CURRENT_USER` 优先,再 `HKEY_LOCAL_MACHINE`)→
  系统 `PATH` 扫描。全程**优先 Google Chrome**,解决“免管理员用户级安装 / 非默认盘”探测不到的问题。
- **CDP 启动便捷方法**:`ChromiumBrowser::launch_with(path, headless)`(指定可执行文件)、
  `ChromiumBrowser::find_chrome()` 与 `cdp::chrome_path()`(诊断“为何没找到浏览器”)。
- **`Page` 一行起步门面**(Camoufox 后端,对标 DP `ChromiumPage`):`Page::new()` / `headless()` /
  `connect()`,经 `Deref` 直接拥有全部 `Tab` 方法;`tab.click/input/exists` 高频捷径。
- **后端无关共享模块**:`crate::keys`(`Keys` / `KeyInput`)、`crate::net`(`DataPacket` /
  `RequestData` / `ResponseData` / `ListenFilter` / `ResumeOptions`),两后端复用、始终编译。

### 变更 Changed

- 后端 feature 真正 gate:不开 `camoufox` 则 `browser` / `launcher` / `page` / `web_page` /
  `session` / `pool` 均不编译;纯 CDP 默认构建独立、不含 Camoufox 代码。
- 文档与示例全面对齐新默认(示例按需 `required-features`,Camoufox 示例需 `--features camoufox`)。

## [0.1.1] - 2026-06-20

> `0.1.0` 发布后累积的能力(后端、双模、采集、池化)与工程化基建。
> 均为**向后兼容的新增**,故按补丁号递增。

### 新增 Added

- **CDP / Chromium 后端**(`--features cdp`):`ChromiumBrowser` / `ChromiumTab` / `ChromiumElement`,
  可驱动或接管 Chrome / Edge / Brave / Electron。含元素句柄、原生可信点击、拟人输入、
  `Network` 网络监听与 `Fetch` 请求拦截,数据类型与 Camoufox 后端共用。
- **Session(HTTP)双模**:`SessionPage` / `SessionOptions` / `PostData`,不开浏览器的纯 HTTP 会话,
  自管理 cookie jar,与浏览器 cookie 双向互通(`load_cookies_from_tab` / `apply_cookies_to_tab`)及存盘复用登录态。
- **`WebPage` 双模门面**:`WebPage` + `PageMode{Driver,Session}`,`change_mode` 自动同步 cookie。
- **采集导出**:`scrape` 模块(`rows_to_csv` / `records_to_csv` / `records_to_json` / `write_csv` / `write_json`)、
  表格提取(`StaticElement::table` / `Element::table`)、翻页(`Tab::paginate`)。
- **代理池健康检查**:`ProxyGeo` / `ProxyHealth` / `locale_for_country`,出口 IP 地理 ↔ 指纹自洽覆盖,住宅代理轮换。
- **登录态持久化**:`storage_state` / `save_storage_state` / `load_storage_state`(cookie 全量 + localStorage/sessionStorage)。
- **逐字符拟人输入** `ele.input_human`;元素相对定位与 **Shadow DOM**(`ShadowRoot`)。
- **下载管理**(`tab.downloads()`)、**拦截句柄**(`tab.intercept()`)、**窗口尺寸**(`tab.set().window()`)。
- **Cloudflare 自动过盾**:`tab.pass_cloudflare()`(交互式 Turnstile 可信点击 + 非交互式自动放行)。
- **顶象(Dingxiang)缺口距离通用算法**:`GapMethod::ContentNcc` + `SliderConfig::dingxiang(i)`(`--features slider`)。
- **工程化基建**:GitHub Actions CI(fmt / clippy / 多平台 test / feature 矩阵 / 跨平台 check / docs.rs 构建)、
  离线集成测试(`tests/`)、criterion 基准(`benches/`)、`CHANGELOG`、`CONTRIBUTING`、`SECURITY`、`rust-toolchain.toml`。

### 变更 Changed

- **后端 feature 化**:`default = ["camoufox"]`;CDP 收为可选 `cdp` feature,默认构建不含 Chromium 后端。
- **验证码能力可选**:`slider`(纯 JS + std,零额外依赖)与 `ocr`(tract + image 重依赖)均默认关、按需开。
- **GeeTest 滑块缺口距离**改为模板匹配(修正旧的“边缘差”系统性偏移),并沉淀为厂商无关的通用滑块库(`tab.solve_slider`)。
- **docs.rs**:启用 `all-features` 构建,使 `ocr` / `slider` / `cdp` 的 API 在文档站可见并带 feature 徽标。

### 修复 Fixed

- **Windows 有头传输**:补 `-wait-for-browser`,修复首个命令即“连接已关闭”。
- **吐环境**:opaque origin 下 storage 访问报错导致 seed 全空;模板占位符替换不彻底;canvas 加噪导致对比口径错位等。

## [0.1.0] - 2026

首个公开版本。

### 新增 Added

- 基于 **Juggler** 协议的 Camoufox/Firefox 反检测浏览器驱动(`tokio` 异步,DrissionPage 风格 API)。
- 多标签并发(独立 cookie / BrowserContext)、元素定位与交互、动作链、表单、iframe、对话框、文件上传。
- 网络 **XHR/Fetch 监听**(抓响应体)与**请求拦截改写**;控制台监听;WebSocket 帧监听。
- 反检测(`webdriver=false`、指纹定制、`block_webrtc`);截图与录像;接管浏览器(`BrowserServer` / `Browser::connect`)。
- **通用吐环境**(`tab.dump_env()`)+ canvas/webgl/audio 指纹补环境 + 一键导出可 `node` 运行的工程。
- **高并发池** `BrowserPool`(代理/指纹轮换 + 重试 + 断点续抓)。
- **内置验证码 OCR**(ddddocr 模型 + tract 纯 Rust 推理)与**图片滑块缺口距离识别**(极验)。
- 跨平台:macOS / Linux / Windows(命名管道传输)。

[Unreleased]: https://github.com/MageGojo/drission-rs/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/MageGojo/drission-rs/compare/v0.1.1...v0.2.0
[0.1.1]: https://github.com/MageGojo/drission-rs/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/MageGojo/drission-rs/releases/tag/v0.1.0
