//! [`ChromiumBrowser`]:启动本机 Chrome/Edge/Brave/Chromium(无头或有头),或**接管**已开启
//! CDP 调试端口的浏览器 / Electron 应用。标签创建后返回 [`ChromiumTab`],由 [`CdpCore`] 驱动。

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::time::sleep;

use crate::cdp::core::CdpCore;
use crate::cdp::options::{ChromiumContextOverride, ChromiumOptions};
use crate::cdp::tab::ChromiumTab;
use crate::cdp::{container, fetch, locate, stealth};
use crate::protocol::Connection;
use crate::{Error, Result};

/// 一个 Chromium 浏览器(自启动或接管)。
pub struct ChromiumBrowser {
    conn: Connection,
    // 内部可变:`quit(&self)` 需要取出子进程(对齐 camoufox `Browser::quit(&self)`)。
    child: std::sync::Mutex<Option<tokio::process::Child>>,
    user_data_dir: Option<PathBuf>,
    /// 是否启用反检测(决定 attach 时是否注入兜底脚本;`Runtime.enable` 始终不调用)。
    stealth: bool,
    /// 是否无头(无头时额外注入屏幕尺寸补丁,消除 800x600 这个无头破绽)。
    headless: bool,
    /// 无头补全高熵 Client Hints(`ChromiumOptions::full_ua_metadata`,opt-in)。
    /// 仅当为 `true` 且 `ua_override`/`ua_full_version` 就绪时,attach 才用 CDP 补 `userAgentMetadata`。
    full_ua_metadata: bool,
    /// 实际经 `--user-agent` 下发的 UA 串(= 无头 mask 路径构造的精简 UA);补环境时复用为
    /// `Emulation.setUserAgentOverride` 的 `userAgent`,与浏览器级 UA 一致。仅 mask 路径触发时为 `Some`。
    ua_override: Option<String>,
    /// Chrome 完整版本号(如 `149.0.7827.115`),补 `fullVersionList` 用。
    ua_full_version: Option<String>,
    /// 平台版本(CH 高熵 `platformVersion`,如 mac 的 `26.5.1`);据真机探测。
    platform_version: Option<String>,
    /// 下载目录(`ChromiumOptions::download_path`):每个新标签 attach 时设下载行为并传给 `CdpCore`。
    download_dir: Option<PathBuf>,
    /// 导航前注入脚本(`ChromiumOptions::init_scripts`):每个新标签 attach 时按序注入(深指纹伪装等),
    /// 在内置反检测脚本之后。
    init_scripts: Vec<String>,
    /// 容器 / 无 GPU Linux「补环境」WebGL renderer 改写脚本(`Some` 即在 attach 时注入,见
    /// [`container`](crate::cdp::container))。在内置反检测脚本之后、用户 init_scripts 之前注入,
    /// 把软渲染破绽(SwiftShader/llvmpipe)改写成常见 Linux 桌面 GPU。
    container_js: Option<String>,
    /// Windows:把 Chrome 绑入 KILL_ON_JOB_CLOSE 的 Job,quit/Drop 时级联杀掉渲染/GPU 等子进程
    /// (`kill_on_drop` 只杀主进程,会留孤儿)。仅自启动时为 `Some`;接管不接管生命周期。
    #[cfg(windows)]
    job: std::sync::Mutex<Option<crate::transport::JobHandle>>,
}

impl ChromiumBrowser {
    /// 启动本机浏览器(`headless=true` 无头),开 CDP 调试端口并连接。
    ///
    /// 浏览器**默认 Google Chrome**,**开箱即用**(对标 CloakBrowser 首次运行自动下载):
    /// 1. 先定位系统已装的可执行文件(见 [`locate::chrome_path`]):`CHROME_BIN`/`DRISSION_CHROME`
    ///    环境变量 → 常见安装路径(Windows 含用户级 `%LOCALAPPDATA%`)→ Windows 注册表 `App Paths`
    ///    → 系统 `PATH`;
    /// 2. **都找不到时,自动从 Chrome for Testing 下载并缓存**(见 [`fetch::ensure_chrome`])。
    ///
    /// 一行起步:**有头 + 反检测开箱即用**(对齐 camoufox `Browser::launch_default`)。
    /// 无头一行 `launch(ChromiumOptions::new().headless(true))`。
    pub async fn launch_default() -> Result<Self> {
        Self::launch(ChromiumOptions::new()).await
    }

    /// 按 [`ChromiumOptions`] 启动(对齐 camoufox `Browser::launch(BrowserOptions)`):有头/无头、
    /// 窗口大小、反检测开关、指定可执行文件、持久 profile、UA/地区/代理覆盖、额外参数。
    ///
    /// **默认启用反检测**(过 Cloudflare 盾的基础设施):反检测启动参数 + 导航前注入 +
    /// **不调用 `Runtime.enable`**(经典 CDP 探测泄漏)。找不到系统浏览器且未指定 `binary_path`
    /// 时**自动下载** Chrome for Testing。只想定位/不下载用 [`Self::find_chrome`]。
    pub async fn launch(opts: ChromiumOptions) -> Result<Self> {
        opts.validate()?;
        let exe = match opts.binary_path.clone() {
            Some(p) => p,
            None => fetch::ensure_chrome().await?,
        };
        let persist = opts.user_data_dir.is_some();
        let dir = match opts.user_data_dir.clone() {
            Some(d) => d,
            None => std::env::temp_dir().join(format!(
                "drission-cdp-{}-{}",
                std::process::id(),
                now_ms()
            )),
        };
        Self::launch_inner(&exe, dir, persist, &opts).await
    }

