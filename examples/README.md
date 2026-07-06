# 示例索引 · Examples

> `drission` 自带 60+ 个端到端示例,**按能力分文件夹**存放(见下「目录结构」)。**带 🔌 的完全离线**
> (进程内起 HTTP 服务 / `data:` / `file://` 页,不依赖外网,可直接当集成测试跑);**带 🌐 的需要联网**(访问真实站点)。
>
> **默认后端 = Chromium / CDP(Google Chrome)**:`cdp/` 与 `clickword/`、`cloudflare/exa_cf` 等 cdp 示例默认即可跑。
> 其余示例多数用 **Camoufox** 后端,需 **`--no-default-features --features camoufox`**
> (首次运行自动下载 Camoufox 到 `~/.cache/camoufox`,`fetch_browser` 可预热)。

**通用运行方式**(`cargo run --example <名字>` 用的始终是「示例名」,与所在文件夹无关)

```bash
# cdp 示例:默认即含 cdp,无需 feature
cargo run --example cdp_demo
# Camoufox 系示例(下表绝大多数):务必关掉默认 cdp,只开 camoufox
cargo run --example <名字> --no-default-features --features camoufox
# 滑块(slider 自带 camoufox)/ 再叠加 ocr
cargo run --example <名字> --no-default-features --features slider
cargo run --example <名字> --no-default-features --features camoufox,ocr
# cdp + ocr(点选 / 检测)
cargo run --example yidun_click_stable --features cdp,ocr
# Session TLS 指纹 / 纯算签名(后端无关,可直接叠加)
cargo run --example session_tls --features impersonate
cargo build  --example env_signer --features signer
```

> ⚠️ **为什么 Camoufox 系示例要 `--no-default-features`**:默认 `cdp` 与 `camoufox` 同时开启时,统一接口(`Browser`/`Tab`/`Page`…)
> 按「**cdp 优先**」解析,会与 Camoufox 示例期望的类型冲突(轻则跑成 cdp 后端、重则编译报错)。关掉默认 cdp、只留 camoufox,
> 示例才会用 Camoufox 后端并正确编译。cdp 示例反之(默认即可)。

`需要` 列:`· camoufox/ocr/slider/signer/cdp/impersonate` 表示必须加对应 `--features`(camoufox 系记得配 `--no-default-features`;`slider` 会自动带入 `camoufox`);`🔌` 离线 / `🌐` 联网。

## 目录结构(按能力分文件夹)

| 文件夹 | 内容 |
|---|---|
| `cdp/` | CDP/Chromium 后端核心:demo / 高级能力 / 自动下载 Chrome / 下载管理 / 键盘 / 并发池 / 吐环境 |
| `camoufox/` | Camoufox 后端页面·元素·交互:页面基础/进阶、表单、上传、相对定位/Shadow、动作链、截图、下载、预热 |
| `antidetect/` | 反检测 / 指纹:webdriver/stealth、CDP 指纹池、第三方指纹验证、无头 GPU/CH 诊断 |
| `cloudflare/` | 过 Cloudflare 盾:Camoufox(`cf_check`)与 CDP(`exa_cf`)两条路径 |
| `ocr/` | 验证码·字符 OCR:`ocr_image`、登录端到端、目标检测探针、模型热替换、取样 |
| `slider/` | 验证码·滑块/缺口:通用滑块库、极验、顶象、诊断/取证 |
| `clickword/` | 验证码·文字点选(易盾):稳定版点选 + 字形样本库自增长、采集、侦察 |
| `network/` | 网络监听 / 拦截 / console / WebSocket / 接管浏览器 |
| `session/` | HTTP 会话双模 / TLS·JA3 指纹 / WebPage 采集 |
| `env/` | 吐环境(补环境)/ 纯算签名 |
| `pool/` | 高并发池 / 代理健康 |
| `sites/` | 站点实战采集(抖音 / B 站 / 政府公开数据,长监听翻页签名与 CSV 导出) |
| `windows/` | Windows 专项(在 Windows 上运行的验证包) |

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

