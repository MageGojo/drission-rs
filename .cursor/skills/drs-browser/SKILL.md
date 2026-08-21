---
name: drs-browser
description: Mandatory browser and live web content gateway via the local `drs` CLI or `drs` MCP server. Use whenever opening pages, extracting latest web content, screenshots, accessibility trees, network captures, Cloudflare pages, or any browser automation — never use Playwright, Puppeteer, Selenium, WebFetch, or raw curl for interactive pages.
---

# drs 浏览器网关（强制）

产品命令和语义 MCP 的对照、可复制参数见仓库 [`docs/产品级CLI.md`](../../docs/产品级CLI.md)。完整字段见 [`docs/CLI.md`](../../docs/CLI.md)。

## 铁律

1. **所有浏览器操作必须走 `drs`**：`drs --json ...`，或 Cursor MCP `browser_*`（server=`user-drs`）。
2. **禁止绕过**：不得用 Playwright / Puppeteer / Selenium / chromedriver / WebFetch / curl 抓需 JS、登录态、反爬或 Cloudflare 的页。
3. **机器读取一律 `--json`**：`{ "ok": true, "data": ... }` 或 `{ "ok": false, "error": ... }`。
4. **先确保 daemon**：`drs ensure-serve --headless`，或命令加 `--ensure-serve --ensure-headless`。
5. **内容先落盘**：bundle / 网络包 / 截图先写入项目 `data/browser/`，再分析。
6. **用完必须 quit**：本轮做完立刻 `drs --json stop`（停 daemon **和** 浏览器）。只 `browser_close` / `drs close` 不够。Cookie 在 profile 里，quit 不丢登录。用户没说「别关」就不要挂着。

## MCP server 名

| 位置 | 名字 |
|---|---|
| mcp.json 的 key | **`drs`** |
| Cursor Agent 的 server id | **`user-drs`** |
| Codex | **`drs`** |

`CallMcpTool` 用 **`user-drs`**。

## Schema（会话内复用）

对 `user-drs` **最多完整发现一次**，之后直接 call。默认 `core` 已含网络 / 控制台 / 下载，**抓 XHR 不必改 `DRS_MCP_TOOLS`**。

PDF / inspect / record / 旧 `network_*` / CF / identity 才要用户把 `DRS_MCP_TOOLS` 设成 `browser` / `identity` / `all` 后重启 MCP。

稳定参数：

- `browser_open` `{url}`
- `browser_snapshot` `{budget_ms?, max_items?, max_text_chars?, max_markdown_chars?}`
- `browser_extract` `{url?, wait_selector?, pass_cf?, include_markdown?}`
- `browser_click` `{ref?|selector?, return_snapshot?}`
- `browser_type` `{ref?|selector?, text, return_snapshot?}`
- `browser_press` `{key, selector?, return_snapshot?}`
- `browser_wait` `{selector, timeout_ms?, return_snapshot?}`
- `browser_screenshot` `{out?, full?, inline?}`
- `browser_tabs` / `browser_use_tab` `{tab_id}` / `browser_close` `{tab_id?}`
- `browser_network` `{action: inspect|listen|wait|block|mock|rewrite|record|replay|stop, keywords?, methods?, xhr_only?, count?, timeout_ms?, body?, out?, har?, fallback?, limit?}`
- `browser_console` `{action: start|wait|list|stop, level?, text?, timeout_ms?}`
- `browser_download` `{action: start|list|wait|stop, dir?, timeout_ms?, count?}`

多步交互优先 `return_snapshot: true`。`stale_ref` → 立刻再 `browser_snapshot`，用新 `eN`。

## 读当前页（首选 snapshot）

```bash
drs ensure-serve --headless
drs --json tab open https://example.com
drs --json snapshot
```

返回 `title` / `url` / `snapshot`（`[ref=e1]`）/ `refs` / `markdown` / 短正文。然后：

```bash
drs --json click "ref:e1"
drs --json type "ref:e2" "hello"
```

MCP：`browser_snapshot` → `browser_click({ ref: "e1", return_snapshot: true })`。
一次性打开+抽正文用 `browser_extract` / `drs extract`。重页不要用 `browser_html` / 完整 `ax`。

## 常用 CLI（加 `--ensure-serve --ensure-headless --json`）

| 任务 | 命令 |
|---|---|
| 浏览器 / 标签 | `drs browser list`；`drs tab list`；`drs tab open URL` |
| **读懂当前页** | **`drs snapshot`** |
| 调试包 | `drs inspect`（snapshot + 网络 + console peek） |
| 截图 / PDF | `drs screenshot --out data/browser/page.png --full`；`drs pdf --out data/browser/page.pdf` |
| 抓 XHR | `drs listen start /api/ --xhr-only --method POST` → `listen wait` |
| 看已抓包 | `drs network inspect --limit 20`（不消费缓冲） |
| 拦 / mock / HAR | `drs network block /tracker`；`network mock /api/user --body '{…}'`；`network record --out …`；`drs replay FILE.har` |
| 控制台 | `drs console start --level error` → `console list` → `console stop` |
| 下载 | `drs download start --dir data/browser/downloads` → 再点击 → `download wait` |
| 录操作 | `drs record start` → 操作 → `record stop --out flow.rs` |
| Context / Profile | `drs context use account_001`；`drs profile list` |
| 过 CF | `drs pass-cf --timeout-ms 30000` |
| **quit** | **`drs stop`** |

`open`/`tabs`/`use`/`close` 仍可用。`console start` / `record start` 会开 `Runtime.enable`，过盾页别开。PDF / replay / record 只支持 CDP。

## MCP 配方（`user-drs`）

默认 `core`：`browser_snapshot`（读页）/ `extract` / `open` / `tabs` / `click` / `type` / `press` / `wait` / `screenshot` / **`browser_network` / `browser_console` / `browser_download`**。

抓接口（core 就能用）：

```
browser_network { "action": "listen", "keywords": ["/api/"], "xhr_only": true }
browser_network { "action": "wait", "count": 3, "timeout_ms": 5000 }
browser_network { "action": "inspect", "limit": 20 }
browser_network { "action": "stop" }
```

控制台 / 下载：

```
browser_console { "action": "start", "level": "error" }
browser_console { "action": "list" }
browser_download { "action": "start", "dir": "data/browser/downloads" }
browser_download { "action": "wait", "timeout_ms": 15000 }
```

`DRS_MCP_TOOLS=browser` 才有 `browser_pdf` / `browser_inspect` / `browser_record` / 旧 `network_*` / `browser_pass_cf` / `browser_html`。`=identity` 才有 `identity_assets_*`。

## 决策树

```
需要浏览器或动态页？
├─ 是 → 只用 drs（CLI 或 MCP user-drs）
│   ├─ 看当前页 → browser_snapshot
│   ├─ 打开+读 → browser_extract
│   ├─ 多步交互 → click/type/press/wait + return_snapshot=true
│   ├─ 抓 API → browser_network action=listen → wait / inspect
│   ├─ console / 下载 → browser_console / browser_download
│   └─ 做完 → drs --json stop
└─ 否（静态 JSON、无反爬）→ 普通 HTTP 客户端
```

## 安装

```bash
curl -fsSL https://raw.githubusercontent.com/MageGojo/drission-rs/main/install/drs-install.sh | sh
drs setup --scope global
cargo install --path crates/drission-cli --bin drs   # 从本仓库
```

改完 CLI 后必须 `drs stop` 再 `ensure-serve`，并重载 Cursor MCP。
