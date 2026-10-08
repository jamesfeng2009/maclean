# 06 — CODEOWNERS / CI Boundary

## Public CODEOWNERS

Recommended:

```text
* @jamesfeng2009
/.github/ @jamesfeng2009
/crates/maclean-core/ @jamesfeng2009
/crates/maclean-types/ @jamesfeng2009
```

GitHub supports CODEOWNERS in `.github/`, repository root, or `docs/`; required code-owner review can be enforced with branch protection/rulesets. citeturn0search3turn0search9

## Public CI

Required:

```text
cargo fmt --check
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
secret scan
dependency/license audit
```

## Private CI

Add:

```text
Pro integration tests
license tests
activation tests
offline tests
release signing
notarization
commercial regression tests
```

## Security

GitHub secret scanning scans repository history for hardcoded credentials and public repositories have secret scanning available automatically/free; use it before the repository is intentionally treated as open source. citeturn0search2turn0search10

Where available, enable push protection / merge blocking so new secrets cannot reach protected branches. citeturn0search7turn0search8
