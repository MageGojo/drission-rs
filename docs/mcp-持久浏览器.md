# MCP 持久浏览器（attach daemon）

## 背景问题

`drs` 有两条运行时:

- **CLI daemon**（`drs serve`）: 一个常驻进程持有浏览器 `BrowserState`，其它 `drs` 子命令通过本地 `127.0.0.1` + token 的 JSONL 协议连上来，复用同一组标签。CLI 侧“持久浏览器”一直是好的。
- **MCP**（`drs mcp`）: 旧实现里 MCP **在自己进程内单独 launch 一个浏览器**。

旧 MCP 的三个痛点:

1. MCP 浏览器和 CLI daemon 浏览器是两个，互相看不到对方的标签/登录态。
2. Cursor 只要重启 MCP server（改配置、重开窗口、agent 会话结束），MCP 进程一死，浏览器连同 cookie / 登录态 / 已开标签全丢。
3. 结果就是 AI 每次通过 MCP “查数据”都是全新浏览器，登录站点要反复重新登录。

## 目标

- MCP 默认**接到常驻 daemon 的同一个持久浏览器**，浏览器活在长命的 `drs serve` 进程里，MCP 重启不影响它。
- daemon 用**固定 profile 目录**，即使 daemon 本身重启，Chrome 复用同一 profile → cookie / 登录态存活。
- CLI 和 MCP 共享同一浏览器: CLI 里 `drs open` 打开并登录的标签，AI 通过 MCP `browser_*` 能直接接着用，反之亦然。

## 设计

### 1. MCP 默认 attach daemon

`drs mcp` 启动时:

1. 默认 `ensure_daemon(backend, headless, user_data_dir)`——没有健康 daemon 就后台拉起 `drs serve`（子进程 stdout/stderr 置空，不污染 MCP 的 stdio JSON-RPC）。
2. 所有 `browser_*` / `network_*` 工具调用不再锁本进程的浏览器，而是走 `daemon::send_to_daemon(command)` 发到 daemon。
3. 浏览器状态全部留在 daemon 进程；MCP 进程本身无状态，随便重启。

`--standalone` 回退旧行为: MCP 在自己进程内持有浏览器（用于不想常驻 daemon 的一次性场景）。

实现: `DrsMcp` 内部持有 `DrsBackend` 枚举:

```text
enum DrsBackend {
    Daemon,                         // 默认：转发到 drs serve
    Local(Arc<Mutex<BrowserState>>) // --standalone：进程内浏览器
}
```

`exec_response(command)` 按 backend 分派:

- `Daemon` → `daemon::send_to_daemon(command).await`
- `Local` → 锁 state 本地执行

两条路径都返回统一的 `JsonResponse`，上层 `browser_screenshot` 内联图片等逻辑不变。

`identity_assets_*` 等纯文件治理工具不需要浏览器，保持进程内直接调用。

### 2. 固定持久 profile

新增 `paths::default_profile_dir()` = `<cache>/drission/cli/profile`。

`serve` / `ensure-serve` / `mcp` / 全局 `--ensure-serve` 在未显式给 `--user-data-dir` 时，默认用这个固定目录。于是:

- daemon 重启 / 机器重启后，Chrome 仍读同一 profile，登录态不丢。
- 显式 `--user-data-dir` 仍可覆盖（多 profile / 账号池场景）。

### 3. Cursor 配置

`.cursor/mcp.json` 的 `drs` server 用 `drs mcp --backend cdp --headless`，靠默认逻辑 attach 持久 daemon + 固定 profile，无需额外参数。

## 验收标准

- `drs mcp`（无 --standalone）启动后，`drs serve` daemon 被拉起或复用；MCP 工具与 CLI 命令操作的是同一浏览器（同一 `status` 的 pid / 标签）。
  - 产物: `crates/drission-cli/src/mcp.rs`、`daemon.rs`、`cli.rs`、`main.rs`、`paths.rs`
- MCP 进程杀掉重开后，之前 `browser_open` 的标签与登录态仍在（daemon 未死）。
- `drs mcp --standalone` 仍在进程内独立跑浏览器（回退可用）。
- `cargo build -p drission-cli` / `cargo clippy` / `cargo test -p drission-cli` 全绿。

## 状态

- 已完成。已实测:
  - `ensure-serve` 拉起的 daemon 用 `process_group(0)` 脱离启动者进程组,启动进程被杀后 daemon 仍存活。
  - MCP 会话 #1 `browser_open` 的标签,在 MCP 进程退出后由 daemon 保留;新起的 MCP 会话 #2 `browser_tabs` 仍能看到 CLI 与 MCP 先后开的全部标签。
  - `drs mcp --standalone` 独立进程内浏览器,不影响 daemon。
  - `cargo test -p drission-cli` 94 passed;clippy 无新增告警(仍 41 条既有)。