## 🎓 教学「活教材」(learn/,默认 cdp,边用库边学 Rust)

> 面向「想学 Rust + 学会用本库」的人:每个示例都在**真实使用库**的同时,把关键 Rust 概念讲到位(注释即教程)。

| 示例 | 说明 | 需要 |
|---|---|---|
| [learn_basics](learn/learn_basics.rs) | **第 ① 课(完全离线)**:`set_content` 灌页,分 9 课讲 async/`?` · 所有权 · Arc 共享句柄 · Vec/迭代 · Option · 借用参数 · 闭包 move+并发 · 生命周期 `'a` · Send/`!Send` | 🔌 |
| [bilibili_covers](learn/bilibili_covers.rs) | **第 ② 课(实战·联网)**:爬 B 站热门视频封面——等渲染/滚动懒加载 → `eles`+`attr`+`text` 抓卡片 → URL 清洗去重 → **`tokio` 并发下载(spawn+分批限流)** → 落盘 + `scrape::write_json` 导清单。学 struct/迭代器/并发/文件 IO | 🌐 |

```bash
cargo run --example learn_basics                 # 离线,随便跑
cargo run --example bilibili_covers              # 联网爬 B 站热门封面(默认抓 10 张)
cargo run --example bilibili_covers -- "https://www.bilibili.com/v/popular/all" 8   # 指定页 + 数量
HL=0 cargo run --example bilibili_covers         # 有头,看着它跑
```

---

## 入门 / 核心闭环(camoufox/)

| 示例 | 说明 | 需要 |
|---|---|---|
| [quickstart](camoufox/quickstart.rs) | 最小闭环(**`Page` 一行起步**):访问 → 读标题/URL → 查元素读文本 → 退出 | 🌐 |
| [page_basics](camoufox/page_basics.rs) | 页面基础能力(对标 DrissionPage)端到端自验证 | 🔌 |
| [page_extras](camoufox/page_extras.rs) | 进阶页面:iframe 内元素 / JS 对话框 / 文件上传 / 静态 XPath | 🔌 |

## 元素定位与交互(camoufox/)

| 示例 | 说明 | 需要 |
|---|---|---|
| [form_input](camoufox/form_input.rs) | 注入输入框 + 按钮,用 `page.input/click/exists` 捷径输入并点击,读回结果 | 🔌 |
| [form_fill](camoufox/form_fill.rs) | 自动填完 `examples/1.html` 的 4 步问卷并校验 | 🔌 |
| [file_upload](camoufox/file_upload.rs) | 文件上传三种写法端到端自验证 | 🔌 |
| [ele_extras](camoufox/ele_extras.rs) | 元素几何/状态/属性 + 元素级 wait + 键盘(组合键/序列) | 🔌 |
| [relative_shadow](camoufox/relative_shadow.rs) | 元素相对定位 + Shadow DOM | 🔌 |
| [new_tab](camoufox/new_tab.rs) | `tab.wait().new_tab()`:可信点击 `target=_blank` → 捕获弹窗为新 `Tab` | 🔌 |
| [actions_drag](camoufox/actions_drag.rs) | 动作链 `tab.actions()`:拖放(移到源 → 按住 → 拖到目标 → 释放) | 🔌 |

## 网络监听 / 拦截 / 接管(network/)

| 示例 | 说明 | 需要 |
|---|---|---|
| [listen_handle](network/listen_handle.rs) | DP 风格网络监听句柄 `tab.listen()` | 🔌 |
| [concurrent_listen](network/concurrent_listen.rs) | 多标签并发监听三大核心能力演示 | 🌐 |
| [intercept](network/intercept.rs) | 请求拦截:伪造响应 / 中止 / 改写放行 | 🌐 |
| [intercept_window](network/intercept_window.rs) | `tab.intercept()` 句柄 + 窗口尺寸句柄 `tab.set().window()` | 🔌 |
| [console_listen](network/console_listen.rs) | 控制台监听(对标 DP `tab.console`) | 🔌 |
| [ws_listen](network/ws_listen.rs) | WebSocket 帧监听 `tab.websocket()` | 🔌 |
| [ws_connect](network/ws_connect.rs) | WS 接管浏览器(`BrowserServer` + `Browser::connect`) | 🔌 |

