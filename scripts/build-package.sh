#!/bin/bash
# maclean GUI 打包脚本
# 生成 .app 应用包、.pkg 安装包和 .dmg 磁盘镜像
# 同时生成 Universal Binary 以及独立的 arm64 / x86_64 版本
#
# 用法: ./scripts/build-package.sh

set -e

PROJECT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
APP_NAME="maclean"
# 版本号单一来源：Cargo.toml
# 不要在这里手写版本号——Cargo.toml / Makefile / 本脚本曾出现 0.2.0 / 0.1.0 / 0.2.0 三处不一致
VERSION=$(grep -m1 '^version' "$PROJECT_DIR/Cargo.toml" | cut -d'"' -f2)
if [ -z "$VERSION" ]; then
    echo "错误：无法从 Cargo.toml 读取版本号" >&2
    exit 1
fi
BUILD_DIR="$PROJECT_DIR/target/release"
PKG_DIR="$PROJECT_DIR/target/package"
APP_BUNDLE="$PKG_DIR/maclean.app"
PAYLOAD_DIR="$PKG_DIR/payload"
DMG_DIR="$PKG_DIR/dmg"

# ============================================================
#  代码签名与公证
# ============================================================
# 未配置证书时退化为 ad-hoc 签名（本机可用，但 Gatekeeper 会拦截分发）。
# 正式发布请设置以下环境变量：
#   MACLEAN_SIGN_IDENTITY   Developer ID Application 证书名
#                           例: "Developer ID Application: Foo (TEAMID12345)"
#   MACLEAN_NOTARY_PROFILE  notarytool 的 keychain profile（推荐）
#   APPLE_ID / TEAM_ID / APP_PASSWORD   未配置 profile 时使用
notarize_app() {
    local app="$1"

    if [ -z "${MACLEAN_SIGN_IDENTITY:-}" ]; then
        echo "  ⚠ 未设置 MACLEAN_SIGN_IDENTITY，仅做 ad-hoc 签名"
        echo "    （分发给用户时 Gatekeeper 会拦截；正式发布需 Developer ID 签名 + 公证）"
        codesign --force --deep --sign - "$app" 2>/dev/null || true
        return 0
    fi

    echo "  签名: $MACLEAN_SIGN_IDENTITY"
    codesign --force --deep --options runtime --timestamp \
        --sign "$MACLEAN_SIGN_IDENTITY" "$app"

    if [ -z "${MACLEAN_NOTARY_PROFILE:-}" ] && { [ -z "${APPLE_ID:-}" ] || [ -z "${TEAM_ID:-}" ] || [ -z "${APP_PASSWORD:-}" ]; }; then
        echo "  ⚠ 未提供公证凭据（MACLEAN_NOTARY_PROFILE 或 APPLE_ID+TEAM_ID+APP_PASSWORD），跳过公证"
        return 0
    fi

    local zip_path submit_rc=0
    zip_path="$(mktemp -t maclean_notarize).zip"
    echo "  提交公证..."
    ditto -c -k --keepParent "$app" "$zip_path"

    if [ -n "${MACLEAN_NOTARY_PROFILE:-}" ]; then
        xcrun notarytool submit "$zip_path" \
            --keychain-profile "$MACLEAN_NOTARY_PROFILE" --wait || submit_rc=$?
    else
        xcrun notarytool submit "$zip_path" \
            --apple-id "$APPLE_ID" --team-id "$TEAM_ID" --password "$APP_PASSWORD" \
            --wait || submit_rc=$?
    fi

    if [ "$submit_rc" -eq 0 ]; then
        xcrun stapler staple "$app" || true
        echo "  ✅ 公证完成并已 staple"
    else
        echo "  ⚠ 公证失败（退出码 $submit_rc），产物未经公证，Gatekeeper 会拦截" >&2
    fi

    rm -f "$zip_path"
    return 0
}

echo "=========================================="
echo "  maclean v$VERSION GUI 打包脚本"
echo "=========================================="
echo ""

