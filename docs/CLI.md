# drs CLI / MCP

`drs` 是 `drission` 的同仓库命令行与 MCP 入口。它面向 AI Agent 和自动化脚本:浏览器由一个本地 daemon 持有,普通命令通过本地 JSONL 协议驱动同一组标签;MCP 模式则直接在 stdio 进程内持有浏览器。

## 安装

从 crates.io 安装:

```bash
cargo install drission-cli --bin drs
```

默认构建 CDP/Chrome 后端。按需启用能力:

```bash
cargo install drission-cli --bin drs --features cdp,ocr
cargo install drission-cli --bin drs --no-default-features --features camoufox
```

开发期从本仓库安装:

```bash
cargo install --path crates/drission-cli --bin drs
```

## Daemon 模式

启动:

```bash
drs serve --backend cdp --headless
```

连接信息写入用户缓存目录下的 `drission/cli/drs-server.json`,包含 `host`、`port`、`token`、`pid` 和 `backend`。其它命令会读取该文件并带 token 调用本地 daemon。

常用命令:

```bash
drs --json status
drs --json open https://example.com
drs --json tabs
drs --json use 1
drs ax --outline
drs --json ax --json
drs html
drs text h1
drs eval "document.title"
drs click "text:登录"
drs type "#kw" "drission"
drs press Enter --selector "#kw"
drs wait "#result" --timeout-ms 5000
drs screenshot --out /tmp/page.png --full
drs listen start /api/ --xhr-only
drs --json listen wait --count 3 --timeout-ms 5000
drs listen stop
drs pass-cf --timeout-ms 30000
drs close
drs stop
```

机器读取建议始终使用 `drs --json ...`。成功响应:

```json
{ "ok": true, "data": {} }
```

失败响应:

```json
{ "ok": false, "error": { "code": "daemon_not_running", "message": "...", "hint": "..." } }
```

## MCP 模式

启动 stdio MCP server:

```bash
drs mcp --backend cdp --headless
```

MCP 模式不依赖 `drs serve`;该进程自己持有浏览器状态。首版暴露这些稳定工具名(客户端不要依赖 `tools/list` 的返回顺序):

- `browser_open`
- `browser_tabs`
- `browser_use_tab`
- `browser_ax`
- `browser_html`
- `browser_text`
- `browser_eval`
- `browser_click`
- `browser_type`
- `browser_wait`
- `browser_screenshot`
- `network_listen_start`
- `network_listen_wait`
- `network_listen_stop`
- `browser_pass_cf`

`browser_screenshot` 默认保存 PNG 并返回路径;传 `inline=true` 时同时返回 base64 与 MCP image content。

## OCR

开启 `ocr` feature 后可使用纯图片点选求解:

```bash
drs --json ocr clickword ./captcha.png 税实企
```

输出包含按目标顺序的 `points` 和每个命中的 `bbox` / `affinity` / `template`。

## 常见错误

| 错误码 | 含义 | 处理 |
|---|---|---|
| `daemon_not_running` | 没找到可用 daemon | 先运行 `drs serve --headless` |
| `daemon_unreachable` | state 文件存在但端口连不上 | 重启 `drs serve`;CLI 会移除 stale state |
| `unauthorized` | token 不匹配 | 删除缓存中的 `drs-server.json` 或重启 daemon |
| `command_failed` | 浏览器动作失败 | 查看 `message`;常见是 selector 未命中或页面超时 |
| `Session with given id not found` | active tab 对应的浏览器 target 已关闭,常见于打开会触发下载的 URL | 用 `drs open` 打开 HTML 页面;下载型资源用网络监听或 HTTP 客户端处理 |

## 设计边界

首版聚焦 AI 可调用的浏览器运行时,不包含完整批量爬虫、HAR 回放、recorder、dump_env、代理池 UI、滑块全流程命令。这些能力仍可通过 Rust API 使用,后续可逐步接入 CLI/MCP。
