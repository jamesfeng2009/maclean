#!/bin/bash
# =============================================================================
# maclean CLI 发行产物打包脚本（2026-10-13）
#
# 公开仓库 = 纯 CLI。产物为平台压缩包 + SHA256 校验和，供 GitHub Release
# 使用。不生成 .app / .pkg / .dmg（GUI App 打包在私有 maclean-pro 仓库）。
#
# 用法:
#   ./scripts/build-cli-package.sh            # 当前平台
#   MACLEAN_TARGETS="aarch64-apple-darwin x86_64-apple-darwin" \
#     ./scripts/build-cli-package.sh          # 交叉构建多个 target
#
# 产物输出到 target/package/：
#   maclean-<version>-<target>.tar.gz
#   maclean-<version>-<target>.tar.gz.sha256
# =============================================================================
set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$PROJECT_DIR"

# 版本号单一来源：Cargo.toml
VERSION=$(grep -m1 '^version' apps/maclean-cli/Cargo.toml | cut -d'"' -f2)
if [ -z "$VERSION" ]; then
    echo "错误：无法从 apps/maclean-cli/Cargo.toml 读取版本号" >&2
    exit 1
fi

HOST_TARGET="$(rustc -vV | sed -n 's/^host: //p')"
TARGETS="${MACLEAN_TARGETS:-$HOST_TARGET}"
PKG_DIR="$PROJECT_DIR/target/package"
mkdir -p "$PKG_DIR"

for TARGET in $TARGETS; do
    echo "=========================================="
    echo "  maclean v$VERSION · $TARGET"
    echo "=========================================="

    if [ "$TARGET" = "$HOST_TARGET" ]; then
        cargo build --release -p maclean
        BIN="target/release/maclean"
    else
        cargo build --release -p maclean --target "$TARGET"
        BIN="target/$TARGET/release/maclean"
    fi

    if [ ! -x "$BIN" ]; then
        echo "错误：未找到构建产物 $BIN" >&2
        exit 1
    fi

    # 复制到临时目录，以 maclean 为文件名打包（去掉 target 名冲突）
    STAGE="$(mktemp -d)"
    cp "$BIN" "$STAGE/maclean"
    if [ "$(uname -s)" = "Darwin" ]; then
        # macOS：移除隔离属性避免 Gatekeeper 拦截
        xattr -dr com.apple.quarantine "$STAGE/maclean" 2>/dev/null || true
    fi

    ARCHIVE="$PKG_DIR/maclean-$VERSION-$TARGET.tar.gz"
    tar -C "$STAGE" -czf "$ARCHIVE" maclean
    shasum -a 256 "$ARCHIVE" | awk '{print $1}' > "$ARCHIVE.sha256"
    rm -rf "$STAGE"

    echo "  产物: $ARCHIVE"
    echo "  sha256: $(cat "$ARCHIVE.sha256")"
done

echo "=========================================="
echo "  完成。产物目录: $PKG_DIR"
echo "=========================================="
