#!/bin/bash
# 卸载 maclean.app
# 删除 /Applications 中所有 maclean.app 变体

set -e

TARGET_DIR="/Applications"

echo "正在卸载 maclean.app..."
for app in "$TARGET_DIR/maclean.app" "$TARGET_DIR/maclean"\ *.app; do
    if [ -e "$app" ]; then
        echo "  删除: $app"
        rm -rf "$app"
    fi
done

# 清理用户数据
rm -rf "$HOME/.maclean" 2>/dev/null || true

echo "✓ maclean.app 已卸载"