    /// 定位本机 Chrome/Edge/Brave/Chromium 可执行文件(**仅定位、不下载**)。
    /// 便于诊断“为何没找到浏览器”。要「找不到就自动下载」用 [`Self::download_chrome`]。
    pub fn find_chrome() -> Result<PathBuf> {
        locate::chrome_path()
    }

    /// 确保本机有可用的 Chrome:先定位系统已装,**找不到则自动下载** Chrome for Testing 到缓存,
    /// 返回其可执行文件路径(不启动)。对标 CloakBrowser / Camoufox 的「首次运行自动下载」。
    ///
    /// 跨平台预取(如在 mac 上为分发预取 `win64`)用 [`fetch::download_chrome_for`]。
    pub async fn download_chrome() -> Result<PathBuf> {
        fetch::ensure_chrome().await
    }

    /// 用**指定的可执行文件**启动浏览器(`headless=true` 无头),开 CDP 调试端口并连接。
    /// 当自动探测找不到、或要强制使用某个浏览器(Chrome/Edge/Brave/Chromium)时用它。
    ///
    /// 使用**临时** user-data-dir(每次全新、退出即删),不持久化登录态/缓存。
    /// 要复用同一份 profile(持久登录、记住网站状态)用 [`Self::launch_with_profile`]。
    pub async fn launch_with(exe: impl AsRef<Path>, headless: bool) -> Result<Self> {
        Self::launch(
            ChromiumOptions::new()
                .headless(headless)
                .binary_path(exe.as_ref().to_path_buf()),
        )
        .await
    }

    /// 用**指定可执行文件 + 持久 user-data-dir** 启动:profile 跨进程复用,**登录态/Cookie/缓存
    /// 持久化**,[`quit`](Self::quit) 退出时**不删除**该目录(下次启动即恢复登录态)。
    ///
    /// 适合"自动化助手记住网站登录"这类场景。同一 profile 目录同一时刻只应有一个浏览器在用。
    pub async fn launch_with_profile(
        exe: impl AsRef<Path>,
        headless: bool,
        user_data_dir: impl AsRef<Path>,
    ) -> Result<Self> {
        Self::launch(
            ChromiumOptions::new()
                .headless(headless)
                .binary_path(exe.as_ref().to_path_buf())
                .user_data_dir(user_data_dir.as_ref().to_path_buf()),
        )
        .await
    }

