#!/usr/bin/env bash
set -euo pipefail

ROOT="${1:-.}"

echo "maclean Open Core audit"
echo "root: $ROOT"
echo

echo "== Rust files =="
find "$ROOT" -type f \( -name '*.rs' -o -name 'Cargo.toml' \) \
  -not -path '*/target/*' \
  -not -path '*/.git/*' | sort

echo
echo "== Candidate secret files =="
find "$ROOT" -type f \
  \( -name '.env' -o -name '.env.*' -o -name '*.pem' -o -name '*.key' -o -name '*.p12' -o -name '*.mobileprovision' \) \
  -not -path '*/target/*' \
  -not -path '*/.git/*' | sort || true

echo
echo "== Sensitive strings =="
grep -RInE \
  --exclude-dir=.git \
  --exclude-dir=target \
  --exclude='*.lock' \
  '(STRIPE|PRIVATE_KEY|BEGIN .*PRIVATE KEY|API_KEY|SECRET|PASSWORD|TOKEN|AWS_ACCESS_KEY|AWS_SECRET)' \
  "$ROOT" || true

echo
echo "== Git history secret-pattern scan =="
if git -C "$ROOT" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  git -C "$ROOT" log --all --format='%H' | while read -r commit; do
    git -C "$ROOT" grep -nEi \
      '(BEGIN .*PRIVATE KEY|STRIPE_SECRET|AWS_SECRET|API_KEY|PRIVATE_KEY)' \
      "$commit" -- . ':!target' 2>/dev/null || true
  done
else
  echo "Not a git repository."
fi

echo
echo "== Classification reminder =="
echo "Every source file must be classified as OPEN / PRIVATE / MOVE / REWRITE."
echo "Any UNKNOWN file blocks publication."
