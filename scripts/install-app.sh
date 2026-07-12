#!/bin/bash
# 安装 maclean.app 到 /Applications
# 安装前会强制删除所有已存在的 maclean.app 变体，确保只有一个

set -e

PROJECT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
VERSION="0.2.0"
DMG_FILE="$PROJECT_DIR/target/package/maclean-$VERSION.dmg"
MOUNT_DIR="/tmp/maclean-install-$$"
APP_NAME="maclean.app"
TARGET_DIR="/Applications"

if [ ! -f "$DMG_FILE" ]; then
    echo "错误: 未找到 $DMG_FILE，请先运行 make package" >&2
    exit 1
fi

# 取消注册任何可能残留的构建目录下的 maclean.app
for stale in "$PROJECT_DIR/target/package/maclean.app" "$PROJECT_DIR/target/package/payload/Applications/maclean.app"; do
    if [ -e "$stale" ]; then
        /System/Library/Frameworks/CoreServices.framework/Versions/A/Frameworks/LaunchServices.framework/Versions/A/Support/lsregister -u "$stale" 2>/dev/null || true
    fi
done

echo "[1/4] 清理旧版本..."
# 删除任何可能的 maclean.app 变体（包括系统生成的 "maclean 2.app"）
for app in "$TARGET_DIR/maclean.app" "$TARGET_DIR/maclean"\ *.app; do
    if [ -e "$app" ]; then
        echo "  删除旧版本: $app"
        rm -rf "$app"
    fi
done

echo "[2/4] 挂载 DMG..."
mkdir -p "$MOUNT_DIR"
hdiutil attach "$DMG_FILE" -mountpoint "$MOUNT_DIR" -nobrowse -quiet

echo "[3/4] 复制新版本..."
cp -R "$MOUNT_DIR/$APP_NAME" "$TARGET_DIR/"

# 卸载 DMG
hdiutil detach "$MOUNT_DIR" -quiet
rmdir "$MOUNT_DIR" 2>/dev/null || true

# 修复权限和隔离属性
echo "[4/4] 修复权限..."
chmod -R 755 "$TARGET_DIR/$APP_NAME"
xattr -dr com.apple.quarantine "$TARGET_DIR/$APP_NAME" 2>/dev/null || true

# 刷新 LaunchServices
/System/Library/Frameworks/CoreServices.framework/Versions/A/Frameworks/LaunchServices.framework/Versions/A/Support/lsregister -f "$TARGET_DIR/$APP_NAME" 2>/dev/null || true

echo "✓ maclean.app 已安装到 $TARGET_DIR"