# 检测已安装的 target
HAS_X86=$(rustup target list --installed 2>/dev/null | grep "x86_64-apple-darwin" || true)
HAS_ARM=$(rustup target list --installed 2>/dev/null | grep "aarch64-apple-darwin" || true)

# 检测当前架构
CURRENT_ARCH=$(uname -m)
echo "  当前架构: $CURRENT_ARCH"

# 1. 编译各架构二进制
echo "[1/7] 编译二进制..."
cd "$PROJECT_DIR"

if [ -n "$HAS_ARM" ]; then
    echo "  编译 aarch64-apple-darwin..."
    cargo build --release --target aarch64-apple-darwin 2>&1 | tail -1
fi

if [ -n "$HAS_X86" ]; then
    echo "  编译 x86_64-apple-darwin..."
    cargo build --release --target x86_64-apple-darwin 2>&1 | tail -1
fi

# 2. 准备通用二进制
echo ""
echo "[2/7] 准备二进制..."

mkdir -p "$BUILD_DIR"

if [ -n "$HAS_ARM" ] && [ -n "$HAS_X86" ]; then
    echo "  合并 Universal Binary..."
    lipo -create \
        "$PROJECT_DIR/target/aarch64-apple-darwin/release/maclean" \
        "$PROJECT_DIR/target/x86_64-apple-darwin/release/maclean" \
        -output "$BUILD_DIR/maclean"
    echo "  ✓ Universal Binary 编译完成 (M1 + Intel)"
elif [ -n "$HAS_ARM" ]; then
    cp "$PROJECT_DIR/target/aarch64-apple-darwin/release/maclean" "$BUILD_DIR/maclean"
    echo "  ✓ Apple Silicon 版本编译完成 (仅 M1+)"
else
    cargo build --release 2>&1 | tail -1
    echo "  ✓ 当前架构版本编译完成"
fi
echo ""

# 3. 准备打包目录
echo "[3/7] 准备 .app 应用包目录..."
rm -rf "$PKG_DIR"
mkdir -p "$APP_BUNDLE/Contents/MacOS"
mkdir -p "$APP_BUNDLE/Contents/Resources"
mkdir -p "$PAYLOAD_DIR/Applications"
mkdir -p "$DMG_DIR"
echo "  ✓ 目录就绪"
echo ""

# 4. 创建 Info.plist
cat > "$APP_BUNDLE/Contents/Info.plist" << EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>Maclean</string>
    <key>CFBundleDisplayName</key>
    <string>Maclean</string>
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

# 复制应用图标
cp "$PROJECT_DIR/assets/icon/AppIcon.icns" "$APP_BUNDLE/Contents/Resources/AppIcon.icns"

# 5. 创建组件属性列表
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

