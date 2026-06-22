#!/usr/bin/env bash
# 用 cross 把 drission 编成 **musl 静态二进制**(x86_64 + aarch64),一份跑遍所有 Linux 发行版
# (CentOS7 / Alibaba Cloud Linux / Ubuntu / Debian / Alpine …,不依赖任何 glibc),覆盖国内绝大多数服务器。
#
# 为何用 cross:drission 始终依赖 reqwest(rustls 默认引入 aws-lc-sys 这一 C 依赖),musl 交叉需要
# 目标 C 工具链 + cmake/clang;cross(https://github.com/cross-rs/cross)用 Docker 镜像内置 musl
# 交叉工具链,配仓库根 Cross.toml(pre-build 装 cmake/clang)即可零环境编出完全静态二进制。
#
# 依赖:Docker(cross 用它跑交叉镜像)+ Rust/cargo。脚本会自动 `cargo install cross`(若缺)。
#
# 用法:
#   scripts/linux-musl-build.sh                                   # 默认:--features ocr --example cdp_demo(cdp+ocr)
#   scripts/linux-musl-build.sh --features ocr --example yidun_click   # 编点选示例(cdp+ocr)
#   scripts/linux-musl-build.sh --no-default-features --features camoufox,slider --example quickstart
#   TARGETS="x86_64-unknown-linux-musl" scripts/linux-musl-build.sh   # 只编单架构
set -euo pipefail
cd "$(dirname "$0")/.."

# 目标架构(可用 TARGETS 环境变量覆盖,空格分隔)。aarch64 覆盖鲲鹏/倚天/飞腾/信创 ARM 服务器。
read -r -a TARGETS <<<"${TARGETS:-x86_64-unknown-linux-musl aarch64-unknown-linux-musl}"

# 构建参数(可传参覆盖);默认编 lib + cdp_demo 示例(cdp 默认后端 + ocr 纯 Rust)。
ARGS=("$@")
if [ "${#ARGS[@]}" -eq 0 ]; then
  ARGS=(--features ocr --example cdp_demo)
fi

if ! command -v docker >/dev/null 2>&1; then
  echo "✗ 找不到 docker。cross 需要 Docker 跑 musl 交叉镜像,请先安装并启动 Docker。" >&2
  exit 1
fi
if ! command -v cross >/dev/null 2>&1; then
  echo "→ 未发现 cross,正在安装(cargo install cross --locked)…"
  cargo install cross --locked
fi

for t in "${TARGETS[@]}"; do
  echo ""
  echo "========================================================"
  echo "→ 编译 musl 静态:$t   ${ARGS[*]}"
  echo "========================================================"
  rustup target add "$t" >/dev/null 2>&1 || true
  cross build --release --target "$t" "${ARGS[@]}"

  # 列出并验证产物是否「完全静态链接」(跨发行版通用的关键)。
  outdir="target/$t/release"
  echo "→ 产物($outdir,含 examples/):"
  while IFS= read -r bin; do
    [ -n "$bin" ] || continue
    desc="$(file "$bin")"
    if printf '%s' "$desc" | grep -q "statically linked"; then
      printf '   ✅ %s  [静态·通用]\n' "$bin"
    else
      printf '   ⚠  %s  [非静态]\n' "$bin"
    fi
  done < <(find "$outdir" -maxdepth 2 -type f -perm -u+x ! -name '*.d' ! -name '*.so' 2>/dev/null | sort)
done

echo ""
echo "完成。把上面标 [静态·通用] 的二进制直接拷到任意 Linux 服务器即可运行"
echo "(运行时仍需目标机有浏览器:Chrome/Chromium 或让 drission 自动下载/自带 Chrome for Testing)。"
