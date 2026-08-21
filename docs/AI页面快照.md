# AI 页面快照 + MCP 防卡退

## 目标

1. **让 AI 读懂当前页**：一条命令拿到「页面上有什么、能点什么」，而不是整页 HTML 或可能爆炸的完整无障碍树。
2. **修部分页面对自动化不友好导致 MCP 卡退**：重 SPA / 广告页上 `html`/`ax`/`innerText` 可能跑很久或回包巨大，MCP 工具调用挂死 → 客户端超时断连。

## 方案

### `drs snapshot` / MCP `browser_snapshot`（首选读页）

对活动标签做 **interesting-only** 语义快照：

- 只收集可交互控件 + 标题/地标（button/link/textbox/checkbox/… + heading/main/nav/…）
- 给每个可操作节点打 `data-drs-ref="eN"`，输出 YAML 风格大纲 + `refs` 表
- JS 内置 **时间预算**（默认 2s）与 **节点上限**（默认 250），超时返回 `partial:true` 而不是挂死
- **HTML→Markdown**（Rust [`htmd`](https://crates.io/crates/htmd)）：读 `body.innerHTML` 转 MD，默认截断 50k；比 raw HTML 更适合喂 LLM
- 顺带截断可见正文（默认 8k 字符）

交互：`click` / `type` 支持选择器 `ref:e1`（解析为 `[data-drs-ref="e1"]`）；MCP 另提供可选 `ref` 参数。

单独只要 Markdown：`drs markdown` / MCP `browser_markdown`。`extract` 默认也带 `markdown`（`--no-markdown` 可关）。

整页调试包用 `drs inspect` / MCP `browser_inspect`（后者要 `DRS_MCP_TOOLS=browser`）：轻量 snapshot + `network inspect` peek + console peek。抓 XHR 用默认 core 的 `browser_network`，不必先改 toolset。怎么选命令见 [`产品级CLI.md`](产品级CLI.md)。

### 防卡退

| 层 | 措施 |
|---|---|
| Daemon RPC | `send_to_daemon` 读响应加超时（默认 60s，`DRS_DAEMON_TIMEOUT_MS`） |
| MCP `exec` | 同样包一层超时，超时返回结构化错误而非永不返回 |
| `browser_html` / `Html` | 默认截断（200k Unicode scalars），可配 |
| `extract` 的 ax | 单独短超时；失败时 outline 置空并标 `outlineError`，不拖垮整包 |
| 快照 JS | 时间预算 + 上限 + interesting-only，避免全 DOM `getComputedStyle` 扫爆 |

## 验收

- `drs --json snapshot` 在已打开标签上返回 `title`/`url`/`snapshot`/`refs`/`markdown`
- MCP 工具列表含 `browser_snapshot`、`browser_markdown`
- 对超大 HTML 页，`html` 带截断标记且不拖垮 MCP
- daemon/MCP 超时后返回 `timeout` 错误码，进程不挂死

## 产物

- `src/ai_snapshot.rs`、`src/html_md.rs`（htmd）
- `crates/drission-cli/src/{protocol,cli,engine,backend,mcp,daemon}.rs` 接线
- `docs/CLI.md` / skill 同步
