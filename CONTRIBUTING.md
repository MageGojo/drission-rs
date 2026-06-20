# 贡献指南 / Contributing

感谢你愿意为 **drission** 出力。本项目由 [极数本源(apizero.cn)](https://apizero.cn) 维护。

## 开发环境

- Rust ≥ **1.85**(edition 2024)。仓库带 `rust-toolchain.toml`,会自动选用 stable + rustfmt/clippy。
- 默认后端是 Camoufox(Firefox/Juggler),首次运行示例会自动下载浏览器到 `~/.cache/camoufox`。

## 本地校验(提交前请全过)

```bash
cargo fmt --all                                   # 格式化
cargo clippy --all-targets --features slider,cdp -- -D warnings   # 静态检查(零警告)
cargo test --features slider,cdp --lib --tests    # 单测 + 离线集成测试
cargo test --features ocr --lib                   # OCR 档(引入 tract/image 重依赖)
cargo bench --bench parsing                        # 可选:性能基准
```

跨平台编译(在 macOS/Linux 上验证 Windows 分支):

```bash
rustup target add x86_64-pc-windows-gnu
cargo check --target x86_64-pc-windows-gnu --features slider,cdp
```

## Feature 说明

| feature | 能力 | 默认 |
|---|---|---|
| `camoufox` | Camoufox/Firefox(Juggler)后端 —— 核心 | 开 |
| `cdp` | Chromium(Chrome/Edge/Brave/Electron)后端 | 关 |
| `slider` | 图片滑块缺口距离识别(纯 JS+std,零额外依赖) | 关 |
| `ocr` | 字符验证码 OCR(ddddocr + tract) | 关 |

新增对外类型记得在 `src/lib.rs` 的 `prelude` 导出;feature-gated 的类型用 `#[cfg(feature = "...")]` 守卫。

## 代码约定

- **组件化**:按职责拆模块,不要把代码堆进入口文件;新能力放到对应 `src/<module>/`。
- **不 panic**:非测试代码不要 `unwrap`/`expect`/`panic!`,错误一律走 `Result`(`crate::Error`)。
- **注释只解释“为什么”**:不写复述代码的废话注释。
- **像素重活留在页面 JS**:`getImageData` 等只回传标量,别把大数组经协议回传。
- 新增可验证能力时,优先补一个**离线**(不出网/不开浏览器或本地页)的 example,末行打印 `ALL CHECKS PASSED`。

## 提交信息(Conventional Commits)

沿用现有风格,如:

```
feat(cdp): Chromium 后端 MVP
fix(transport): Windows 有头补 -wait-for-browser
docs: README 能力清单
chore: 移除示例运行残留
```

## Pull Request

1. 从 `main` 切分支开发。
2. 跑完上面的本地校验。
3. PR 描述清楚动机与改动范围;涉及行为变化的请更新 `CHANGELOG.md` 的 `[Unreleased]`。
4. CI(fmt/clippy/test/docs)需全绿。

## 许可

提交即表示你同意你的贡献以本仓库 [`LICENSE`](LICENSE) 的条款分发。
