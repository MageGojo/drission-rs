# 产品级 CLI / 语义 MCP（给 AI 用）

> `drs` 是浏览器调试 CLI，和库同一版本。库能做的，CLI 和 MCP 也能做。
> AI 只面对语义工具，不要猜 CDP，不要再用 Playwright / WebFetch 抓动态页。

Cursor 里 MCP server id 是 **`user-drs`**（配置 key 是 `drs`）。机器读一律 `drs --json`。
做完浏览立刻 `drs --json stop`（停 daemon 和浏览器；Cookie 在磁盘 profile，下次还在）。

更全的字段见 [`CLI.md`](CLI.md)。本页是「怎么选命令」。

## 先选哪条

```
要浏览器或动态页？
├─ 读当前页 → browser_snapshot / drs snapshot
├─ 打开并抽正文 → browser_extract / drs extract
├─ 点、输入、按键 → browser_click / type / press（可 return_snapshot=true）
├─ 看标签 → browser_tabs 或 drs tab list
├─ 抓 XHR / 拦 / mock / HAR → browser_network（默认 core 就有）
├─ 看 console → browser_console
├─ 等下载 → browser_download（先 start 再触发下载）
├─ 整页调试包 → drs inspect / browser_inspect（后者要 DRS_MCP_TOOLS=browser）
├─ 出 PDF → drs pdf / browser_pdf（无头 Chrome；要 browser toolset）
└─ 录操作出 Rust → drs record / browser_record（要 browser toolset）
```

旧扁平命令仍可用：`open`=`tab open`，`tabs`=`tab list`，`use`=`tab use`，`close`=`tab close`。
旧 `network_listen_*` 仍在 `browser` toolset，新代码优先 `browser_network`。

## CLI

均建议 `--ensure-serve --ensure-headless --json`。

```bash
drs ensure-serve --headless

# 标签
drs --json browser list
drs --json tab list
drs --json tab open https://example.com
drs --json tab use 1
drs --json tab close

# 读页（首选 snapshot，不要先拉整页 HTML）
drs --json snapshot
drs --json inspect                          # snapshot + 网络 peek + console peek
drs --json pdf --out data/browser/page.pdf  # 无头 Chrome

# 网络（inspect 不消费缓冲）
drs --json listen start /api/ --xhr-only --method POST
drs --json network inspect --limit 20
drs --json listen wait --count 3 --timeout-ms 5000
drs --json network block /tracker
drs --json network mock /api/user --body '{"ok":true}'
drs --json network record --out data/browser/session.har
drs --json replay data/browser/session.har --fallback   # 导航前回放
drs --json network stop

# 控制台（会开 Runtime.enable，过盾页别开）
drs --json console start --level error
drs --json console list
drs --json console stop

# 下载（先 start 再点下载）
drs --json download start --dir data/browser/downloads
drs --json download wait --timeout-ms 15000
drs --json download list

# 录操作 → Rust
drs --json record start
drs --json record stop --out data/browser/flow.rs

drs --json stop
```

| 产品命令 | 库 | 旧别名 |
|---|---|---|
| `browser list` | daemon + 标签 + context | `status` |
| `tab list\|open\|use\|close` | `new_tab` | `tabs` / `open` / `use` / `close` |
| `context` / `profile` | Context / ProfileManager | — |
| `network inspect` | `listen.peek()` | — |
| `network block\|mock\|rewrite\|record\|replay\|stop` | `tab.network()` | `listen` 仍可用 |
| `record start\|stop` | `tab.recorder()` | — |
| `replay FILE.har` | `network().replay()` | `network replay` |
| `pdf` | `save_pdf`（无头） | — |
| `inspect` | snapshot + peek | — |
| `console` / `download` | `tab.console()` / `tab.downloads()` | — |

PDF / HAR replay / codegen 只支持 CDP。

## MCP（`user-drs`）

会话内对 `user-drs` 最多完整发现一次，之后直接 `CallMcpTool`。

默认 `DRS_MCP_TOOLS=core`：

| 工具 | 参数 | 何时用 |
|---|---|---|
| `browser_open` | `{url}` | 开新标签 |
| `browser_snapshot` | `{budget_ms?, max_items?, max_text_chars?, max_markdown_chars?}` | **读当前页首选** |
| `browser_extract` | `{url?, wait_selector?, pass_cf?, include_markdown?}` | 打开并抽正文 |
| `browser_click` | `{ref?\|selector?, return_snapshot?}` | 点。`ref` 来自 snapshot 的 `e1` |
| `browser_type` | `{ref?\|selector?, text, return_snapshot?}` | 输入 |
| `browser_press` | `{key, selector?, return_snapshot?}` | Enter / Tab / Escape |
| `browser_wait` | `{selector, timeout_ms?, return_snapshot?}` | 等元素 |
| `browser_screenshot` | `{out?, full?, inline?}` | 截图 |
| `browser_tabs` / `use_tab` / `close` | `{tab_id?}` | 标签 |
| **`browser_network`** | `{action, keywords?, methods?, xhr_only?, count?, timeout_ms?, …}` | 听 / 看包 / 拦 / mock / HAR |
| **`browser_console`** | `{action, level?, text?, timeout_ms?}` | 控制台 |
| **`browser_download`** | `{action, dir?, timeout_ms?, count?}` | 下载 |

`DRS_MCP_TOOLS=browser` 才有：`browser_pdf`、`browser_inspect`、`browser_record`、旧 `network_*`、`browser_pass_cf`、`browser_html`、`browser_context_*`、`profile_*`。

### 复制即用

读页：

```
browser_snapshot
browser_click { "ref": "e1", "return_snapshot": true }
browser_type { "ref": "e2", "text": "hello", "return_snapshot": true }
```

抓接口（**不必**改 `DRS_MCP_TOOLS`）：

```
browser_network { "action": "listen", "keywords": ["/api/"], "xhr_only": true, "methods": ["POST"] }
browser_network { "action": "wait", "count": 3, "timeout_ms": 5000 }
browser_network { "action": "inspect", "limit": 20 }
browser_network { "action": "stop" }
```

拦 / 假响应 / 录 HAR / 回放：

```
browser_network { "action": "block", "keywords": ["/tracker"] }
browser_network { "action": "mock", "keywords": ["/api/user"], "status": 200, "body": "{\"ok\":true}" }
browser_network { "action": "record", "out": "data/browser/session.har" }
browser_network { "action": "replay", "har": "data/browser/session.har", "fallback": true }
browser_network { "action": "stop" }
```

控制台 / 下载：

```
browser_console { "action": "start", "level": "error" }
browser_console { "action": "list" }
browser_console { "action": "stop" }

browser_download { "action": "start", "dir": "data/browser/downloads" }
# 再 click 触发下载
browser_download { "action": "wait", "timeout_ms": 15000, "count": 1 }
browser_download { "action": "list" }
```

`browser` toolset：

```
browser_inspect { "include_network": true, "include_console": true, "max_packets": 20 }
browser_pdf { "out": "data/browser/page.pdf" }
browser_record { "action": "start" }
browser_record { "action": "stop", "out": "data/browser/flow.rs" }
```

`stale_ref` → 立刻再 `browser_snapshot`，用新的 `eN`。不要重试旧 ref。
`console start` / `record start` 会开 `Runtime.enable`，过盾页不要开。
