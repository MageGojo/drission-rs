#!/usr/bin/env bash
# 从 macOS / Linux 交叉编译 drission 到 Windows(x86_64-pc-windows-gnu)。
#
# 为何需要本脚本:`impersonate` feature 用 wreq + BoringSSL(boring-sys2),其 build.rs 在交叉编译时
# 跑 bindgen 解析 BoringSSL 头文件——但 clang 默认不知道 mingw 的 sysroot,会报
# `fatal error: 'sys/types.h' file not found`。本脚本动态探测 mingw sysroot 喂给 bindgen
# (`BINDGEN_EXTRA_CLANG_ARGS`)并指定 mingw 链接器,使整棵树(含 BoringSSL 静态库)能交叉编译并链接出 .exe。
# BoringSSL 的 C/汇编本身用 mingw-w64 + nasm 即可编译,无需改任何代码。
#
# 依赖(macOS):brew install mingw-w64 nasm cmake  +  rustup target add x86_64-pc-windows-gnu
# 依赖(Debian/Ubuntu):apt-get install mingw-w64 nasm cmake  +  rustup target add x86_64-pc-windows-gnu
#
# 用法:
#   scripts/win-cross-build.sh                       # 默认:check --features impersonate
#   scripts/win-cross-build.sh build --features impersonate --example session_tls
#   scripts/win-cross-build.sh build --release --features camoufox,impersonate,cdp
#
# 说明:本脚本只为交叉编译临时设置环境变量,**不**写进 .cargo/config.toml
#       (那会无条件污染本机原生构建的 bindgen)。
set -euo pipefail

TARGET="x86_64-pc-windows-gnu"
CC_PREFIX="x86_64-w64-mingw32"

if ! command -v "${CC_PREFIX}-gcc" >/dev/null 2>&1; then
  echo "✗ 找不到 ${CC_PREFIX}-gcc。请先装 mingw-w64(mac: brew install mingw-w64;deb: apt-get install mingw-w64)。" >&2
  exit 1
fi
if ! command -v nasm >/dev/null 2>&1; then
  echo "✗ 找不到 nasm(BoringSSL x86_64 汇编需要)。请装:brew install nasm / apt-get install nasm。" >&2
  exit 1
fi

SYSROOT="$(${CC_PREFIX}-gcc -print-sysroot)"
# 某些发行版 -print-sysroot 为空;回退到编译器所在前缀的 ../<triple> 布局。
if [ -z "${SYSROOT}" ] || [ ! -d "${SYSROOT}/${CC_PREFIX}/include" ]; then
  GCC_BIN="$(command -v ${CC_PREFIX}-gcc)"
  CAND="$(cd "$(dirname "${GCC_BIN}")/.." && pwd)"
  if [ -d "${CAND}/${CC_PREFIX}/include" ]; then
    SYSROOT="${CAND}"
  fi
fi
INCLUDE="${SYSROOT}/${CC_PREFIX}/include"
if [ ! -f "${INCLUDE}/sys/types.h" ]; then
  echo "✗ 未在 ${INCLUDE} 找到 mingw 头文件(sys/types.h)。请确认 mingw-w64 安装完整。" >&2
  exit 1
fi

export BINDGEN_EXTRA_CLANG_ARGS="--target=${CC_PREFIX} --sysroot=${SYSROOT} -I${INCLUDE}"
export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER="${CC_PREFIX}-gcc"
export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_AR="${CC_PREFIX}-ar"

echo "→ TARGET=${TARGET}"
echo "→ mingw sysroot=${SYSROOT}"
echo "→ BINDGEN_EXTRA_CLANG_ARGS=${BINDGEN_EXTRA_CLANG_ARGS}"

# 默认动作:check --features impersonate;否则用用户给的参数。
if [ "$#" -eq 0 ]; then
  set -- check --features impersonate
fi

exec cargo "$@" --target "${TARGET}"
