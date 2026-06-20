# DrissionPage → drission(Rust)API 映射

> 目标:让用过 DP 的人几乎零成本上手。Rust 版多了 `.await` 与 `Result`。

## 选后端:默认 CDP/Chrome(最贴近 DP)还是 Camoufox 反检测

DP 本身驱动 Chromium,所以与 DP 最对应的是 drission 的**默认 CDP 后端**(`ChromiumBrowser`,直接驱动 Google Chrome):

| DrissionPage (Python) | drission · CDP 后端(默认,Google Chrome) |
|---|---|
| `from DrissionPage import Chromium` | `use drission::prelude::*;` |
| `browser = Chromium()`(本机 Chrome) | `let browser = ChromiumBrowser::launch_default().await?;`(有头 + 反检测默认开) |
| 无头 | `ChromiumBrowser::launch(ChromiumOptions::new().headless(true)).await?` |
| `ChromiumOptions().set_browser_path(path)` | `ChromiumBrowser::launch_with(path, headless).await?`(指定浏览器) |
| `browser = Chromium('127.0.0.1:9222')`(接管) | `ChromiumBrowser::connect("http://127.0.0.1:9222").await?` |
| `tab = browser.new_tab(url)` | `let tab = browser.new_tab(Some(url)).await?;` |
| `browser.quit()` | `browser.quit().await?;` |
| (探测/诊断浏览器路径) | `ChromiumBrowser::find_chrome()?` / `drission::cdp::chrome_path()?` |

> **浏览器路径探测对标 DP `get_chrome_path`**:`CHROME_BIN`/`DRISSION_CHROME` → 安装路径(Windows 含用户级
> `%LOCALAPPDATA%`)→ Windows 注册表 `App Paths\chrome.exe` → 系统 `PATH`,**默认优先 Google Chrome**。

需要 **Firefox 反检测内核**(过盾 / 吐环境 / 池 / 滑块 / Session 双模等全部高层能力)时,开 `--features camoufox`,
用下面的 `Browser` / `Page` / `WebPage` / `SessionPage`(语法同样对标 DP,且默认即反检测)。

## 大道至简:一行起步(`Page` ≈ DP `ChromiumPage`,需 `--features camoufox`)

DP 里 `page = ChromiumPage()` 一行开浏览器并直接驱动。drission 的 [`Page`] 把「开浏览器 + 当前标签」
合一,通过 `Deref` 拥有**全部 `Tab` 方法**,日常脚本首选:

| DrissionPage (Python) | drission (Rust) |
|---|---|
| `page = ChromiumPage()`(有头) | `let page = Page::new().await?;` |
| 无头 | `let page = Page::headless().await?;` |
| 自定义选项 | `let page = Page::with(BrowserOptions::new().headless(true)).await?;` |
| `page.get(url)` | `page.get(url).await?;` |
| `page.ele('#x')` | `page.ele("#x").await?` |
| `page.ele('#x').click()` | `page.click("#x").await?`(捷径)或 `page.ele("#x").await?.click().await?` |
| `page.ele('#x').input('hi')` | `page.input("#x", "hi").await?`(捷径) |
| 判断元素是否存在 | `page.exists("#x").await?`(立即判定,不等待) |
| `page.title` / `page.url` / `page.html` | `page.title().await?` / `page.url().await?` / `page.html().await?` |
| `page.run_js('...')` | `page.run_js("...").await?` |
| `tab = page.new_tab(url)` | `let tab = page.new_tab(Some(url)).await?;` |
| 接管已开浏览器 | `let page = Page::connect(ws).await?;` |
| `page.quit()` | `page.quit().await?;` |
| (更底层:多标签 / 接管 / 并发) | `page.browser()` 拿 `Browser`,或直接用 `Browser` + `Tab` |

> `Page` 是**附加**门面,不替代 `Browser`/`Tab`——下面各表的 `tab.*` 方法在 `page` 上**同样可用**
> (经 `Deref`)。需要 Driver/Session 双模见 `WebPage`,纯 HTTP 见 `SessionPage`。

## 启动与标签

| DrissionPage (Python) | drission (Rust) |
|---|---|
| `from DrissionPage import Chromium` | `use drission::prelude::*;` |
| `browser = Chromium()` | `let browser = Browser::launch_default().await?;`(有头 + 反检测默认开) |
| `browser = Chromium(options)` | `let browser = Browser::launch(opts).await?;` |
| 无头 | `Browser::launch(BrowserOptions::new().headless(true)).await?` |
| `tab = browser.latest_tab` | `let tab = browser.latest_tab().await?;` |
| `tab = browser.new_tab(url)` | `let tab = browser.new_tab(Some(url)).await?;` |
| `tab2 = browser.get_tab(1)` | `let tab2 = browser.get_tab(1).await?;` |
| `browser.quit()` | `browser.quit().await?;` |

