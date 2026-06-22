#!/usr/bin/env bash
# =============================================================================
# drission · macOS 一键配置 Rust 开发环境(国内 rsproxy 镜像加速)
#
# 给谁用:不熟悉 Rust 的 Python / TS 等开发者,想快速体验 drission。
# 怎么用:**双击本文件**(会在「终端」里运行);或终端执行 `bash install-mac.command`。
# 做了啥:① 确保 Xcode 命令行工具(编译/链接必需)② 用 rsproxy 国内镜像装 rustup+Rust
#         ③ 配置 cargo 走国内镜像(拉依赖飞快)④ 验证 rustc / cargo 版本号。
# 权限:  全程**装在用户目录、无需 sudo/管理员**;Xcode 工具用 Apple 官方安装器(弹窗点“安装”)。
# 不依赖 Homebrew —— 不假设你装过任何包管理器。
# =============================================================================
set -u

RED=$'\033[31m'; GRN=$'\033[32m'; YEL=$'\033[33m'; CYN=$'\033[36m'; BLD=$'\033[1m'; RST=$'\033[0m'
step(){ printf "\n${CYN}${BLD}==> %s${RST}\n" "$*"; }
ok(){   printf "${GRN}✓ %s${RST}\n" "$*"; }
warn(){ printf "${YEL}! %s${RST}\n" "$*"; }
err(){  printf "${RED}✗ %s${RST}\n" "$*"; }
pause_exit(){ printf "\n%s\n" "$1"; printf "(按回车键关闭本窗口)"; read -r _ || true; exit "${2:-0}"; }

RSPROXY="https://rsproxy.cn"
export RUSTUP_DIST_SERVER="$RSPROXY"
export RUSTUP_UPDATE_ROOT="$RSPROXY/rustup"
CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"

clear 2>/dev/null || true
printf "${BLD}drission · macOS 一键 Rust 环境配置${RST}\n"
printf "镜像:%s(国内加速)   安装位置:%s\n" "$RSPROXY" "$CARGO_HOME"
printf "本脚本不需要管理员权限,装在你的用户目录。\n"

# ---------------------------------------------------------------------------
# [1/5] Xcode 命令行工具(提供 cc/clang 链接器 —— 编译任何 Rust 程序都必需)
# ---------------------------------------------------------------------------
step "[1/5] 检查 Xcode 命令行工具(编译/链接必需)"
if xcode-select -p >/dev/null 2>&1 && /usr/bin/xcrun --find cc >/dev/null 2>&1; then
  ok "已安装:$(xcode-select -p 2>/dev/null)"
else
  warn "未检测到命令行工具 —— 即将弹出 Apple 官方安装器,请在弹窗点「安装」并等待完成。"
  xcode-select --install 2>/dev/null || true
  printf "等待安装完成(装好后自动继续,最多约 20 分钟)"
  for _ in $(seq 1 240); do
    if xcode-select -p >/dev/null 2>&1 && /usr/bin/xcrun --find cc >/dev/null 2>&1; then break; fi
    sleep 5; printf "."
  done
  printf "\n"
  if xcode-select -p >/dev/null 2>&1; then ok "命令行工具就绪"
  else warn "命令行工具仍未就绪;装好后重跑本脚本即可(rustup 仍会先装好)"; fi
fi

# ---------------------------------------------------------------------------
# [2/5] 安装 rustup + Rust 工具链(走 rsproxy 国内镜像)
# ---------------------------------------------------------------------------
step "[2/5] 安装 Rust 工具链(rustup,国内镜像,带进度)"
if command -v rustc >/dev/null 2>&1 && command -v cargo >/dev/null 2>&1; then
  ok "已检测到 Rust:$(rustc --version) —— 跳过安装"
