# drission 文档 · Documentation

> [`drission`](https://crates.io/crates/drission) 的设计与 API 文档索引。
> API 参考(rustdoc)见 [docs.rs/drission](https://docs.rs/drission);快速上手见仓库根目录的
> [`README.md`](../README.md) / [`README.en.md`](../README.en.md)。

---

## 📚 文档清单

| 文档 | 内容 | 适合谁 |
|---|---|---|
| [法律风险.md](法律风险.md) | 公开示例只测本地页和自己的站。OCR 代码还在，文档和示例不再写用法 | 看公开仓库现在承诺什么 |
| [运行时分层.md](运行时分层.md) | **Browser / HTTP / AI / Task 四层** + 一等公民 `BrowserContext` / `ProfileManager` / 网络门面;避免巨大 Browser 类 | 做多账号、池化、网络调试,或判断下一步往哪长的人 |
| [Agent运行时.md](Agent运行时.md) | LLM → 语义动作 → Locator → AX/DOM;`tab.agent()` / `Locator` / `wait()` / `capture()` / `TaskScheduler` | 写 Agent、不想让模型直接拼 CSS 的人 |
| [进度.md](进度.md) | 里程碑 90 可验收进度(Context / Profile / 网络门面) | 断点续作、对账「做到哪了」 |
| [设计.md](设计.md) | 自顶向下分层架构、Juggler 协议选型、异步并发模型、启动引导、监听/拦截/过盾接线、截图录像、WS 接管、吐环境、CDP 后端 | 想理解内部实现 / 参与贡献的人 |
| [差异化路线.md](差异化路线.md) | 为什么 drission-rs 不应只做 Python DrissionPage 的 Rust 复刻,以及账号/Profile 运行时治理 sidecar 的产品路线 | 判断下一步功能优先级、做产品/架构取舍的人 |
| [../python/README.md](../python/README.md) | Python 旧业务接入 `identity-job run` 的薄 helper:读取 `DRS_IDENTITY_*`、写 `DRS_IDENTITY_RESULT_OUT` | 想保留 Python 业务逻辑,只把账号/Profile 治理交给 Rust 的人 |
| [API映射.md](API映射.md) | **DrissionPage(Python)→ drission(Rust)** 逐条 API 对照表,覆盖启动、页面、元素、定位、监听、拦截、滑块、动作链、iframe 等 | 从 DrissionPage 迁移、想快速查等价写法的人 |
| [CLI.md](CLI.md) | `drs` CLI / MCP 使用说明:daemon、JSON 输出、AI Agent 调用、stdio MCP 工具 | 想让 AI 或脚本直接操作浏览器的人 |
| [产品级CLI.md](产品级CLI.md) | **AI 怎么选命令**：产品 CLI + 语义 MCP 配方（`browser_network` / `console` / `download`） | Agent / 用 `drs` 当调试器 |
| [并发池.md](并发池.md) | `BrowserPool` / 代理池 / 断点续跑 | 做并发任务的人 |
| [标配补齐.md](标配补齐.md) | 对标 Playwright/Puppeteer/DrissionPage 的通用能力:PDF/MHTML/set_content、HAR 录制+回放、expose_function、媒体·网络·CPU 模拟、移动端设备预设、权限/storage、wait 补齐(及两端可行性) | 想要"开箱即用"的浏览器标配能力的人 |
| [录制与无障碍.md](录制与无障碍.md) | 录制→生成可运行 Rust 代码(codegen/recorder,对标 PW codegen)+ 无障碍 `role "name"` 语义树快照(a11y,抗改版断言 / 喂 LLM) | 想录操作出代码、或用语义树做断言/喂 LLM 的人 |
| [后端能力矩阵.md](后端能力矩阵.md) | **CDP(默认) vs Camoufox** 能力对照 | 选后端 |
| [Lightpanda.md](Lightpanda.md) | Lightpanda 作为 CDP 引擎变体:同一套 Browser/Tab,只换 `serve`;下载、能力边界。`drs` 不接 | 服务器上要轻量无头、或对接 Lightpanda Cloud 的人 |
| [分析-Lightpanda能否用Rust复现.md](分析-Lightpanda能否用Rust复现.md) | Zig 难度、引擎体量、Rust 积木对照、三档工期、许可证；结论是不在本库复刻内核 | 想自己写无头引擎、或判断要不要 Rust 重写 Lightpanda 的人 |

> 更新历史见 [`CHANGELOG.md`](../CHANGELOG.md);贡献指南见 [`CONTRIBUTING.md`](../CONTRIBUTING.md);
> 安全策略见 [`SECURITY.md`](../SECURITY.md)。

## 🧪 示例

48 个以内的示例索引见 [`examples/README.md`](../examples/README.md)。先跑 `cargo run --example own_site`。

## 🗂️ 阅读建议

- **第一次用** → 先看根 [`README.md`](../README.md)，再跑 `cargo run --example own_site`。
- **写 Agent / 用 MCP 控浏览器** → 先读 [产品级CLI.md](产品级CLI.md)，再按需查 [CLI.md](CLI.md)。抓 XHR 用默认 core 的 `browser_network`，不必改 `DRS_MCP_TOOLS`。
- **从 DrissionPage 来** → 先看 [差异化路线.md](差异化路线.md)；只查 API 等价写法时再看 [API映射.md](API映射.md)。
- **想改源码 / 提 PR** → 通读 [设计.md](设计.md)。