## WS 接管已运行的浏览器(对应 DP 接管模式)

把浏览器跑成常驻 ws 服务,再用客户端 `connect` 接管驱动(对标 DP 用调试端口接管已开浏览器)。

| DrissionPage (Python) | drission (Rust) |
|---|---|
| 浏览器开远程调试端口常驻 | `let server = BrowserServer::launch(BrowserOptions::new().headless(true)).await?;` |
| (取接管地址) | `let ws = server.ws_endpoint();`(形如 `ws://127.0.0.1:<port>/<token>`) |
| `browser = Chromium('127.0.0.1:9222')`(接管) | `let browser = Browser::connect(ws).await?;` |
| 接管 + 自定义选项 | `Browser::connect_with(ws, BrowserOptions::new()).await?` |
| `browser.quit()`(只断开本地,不关浏览器) | `browser.quit().await?`(接管模式仅断开本地连接) |
| (真正关掉被接管的远端浏览器) | `browser.close_remote().await?` |
| (关掉服务端 + 浏览器) | `server.stop().await?` |

> **注意**:`ws://…/token` 讲的是**原始 Juggler**,只能由本库 `BrowserServer` 暴露;**不**兼容 `python -m camoufox server`
> 的 Playwright RPC 端点(协议不同)。当前为**单活动客户端**(同一时刻一个 `connect`,客户端断开后可再次接管);
> 当前 Camoufox 已移除浏览器内置 ws 模式,故由库自建"管道 ⇆ ws"中转。完整端到端自验证见 `examples/ws_connect`。

## 选项 / 浏览器信息(ChromiumOptions → BrowserOptions)

> **默认即反检测(大道至简,这块不对标 DP)**:`BrowserOptions::default()` = **有头** +
> `humanize` + `block_webrtc` 默认开 + 自动定位/下载浏览器。所以零配置 `Browser::launch_default()`
> 就是一个反检测浏览器;想改哪项再链式覆盖(`.headless(true)` 无头、`.humanize(false)` 关拟人…)。
> 不默认 locale/timezone:强设与本机 IP 不符的地区反而降低可信度,按需自己设(见 `cf_check`)。

| DrissionPage | drission |
|---|---|
| `co.headless(True)` / `headless(False)` | `BrowserOptions::new().headless(true)` / `.headless(false)`(默认有头) |
| `co.set_user_agent(ua)` | `.user_agent(ua)` |
| `co.set_argument('--xx')` | `.add_arg("--xx")` |
| `co.set_proxy('http://…')` | `.proxy(Proxy::new("http://…"))` |
| 设置语言 | `.locale("zh-CN")` |
| 设置时区 | `.timezone("Asia/Shanghai")` |
| 设置窗口大小 | `.window_size(1280, 800)` |
| 设置地理位置 | `.geolocation(31.23, 121.47)` |
| 指定浏览器路径 | `.binary_path("/path/Camoufox")` |
| (默认自动下载 Camoufox) | 留空 `binary_path` 即自动下载分发 |
| 拟人化光标(过盾) | `.humanize(true)` / `.humanize_max_time(1.5)` |
| 阻断 WebRTC(防 IP 泄漏) | `.block_webrtc(true)` |
| 自定义 Firefox pref | `.add_pref("media.peerconnection.enabled", json!(false))` |

## 页面操作

| DrissionPage | drission |
|---|---|
| `tab.get(url)` →`bool` | `tab.get(url).await?` →`bool` |
| `tab.get(url, retry=2, timeout=10, ...)` | `tab.get_with(url, &GetOptions::new().retry(2).timeout(d).load_mode(LoadMode::Eager)).await?` |
| `tab.title` | `tab.title().await?` |
| `tab.url` | `tab.url().await?` |
| `tab.html` | `tab.html().await?` |
| `tab.user_agent` | `tab.user_agent().await?` |
| `tab.ready_state` | `tab.ready_state().await?` |
| `tab.url_available` | `tab.url_available()`(同步,读最近一次 `get` 结果) |
| `tab.run_js(js)` | `tab.run_js(js).await?` |
| `tab.refresh()` | `tab.reload().await?` |
| `tab.back()` | `tab.back().await?` |
| `tab.forward()` | `tab.forward().await?` |
| `tab.stop_loading()` | `tab.stop_loading().await?`(JS `window.stop()`) |
| `tab.wait.doc_loaded()` | `tab.wait_loaded().await?` 或 `tab.wait().doc_loaded(None).await?` |

> `get` 失败返回 `false`(不抛错);加载模式 `LoadMode::{Normal,Eager,None}` 对应 DP 的 `'normal'/'eager'/'none'`。

## 静态元素(对应 DP 的 `s_ele`/`s_eles`,离线解析页面 HTML)

