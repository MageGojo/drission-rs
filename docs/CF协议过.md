<!-- 路由别名: cf协议过, cloudflare协议, turnstile逆向, cf盾, attach_oopifs, oopif -->
# CF 盾「协议过」逆向(靶场 auth.exa.ai · Turnstile)

> 目标:**不做任何业务,只做 CF 挑战本身**。路线:**先浏览器辅助打通端到端,再往纯协议推**。
> 靶场 `auth.exa.ai` 挂的是 **Cloudflare Turnstile(L3,最硬一级)**;产物是登录表单的
> `cf-turnstile-response` token(**不是**整站 `cf_clearance`)。所以「纯协议」= 不开浏览器把这个
> token 算出来。

## 与既有「浏览器过盾」的区别(别串台)
- **浏览器过盾**(里程碑 36/52,`examples/cloudflare/exa_cf.rs`):真 Chrome + 反检测,让 Turnstile
  在真实环境**自然产出 token**(不开 `Runtime.enable`、真 GPU、屏幕自洽)。这是已完成的能力。
- **本主题(协议过)**:进 Turnstile 的**跨域 iframe** 扣出 VM、补环境,(终极)**不开浏览器纯算 token**。

## CF 挑战分级 & 纯协议可行性(诚实边界)
`L0 TLS/JA3/H2`(impersonate 已有)→ `L1 JS Challenge`(扣代码主战场)→ `L2 Managed` →
`L3 Turnstile`(VMP + 行为信号,纯协议地狱级,业界多退回真浏览器)。**auth.exa.ai = L3**。
纯协议且长期稳定通用业界都做不到;务实目标 = 跑通 PoC + 沉淀通用工具链,并标清每层哪能纯协议。

## 第一块通用能力:进跨域 iframe(OOPIF)逆向 —— `tab.attach_oopifs()`
- **背景**:Turnstile 跑在 `challenges.cloudflare.com` 的 **OOPIF(独立进程)**,主框架的
  `run_js` / `Page.createIsolatedWorld{frameId}` 进不去(`frame.rs` 的同进程子帧能力不适用)。
- **实现** `src/cdp/oopif.rs`:`Target.setAutoAttach{flatten:true}` 收子 target → 每个 `sessionId`
  造 `CdpCore` → `ChromiumTab`(**自带 `scripts()/debugger()/hook()/listen()`**,因为内核全按 session 工作)。
- **API**:`tab.attach_oopifs(settle) -> Vec<ChildTarget>` / `tab.wait_oopif(url_substr, timeout)`;
  `ChildTarget{ target_id, session_id, kind, url, tab() }`。嵌套 OOPIF 对 `child.tab().attach_oopifs()` 下钻。
- **example**:`examples/cloudflare/cf_turnstile_recon.rs`(对站点零硬编码逻辑,`attach_oopifs` 通用)。

## 侦察实测结论(2026-06-25)
- ✅ **`attach_oopifs` 成功进入 Turnstile OOPIF**:`challenges.cloudflare.com/cdn-cgi/challenge-platform/h/b/turnstile/f/ov2/av0/rch/...`。
- **iframe 内 27 个脚本**:入口 ~**225KB**(`turnstile/f/ov2` 主脚本)+ **VM 大脚本 ~1.3MB(被多次解析)**
  + 一堆 4~71B 小脚本(VM `eval` 痕迹)。
- **网络编排**:`turnstile/v0/b/<id>/api.js`(67KB loader)+ `/cdn-cgi/challenge-platform/h/b/scripts/jsd/<id>/main.js`
  (JSD 指纹采集 19.7KB)+ `POST .../jsd/oneshot/...`(指纹回传,0B 响应)。
- **VMP 实锤**:iframe 脚本明文 grep `turnstile`/`challenge`/`chl` = **0 命中**(字符串加密)。
- **逆向 vs 过盾冲突**:本次开 `Debugger` 进 iframe → **token=0**(CDP 探测代价)。故**三线分开**:
  - **逆向/扣代码线**:开 `Debugger` 进 iframe(不在乎本次出不出 token)。
  - **端到端线**:走 `exa_cf` 干净过盾(不开 `Runtime/Debugger`),稳定出 token。
  - **纯协议线**:扣出 VM 在 Node/QuickJS 跑(根本不碰浏览器,无 CDP 探测)。

