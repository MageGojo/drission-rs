# 更新日志 / Changelog

本项目的所有重要变更都记录在此文件。

格式遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/),
版本号遵循 [语义化版本](https://semver.org/lang/zh-CN/)。

## [Unreleased]

## [0.6.4] - 2026-09-07

### 变更 Changed

- README 恢复成库文档结构：上手路径仍是 Chrome；Lightpanda 单独成节，不再打断示例。
- `drission` 与 `drission-cli` 都是 **0.6.4**。

## [0.6.3] - 2026-09-06

库侧新增 Lightpanda 引擎变体。`drs` 仍是本机 Chrome / Agent / MCP，不接 Lightpanda。

### 新增 Added

- **Lightpanda 适配**（CDP 引擎变体，接口不变）：`BrowserOptions::new().lightpanda()` 启动
  `lightpanda serve`，连上后仍是 `ChromiumBrowser` / `ChromiumTab`。找不到二进制时从 GitHub
  nightly 下载。`connect` 现接受 `ws://` / `wss://`（含 Lightpanda Cloud）。
  本机可视化、强反爬继续用 Chrome；服务器简单听接口换引擎即可。见 `docs/Lightpanda.md`。
- `ChromiumElement::image_bytes()`：从 `data:` / canvas / `fetch(src)` 取原图，不靠
  `Page.captureScreenshot`（Lightpanda 截图常是占位）。`ocr_image` / `ocr_calc` / `ocr_code` 走这条路径。
- 案例 `chuhe_price`：`get` / `click` / `input` / `listen` 听初和站报价，不在页面里 fetch。
  `./scripts/pack-chuhe-dist.sh` 打进 `dist/chuhe`，旁边内置 Lightpanda。
  见 `docs/案例-chuhe价格监听.md`。

### 变更 Changed

- `drission` 与 `drission-cli` 都是 **0.6.3**。`drs` 功能未变（默认本机 Chrome）。

## [0.6.2] - 2026-08-21

库 `drission` 和命令行 `drs`（`drission-cli`）同一号。Agent 编排、产品 CLI、语义 MCP。

### 新增 Added

- **Agent 编排层** `tab.agent()` / `browser.agent()`:`observe()` 给 AX 大纲 + `{role,name}`;`find` / `click` / `type_text` / `wait_for`。LLM 不直接写 `querySelector`。见 `docs/Agent运行时.md`。
- **统一 `Locator`**:`role` / `name` / `text` / `css` / `xpath` / `id` / `label` / `placeholder`;「登录按钮」解析成 role+name。
- **链式等待** `tab.wait().element("#x").visible().timeout(...)`,以及 `url` / `title` / `text` / `function` / `idle`。
- **媒体入口** `tab.capture()`:`screenshot` / `full_page` / `pdf` / `mhtml` / `html` / `har`;`tab.download()` / `tab.upload()`。
- **`TaskScheduler`**:在 `ChromiumPool` 上加任务超时。并发、代理、重试仍在池里。
- Resolver 最后一档:可见字旁边的可点区域;`--features ocr` 时对无字按钮截图识别。
- `tab.wait().popup()` / `network().idle()|.url()` / `download()`。
- 分层池:`acquire_context` / `acquire_tab` / `proxies()`;`recycle()` 按空闲时间和内存/CPU 阈值回收。
- 示例 `cdp_agent`(离线)。
- **产品级 CLI**：`drs browser list`、`drs tab list|open|use|close`、`drs network inspect|replay`、`drs record`、`drs replay`、`drs pdf`、`drs inspect`、`drs console`、`drs download`。旧 `open`/`tabs`/`use`/`close` 仍可用。
- **语义 MCP**：`browser_network` / `browser_console` / `browser_download` 进入默认 `core`；`browser_pdf` / `browser_inspect` / `browser_record` 在 `browser` toolset。
- 库：`listen.peek()` / `console.peek()`（不消费缓冲），给 inspect 用。

### 变更 Changed

- `drission` 与 `drission-cli` 都是 **0.6.2**。`drs --version` 与库、GitHub tag `v0.6.2` 同一号。
- README / 安装说明只写这一版，不再并列两套号。
- AI 入口：`docs/产品级CLI.md` 可复制配方；仓库 skill 与 `~/.claude/skills/drs-browser` 同步；抓 XHR 用默认 core 的 `browser_network`。

## [0.6.1] - 2026-08-21

`drs` 补上 Context / 网络门面 / 磁盘 profile；README 改成对着 0.6.x 写。

### 新增 Added

- `ChromiumBrowser::named_contexts()`
- `drs context list|use|close`，`open --context`
- `drs network block|mock|rewrite|record|stop`；`listen start` 加 `--method`
- `drs profile list|show`，`serve --profile <id>`
- MCP（`tools=browser|all`）：`browser_context_*`、`network_block/mock/rewrite/record/stop`、`profile_list/show`

### 变更 Changed

- `drission` 0.6.1，`drission-cli` 0.3.3
- README 不再写 crates.io 停在旧版 / 请用 v0.4.0 tag

## [0.6.0] - 2026-08-21

> **运行时分层 + Context / Profile / 网络门面**:账号身份不再堆进巨大 `Browser`;`drs` MCP 默认只暴露浏览核心。

### 新增 Added

- **运行时分层**:`src/runtime` 按 Browser / HTTP / AI / Task 再导出,避免继续把能力堆进巨大 `Browser` 类。设计见 `docs/运行时分层.md`。
- **一等公民 `BrowserContext`**:`browser.context("account_001")` / `new_context(spec)`;Context 管 Cookie / Proxy / UA / locale / timezone / headers / permissions / viewport。同名复用,关 Context 才 dispose。
- **`ProfileManager`**:磁盘 `profiles/<id>/{manifest.json,chrome/,.lock}`;`acquire` 排他租约后 `launch`。仅用于自有或已授权账号。
- **网络门面** `tab.network().filter("/api/").method("POST").listen/block/mock/rewrite/record`;统一 `NetRequest` / `NetResponse`。旧 `listen` / `intercept` / HAR 保留。
- **`ChromiumPool::acquire`**:领取并发槽 + worker,Deref 到 Browser,再 `context` / `new_tab`。
- 离线示例 `cdp_context`。
- MCP `browser_click` / `browser_type` / `browser_press` / `browser_wait` 支持可选
  `return_snapshot=true`：成功后附带轻量 `page` 快照（refs + 短正文，跳过 markdown）。
- MCP 工具面分组：`DRS_MCP_TOOLS` / `drs mcp --tools` / `drs setup --tools`，取值
  `core`（默认，~11 个浏览核心工具）/ `browser` / `identity` / `all`。
- MCP `ServerInfo.instructions` 内嵌稳定核心 schema 与 Cursor server id 说明。
- 针对失效 snapshot ref 返回错误码 `stale_ref`，hint 明确要求重新 `browser_snapshot`。

### 变更 Changed

- `drission` 升到 0.6.0,`drission-cli` 升到 0.3.2。
- Skill / setup 统一说明：配置 key 为 `drs`，Cursor Agent 调用 id 通常为 `user-drs`。
- `drs setup` 写入 `env.DRS_MCP_TOOLS`（默认 `core`）。

## [0.5.1] - 2026-07-31

> **AI 读页 + MCP 防卡退**:让 Agent 一条命令读懂当前页(大纲 / refs / Markdown),并避免重页把 MCP 卡死。

### 新增 Added

- **`drs snapshot` / MCP `browser_snapshot`**:interesting-only 语义大纲 + 可交互 `ref=eN` + 短正文;
  `click` / `type` 支持 `ref:e1`(写入 `data-drs-ref`)。见 `src/ai_snapshot.rs`。
- **HTML→Markdown(`htmd`)**:`src/html_md.rs`;`snapshot` / `extract` 默认附带 `markdown`,
  另增 `drs markdown` / MCP `browser_markdown`。
- 文档:`docs/AI页面快照.md`、`docs/工具与约定.md`;skill / `docs/CLI.md` 同步首选读页路径。

### 变更 Changed

- `drission` 升到 0.5.1,`drission-cli` 升到 0.3.1。

### 修复 Fixed

- MCP / daemon RPC 默认 60s 超时(`DRS_MCP_TIMEOUT_MS` / `DRS_DAEMON_TIMEOUT_MS`),超时返回 `timeout`
  而非挂死。
- `browser_html` 默认截断 20 万字符;`extract` 的 ax 短超时软失败(`outlineError`);
  `AX_SNAPSHOT_JS` 加时间预算,降低重 SPA 卡死概率。

## [0.5.0] - 2026-07-26

> **默认后端(CDP)能力大补齐 + 滑块后端无关**:把此前只在 Camoufox 后端的登录态存取、代理池、滑块
> 求解搬到默认的 CDP 后端,并深化每浏览器指纹、补上计算题验证码,MCP 工具面同步扩齐。

### 新增 Added

- **登录态 storageState 上 CDP**:`ChromiumTab` 的强类型 `storage_state` / `save_storage_state` /
  `load_storage_state` / `apply_storage_state`(cookie 全量 + localStorage **+ sessionStorage**,对齐
  camoufox)。见 `src/cdp/storage.rs`。
- **滑块求解后端无关**:抽出后端无关核心 `src/slider.rs`(类型 + 页面内算法 JS + `SliderTab` trait),
  Camoufox 与 CDP 各实现一次,**CDP 后端现在也能** `tab.slider_gap` / `solve_slider` /
  `solve_geetest_slide` / `solve_dingxiang_slide`。`slider` feature 不再强制 camoufox。
- **计算题验证码**:`ocr::eval_calc`(中文数字 / 全角 / 中文运算符 / 优先级 / 一元负号 / 除零保护)+
  `Ocr::recognize_calc` + 两后端 `tab.ocr_calc`。见 `src/ocr/calc.rs`。
- **CDP 代理池 + 健康检查**:`ProxyPool` / `ProxyHealth` / 出口地理探测从 camoufox 门解耦到两后端,
  `ChromiumPoolOptions::proxy_pool`——每任务取**健康**代理 + **与出口地理自洽**的时区 / 语言。
- **每浏览器指纹深化**:`CdpFingerprint` 新增 **ClientRects** / **字体枚举(measureText)** /
  **Battery** farbling 与 **WebGPU** adapter info 伪装(跟随 WebGL 派生;同 OS 保真模式不撒谎)。
- **drs MCP 新工具**:`browser_press`、`browser_save_state` / `browser_load_state`、`browser_ocr`、
  `browser_solve_slider`;全局 skill 与 `docs/CLI.md` 工具清单补齐。
- **`docs/后端能力矩阵.md`**:CDP(默认)vs Camoufox 逐条能力对照 + 选后端建议。

### 变更 Changed

- OCR 模型热替换(`set_default_ocr` / `tab.ocr_image`)改为**后端无关**,CDP 与 Camoufox 共用同一热替换槽。
- `drission` 升到 0.5.0,`drission-cli` 升到 0.3.0。

### 修复 Fixed

- `--all-features --all-targets` 下 30 个 Camoufox 示例的 prelude 类型冲突;新增 CI「双后端并存」回归门禁。
- CDP `ChromiumTab::ocr_image` 不再对缺失 / 坏图静默跑空字节;删除死变体 `Error::NotImplemented`;
  修 `ocr::template_gate` 单调性单测的取样点。

## [0.4.0] - 2026-07-07

> **drs 变成给 Cursor / Codex 用的浏览器采集 MCP**:当 AI 需要抓「难获取」的数据(登录态 / 反爬 /
> Cloudflare / 纯前端渲染)时,通过一个持久真实浏览器去打开、渲染、抓取,而不是用 curl / WebFetch 抓空壳。
> 同时把账号 / Profile 运行时治理 sidecar 沉淀为库能力。

### 新增 Added

- **MCP 持久浏览器**:`drs mcp` 默认 attach 常驻 `drs serve` daemon 的同一个浏览器,标签与登录态跨 MCP
  重启存活;daemon 用固定 profile 且脱离启动者进程组常驻;`--standalone` 回退进程内浏览器。见
  [`docs/mcp-持久浏览器.md`](docs/mcp-持久浏览器.md)。
- **`drs setup`**:一条命令自动为 Cursor(`.cursor/mcp.json`)与 Codex(`~/.codex/config.toml`)写好 MCP
  配置,`command` 用 drs 绝对路径,幂等合并、不覆盖其它 server。
- **免 Rust 安装**:`install/drs-install.sh`(mac/linux)/ `install/drs-install.ps1`(windows)从 GitHub
  Release(GitCode 备用镜像)下载预编译静态 `drs`;新增 `.github/workflows/release.yml` 按 `v*` tag 产出
  mac / linux(musl x64+arm64)/ windows 二进制并附校验和。
- **账号 / Profile 身份治理 sidecar**:`identity_report` / `IdentityPoolReport`(准入、漂移、lifecycle、
  熵预算、容量计划、隔离/修复动作队列)沉淀进库;`drs identity-*`、`identity-job run`、`identity-ledger
  query/explain/compact/dashboard` 提供运行时租约、失败判责、冷却、runtime risk 与审计流水。

### 变更 Changed

- README(中/英)重定位为「给 Cursor / Codex 的浏览器采集 MCP + 免 Rust 安装 + AI 一键话术」。
- `drission-cli` 升到 0.2.0,依赖 `drission` 0.4.0。

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

[0.6.4]: https://github.com/MageGojo/drission-rs/compare/v0.6.3...v0.6.4
[0.6.3]: https://github.com/MageGojo/drission-rs/compare/v0.6.2...v0.6.3
[0.6.2]: https://github.com/MageGojo/drission-rs/compare/v0.6.1...v0.6.2
[0.6.1]: https://github.com/MageGojo/drission-rs/compare/v0.6.0...v0.6.1
[0.6.0]: https://github.com/MageGojo/drission-rs/compare/v0.5.1...v0.6.0
[0.5.1]: https://github.com/MageGojo/drission-rs/compare/v0.5.0...v0.5.1
[0.5.0]: https://github.com/MageGojo/drission-rs/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/MageGojo/drission-rs/compare/v0.3.2...v0.4.0
[0.3.2]: https://github.com/MageGojo/drission-rs/compare/v0.3.1...v0.3.2
[0.3.1]: https://github.com/MageGojo/drission-rs/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/MageGojo/drission-rs/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/MageGojo/drission-rs/compare/v0.1.1...v0.2.0
[0.1.1]: https://github.com/MageGojo/drission-rs/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/MageGojo/drission-rs/releases/tag/v0.1.0