    /// 启动实现:`persist=false` 临时 profile(quit 删),`persist=true` 持久 profile(quit 不删)。
    /// 反检测(`opts.stealth`,默认开)在此落地:反检测启动参数 + 后续 attach 注入兜底脚本 +
    /// 全程不调用 `Runtime.enable`(见 `attach`)。
    async fn launch_inner(
        exe: &Path,
        dir: PathBuf,
        persist: bool,
        opts: &ChromiumOptions,
    ) -> Result<Self> {
        std::fs::create_dir_all(&dir)
            .map_err(|e| Error::msg(format!("CDP: 建 user-data-dir 失败: {e}")))?;
        // 持久 profile 复用时上次运行可能残留单例/端口标记:
        // - 残留的 `DevToolsActivePort` 会让我们读到**已失效的旧端口** → 连 `/json/version` 失败;
        // - 残留的 `SingletonLock` 等会让新 Chrome 误判 "profile 占用中" 而直接退出。
        // 启动前清掉这些陈旧标记(本方法约定同一 profile 同一时刻只有一个受控浏览器)。临时 profile 下为 no-op。
        for stale in [
            "DevToolsActivePort",
            "SingletonLock",
            "SingletonCookie",
            "SingletonSocket",
            // 删除版本标记:避免「锁定较旧主版本」时 Chrome 因 profile 由更新版本创建而拒绝打开(降级保护)。
            "Last Version",
        ] {
            let _ = std::fs::remove_file(dir.join(stale));
        }
        let mut cmd = tokio::process::Command::new(exe);
        // 受库管理的基础参数(用户参数不得覆盖,见 ChromiumOptions::validate)。
        // 用**已知空闲端口**而非 `0`:部分 Chromium 变体(如 CloakBrowser)**不写 `DevToolsActivePort` 文件**,
        // 若用 `0` 让内核随机选端口,我们就无从得知端口、连不上(现象:浏览器开了但页面空白卡死)。
        // 指定一个先探到的空闲端口,即可直接轮询 `http://127.0.0.1:<port>/json/version` 拿 WS 端点。
        let debug_port = pick_free_port()?;
        cmd.arg(format!("--remote-debugging-port={debug_port}"))
            .arg(format!("--user-data-dir={}", dir.display()));
        if opts.stealth {
            // 反检测启动参数(核心:关掉 AutomationControlled,不传 --enable-automation)。
            for a in stealth::stealth_args() {
                cmd.arg(a);
            }
        } else {
            cmd.arg("--no-first-run")
                .arg("--no-default-browser-check")
                .arg("--disable-background-networking")
                .arg("--disable-features=Translate,OptimizationHints");
        }
        if let Some((w, h)) = opts.window_size {
            cmd.arg(format!("--window-size={w},{h}"));
        }
        // Windows 高 DPI 修正(关键):把设备缩放强制为 1,使 CDP `Input.dispatchMouseEvent` 的
        // 视口坐标与页面 `getBoundingClientRect()` 的 CSS 像素严格一致。否则在 125%/150% 缩放的
        // Windows 桌面(Win11 默认即非 100%)上,合成鼠标事件按**物理像素**命中(偏移=缩放比)→
        // Cloudflare Turnstile 复选框等"点不中"(mac Retina 由 fingerprint 维持 dpr=2,且其 Turnstile
        // 走自动放行不依赖点击,故仅在 Windows 上强制)。
        #[cfg(windows)]
        {
            cmd.arg("--force-device-scale-factor=1")
                .arg("--high-dpi-support=1");
        }
        if let Some(server) = &opts.proxy {
            cmd.arg(format!("--proxy-server={server}"));
        }
        // UA:**走 `--user-agent` 启动参数**(浏览器级,覆盖所有帧含 Turnstile 跨域 iframe ——
        // 对标 DrissionPage `set_user_agent`;per-session 的 Emulation 覆盖到不了 OOPIF 子帧)。
        // 显式 UA 优先;否则无头 + stealth + mask_ua 时,探测真实 Chrome 版本构造"去 Headless"的精简 UA。
        // `auto_mask` = 走"自动构造精简 UA"分支(此时 `--user-agent` 会清空高熵 CH,补环境据此触发)。
        let auto_mask = opts.user_agent.is_none() && opts.headless && opts.stealth && opts.mask_ua;
        // 版本号(无头 mask 构造精简 UA、以及补环境 fullVersionList 都要):**跨平台**探测。
        // ⚠ Windows 下 GUI 版 Chrome/Edge 的 `exe --version` 不往 stdout 打印(还会误启动浏览器),
        // 故 Windows 走 PE 文件版本资源,其它平台才用 `--version`(见 [`probe_chrome_version`])。
        let chrome_version = if auto_mask || opts.full_ua_metadata {
            probe_chrome_version(exe).await
        } else {
            None
        };
        let ua = if let Some(u) = &opts.user_agent {
            Some(u.clone())
        } else if auto_mask {
            chrome_version
                .as_deref()
                .and_then(stealth::parse_chrome_major)
                .map(stealth::reduced_ua)
        } else {
            None
        };
        if let Some(ua) = &ua {
            cmd.arg(format!("--user-agent={ua}"));
        }
        // 反检测一致性(关键):只要走了"自动 mask 成 Chrome UA"分支,就**必须**补回与该 UA 一致的
        // `userAgentMetadata`。因为 `--user-agent` 只改 UA 串:① 会清空高熵 CH(`fullVersionList`/
        // `architecture`/`platformVersion` 等留空 —— 强无头信号);② 在**非 Chrome 浏览器(如 Edge)**上
        // `navigator.userAgentData.brands` 仍是原厂品牌(Microsoft Edge),与伪装的 Chrome UA **自相矛盾**
        // (UA=Chrome 而 brands=Edge,被严格 CF 一眼识破)。补回后 UA / 品牌 / 高熵 CH 三者一致呈现为
        // Google Chrome。`full_ua_metadata` 仍可在显式 UA 等非 auto_mask 场景强制开启。
        let want_meta = ua.is_some() && opts.stealth && (auto_mask || opts.full_ua_metadata);
        let ua_override = if want_meta { ua.clone() } else { None };
        let (ua_full_version, platform_version) = if want_meta {
            (chrome_version.clone(), detect_platform_version().await)
        } else {
            (None, None)
        };
        // 地区:走启动参数 / 环境变量(比 CDP Emulation 覆盖更干净)。
        if let Some(loc) = &opts.locale {
            cmd.arg(format!("--lang={loc}"));
            cmd.env("LANGUAGE", loc);
        }
        if let Some(tz) = &opts.timezone {
            cmd.env("TZ", tz);
        }
        for a in &opts.args {
            cmd.arg(a);
        }
        // 容器 / 无 GPU Linux「补环境」判定(仅 Linux 触发,见 cdp::container):用于下方有头补
        // `--enable-unsafe-swiftshader`(软件 WebGL 可用)与 attach 时注入 WebGL renderer 改写脚本。
        let spoof_gpu = container::should_spoof(opts.stealth, opts.spoof_container);
        // 预生成「补环境」注入脚本:把软渲染 renderer(SwiftShader/llvmpipe)改写成常见 Linux 桌面 GPU。
        // 用户已自带 WebGL 覆盖(CdpFingerprint persona 等)且未显式要求补环境时,尊重用户、不重复注入。
        let container_js = if spoof_gpu && opts.stealth {
            let user_has_webgl = opts.init_scripts.iter().any(|s| s.contains("37446"));
            if user_has_webgl && opts.spoof_container != Some(true) {
                None
            } else {
                Some(container::spoof_js(
                    &container::spoof_vendor(opts.webgl_vendor.as_deref()),
                    &container::spoof_renderer(opts.webgl_renderer.as_deref()),
                ))
            }
        } else {
            None
        };
        // Linux 服务器(无显示器即可,用无头;但有无界面之外还有两条**硬条件**,否则 Chromium 直接崩):
        // - 以 root 运行(Docker / 多数云服务器默认 root)→ user-namespace sandbox 起不来,报
        //   "No usable sandbox! / Failed to move to new namespace",必须 `--no-sandbox` 旁路;
        // - 容器内 `/dev/shm` 常仅 64MB,渲染进程写爆后页面崩(Tab crashed)→ `--disable-dev-shm-usage`
        //   改用临时文件。故 Linux 自动补这两个;用户已在 args 显式给出则不重复,`DRISSION_NO_SANDBOX=0`
        //   可在已配好非 root 沙箱的环境关掉 sandbox 旁路。
        #[cfg(target_os = "linux")]
        {
            let has = |needle: &str| opts.args.iter().any(|a| a.starts_with(needle));
            let keep_sandbox = matches!(
                std::env::var("DRISSION_NO_SANDBOX").ok().as_deref(),
                Some("0") | Some("off") | Some("false")
            );
            if !keep_sandbox && !has("--no-sandbox") {
                cmd.arg("--no-sandbox");
            }
            if !has("--disable-dev-shm-usage") {
                cmd.arg("--disable-dev-shm-usage");
            }
            // 有头(Xvfb)+ 容器/无 GPU:补 `--enable-unsafe-swiftshader`,让**软件 WebGL 可用**。
            // 近期 Chrome(≥121)默认禁用 SwiftShader 的 WebGL,不加这个 `getContext('webgl')` 直接返回
            // null(=完全没有 WebGL,比软渲染更可疑),后续 renderer 改写也就无从谈起。无头路径在下方
            // GPU 策略里本就始终带该旁路,故这里仅补"有头"这一缺口(避免重复)。
            if spoof_gpu && !opts.headless && !has("--enable-unsafe-swiftshader") {
                cmd.arg("--enable-unsafe-swiftshader");
            }
        }
        if opts.headless {
            cmd.arg("--headless=new");
            // GPU 策略(无头反检测核心,**按是否有真实 GPU 自适应**):
            // - **有真实 GPU** → `--enable-gpu` 走硬件 ANGLE(真实 renderer,关掉 `--headless=new` 默认
            //   强制的 SwiftShader 软渲染——SwiftShader 是被 Turnstile 识破的无头破绽);mac 显式 Metal。
            // - **无真实 GPU**(VM / RDP / "Microsoft Basic Display Adapter")→ `--disable-gpu`:Windows 下
            //   退 **D3D11 WARP**(WebGL renderer = "Microsoft Basic Render Driver",**WebGL 可用且对该硬件
            //   真实**)。若此时仍 `--enable-gpu`,无 D3D11 设备会让 **WebGL 创建直接失败(`ok:false`)**——
            //   这比软渲染**更可疑**(真机几乎不会 WebGL 全废)。`--enable-unsafe-swiftshader` 再兜一层。
            // Windows 经注册表显示适配器自动探测;`DRISSION_HEADLESS_GPU` 可强制(0/software 软、1/hardware 硬)。
            #[cfg(windows)]
            let auto_hardware = windows_has_hardware_gpu();
            #[cfg(not(windows))]
            let auto_hardware = true; // mac/linux 默认按有 GPU(mac Metal)。
            let use_hardware = match std::env::var("DRISSION_HEADLESS_GPU").ok().as_deref() {
                Some("0") | Some("off") | Some("software") | Some("swiftshader") => false,
                Some("1") | Some("on") | Some("hardware") => true,
                _ => auto_hardware,
            };
            if use_hardware {
                cmd.arg("--enable-gpu").arg("--enable-unsafe-swiftshader");
                #[cfg(target_os = "macos")]
                cmd.arg("--use-angle=metal");
                #[cfg(windows)]
                cmd.arg("--ignore-gpu-blocklist");
            } else {
                // 软件渲染:`--disable-gpu`。Windows 下据此退 **D3D11 WARP**(renderer = "Microsoft Basic
                // Render Driver",对无 GPU 的 Windows 机**最真实**,WARP 恒可用);**不**加
                // `--enable-unsafe-swiftshader`(它会把 renderer 抢成 Chrome 自带 SwiftShader,真实度更低)。
                // 非 Windows(linux 无 GPU)才用 SwiftShader 兜底,否则 WebGL 可能不可用。
                cmd.arg("--disable-gpu");
                #[cfg(not(windows))]
                cmd.arg("--enable-unsafe-swiftshader");
            }
        }
        cmd.arg("about:blank");
        cmd.stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let child = cmd
            .spawn()
            .map_err(|e| Error::msg(format!("CDP: 启动浏览器失败({}): {e}", exe.display())))?;

        // Windows:把 Chrome 主进程绑入 KILL_ON_JOB_CLOSE 的 Job(其渲染/GPU 子进程默认随之入 Job),
        // 使 quit/Drop 能级联杀整棵树,避免 `kill_on_drop` 只杀主进程留下的孤儿。
        #[cfg(windows)]
        let job_guard = child
            .raw_handle()
            .and_then(|h| crate::transport::JobHandle::create_for(h as _));

        // 等 DevTools 端点就绪并取浏览器级 WS 端点:**主路径**用我们指定的已知端口轮询 `/json/version`
        // (不依赖 `DevToolsActivePort` 文件——CloakBrowser 等不写它);**兜底**再读该文件(万一端口写到了别处)。
        let ws_url = wait_for_ws_url(
            debug_port,
            &dir.join("DevToolsActivePort"),
            Duration::from_secs(30),
        )
        .await?;
        let ws = crate::transport::ws_connect(&ws_url).await?;
        Ok(Self {
            conn: Connection::from_ws(ws),
            child: std::sync::Mutex::new(Some(child)),
            // 仅临时 profile 记录目录(quit 时清理);持久 profile 不记录、不删除。
            user_data_dir: if persist { None } else { Some(dir) },
            stealth: opts.stealth,
            headless: opts.headless,
            // = want_meta:走了 mask 路径就补一致的 userAgentMetadata(见 attach)。
            full_ua_metadata: want_meta,
            ua_override,
            ua_full_version,
            platform_version,
            download_dir: opts.download_path.clone(),
            init_scripts: opts.init_scripts.clone(),
            container_js,
            #[cfg(windows)]
            job: std::sync::Mutex::new(job_guard),
        })
    }

