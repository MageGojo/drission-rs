# 示例索引 · Examples

> `drission` 自带 48+ 个端到端示例。**带 🔌 的完全离线**(进程内起 HTTP 服务 / `data:` / `file://` 页,
> 不依赖外网,可直接当集成测试跑);**带 🌐 的需要联网**(访问真实站点)。
>
> **默认后端 = Chromium / CDP(Google Chrome)**:`cdp_demo` / `cdp_advanced` 等 cdp 示例默认即可跑。其余示例多数用
> **Camoufox** 后端,需 **`--no-default-features --features camoufox`**(首次运行自动下载 Camoufox 到 `~/.cache/camoufox`,`fetch_browser` 可预热)。

**通用运行方式**

```bash
# cdp 示例:默认即含 cdp,无需 feature
cargo run --example cdp_demo
# Camoufox 系示例(下表绝大多数):务必关掉默认 cdp,只开 camoufox
cargo run --example <名字> --no-default-features --features camoufox
# 滑块(slider 自带 camoufox)/ 再叠加 ocr
cargo run --example <名字> --no-default-features --features slider
cargo run --example <名字> --no-default-features --features camoufox,ocr
# Session TLS 指纹 / 纯算签名(后端无关,可直接叠加)
cargo run --example session_tls --features impersonate
cargo build  --example env_signer --features signer
```

> ⚠️ **为什么 Camoufox 系示例要 `--no-default-features`**:默认 `cdp` 与 `camoufox` 同时开启时,统一接口(`Browser`/`Tab`/`Page`…)
> 按「**cdp 优先**」解析,会与 Camoufox 示例期望的类型冲突(轻则跑成 cdp 后端、重则编译报错)。关掉默认 cdp、只留 camoufox,
> 示例才会用 Camoufox 后端并正确编译。cdp 示例反之(默认即可)。

`需要` 列:`· camoufox/ocr/slider/signer/cdp` 表示必须加对应 `--features`(camoufox 系记得配 `--no-default-features`;`slider` 会自动带入 `camoufox`);`🔌` 离线 / `🌐` 联网。

## 大道至简:`Page` 一行起步(对标 DrissionPage `ChromiumPage`,需 `--no-default-features --features camoufox`)

日常脚本推荐用 `Page` 门面——开浏览器 + 驱动当前标签合一,像写 Python:

```rust
use drission::prelude::*;
let page = Page::headless().await?;            // 或 Page::new()(有头)/ Page::with(opts)
page.get("https://example.com").await?;
println!("{}", page.title().await?);
page.click("@id:more").await?;                 // 「找+点」一步;另有 page.input(sel, text)/page.exists(sel)
page.quit().await?;
```

`page` 通过 `Deref` 拥有**全部 `Tab` 方法**(`ele`/`run_js`/`listen`/`actions`/`dump_env`…)。需要多标签 /
接管 / 并发池等更底层控制时,仍可用 `Browser` + `Tab`(`Page` 是附加门面,不替代它们)。

---

## 入门 / 核心闭环

| 示例 | 说明 | 需要 |
|---|---|---|
| [quickstart](quickstart.rs) | 最小闭环(**`Page` 一行起步**):访问 → 读标题/URL → 查元素读文本 → 退出 | 🌐 |
| [page_basics](page_basics.rs) | 页面基础能力(对标 DrissionPage)端到端自验证 | 🔌 |
| [page_extras](page_extras.rs) | 进阶页面:iframe 内元素 / JS 对话框 / 文件上传 / 静态 XPath | 🔌 |

## 元素定位与交互

| 示例 | 说明 | 需要 |
|---|---|---|
| [form_input](form_input.rs) | 注入输入框 + 按钮,用 `page.input/click/exists` 捷径输入并点击,读回结果 | 🔌 |
| [form_fill](form_fill.rs) | 自动填完 `examples/1.html` 的 4 步问卷并校验 | 🔌 |
| [file_upload](file_upload.rs) | 文件上传三种写法端到端自验证 | 🔌 |
| [ele_extras](ele_extras.rs) | 元素几何/状态/属性 + 元素级 wait + 键盘(组合键/序列) | 🔌 |
| [relative_shadow](relative_shadow.rs) | 元素相对定位 + Shadow DOM | 🔌 |
| [actions_drag](actions_drag.rs) | 动作链 `tab.actions()`:拖放(移到源 → 按住 → 拖到目标 → 释放) | 🔌 |