| DrissionPage | drission |
|---|---|
| `tab.s_ele('tag:h1')` | `tab.s_ele("tag:h1").await?` →`StaticElement` |
| `tab.s_eles('tag:a')` | `tab.s_eles("tag:a").await?` →`Vec<StaticElement>` |
| (取静态根) | `tab.s_root().await?` |
| `ele.s_ele('.sub')` | `ele.s_ele(".sub").await?`(解析元素 outerHTML) |
| `s_ele.text` | `s_ele.text()?`(同步) |
| `s_ele.attr('href')` | `s_ele.attr("href")?` |
| `s_ele.tag` | `s_ele.tag()?` |
| `s_ele.html` / `inner_html` | `s_ele.html()?` / `s_ele.inner_html()?` |
| `s_ele.ele(..)` / `s_ele.eles(..)` | `s_ele.ele("..")?` / `s_ele.eles("..")?` |
| `tab.s_ele('xpath://li[2]')` | `tab.s_ele("xpath://li[2]").await?`(内置 XPath 子集) |
| 离线解析任意 HTML 串 | `StaticElement::parse(html)?` |

> 静态端 `xpath:` 由**内置 XPath 1.0 子集求值器**支持:`//`/`/`/`*`/标签名、谓词 `[n]`/`[last()]`/
> `[position()=n]`/`[@a]`/`[@a="v"]`/`[@a!='v']`/`contains(@a,..)`/`contains(text(),..)`/
> `starts-with(..)`/`text()="v"`,以及 `and`/`or`/`not(...)`/括号组合。不支持的轴(`..`/
> `following-sibling::` 等)会报错并提示改用实时 `tab.ele()`(走浏览器原生 `document.evaluate`)。
> CSS / `tag:` / `@attr` / `text:` 也都支持。
> `StaticElement` 非 `Send`(基于 `scraper`/`Rc`),勿跨 `tokio::spawn` 任务传递。

## 句柄对象(对应 DP 的 `tab.wait` / `tab.scroll` / `tab.set`)

| DrissionPage | drission |
|---|---|
| `tab.wait(2)` | `tab.wait().secs(2.0).await` |
| `tab.wait.doc_loaded()` | `tab.wait().doc_loaded(None).await?` |
| `tab.wait.ele_displayed(loc)` | `tab.wait().ele_displayed("#id", None).await?` |
| `tab.wait.ele_loaded(loc)` | `tab.wait().ele_loaded("#id", None).await?` |
| `tab.wait.ele_deleted(loc)` | `tab.wait().ele_deleted("#id", None).await?` |
| `tab.scroll.to_top()` / `to_bottom()` | `tab.scroll().to_top().await?` / `to_bottom().await?` |
| `tab.scroll.to_location(x,y)` | `tab.scroll().to_location(x, y).await?` |
| `tab.scroll.up/down/left/right(px)` | `tab.scroll().down(px).await?` 等 |
| `tab.set.timeouts(t)` | `tab.set().timeout(Duration::from_secs(10))` |
| `tab.set.load_mode('eager')` | `tab.set().load_mode(LoadMode::Eager)` |
| `tab.set.user_agent(ua)` | `tab.set().user_agent(ua).await?` |
| `tab.set.cookies(...)` | `tab.set().cookies(cookies).await?` |

## 截图 / 尺寸(对应 DP `browser_control/screen`)

| DrissionPage | drission |
|---|---|
| `tab.get_screenshot(path, full_page=True)` | `tab.get_screenshot(path, true).await?` →`PathBuf`(格式按 path 后缀) |
| `tab.get_screenshot(as_bytes='png')` | `tab.screenshot_bytes(full_page).await?` →`Vec<u8>`(PNG) |
| `tab.get_screenshot(as_base64='png')` | `tab.screenshot_base64(full_page).await?` →`String` |
| `tab.get_screenshot(as_bytes='jpeg')` | `tab.screenshot(&ShotOpts::new().format(ImageFormat::Jpeg).quality(80)).await?` →`Vec<u8>` |
| `tab.get_screenshot(left_top=(l,t), right_bottom=(r,b))` | `tab.screenshot(&ShotOpts::new().region((l,t),(r,b))).await?` →`Vec<u8>` |
| `tab.get_screenshot(full_page=True)`(字节) | `tab.screenshot(&ShotOpts::new().full_page(true)).await?` |
| `img = tab('tag:img'); img.get_screenshot()` | `tab.ele("tag:img").await?.get_screenshot(path).await?` →`PathBuf` |
| `img.get_screenshot(as_bytes='png')` | `tab.ele("tag:img").await?.screenshot_bytes().await?` →`Vec<u8>` |
| `tab.rect.size` | `tab.size().await?` →`(w, h)` 视口 |
| (整页内容尺寸) | `tab.page_size().await?` →`(w, h)` |
| `tab.rect.*` | `tab.rect().await?` →`PageRect` |

