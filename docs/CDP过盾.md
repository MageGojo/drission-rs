# CDP 后端过 Cloudflare 盾(反检测深化)

> 关联里程碑:52。案例 `examples/exa_cf.rs`(改用**谷歌浏览器 + 有头** CDP 跑)。

## 背景 / 问题

CDP 后端(`ChromiumBrowser`/`ChromiumTab`,驱动本机 Google Chrome)此前**过不了 Cloudflare 盾**
(交互式 Turnstile 不出 token、托管挑战不放行)。Camoufox 后端早已过盾(里程碑 17/36),
但 CDP 后端缺反检测,被 CF 一眼识破为自动化。

## 根因(为什么 CDP 被识破)

1. **`Runtime.enable` 泄漏(头号原因)**。`attach` 时无条件 `core.send("Runtime.enable", …)`。
   Cloudflare / DataDome 等用经典手法探测**已开启的 CDP `Runtime` 域**:页面构造一个带 getter
   的对象并 `console.log/console.debug` 它,若 `Runtime.enable` 在线,调试端会**序列化**该对象、
   触发 getter → 页面据此判定"正被 CDP 调试" → 判机器人。nodriver / rebrowser-patches 的核心
   就是**绝不调用 `Runtime.enable`**。`Runtime.evaluate`/`callFunctionOn` **无需** `Runtime.enable`
   即可工作(省略 `contextId` 时在被检视页面的默认上下文求值),故可安全移除。

2. **无反检测启动参数**。`launch` 只给了基础参数,缺 `--disable-blink-features=AutomationControlled`
   等;且未排除自动化开关。

3. **无导航前注入**。没有 `Page.addScriptToEvaluateOnNewDocument`,无法在页面脚本运行前消除
   残留指纹(如个别情况下 `navigator.webdriver`)。

## 方案(大道至简,组件化)

CDP 后端默认**反检测开箱即用**(对齐 Camoufox 后端默认有头 + 反检测的取向):

- **去掉 `Runtime.enable` 泄漏**:`attach` 不再调用它;`run_js`/`ele`/`callFunctionOn` 照常工作
  (省略 contextId,在页面默认上下文求值,导航后自动指向新主帧上下文)。保留 `Page.enable`
  (导航 `loadEventFired` 需要,且**不是** CF 探测点)。
- **反检测启动参数**(`src/cdp/stealth.rs`):`--disable-blink-features=AutomationControlled`
  + 不加 `--enable-automation`(无"受自动化控制"信息栏)+ `--no-first-run`/`--no-default-browser-check`/
  `--password-store=basic`/(mac)`--use-mock-keychain`/`--disable-background-networking` 等良性项。
- **导航前注入**(`Page.addScriptToEvaluateOnNewDocument`):一段极小的兜底脚本——仅当
  `navigator.webdriver` 仍为 `true` 时把它改回 `false`(配合启动参数,通常是 no-op,**不引入
  可被探测的多余 getter**)。真实有头 Chrome 本就干净,不伪造 plugins/chrome 等(伪造反而更易被识破)。
- **UA / 地区走启动参数**(对标 DrissionPage,浏览器级、覆盖所有帧含 Turnstile 跨域 iframe;
  per-session 的 `Emulation` 覆盖到不了 OOPIF 子帧):UA 走 `--user-agent`(无头 `mask_ua` 自动
  去 `HeadlessChrome`、或显式指定),locale 走 `--lang`,timezone 走 `TZ` 环境变量。地区默认
  **不设**——与出口 IP 不符反降可信度(同 Camoufox 取舍)。

### 交互式 Turnstile 点击

已有 `tab.pass_cloudflare(timeout)`(`src/cdp/cloudflare.rs`):读跨域 iframe
(`challenges.cloudflare.com`)视口位置 → CDP `Input.dispatchMouseEvent`(`isTrusted=true`)
拟人**可信点击**左侧复选框。本次反检测到位后,该路径才真正有效(否则 CF 直接不放行)。

## API(新增,非破坏)

- `ChromiumOptions`:链式 builder(`headless`/`window_size`/`user_agent`/`locale`/`timezone`/
  `proxy`/`user_data_dir`/`stealth`/`binary_path`/`add_arg`)。
- `ChromiumBrowser::launch_opts(ChromiumOptions)`:按选项启动。
- 既有 `launch(headless)`/`launch_with(exe, headless)`/`launch_with_profile(...)` 保持不变,
  **默认启用 stealth**(内部走 `ChromiumOptions { stealth: true, .. }`)。