## 站点实战:长监听(连续抓翻页签名)(sites/)

| 示例 | 说明 | 需要 |
|---|---|---|
| [douyin_listen](sites/douyin_listen.rs) | 抓抖音 `aweme/detail` 响应(一次) | 🌐 |
| [douyin_listen_long](sites/douyin_listen_long.rs) | 连续抓「下一个视频」各自的签名 detail(后台抽取不丢包 + `press_key` 翻页) | 🌐 |
| [bilibili_listen_long](sites/bilibili_listen_long.rs) | bilibili 多 P 视频页连续抓每分集 playurl 签名(wbi `w_rid`/`wts`) | 🌐 |
| [hubei_zfwj_csv](sites/hubei_zfwj_csv.rs) | 湖北省人民政府「政府文件」公开列表:动态翻页,逐条进详情页抽正文/发文字号/来源并导出 CSV | 🌐 · cdp |
| [hubei_zfwj_protocol](sites/hubei_zfwj_protocol.rs) | 湖北省政府文件纯协议版:SessionPage 抓挑战页,本地 JS 引擎解 challenge 后抓列表/详情并导出 CSV | 🌐 · camoufox,signer |

## 验证码·字符 OCR(ocr/,`--features ocr`)

| 示例 | 说明 | 需要 |
|---|---|---|
| [ocr_captcha](ocr/ocr_captcha.rs) | 验证码 OCR 端到端 demo(`Tab::ocr_image`) | 🌐 · camoufox,ocr |
| [apizero_login](ocr/apizero_login.rs) | 端到端:填账号密码 + OCR 识别验证码并填入 → 登录 | 🌐 · camoufox,ocr |
| [det_probe](ocr/det_probe.rs) | 目标检测探针:`Det` 跑通 `common_det.onnx`(点选找字框) | 🌐 · cdp,ocr |
| [ocr_hotswap](ocr/ocr_hotswap.rs) | OCR/检测模型热替换(自训模型上线,自定义字符集 + 环境变量零改码) | ocr |
| [ocr_probe](ocr/ocr_probe.rs) | 验证码 `<img>` 截图取样(取样工具,不需 ocr feature) | 🌐 |

## 验证码·文字点选 / 易盾(clickword/,`--features cdp,ocr`)

| 示例 | 说明 | 需要 |
|---|---|---|
| [yidun_click_stable](clickword/yidun_click_stable.rs) | 易盾点选**稳定版**:确认出图 + 拟人轨迹 + Outcome 诚实分类 + **字形样本库「过盾即验真」自增长**(`YIDUN_HARVEST=1` 连续采样) | 🌐 · cdp,ocr |
| [yidun_click](clickword/yidun_click.rs) | 易盾点选:监听取图取序 → det → 逐框 OCR → 全局指派 → 拟人可信点击 → check 铁证 | 🌐 · cdp,ocr |
| [yidun_collect](clickword/yidun_collect.rs) | 点选样本采集:反复弹挑战 → 切字框落盘 + OCR 猜测标签(为自训攒数据) | 🌐 · cdp,ocr |
| [yidun_probe](clickword/yidun_probe.rs) | 请求侦察:看验证码图片格式/来源 + 提示文字下发方式 | 🌐 · cdp |
| [yidun_trace](clickword/yidun_trace.rs) | 提交侦察:dump check 的 query + up 的 postData,定位轨迹字段 | 🌐 · cdp,ocr |
| [yidun_detect](clickword/yidun_detect.rs) | 风控侦察:普查监听事件 + 测点击事件密度 + 抓提交/check | 🌐 · cdp |

> 自训管线见 [`docs/OCR模型热替换.md`](../docs/OCR模型热替换.md);样本库/数据集脚本在仓库根 `yidun-train/tools/`。

## 验证码·图片滑块缺口(slider/,`--features slider`)

