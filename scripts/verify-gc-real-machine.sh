#!/usr/bin/env bash
# 真机验证：清理崩溃残留 GC 的 P3 场景 + review 表格里「需要权限/外部 runner」的部分。
#
#   scripts/verify-gc-real-machine.sh
#
# 覆盖：
#   1. 真 SIGKILL（kill -9）后残留清零 / df -k 可用量上升 / 日志页收尾记录
#      —— 由 crates/maclean-core/tests/real_machine_gc.rs 的 #[ignore] 用例实跑
#   2. 权限失败路径：chflags immutable（无需 sudo）/ root 拥有项（sudo -n）
#      —— 走真实 maclean 二进制的命令入口 recovery，不是单测夹具
#   3. TCC 受保护目录：环境探测 + 授权指引（不在真实系统目录里造垃圾）
#   4. 30s 预算：注入后的确定性回归
#
# 刻意不做的假绿：mdfind 对 `.maclean_stage_*` 隐藏项默认不进 Spotlight 索引，
# 「mdfind 查不到」是恒真，不能拿来断言残留清零。脚本里的硬断言用 read_dir +
# symlink_metadata，mdfind 只打印观察值。
#
# Windows 原生 runner 不在此脚本内（本机无 Windows 环境），见 README 里的
# `gh workflow run` 一段。

set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
export MACLEAN_NO_TELEMETRY=1   # 隔离 HOME 下也别触发匿名首跑上报
REAL_HOME="${HOME}"

PASS=0
FAIL=0
SKIP=0
pass() { printf '  PASS  %s\n' "$1"; PASS=$((PASS + 1)); }
fail() { printf '  FAIL  %s\n' "$1"; FAIL=$((FAIL + 1)); }
skip() { printf '  SKIP  %s\n' "$1"; SKIP=$((SKIP + 1)); }

echo "=========================================================="
echo " maclean GC 真机验证  $(date '+%Y-%m-%d %H:%M:%S')"
echo " 仓库: $ROOT"
echo "=========================================================="

if [ "$(uname -s)" != "Darwin" ]; then
  echo "本脚本的 df -k / chflags / stat -f 为 macOS 口径，当前系统不适用。"
  exit 2
fi

# ---------------------------------------------------------------------------
# 0. 构建真实 CLI 二进制（权限场景走命令入口的 recover_if_needed）
# ---------------------------------------------------------------------------
echo
echo "[0/4] 构建 maclean CLI (debug)"
if cargo build -q -p maclean-cli --bin maclean 2>&1; then
  BIN="$ROOT/target/debug/maclean"
  pass "CLI 构建完成: $BIN"
else
  fail "CLI 构建失败"
  echo "无法继续（后续权限场景依赖真实二进制）"
  exit 1
fi

# ---------------------------------------------------------------------------
# 1. 权限失败路径：走真实 CLI，断言软失败 + 记录保留 + 日志可见
# ---------------------------------------------------------------------------
echo
echo "[1/4] 权限失败路径（真实 CLI 命令入口 recovery）"