    /// 接管一个已开启 CDP 调试端口的浏览器 / Electron 应用(对齐 camoufox `Browser::connect`)。
    /// `debug_http_url` 形如 `http://127.0.0.1:9222`(对方需以 `--remote-debugging-port=9222` 启动)。
    pub async fn connect(debug_http_url: &str) -> Result<Self> {
        Self::connect_with(debug_http_url, ChromiumOptions::new()).await
    }

    /// 同 [`connect`](Self::connect),可用 [`ChromiumOptions`] 指定反检测开关(对齐 camoufox
    /// `Browser::connect_with`)。接管不重启浏览器,故只有 `stealth` 等会话级项生效。
    pub async fn connect_with(debug_http_url: &str, opts: ChromiumOptions) -> Result<Self> {
        let ws_url = browser_ws_url(debug_http_url.trim_end_matches('/')).await?;
        let ws = crate::transport::ws_connect(&ws_url).await?;
        Ok(Self {
            conn: Connection::from_ws(ws),
            child: std::sync::Mutex::new(None),
            user_data_dir: None,
            // 接管的浏览器同样不调用 Runtime.enable;按 opts 决定是否注入兜底脚本。
            stealth: opts.stealth,
            // 接管方未知是否无头,保守不注入屏幕补丁。
            headless: false,
            // 接管不重启浏览器(无 `--user-agent` 注入路径),补环境不适用。
            full_ua_metadata: false,
            ua_override: None,
            ua_full_version: None,
            platform_version: None,
            download_dir: opts.download_path.clone(),
            init_scripts: opts.init_scripts.clone(),
            // 接管不重启浏览器(无法改启动 GPU 参数),补环境不适用。
            container_js: None,
            // 接管不接管对方生命周期,故不绑 Job。
            #[cfg(windows)]
            job: std::sync::Mutex::new(None),
        })
    }

