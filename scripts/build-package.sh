#!/bin/bash
# maclean 打包脚本
# 生成 .pkg 安装包和 .dmg 磁盘镜像
#
# 用法: ./scripts/build-package.sh

set -e

PROJECT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
APP_NAME="maclean"
VERSION="0.1.0"
BUILD_DIR="$PROJECT_DIR/target/release"
PKG_DIR="$PROJECT_DIR/target/package"
PAYLOAD_DIR="$PKG_DIR/payload"
DMG_DIR="$PKG_DIR/dmg"

echo "=========================================="
echo "  maclean v$VERSION 打包脚本"
echo "=========================================="
echo ""

# 1. 编译 release 版本
echo "[1/6] 编译 release 版本..."
cd "$PROJECT_DIR"
cargo build --release
echo "  ✓ 编译完成"
echo ""

# 2. 准备打包目录
echo "[2/6] 准备打包目录..."
rm -rf "$PKG_DIR"
mkdir -p "$PAYLOAD_DIR/usr/local/bin"
mkdir -p "$PAYLOAD_DIR/usr/local/share/doc/maclean"
mkdir -p "$DMG_DIR"
echo "  ✓ 目录就绪"
echo ""

# 3. 复制文件
echo "[3/6] 复制文件..."
cp "$BUILD_DIR/maclean" "$PAYLOAD_DIR/usr/local/bin/maclean"
chmod +x "$PAYLOAD_DIR/usr/local/bin/maclean"

# 创建 README
cat > "$PAYLOAD_DIR/usr/local/share/doc/maclean/README.txt" << 'EOF'
maclean - macOS 磁盘清理 TUI 工具 v0.1.0
==========================================

安装后直接在终端运行: maclean

快捷键:
  Tab/h/l    切换 Tab
  ↑↓/j/k     上下移动
  Space      勾选/取消
  a          全选
  n          取消全选
  d          删除选中项
  r          重新扫描
  q          退出

四个扫描模块:
  1. 开发者缓存 - Rust/Xcode/Simulator/Node/Go/Homebrew/pip/JetBrains
  2. 大文件 - 主目录/Downloads/Desktop 大文件
  3. App缓存 - 微信/飞书/QQ/Telegram 等容器缓存
  4. APFS快照 - Time Machine 快照 + iOS 模拟器运行时

卸载: sudo rm /usr/local/bin/maclean
EOF

echo "  ✓ 文件复制完成"
echo ""

# 4. 创建 .pkg 安装包
echo "[4/6] 创建 .pkg 安装包..."
PKG_FILE="$PKG_DIR/maclean-$VERSION.pkg"

# 创建组件属性列表
cat > "$PKG_DIR/component.plist" << EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleIdentifier</key>
    <string>com.maclean.pkg</string>
    <key>CFBundleVersion</key>
    <string>$VERSION</string>
    <key>IFPkgFlagAllowBackRev</key>
    <false/>
    <key>IFPkgFlagAuthorizationAction</key>
    <string>RootAuthorization</string>
    <key>IFPkgFlagDefaultLocation</key>
    <string>/</string>
    <key>IFPkgFlagFollowLinks</key>
    <true/>
    <key>IFPkgFlagInstallFat</key>
    <false/>
    <key>IFPkgFlagInstalledSize</key>
    <integer>1024</integer>
    <key>IFPkgFlagIsRequired</key>
    <false/>
    <key>IFPkgFlagOverwritePermissions</key>
    <false/>
    <key>IFPkgFlagRelocatable</key>
    <false/>
    <key>IFPkgFlagRestartAction</key>
    <string>NoRestart</string>
    <key>IFPkgFlagRootVolumeOnly</key>
    <true/>
</dict>
</plist>
EOF

pkgbuild \
    --root "$PAYLOAD_DIR" \
    --component-plist "$PKG_DIR/component.plist" \
    --identifier "com.maclean.pkg" \
    --version "$VERSION" \
    --install-location "/" \
    --scripts "$PROJECT_DIR/scripts/pkg-scripts" \
    "$PKG_FILE" 2>/dev/null || pkgbuild \
    --root "$PAYLOAD_DIR" \
    --identifier "com.maclean.pkg" \
    --version "$VERSION" \
    --install-location "/" \
    "$PKG_FILE"

echo "  ✓ .pkg 创建完成: $PKG_FILE"
echo ""

# 5. 创建 .dmg 磁盘镜像
echo "[5/6] 创建 .dmg 磁盘镜像..."

# 准备 DMG 内容目录
DMG_CONTENT="$PKG_DIR/dmg_content"
mkdir -p "$DMG_CONTENT"

# 复制 pkg 到 dmg 内容目录
cp "$PKG_FILE" "$DMG_CONTENT/maclean-$VERSION.pkg"

# 创建安装说明
cat > "$DMG_CONTENT/安装说明.txt" << 'EOF'
maclean 安装说明
================

方法一: 双击 maclean-0.1.0.pkg 安装
  安装后终端运行: maclean

方法二: 命令行安装
  sudo installer -pkg maclean-0.1.0.pkg -target /
  安装后终端运行: maclean

卸载:
  sudo rm /usr/local/bin/maclean
EOF

# 创建 README
cp "$PAYLOAD_DIR/usr/local/share/doc/maclean/README.txt" "$DMG_CONTENT/README.txt"

DMG_FILE="$PKG_DIR/maclean-$VERSION.dmg"

# 用 hdiutil 创建 DMG
hdiutil create \
    -volname "maclean $VERSION" \
    -srcfolder "$DMG_CONTENT" \
    -ov \
    -fs HFS+ \
    -format UDZO \
    "$DMG_FILE" 2>&1 | grep -v "^$" || true

echo "  ✓ .dmg 创建完成: $DMG_FILE"
echo ""

# 6. 验证
echo "[6/6] 验证打包结果..."
echo ""
echo "=== 打包结果 ==="
echo ""
ls -lh "$PKG_FILE" "$DMG_FILE" 2>/dev/null
echo ""
echo "=== 可执行文件信息 ==="
file "$BUILD_DIR/maclean"
echo ""
echo "=== 安装方式 ==="
echo "  .pkg: 双击安装，或 sudo installer -pkg maclean-$VERSION.pkg -target /"
echo "  .dmg: 挂载后拖拽安装"
echo "  运行: 终端输入 maclean"
echo ""
echo "=========================================="
echo "  打包完成!"
echo "=========================================="