> 图片格式 `ImageFormat`:仅 `Png`/`Jpeg`(Camoufox `Page.screenshot` 实测只支持这两种,**不支持 webp**)。
> `get_screenshot` 按文件后缀自动选格式(`.jpg`/`.jpeg`→JPEG,其余 PNG)。常用走 `get_screenshot`/`screenshot_bytes`,
> 要区域 / JPEG 质量 / 整页字节就用 `screenshot(&ShotOpts)`。元素截图**默认先滚到视口中央**(同 DP `scroll_to_center=True`)。

## 录像(对应 DP `tab.screencast`)

链式句柄;**大道至简 = 后台按帧间隔反复截视口**(不走 Juggler 原生 screencast)。

| DrissionPage | drission |
|---|---|
| `tab.screencast.set_save_path('video')` | `tab.screencast().set_save_path("video")`(同步链式) |
| `tab.screencast.set_mode.imgs_mode()` | `tab.screencast().set_mode(ScreencastMode::Imgs)` |
| `tab.screencast.set_mode.frugal_imgs_mode()` | `tab.screencast().set_mode(ScreencastMode::FrugalImgs)` |
| `tab.screencast.set_mode.video_mode()` | `tab.screencast().set_mode(ScreencastMode::Video)`(停止时 ffmpeg 合成 mp4) |
| `tab.screencast.set_mode.frugal_video_mode()` | `tab.screencast().set_mode(ScreencastMode::FrugalVideo)` |
| (设置帧率,DP 无) | `tab.screencast().set_fps(10.0)` |
| `tab.screencast.start()` | `tab.screencast().start(None::<&str>).await?` |
| `tab.screencast.start('video')` | `tab.screencast().start(Some("video")).await?` |
| `path = tab.screencast.stop()` | `let path = tab.screencast().stop().await?` →`PathBuf`(imgs=帧目录 / video=mp4) |
| (是否在录) | `tab.screencast().is_recording()`(同步) |

```rust
let cast = tab.screencast();
cast.set_save_path("video").set_mode(ScreencastMode::Imgs).set_fps(10.0);
cast.start(None::<&str>).await?;
tab.wait().secs(3.0).await;
let out = cast.stop().await?;   // imgs→帧目录;video→mp4
```

> 模式:`Imgs` 持续逐帧存图、`FrugalImgs` 仅画面变化才存(按字节去重)、`Video`/`FrugalVideo` 取帧后用
> **ffmpeg** 合成 mp4(对标 DP video 模式需 opencv,这里换成更通用的 ffmpeg CLI,**不引 Rust 依赖**;
> 未装 ffmpeg 会报错并提示改用 `Imgs`)。`set_*` 同步链式,`start`/`stop` 异步。完整自验证见 `examples/screencast`。

## 元素定位(语法完全一致)

| DrissionPage | drission |
|---|---|
| `tab.ele('#kw')` | `tab.ele("#kw").await?` |
| `tab.ele('.cls')` | `tab.ele(".cls").await?` |
| `tab.ele('@id:kw')` / `@id=kw` | `tab.ele("@id:kw").await?` |
| `tab.ele('tag:li')` / `t:li` | `tab.ele("tag:li").await?` |
| `tab.ele('text:登录')` / `登录` | `tab.ele("登录").await?` |
| `tab.ele('css:div.box')` | `tab.ele("css:div.box").await?` |
| `tab.ele('xpath://a')` | `tab.ele("xpath://a").await?` |
| `tab.eles('tag:li')` | `tab.eles("tag:li").await?` |
| `ele.ele('.sub')`(下级) | `ele.ele(".sub").await?` |

## 元素操作

| DrissionPage | drission |
|---|---|
| `ele.input('text')` | `ele.input("text").await?` |
| `ele.click()` | `ele.click().await?` |
| `ele.clear()` | `ele.clear().await?` |
| `ele.text` | `ele.text().await?` |
| `ele.attr('href')` | `ele.attr("href").await?` |
| `ele.run_js(js)` | `ele.run_js(js).await?` |
| `ele.is_displayed()` | `ele.is_displayed().await?` |
| `ele.hover()` | `ele.hover().await?` |
| `ele.drag(dx, dy, dur)` | `ele.drag(dx, dy, 0.8).await?`(拟人轨迹) |
| `ele.drag_to(x, y, dur)` | `ele.drag_to(x, y, 0.8).await?` |
| `ele.set_files(paths)` / 上传 | `ele.set_files(&["/abs/f"]).await?` / `ele.upload("/abs/f").await?` |

## 相对定位(对应 DP 的相对元素查找)

从一个锚点元素出发,按 DOM 关系取父 / 子 / 兄弟元素。返回的 `Element` 与锚点**归属同一 frame**
(iframe 内元素的相对定位也正确)。