## 端到端基线(浏览器辅助,已彻底坐实 ✅)
`examples/cloudflare/cf_turnstile_e2e.rs`(v3,全用库能力 `listen`/`ele`/`input_human`/`click`/截图):
- 干净 Chrome(不开 Debugger)13~21s 出 **816 字节** token,widget 绿「成功!」(截图 `cf_e2e_shot.png` 三次复现)。
- 点 **email 的 `Continue`**(xpath 精确匹配、排除 `Continue with Google`)走 magic-link → `tab.listen()` 抓到
  完整流程:`GET /api/auth/session` → **`POST /api/auth/verify-turnstile {token}` → `{"success":true,"mode":"verified"}`**
  → `providers`/`csrf` → `POST /api/auth/signin/email`(magic link 发出)。
- **token 真有效铁证** = `verify-turnstile` 返回 `success:true` 且流程走到 `signin/email`。
- **验真姿势(坑)**:必须让**页面带 provider 上下文**调 verify(点 email Continue);裸 `fetch` 只给 token
  会被拒 `{"success":false,"error":"Invalid auth provider"}`(这**不是** token 无效,是缺登录上下文)。
- **IP 风控**:反复跑同 IP 会变慢/升级(13s→21s,甚至升整页托管挑战);严重时挂代理或等冷却。
- 诊断:`is_cloudflare()`/`pass_cloudflare()` 语义对**整页托管挑战**;对**表单内嵌 widget** 会误判 `true`/等不到消失,
  内嵌 Turnstile 以 `cf-turnstile-response` 是否非空判过。

## 路线 B:补环境 Node 跑 entry.js(`cf-protocol-poc/`,2026-06-25)

> 目标:把 dump 出的 iframe 入口脚本 `entry.js` 原样搬进 Node,补一套「尽量真实 + 全程 Proxy 记录」的
> 浏览器环境让它自跑,只在边界 hook(XHR / `parent.postMessage`),数据驱动摸清它读哪些环境、token 怎么出。
> 工具:`run.js`(补环境骨架 + parent 模拟器)、`decode.js`(用 `strings.json` 解任意 dict 字面量)、
> `entry.raw.js`/`entry.deobf.js`(从 `target/cf-dump` 复制,save-first 防 `cargo clean`)。

### ✅ 已跑通(全程无浏览器)
- entry.js 顶层引导、**RSA 会话密钥协商**(`S^65537 mod W`,1024bit)、`trustedTypes` 策略、postMessage 心跳全部执行。
- **完整解码 iframe↔parent 握手协议**(`decode.js` 解 dict):
  - iframe→parent:`init` / `requestExtraParams` / `widgetStale` / `reloadApiJsRequest` / `turnstileResults` / `reject`。
  - parent→iframe:**`extraParams`**(带 `cData`/`chlPageData`/各 `timeXxxMs` 计时/`action`/`render`)→ 置 `E` 标志;
    **`execute`** → `oZ()`→`oj()` 启动主流程;`requestTurnstileResults` → 回 `turnstileResults`(**token 出口 = `cfChlOut`**);`forceFail`/`reloadApiJsRejected`。
  - **gate**:`oj` 启动后 `setTimeout(…, RnIvE4=120000)` 内若 `j['rOjl5']` 未置位 → `widgetStale`(等 parent 的 `execute`)。
  - **api.js 版本握手**:`extraParams.ch` 必须 == `oc`(=`(41673101345184).toString(16)`=`25e6c66701a0`),否则 iframe 发 `reloadApiJsRequest`。
- **解码并通过 `or()` 反爬环境能力门**(顺序 `1|2|5|7|0|6|3|4`):探测
  ① `crypto.getRandomValues` ② **Worker(`new Worker(URL.createObjectURL(new Blob(['"you"==="bot"'],{type:'text/javascript'})))`)**
  ③ `PerformanceObserver` ④(`xF` 标志短路)⑤ `ReadableStream.prototype.pipeTo` ⑥ `performance.getEntries` ⑦ `BigInt`。
  任一缺失 → `reject`(本 PoC 起初就栽在缺 `URL.createObjectURL`)。
- 模拟 parent 发 `extraParams`+`execute` → **`oj` 跑起来,构建出真实 challenge XHR URL**:
  `POST /cdn-cgi/challenge-platform/h/b/b/ov1/<…>:<ts>:ISFF-…/<rayId=BcQG3>/<iHqHz5 票据>/chl_api_m`,body=`F4(V)`(自定义编码的加密 env 包)。

### 🧱 纯离线边界(诚实结论)
- `execute`→`oj`→`runProgram` 进入**递归多态 VM**(`x1`@8029 → `FM` → `FB` → `x1`,热点 `Fo`@6298 重 BigInt)。
  V8 profiler:**83.8% C++**(BigInt 重算),**4s 派发 ~99.5 万 opcode 仍跑不完**(~25 万/s),类 proof-of-work。
