#!/bin/bash
# 安装 maclean.app 到 /Applications
# 安装前会强制删除所有已存在的 maclean.app 变体，确保只有一个
#
# 用法:
#   install-app.sh           安装 arm64 版本 (Apple Silicon 默认)
#   install-app.sh x86_64    安装 Intel 版本
#   install-app.sh universal 安装 Universal 版本
#
# 权限说明：使用 sudo 执行 rm/cp/chmod 操作 /Applications。
# sudo 通过 PAM 框架认证：若系统启用 Touch ID for sudo 则弹 Touch ID，否则弹密码框。
# sudo 会话复用，整个安装只弹一次认证。
# 不要用 osascript "with administrator privileges"——那只弹密码框，不弹 Touch ID。

set -e

PROJECT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
VERSION="0.2.0"
ARCH="${1:-arm64}"
MOUNT_DIR="/tmp/maclean-install-$$"
APP_NAME="maclean.app"
TARGET_DIR="/Applications"

# 根据架构选择 DMG 文件
case "$ARCH" in
    arm64|aarch64)
        DMG_FILE="$PROJECT_DIR/target/package/maclean-$VERSION-arm64.dmg"
        ;;
    x86_64|intel)
        DMG_FILE="$PROJECT_DIR/target/package/maclean-$VERSION-x86_64.dmg"
        ;;
    universal|uni)
        DMG_FILE="$PROJECT_DIR/target/package/maclean-$VERSION.dmg"
        ;;
    *)
        echo "错误: 不支持的架构 '$ARCH'" >&2
        echo "支持的架构: arm64, x86_64, universal" >&2
        exit 1
        ;;
esac

if [ ! -f "$DMG_FILE" ]; then
    echo "错误: 未找到 $DMG_FILE，请先运行 make package" >&2
    exit 1
fi

echo "准备安装 $ARCH 版本: $DMG_FILE"

# 取消注册任何可能残留的构建目录下的 maclean.app
for stale in "$PROJECT_DIR/target/package/maclean.app" "$PROJECT_DIR/target/package/payload/Applications/maclean.app"; do
    if [ -e "$stale" ]; then
        /System/Library/Frameworks/CoreServices.framework/Versions/A/Frameworks/LaunchServices.framework/Versions/A/Support/lsregister -u "$stale" 2>/dev/null || true
    fi
done

echo "[1/4] 清理旧版本..."
# 删除任何可能的 maclean.app 变体（包括系统生成的 "maclean 2.app"）
# 使用 sudo：若系统启用了 Touch ID for sudo，会弹 Touch ID；否则弹密码框
for app in "$TARGET_DIR/maclean.app" "$TARGET_DIR/maclean"\ *.app; do
    if [ -e "$app" ]; then
        echo "  删除旧版本: $app"
        sudo rm -rf "$app"
    fi
done

echo "[2/4] 挂载 DMG..."
mkdir -p "$MOUNT_DIR"
hdiutil attach "$DMG_FILE" -mountpoint "$MOUNT_DIR" -nobrowse -quiet

echo "[3/4] 复制新版本..."
# sudo cp 到 /Applications（sudo 认证只弹一次，复用上面的 sudo 会话）
sudo cp -R "$MOUNT_DIR/$APP_NAME" "$TARGET_DIR/"

# 卸载 DMG
hdiutil detach "$MOUNT_DIR" -quiet
rmdir "$MOUNT_DIR" 2>/dev/null || true

# 修复权限和隔离属性
echo "[4/4] 修复权限..."
sudo chmod -R 755 "$TARGET_DIR/$APP_NAME"
sudo xattr -dr com.apple.quarantine "$TARGET_DIR/$APP_NAME" 2>/dev/null || true

# 刷新 LaunchServices
/System/Library/Frameworks/CoreServices.framework/Versions/A/Frameworks/LaunchServices.framework/Versions/A/Support/lsregister -f "$TARGET_DIR/$APP_NAME" 2>/dev/null || true

echo "✓ maclean.app ($ARCH) 已安装到 $TARGET_DIR"
