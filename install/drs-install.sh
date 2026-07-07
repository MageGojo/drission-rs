#!/usr/bin/env sh
# =============================================================================
# drs · no-Rust installer (download a prebuilt `drs` binary)
#
# For anyone who just wants the `drs` browser MCP / CLI without a Rust toolchain.
# Downloads a prebuilt static binary from GitHub Releases (GitCode mirror as a
# fallback), installs it to ~/.local/bin, and prints the next step (`drs setup`).
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/MageGojo/drission-rs/main/install/drs-install.sh | sh
#
# Env overrides:
#   DRS_VERSION=v0.3.2      install a specific tag (default: latest)
#   DRS_INSTALL_DIR=~/bin   install location  (default: ~/.local/bin)
#   DRS_REPO=owner/name     source repo       (default: MageGojo/drission-rs)
# =============================================================================
set -eu

REPO="${DRS_REPO:-MageGojo/drission-rs}"
GITCODE_REPO="${DRS_GITCODE_REPO:-Roufsi/drission-rs}"
VERSION="${DRS_VERSION:-latest}"
INSTALL_DIR="${DRS_INSTALL_DIR:-$HOME/.local/bin}"

RED=$(printf '\033[31m'); GRN=$(printf '\033[32m'); YEL=$(printf '\033[33m'); CYN=$(printf '\033[36m'); BLD=$(printf '\033[1m'); RST=$(printf '\033[0m')
info() { printf "%s==>%s %s\n" "$CYN$BLD" "$RST" "$*"; }
ok()   { printf "%s✓%s %s\n" "$GRN" "$RST" "$*"; }
warn() { printf "%s!%s %s\n" "$YEL" "$RST" "$*"; }
die()  { printf "%s✗ %s%s\n" "$RED" "$*" "$RST" >&2; exit 1; }

# --- detect platform ---------------------------------------------------------
OS="$(uname -s 2>/dev/null || echo unknown)"
ARCH="$(uname -m 2>/dev/null || echo unknown)"

case "$OS" in
  Darwin) PLATFORM="apple-darwin"; EXT="tar.gz" ;;
  Linux)  PLATFORM="unknown-linux-musl"; EXT="tar.gz" ;;
  MINGW*|MSYS*|CYGWIN*)
    die "Windows detected — use the PowerShell installer instead:\n  irm https://raw.githubusercontent.com/$REPO/main/install/drs-install.ps1 | iex" ;;
  *) die "unsupported OS: $OS" ;;
esac

case "$ARCH" in
  arm64|aarch64) RARCH="aarch64" ;;
  x86_64|amd64)  RARCH="x86_64" ;;
  *) die "unsupported architecture: $ARCH" ;;
esac

TARGET="${RARCH}-${PLATFORM}"
ASSET="drs-${TARGET}.${EXT}"
info "platform: $OS/$ARCH -> $TARGET"

# --- pick a downloader -------------------------------------------------------
if command -v curl >/dev/null 2>&1; then
  DL() { curl -fsSL "$1" -o "$2"; }
elif command -v wget >/dev/null 2>&1; then
  DL() { wget -qO "$2" "$1"; }
else
  die "need curl or wget to download"
fi

if [ "$VERSION" = "latest" ]; then
  GH_URL="https://github.com/$REPO/releases/latest/download/$ASSET"
else
  GH_URL="https://github.com/$REPO/releases/download/$VERSION/$ASSET"
fi
GITCODE_URL="https://gitcode.com/$GITCODE_REPO/releases/download/${VERSION}/$ASSET"

TMP="$(mktemp -d 2>/dev/null || echo "${TMPDIR:-/tmp}/drs-install.$$")"
mkdir -p "$TMP"
trap 'rm -rf "$TMP"' EXIT
TARBALL="$TMP/$ASSET"

info "downloading $ASSET ($VERSION)"
if DL "$GH_URL" "$TARBALL" 2>/dev/null && [ -s "$TARBALL" ]; then
  ok "fetched from GitHub"
elif [ "$VERSION" != "latest" ] && DL "$GITCODE_URL" "$TARBALL" 2>/dev/null && [ -s "$TARBALL" ]; then
  ok "fetched from GitCode mirror"
else
  die "download failed. Check that a release asset named '$ASSET' exists at:\n  $GH_URL\nOr build from source: cargo install --path crates/drission-cli --bin drs"
fi

# --- extract -----------------------------------------------------------------
info "extracting"
tar -xzf "$TARBALL" -C "$TMP" || die "failed to extract $TARBALL"
BIN="$(find "$TMP" -type f -name drs 2>/dev/null | head -n 1)"
[ -n "$BIN" ] || die "no 'drs' binary found inside $ASSET"
chmod +x "$BIN"

# --- install -----------------------------------------------------------------
mkdir -p "$INSTALL_DIR"
mv "$BIN" "$INSTALL_DIR/drs"
ok "installed: $INSTALL_DIR/drs"

# --- PATH hint ---------------------------------------------------------------
case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *)
    warn "$INSTALL_DIR is not on your PATH. Add it, e.g.:"
    printf "    echo 'export PATH=\"%s:\$PATH\"' >> ~/.zshrc && exec zsh\n" "$INSTALL_DIR"
    ;;
esac

printf "\n"
"$INSTALL_DIR/drs" --version 2>/dev/null || true
printf "\n%s%s Next:%s configure the MCP server for Cursor / Codex:\n" "$GRN" "$BLD" "$RST"
printf "    %s/drs setup\n\n" "$INSTALL_DIR"
printf "Then ask your AI agent to use the %sdrs%s browser tools for any hard-to-get web data.\n" "$BLD" "$RST"