| 示例 | 说明 | 需要 |
|---|---|---|
| [slider_local](slider/slider_local.rs) | 通用滑块库能力**离线自验证**(本地合成 `<img>` 版滑块) | 🔌 · slider |
| [geetest_slide](slider/geetest_slide.rs) | 极验 v4 滑块——通用滑块库求解(`SliderConfig::geetest_v4()`) | 🌐 · slider |
| [dx_slide](slider/dx_slide.rs) | 顶象滑块缺口识别(跨域 taint `<img>`,截图 + 内容 NCC 法) | 🌐 · slider |
| [geetest_probe](slider/geetest_probe.rs) | 极验诊断探针:看厂商从拖拽里读到什么(取证,不过码) | 🌐 |
| [geetest_diag](slider/geetest_diag.rs) | 极验缺口检测诊断:三图落盘 PNG + 逐列 diff 剖面 | 🌐 |

## 反检测 / 指纹(antidetect/)

| 示例 | 说明 | 需要 |
|---|---|---|
| [anti_detect](antidetect/anti_detect.rs) | 抗检测基本面:`navigator.webdriver` 应为 false(bot.sannysoft.com) | 🌐 · camoufox |
| [stealth_check](antidetect/stealth_check.rs) | 跑四大检测站,每站 PASS/FAIL + 汇总 | 🌐 · camoufox |
| [cdp_fingerprint](antidetect/cdp_fingerprint.rs) | 每浏览器不同指纹:`CdpFingerprintPool` 起 N 个浏览器各套一份并 dump 验证各异 | 🌐 · cdp |
| [fp_verify](antidetect/fp_verify.rs) | 指纹「确实换了」第三方验证(FingerprintJS visitorId + canvas/webgl 哈希) | 🌐 · cdp |
| [cdp_fp](antidetect/cdp_fp.rs) | 指纹探针:dump 全面指纹 JSON,有头/无头 diff 出无头破绽 | 🌐 · cdp |
| [gpu_probe](antidetect/gpu_probe.rs) | 无头 GPU 探针:读 WebGL UNMASKED_RENDERER 判断是否真实 GPU | 🌐 · cdp |
| [hl_ch](antidetect/hl_ch.rs) | 无头 Client Hints 诊断:mask_ua 对高熵 CH/UA 的影响 | 🌐 · cdp |

## 过 Cloudflare 盾(cloudflare/)

| 示例 | 说明 | 需要 |
|---|---|---|
| [cf_check](cloudflare/cf_check.rs) | 过 Cloudflare 盾:访问受保护页,观察自动通过 challenge | 🌐 · camoufox |
| [exa_cf](cloudflare/exa_cf.rs) | auth.exa.ai 交互式过盾:填邮箱 → 触发 Turnstile → 等 token | 🌐 · cdp |

## 高并发池 / 代理(pool/)

| 示例 | 说明 | 需要 |
|---|---|---|
| [pool_crawl](pool/pool_crawl.rs) | `BrowserPool` 并发 + cookie 隔离 + 指纹轮换 + 重试 + 断点续抓 | 🔌 · camoufox |
| [proxy_health](pool/proxy_health.rs) | 代理池健康检查 + 出口地理探测 + IP↔指纹一致性 | 🌐 · camoufox |

## 双模 / 会话 / TLS 指纹(session/)

| 示例 | 说明 | 需要 |
|---|---|---|
| [session_mode](session/session_mode.rs) | Session(HTTP)双模 + cookie 互通(对标 DP Driver+Session) | 🔌 · camoufox |
| [web_page_scrape](session/web_page_scrape.rs) | WebPage 双模 + cookie 同步 + 表格提取 + 翻页 + CSV/JSON 导出 | 🔌 · camoufox |
| [session_tls](session/session_tls.rs) | Session 浏览器 TLS/JA3/JA4 指纹(wreq + BoringSSL),JA3 实测对比 | 🌐 · impersonate |

## 截图 / 录像 / 下载 / 持久化(camoufox/)

