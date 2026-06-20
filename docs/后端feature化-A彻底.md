# 后端 feature 化 · 方案 A(彻底):`default = ["cdp"]`,Camoufox 全 opt-in

> 决策来源:用户选「A(彻底)」。把里程碑 46 的默认后端**反过来**——默认纯 CDP,Camoufox
> 改为真正 gate 的可选后端;高层能力随 `camoufox` feature 走;共享类型抽出到始终编译位置,
> 让**纯 CDP 能独立编译**(不含任何 camoufox 代码)。

## 目标

1. `default = ["cdp"]`:默认构建 = 纯 Chromium/CDP 后端,最精简。
2. `camoufox` feature **真正门控**:`browser` 及其全部高层能力(Page/WebPage/Session/Pool/
   launcher/吐环境/过盾/滑块…)只有 `--features camoufox` 才编译。
3. 纯 CDP 构建**独立编译**:不依赖 `browser`/`launcher` 等 camoufox 模块。
4. CDP 版 Page/WebPage 留到下个版本(本次纯 CDP 仍是 `ChromiumBrowser`/`ChromiumTab` 这套)。

## 共享类型抽取(始终编译)

CDP 与 Camoufox 后端共用一批**后端无关**的数据类型。此前它们住在 `browser` 子模块里
(`browser::keys`、`browser::listener`、`browser::interceptor`),`browser` 一旦被 gate,
纯 CDP 就拿不到。故抽到两个始终编译的顶层模块:

| 新位置(始终编译) | 内容 | 原位置 |
|---|---|---|
| `crate::keys` | `Keys`、`KeyInput` | `browser::keys` |
| `crate::net` | `DataPacket`、`RequestData`、`ResponseData`、`ListenFilter`、`ResumeOptions` | `browser::listener` / `browser::interceptor` |

**向后兼容**:`browser::listener` / `browser::interceptor` / `browser::mod` 改为
`pub use crate::net::{…}` / `pub use crate::keys::{…}` **再导出**,故
`crate::browser::DataPacket` / `crate::browser::Keys` 等老路径在开了 camoufox 时仍可用。

`ListenFilter::matches` 仍是 `pub(crate)`,移到 `net` 后**全 crate 可见**,CDP 的
listener/interceptor 照常调用。camoufox 专有的 hook 脚本 / `parse_packets` / `parse_headers` /
`InterceptedRequest`(持 `Connection`)/ `ListenBuffer` 仍留在 `browser`。

## 模块 gate 矩阵

| 模块 | gate | 说明 |
|---|---|---|
| `codec` `error` `locator` `protocol` `transport` `util` | 始终 | 后端无关基础设施 |
| `keys` `net` | 始终 | **新增**,共享类型 |
| `scrape` | 始终 | 纯 std 采集导出(CSV/JSON),CDP 也可用 |
| `cdp` | `#[cfg(feature = "cdp")]` | Chromium 后端 |
| `browser` | `#[cfg(feature = "camoufox")]` | Camoufox/Juggler 后端 + 大量能力 |
| `launcher` | `#[cfg(feature = "camoufox")]` | Camoufox 选项/下载/spawn |
| `page` `web_page` `session` `pool` | `#[cfg(feature = "camoufox")]` | 高层门面,依赖 `browser`/`launcher` |
| `ocr` | `#[cfg(feature = "ocr")]` | `Ocr` 识别器后端无关;`Tab::ocr_image` 便捷 `#[cfg(camoufox)]` |

`transport`:`ws_connect` 被 CDP 与 camoufox 共用(始终编译);fd3/4 管道相关项是
`pub use` 公共 API,纯 CDP 下休眠但不产生 dead-code 警告。CDP 自己用 `tokio::process::Child`,
不碰 `transport::Child`。

## Cargo features

```toml
default = ["cdp"]
cdp = []
camoufox = []
slider = ["camoufox"]          # 滑块基于 camoufox Tab,故 imply camoufox
ocr = ["dep:image", "dep:tract-onnx"]   # Ocr 识别器后端无关
signer = ["dep:rquickjs"]      # QuickJS 纯算,后端无关
```

## prelude 分区

- **始终可用**:`Error`/`Result`、`Query`/`parse_locator`、`Keys`/`KeyInput`(`crate::keys`)、
  `DataPacket`/`RequestData`/`ResponseData`/`ListenFilter`/`ResumeOptions`(`crate::net`)、
  scrape 导出函数。
- `#[cfg(camoufox)]`:`Browser`/`Tab`/`Element`/…、`BrowserOptions` 等 launcher 类型、
  `Page`、`WebPage`/`PageMode`、`SessionPage`/…、`BrowserPool`/…。
- `#[cfg(slider)]`(imply camoufox):`SliderConfig`/`ImageSource`/…。
- `#[cfg(cdp)]`:`ChromiumBrowser`/`ChromiumTab`/…。
- `#[cfg(ocr)]`:`Ocr`。

## examples 影响

默认翻成 cdp 后,`cargo build --examples`(默认)只编 feature 无关 + cdp 示例;**所有用
Camoufox `Browser`/`Page` 的示例必须加 `required-features`**,否则默认构建编译失败。归类:

- `cdp_demo` / `cdp_advanced` → `["cdp"]`
- `env_signer` → `["signer"]`(QuickJS,不用浏览器)
- `geetest_slide` / `dx_slide` / `slider_local` → `["slider"]`(imply camoufox)
- `ocr_captcha` / `apizero_login` → `["camoufox", "ocr"]`(用 `tab.ocr_image`)
- 其余约 41 个 → `["camoufox"]`

`bench parsing` 只用 `codec`/`locator`/`scrape`(始终编译),纯 CDP 下也能跑,无需 required-features。

## 验证矩阵

| 构建 | 期望 |
|---|---|
| `cargo build`(默认 cdp) | 纯 CDP lib,无 camoufox 代码 |
| `cargo build --no-default-features`(零 feature) | lib 仅基础设施 + 共享类型 + scrape |
| `cargo test --no-default-features`(零 feature) | net/keys/codec/locator/scrape 等后端无关单测 |
| `cargo test --lib --features camoufox` | camoufox 全量单测 |
| `cargo test --lib --features camoufox,cdp,slider` | 两后端 + 滑块 |
| `cargo build --all-targets`(默认 cdp) | cdp 示例 + bench |
| `cargo build --examples --features camoufox,slider,ocr,signer` | camoufox 全部示例 |
| `cargo clippy`(默认 cdp / `--features camoufox`) | 新代码零警告 |