- `connect(...)` 接管的浏览器也注入 stealth init script、且不调用 `Runtime.enable`。

## 验证

- `examples/exa_cf.rs` 改用 `ChromiumBrowser`(谷歌浏览器,**有头**):访问 auth.exa.ai →
  (有托管挑战则 `pass_cloudflare`)→ 填邮箱 → 轮询 `input[name=cf-turnstile-response]` 出
  非空 token 即判过盾。
- `cargo test --features cdp --lib` 不回归;`cdp_demo`/`cdp_advanced` 行为不变(它们不依赖
  `Runtime.enable`)。

### 读 Python DrissionPage 源印证

clone `g1879/DrissionPage` 后核对(用户"听说能无头过"):
- **不全局开 `Runtime.enable`**:仅 `_units/console.py` 控制台监听时才开;`run_js`/evaluate/
  callFunctionOn/getProperties 全程不开 —— **与本次核心修复完全一致**。
- `set_user_agent` 走 **`--user-agent` 启动参数**(`_configs/chromium_options.py`),浏览器级、
  覆盖所有帧(含 Turnstile 跨域 iframe)。
- 默认参数(`configs.ini`)含 `--disable-site-isolation-trials`/`--test-type`/`--disable-infobars`。

### 无头也过盾:数据驱动"测差异→补差异"(亮点)

不抄别人 stealth、不认命,而是**自己量出无头 vs 有头到底差哪些指纹,精准补齐**。

1. **探针** `examples/cdp_fp`:全量 dump navigator/screen/window/WebGL/WebGL2/WebGPU/audio/
   permissions/plugins/chrome/mediaQuery/connection… 有头、无头各跑一遍。
2. **diff 实锤**:新无头(`--headless=new`)与有头**只差两处**——
   - **WebGL 完全不可用**(`getContext('webgl')` 返回 `null`,`ok:false`)←头号破绽;
   - **`screen` 是 800×600**(没有真实显示器是这个尺寸)。
   - 其余 `plugins=5`/`window.chrome`(loadTimes/csi/app)/permissions(denied 一致)/
     `hardwareConcurrency=10`/UA/`userAgentData.brands`/audio 等**新无头已与有头自洽**。
3. **精准补两处**:
   - **WebGL**:无头**不禁 GPU** + (mac)`--use-angle=metal` → 直接拿到**真实** `ANGLE Metal
     Apple M4`(与有头逐字一致,**零伪造、真 GPU**)。`DRISSION_HEADLESS_GPU=0` 可退回禁 GPU
     (GPU-less 服务器,但那样 WebGL 退化)。
   - **screen**:`Page.addScriptToEvaluateOnNewDocument` 注入 `headless_screen_js`,把
     `screen` 800×600 → 1920×1080(与窗口自洽)。
4. **补完再 diff**:只剩 `availHeight`(本机 dock 高度,CF 无从得知)、`screenY`(窗口位置)、
   `connection` rtt(动态网络)等**真实机器间本就会变**的项,**无一是无头签名**。

**实测**:**有头 + 无头(`HEADLESS=1`)访问 `auth.exa.ai` 都在 1 秒出 880 字节 Turnstile token、
可复现、结果一致** —— 无头也过盾,推翻"Turnstile 无头不可靠"的旧结论。

> **反直觉教训**:照搬 DrissionPage 默认的 `--test-type` + `--disable-site-isolation-trials`,
> 反而把**有头**也从"出 880 token"打成"不出 token"(`--test-type` 是已知自动化信号);**最小反检测集
> + 精准补实测差异**才是正解 —— 过 Turnstile **参数越少越准,不是越多越好**。

### 无头自动补:`mask_ua` + WebGL GPU + screen

- `ChromiumOptions::mask_ua`(默认开):无头探测 `chrome --version` 主版本→构造精简 UA
  (`Chrome/<major>.0.0.0`,无 `HeadlessChrome`),经 **`--user-agent` 启动参数**(浏览器级,
  覆盖 Turnstile 跨域子帧)下发。新无头 `userAgentData.brands` 默认就不含 Headless。
- 无头 + stealth 自动:不禁 GPU(真实 WebGL)+ 注入 screen 补丁。三者合一让无头指纹与有头无异。
