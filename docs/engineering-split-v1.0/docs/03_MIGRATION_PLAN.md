# 03 — Migration Plan

## Phase 0 — Freeze current behavior

Before moving code:

```bash
git checkout -b open-core/migration-v1
git tag maclean-pre-open-core-v1
```

Do not combine the split with a large behavior rewrite.

---

## Phase 1 — Inventory

Run:

```bash
./scripts/maclean-open-core-audit.sh /path/to/maclean
```

Output:

```text
OPEN
PRIVATE
MOVE
REWRITE
UNKNOWN
```

Any `UNKNOWN` must be manually classified.

---

## Phase 2 — Create public crates

Create:

```text
crates/maclean-types
crates/maclean-core
crates/maclean-storage
crates/maclean-platform
crates/maclean-cli
apps/maclean-free
```

Do this before deleting or moving old source.

---

## Phase 3 — Extract domain types

Move stable structs/enums first.

Target:

```text
maclean-types/src/domain/
```

The six core domain models are:

```text
StorageEntity
StorageObservation
DeveloperProject
CleanupCandidate
CleanupPolicy
CleanupRun
```

---

## Phase 4 — Extract safety

Move and test:

```text
sanitize_before_delete
protected path checks
TOCTOU checks
symlink checks
busy-file handling
privilege handling
Trash/restore semantics
```

Safety must remain behaviorally equivalent.

---

## Phase 5 — Extract scanners

Move:

```text
cache_registry.rs
dev_cache.rs
app_cache.rs
apfs.rs
large_files.rs
residual_match.rs
windows_apps.rs
```

into scanner/platform crates.

Do not add new scanner behavior during this phase.

---

## Phase 6 — Extract CLI

CLI becomes a thin adapter:

```text
CLI
 ↓
service
 ↓
core
 ↓
platform
```

No filesystem deletion logic should remain in CLI argument handlers.

---

## Phase 7 — Extract Free UI

Move only basic workflows.

Keep:

```text
scan
inventory
basic cleanup
safety
restore
```

Remove advanced commercial dashboard dependencies.

---

## Phase 8 — Create private Pro

Create the private repo:

```text
maclean-pro
```

Move:

```text
advanced intelligence
history
forecast
policy
automation
premium rules
AI
entitlement
Pro UI
```

---

## Phase 9 — License boundary

Add to public repo:

```text
LICENSE = Apache-2.0
NOTICE
CONTRIBUTING.md
SECURITY.md
DCO
CODE_OF_CONDUCT.md
GOVERNANCE.md
TRADEMARK_POLICY.md
```

Only after history/security audit.

---

## Phase 10 — Build gates

Public:

```bash
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Private:

```text
public core + private Pro
```

---

## Phase 11 — Platform validation

Required before claiming support:

```text
macOS Apple Silicon
macOS Intel
Windows x64
```

The previous product decision remains:

```text
Apple Silicon = P0
Intel macOS    = P1
Windows x64    = P1
Windows ARM64  = P2/defer
```

---

# 4. Rollback strategy

Every migration phase should be independently revertible.

Never perform:

```bash
rm -rf old-source
```

as part of the automated migration.

The supplied migration script only copies/moves after explicit `--apply`, and creates a report.

---

# 5. Completion definition

The split is complete only when:

```text
public build works
AND
private Pro build works
AND
public repo has no private dependencies
AND
public repo contains no commercial secrets
AND
safety tests are unchanged/passing
AND
all UNKNOWN files are classified
```