## 网络监听 / 拦截

| 示例 | 说明 | 需要 |
|---|---|---|
| [listen_handle](listen_handle.rs) | DP 风格网络监听句柄 `tab.listen()` | 🔌 |
| [concurrent_listen](concurrent_listen.rs) | 多标签并发监听三大核心能力演示 | 🌐 |
| [intercept](intercept.rs) | 请求拦截:伪造响应 / 中止 / 改写放行 | 🌐 |
| [intercept_window](intercept_window.rs) | `tab.intercept()` 句柄 + 窗口尺寸句柄 `tab.set().window()` | 🔌 |
| [console_listen](console_listen.rs) | 控制台监听(对标 DP `tab.console`) | 🔌 |
| [ws_listen](ws_listen.rs) | WebSocket 帧监听 `tab.websocket()` | 🔌 |

## 长监听(连续抓翻页签名)

| 示例 | 说明 | 需要 |
|---|---|---|
| [douyin_listen](douyin_listen.rs) | 抓抖音 `aweme/detail` 响应(一次) | 🌐 |
| [douyin_listen_long](douyin_listen_long.rs) | 连续抓「下一个视频」各自的签名 detail(后台抽取不丢包 + `press_key` 翻页) | 🌐 |
| [bilibili_listen_long](bilibili_listen_long.rs) | bilibili 多 P 视频页连续抓每分集 playurl 签名(wbi `w_rid`/`wts`) | 🌐 |

## 验证码 OCR(`--features ocr`)

| 示例 | 说明 | 需要 |
|---|---|---|
| [ocr_captcha](ocr_captcha.rs) | 验证码 OCR 端到端 demo(`Tab::ocr_image`) | 🌐 · ocr |
| [apizero_login](apizero_login.rs) | 端到端:填账号密码 + OCR 识别验证码并填入 → 登录 | 🌐 · ocr |
| [ocr_probe](ocr_probe.rs) | 验证码 `<img>` 截图取样(取样工具,不需 ocr feature) | 🌐 |

## 图片滑块缺口(`--features slider`)

| 示例 | 说明 | 需要 |
|---|---|---|
| [slider_local](slider_local.rs) | 通用滑块库能力**离线自验证**(本地合成 `<img>` 版滑块) | 🔌 · slider |
| [geetest_slide](geetest_slide.rs) | 极验 v4 滑块——通用滑块库求解(`SliderConfig::geetest_v4()`) | 🌐 · slider |
| [dx_slide](dx_slide.rs) | 顶象滑块缺口识别(跨域 taint `<img>`,截图 + 浮雕镂空对比度法) | 🌐 · slider |
| [geetest_probe](geetest_probe.rs) | 极验诊断探针:看厂商从拖拽里读到什么(取证,不过码) | 🌐 |
| [geetest_diag](geetest_diag.rs) | 极验缺口检测诊断:三图落盘 PNG + 逐列 diff 剖面 | 🌐 |

## 反检测 / 过盾 / 代理

| 示例 | 说明 | 需要 |
|---|---|---|
| [anti_detect](anti_detect.rs) | 抗检测基本面:`navigator.webdriver` 应为 false(bot.sannysoft.com) | 🌐 |
| [stealth_check](stealth_check.rs) | 跑四大检测站,每站 PASS/FAIL + 汇总 | 🌐 |
| [cf_check](cf_check.rs) | 过 Cloudflare 盾:访问受保护页,观察自动通过 challenge | 🌐 |
| [exa_cf](exa_cf.rs) | auth.exa.ai 交互式过盾:填邮箱 → 触发 Turnstile → 等 token | 🌐 |
| [proxy_health](proxy_health.rs) | 代理池健康检查 + 出口地理探测 + IP↔指纹一致性 | 🌐 |

