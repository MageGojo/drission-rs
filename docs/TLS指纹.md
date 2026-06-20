# Session 模式 TLS/JA3/JA4 + HTTP2 指纹伪装(里程碑 63)

> 恢复上下文顺序:`docs/需求.md` → `docs/设计.md` → `docs/进度.md` → 本文件。

## 一句话目标

给 **Session(HTTP)模式**(`SessionPage`)加上**浏览器级 TLS / JA3 / JA4 + HTTP2 指纹**,
让"**Driver 过盾 → Session 接力**"双模真正成立——浏览器辛苦过的盾,后续纯 HTTP 抓取不再因
**Rust 默认 TLS 指纹**被现代 WAF(Akamai / Cloudflare / DataDome)一眼拦下。

## 背景 / 痛点

- 现状 `src/session/mod.rs` 走原生 `reqwest`(rustls),其 **ClientHello / JA3 / JA4 / HTTP2 SETTINGS**
  是固定的 Rust 指纹,与任何真实浏览器都不一致。
- 现代反爬第一道常是 **TLS 指纹**(不看 UA、不看 cookie 就能判"非浏览器")。
- 因此本库引以为傲的双模有个洞:**浏览器过盾拿到 cookie 灌进 Session 后,HTTP 一请求又露馅**。
- 这是**网络层的"补环境"**,与 canvas/webgl/audio 的"吐环境回放"一脉相承。

## 选型(经调研)

| 方案 | 结论 |
|---|---|
| **`wreq` + `wreq-util`**(✅ 选用) | reqwest 硬分叉,API 几乎一致;基于 **BoringSSL** 精细控制 TLS/HTTP2 扩展;`wreq-util` 内置 **100+ 浏览器模拟档**(`Emulation::Chrome137/Firefox139/Safari18_5/Edge134`…);2026 年仍活跃维护、1.1M 下载、789 依赖、MSRV 1.85 + edition 2024(与本库一致)、Apache-2.0;支持 **HTTP/1 头大小写保真**(部分 WAF 据此拒纯小写头) |
| `rquest` | 同类(BoringSSL),但最新稳定版被 yank、近一年更新慢于 wreq |
| `impcurl` / curl-impersonate | 需**运行时挂 `libcurl-impersonate` 动态库**(FFI + 自动下载 .so/.dylib),违背本库**纯 Rust + 干净跨平台**原则 |
| 自己基于 boring 拼 ClientHello | 维护成本高、易过时;wreq 已把"按浏览器版本对齐 TLS+HTTP2"做成可维护档 |

**版本取舍:用稳定线 `wreq = "5"` + `wreq-util = "2"`(而非 `6.0.0-rc`/`3.0.0-rc`)** —— 本库已发布到
crates.io,依赖预发布(rc)会让下游 `cargo build` 被迫开启 prerelease 解析、且 rc 易破坏 API。
模拟档版本号(Chrome137 等)集中在一处 `profile_to_emulation` 映射,后续随 wreq-util 升级一键上调。

## 架构(非破坏 + 单一代码路径)

### feature 门控(默认关,沿用 slider/ocr/signer 哲学)
- 新增 **`impersonate`** feature,默认**关**;开启才引入 `wreq` + `wreq-util`(BoringSSL 重依赖)。
- `impersonate = ["camoufox", "dep:wreq", "dep:wreq-util"]` —— 因 `SessionPage` 当前在 `camoufox` 门下,
  故 imply camoufox(与 `slider=["camoufox"]` 同理)。**默认构建(纯 CDP)零成本、不引 BoringSSL**。
- **未来**:把 `SessionPage`/`SessionOptions` 从 camoufox 解耦为后端无关,可让纯 CDP / 无浏览器用户也用上
  指纹 HTTP 客户端(本里程碑先聚焦 Session,范围收敛)。

### enum 后端 + 急取 RawResponse(关键)
为**只保留一份**重定向/cookie 逻辑(易维护、不双写),把 HTTP 客户端抽象为:

```text
enum HttpBackend {
    Plain(reqwest::Client),                 // 始终可用(默认行为,零新依赖)
    #[cfg(feature = "impersonate")]
    Impersonate(wreq::Client),              // profile != None 时
}
struct RawResponse { status: u16, headers: Vec<(String,String)>, body: String }
```

- `SessionPage::request()` 的重定向/cookie 循环**完全后端无关**:每跳把"请求头列表 + 可选体"交给
  `backend.send_once(...)`,拿回 `RawResponse`(状态 + **全部头含重复 set-cookie** + 正文)再走既有逻辑。
- **取舍**:`send_once` **每跳急取正文**(原 reqwest 实现只在终点读 body)。重定向跳的 body 通常很小,
  双模抓取场景代价可忽略;换来"两后端共用一条循环"的可维护性。
- cookie jar 仍用 `reqwest::Url`(就是 `url::Url`)解析,URL 以 `.as_str()` 传给任一后端(不引 wreq 的 URL 类型)。

### UA / 默认头策略
- **plain 后端**:UA 走 `reqwest` client 的 `.user_agent(opts.user_agent)`(行为不变)。
- **impersonate 后端**:**UA + sec-ch-ua + accept 等默认头由模拟档驱动**;`opts.user_agent` 被忽略
  (避免与 TLS 指纹"打架";如某 Chrome 档自带 Chrome UA,却塞个 Firefox UA 反而更可疑)。
