# 安装 drission / drs

## A. 只想用 `drs` CLI / MCP(不需要 Rust)—— 推荐

装预编译二进制,一条命令:

```bash
# mac / linux
curl -fsSL https://raw.githubusercontent.com/MageGojo/drission-rs/main/install/drs-install.sh | sh
# windows(PowerShell)
irm https://raw.githubusercontent.com/MageGojo/drission-rs/main/install/drs-install.ps1 | iex
```

`drs-install.sh` / `drs-install.ps1` 会:

1. 识别系统与架构(mac arm64/x64、linux x64/arm64 musl 静态、windows x64);
2. 从 [GitHub Releases](https://github.com/MageGojo/drission-rs/releases)(GitCode 备用镜像)下载对应 `drs`;
3. 装到 `~/.local/bin`(Windows 装到 `%LOCALAPPDATA%\drs\bin`,并加进用户 PATH);
4. 打印版本号,提示下一步 `drs setup`。

装完接入 AI 编辑器:

```bash
drs setup            # 自动写 Cursor(.cursor/mcp.json)+ Codex(~/.codex/config.toml)的 MCP 配置
drs setup --target cursor   # 只配 Cursor;--target codex 只配 Codex;--scope global 写全局
```

环境变量:`DRS_VERSION=v0.3.2` 装指定版本、`DRS_INSTALL_DIR=~/bin` 换安装目录。

---

## B. 想写 Rust 用 `drission` 库 —— 一键配 Rust 环境

不熟悉 Rust?是 Python / TS 爬虫开发者,只想快速跑起来?用下面的**一键脚本**:
自动用**国内镜像([rsproxy.cn](https://rsproxy.cn))**装好 Rust + 配好 cargo 加速,**带进度、自动验证版本号**。
全程**装在用户目录、无需管理员权限**,也**不依赖 Homebrew 等包管理器**。

---

## macOS

1. 下载本目录的 **`install-mac.command`**。
2. **双击运行**(会自动打开「终端」)。
   - 若提示“无法打开,因为来自身份不明的开发者”:**右键 → 打开 → 打开**;或终端执行
     `xattr -d com.apple.quarantine install-mac.command` 后再双击。
   - 若双击没反应(下载后丢了可执行权限):终端执行 `chmod +x install-mac.command` 再双击,
     或直接 `bash install-mac.command`。
3. 首次可能弹出 **Apple 命令行工具**安装器(编译 Rust 必需),点「安装」等它装完,脚本会自动继续。

做的事:确保 Xcode 命令行工具 → rsproxy 装 rustup/Rust → 配 cargo 国内镜像 → 打印 `rustc` / `cargo` 版本号。

## Windows

1. 下载本目录的 **`install-windows.bat`** 和 **`install-windows.ps1`**(放同一个文件夹)。
2. **双击 `install-windows.bat`**。
   - 若 SmartScreen 拦截:点“更多信息 → 仍要运行”。
3. 等待进度跑完,最后会打印 `rustc` / `cargo` 版本号。

安装的是 **`x86_64-pc-windows-gnu`** 工具链:**自带链接器、免装 Visual Studio**,纯 Rust 项目
(含 drission 默认 `cdp` 后端)开箱即编。**无需管理员权限**(不会弹 UAC)。

---

## 装完怎么用

**新开一个终端 / 命令行窗口**(让 PATH 生效),然后:

```bash
cargo new demo && cd demo
cargo add drission
cargo run
```

开发本仓库内置的 `drs` CLI / MCP 入口:

```bash
cargo install drission-cli --bin drs
# 或从当前仓库源码安装:
cargo install --path crates/drission-cli --bin drs
drs serve --backend cdp --headless
```

## 进阶 / 可选的 C 编译环境

以下功能依赖 C 工具链编译,**体验默认功能用不到**,需要时再装:

- **Windows**:`impersonate`(TLS/JA3 指纹,BoringSSL)等需 **Visual Studio Build Tools + nasm + cmake**;
  或改用 MSVC 工具链 `rustup default stable-x86_64-pc-windows-msvc`。
- **macOS**:已随 Xcode 命令行工具具备 `clang`/`cmake` 可按需补(`xcode-select --install`)。

## 手动配置(脚本失败时的等价命令)

```bash
# 1) rustup 镜像
export RUSTUP_DIST_SERVER="https://rsproxy.cn"
export RUSTUP_UPDATE_ROOT="https://rsproxy.cn/rustup"
# 2) 安装(mac/linux)
curl --proto '=https' --tlsv1.2 -sSf https://rsproxy.cn/rustup-init.sh | sh
# 3) cargo 镜像:把以下写入 ~/.cargo/config.toml
#   [source.crates-io]
#   replace-with = "rsproxy-sparse"
#   [source.rsproxy-sparse]
#   registry = "sparse+https://rsproxy.cn/index/"
```