    /// 新建一个标签页并附着,返回可驱动的 [`ChromiumTab`]。`url=None` 开 `about:blank`
    /// (对齐 camoufox `Browser::new_tab(Option<&str>)`)。
    pub async fn new_tab(&self, url: Option<&str>) -> Result<ChromiumTab> {
        let r = self
            .conn
            .send(
                "Target.createTarget",
                json!({ "url": url.unwrap_or("about:blank") }),
                None,
            )
            .await?;
        let target_id = r["targetId"]
            .as_str()
            .ok_or_else(|| Error::msg("CDP: 创建标签无 targetId"))?
            .to_string();
        self.attach(target_id).await
    }

    /// 按**上下文覆盖**开标签(对齐 camoufox `Browser::new_tab_with`):带 `proxy` 时新建独立
    /// `BrowserContext`(CDP 原生 **per-context 代理**),`close` 时连同上下文一起销毁;UA/locale/
    /// 时区经会话级 `Emulation` 覆盖。用于并发池每任务轮换出口/指纹。
    pub async fn new_tab_with(&self, ov: &ChromiumContextOverride) -> Result<ChromiumTab> {
        // **总是**新建独立 BrowserContext(每任务 cookie/缓存/storage 隔离,对齐 camoufox 每标签
        // 独立 context);带 `proxy` 时该上下文走指定出口(CDP 原生 per-context 代理)。
        let mut params = json!({});
        if let Some(proxy) = &ov.proxy {
            params["proxyServer"] = json!(proxy);
            if let Some(b) = &ov.proxy_bypass {
                params["proxyBypassList"] = json!(b);
            }
        }
        let r = self
            .conn
            .send("Target.createBrowserContext", params, None)
            .await?;
        let ctx_id = r["browserContextId"].as_str().map(String::from);
        let mut tparams = json!({ "url": "about:blank" });
        if let Some(c) = &ctx_id {
            tparams["browserContextId"] = json!(c);
        }
        let r = self.conn.send("Target.createTarget", tparams, None).await?;
        let target_id = r["targetId"]
            .as_str()
            .ok_or_else(|| Error::msg("CDP: 创建标签无 targetId"))?
            .to_string();
        let tab = self.attach_in_context(target_id, ctx_id).await?;
        ov.apply_emulation(&tab).await;
        Ok(tab)
    }

    /// 附着到最近一个 page target(接管现有标签用)。
    pub async fn latest_tab(&self) -> Result<ChromiumTab> {
        let r = self.conn.send("Target.getTargets", json!({}), None).await?;
        let targets = r["targetInfos"].as_array().cloned().unwrap_or_default();
        let page = targets
            .iter()
            .rev()
            .find(|t| t["type"].as_str() == Some("page"))
            .and_then(|t| t["targetId"].as_str())
            .ok_or_else(|| Error::msg("CDP: 没有可附着的 page 标签"))?
            .to_string();
        self.attach(page).await
    }

    /// 附着到第 `index` 个 page target(0 基,对齐 camoufox `Browser::get_tab`)。
    pub async fn get_tab(&self, index: usize) -> Result<ChromiumTab> {
        let r = self.conn.send("Target.getTargets", json!({}), None).await?;
        let targets = r["targetInfos"].as_array().cloned().unwrap_or_default();
        let page = targets
            .iter()
            .filter(|t| t["type"].as_str() == Some("page"))
            .nth(index)
            .and_then(|t| t["targetId"].as_str())
            .ok_or_else(|| Error::msg("CDP: 标签下标越界"))?
            .to_string();
        self.attach(page).await
    }

