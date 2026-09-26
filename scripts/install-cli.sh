#!/usr/bin/env bash
# =============================================================================
# maclean CLI 安装脚本
#
# 用法：
#   ./scripts/install-cli.sh                # 构建 release 并安装到 ~/.local/bin
#   ./scripts/install-cli.sh --system       # 安装到 /usr/local/bin（需 sudo）
#   ./scripts/install-cli.sh --bin PATH     # 使用已构建的二进制，跳过 cargo build
#
# 说明：
#   maclean 是"单二进制"设计（GUI + CLI 同一可执行文件）：
#   - 无参数启动 GUI
#   - 带子命令走 CLI（scan/clean/startup/optimize/check-disk ...）
#   因此"独立 CLI crate"不作为单独二进制交付，安装脚本只负责把同一个
#   release 二进制放到 PATH 里。若你确实需要"只有 CLI 的二进制"，
#   可用 --feature 裁剪（见 Cargo.toml 的 feature 定义）。
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
    echo "==> cargo build --release（项目根: $ROOT）"
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

# 提示 PATH
case ":$PATH:" in
    *":$DEST_DIR:"*) ;;
    *)
        echo ""
        echo "提示: $DEST_DIR 不在 PATH 中，可执行:"
        echo "  export PATH=\"$DEST_DIR:\$PATH\""
        ;;
esac