| DrissionPage | drission |
|---|---|
| `ele.parent()` | `ele.parent().await?` →`Element`(无父则 `Err`) |
| `ele.parent(level)` | `ele.parent_n(level).await?`(`level=1` 即直接父级) |
| `ele.parent('tag:div')` | `ele.parent_until("tag:div").await?`(最近的匹配祖先;仅 CSS 系) |
| `ele.children()` | `ele.children().await?` →`Vec<Element>`(直接子元素) |
| `ele.child(i)` | `ele.child(i).await?`(**0 基**,注意 DP 为 1 基;越界 `Err`) |
| `ele.next()` | `ele.next().await?`(下一个同级元素) |
| `ele.prev()` | `ele.prev().await?`(上一个同级元素) |
| `ele.nexts()` | `ele.nexts().await?` →`Vec<Element>`(后面所有同级,文档序) |
| `ele.prevs()` | `ele.prevs().await?` →`Vec<Element>`(前面所有同级,文档序) |
| (所有同级,不含自身) | `ele.siblings().await?` →`Vec<Element>` |

> `parent_until` 仅支持 CSS 系定位(`#`/`.`/`tag:`/`@attr`→不支持、`css:`);xpath 祖先请改用实时
> `tab.ele("xpath://...")`(走浏览器原生 `document.evaluate`)。找不到目标(如末元素的 `next()`、越界
> `child()`)统一返回 `Err(ElementNotFound)`,与 `tab.ele` 风格一致。端到端自验证见 `examples/relative_shadow`。

## Shadow DOM(对应 DP `ele.shadow_root`)

带 Web Component 的站点(自定义元素挂 **open** shadow root)用它进入 shadow 内继续查元素。

| DrissionPage | drission |
|---|---|
| `root = ele.shadow_root` | `let root = ele.shadow_root().await?` →`ShadowRoot` |
| `root.ele('.btn')` | `root.ele(".btn").await?`(shadow 内查找) |
| `root.eles('tag:span')` | `root.eles("tag:span").await?` →`Vec<Element>` |
| `root.html` | `root.html().await?`(shadow 内 `innerHTML`) |
| `root.run_js(js)`(`root` 即 shadow root) | `root.run_js("return root.childElementCount;").await?` |
| `root.s_ele(..)` / `root.s_eles(..)` | `root.s_ele("..").await?` / `root.s_eles("..").await?` |

> 仅 `mode:'open'` 的 shadow 可被脚本访问(`closed` 的 `shadowRoot` 为 `null` → 返回错误)。shadow 内
> **不支持** `xpath:`(`document.evaluate` 不进 shadow),定位仅 CSS 系。shadow 内查到的 `Element` 是普通元素,
> 后续 `click`/`text`/`input` 等照常可用。端到端自验证见 `examples/relative_shadow`。

## 鼠标与拖拽(滑块验证码等)

| 能力 | drission |
|---|---|
| 元素拟人拖拽(按住→轨迹→释放) | `ele.drag(dx, dy, duration_secs).await?` |
| 拖到视口绝对坐标 | `ele.drag_to(x, y, duration_secs).await?` |
| 悬停 | `ele.hover().await?` |
| 低层:移动(未按) | `tab.mouse_move(x, y).await?` |
| 低层:按下左键 | `tab.mouse_down(x, y).await?` |
| 低层:**按住**移动(拖拽中) | `tab.mouse_drag(x, y).await?`(`buttons=1`) |
| 低层:松开左键 | `tab.mouse_up(x, y).await?` |
| 低层(**不等往返**):移动 | `tab.mouse_move_fast(x, y)?`(同步,密集采样用) |
| 低层(**不等往返**):按住移动 | `tab.mouse_drag_fast(x, y)?`(同步,`buttons=1`) |
| 输入反检测:修补空 `pointerType` | `tab.apply_pointer_stealth().await?`(**导航前**调用) |

> `ele.drag` 轨迹为 **minimum-jerk 钟形速度**(`10t³-15t⁴+6t⁵`,慢起→中段最快→慢收)+ 时间驱动密集
> 采样(真人 60~120Hz)+ 手抖 + 纵向漂移 + 偶发迟疑 + 末段过冲回拉,中间 move 走 fire 快路径。
> 需要**边拖边读真实位置纠偏**(如滑块对齐缺口)时,用 `mouse_down`→多次 `mouse_drag_fast`(密集)→
> 读位置前用一次会等待的 `mouse_drag` 作"屏障"→`mouse_up` 做闭环,见 `examples/geetest_slide`。
> `dispatchMouseEvent` 是可信事件(`isTrusted=true`),能真正驱动滑块。
>
> **`*_fast`(fire 快路径)说明**:普通 `mouse_move/drag` 每事件等一次 ~20ms 往返,密集轨迹会偏稀疏、
> 节奏规整(滑块风控破绽);`*_fast` 不等往返,把节奏交给 `sleep`,实测采样 ~60ms/点 → ~20ms/点。
> 仅适合 move/down/up 这类**无需返回值**的输入;需要读取结果就用会等待的版本(它天然是 fire 之后的"屏障")。
>
> **`apply_pointer_stealth`**:经 `Page.dispatchMouseEvent` 衍生的 PointerEvent 其 `pointerType` 为空串 `""`
> (真实鼠标应 `"mouse"`),是滑块/行为风控识别合成输入的破绽。本方法用一个轻量 getter 补丁把空值改回
> `"mouse"`(只补这一处、不动 `Function.prototype.toString`),须在**导航前**注入(经 `add_init_script` 生效)。