    /// 当前 page 标签数量(对齐 camoufox `Browser::tab_count`)。
    pub async fn tab_count(&self) -> usize {
        match self.conn.send("Target.getTargets", json!({}), None).await {
            Ok(r) => r["targetInfos"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter(|t| t["type"].as_str() == Some("page"))
                        .count()
                })
                .unwrap_or(0),
            Err(_) => 0,
        }
    }

    async fn attach(&self, target_id: String) -> Result<ChromiumTab> {
        self.attach_in_context(target_id, None).await
    }

    /// 附着到 target,并记录其所属 BrowserContext(`new_tab_with` 带代理时为 `Some`,`close` 时销毁)。
    async fn attach_in_context(
        &self,
        target_id: String,
        browser_context_id: Option<String>,
    ) -> Result<ChromiumTab> {
        let a = self
            .conn
            .send(
                "Target.attachToTarget",
                json!({ "targetId": target_id, "flatten": true }),
                None,
            )
            .await?;
        let session_id = a["sessionId"]
            .as_str()
            .ok_or_else(|| Error::msg("CDP: 附着无 sessionId"))?
            .to_string();
        let core = CdpCore::new(
            self.conn.clone(),
            session_id,
            target_id,
            self.download_dir.clone(),
            browser_context_id,
        );
        // 开启 Page 域(导航 `loadEventFired` 需要;**不是** CF 探测点)。
        let _ = core.send("Page.enable", json!({})).await;
        // 配了下载目录则启动期即设下载行为(让 `download_path` 开箱即用,不必显式 set_download_path)。
        if let Some(dir) = &self.download_dir {
            let _ = std::fs::create_dir_all(dir);
            let _ = core
                .send(
                    "Browser.setDownloadBehavior",
                    json!({ "behavior": "allow", "downloadPath": dir.display().to_string(), "eventsEnabled": true }),
                )
                .await;
        }
        // 反检测关键点:**绝不调用 `Runtime.enable`**。
        // CF/DataDome 用"console 序列化带 getter 对象"探测在线的 CDP `Runtime` 域;只要不开它,
        // 这条泄漏就不存在。`Runtime.evaluate`/`callFunctionOn`(省略 contextId)无需 enable 即可工作。
        if self.stealth {
            // 导航前注入兜底脚本(下次新文档生效),消除残留的 webdriver 痕迹。
            let _ = core
                .send(
                    "Page.addScriptToEvaluateOnNewDocument",
                    json!({ "source": stealth::STEALTH_JS }),
                )
                .await;
            // 无头额外补屏幕尺寸(无头默认 800x600 是显式破绽;WebGL 已靠 GPU 参数解决)。
            if self.headless {
                let _ = core
                    .send(
                        "Page.addScriptToEvaluateOnNewDocument",
                        json!({ "source": stealth::headless_screen_js() }),
                    )
                    .await;
            }
            // 容器 / 无 GPU Linux「补环境」:把软渲染 WebGL renderer 改写成常见 Linux 桌面 GPU,
            // 抹掉 SwiftShader/llvmpipe 破绽(在用户/指纹脚本之前注入,默认占先;用户显式 webgl 覆盖见
            // launch_inner 的 user_has_webgl 判定)。
            if let Some(js) = &self.container_js {
                let _ = core
                    .send(
                        "Page.addScriptToEvaluateOnNewDocument",
                        json!({ "source": js }),
                    )
                    .await;
            }
        }
        // 用户/指纹注入脚本(深指纹伪装等):在内置反检测脚本之后按序注入,与反检测兼容。
        for src in &self.init_scripts {
            let _ = core
                .send(
                    "Page.addScriptToEvaluateOnNewDocument",
                    json!({ "source": src }),
                )
                .await;
        }
        // 无头补环境(opt-in):补回被 `--user-agent` 清空的高熵 Client Hints。
        // 用 CDP `Emulation.setUserAgentOverride` 下发完整 `userAgentMetadata`(会话级持久,跨导航有效)。
        if self.full_ua_metadata {
            if let (Some(ua), Some(full)) = (&self.ua_override, &self.ua_full_version) {
                apply_ua_metadata(&core, ua, full, self.platform_version.as_deref()).await;
            }
        }
        Ok(ChromiumTab::new(core))
    }

    /// 优雅关闭:`Browser.close` + 杀子进程(自启动时)+ 清临时 profile。
    /// 取 `&self`(对齐 camoufox `Browser::quit`);可省略,`Drop` 时 `kill_on_drop` 兜底杀进程。
    pub async fn quit(&self) -> Result<()> {
        let _ = self.conn.send("Browser.close", json!({}), None).await;
        // 先从 Mutex 取出再 await(不跨 await 持锁)。
        let child = self.child.lock().ok().and_then(|mut g| g.take());
        if let Some(mut c) = child {
            let _ = c.kill().await;
        }
        // Windows:关闭 Job → 级联终止 Chrome 渲染/GPU 等子进程(防孤儿)。
        #[cfg(windows)]
        if let Ok(mut g) = self.job.lock() {
            let _ = g.take();
        }
        if let Some(d) = &self.user_data_dir {
            let _ = std::fs::remove_dir_all(d);
        }
        Ok(())
    }
}

/// 探测浏览器主版本号:运行 `<exe> --version`(打印版本即退出)解析。失败返回 `None`。
/// 用于无头时构造与真实版本一致的"去 Headless"UA(版本对不上 fingerprintjs/CF 也会拦)。
/// 探测浏览器**完整版本号**(如 `149.0.7827.115`)。
///
/// - **Windows**:读 PE **文件版本资源**(`VS_FIXEDFILEINFO`)。原因:GUI 版 Chrome/Edge 的
///   `exe --version` **不向 stdout 打印**(且会误启动一个浏览器实例触发单例锁报错),不可用;
/// - **其它平台**:`<exe> --version` 解析(mac/linux 正常输出)。
///
/// 失败返回 `None`(则无头不去 Headless 化、补环境不触发——退回安全默认,不破坏)。
async fn probe_chrome_version(exe: &Path) -> Option<String> {
    #[cfg(windows)]
    {
        win_file_version(exe)
    }
    #[cfg(not(windows))]
    {
        let out = tokio::process::Command::new(exe)
            .arg("--version")
            .output()
            .await
            .ok()?;
        let s = String::from_utf8_lossy(&out.stdout);
        stealth::parse_chrome_full(&s)
            .or_else(|| stealth::parse_chrome_major(&s).map(|m| format!("{m}.0.0.0")))
    }
}

