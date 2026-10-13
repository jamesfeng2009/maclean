#!/usr/bin/env bash
# =============================================================================
# maclean CLI 安装脚本（2026-10-13 商业化边界调整）
#
# 公开仓库 = 纯 CLI（永久免费 · Apache-2.0 开源）。
# maclean 二进制就是 CLI 本身：无参数运行提示用法，带子命令走
# scan / clean / schedule / check-disk / list / apps / uninstall / log /
# backups / restore / startup / optimize / dup-ignore。
#
# 用法：
#   ./scripts/install-cli.sh                # 构建 release 并安装到 ~/.local/bin
#   ./scripts/install-cli.sh --system       # 安装到 /usr/local/bin（需 sudo）
#   ./scripts/install-cli.sh --bin PATH     # 使用已构建的二进制，跳过 cargo build
#
# GUI App（14 天试用 / 付费）位于私有 maclean-pro 仓库，与本脚本无关。
# =============================================================================
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN_NAME="maclean"
DEST_DIR="${HOME}/.local/bin"
SKIP_BUILD=0

while [[ $# -gt 0 ]]; do
    case "$1" in
        --system)
            DEST_DIR="/usr/local/bin"
            shift
            ;;
        --bin)
            SKIP_BUILD=1
            BIN_PATH="$2"
            shift 2
            ;;
        *)
            echo "未知参数: $1"
            exit 2
            ;;
    esac
done

if [[ "$SKIP_BUILD" == "1" ]]; then
    if [[ ! -x "$BIN_PATH" ]]; then
        echo "错误: $BIN_PATH 不是可执行文件"
        exit 1
    fi
else
    echo "==> cargo build --release（纯 CLI，无 GUI / License 依赖）"
    (cd "$ROOT" && cargo build --release)
    BIN_PATH="$ROOT/target/release/$BIN_NAME"
fi

mkdir -p "$DEST_DIR"
cp "$BIN_PATH" "$DEST_DIR/$BIN_NAME"

# macOS 上从网络下载/未签名二进制需移除隔离属性，否则 Gatekeeper 会拦截
if [[ "$(uname -s)" == "Darwin" ]]; then
    xattr -dr com.apple.quarantine "$DEST_DIR/$BIN_NAME" 2>/dev/null || true
fi

echo "==> 已安装到 $DEST_DIR/$BIN_NAME"
echo "    验证: $DEST_DIR/$BIN_NAME --help"
echo "    形态: 纯 CLI（无 GUI；未带子命令时提示用法）"

# 提示 PATH
case ":$PATH:" in
    *":$DEST_DIR:"*) ;;
    *)
        echo ""
        echo "提示: $DEST_DIR 不在 PATH 中，可执行:"
        echo "  export PATH=\"$DEST_DIR:\$PATH\""
        ;;
esac
