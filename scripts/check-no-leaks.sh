#!/bin/bash
# H-06 / G-6：CI 防泄漏 grep（关闭 gitleaks 官方 action 后的仓库内替代）。
# 关闭理由见 ci.yml 的 security job 注释与 CONTRIBUTING.md：gitleaks-action 组织版
# 需 GITLEAKS_LICENSE 授权（付费）且引入第三方 action 有供应链风险；本公开仓库
# 无密钥历史，用可审计的 grep 覆盖已知模式即可。失败即阻塞（CI 无 continue-on-error）。
#
# 只扫源码/配置（.rs/.toml/.sh/.yml/.yaml），排除本脚本自身与 *.md
# （文档里合法讨论这些字符串）。模式用拼接构造，避免脚本被自己命中。
# CORE-04：仓库根目录的 Cargo.toml / Cargo.lock / rust-toolchain.toml /
# deny.toml / Makefile 同样要扫 —— 根 Cargo.toml 恰是最可能写私有依赖的位置。
set -euo pipefail

cd "$(dirname "$0")/.."

SELF="scripts/$(basename "$0")"

P_STRIPE='sk_(live|test)_''[A-Za-z0-9]{16,}'   # Stripe secret key 字面量
P_AWS='AKIA''[0-9A-Z]{16}'                     # AWS access key id
P_PRIV='-----BEGIN ''[A-Z ]*PRIVATE KEY-----'  # 私钥 PEM 头
# CORE-04：旧正则要求连字符后缀，`maclean-pro = "..."` 不命中。
P_PRODEP='^maclean-pro(-[A-Za-z0-9_-]*)? *='   # Cargo.toml 私有 crate 依赖声明
P_PROUSE='maclean_pro[a-z0-9_]*::'             # use 私有 crate
P_TEAMURL='license\.getmaclean\.com'           # Team 服务端硬编码 URL

PATTERN="$P_STRIPE|$P_AWS|$P_PRIV|$P_PRODEP|$P_PROUSE|$P_TEAMURL"

# 待扫文件集合：子目录递归 + 仓库根目录一级（二进制/图片/macOS 元数据一律不扫）。
# 用 NUL 分隔，避免路径里的空格把 xargs 拆开。
scan_files() {
    find apps crates scripts .github -type f \
        \( -name '*.rs' -o -name '*.toml' -o -name '*.sh' \
           -o -name '*.yml' -o -name '*.yaml' \) \
        ! -path "$SELF" -print0
    find . -maxdepth 1 -type f \
        \( -name '*.toml' -o -name '*.rs' -o -name '*.sh' \
           -o -name '*.yml' -o -name '*.yaml' \
           -o -name 'Cargo.lock' -o -name 'Makefile' -o -name '*.rb' \) \
        -print0
}

HITS=$(scan_files | xargs -0 grep -EnH "$PATTERN" 2>/dev/null || true)

if [ -n "$HITS" ]; then
    echo "::error::防泄漏扫描命中可疑模式（密钥 / 私有依赖 / Team URL），禁止提交：" >&2
    printf '%s\n' "$HITS" >&2
    exit 1
fi

COUNT=$(scan_files | tr '\0' '\n' | grep -c . || true)
echo "防泄漏扫描通过：${COUNT} 个文件，0 命中"