- `opts.headers`(用户额外头)对**两后端**都改为**每请求附加**(而非 client 默认头),避免覆盖模拟档的头集。
- 证书校验:plain 用 `danger_accept_invalid_certs(true)`;impersonate 用 wreq 的 **`cert_verification(false)`**(注意改名)。
- 代理:两后端都支持 `Proxy::all(server)` + `basic_auth(u,p)`(wreq `proxy<P: IntoProxy>`)。

## 对外 API(后端无关、feature 开关不改变 API 形状)

```rust
pub enum BrowserProfile { None, Chrome, Firefox, Safari, Edge }   // 始终存在(camoufox 下)

SessionOptions::new().profile(BrowserProfile::Chrome)   // 开浏览器 TLS 指纹
```

- `BrowserProfile` 与 `.profile()` builder 方法**始终编译**(不随 impersonate 变),用户代码**切 feature 不改一行**。
- 若设了 `profile != None` 但**没编 `impersonate`**:运行期 `tracing::warn!` 一次并**优雅回退**纯 reqwest
  (不报错、不 panic;符合本库"非测试代码不 panic"原则)。
- 默认 `BrowserProfile::None` = 现有纯 reqwest 行为,**完全不回归**。

## 接线(改动文件)
- `Cargo.toml`:加可选 `wreq`/`wreq-util` + `impersonate` feature + 示例 `session_tls` 的 `required-features`。
- `src/error.rs`:加 feature 门控变体 `#[from] wreq::Error`(`?` 直通)。
- `src/session/http.rs`(**新**,组件化):`HttpBackend` / `RawResponse` / `BrowserProfile` / `profile_to_emulation` / `build_*_client` / `send_once`。
- `src/session/mod.rs`:`SessionOptions` 加 `profile`;`SessionPage` 字段换 `backend`+`extra_headers`;`new()`/`request()` 改走后端抽象。
- `src/lib.rs` prelude:导出 `BrowserProfile`(camoufox 门下,与 `SessionOptions`/`SessionPage` 同组)。
- 文档:README 中英 feature 表 + CHANGELOG + 本文件 + `docs/进度.md` 里程碑。

## 验证计划(对齐本库"自验证"惯例)
1. **构建矩阵**:默认(纯 cdp,**不含 BoringSSL**)、`camoufox`、`camoufox,impersonate`、`--no-default-features`、
   `all`;**`x86_64-pc-windows-gnu` 交叉编译 ✅ 已实测**(BoringSSL+mingw+nasm 编译、bindgen 喂 sysroot 后整树通过、
   `session_tls.exe` 链接成真 PE32+ 二进制,见上"Windows 构建")。
2. **clippy + fmt**:各 backend 干净。
3. **单测**:`profile_to_emulation` 映射、`BrowserProfile` 默认值、内容类型/头构建纯函数。
4. **JA3 实测对比**(示例 `session_tls`,需联网):同一进程分别用 `None` 与 `Chrome` 打 `https://tls.peet.ws/api/all`,
   打印两者 **JA3 / JA4 / Akamai 指纹**——证 impersonate 后指纹变为浏览器形态(与纯 reqwest 明显不同)。
5. **不回归**:`session_mode` 示例(默认无 impersonate)行为不变。

## Windows 构建(已验证)

`impersonate` 用 BoringSSL,需汇编器 **nasm**(x86_64 优化汇编)+ `cmake` + C/C++ 编译器。**默认构建不含 BoringSSL**,不受影响。

- **Windows 原生(MSVC,推荐)**:装 VS Build Tools(cmake + MSVC)+ `nasm` 后 `cargo build --features impersonate` 即可——
  BoringSSL 对 MSVC 是一等公民,原生头文件齐全,无 bindgen sysroot 问题。
- **Windows 原生(GNU/mingw)**:装 mingw-w64 + `nasm` 即可。
- **从 macOS / Linux 交叉编译 → `x86_64-pc-windows-gnu`(本库 CI 约定)**:✅ **已实测跑通**(产出真 `PE32+ .exe`)。
  唯一坑:boring-sys2 的 build.rs 跑 bindgen 解析 BoringSSL 头时,clang 不知道 mingw sysroot → `fatal error: 'sys/types.h' file not found`。
  **修法**:把 mingw sysroot 喂给 bindgen(`BINDGEN_EXTRA_CLANG_ARGS`)+ 指定 mingw 链接器——已封装为 **`scripts/win-cross-build.sh`**:
  ```bash
  # mac: brew install mingw-w64 nasm cmake ; rustup target add x86_64-pc-windows-gnu
  scripts/win-cross-build.sh build --features impersonate --example session_tls
  # 产出 target/x86_64-pc-windows-gnu/debug/examples/session_tls.exe(PE32+ x86-64)
  ```
  脚本动态探测 mingw sysroot(`gcc -print-sysroot`),**只为交叉编译临时设环境变量,不写进 `.cargo/config.toml`**
  (否则会无条件污染本机原生构建的 bindgen)。BoringSSL 的 C/汇编本身用 mingw + nasm 直接编,无需改任何代码。
- **MSVC 交叉(从 mac/Linux)**:可用 `cargo-xwin`(`brew install llvm` + `cargo install cargo-xwin` + nasm),
  `cargo xwin build --target x86_64-pc-windows-msvc --features impersonate`;属可选路径,本库 CI 走更轻的 gnu 交叉。

## 已知边界
- **不是银弹**:TLS 指纹只是第一层;Akamai/DataDome 等还要 sensor/token(见路线图"更多反爬盾 preset")。
- **Session 仍 camoufox 门下**:解耦为后端无关属后续。
