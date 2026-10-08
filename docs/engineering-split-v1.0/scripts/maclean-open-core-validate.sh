#!/usr/bin/env bash
set -euo pipefail

ROOT="${1:-.}"
cd "$ROOT"

echo "== Public workspace validation =="

if [[ ! -f Cargo.toml ]]; then
  echo "ERROR: public Cargo.toml missing"
  exit 1
fi

if grep -RInE \
  --exclude-dir=.git \
  --exclude-dir=target \
  'maclean-pro|license-server|stripe|team-api|private.*repository' \
  crates apps Cargo.toml 2>/dev/null; then
  echo
  echo "ERROR: possible private dependency/reference found in public repository."
  exit 1
fi

if grep -RInE \
  --exclude-dir=.git \
  --exclude-dir=target \
  '(STRIPE_SECRET|PRIVATE_KEY|BEGIN .*PRIVATE KEY|AWS_SECRET_ACCESS_KEY)' \
  crates apps .github Cargo.toml 2>/dev/null; then
  echo
  echo "ERROR: possible secret found."
  exit 1
fi

if command -v cargo >/dev/null 2>&1; then
  cargo check --workspace
  cargo test --workspace
  cargo clippy --workspace --all-targets -- -D warnings
else
  echo "cargo not installed; structural validation only."
fi

echo
echo "Public workspace validation passed."
