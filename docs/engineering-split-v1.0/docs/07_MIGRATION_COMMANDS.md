# 07 — Exact Migration Sequence

Assume the current repo is:

```bash
cd maclean
```

## 1. Freeze

```bash
git status
git checkout -b open-core/migration-v1
git tag maclean-pre-open-core-v1
```

## 2. Audit

```bash
./scripts/maclean-open-core-audit.sh .
```

## 3. Dry-run

```bash
./scripts/maclean-open-core-migrate.sh . 
```

Review:

```text
open-core-migration/MIGRATION_REPORT.txt
```

## 4. Apply known file moves

```bash
./scripts/maclean-open-core-migrate.sh . --apply
```

## 5. Create crate skeletons

Copy the templates from:

```text
workspace/
```

into the repository.

## 6. Fix imports

Use:

```bash
cargo check --workspace
```

and resolve module paths incrementally.

Do NOT mass-rewrite all imports automatically.

## 7. Extract domain types first

```text
old structs
   ↓
maclean-types
   ↓
core/CLI/UI
```

## 8. Extract safety second

No feature work until safety tests pass.

## 9. Extract scanners third

Keep scanner behavior unchanged.

## 10. Split private logic

Move commercial logic into the private `maclean-pro` repository.

## 11. Validate

```bash
./scripts/maclean-open-core-validate.sh .
```

## 12. Commit in layers

Recommended commits:

```text
01 chore: freeze pre-open-core baseline
02 refactor: introduce public domain contracts
03 refactor: extract maclean-core
04 refactor: extract platform adapters
05 refactor: extract public CLI
06 refactor: extract free app
07 chore: create private pro workspace
08 refactor: move commercial intelligence
09 chore: add Apache-2.0 and governance files
10 chore: harden CI and secret scanning
```

Do not make one giant migration commit.
