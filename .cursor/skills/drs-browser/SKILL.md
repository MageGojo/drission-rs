---
name: drs-browser
description: Mandatory browser and live web content gateway via the local `drs` CLI or `drs` MCP server. Use whenever opening pages, extracting latest web content, screenshots, accessibility trees, network captures, Cloudflare pages, or any browser automation — never use Playwright, Puppeteer, Selenium, WebFetch, or raw curl for interactive pages.
---

# drs 浏览器网关（强制）

## 铁律

1. **所有浏览器操作和内容提取必须走 `drs`**：`drs --json ...` CLI，或 Cursor MCP 工具 `browser_*` / `network_*`。
2. **禁止绕过**：不得用 Playwright / Puppeteer / Selenium / chromedriver / WebFetch / curl 直接抓需要 JS 渲染、登录态、反爬或 Cloudflare 的页面。
3. **机器读取一律 `--json`**：解析 `{ "ok": true, "data": ... }` 或 `{ "ok": false, "error": ... }`。
4. **先确保 daemon**：任意 daemon 命令前执行 `drs ensure-serve --headless`，或给命令加 `--ensure-serve --ensure-headless`。
5. **内容先落盘**：抓到的页面 bundle / 网络包 / 截图先写入项目 `data/browser/`（JSON/PNG），再分析。

## MCP server 名（统一）

| 位置 | 名字 |
|---|---|
| `~/.cursor/mcp.json` / 项目 `.cursor/mcp.json` 的 key | **`drs`** |
| Cursor Agent 里调用 MCP 时的 server id | **`user-drs`**（Cursor 给用户级配置加的 `user-` 前缀） |
| Codex `~/.codex/config.toml` | **`drs`** |

调用时用 **`user-drs`**（Cursor）。Skill / 文档里写的 `drs` 指配置 key，不是 CallMcpTool 的 server 参数。

## Schema 复用（少打 GetMcpTools）

核心工具 schema **会话内稳定**。推荐：

1. 会话里对 `user-drs` **最多完整发现一次**：`GetMcpTools({ server: "user-drs" })`，或按需 `GetMcpTools({ server: "user-drs", toolName: "browser_click" })`。
2. 之后对已知核心工具直接 `CallMcpTool`，不必每次重拉全量 catalog（全量含 identity 时很重；默认 toolset=`core` 已缩到 ~11 个）。
3. 默认只暴露浏览核心；需要网络监听 / CF / identity 资产时，让用户把 `DRS_MCP_TOOLS` 设为 `browser` / `identity` / `all` 后重启 MCP。

稳定核心参数（可按此直接 call）：

- `browser_open` `{url}`
- `browser_snapshot` `{budget_ms?, max_items?, max_text_chars?, max_markdown_chars?}`
- `browser_extract` `{url?, wait_selector?, pass_cf?, include_markdown?}`
- `browser_click` `{ref?|selector?, return_snapshot?}`
- `browser_type` `{ref?|selector?, text, return_snapshot?}`
- `browser_press` `{key, selector?, return_snapshot?}`
- `browser_wait` `{selector, timeout_ms?, return_snapshot?}`
- `browser_screenshot` `{out?, full?, inline?}`
- `browser_tabs` / `browser_use_tab` `{tab_id}` / `browser_close` `{tab_id?}`

多步交互优先 `return_snapshot: true`，避免再单独打一轮 snapshot。

错误码 `stale_ref`：旧 ref 失效 → **立刻再 `browser_snapshot`，用新 ref 重试**。

## 首选：读懂当前页

已有打开的标签时，用 **`browser_snapshot` / `drs snapshot`**（不要先拉整页 HTML）：

```bash
drs ensure-serve --headless
drs --json open https://example.com
drs --json snapshot
```

返回 `title` / `url` / `snapshot`（可交互节点带 `[ref=e1]`）/ `refs` / **`markdown`(htmd)** / 短正文。随后点击/输入：

```bash
drs --json click "ref:e1"
drs --json type "ref:e2" "hello"
```

MCP 等价（server=`user-drs`）：

`browser_snapshot` → `browser_click({ ref: "e1", return_snapshot: true })` / `browser_type({ ref: "e2", text: "hello", return_snapshot: true })`。

一次性打开并抽取仍可用 `extract`：

```bash
drs --json extract https://example.com \
  --save-out data/browser/example.json
```

重页/反自动化页优先 `snapshot`，避免 `browser_html` / 完整 `ax` 卡死 MCP（超时错误码 `timeout`）。

## 常用 CLI（均建议 `--ensure-serve --ensure-headless --json`）

| 任务 | 命令 |
|---|---|
| 状态 | `drs --json status` |
| 打开页 | `drs --json open URL` |
| **读懂当前页** | **`drs --json snapshot`**（首选） |
| 读语义树 | `drs --json ax --json` 或 `drs ax --outline` |
| 读正文 | `drs text` 或 `drs text "css:h1"` |
| 读标题/URL | `drs --json title` / `drs --json url` |
| 截图 | `drs screenshot --out data/browser/page.png --full` |
| 监听 XHR | `drs --json listen start /api/ --xhr-only` → `listen wait` |
| 过 CF | `drs --json pass-cf --timeout-ms 30000` |
| 停止 | `drs stop` |

## MCP（Cursor 内）

配置 key **`drs`**，Agent 调用 id **`user-drs`**。默认 toolset=`core`：

- **`browser_snapshot`** — 读懂当前页（首选）
- **`browser_extract`** — 打开 URL 并返回 title/url/text/outline
- `browser_open` / `browser_tabs` / `browser_use_tab` / `browser_close`
- `browser_click` / `browser_type` / `browser_press` / `browser_wait`（可 `return_snapshot`）
- `browser_screenshot`

`DRS_MCP_TOOLS=browser` 才有 `network_*` / `browser_pass_cf` / `browser_html` 等；`=identity` 才有 `identity_assets_*`；`=all` 全开。

MCP 默认 **attach 到常驻 daemon**：标签/登录态跨 MCP 重启存活。一次性独立浏览器用 `drs mcp --standalone`。

## 决策树

```
需要浏览器或动态页面内容？
├─ 是 → 只用 drs（CLI 或 MCP user-drs）
│   ├─ 看当前页有什么 → browser_snapshot（首选）
│   ├─ 一次性打开+读 → browser_extract
│   ├─ 多步交互 → click/type/press/wait + return_snapshot=true
│   └─ 抓 API → DRS_MCP_TOOLS=browser 后 listen start + wait
└─ 否（静态 JSON/API，无反爬）→ 可用普通 HTTP 客户端
```

## 安装 / 开发

```bash
curl -fsSL https://raw.githubusercontent.com/MageGojo/drission-rs/main/install/drs-install.sh | sh
drs setup --scope global   # 写 ~/.cursor/mcp.json（key=drs，env DRS_MCP_TOOLS=core）
```

从源码：

```bash
cargo install --path crates/drission-cli --bin drs
```

完整命令见 [docs/CLI.md](../../docs/CLI.md)。