| 示例 | 说明 | 需要 |
|---|---|---|
| [screencast](camoufox/screencast.rs) | 截图与录像(对标 DP `browser_control/screen`) | 🔌 |
| [download_manager](camoufox/download_manager.rs) | 下载管理 `tab.downloads()` 端到端自验证 | 🔌 |
| [extras_demo](camoufox/extras_demo.rs) | 逐字符拟人输入 + 登录态全量持久化(storageState) | 🔌 |
| [fetch_browser](camoufox/fetch_browser.rs) | 预下载 / 校验本机 Camoufox 可执行文件 | 🌐 |

## 吐环境(补环境)/ 纯算签名(env/ · sites/)

| 示例 | 说明 | 需要 |
|---|---|---|
| [dump_env_fingerprint](env/dump_env_fingerprint.rs) | canvas/webgl/audio 指纹补环境 + 一键导出工程 | 🔌 · camoufox |
| [env_signer](env/env_signer.rs) | 自包含「补环境 + 纯算签名」运行器(内嵌 QuickJS,无 Node/浏览器) | 🔌 · signer |
| [douyin_dump_env](sites/douyin_dump_env.rs) | 通用吐环境能力(以抖音 a_bogus 为目标参数) | 🌐 · camoufox |
| [douyin_capture](sites/douyin_capture.rs) | 抓指定视频 detail(含 a_bogus)并导出可复现包 | 🌐 · camoufox |

## CDP / Chromium 后端(cdp/,**默认后端**,Google Chrome,无需 feature)

| 示例 | 说明 | 需要 |
|---|---|---|
| [cdp_demo](cdp/cdp_demo.rs) | CDP demo:启动/接管 Google Chrome → 导航 → run_js → 元素文本 → 截图 | 🌐 |
| [cdp_advanced](cdp/cdp_advanced.rs) | CDP 深化能力端到端自验证(进程内 HTTP 服务,Chrome 访问 localhost) | 🔌 |
| [cdp_fetch](cdp/cdp_fetch.rs) | **自动下载 Chrome for Testing** 并用它驱动 example.com | 🌐 |
| [cdp_download](cdp/cdp_download.rs) | CDP 原生下载事件多任务管理 `tab.downloads()` | 🔌 |
| [cdp_keyboard](cdp/cdp_keyboard.rs) | 键盘:拟人输入 + 修饰组合键/热键(Ctrl/Cmd+A 等) | 🔌 |
| [cdp_pool](cdp/cdp_pool.rs) | CDP 高并发池 `ChromiumPool`(多 worker + 每任务独立 context + 断点续抓) | 🔌 |
| [cdp_dump_env](cdp/cdp_dump_env.rs) | CDP 吐环境 `tab.dump_env()`(探针 + env.js + 导出工程 + 双跑验证) | 🌐 |
| [cdp_extras](cdp/cdp_extras.rs) | **标配补齐**(对标 PW/Puppeteer/DP):set_content / 媒体深色 / 网络离线 / 设备 / storage / `wait().network_idle` / HAR / expose / PDF / MHTML 端到端 | 🔌 |
| [cdp_recorder](cdp/cdp_recorder.rs) | **录制生成代码 + 无障碍快照**(对标 PW codegen / a11y):`tab.recorder()` 录操作 → 生成可运行 Rust;`ax_tree()` / `ax_snapshot()` 语义树 | 🔌 |

## Windows 专项(windows/,在 Windows 上运行)

| 示例 | 说明 | 需要 |
|---|---|---|
| [win_smoke](windows/win_smoke.rs) | Windows 冒烟:启动 Camoufox → fd3/4 命名管道 Juggler 握手 | 🌐 |
| [win_diag](windows/win_diag.rs) | Windows 传输诊断:无头 / 有头两种模式完整链路 | 🌐 |
| [win_cf_test](windows/win_cf_test.rs) | Windows 过 CF 盾:有头启动 → 访问受保护页 | 🌐 |
| [win_bilibili_test](windows/win_bilibili_test.rs) | Windows 硬核:多 P 长监听 + 后台抽取不丢包 + 点分集 | 🌐 |

---

更多用法对照见 [DrissionPage → drission API 映射](../docs/API映射.md);设计原理见 [docs/](../docs/README.md)。
