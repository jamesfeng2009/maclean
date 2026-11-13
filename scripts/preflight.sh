#!/usr/bin/env bash
# =============================================================================
# 本地质量门禁（替代远端 macOS CI）
#
# 2026-11-13：GitHub Actions 额度按 2000 分钟/月计、macOS runner 10 倍计费，
# 而本机就是 macOS —— 远端再跑一遍 fmt/clippy/test 只是白烧额度。
# 这台机器上跑同一套命令即可，结果等价（差异只有"干净检出 vs 本地工作区"，
# 所以本脚本会先检查 git 工作区是否有未提交改动并提醒）。
#
# 用法：
#   ./scripts/preflight.sh            # fmt + clippy + 全量测试
#   ./scripts/preflight.sh --fast     # 跳过测试（只跑 fmt + clippy）
#
# 发版前必须通过。远端仍保留 Linux 原生编译/测试（本机跑不了）。
# =============================================================================
set -euo pipefail
cd "$(dirname "$0")/.."

FAST=0
[ "${1:-}" = "--fast" ] && FAST=1

step() { printf '\n\033[1m▶ %s\033[0m\n' "$1"; }

if [ -n "$(git status --porcelain)" ]; then
  echo "⚠ 工作区有未提交改动：本地门禁跑的不是将要发布的那个提交。"
  git status --short | head -10
fi

step "cargo fmt --check"
cargo fmt --all -- --check

step "cargo clippy --workspace --all-targets -D warnings"
cargo clippy --workspace --all-targets -- -D warnings

if [ "$FAST" -eq 0 ]; then
  step "cargo test --workspace"
  cargo test --workspace
fi

printf '\n\033[32m✓ preflight 通过\033[0m\n'
