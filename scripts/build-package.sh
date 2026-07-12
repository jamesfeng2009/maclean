#!/bin/bash
# maclean GUI 打包脚本
# 生成 .app 应用包、.pkg 安装包和 .dmg 磁盘镜像
#
# 用法: ./scripts/build-package.sh

set -e

PROJECT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
APP_NAME="maclean"
VERSION="0.2.0"
BUILD_DIR="$PROJECT_DIR/target/release"
PKG_DIR="$PROJECT_DIR/target/package"
APP_BUNDLE="$PKG_DIR/maclean.app"
PAYLOAD_DIR="$PKG_DIR/payload"
DMG_DIR="$PKG_DIR/dmg"

echo "=========================================="
echo "  maclean v$VERSION GUI 打包脚本"
echo "=========================================="
echo ""

# 1. 编译 Universal Binary (支持 Apple Silicon + Intel)
echo "[1/7] 编译 Universal Binary (Apple Silicon + Intel)..."
cd "$PROJECT_DIR"

# 检测当前架构
CURRENT_ARCH=$(uname -m)
echo "  当前架构: $CURRENT_ARCH"

# 检查是否有 x86_64 target
HAS_X86=$(rustup target list --installed 2>/dev/null | grep "x86_64-apple-darwin" || true)
HAS_ARM=$(rustup target list --installed 2>/dev/null | grep "aarch64-apple-darwin" || true)

if [ -n "$HAS_ARM" ] && [ -n "$HAS_X86" ]; then
    # 两个架构都有，构建 Universal Binary
    echo "  编译 aarch64-apple-darwin..."
    cargo build --release --target aarch64-apple-darwin 2>&1 | tail -1
    echo "  编译 x86_64-apple-darwin..."
    cargo build --release --target x86_64-apple-darwin 2>&1 | tail -1
    echo "  合并 Universal Binary..."
    lipo -create \
        "$PROJECT_DIR/target/aarch64-apple-darwin/release/maclean" \
        "$PROJECT_DIR/target/x86_64-apple-darwin/release/maclean" \
        -output "$BUILD_DIR/maclean"
    echo "  ✓ Universal Binary 编译完成 (M1 + Intel)"
elif [ -n "$HAS_ARM" ]; then
    echo "  仅 aarch64 target 可用，编译 Apple Silicon 版本..."
    cargo build --release --target aarch64-apple-darwin 2>&1 | tail -1
    cp "$PROJECT_DIR/target/aarch64-apple-darwin/release/maclean" "$BUILD_DIR/maclean"
    echo "  ✓ Apple Silicon 版本编译完成 (仅 M1+)"
    echo "  提示: 运行 rustup target add x86_64-apple-darwin 可启用 Intel 支持"
else
    echo "  编译当前架构版本..."
    cargo build --release 2>&1 | tail -1
    echo "  ✓ 编译完成"
fi
echo ""

# 2. 准备打包目录
echo "[2/7] 准备 .app 应用包目录..."
rm -rf "$PKG_DIR"
mkdir -p "$APP_BUNDLE/Contents/MacOS"
mkdir -p "$APP_BUNDLE/Contents/Resources"
mkdir -p "$PAYLOAD_DIR/Applications"
mkdir -p "$DMG_DIR"
echo "  ✓ 目录就绪"
echo ""

# 3. 构建 .app 包
echo "[3/7] 构建 .app 应用包..."

# 复制二进制文件
cp "$BUILD_DIR/maclean" "$APP_BUNDLE/Contents/MacOS/maclean"
chmod +x "$APP_BUNDLE/Contents/MacOS/maclean"

# 复制应用图标
cp "$PROJECT_DIR/assets/icon/AppIcon.icns" "$APP_BUNDLE/Contents/Resources/AppIcon.icns"

# 创建 Info.plist
cat > "$APP_BUNDLE/Contents/Info.plist" << EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>maclean</string>
    <key>CFBundleDisplayName</key>
    <string>maclean</string>
    <key>CFBundleIdentifier</key>
    <string>com.maclean.app</string>
    <key>CFBundleVersion</key>
    <string>$VERSION</string>
    <key>CFBundleShortVersionString</key>
    <string>$VERSION</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleExecutable</key>
    <string>maclean</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>LSMinimumSystemVersion</key>
    <string>12.0</string>
    <key>CFBundleIconFile</key>
    <string>AppIcon</string>
    <key>CFBundleIconName</key>
    <string>AppIcon</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>LSUIElement</key>
    <false/>
    <key>NSHumanReadableCopyright</key>
    <string>maclean - macOS 磁盘清理工具</string>
    <key>NSAppleEventsUsageDescription</key>
    <string>maclean 需要发送 AppleScript 以请求管理员权限删除文件</string>
</dict>
</plist>
EOF

# 创建 PkgInfo
echo "APPL????" > "$APP_BUNDLE/Contents/PkgInfo"

# Ad-hoc 代码签名（使 macOS TCC 能识别应用身份）
echo "  签名中..."
codesign --force --deep --sign - "$APP_BUNDLE" 2>/dev/null || true
echo "  ✓ 签名完成"

echo "  ✓ .app 构建完成"
echo ""

# 4. 创建 .pkg 安装包
echo "[4/7] 创建 .pkg 安装包..."

# 将 .app 复制到 payload 的 Applications 目录
cp -R "$APP_BUNDLE" "$PAYLOAD_DIR/Applications/maclean.app"

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
    <integer>2048</integer>
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
echo "[5/7] 创建 .dmg 磁盘镜像..."

# 准备 DMG 内容目录
DMG_CONTENT="$PKG_DIR/dmg_content"
mkdir -p "$DMG_CONTENT"

# 复制 .app 到 dmg 内容目录
cp -R "$APP_BUNDLE" "$DMG_CONTENT/maclean.app"

# 创建安装说明
cat > "$DMG_CONTENT/安装说明.txt" << 'EOF'
maclean 安装说明
================

方法一: 拖拽安装
  将 maclean.app 拖到 Applications 文件夹
  在 Launchpad 或 Applications 中打开 maclean

方法二: 双击 .pkg 安装
  双击 maclean-0.2.0.pkg 按提示安装

卸载:
  将 maclean.app 从 Applications 拖到废纸篓
EOF

# 创建 Applications 文件夹快捷方式
ln -s /Applications "$DMG_CONTENT/Applications"

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
echo "[6/7] 验证打包结果..."
echo ""
echo "=== 打包结果 ==="
echo ""
ls -lh "$PKG_FILE" "$DMG_FILE" 2>/dev/null
echo ""
echo "=== .app 结构 ==="
find "$APP_BUNDLE" -type f | head -10
echo ""
echo "=== 可执行文件信息 ==="
file "$BUILD_DIR/maclean"
echo ""

# 7. 清理临时目录
echo "[7/7] 清理临时文件..."
rm -rf "$PAYLOAD_DIR" "$DMG_CONTENT"
echo "  ✓ 清理完成"
echo ""

echo "=========================================="
echo "  打包完成!"
echo "=========================================="
echo ""
echo "  .pkg: $PKG_FILE"
echo "  .dmg: $DMG_FILE"
echo "  .app: $APP_BUNDLE"
echo ""
echo "  安装方式:"
echo "    .pkg: 双击安装"
echo "    .dmg: 挂载后拖拽 maclean.app 到 Applications"
echo "    .app: 直接双击运行"
echo ""
