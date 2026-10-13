#!/bin/bash
# 发布 GitHub Release 后，自动回填 homebrew/maclean.rb 的 version 与 sha256。
#
# maclean CLI formula 从源码 tarball 构建（cargo install apps/maclean-cli），
# sha256 对应 GitHub tag 的源码归档，不是预编译二进制。
#
# 用法:
#   ./scripts/update-homebrew-formula.sh
#   MACLEAN_GITHUB_REPO=owner/repo ./scripts/update-homebrew-formula.sh

set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$PROJECT_DIR"

VERSION=$(grep -m1 '^version' apps/maclean-cli/Cargo.toml | cut -d'"' -f2)
REPO="${MACLEAN_GITHUB_REPO:-jamesfeng2009/maclean}"
FORMULA="homebrew/maclean.rb"
URL="https://github.com/$REPO/archive/refs/tags/v$VERSION.tar.gz"

echo "=========================================="
echo "  更新 Homebrew formula → v$VERSION"
echo "  $URL"
echo "=========================================="

TMP="$(mktemp -t maclean_sha)"
if ! curl -fsSL "$URL" -o "$TMP"; then
    echo "错误：无法下载 $URL" >&2
    echo "      请确认 v$VERSION tag 已推送" >&2
    rm -f "$TMP"
    exit 1
fi
SHA=$(shasum -a 256 "$TMP" | awk '{print $1}')
rm -f "$TMP"

python3 - "$FORMULA" "$VERSION" "$SHA" <<'PY'
import re
import sys

path, version, sha = sys.argv[1:4]

with open(path, encoding="utf-8") as f:
    text = f.read()

text = re.sub(r'^\s*url "[^"]+"', f'  url "https://github.com/jamesfeng2009/maclean/archive/refs/tags/v{version}.tar.gz"', text, count=1, flags=re.M)
text = re.sub(r'^\s*sha256 "[0-9a-f]+"', f'  sha256 "{sha}"', text, count=1, flags=re.M)

with open(path, "w", encoding="utf-8") as f:
    f.write(text)
PY

echo "  已更新 $FORMULA → v$VERSION (sha256: $SHA)"
