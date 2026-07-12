#!/bin/bash
# 安装 maclean.app 到 /Applications
# 安装前会强制删除所有已存在的 maclean.app 变体，确保只有一个

set -e

PROJECT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
APP_NAME="maclean.app"
SOURCE_APP="$PROJECT_DIR/target/package/$APP_NAME"
TARGET_DIR="/Applications"

if [ ! -d "$SOURCE_APP" ]; then
    echo "错误: 未找到 $SOURCE_APP，请先运行 make package" >&2
    exit 1
fi

echo "[1/3] 清理旧版本..."
# 删除任何可能的 maclean.app 变体（包括系统生成的 "maclean 2.app"）
for app in "$TARGET_DIR/maclean.app" "$TARGET_DIR/maclean"\ *.app; do
    if [ -e "$app" ]; then
        echo "  删除旧版本: $app"
        rm -rf "$app"
    fi
done

echo "[2/3] 复制新版本..."
cp -R "$SOURCE_APP" "$TARGET_DIR/"

# 修复权限和隔离属性
echo "[3/3] 修复权限..."
chmod -R 755 "$TARGET_DIR/$APP_NAME"
xattr -dr com.apple.quarantine "$TARGET_DIR/$APP_NAME" 2>/dev/null || true

# 刷新 LaunchServices
/System/Library/Frameworks/CoreServices.framework/Versions/A/Frameworks/LaunchServices.framework/Versions/A/Support/lsregister -f "$TARGET_DIR/$APP_NAME" 2>/dev/null || true

echo "✓ maclean.app 已安装到 $TARGET_DIR"
