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

# 版本号单一来源：根 Cargo.toml 的 [workspace.package] version
VERSION=$(sed -n '/^\[workspace\.package\]/,/^\[/p' Cargo.toml \
    | grep -m1 '^version = "' | cut -d'"' -f2)
if [ -z "$VERSION" ]; then
    echo "错误：无法从 Cargo.toml 的 [workspace.package] 读取版本号" >&2
    exit 1
fi
REPO="${MACLEAN_GITHUB_REPO:-jamesfeng2009/maclean}"
FORMULA="homebrew/maclean.rb"
URL="https://github.com/$REPO/archive/refs/tags/v$VERSION.tar.gz"

echo "=========================================="
echo "  更新 Homebrew formula → v$VERSION"
echo "  $URL"
echo "=========================================="

TMP="$(mktemp -t maclean_sha)"
# github.com 不可达时退回 codeload（同一归档，字节一致，sha256 相同）
if ! curl -fsSL --connect-timeout 10 "$URL" -o "$TMP"; then
    CODELOAD="https://codeload.github.com/$REPO/tar.gz/refs/tags/v$VERSION"
    echo "  github.com 直连失败，改用 codeload: $CODELOAD" >&2
    if ! curl -fsSL --connect-timeout 10 "$CODELOAD" -o "$TMP"; then
        echo "错误：无法下载 $URL" >&2
        echo "      请确认 v$VERSION tag 已推送" >&2
        rm -f "$TMP"
        exit 1
    fi
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