- 跑不完的根因(两条,均非「再补一个属性」可解):
  1. **VM 交叉校验环境真实性** —— 实测它会读 `iframe.contentWindow.eval`(从纯净 iframe 取原生函数反 hook)、
     `navigator.gpu.requestAdapter`(WebGPU)、`RTCPeerConnection`(WebRTC)、`speechSynthesis`、canvas/svg 渲染、
     `keydown/pointermove/touchstart/mousemove/wheel` 行为信号 —— 桩值喂不饱,需**真实浏览器指纹**(正是库 `dump_env`/`hook` 干的)。
  2. **`chlPageData` 来自 CF 服务器**(经 parent api.js 下发),且 token 最终需**活服务器轮替 + 新鲜票据**(dump 的票据已过期 48min+)。
- 故 **L3 Turnstile「纯离线算 token」结构性不可行**(与本文档开头评估一致);务实落点 = 端到端浏览器基线(已坐实)+ 本 PoC 这套「补环境跑 + 协议/环境全解码」工具链。

### entry.js 实际读取的浏览器环境清单(补环境依据,`out/access_report.json`)
DOM:`createElement(div/a/span/p/br/style/iframe)` + `createElementNS(svg/path/line/circle/g)`(渲染 SVG widget)、`attachShadow`+`querySelector`;
指纹:`navigator.{userAgent,language,maxTouchPoints,hardwareConcurrency,gpu.requestAdapter}`、`RTCPeerConnection`、`speechSynthesis.getVoices`、`performance.{now,getEntries,timing}`、`PerformanceObserver`;
反篡改:`iframe.contentWindow.eval`、`Function.prototype.toString`=`[native code]`;
行为:`addEventListener(keydown/pointermove/pointerover/touchstart/mousemove/wheel/click)`;
算力:`Worker`+`URL.createObjectURL`+`Blob`、`BigInt`、`crypto.getRandomValues`。

## 混合方案落地:浏览器只铸 token + 纯协议跑 verify/signin(✅ 端到端坐实,2026-06-25)

> 既然 L3「零浏览器纯算 token」结构性不可行,就把浏览器压缩成**只干一件事:过盾铸 Turnstile token**,
> 其余(cf_clearance / session / csrf / verify-turnstile / signin/email)全走 **curl_cffi 纯协议**。
> **实测整条链路打通,token 可跨客户端搬运。**

### 决定性侦察(`cf-protocol-poc/jsd/signin_probe.py`)
- 纯协议 `cf_clearance`(`jsd_trust.establish_clearance`:golden 指纹 + `lzcodec` 字节级编码 POST `jsd/oneshot`)**可稳定拿到**(`replay_*.json`/`trusted_cookies.json` 多次复现)。
- **但 `signin/email` 是服务端硬门**:仅带 `cf_clearance`+csrf → `403 {"error":"Turnstile verification required"}`;`verify-turnstile token=""` → `400 {"error":"Invalid auth provider"}`。⇒ 必须有**服务端验过的 Turnstile token**,token 只能真浏览器铸 ⇒ **全程零浏览器不可行,混合是唯一务实解**。

### 真实请求形状(`cf-protocol-poc/recon/verify_shape.json`,capture 模式抓全)
- `POST /api/auth/verify-turnstile` body = `{"token","provider":"email","email","callbackUrl","fallbackReason":null}`
  —— 之前裸 `{token}` 报 `Invalid auth provider` 就是**缺 `provider`/`email`/`callbackUrl`**(并非 token 无效)。
- verify 成功 → 服务端**回种 `__turnstile_auth`**(HttpOnly JWT:`{v,mode:"verified",nonce,action:"auth_signin",provider:"email",emailHash,callbackUrlHash,exp}`)= 服务端验证凭证,`signin/email` 靠它放行。
- `POST /api/auth/signin/email` 表单 = `email&redirect=false&callbackUrl&csrfToken&json=true` → `200 {"url":".../verify-request?provider=email&type=email"}` = magic-link 发出。

### 工具(本仓)
- 浏览器侧 `examples/cloudflare/cf_turnstile_hybrid.rs`(cdp 干净 Chrome):
  - `HYBRID_MODE=mint`(默认):铸**未消费**新鲜 token(不点 Continue)+ `tab.cookies()` 导全 cookie(含 HttpOnly `cf_clearance`)→ `recon/fresh_token.json`。
  - `HYBRID_MODE=capture`:点 Continue 走真实流程,抓全 verify/signin 请求体+响应 → `recon/verify_shape.json`(学形状)。