## 高并发池

| 示例 | 说明 | 需要 |
|---|---|---|
| [pool_crawl](pool_crawl.rs) | `BrowserPool` 并发 + cookie 隔离 + 指纹轮换 + 重试 + 断点续抓 | 🔌 |

## 双模 / 会话 / 采集 / 持久化

| 示例 | 说明 | 需要 |
|---|---|---|
| [session_mode](session_mode.rs) | Session(HTTP)双模 + cookie 互通(对标 DP Driver+Session) | 🔌 |
| [web_page_scrape](web_page_scrape.rs) | WebPage 双模 + cookie 同步 + 表格提取 + 翻页 + CSV/JSON 导出 | 🔌 |
| [extras_demo](extras_demo.rs) | 逐字符拟人输入 + 登录态全量持久化(storageState) | 🔌 |

## 截图 / 录像 / 下载

| 示例 | 说明 | 需要 |
|---|---|---|
| [screencast](screencast.rs) | 截图与录像(对标 DP `browser_control/screen`) | 🔌 |
| [download_manager](download_manager.rs) | 下载管理 `tab.downloads()` 端到端自验证 | 🔌 |

## 吐环境(补环境)/ 纯算签名

| 示例 | 说明 | 需要 |
|---|---|---|
| [dump_env_fingerprint](dump_env_fingerprint.rs) | canvas/webgl/audio 指纹补环境 + 一键导出工程 | 🔌 |
| [douyin_dump_env](douyin_dump_env.rs) | 通用吐环境能力(以抖音 a_bogus 为目标参数) | 🌐 |
| [douyin_capture](douyin_capture.rs) | 抓指定视频 detail(含 a_bogus)并导出可复现包 | 🌐 |
| [env_signer](env_signer.rs) | 自包含「补环境 + 纯算签名」运行器(内嵌 QuickJS,无 Node/浏览器) | 🔌 · signer |

## 接管浏览器(WS)/ 运维

| 示例 | 说明 | 需要 |
|---|---|---|
| [ws_connect](ws_connect.rs) | WS 接管浏览器(`BrowserServer` + `Browser::connect`) | 🔌 |
| [fetch_browser](fetch_browser.rs) | 预下载 / 校验本机 Camoufox 可执行文件 | 🌐 |

## CDP / Chromium 后端(**默认后端**,Google Chrome,无需 feature)

| 示例 | 说明 | 需要 |
|---|---|---|
| [cdp_demo](cdp_demo.rs) | CDP 后端 demo:启动/接管 Google Chrome → 导航 → run_js → 元素文本 → 截图(默认浏览器,自动探测路径) | 🌐 |
| [cdp_advanced](cdp_advanced.rs) | CDP 深化能力端到端自验证(进程内 HTTP 服务,Chrome 访问 localhost) | 🔌 |
| [cdp_fetch](cdp_fetch.rs) | **自动下载 Chrome for Testing**(对标 CloakBrowser 首次运行自动下载)并用它驱动 example.com;`DRISSION_PREFETCH_WIN=1` 顺带预取 win64 | 🌐 |

## Windows 专项(在 Windows 上运行)

| 示例 | 说明 | 需要 |
|---|---|---|
| [win_smoke](win_smoke.rs) | Windows 冒烟:启动 Camoufox → fd3/4 命名管道 Juggler 握手 | 🌐 |
| [win_diag](win_diag.rs) | Windows 传输诊断:无头 / 有头两种模式完整链路 | 🌐 |
| [win_cf_test](win_cf_test.rs) | Windows 过 CF 盾:有头启动 → 访问受保护页 | 🌐 |
| [win_bilibili_test](win_bilibili_test.rs) | Windows 硬核:多 P 长监听 + 后台抽取不丢包 + 点分集 | 🌐 |

---

更多用法对照见 [DrissionPage → drission API 映射](../docs/API映射.md);设计原理见 [docs/](../docs/README.md)。