/// Windows:读取可执行文件的 PE 版本资源,返回 `ProductVersion`(如 `149.0.7827.115`)。
/// 用 `VS_FIXEDFILEINFO` 的数值字段拼装(免去字符串/代码页查询)。失败返回 `None`。
#[cfg(windows)]
fn win_file_version(exe: &Path) -> Option<String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileVersionInfoSizeW, GetFileVersionInfoW, VS_FIXEDFILEINFO, VerQueryValueW,
    };
    let wide: Vec<u16> = exe
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        let mut handle: u32 = 0;
        let size = GetFileVersionInfoSizeW(wide.as_ptr(), &mut handle);
        if size == 0 {
            return None;
        }
        let mut buf = vec![0u8; size as usize];
        if GetFileVersionInfoW(wide.as_ptr(), 0, size, buf.as_mut_ptr().cast()) == 0 {
            return None;
        }
        // 查询根块 "\" 得到 VS_FIXEDFILEINFO。
        let sub: [u16; 2] = ['\\' as u16, 0];
        let mut value_ptr: *mut core::ffi::c_void = core::ptr::null_mut();
        let mut value_len: u32 = 0;
        if VerQueryValueW(
            buf.as_ptr().cast(),
            sub.as_ptr(),
            &mut value_ptr,
            &mut value_len,
        ) == 0
            || value_ptr.is_null()
            || (value_len as usize) < core::mem::size_of::<VS_FIXEDFILEINFO>()
        {
            return None;
        }
        let info = &*(value_ptr as *const VS_FIXEDFILEINFO);
        let ms = info.dwProductVersionMS;
        let ls = info.dwProductVersionLS;
        Some(format!(
            "{}.{}.{}.{}",
            (ms >> 16) & 0xffff,
            ms & 0xffff,
            (ls >> 16) & 0xffff,
            ls & 0xffff
        ))
    }
}

/// Windows:枚举显示适配器(注册表 Class GUID `{4d36e968-…}` 下各子键的 `DriverDesc`),
/// 判断是否存在**真实硬件 GPU**。仅有 "Microsoft Basic Display Adapter" / "Basic Render Driver" /
/// 远程显示等软件适配器 → 视为**无硬件 GPU**(无头应走 `--disable-gpu` 退 D3D11 WARP 软渲染,
/// WebGL 才可用;`--enable-gpu` 在无设备机器上会让 WebGL 创建失败)。读不到注册表时**保守返回
/// `true`**(按有 GPU 处理 → 不改变既有 `--enable-gpu` 行为)。
#[cfg(windows)]
fn windows_has_hardware_gpu() -> bool {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_LOCAL_MACHINE, KEY_READ, RegCloseKey, RegEnumKeyExW, RegOpenKeyExW,
        RegQueryValueExW,
    };
    let class_key: Vec<u16> =
        r"SYSTEM\CurrentControlSet\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
    let valname: Vec<u16> = "DriverDesc"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        let mut root: HKEY = std::ptr::null_mut();
        if RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            class_key.as_ptr(),
            0,
            KEY_READ,
            &mut root,
        ) != 0
        {
            return true; // 读不到 → 保守按有 GPU。
        }
        let mut found_hw = false;
        let mut idx: u32 = 0;
        loop {
            let mut name = [0u16; 256];
            let mut name_len: u32 = name.len() as u32;
            let st = RegEnumKeyExW(
                root,
                idx,
                name.as_mut_ptr(),
                &mut name_len,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut::<FILETIME>(),
            );
            if st != 0 {
                break; // ERROR_NO_MORE_ITEMS。
            }
            idx += 1;
            let sub: Vec<u16> = name[..name_len as usize]
                .iter()
                .copied()
                .chain(std::iter::once(0))
                .collect();
            let mut sk: HKEY = std::ptr::null_mut();
            if RegOpenKeyExW(root, sub.as_ptr(), 0, KEY_READ, &mut sk) != 0 {
                continue;
            }
            let mut buf = [0u16; 512];
            let mut len: u32 = (buf.len() * 2) as u32;
            let r = RegQueryValueExW(
                sk,
                valname.as_ptr(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                buf.as_mut_ptr() as *mut u8,
                &mut len,
            );
            RegCloseKey(sk);
            if r == 0 && len >= 2 {
                let count = (len as usize / 2).min(buf.len());
                let desc = String::from_utf16_lossy(&buf[..count])
                    .trim_end_matches('\0')
                    .to_lowercase();
                let is_software = desc.is_empty()
                    || desc.contains("basic display adapter")
                    || desc.contains("basic render")
                    || desc.contains("remote display")
                    || desc.contains("rdpudd")
                    || desc.contains("mirror");
                if !is_software {
                    found_hw = true;
                    break;
                }
            }
        }
        RegCloseKey(root);
        found_hw
    }
}

