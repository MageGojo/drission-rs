# drission 文档 · Documentation

> [`drission`](https://crates.io/crates/drission) 的设计与 API 文档索引。
> API 参考(rustdoc)见 [docs.rs/drission](https://docs.rs/drission);快速上手见仓库根目录的
> [`README.md`](../README.md) / [`README.en.md`](../README.en.md)。

---

## 📚 文档清单

| 文档 | 内容 | 适合谁 |
|---|---|---|
| [设计.md](设计.md) | 自顶向下分层架构、Juggler 协议选型、异步并发模型、启动引导、监听/拦截/过盾接线、截图录像、WS 接管、吐环境、CDP 后端 | 想理解内部实现 / 参与贡献的人 |
| [API映射.md](API映射.md) | **DrissionPage(Python)→ drission(Rust)** 逐条 API 对照表,覆盖启动、页面、元素、定位、监听、拦截、滑块、动作链、iframe 等 | 从 DrissionPage 迁移、想快速查等价写法的人 |
| [CLI.md](CLI.md) | `drs` CLI / MCP 使用说明:daemon、JSON 输出、AI Agent 调用、stdio MCP 工具、OCR 命令 | 想让 AI 或脚本直接操作浏览器的人 |
| [并发池.md](并发池.md) | `BrowserPool` / 代理池 / 指纹池 / 断点续抓 的设计:两层指纹约束、轮换策略、健康自愈、Checkpoint、踩坑记录 | 做高并发规模化采集的人 |
| [长监听与滑动.md](长监听与滑动.md) | 长会话持续监听(后台抽取 + 流式 API,不丢包)+ 输入驱动翻页(`press_key`/`wheel`)的设计 | 需要连续抓取 SPA 翻页签名(如抖音 feed)的人 |
| [标配补齐.md](标配补齐.md) | 对标 Playwright/Puppeteer/DrissionPage 的通用能力:PDF/MHTML/set_content、HAR 录制+回放、expose_function、媒体·网络·CPU 模拟、移动端设备预设、权限/storage、wait 补齐(及两端可行性) | 想要"开箱即用"的浏览器标配能力的人 |
| [录制与无障碍.md](录制与无障碍.md) | 录制→生成可运行 Rust 代码(codegen/recorder,对标 PW codegen)+ 无障碍 `role "name"` 语义树快照(a11y,抗改版断言 / 喂 LLM) | 想录操作出代码、或用语义树做断言/喂 LLM 的人 |

> 更新历史见 [`CHANGELOG.md`](../CHANGELOG.md);贡献指南见 [`CONTRIBUTING.md`](../CONTRIBUTING.md);
> 安全策略见 [`SECURITY.md`](../SECURITY.md)。

## 🧪 示例

48+ 个端到端示例(大多**完全离线自验证**)的「能力 → 示例 → 运行命令」总览见
[`examples/README.md`](../examples/README.md)。

## 🗂️ 阅读建议

- **第一次用** → 先看根 [`README.md`](../README.md) 的「快速开始」,再跑 `cargo run --example quickstart`。
- **从 DrissionPage 来** → 直接查 [API映射.md](API映射.md),按表把 Python 写法换成 Rust。
- **要做大规模采集** → 读 [并发池.md](并发池.md) + 跑 `cargo run --example pool_crawl`。
- **想改源码 / 提 PR** → 通读 [设计.md](设计.md),它和 `src/` 的目录结构一一对应。
