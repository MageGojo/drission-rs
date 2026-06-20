# 更新日志 / Changelog

本项目的所有重要变更都记录在此文件。

格式遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/),
版本号遵循 [语义化版本](https://semver.org/lang/zh-CN/)。

## [Unreleased]

### 新增 Added

- **CDP 修饰组合键 / 热键**(`tab.key_combo(&[Keys::CONTROL, "a"])` / `ele.shortcut(...)`):CDP 原生
  `modifiers` 位掩码下发,页面读得到 `e.ctrlKey`/`metaKey` 等为 `true`(真组合键);对常见编辑快捷键
  (Ctrl/Cmd + A/C/X/V/Z/Y)额外带 CDP `commands`(selectAll/copy…),**无头下也真正执行编辑动作**
  (对齐 Puppeteer 做法)。补齐"CDP 能真做 Ctrl+A"的能力(Camoufox/Juggler 无 modifiers 字段、仍是
  已知限制)。示例 `cdp_keyboard` 真机自验证 ALL PASS(拟人输入 + Cmd/Ctrl+A 全选 + 删除清空)。
- **CDP 吐环境 `tab.dump_env()`**(对齐 camoufox):把吐环境**后端无关核心**抽到新模块 `crate::envkit`
  (探针/`env.js`/导出工程/同构双跑验证 + canvas/webgl/audio/字体/像素/WebRTC/plugins 指纹回放 + 反 hook +
  签名 sink 定位 + 资产模板),两后端经新 `EnvBackend` trait(导航前注入 + 求值)复用同一套逻辑;camoufox
  `dump_env` 改为薄胶水(零行为变化)。CDP 注入走 `Page.addScriptToEvaluateOnNewDocument`、求值走
  `Runtime.evaluate`。`EnvDump`/`EnvScope`/`EnvTarget` 后端无关;`EnvDumper`/`EnvProbe` canonical cdp 优先
  (camoufox 用 `Camoufox*`、cdp 用 `Chromium*` 显式名)。示例 `cdp_dump_env` 真机自验证 ALL PASS(采种子 →
  生成 env.js → 导出工程 → **同构双跑 45/45 字段一致**)。
- **CDP 高并发池 `ChromiumPool`**(对齐 camoufox `BrowserPool`):多 worker(Chrome 进程)+ 信号量并发上限
  + 失败重试(指数退避)+ 健康自愈(连接断/进程退惰性重建)+ `map`(保序)/ `map_resumable`(断点续抓,
  配 `Checkpoint`)/ `run`/`run_keyed`/`shutdown`。**每任务一个独立 `BrowserContext`**(cookie/缓存/storage
  隔离),带 `proxy` 时该上下文走 **CDP 原生 per-context 代理**(`Target.createBrowserContext{proxyServer}`)+
  UA/locale/时区经会话级 `Emulation` 覆盖;`ChromiumContextOverride` + `ChromiumBrowser::new_tab_with` +
  `ChromiumTab::close` 配套。后端无关的 `RetryPolicy`/`RotateStrategy`/`Checkpoint` 抽为 cdp/camoufox 共用
  (`crate::pool` 改 `any(camoufox, cdp)` 编译)。prelude canonical:`Pool`(=ChromiumPool),cdp-only 时
  `PoolOptions`/`ContextOverride` 亦为 canonical。示例 `cdp_pool` 真机自验证 ALL PASS(2×2 并发=4、保序 map、
  每任务 context 隔离、断点续抓续跑只补未完成)。
- **Windows 进程生命周期兜底(Job Object)**:两后端启动浏览器后把进程绑入 `KILL_ON_JOB_CLOSE` 的
  Job —— Camoufox `WinChild`(解决"持的是 Firefox launcher 句柄、`TerminateProcess` 打不到再 fork 的
  真浏览器 → 孤儿进程")、CDP `ChromiumBrowser`(`kill_on_drop` 只杀主进程,会留渲染/GPU 子进程)。
  `quit`/`Drop` 关闭 Job 即级联终止整棵进程树。`x86_64-pc-windows-gnu` 交叉编译 + clippy 干净。

- **CDP 后端全面对齐 Camoufox**(同一份用户代码,切 feature 即换后端):在已对齐的导航/元素/输入/
  静态元素之上,补齐高层句柄与能力 —— iframe(`tab.get_frame`/`ele.content_frame`)、Shadow DOM
  (`ele.shadow_root`)、动作链(`tab.actions()`)、控制台 / WebSocket 监听(`tab.console()`/
  `tab.websocket()`)、`wait()`/`scroll()`/`set()`/`set().window()` 句柄、对话框(`handle_next_dialog`)、
  文件上传(`set_files`/`click_to_upload`)、登录态(`storage_state`/`apply_storage_state`)、cookie
  (`cookies`/`set_cookies`)、翻页(`paginate`)、录像(`tab.screencast()`)、OCR(`tab.ocr_image`)。
- **CDP 下载管理 `tab.downloads()`**(`ChromiumDownloads`,对齐 camoufox `Downloads`):多任务并发跟踪 +
  任务列表 + 实时进度 + 自定义重命名。**基于 CDP 原生 `Page.downloadWillBegin`/`downloadProgress`
  事件按 `guid` 聚合**(比文件系统轮询更准、自带 received/total 字节)。下载目录用新增的
  `ChromiumOptions::download_path` 或 `tab.set_download_path` 设置。`start`/`wait_new`/`wait_done`/
  `wait_count_done`/`missions`/`stop` + `DownloadMission`(`save_as`/`downloaded_bytes`)。
  `Downloads`/`DownloadMission`/`DownloadState` 进 `prelude`(canonical:cdp 优先,camoufox 用
  `CamoufoxDownloads`)。示例 `cdp_download`:进程内 HTTP 服务以 `Content-Disposition: attachment`
  供文件,真实 Chrome 点 `<a download>` 触发下载,**真机端到端自验证 ALL CHECKS PASSED**(顺序
  `wait_new`+`wait_done`、**并发** `wait_count_done(2)`、20KB 文件 `received==total==20000` 证 CDP
  事件自带真实进度字节、`missions()==3`、`save_as` 重命名、`stop` 翻转)。
- **点选 / 文字点选验证码**(`feature = "ocr"`,对标 ddddocr `det=True`):
  - **`Det`** —— ddddocr 目标检测模型 `common_det.onnx`(YOLOX),**tract 纯 Rust 推理**(无原生 onnxruntime):
    416 灰边 letterbox + 解码 + NMS → `Vec<BBox>`(原图坐标 + 置信度)。`Det::new()` 首次自动下载模型到缓存。
  - **`ClickWord`** —— 点选求解器:`chars()`(检测 → 逐框 OCR)、`points_for(img, targets)`(按提示顺序匹配出
    **依次点击点**)。配合浏览器可信点击即可自动点选。
  - 导出 `BBox` / `Det` / `ClickWord` 进 `prelude`(ocr feature)。
  - 示例 `det_probe`(检测可行性)、`yidun_click`(易盾 picture-click 全链路)。
  - 局限:单字艺术体 OCR 非 100%(ddddocr 固有);易盾另有行为风控,字点准≠必过。详见 `docs/第三梯队.md` 里程碑 54。

### 修复 Fixed

- `det_probe` / `yidun_click` 示例的 `required-features` 由 `["ocr"]` 补为 `["cdp", "ocr"]`:它们驱动
  `ChromiumBrowser`(CDP),原配置在 `--features camoufox,ocr`(无 cdp)构建下编译失败。
- `cdp::stealth::stealth_args()` 在非 macOS 目标(Windows / Linux)的 `unused_mut` 警告
  (`--use-mock-keychain` 仅 macOS 追加)—— 改条件 shadow,跨平台零警告。

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