/// 探测平台版本(CH 高熵 `platformVersion`)。mac 用 `sw_vers -productVersion`(精确);
/// Windows 最佳努力(`cmd /c ver` 取 build:Win11≥22000→`15.0.0`,否则 `10.0.0`);其它平台 `None`。
/// 无头补环境是 opt-in,主要在 mac 验证;Win 后续可按需细化映射。
async fn detect_platform_version() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        let out = tokio::process::Command::new("sw_vers")
            .arg("-productVersion")
            .output()
            .await
            .ok()?;
        let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
        (!v.is_empty()).then_some(v)
    }
    #[cfg(target_os = "windows")]
    {
        let out = tokio::process::Command::new("cmd")
            .args(["/c", "ver"])
            .output()
            .await
            .ok()?;
        let s = String::from_utf8_lossy(&out.stdout);
        // 形如 "Microsoft Windows [Version 10.0.22631.4317]" → 取 build 段(第三段)。
        let build = s
            .split(|c: char| !c.is_ascii_digit() && c != '.')
            .find(|t| t.starts_with("10.0."))
            .and_then(|t| t.split('.').nth(2))
            .and_then(|b| b.parse::<u32>().ok());
        Some(match build {
            Some(b) if b >= 22000 => "15.0.0".to_string(),
            _ => "10.0.0".to_string(),
        })
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        None
    }
}

/// 无头补环境核心:据**纯 Rust 复现的 Chromium GREASE 品牌算法**(版本正确、无需读页面、
/// 不依赖 about:blank 的 opaque origin)构造完整 `userAgentMetadata`,经 CDP
/// `Emulation.setUserAgentOverride` 下发(会话级、跨导航持久)→ 补回被 `--user-agent` 清空的
/// 高熵 Client Hints(`fullVersionList`/`architecture`/`bitness`/`platformVersion`/`uaFullVersion`)。
/// 失败静默放弃,绝不破坏既有行为。
async fn apply_ua_metadata(
    core: &CdpCore,
    ua: &str,
    full_version: &str,
    platform_version: Option<&str>,
) {
    let Some(major) = stealth::parse_chrome_major(full_version) else {
        return;
    };
    let to_json = |list: Vec<(String, String)>| -> Vec<Value> {
        list.into_iter()
            .map(|(b, v)| json!({ "brand": b, "version": v }))
            .collect()
    };
    let brands = to_json(stealth::ua_brand_list(major, full_version, false));
    let full_list = to_json(stealth::ua_brand_list(major, full_version, true));
    let meta = json!({
        "brands": brands,
        "fullVersion": full_version,
        "fullVersionList": full_list,
        "platform": stealth::ch_platform(),
        "platformVersion": platform_version.unwrap_or(""),
        "architecture": stealth::ch_architecture(),
        "model": "",
        "mobile": false,
        "bitness": stealth::ch_bitness(),
        "wow64": false,
    });
    let _ = core
        .send(
            "Emulation.setUserAgentOverride",
            json!({ "userAgent": ua, "userAgentMetadata": meta }),
        )
        .await;
}

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// 选一个空闲 TCP 端口(绑定到 `127.0.0.1:0` 拿到内核分配的端口后立即释放)。
/// 用于 `--remote-debugging-port`,从而**不依赖 `DevToolsActivePort` 文件**发现端口
/// (CloakBrowser 等部分 Chromium 不写该文件)。释放到 Chrome 绑定之间有极小竞态,单机自动化可接受。
fn pick_free_port() -> Result<u16> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")
        .map_err(|e| Error::msg(format!("CDP: 选空闲调试端口失败: {e}")))?;
    let port = listener
        .local_addr()
        .map_err(|e| Error::msg(format!("CDP: 读空闲端口失败: {e}")))?
        .port();
    Ok(port)
}

/// 轮询取浏览器级 WebSocket 端点:**主路径**用已知 `port` 打 `/json/version`;
/// **兜底**读 `DevToolsActivePort` 文件(若浏览器把端口写到了别处)。每次尝试都带超时,绝不无限等待。
async fn wait_for_ws_url(port: u16, devtools_file: &Path, timeout: Duration) -> Result<String> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match browser_ws_url(&format!("http://127.0.0.1:{port}")).await {
            Ok(url) => return Ok(url),
            Err(e) => {
                // 兜底:浏览器若写了 DevToolsActivePort 且端口与我们指定的不同,用它再试一次。
                if let Ok(s) = std::fs::read_to_string(devtools_file) {
                    if let Some(p) = s.lines().next().and_then(|l| l.trim().parse::<u16>().ok()) {
                        if p != port {
                            if let Ok(url) = browser_ws_url(&format!("http://127.0.0.1:{p}")).await
                            {
                                return Ok(url);
                            }
                        }
                    }
                }
                if std::time::Instant::now() >= deadline {
                    return Err(Error::msg(format!(
                        "CDP: 等待调试端点就绪超时(端口 {port});最后错误: {e}"
                    )));
                }
            }
        }
        sleep(Duration::from_millis(150)).await;
    }
}

/// 查询 `{http}/json/version` 拿浏览器级 WebSocket 调试端点。**带超时**(5s),
/// 避免端口已打开但 HTTP 不应答时 `reqwest` 无限挂起。
async fn browser_ws_url(http: &str) -> Result<String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|e| Error::msg(format!("CDP: 构建 HTTP 客户端失败: {e}")))?;
    let body: Value = client
        .get(format!("{http}/json/version"))
        .send()
        .await
        .map_err(|e| Error::msg(format!("CDP: 访问 {http}/json/version 失败: {e}")))?
        .json()
        .await
        .map_err(|e| Error::msg(format!("CDP: 解析 /json/version 失败: {e}")))?;
    body["webSocketDebuggerUrl"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| Error::msg("CDP: /json/version 无 webSocketDebuggerUrl"))
}