else
  TMP="$(mktemp -t rustup-init 2>/dev/null || echo "$HOME/.rustup-init.sh")"
  if curl --proto '=https' --tlsv1.2 -fL "$RSPROXY/rustup-init.sh" -o "$TMP"; then
    ok "已下载 rustup-init.sh(rsproxy)"
  else
    warn "rsproxy 下载失败,回退官方 sh.rustup.rs"
    curl --proto '=https' --tlsv1.2 -fL "https://sh.rustup.rs" -o "$TMP" \
      || pause_exit "${RED}✗ rustup-init 下载失败,请检查网络后重试${RST}" 1
  fi
  # -y 非交互;minimal 只装 rustc/cargo/std(最快);稳定版工具链。
  sh "$TMP" -y --profile minimal --default-toolchain stable \
    || pause_exit "${RED}✗ rustup 安装失败${RST}" 1
  rm -f "$TMP" 2>/dev/null || true
  ok "rustup 安装完成"
fi
# 让当前窗口立即可用
[ -f "$CARGO_HOME/env" ] && . "$CARGO_HOME/env"
export PATH="$CARGO_HOME/bin:$PATH"

# ---------------------------------------------------------------------------
# [3/5] 配置 cargo 国内镜像(crates.io → rsproxy sparse)
# ---------------------------------------------------------------------------
step "[3/5] 配置 cargo 国内镜像(拉依赖加速)"
mkdir -p "$CARGO_HOME"
CFG="$CARGO_HOME/config.toml"
if [ -f "$CFG" ] && grep -q "rsproxy" "$CFG" 2>/dev/null; then
  ok "已有 rsproxy 镜像配置,跳过"
else
  [ -f "$CFG" ] && cp "$CFG" "$CFG.bak.$(date +%s)" && warn "已备份原 config.toml"
  cat > "$CFG" <<'EOF'
[source.crates-io]
replace-with = 'rsproxy-sparse'
[source.rsproxy-sparse]
registry = "sparse+https://rsproxy.cn/index/"
[registries.rsproxy]
index = "sparse+https://rsproxy.cn/index/"
[net]
git-fetch-with-cli = true
EOF
  ok "已写入镜像配置:$CFG"
fi

# ---------------------------------------------------------------------------
# [4/5] 持久化 rustup 镜像环境变量(供日后 rustup update 也走国内镜像)
# ---------------------------------------------------------------------------
step "[4/5] 持久化镜像环境变量到 shell 配置"
persist_rc(){
  local rc="$1"; [ -e "$rc" ] || return 0
  if grep -q "RUSTUP_DIST_SERVER" "$rc" 2>/dev/null; then ok "$rc 已含镜像变量"; return 0; fi
  printf '\n# drission / rsproxy Rust 镜像\nexport RUSTUP_DIST_SERVER="%s"\nexport RUSTUP_UPDATE_ROOT="%s/rustup"\n' "$RSPROXY" "$RSPROXY" >> "$rc"
  ok "已更新 $rc"
}
[ -e "$HOME/.zshrc" ] || touch "$HOME/.zshrc"   # macOS 默认 zsh
persist_rc "$HOME/.zshrc"
persist_rc "$HOME/.bashrc"
persist_rc "$HOME/.bash_profile"

# ---------------------------------------------------------------------------
# [5/5] 验证(读取版本号 —— 这一步能打印版本号就算成功)
# ---------------------------------------------------------------------------
step "[5/5] 验证安装(读取版本号)"
FAIL=0
if command -v rustc >/dev/null 2>&1; then ok "rustc:$(rustc --version)"; else err "rustc 未就绪"; FAIL=1; fi
if command -v cargo >/dev/null 2>&1; then ok "cargo:$(cargo --version)"; else err "cargo 未就绪"; FAIL=1; fi

if [ "$FAIL" = 0 ]; then
  printf "\n${GRN}${BLD}🎉 环境配置完成!${RST}\n"
  printf "新开一个终端窗口即可开始用 drission:\n"
  printf "  ${BLD}cargo new demo && cd demo && cargo add drission && cargo run${RST}\n"
  printf "预编译示例(免编译):见 dist/mac 目录。\n"
  pause_exit "${GRN}全部完成。${RST}" 0
else
  pause_exit "${YEL}部分步骤未完成。请新开终端重试,或把以上红色信息发我。${RST}" 1
fi
