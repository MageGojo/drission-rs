# 更新日志 / Changelog

本项目的所有重要变更都记录在此文件。

格式遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/),
版本号遵循 [语义化版本](https://semver.org/lang/zh-CN/)。

## [0.3.2] - 2026-07-06

> **标配补齐**:对标 Playwright / Puppeteer / DrissionPage 的通用浏览器能力。这些能力为 **Chromium / CDP
> 专有**(Firefox/Juggler 无对应),仅默认 CDP 后端提供。设计与两端可行性见 [`docs/标配补齐.md`](docs/标配补齐.md)。

### 新增 Added

- **`drs` CLI / MCP(AI Agent 入口)**:新增 workspace 子包 `crates/drission-cli`(`package = "drission-cli"`,
  二进制 `drs`)。CLI 依赖独立,不污染核心库;默认 CDP/Chrome,可按 feature 切到 Camoufox 或启用 OCR。
  支持 `drs serve` 本地 daemon、`drs --json` 统一响应、页面打开/标签/无障碍快照/HTML/文本/JS/截图/
  点击输入/按键/等待/网络监听/Cloudflare pass,以及 `drs mcp` stdio MCP server。文档见 [`docs/CLI.md`](docs/CLI.md)。
- **PDF 导出** `tab.print_to_pdf(&PdfOptions)` / `save_pdf`(`Page.printToPDF`,无头);**保存 MHTML**
  `tab.mhtml()` / `save_mhtml`(`Page.captureSnapshot`);**`set_content`** 直接灌 HTML(`Page.setDocumentContent`,回退 `document.write`)。
- **媒体模拟** `tab.set().emulate_dark(bool)` / `emulate_media(media, features)`(深色 `prefers-color-scheme` / print / reduced-motion)。
- **网络条件** `tab.set().offline(bool)` / `network_conditions(&NetworkConditions)`(`slow_3g` / `fast_3g` / `offline` 预设);**CPU 节流** `cpu_throttling(rate)`。
- **运行时权限授予** `tab.set().grant_permissions(origin, &[..])` / `reset_permissions`(`Browser.grantPermissions`)。
- **localStorage / sessionStorage 便捷读写** `tab.set().local_storage_set/get/remove/clear` + `session_storage_*`。
- **等待** `tab.wait().new_tab`(等弹窗 / 新标签 → 返回新 `Tab`)/ `download_begin` / `network_idle(idle_secs, timeout)` /
  `ele_loaded`(cdp 补齐);Camoufox 端补 `wait().title_contains` / `url_contains` 做两端对齐。
- **移动端 / 触摸模拟 + 设备预设库** `tab.set().device(&Device)` / `touch(bool)` / `clear_device`;
  `Device::{iphone_13, iphone_se, ipad, pixel_7, galaxy_s9}`(UA + 视口 + DPR + mobile + 触摸)。
- **HAR 录制** `tab.har_record()` / `har_record_with(capture_bodies)` → `HarRecorder::stop() -> HarLog`
  (`entry_count` / `to_json` / `save(.har)`,含响应体;符合"请求必须先保存")。
- **`expose_function`** `tab.expose_function(name, |args| -> Result<Value>)`:把 Rust 闭包暴露为页面全局异步函数
  `window.<name>(...)`(`Runtime.addBinding` + 注入 stub + 回写 Promise;需 `Runtime.enable`,反检测取舍同 `console()`)。
- **HAR 回放** `tab.route_from_har(path, &HarReplayOptions)` / `route_from_har_log(&HarLog, ..)`(CDP,`Fetch` 拦截
  匹配:命中用 HAR 录的响应满足,未命中按 `HarNotFound::{Abort, Fallback}` 处理)。与 HAR 录制配成"录制 → 回放"闭环。
- **Camoufox 端对齐**:`tab.set_content(html)` + `tab.set().local_storage_*` / `session_storage_*`(纯 JS 通用),
  连同 `wait().title_contains` / `url_contains` —— 两端常用能力对齐(CDP 专有项不在 Juggler 端重复)。
- **`wait().new_tab` 两端补齐**:等本标签弹出的新标签 / 弹窗并返回可驱动的新 `Tab`(CDP `Target.targetCreated`、
  Camoufox `Browser.attachedToTarget`,按 BrowserContext 精准识别)。新增 Camoufox 示例 `new_tab`。
- **新示例 `cdp_extras`**:上述能力进程内离线端到端自验证(本机 Chrome 实测 ALL CHECKS PASSED,含 HAR 录制 + 回放命中/未命中)。
- **录制生成代码(codegen / recorder)** —— 录一遍页面操作就拿到**可运行的 Rust 代码**(DrissionPage 风格选择器),对标
  Playwright `codegen`。后端无关核心 `RecordedAction` / `RecordedScript`(`codegen` 模块,始终可用):`to_rust()` 出完整
  可运行程序、`to_rust_body()` 出动作语句、`to_json()` 出中间表示;`push` 自动收敛连续输入 / 去重导航 / 去重悬停。CDP 录制句柄
  `tab.recorder()`([`ChromiumRecorder`],canonical 名 `Recorder`):`start()`(导航前)→ 注入录制脚本(钩
  click/change/keydown/mouseover/拖拽,计算 `#id` > `@name:val` > `css` 选择器)+ 收 `Runtime.bindingCalled` / 主框架
  `Page.frameNavigated` → `stop() -> RecordedScript`。录制开 `Runtime.enable`(开发期行为,反检测取舍同 `console()`)。
  覆盖动作:导航 / 点击 / 输入 / 勾选 / 下拉 / 按键 / **悬停**(防抖)/ **拖拽**(HTML5 DnD + 指针,元素→元素)/
  **iframe 内动作**(元素动作带 `frame` 选择器,同源经 `frameElement`,生成 `tab.get_frame(..).ele(..)`)/
  **多标签**(录制器自动附着本标签打开的弹窗,`NewTab` 让生成代码切到 `tab_2`/`tab_3`…)。
- **无障碍快照(accessibility)** —— 把页面压成 `role "name"` 语义树 [`AxTree`],用于**抗改版断言**或**喂 LLM**(比整页 HTML 小一个
  数量级),对标 Playwright `accessibility.snapshot`。后端无关 `AxNode` / `AxTree`(`a11y` 模块):`to_outline()` / `to_json()` /
  `find_by_role` / `find_by_name`。两条获取路径:`tab.ax_tree()`(**CDP 原生** `Accessibility.getFullAXTree`,最准,仅 cdp)与
  `tab.ax_snapshot()`(**DOM 派生**,注入 ARIA 规则脚本,**cdp / camoufox 两端一致**);`tab.ax_find(role)` 便捷检索。
- **新示例 `cdp_recorder`**:录制生成代码 + 无障碍快照进程内离线端到端自验证(本机 Chrome 实测 ALL CHECKS PASSED——录到
  Navigate/Fill/Check/Select/Click 并生成正确 Rust、ax_snapshot / ax_tree 语义树按角色与名断言通过)。

## [0.3.1] - 2026-06-22

> Windows 实机点选 / 过盾精准度收尾:**高 DPI 坐标对齐** + **无头 GPU 自适应** + **反检测身份一致**,
> 并增强 **Cloudflare 内嵌 Turnstile** 过盾与 **CDP 隔离上下文 cookie**。均向后兼容,补丁号递增。

### 新增 Added

- **无头 GPU 自适应(反检测核心)**:新增 Windows 显示适配器探测 `windows_has_hardware_gpu()`(读注册表
  Class `{4d36e968-…}` 各 `DriverDesc`)。**有真实 GPU** → `--enable-gpu` 走硬件 ANGLE(真实 renderer,
  绕开 `--headless=new` 默认的 SwiftShader 软渲染——被 Turnstile 识破的破绽);**无 GPU**(VM / RDP /
  "Microsoft Basic Display Adapter")→ `--disable-gpu` 退 **D3D11 WARP**(renderer = "Microsoft Basic
  Render Driver",WebGL 可用且对该硬件**真实**;此时强行 `--enable-gpu` 会让 WebGL 创建直接失败,比软渲染
  更可疑)。`DRISSION_HEADLESS_GPU=0/software | 1/hardware` 可强制。
- **无头补全高熵 Client Hints(`full_ua_metadata`)+ 身份一致**:走"自动 mask 成 Chrome UA"分支时**必定**
  补回与该 UA 一致的 `userAgentMetadata`(品牌 GREASE + `fullVersionList` + 架构 / 位数 / 平台版本),令 UA /
  `navigator.userAgentData.brands` / 高熵 CH 三者一致呈现为 Google Chrome——消除 ① `--user-agent` 清空高熵
  CH(空 `fullVersionList` 是强无头信号)、② **非 Chrome 浏览器(如 Edge)** 上 brands 仍是原厂、与伪装的
  Chrome UA 自相矛盾 两处破绽。
- **`download_chrome` 锁定 Chrome 主版本**:自动下载 / 分发的 Chrome for Testing 与无头 mask 构造的 UA、
  补环境 `fullVersionList`、以及 `impersonate`(wreq)的 TLS / JA3 模拟档**主版本对齐**,避免版本错配被风控。
- **新增示例 `gpu_probe`**:探测无头是否拿到真实 GPU(读 WebGL `UNMASKED_RENDERER_WEBGL`),判断"真无头
  (完全无窗口)"是否可行;`exa_cf` 增 `HIDDEN=1`"视觉无头"(窗口移屏外 + 关遮挡节流,保真实 GPU 渲染)
  与过盾截图证据。

### 修复 Fixed

- **Windows 高 DPI 合成点击偏移(关键)**:Windows 下启动浏览器追加 `--force-device-scale-factor=1` +
  `--high-dpi-support=1`,令 CDP `Input.dispatchMouseEvent` 的视口坐标与页面 `getBoundingClientRect()` 的
  CSS 像素严格一致。否则在 125% / 150% 缩放的 Windows 桌面(Win11 默认即非 100%)上合成鼠标按**物理像素**
  命中(偏移 = 缩放比)→ Cloudflare Turnstile 复选框 / 易盾点选"点不中"。`yidun_click` 同步强制
  device-scale=1,并改为"图内像素**分数** × 元素**实时** rect"还原页面坐标(避开一次性 rect 过期)。
- **Cloudflare 内嵌 Turnstile 过盾**:CF 探测改 **三级定位**——① light DOM + **开放 shadow DOM** 找 iframe;
  ② iframe 在**闭合 shadow DOM** 时,用 `cf-turnstile-response` 隐藏域的"可见祖先盒"定位(CDP 屏幕坐标合成
  点击可穿透闭合 shadow / 跨域 iframe);③ host 容器兜底。并以 **token 是否产出**为过盾判据(widget 过盾后
  仍留在 DOM,不能等"挑战消失")——**支持表单内嵌 Turnstile**,不止整页托管质询。
- **CDP 隔离 BrowserContext 的 cookie**:`get_cookies` / `cookies` / `set_cookies` 对 `new_tab_with` 建的
  隔离上下文标签带上 `browserContextId`,否则读写落到 default context、本标签拿不到 / 用不上其 cookie
  (普通标签行为不变)。
- **发布二进制的输出目录**:`yidun_click` 等支持 `YIDUN_OUT` 指向可写目录——`CARGO_MANIFEST_DIR` 是**编译期**
  路径,发布二进制在别的机器上不存在 → 截图 / 叠加图会静默写不出。

### 变更 Changed

- **发布产物体积优化**:`[profile.release]` 设 `opt-level="z"` + `lto=true` + `codegen-units=1` + `strip=true`
  (零环境交付的 Windows 测试二进制尽量小);**保留 `panic="unwind"`**(tokio / mutex 等依赖栈展开,`abort`
  会让一次 panic 直接杀进程)。

## [0.3.0] - 2026-06-21

### 新增 Added

- **AI 编程技能文档 `docs/SKILL.md`(AI 必读)**:面向 AI 编程助手的**接口/feature/构建规则权威速查**,覆盖
  从基础(Page/元素/网络)到**点选验证码点击全流程**的复制即用代码,并明确「Camoufox 系示例必须
  `--no-default-features --features camoufox`」等铁律与易错点。README(中英)顶部声明:AI 基于本库开发须先遵循该 skill
  (不支持 skill 机制可忽略)。所有代码片段按真实签名核验(`ClickWord::solve(&[String])`、`tab.human_click(&[(f64,f64)])`、
  `tab.image_view`/`fetch_image`、`apply_pointer_stealth` 仅 camoufox 等)。
- **示例运行命令全部修正(复制即用)**:v0.2 默认后端翻为 cdp 后,~45 个 camoufox/slider/ocr 示例头注释的运行命令
  过时(漏 feature 或漏 `--no-default-features`)。逐个改为正确命令(camoufox 系 `--no-default-features --features camoufox`、
  滑块 `--features slider`、camoufox+ocr `camoufox,ocr`),并修正 `examples/README.md` 与 `Cargo.toml` 顶部注释;
  五组 feature 组合全部实测 `cargo build --examples` 通过。
- **Session 浏览器 TLS / JA3 / JA4 + HTTP2 指纹伪装**(新增可选 `--features impersonate`):给 Session(HTTP)
  模式套**真实浏览器的 TLS 握手指纹**,让"浏览器过盾 → cookie 灌进 Session → 纯 HTTP 接力"的双模不再被现代
  WAF(Akamai / Cloudflare / DataDome)凭 Rust 默认 TLS 指纹一眼拦下——这是**网络层的"补环境"**。
  `SessionOptions::new().profile(BrowserProfile::Chrome)`(另有 `Firefox`/`Safari`/`Edge`;`None`=默认不伪装)。
  底层基于 `wreq` + `wreq-util`(reqwest 硬分叉 + BoringSSL,内置 100+ 浏览器模拟档),抽象为 enum 后端
  (纯 reqwest / wreq),**重定向/cookie 循环单写一份**、非破坏。开启 profile 时 UA + 默认头由模拟档驱动(避免与
  TLS 指纹打架)。**默认关、零成本**(默认构建不引 BoringSSL;`impersonate` imply `camoufox`,需 `cmake` + `nasm`)。
  示例 `session_tls` 真机自验证:同进程 `None` vs `Chrome` 打 `tls.peet.ws`,**JA3/JA4/Akamai 指纹均改变**
  (`t13d1011h2…`→`t13d1516h2…`、UA Firefox→Chrome137),ALL CHECKS PASSED。**Windows 支持**:`x86_64-pc-windows-gnu`
  交叉编译已实测(BoringSSL+mingw+nasm,bindgen 喂 sysroot,产出真 `PE32+ .exe`),封装 `scripts/win-cross-build.sh`;
  原生 Windows 走 MSVC(VS Build Tools + nasm)或 mingw。设计见 `docs/TLS指纹.md`。
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

[0.3.2]: https://github.com/MageGojo/drission-rs/compare/v0.3.1...HEAD
[0.3.1]: https://github.com/MageGojo/drission-rs/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/MageGojo/drission-rs/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/MageGojo/drission-rs/compare/v0.1.1...v0.2.0
[0.1.1]: https://github.com/MageGojo/drission-rs/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/MageGojo/drission-rs/releases/tag/v0.1.0
