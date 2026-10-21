# 示例

从这里开始：

```bash
cargo run --example own_site
```

`own_site` 先在本地页填写并核对结果，再打开 [apizero.cn](https://apizero.cn/) 点进文档。测试你自己的网站时，把示例里的地址换成测试环境。说明见仓库根目录的 README。

其他常用示例：

```bash
cargo run --example cdp_demo
cargo run --example cdp_agent
cargo run --example quickstart --no-default-features --features camoufox
cargo run --example slider_local --no-default-features --features camoufox,slider
```

Camoufox 示例要加 `--no-default-features --features camoufox`。默认 feature 是 `cdp`，两套同时开时类型会按 CDP 解析。

## 自己的页面

| 示例 | 做什么 |
|---|---|
| [own_site](cdp/own_site.rs) | 本地页填名称并保存；再打开 apizero.cn，点进公开文档 |
| [self_check.html](pages/self_check.html) | `own_site` 用的本地页 |

## CDP（默认）

| 示例 | 做什么 |
|---|---|
| [cdp_demo](cdp/cdp_demo.rs) | 打开 example.com，读标题、跑一段 JS、截图 |
| [cdp_agent](cdp/cdp_agent.rs) | 本地页上的语义点击、等待、弹窗 |
| [cdp_context](cdp/cdp_context.rs) | 两个 Context 的 Cookie 隔离、磁盘 Profile |
| [cdp_extras](cdp/cdp_extras.rs) | PDF、HAR、存储、设备尺寸等，本地页 |
| [cdp_recorder](cdp/cdp_recorder.rs) | 录操作，生成 Rust；无障碍树 |
| [cdp_keyboard](cdp/cdp_keyboard.rs) | 本地页键盘输入 |
| [cdp_download](cdp/cdp_download.rs) | 本地下载 |
| [cdp_pool](cdp/cdp_pool.rs) | 多个浏览器任务，本地页 |
| [cdp_fetch](cdp/cdp_fetch.rs) | 按需下载 Chrome for Testing，打开 example.com |
| [cdp_advanced](cdp/cdp_advanced.rs) | 本地 HTTP 服务上的 CDP 能力 |
| [lightpanda_demo](cdp/lightpanda_demo.rs) | 同一套 API，换 Lightpanda，打开 example.com |
| [learn_basics](learn/learn_basics.rs) | 本地页上边用边学 Rust |

## 调试自己的前端

这些示例默认打 `example.com` 或本地页，用来看自己的请求和脚本。

| 示例 | 做什么 |
|---|---|
| [re_xhr_break](reverse/re_xhr_break.rs) | 在自己页面的 fetch 处下断点 |
| [re_scripts](reverse/re_scripts.rs) | 读当前页脚本 |
| [re_replay](reverse/re_replay.rs) | 把监听到的请求再发出去 |
| [re_probe](reverse/re_probe.rs) | 对你指定的、你有权调试的 URL 做一次探查 |

## 本地页上的其它能力

| 示例 | 做什么 | 需要 |
|---|---|---|
| [slider_local](slider/slider_local.rs) | 本地合成页上的缺口对齐 | camoufox, slider |
| [quickstart](camoufox/quickstart.rs) | Camoufox 打开 example.com | camoufox |
| [form_fill](camoufox/form_fill.rs) | 填写仓库里的本地表单 | camoufox |
| [pool_crawl](pool/pool_crawl.rs) | 本地页上的并发池 | camoufox |
| [session_mode](session/session_mode.rs) | 浏览器和 HTTP 会话共用 Cookie，目标是 example.com | camoufox |
| [web_page_scrape](session/web_page_scrape.rs) | 本地表格导出 | camoufox |

`examples/camoufox/`、`examples/network/`、`examples/windows/` 里其余文件是同一类：本地页或 example.com 上的点击、监听、截图。Windows 冒烟见 `win_smoke` / `win_diag`。