## 通用滑块验证码(drission-rs 库能力,不限极验)

把"缺口**模板匹配** + 闭环拟人拖动 + 判定 + 换图重试"封装成**与厂商无关**的 API:用 `SliderConfig`
描述图源/把手/判定,换厂商只换配置。缺口用模板匹配(整块形状/颜色对齐)比"缺口边缘 − 拼图边缘"
稳得多——后者两套不一致边缘误差叠加、落点系统性偏移。

| 能力 | drission |
|---|---|
| 图源:canvas / img | `ImageSource::canvas(".sel")` / `ImageSource::img("#sel")` |
| 配置(起步:底图 + 把手) | `SliderConfig::new(bg_src, "#handle")` |
| └ 链式:完整底图 / 拼图 / 弹出 / 换图 / 判定 / 比例 / 次数 | `.full_bg(src)` `.piece(src)` `.open("#btn")` `.refresh(".r")` `.success(SuccessCheck::Js("..."))` `.track_ratio(0.9)` `.max_attempts(8)` |
| 极验 v4 预设 | `SliderConfig::geetest_v4()` |
| 纯视觉:求拼图要移动的距离 | `let g = tab.slider_gap(&cfg).await?;`(验证图需已显示) |
| └ 位移 / 选用算法 / 形状法 / 颜色法 / 置信 | `g.displace` / `g.method` / `g.by_shape` / `g.by_color` / `g.confidence` |
| 一把梭:弹出→匹配→拖动→判定→重试 | `let r = tab.solve_slider(&cfg).await?;` |
| └ 结果 | `r.passed` / `r.attempts` / `r.align_error` |
| 极验便捷封装 | `tab.geetest_slide_gap().await?` / `tab.solve_geetest_slide().await?` |

```rust
// 极验:预设一把梭(导航前先注入 pointerType 反检测)。
tab.apply_pointer_stealth().await?;
tab.get("https://demos.geetest.com/slide-float.html").await?;
let r = tab.solve_slider(&SliderConfig::geetest_v4().max_attempts(8)).await?;
println!("通过={} 尝试={}次 对齐={:.1}px", r.passed, r.attempts, r.align_error);

// 其它厂商(只有底图+拼图、img 图源、自定义判定):换个配置即可。
let cfg = SliderConfig::new(ImageSource::img("#bg"), "#slider-handle")
    .piece(ImageSource::img("#puzzle"))
    .success(SuccessCheck::Js("window.captchaPassed===true".into()))
    .track_ratio(0.9);                 // 把手:拼图位移比(给了 piece 则可自动闭环标定,可不设)
let r = tab.solve_slider(&cfg).await?;
```

> **自动选法**(按可得素材):`full_bg`+`piece` → 双图法(最准,极验);只 `piece` → 拼图模板法
>(拼图轮廓对底图边缘);只 `bg` → 缺口探测(纵向边缘最强列,best-effort)。给了 `piece` 拖动会
> **闭环纠偏**(标定把手:拼图比例 + 读真实位置校正);否则按 `track_ratio`(默认 1.0)开环。判定
> `SuccessCheck::{Visible(选择器可见) / Js(表达式) / None(拖完即成)}`,非通过点 `refresh`(逗号多选)
> 或 `open` 换图重试至 `max_attempts`。`geetest_v4()` 预设实测可过 **slide-float 与 slide-custom** 两种
> 模式(成功判定用多信号 JS 兼容两者)。端到端:`examples/geetest_slide`(极验,`URL` 环境变量切
> float/custom)、`examples/slider_local`(离线合成 img 滑块自验证);缺口诊断 + 叠加验证图 `examples/geetest_diag`。

## 动作链(对应 DP `tab.actions` / Selenium ActionChains)

链式串起一组动作,`perform().await` 一次顺序执行。**按住期间的移动自动是拖拽**。

