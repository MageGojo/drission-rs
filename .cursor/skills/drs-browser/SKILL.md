---
name: drs-browser
description: Mandatory browser and live web content gateway via the local `drs` CLI or `drs` MCP server. Use whenever opening pages, extracting latest web content, screenshots, accessibility trees, network captures, Cloudflare pages, or any browser automation — never use Playwright, Puppeteer, Selenium, WebFetch, or raw curl for interactive pages.
---

# drs 浏览器网关（强制）

## 铁律

1. **所有浏览器操作和内容提取必须走 `drs`**：`drs --json ...` CLI，或 Cursor MCP 工具 `browser_*` / `network_*`（server 名 `drs`）。
2. **禁止绕过**：不得用 Playwright / Puppeteer / Selenium / chromedriver / WebFetch / curl 直接抓需要 JS 渲染、登录态、反爬或 Cloudflare 的页面。
3. **机器读取一律 `--json`**：解析 `{ "ok": true, "data": ... }` 或 `{ "ok": false, "error": ... }`。
4. **先确保 daemon**：任意 daemon 命令前执行 `drs ensure-serve --headless`，或给命令加 `--ensure-serve --ensure-headless`。
5. **内容先落盘**：抓到的页面 bundle / 网络包 / 截图先写入项目 `data/browser/`（JSON/PNG），再分析。

## 首选：一条命令提取页面

```bash
drs ensure-serve --headless
drs --json extract https://example.com \
  --save-out data/browser/example.json
```

`extract` 返回 `title`、`url`、`text`、`outline`；需要时用 `--include-html`、`--include-ax-json`、`--pass-cf`、`--wait-selector`、`--screenshot-out`。

## 常用 CLI（均建议 `--ensure-serve --ensure-headless --json`）

| 任务 | 命令 |
|---|---|
| 状态 | `drs --json status` |
| 打开页 | `drs --json open URL` |
| 读语义树 | `drs --json ax --json` 或 `drs ax --outline` |
| 读正文 | `drs text` 或 `drs text "css:h1"` |
| 读标题/URL | `drs --json title` / `drs --json url` |
| 截图 | `drs screenshot --out data/browser/page.png --full` |
| 监听 XHR | `drs --json listen start /api/ --xhr-only` → `listen wait` |
| 过 CF | `drs --json pass-cf --timeout-ms 30000` |
| 停止 | `drs stop` |

## MCP（Cursor 内）

项目已配置 `.cursor/mcp.json`，server 名 **`drs`**。优先工具：

- **`browser_extract`** — 打开 URL 并返回 title/url/text/outline（首选）
- `browser_open` / `browser_ax` / `browser_text` / `browser_html` / `browser_screenshot`
- `network_listen_start` / `network_listen_wait`
- `browser_pass_cf`

MCP 默认 **attach 到常驻 daemon 的同一个持久浏览器**：浏览器活在 `drs serve` 进程里，MCP server 重启也不丢标签/登录态，CLI 与 MCP 操作同一组标签。daemon 用固定 profile（`<cache>/drission/cli/profile`），登录态跨重启存活——这正是「下次查数据直接接着用、不用重登」的关键。想要一次性独立浏览器时用 `drs mcp --standalone`。

## 决策树

```
需要浏览器或动态页面内容？
├─ 是 → 只用 drs（CLI 或 MCP）
│   ├─ 一次性读页面 → browser_extract / drs extract
│   ├─ 多步交互 → ensure-serve + open/click/type/wait
│   └─ 抓 API → listen start + wait
└─ 否（静态 JSON/API，无反爬）→ 可用普通 HTTP 客户端
```

## 安装 / 开发

免 Rust 装预编译二进制(推荐给「只用不写」的场景):

```bash
curl -fsSL https://raw.githubusercontent.com/MageGojo/drission-rs/main/install/drs-install.sh | sh   # mac/linux
drs setup    # 自动写 Cursor(.cursor/mcp.json)+ Codex(~/.codex/config.toml)的 MCP 配置
```

从源码装(要 Rust):

```bash
cargo install --path crates/drission-cli --bin drs
# 或
cargo install drission-cli --bin drs --features cdp,ocr
```

完整命令、`drs setup` 与 identity 治理见 [docs/CLI.md](../../docs/CLI.md)。
