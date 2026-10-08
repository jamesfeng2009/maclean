#!/usr/bin/env bash
set -euo pipefail

ROOT="${1:-}"
APPLY=0

if [[ -z "$ROOT" ]]; then
  echo "usage: $0 /path/to/maclean [--apply]"
  exit 2
fi

if [[ "${2:-}" == "--apply" ]]; then
  APPLY=1
fi

if [[ ! -d "$ROOT" ]]; then
  echo "Repository not found: $ROOT"
  exit 2
fi

TARGET="$ROOT/open-core-migration"
REPORT="$TARGET/MIGRATION_REPORT.txt"

mkdir -p "$TARGET"

log() {
  echo "$*" | tee -a "$REPORT"
}

log "maclean Open Core migration"
log "source=$ROOT"
log "apply=$APPLY"
log

copy_dir() {
  local src="$1"
  local dst="$2"

  if [[ ! -e "$ROOT/$src" ]]; then
    log "SKIP missing: $src"
    return
  fi

  if [[ "$APPLY" -eq 0 ]]; then
    log "WOULD MOVE: $src -> $dst"
  else
    mkdir -p "$(dirname "$ROOT/$dst")"
    git -C "$ROOT" mv "$ROOT/$src" "$ROOT/$dst" 2>/dev/null \
      || mv "$ROOT/$src" "$ROOT/$dst"
    log "MOVED: $src -> $dst"
  fi
}

log "== scanner/core candidates =="

# These are the confirmed modules from the architecture review.
copy_dir "src/cache_registry.rs" "crates/maclean-core/src/scanner/cache_registry.rs"
copy_dir "src/dev_cache.rs" "crates/maclean-core/src/scanner/dev_cache.rs"
copy_dir "src/app_cache.rs" "crates/maclean-core/src/scanner/app_cache.rs"
copy_dir "src/apfs.rs" "crates/maclean-platform/src/macos/apfs.rs"
copy_dir "src/large_files.rs" "crates/maclean-core/src/scanner/large_files.rs"
copy_dir "src/residual_match.rs" "crates/maclean-core/src/scanner/residual_match.rs"
copy_dir "src/windows_apps.rs" "crates/maclean-platform/src/windows/apps.rs"

log
log "== files requiring manual REWRITE =="
log "src/uninstall.rs"
log "license/entitlement modules"
log "GUI modules mixing basic and Pro dashboard logic"
log "scheduler modules mixing basic cleanup and Pro automation"
log
log "These are intentionally NOT auto-moved because semantic splitting is required."

if [[ "$APPLY" -eq 0 ]]; then
  log
  log "DRY RUN ONLY. Re-run with --apply after review."
else
  log
  log "Migration moves completed where source paths existed."
  log "You must now wire Cargo manifests and fix imports."
fi