| DrissionPage | drission |
|---|---|
| `tab.actions.move_to(ele)` | `tab.actions().move_to_ele(&ele)` |
| `tab.actions.move_to((x,y), duration=d)` | `.move_to(x, y, d)` |
| `tab.actions.move(dx, dy, duration=d)` | `.move_by(dx, dy, d)` |
| `tab.actions.up/down/left/right(px)` | `.up(px)` / `.down(px)` / `.left(px)` / `.right(px)` |
| `tab.actions.hold(ele)` | `.hold()` 或 `.hold_on(&ele)` |
| `tab.actions.release(ele)` | `.release()` 或 `.release_on(&ele)` |
| `tab.actions.click()` / `r_click` / `db_click` / `m_click` | `.click()` / `.right_click()` / `.double_click()` / `.middle_click()` |
| `tab.actions.left_down()` / `left_up()` | `.mouse_down(MouseButton::Left)` / `.mouse_up(..)` |
| `tab.actions.scroll(dy, dx)` | `.scroll(dx, dy)` |
| `tab.actions.key_down/key_up(key)` | `.key_down("Shift")` / `.key_up("Shift")` |
| `tab.actions.type(text)` | `.type_text("hi")` |
| `tab.actions.wait(s)` | `.wait(s)` |
| `tab.actions.<...>.perform()`(DP 逐步即时执行) | `.perform().await?`(队列式,末尾统一执行) |

```rust
// 拖放:移到源 → 按住 → 拖到目标 → 释放
tab.actions().move_to_ele(&src).hold().move_to_ele(&dst).release().perform().await?;
```

> 与 DP 的差异:DP 的动作**每调一步即时执行**;Rust 版为契合 async,采用**队列式**——链式收集动作,
> 最后 `.perform().await` 顺序执行(读起来一致,见 `examples/actions_drag`)。

## 每标签独立 cookie

| DrissionPage | drission |
|---|---|
| `tab.set.cookies(...)` | `tab.set_cookies(cookies).await?` |
| `tab.cookies()` | `tab.cookies().await?` |
| (不同标签不同 cookie) | 每个 Tab 绑定独立 BrowserContext,天然隔离 |

## 网络监听 / 抓包

句柄式(对应 DP `tab.listen.*`,推荐):

| DrissionPage | drission |
|---|---|
| `tab.listen.start('api/data')` | `tab.listen().start(&["api/data"]).await?` |
| `tab.listen.listening` | `tab.listen().is_listening().await` |
| `packet = tab.listen.wait()` | `let packet = tab.listen().wait().await?` |
| `tab.listen.wait(count=3)` | `let pkts = tab.listen().wait_count(3, None).await?` |
| (带超时,超时返回 `None`) | `tab.listen().wait_timeout(d).await?` |
| `for p in tab.listen.steps():` | `let s = tab.listen().steps().await?; while let Some(p)=s.next_timeout(d).await? { … }` |
| `tab.listen.stop()` | `tab.listen().stop().await?` |
| `packet.response.body` / `packet.request` | `packet.response.body` / `packet.request` |

扁平式(等价,底层同一实现):`tab.listen_start` / `listen_wait` / `listen_next` / `listen_stream` / `listen_stop`。

> `wait_count(n, timeout)` 在总超时内尽量凑满 `n` 个包,不足则返回已抓到的(按 `len()` 自行判断);
> `steps()` 开启长监听后台抽取(不丢包),适合"边滑边持续抓"。

## 控制台监听(对应 DP `tab.console`)

| DrissionPage | drission |
|---|---|
| `tab.console.start()` | `tab.console().start().await?` |
| (按级别/文本过滤,DP 无) | `tab.console().start_with(ConsoleFilter::new().level("error").contains("sign")).await?` |
| `tab.console.listening` | `tab.console().listening()`(同步) |
| `data = tab.console.wait()` | `let data = tab.console().wait(None).await?`(`None`=无限等待,`Some(d)`=超时返回 `None`) |
| `tab.console.wait(timeout=5)` | `tab.console().wait(Some(Duration::from_secs(5))).await?` →`Option<ConsoleData>` |
| `for d in tab.console.steps():` | `let s = tab.console().steps(); while let Some(d)=s.next(Some(d)).await? { … }` |
| `tab.console.messages` | `tab.console().messages().await` →`Vec<ConsoleData>`(取后清空) |
| `tab.console.clear()` | `tab.console().clear().await` |
| `tab.console.stop()` | `tab.console().stop().await?` |
| `data.text` / `data.level` | `data.text` / `data.level` |
| `data.body`(text 的 json) | `data.body()` →`Option<Value>` |
| `data.source/url/line/column` | `data.source` / `data.url` / `data.line` / `data.column`;另有 `data.args: Vec<Value>` |

> 基于 Juggler **原生 `Runtime.console` 事件**(不 hook 页面 `console`,反检测更友好):只有 `console.log()`
> 等输出到控制台的内容才能拿到。原始类型参数零往返拼 `text`,对象/数组/Error 自动回页面 `JSON.stringify`
> 序列化(故 `console.log({a:1})` 的 `text` 就是 `{"a":1}`,`body()` 可直接解析)。`level`:`log`/`info`/
> `warning`/`error`/`debug`/`dir`/`trace` 等(`warn`→`warning`)。`line`/`column` 为 0 基。Juggler 下无法区分
> 来源,`source` 统一为 `console-api`;噪声多时用 `start_with(ConsoleFilter)` 过滤。