- 协议侧 `cf-protocol-poc/jsd/hybrid_replay.py`(curl_cffi Chrome JA3):注入导出的 cookie → GET csrf → POST verify-turnstile(带 provider 上下文)→ POST signin/email。

### 实测结果(同出口 IP,token 铸于 0s 前)
```
[浏览器 mint] 1s 出 token(880B)+ 4 cookie(cf_clearance=true),未消费
[curl_cffi ] cf_clearance injected:True → GET csrf 200 →
             POST verify-turnstile → {"success":true,"mode":"verified"}(__turnstile_auth 已种)→
             POST signin/email     → 200 {"url":".../verify-request?provider=email"} = magic-link 发出
✅ 浏览器只铸 token,纯协议跑完 verify+signin
```
- **关键事实**:浏览器铸的 token 拿到**完全独立的 curl_cffi 客户端**(同机=同出口 IP)调 `verify-turnstile` **被服务端接受**;cookie 里 `cf_clearance` 负责过 CF 边缘。token 有时效(Cloudflare siteverify 默认 ~300s),mint 后尽快 replay。

## 进度 / 下一步
- [x] 通用能力 `attach_oopifs`/`wait_oopif` + 侦察 example,实测进入 Turnstile OOPIF、摸清结构。
- [x] **dump** 28 脚本落盘(`target/cf-dump/`):entry 240KB + 22 份各 1.3MB **多态 VM**(字节全不同)+ network。
- [x] **端到端基线彻底坐实**:浏览器辅助出 token + `verify-turnstile` `success:true` 验真(全库能力)。
- [x] 扣 `entry.js` 字符串数组解扰器(obfuscator.io,`x(o)=F()[o-106]`)→ `entry.deobf.js` 352KB 可读。
- [x] 摸清 VM 架构(`runProgram`=`x1(new x0(J),0,101,[])`、流式加密字节码、TEA、RSA)。
- [x] **路线 B**:补环境 Node 跑通 entry.js + 全解码握手/反爬门/数据模型 + 摸清环境读取清单 + 定位纯离线边界(见上)。
- [x] **路线 2 de-risk(Node 替浏览器跑 VM)**:① 给 `iframe.contentWindow` 真·独立 `vm` 上下文(真原生 `eval`,`run.js` 的 `makeIframeWindow`)后,**多态 VM 不再死循环**——坐实之前的「死循环」是 VM 在桩 iframe-eval 垃圾值上空转,**非无限 PoW**,VM 能确定性推进。② 随即崩在 VM 寄存器链 `x0.FV@8208 n[oZ]=n[E][n[oA]][n[oW]]` 的 `undefined.prototype`(某寄存器被前序 opcode 写成 undefined = 某浏览器 API 探测返回非真实值);补 `RTCPeerConnection`/`navigator.gpu` 后仍崩同处,`WINSTUB`(未知 window 全局→autoStub)也无效 = undefined 来自寄存器非直接 window 读。③ **本质墙**:多态 VM 探测**整个真实浏览器 API 表面**真实性(WebGPU/canvas/webgl/audio/原生函数身份/DOM 行为 + `iframe.contentWindow.eval` 反 hook),定位每个错值需 VM 寄存器级追踪,补一个又在下个探测崩 → **等价于在 Node 复刻整个浏览器**(= CF 用多态 VM 强制真浏览器的设计目的)。**结论:L3 Turnstile「Node-only 跑 VM 出 token」实践上不可行**(非补一两个值);务实仍是真浏览器 e2e 基线。
- [x] **混合方案端到端坐实**:浏览器只铸 token(`cf_turnstile_hybrid.rs` mint,未消费)→ curl_cffi 纯协议 verify-turnstile+signin/email 全 200、magic-link 发出(token 可跨客户端搬运);并定位 signin 服务端硬门 + 真实请求形状 + `__turnstile_auth` 凭证机制。
- [ ] (可选)把混合方案沉淀成库能力:`mint_turnstile_token()`(浏览器铸 token+导 cookie)+ Session(impersonate)接力 verify/signin,做成一个 Rust example,免依赖外部 Python/curl_cffi。
- [ ] (可选)把 `attach_oopifs`+dump+deobf+decode 这套「进跨域 iframe 逆向 + obfuscator.io 解混淆」沉淀为库 example/文档。
