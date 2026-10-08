#!/bin/bash
# 发布 GitHub Release 后，自动回填 homebrew/maclean.rb 的 version 与 sha256
#
# 背景：sha256 曾以全 0 占位提交，导致 `brew install` 必定校验失败。
# 现在由本脚本从真实产物计算，避免手写出错。
#
# 用法:
#   ./scripts/update-homebrew.sh
#   MACLEAN_GITHUB_REPO=owner/repo ./scripts/update-homebrew.sh   # 指定仓库
#
# 前置条件：v$VERSION 的 GitHub Release 已上传 maclean-$VERSION-{arm64,x86_64}.dmg

set -e

PROJECT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$PROJECT_DIR"

VERSION=$(grep -m1 '^version' apps/maclean-free/Cargo.toml | cut -d'"' -f2)
REPO="${MACLEAN_GITHUB_REPO:-jamesfeng2009/maclean}"
FORMULA="homebrew/maclean.rb"
BASE="https://github.com/$REPO/releases/download/v$VERSION"

echo "=========================================="
echo "  更新 Homebrew formula → v$VERSION"
echo "=========================================="

fetch_sha() {
    local file="$1"
    local url="$BASE/$file"
    local tmp
    tmp="$(mktemp -t maclean_sha)"
    echo "  下载 $file ..."
    if ! curl -fsSL "$url" -o "$tmp"; then
        echo "错误：无法下载 $url" >&2
        echo "      请确认 v$VERSION Release 已发布且包含该产物" >&2
        rm -f "$tmp"
        exit 1
    fi
    shasum -a 256 "$tmp" | awk '{print $1}'
    rm -f "$tmp"
}

ARM_SHA=$(fetch_sha "maclean-$VERSION-arm64.dmg")
INTEL_SHA=$(fetch_sha "maclean-$VERSION-x86_64.dmg")

echo "  arm64  sha256: $ARM_SHA"
echo "  x86_64 sha256: $INTEL_SHA"

python3 - "$FORMULA" "$VERSION" "$ARM_SHA" "$INTEL_SHA" <<'PY'
import re
import sys

path, version, arm_sha, intel_sha = sys.argv[1:5]

with open(path, encoding="utf-8") as f:
    text = f.read()

text = re.sub(r'^\s*version "[^"]+"', f'  version "{version}"', text, count=1, flags=re.M)
text = re.sub(
    r'(on_arm do\s*\n\s*url[^\n]*\n\s*sha256 ")[0-9a-f]+(")',
    lambda m: m.group(1) + arm_sha + m.group(2),
    text,
    count=1,
)
text = re.sub(
    r'(on_intel do\s*\n\s*url[^\n]*\n\s*sha256 ")[0-9a-f]+(")',
    lambda m: m.group(1) + intel_sha + m.group(2),
    text,
    count=1,
)

with open(path, "w", encoding="utf-8") as f:
    f.write(text)
print("  ✅ 已更新 homebrew/maclean.rb")
PY

echo ""
echo "提示：执行 'brew audit --cask --strict ./homebrew/maclean.rb' 做一次校验"
