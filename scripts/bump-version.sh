#!/bin/bash
# 统一 bump workspace 版本号（唯一需要改 Cargo.toml 的地方）。
#
# 为什么需要这个脚本：
#   [workspace.package] version 管成员包自身版本，
#   [workspace.dependencies] 里内部 crate 的 version 管「发布时怎么在 registry
#   上解析依赖」—— 后者不能写成 workspace 继承，必须是字面量。
#   两处必须完全相等，否则 cargo publish 会拿着旧版本号去要一个不存在的依赖。
#
# 用法:
#   ./scripts/bump-version.sh 0.4.3
#
# 执行后仍然需要：
#   1. git diff 确认改动只涉及 Cargo.toml / Cargo.lock
#   2. 提交 + 打 tag（推送 tag 会触发 release-macos.yml 全套发布）

set -euo pipefail

NEW="${1:-}"
if [ -z "$NEW" ]; then
    echo "用法: $0 <新版本号，如 0.4.3>" >&2
    exit 2
fi

if ! printf '%s' "$NEW" | grep -qE '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$'; then
    echo "错误：'$NEW' 不是合法的 semver（期望 x.y.z）" >&2
    exit 2
fi

PROJECT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$PROJECT_DIR"

python3 - "$NEW" <<'PY'
import re
import sys

new = sys.argv[1]
path = "Cargo.toml"

with open(path, encoding="utf-8") as f:
    lines = f.read().splitlines(keepends=True)


def bump_section(lines, header, pattern):
    """在 header 段内替换匹配 pattern 的第一行，返回 (lines, count)。"""
    try:
        start = next(i for i, l in enumerate(lines) if l.strip() == header)
    except StopIteration:
        return lines, 0
    for i in range(start + 1, len(lines)):
        if lines[i].startswith("["):
            return lines, 0
        m = re.match(pattern, lines[i])
        if m:
            lines[i] = m.group(1) + new + m.group(3) + "\n"
            return lines, 1
    return lines, 0


lines, n1 = bump_section(
    lines, "[workspace.package]", r'^(version = ")([^"]*)(")$'
)

# [workspace.dependencies] 里所有内部 crate：path 后面必须跟着 version
start = next(
    (i for i, l in enumerate(lines) if l.strip() == "[workspace.dependencies]"), None
)
if start is None:
    sys.exit("错误：Cargo.toml 缺少 [workspace.dependencies] 段")

n2 = 0
i = start + 1
while i < len(lines) and not lines[i].startswith("["):
    m = re.match(
        r'^(maclean[\w-]* = \{ path = "[^"]+", version = ")([^"]*)(" \})$',
        lines[i],
    )
    if m:
        lines[i] = m.group(1) + new + m.group(3) + "\n"
        n2 += 1
    i += 1

if n1 != 1:
    sys.exit("错误：未能在 [workspace.package] 里找到唯一的 version 字段")
if n2 == 0:
    sys.exit("错误：未能在 [workspace.dependencies] 里找到内部 crate 依赖")

with open(path, "w", encoding="utf-8") as f:
    f.writelines(lines)

print(f"  [workspace.package] version -> {new}")
print(f"  [workspace.dependencies] 内部 crate 同步 {n2} 处")
PY

# 刷新 Cargo.lock 里的包版本（其余依赖版本保持不变）
cargo metadata --format-version 1 >/dev/null 2>&1 || true

echo "=========================================="
echo "  版本已 bump 到 $NEW"
echo "  下一步: git diff -- Cargo.toml Cargo.lock"
echo "          git tag -a v$NEW -m v$NEW && git push origin main --tags"
echo "=========================================="