## iframe / 子框架(对应 DP `tab.get_frame` / `ele.frame`)

| DrissionPage | drission |
|---|---|
| `frame = tab.get_frame('#ifr')` | `let frame = tab.get_frame("#ifr").await?` |
| `frame = ele.frame` / `ele.shadow_root` | `let frame = ele.content_frame().await?` |
| `frame.ele('#inner')` | `frame.ele("#inner").await?`(在该帧上下文内查找) |
| `frame.eles('tag:a')` | `frame.eles("tag:a").await?` |
| `frame.html` | `frame.html().await?` |
| `frame.url` | `frame.url().await?` |
| `frame.run_js(js)` | `frame.run_js(js).await?` |
| `frame.s_ele(..)` / `frame.s_eles(..)` | `frame.s_ele("..").await?` / `frame.s_eles("..").await?` |

> 返回的 `Element` 归属该帧,后续 `click`/`text`/`input` 等都在正确的帧上下文执行。

## 文件上传 / JS 对话框

| DrissionPage | drission |
|---|---|
| `ele.input(path)`(文件框) | `ele.set_files(&["/abs/file"]).await?` / `ele.upload("/abs/file").await?` |
| (多文件) | `ele.set_files(&["/a", "/b"]).await?`(需 input 带 `multiple`) |
| `tab.handle_alert(accept=True)` | `tab.handle_next_dialog(true, None).await?` →`DialogInfo` |
| `tab.handle_alert(accept=True, send='x')`(prompt) | `tab.handle_next_dialog(true, Some("x")).await?` |

> `confirm()`/`prompt()` 会阻塞页面 JS,需与触发动作**并发**(`tokio::join!`)调用,见 `examples/page_extras`。

## 请求拦截 / 改写(DP 之外的增强)

| 能力 | drission |
|---|---|
| 开始拦截(URL 过滤) | `tab.intercept_start(&["/api/"]).await?` |
| 仅拦 XHR/fetch | `tab.intercept_xhr(&["/api/"]).await?` |
| 取下一个被拦请求 | `let req = tab.intercept_next().await?` |
| 原样放行 | `req.resume().await?` |
| 改写后放行 | `req.resume_with(ResumeOptions::new().method("POST")).await?` |
| 伪造响应 | `req.fulfill(200, headers, body).await?` |
| 中止请求 | `req.abort("blockedbyclient").await?` |
| 停止拦截 | `tab.intercept_stop().await?` |

> 开启拦截后,匹配过滤的请求交你决策,**不匹配的自动放行**。每个被拦请求必须用
> `resume`/`resume_with`/`fulfill`/`abort` 之一放行(方法消费所有权,保证只决策一次)。

## WebSocket 帧监听(DP 之外的增强)

| 能力 | drission |
|---|---|
| 开始监听(建连前) | `tab.websocket().start().await?` |
| 按 URL/方向/控制帧过滤 | `tab.websocket().start_with(WsFilter::new().url_contains("/im/").received_only().with_control()).await?` |
| 是否在监听 | `tab.websocket().listening()`(同步) |
| 等一帧(可超时) | `tab.websocket().wait(Some(Duration::from_secs(10))).await?` →`Option<WsMessage>` |
| 等 N 帧 | `tab.websocket().wait_count(5, None).await?` →`Vec<WsMessage>` |
| 取走已缓冲全部帧 | `tab.websocket().messages().await` →`Vec<WsMessage>` |
| 已知连接快照 | `tab.websocket().sockets().await` →`Vec<WsSocket>`(`url`/`opened`/`closed`/`error`) |
| 流式逐帧 | `let s = tab.websocket().steps(); while let Some(m)=s.next(Some(d)).await? { … }` |
| 停止 | `tab.websocket().stop().await?` |
| 帧方向 | `m.direction`(`WsDirection::Sent` / `Received`)、`m.url` / `m.socket_id` |
| 帧文本 / 字节 / JSON | `m.text()` →`Option<String>` / `m.bytes()` →`Vec<u8>` / `m.json()` →`Option<Value>` |
| 帧类型判断 | `m.is_text()` / `m.is_binary()` / `m.is_control()` / `m.opcode`(`m.opcode_name()`) |

> 基于 Juggler **原生 `Page.webSocket*` 事件**(不 hook 页面 `WebSocket`,反检测友好),抓页面 WebSocket 收发的每一帧。
> **务必在建立连接(导航 / `new WebSocket`)之前 `start()`**。`WsMessage.data` 原样保留:**文本帧(opcode 1)为原文,
> 二进制/控制帧为 base64**;用 `bytes()` / `json()` 自动还原。`WsFilter` 默认只收 text/binary 数据帧(丢弃 ping/pong/close),
> `with_control()` 可纳入。DrissionPage 无对应原生 API;完整端到端自验证见 `examples/ws_listen`(进程内本地 echo 服务)。
