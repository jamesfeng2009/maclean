# 02 — Cargo Workspace Split

## 1. Public workspace

The public repository becomes a virtual Cargo workspace.

Cargo workspaces allow packages to be managed together and share a lockfile/output directory; workspace members can be selected explicitly with `-p`/`--workspace`. citeturn0search0turn0search13

Recommended root:

```toml
[workspace]
resolver = "3"

members = [
    "crates/maclean-types",
    "crates/maclean-core",
    "crates/maclean-storage",
    "crates/maclean-platform",
    "crates/maclean-cli",
    "apps/maclean-free",
]

default-members = [
    "apps/maclean-free",
]

[workspace.package]
version = "0.1.0"
edition = "2024"
license = "Apache-2.0"
repository = "https://github.com/jamesfeng2009/maclean"
```

The exact Rust edition/version should follow the current repository's toolchain if it is older; this document does not require a toolchain upgrade as part of the split.

---

# 2. Public package graph

```text
maclean-types
     │
     ├───────────────┐
     ▼               ▼
maclean-storage   maclean-platform
     │               │
     └───────┬───────┘
             ▼
        maclean-core
             │
             ▼
        maclean-cli
             │
             ▼
        maclean-free
```

---

# 3. Public crates

## `maclean-types`

Owns:

```text
StorageEntity
StorageObservation
StorageRelation
DeveloperProject
CleanupCandidate
CleanupPolicy
CleanupRun
CLI envelope
error codes
feature identifiers
```

It must not depend on:

```text
maclean-pro
Stripe
license server
AI provider
Team API
```

---

## `maclean-core`

Owns:

```text
scanner
inventory
classification
safety
cleanup
restore
basic project discovery
basic reclaim calculation
```

It must never contain:

```text
billing
license issuance
private Pro intelligence
Team APIs
```

---

## `maclean-storage`

Owns:

```text
SQLite connection
migrations
repositories
local persistence
```

It stores structured observations, not raw file contents.

---

## `maclean-platform`

Owns OS-specific adapters:

```text
macOS
Windows
```

Examples:

```text
permissions
Trash/Recycle Bin
APFS
Windows app registry
symlink/junction handling
platform paths
```

---

## `maclean-cli`

Owns:

```text
command parsing
JSON/JSONL formatting
exit codes
human-readable rendering
```

The CLI should call core services instead of implementing cleanup logic itself.

---

## `maclean-free`

Owns:

```text
desktop application
basic GUI
menu bar
basic dashboard
basic scan/inventory
```

---

# 4. Private workspace

`maclean-pro` is a separate private repository.

Root:

```toml
[workspace]
resolver = "3"

members = [
    "crates/maclean-pro-intelligence",
    "crates/maclean-pro-history",
    "crates/maclean-pro-forecast",
    "crates/maclean-pro-policy",
    "crates/maclean-pro-automation",
    "crates/maclean-pro-ai",
    "crates/maclean-pro-entitlement",
    "crates/maclean-pro-ui",
]
```

Private crates depend on released/pinned public crates.

They must NOT modify public core through hidden patches in production builds.

---

# 5. Private package graph

```text
maclean-types
      │
maclean-core
      │
      ├───────────────┐
      ▼               ▼
Pro Intelligence   Pro History
      │               │
      ▼               ▼
Pro Forecast      Pro Policy
      │               │
      └───────┬───────┘
              ▼
        Pro Automation
              │
      ┌───────┴────────┐
      ▼                ▼
    Pro AI       Pro Entitlement
      │                │
      └────────┬───────┘
               ▼
             Pro UI
```

---

# 6. Forbidden architecture

Do NOT do this:

```text
maclean/
└── src/
    ├── core/
    └── pro/
```

with:

```toml
features = ["pro"]
```

as the primary protection mechanism.

A Cargo feature is a build switch, not a source-code confidentiality boundary.

---

# 7. Pro dependency rule

Allowed:

```text
maclean-pro → maclean-types
maclean-pro → maclean-core
maclean-pro → maclean-storage
maclean-pro → maclean-platform
```

Forbidden:

```text
maclean-core → maclean-pro
maclean-types → maclean-pro
maclean-storage → maclean-pro
```

This preserves a one-way commercial dependency.

---

# 8. Versioning rule

Public:

```text
maclean-types 0.x
maclean-core  0.x
```

Pro consumes exact compatible versions.

After stable release:

```text
1.x
```

Use semantic versioning for public contracts.

---

# 9. Packaging

For crates that must never be published:

```toml
[package]
publish = false
```

Public crates may be publishable later, but GitHub source release is the first priority.

Cargo package validation should be run on the public workspace before publication; Cargo packages source into distributable `.crate` archives and applies package-specific dependency/path rules. citeturn0search4