run_permission_case() {
  local mode="$1"
  local base home work staging journal dev ino logs
  base="$(mktemp -d "${TMPDIR:-/tmp}/maclean-gc-perm-${mode}.XXXXXX")"
  home="$base/home"
  work="$base/work"
  mkdir -p "$home/.maclean" "$work"
  staging="$work/.maclean_stage_999999_abc_0"
  mkdir -p "$staging"
  printf 'payload' > "$staging/payload.bin"
  dev="$(stat -f '%d' "$staging")"
  ino="$(stat -f '%i' "$staging")"
  journal="$home/.maclean/staging.journal"
  printf '{"v":1,"ts":%s,"pid":999999,"path":"%s","dev":%s,"ino":%s}\n' \
    "$(date +%s)" "$staging" "$dev" "$ino" > "$journal"
  logs="$home/.maclean/logs"

  cleanup_case() {
    chflags -R nouchg "$base" 2>/dev/null || true
    if [ "$mode" = "root" ] && sudo -n true 2>/dev/null; then
      sudo -n chown -R "$(id -u):$(id -g)" "$base" 2>/dev/null || true
      sudo -n rm -rf "$base" 2>/dev/null || true
    fi
    rm -rf "$base" 2>/dev/null || true
  }

  case "$mode" in
    immutable)
      if ! chflags uchg "$staging" 2>/dev/null; then
        skip "chflags uchg 不受当前文件系统支持"
        cleanup_case
        return
      fi
      ;;
    root)
      if ! sudo -n true 2>/dev/null; then
        skip "root 拥有项需要免密 sudo：先执行 'sudo -v' 再重跑（脚本不会索取密码）"
        cleanup_case
        return
      fi
      if ! sudo -n chown -R root "$staging" 2>/dev/null; then
        skip "sudo chown 失败，无法构造 root 拥有项"
        cleanup_case
        return
      fi
      ;;
  esac

  # 任意子命令都会触发命令入口的 recover_if_needed()；stdout/stderr 不重要。
  HOME="$home" "$BIN" list > "$base/cli.out" 2> "$base/cli.err" || true

  local ok=1
  if [ -d "$staging" ]; then
    pass "[$mode] 删不掉的对象原地保留（未误报 cleaned）"
  else
    fail "[$mode] 对象被删掉了 —— 权限失败路径没有软失败"
    ok=0
  fi
  if grep -qF '"path"' "$journal"; then
    pass "[$mode] 未完成记录仍留在 journal，供下次启动重试"
  else
    fail "[$mode] journal 记录丢失，残留会永久失联"
    ok=0
  fi
  if [ -d "$logs" ] && grep -RqF '[gc] 收尾失败' "$logs"; then
    pass "[$mode] 日志页可见「收尾失败」明细"
  else
    fail "[$mode] 日志里没有「收尾失败」记录"
    ok=0
  fi
  if [ -d "$logs" ] && grep -RqF '[gc] 收尾完成' "$logs"; then
    pass "[$mode] 日志页可见收尾汇总（failed 计数）"
  else
    fail "[$mode] 日志里没有收尾汇总"
    ok=0
  fi
  if [ "$ok" = "1" ]; then
    printf '  INFO  [%s] CLI stderr（节选）: %s\n' "$mode" \
      "$(head -c 200 "$base/cli.err" | tr '\n' ' ')"
  fi
  cleanup_case
}

run_permission_case immutable
run_permission_case root

# TCC：与 immutable/root 走同一条 remove_staging_tree 失败分支，代码路径已被
# 上面两项覆盖。这里只做环境探测 + 授权指引，不在真实系统目录里造残留。
echo
echo "  -- TCC 受保护目录探测 --"
tcc_probe="$REAL_HOME/Library/Mail"
if [ -e "$tcc_probe" ] && ls "$tcc_probe" > /dev/null 2>&1; then
  pass "TCC 目录可读（终端已获完全磁盘访问权限）"
  skip "TCC 内造残留需手工在系统目录操作；脚本不改动真实用户数据，失败分支已由上面两项覆盖"
else
  skip "TCC 目录不可读（终端无完全磁盘访问权限）。授权路径：系统设置 > 隐私与安全性 > 完全磁盘访问权限 > 勾选终端，然后重跑"
fi

# ---------------------------------------------------------------------------
# 2. 30s 预算
# ---------------------------------------------------------------------------
echo
echo "[2/4] 30s 预算（可注入，不空跑 30 秒）"
if cargo test -q -p maclean-core --lib gc::tests:: -- --nocapture 2>&1 | tail -25; then
  pass "预算边界 / 耗尽后延后 / 下轮恢复收完"
else
  fail "预算回归测试未通过"
fi

# ---------------------------------------------------------------------------
# 3. 真 SIGKILL + df -k + 日志（128MB 真实占用）
# ---------------------------------------------------------------------------
echo
echo "[3/4] 真 kill -9：残留清零 / df 可用量 / 日志页收尾记录"
echo "  （会写约 128MB 到 \$HOME 下的临时目录，结束后自动删除）"
if cargo test -p maclean-core --test real_machine_gc -- --ignored --nocapture 2>&1; then
  pass "真 SIGKILL 收尾：残留清零 + df 上升 + 日志收尾记录"
else
  fail "真 SIGKILL 收尾用例失败（上方有实测数字与诊断提示）"
fi

# ---------------------------------------------------------------------------
echo
echo "=========================================================="
printf ' 汇总: PASS=%d  FAIL=%d  SKIP=%d\n' "$PASS" "$FAIL" "$SKIP"
echo "=========================================================="
if [ "$FAIL" -gt 0 ]; then
  echo "存在失败项，退出码 1"
  exit 1
fi
echo "全部可执行项通过；SKIP 项需要 sudo / TCC 授权或外部 runner，已在上方标注原因。"
exit 0
