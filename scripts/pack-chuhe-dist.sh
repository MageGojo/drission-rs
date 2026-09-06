#!/usr/bin/env bash
# 把 chuhe_price + 内置 Lightpanda 打进 dist/chuhe，给人自己跑。
# 用法：scripts/pack-chuhe-dist.sh
set -euo pipefail
cd "$(dirname "$0")/.."

DEST="dist/chuhe"
mkdir -p "$DEST"

echo "→ 编译 chuhe_price (release)"
cargo build --release --example chuhe_price --features cdp,ocr

BIN="target/release/examples/chuhe_price"
if [[ ! -f "$BIN" ]]; then
  echo "✗ 找不到 $BIN" >&2
  exit 1
fi
cp "$BIN" "$DEST/chuhe_price"
chmod +x "$DEST/chuhe_price"

LP="${LIGHTPANDA_BIN:-}"
if [[ -z "$LP" ]]; then
  for cand in \
    "$DEST/lightpanda" \
    "$HOME/.cache/drission/lightpanda/nightly/aarch64-macos/lightpanda" \
    "$HOME/.cache/drission/lightpanda/nightly/x86_64-macos/lightpanda" \
    "$HOME/.cache/drission/lightpanda/nightly/x86_64-linux/lightpanda" \
    "$HOME/.cache/drission/lightpanda/nightly/aarch64-linux/lightpanda" \
    /opt/homebrew/bin/lightpanda \
    /usr/local/bin/lightpanda
  do
    if [[ -f "$cand" && "$cand" != "$DEST/lightpanda" ]]; then
      LP="$cand"
      break
    fi
  done
fi
if [[ -z "${LP}" || ! -f "$LP" ]]; then
  echo "✗ 找不到 Lightpanda。设 LIGHTPANDA_BIN，或先 cargo run --example lightpanda_demo 让库下载 nightly。" >&2
  exit 1
fi
cp "$LP" "$DEST/lightpanda"
chmod +x "$DEST/lightpanda"

python3 - <<'PY'
import json, pathlib
root = pathlib.Path("examples/sites")
dest = pathlib.Path("dist/chuhe")
src = dest / "chuhe.config.json"
template = root / "chuhe.config.json"
local = root / "chuhe.config.local.json"
raw = json.loads((local if local.is_file() else template).read_text(encoding="utf-8"))
raw["engine"] = "lightpanda"
raw["profile_dir"] = "profile"
raw["seconds"] = 0
# 模板始终不含密钥；有本地配置才写 local（dist 已 gitignore）
(dest / "chuhe.config.json").write_text(
    json.dumps({
        "url": raw.get("url", ""),
        "username": "",
        "password": "",
        "engine": "lightpanda",
        "webhook": raw.get("webhook", ""),
        "seconds": 0,
        "reload_secs": raw.get("reload_secs", 30),
        "profile_dir": "profile",
    }, ensure_ascii=False, indent=2) + "\n",
    encoding="utf-8",
)
if local.is_file():
    (dest / "chuhe.config.local.json").write_text(
        json.dumps(raw, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )
    print("→ 已带上本地配置（dist 不入库）")
else:
    print("→ 没有 chuhe.config.local.json，请自己在 dist/chuhe 里填账号")
PY

if [[ -f target/chuhe-profile/storage.json ]]; then
  mkdir -p "$DEST/profile"
  cp target/chuhe-profile/storage.json "$DEST/profile/storage.json"
  echo "→ 已带上登录态 profile/storage.json"
fi

cat > "$DEST/怎么跑.txt" <<'EOF'
初和报价 · 自己跑（内置 Lightpanda，不用本机 Chrome）

1. 同目录必须有：
   chuhe_price
   lightpanda
   chuhe.config.local.json   （填 username / password；已有登录态可只留 profile/storage.json）

2. 在这一层执行：

   cd dist/chuhe
   ./chuhe_price

3. 听完一轮会写：
   data/chuhe/models.csv
   data/chuhe/prices.csv
   然后退出。

重新打包：在仓库根目录跑 scripts/pack-chuhe-dist.sh
EOF

echo
echo "==== $DEST ===="
ls -lh "$DEST/chuhe_price" "$DEST/lightpanda" "$DEST"/*.json "$DEST/怎么跑.txt" 2>/dev/null || true
echo
echo "你自己跑：  cd $DEST && ./chuhe_price"