# 6. 为指定架构构建 .app、.pkg、.dmg
build_for_arch() {
    local arch_label="$1"
    local binary_path="$2"
    local suffix="$3"

    echo "  ----------------------------------------"
    echo "  构建 $arch_label 版本..."

    # 复制二进制
    cp "$binary_path" "$APP_BUNDLE/Contents/MacOS/maclean"
    chmod +x "$APP_BUNDLE/Contents/MacOS/maclean"

    # 代码签名 + 公证（见脚本顶部 notarize_app 说明）
    notarize_app "$APP_BUNDLE"

    # .pkg
    rm -rf "$PAYLOAD_DIR/Applications/maclean.app"
    cp -R "$APP_BUNDLE" "$PAYLOAD_DIR/Applications/maclean.app"

    local pkg_file="$PKG_DIR/maclean-$VERSION$suffix.pkg"
    pkgbuild \
        --root "$PAYLOAD_DIR" \
        --component-plist "$PKG_DIR/component.plist" \
        --identifier "com.maclean.pkg" \
        --version "$VERSION" \
        --install-location "/" \
        --scripts "$PROJECT_DIR/scripts/pkg-scripts" \
        "$pkg_file" 2>/dev/null || pkgbuild \
        --root "$PAYLOAD_DIR" \
        --identifier "com.maclean.pkg" \
        --version "$VERSION" \
        --install-location "/" \
        "$pkg_file"

    # .dmg
    local dmg_content="$PKG_DIR/dmg_content$suffix"
    rm -rf "$dmg_content"
    mkdir -p "$dmg_content"
    cp -R "$APP_BUNDLE" "$dmg_content/maclean.app"

    cat > "$dmg_content/安装说明.txt" << EOF
maclean 安装说明
================

方法一: 拖拽安装
  将 maclean.app 拖到 Applications 文件夹
  在 Launchpad 或 Applications 中打开 maclean

方法二: 双击 .pkg 安装
  双击 maclean-$VERSION.pkg 按提示安装

卸载:
  将 maclean.app 从 Applications 拖到废纸篓
EOF

    ln -s /Applications "$dmg_content/Applications"

    local dmg_file="$PKG_DIR/maclean-$VERSION$suffix.dmg"
    hdiutil create \
        -volname "maclean $VERSION $arch_label" \
        -srcfolder "$dmg_content" \
        -ov \
        -fs HFS+ \
        -format UDZO \
        "$dmg_file" 2>&1 | grep -v "^$" || true

    echo "    .pkg: $pkg_file"
    echo "    .dmg: $dmg_file"
}

echo "[4/7] 创建各架构安装包..."
echo ""

# Universal Binary
build_for_arch "Universal" "$BUILD_DIR/maclean" ""

# arm64 独立包
if [ -n "$HAS_ARM" ]; then
    build_for_arch "Apple Silicon" "$PROJECT_DIR/target/aarch64-apple-darwin/release/maclean" "-arm64"
fi

# x86_64 独立包
if [ -n "$HAS_X86" ]; then
    build_for_arch "Intel" "$PROJECT_DIR/target/x86_64-apple-darwin/release/maclean" "-x86_64"
fi

echo ""

# 7. 验证
echo "[5/7] 验证打包结果..."
echo ""
echo "=== 打包结果 ==="
echo ""
ls -lh "$PKG_DIR"/*.pkg "$PKG_DIR"/*.dmg 2>/dev/null
echo ""
echo "=== 可执行文件信息 ==="
file "$BUILD_DIR/maclean"
echo ""

# 8. 清理临时目录
echo "[6/7] 清理临时文件..."
rm -rf "$PAYLOAD_DIR" "$PKG_DIR"/dmg_content*

# 取消注册构建产物 maclean.app，避免 LaunchServices 中显示重复图标
if [ -d "$APP_BUNDLE" ]; then
    /System/Library/Frameworks/CoreServices.framework/Versions/A/Frameworks/LaunchServices.framework/Versions/A/Support/lsregister -u "$APP_BUNDLE" 2>/dev/null || true
    rm -rf "$APP_BUNDLE"
fi

echo "  ✓ 清理完成"
echo ""

echo "=========================================="
echo "  打包完成!"
echo "=========================================="
echo ""
ls -lh "$PKG_DIR"/*.pkg "$PKG_DIR"/*.dmg 2>/dev/null
echo ""
echo "  安装方式:"
echo "    .pkg: 双击安装"
echo "    .dmg: 挂载后拖拽 maclean.app 到 Applications"
echo ""

# 输出各产物的 sha256 —— 发布 GitHub Release 后需填入 homebrew/maclean.rb
echo "  产物 sha256（发布后填入 homebrew/maclean.rb）:"
for f in "$PKG_DIR"/*.pkg "$PKG_DIR"/*.dmg; do
    [ -f "$f" ] || continue
    echo "    $(shasum -a 256 "$f" | awk '{print $1}')  $(basename "$f")"
done
echo ""
if [ -z "${MACLEAN_SIGN_IDENTITY:-}" ]; then
    echo "  ⚠ 本次为 ad-hoc 签名，未公证；正式发布请设置 MACLEAN_SIGN_IDENTITY 与公证凭据"
    echo ""
fi
