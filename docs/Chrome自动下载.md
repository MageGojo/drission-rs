# Chrome 自动下载分发(CDP 后端)

> 对标 CloakBrowser / Camoufox 的「首次运行自动下载浏览器二进制」体验,但走 Google 官方
> **Chrome for Testing**(CfT)分发,**三平台齐全(mac / win / linux)**。
> 模块:`src/cdp/fetch.rs`;与 `src/cdp/locate.rs`(定位系统已装 Chrome)互补。

## 为什么

`launch()` 旧行为是「只定位系统已装的 Chrome,找不到就报错」(`locate::chrome_path`)。
但在干净机器 / CI / 容器里常常没装 Chrome —— CloakBrowser 的招牌体验是 `pip install` 后
**首次启动自动下载** stealth Chromium。本模块给 drission 的 **CDP 后端**补上同样能力:
**找不到系统浏览器时,自动从 Chrome for Testing 下载并缓存**,做到默认开箱即用。

> 注:CloakBrowser 的预编译 stealth 二进制只发 Linux x64(其 README 称还有 mac arm64),
> **没有 Windows 版**;而用户要 mac + win,且明确「谷歌浏览器」,故选官方 Chrome for Testing
> ——它对 `mac-arm64 / mac-x64 / win64 / win32 / linux64` 都有官方 zip。

## 分发源

- 索引:`https://googlechromelabs.github.io/chrome-for-testing/last-known-good-versions-with-downloads.json`
- 结构:`channels.{Stable,Beta,Dev,Canary}.downloads.chrome[] = { platform, url }`
- 资产:`chrome-<platform>.zip`,解压顶层目录 `chrome-<platform>/`。

## 平台标记(`cft_platform()`)

| OS / ARCH | CfT platform |
|---|---|
| macos / aarch64 | `mac-arm64` |
| macos / x86_64 | `mac-x64` |
| windows / x86_64 | `win64` |
| windows / x86 | `win32` |
| linux / x86_64 | `linux64` |

## 解压后可执行文件名(`chrome_exe_name()`)

| platform 前缀 | 可执行文件 | 相对路径示例 |
|---|---|---|
| `mac*` | `Google Chrome for Testing` | `chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing` |
| `win*` | `chrome.exe` | `chrome-win64/chrome.exe` |
| `linux*` | `chrome` | `chrome-linux64/chrome` |

> mac 的 `.app` 包内含**符号链接**(Frameworks 版本目录),解压必须保留符号链接与可执行位,
> 否则 Chrome 无法启动 —— 复用与 Camoufox 同款 `extract_zip`(unix 保留 mode + symlink)。

## 缓存布局

```
~/.cache/drission/            # 可用 DRISSION_CACHE 覆盖
└── chrome/
    ├── mac-arm64/chrome-mac-arm64/Google Chrome for Testing.app/...
    └── win64/chrome-win64/chrome.exe
```

按 **平台分目录**,命中即复用、不重复下载;下载的 zip 解压后即删。

## 公开 API

```rust
use drission::cdp::{ensure_chrome, download_chrome_for, cft_platform};

// 1) 一把梭:locate 系统 Chrome → 命中缓存 → 否则下载当前平台 Stable,返回可执行文件路径。
let exe = ensure_chrome().await?;

// 2) 指定平台预取(跨平台分发用,如 mac 上预取 win64)——「mac 和 win 都要」。
let win = download_chrome_for("win64", "Stable").await?;

// 3) ChromiumBrowser 便捷:
let exe = ChromiumBrowser::download_chrome().await?;   // = ensure_chrome
let browser = ChromiumBrowser::launch(true).await?;    // 找不到系统 Chrome 会自动下载
```

### 解析优先级(`ensure_chrome`)

1. 环境变量 `CHROME_BIN` / `DRISSION_CHROME`(经 `locate`);
2. 系统已安装的 Chrome / Edge / Brave / Chromium(经 `locate`,Windows 含注册表);
3. 缓存 `~/.cache/drission/chrome/<platform>` 中已下载的 CfT;
4. 从 Chrome for Testing 下载当前平台最新 Stable。

> `launch(headless)` 现改为:先 `ensure_chrome()`(定位或下载)再启动 —— 默认开箱即用;
> 只想定位、不想触发下载用 `find_chrome()`(仅 `locate`,不下载)。
