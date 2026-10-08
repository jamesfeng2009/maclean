# 01 — Existing maclean Repository → Open Core / Pro Mapping

## 1. Classification legend

| Label | Meaning |
|---|---|
| `OPEN` | Remains in the public `maclean` repository under Apache-2.0 |
| `PRIVATE` | Commercial/private implementation; never copied into public repo |
| `MOVE` | Move from the current location into a new public/private crate |
| `REWRITE` | Keep the capability but redesign its boundary/API before publishing |

---

# 2. Confirmed current source modules

The following modules were identified during the existing maclean architecture review.

| Current module | Decision | Target |
|---|---|---|
| `cache_registry.rs` | `MOVE` | `maclean-core::scanner::registry` |
| `dev_cache.rs` | `MOVE` | `maclean-core::scanner::developer` |
| `app_cache.rs` | `MOVE` | `maclean-core::scanner::application` |
| `apfs.rs` | `MOVE` | `maclean-platform::macos::apfs` / core adapter |
| `large_files.rs` | `MOVE` | `maclean-core::scanner::large_files` |
| `uninstall.rs` | `REWRITE` | public safety-aware uninstall contract + platform implementations |
| `residual_match.rs` | `MOVE` | `maclean-core::scanner::residuals` |
| `windows_apps.rs` | `REWRITE` | `maclean-platform::windows::apps` |
| `sanitize_before_delete` implementation | `MOVE` | `maclean-core::safety` |
| protected-path checks | `MOVE` | `maclean-core::safety` |
| symlink / TOCTOU checks | `MOVE` | `maclean-core::safety` |
| Trash behavior | `MOVE` | platform adapter + core action model |
| restore workflow | `MOVE` | `maclean-core::restore` |
| CLI scan/list/check-disk | `MOVE` | `maclean-cli` |
| CLI JSON/JSONL envelope | `MOVE` | `maclean-types` + `maclean-cli` |
| GUI basic scan/inventory | `MOVE` | `maclean-free` |
| current license verifier | `REWRITE` | public verifier interface + private issuer |
| Ed25519 public key | `OPEN` | client verification material |
| Ed25519 private key | `PRIVATE` | license server/KMS only |
| payment/Stripe integration | `PRIVATE` | `maclean-license-server` |
| advanced history | `PRIVATE` | `maclean-pro-history` |
| growth intelligence | `PRIVATE` | `maclean-pro-intelligence` |
| forecast | `PRIVATE` | `maclean-pro-forecast` |
| reclaim scoring | `PRIVATE` | `maclean-pro-intelligence` |
| policy planner | `PRIVATE` | `maclean-pro-policy` |
| scheduled automation | `PRIVATE` | `maclean-pro-automation` |
| premium cleanup rules | `PRIVATE` | private premium-rules package |
| AI explanation | `PRIVATE` | `maclean-pro-ai` |
| advanced dashboard | `PRIVATE` | `maclean-pro-ui` |
| entitlement/activation | `PRIVATE` | `maclean-pro-entitlement` |
| Team fleet/policy/audit | `PRIVATE` | future `maclean-team` |

---

# 3. Current feature-to-layer mapping

```text
CURRENT maclean
│
├── filesystem discovery
│      └── OPEN / MOVE → maclean-core
│
├── scanner registry
│      └── OPEN / MOVE → maclean-core
│
├── developer scanners
│      └── OPEN / MOVE → maclean-core
│
├── safety
│      └── OPEN / MOVE → maclean-core
│
├── basic cleanup
│      └── OPEN / MOVE → maclean-core
│
├── restore
│      └── OPEN / MOVE → maclean-core
│
├── CLI
│      └── OPEN / MOVE → maclean-cli
│
├── basic GUI
│      └── OPEN / MOVE → maclean-free
│
├── advanced intelligence
│      └── PRIVATE → maclean-pro
│
├── history / growth / forecast
│      └── PRIVATE → maclean-pro
│
├── smart policy / automation
│      └── PRIVATE → maclean-pro
│
├── premium rules
│      └── PRIVATE → maclean-pro
│
├── AI
│      └── PRIVATE → maclean-pro
│
└── entitlement / billing
       └── PRIVATE → license-server
```

---

# 4. Directory-level policy

The existing repository should be converted toward:

```text
maclean/
├── crates/
│   ├── maclean-types/       OPEN
│   ├── maclean-core/        OPEN
│   ├── maclean-storage/     OPEN
│   ├── maclean-platform/    OPEN
│   └── maclean-cli/         OPEN
│
├── apps/
│   └── maclean-free/        OPEN
│
├── schemas/                 OPEN
├── docs/                    OPEN
├── examples/                OPEN
├── tests/                   OPEN
└── scripts/                 OPEN
```

Private:

```text
maclean-pro/
├── crates/
│   ├── maclean-pro-intelligence/
│   ├── maclean-pro-history/
│   ├── maclean-pro-forecast/
│   ├── maclean-pro-policy/
│   ├── maclean-pro-automation/
│   ├── maclean-pro-ai/
│   ├── maclean-pro-entitlement/
│   └── maclean-pro-ui/
│
├── premium-rules/
└── release/
```

---

# 5. Rules for files not explicitly listed

The migration script applies these rules:

### OPEN

- public domain types
- scanner interfaces
- basic scanner implementations
- safety primitives
- basic cleanup executor
- restore primitives
- CLI contracts
- JSON schemas
- public SQLite schema
- tests for open functionality
- documentation
- examples

### PRIVATE

- payment
- Stripe
- license issuance
- signing private keys
- activation backend
- customer records
- Pro-only intelligence
- forecast models
- policy optimization
- automation orchestration
- premium rule source
- AI prompts/provider credentials
- Team APIs
- Team dashboard

### MOVE

A file is moved when it is already conceptually correct but lives in the wrong package.

### REWRITE

A file is rewritten when it mixes public and private responsibilities.

Typical example:

```text
old:
license.rs
├── verify
├── activate
├── server communication
├── customer lookup
└── feature gates
```

becomes:

```text
public:
license verifier
entitlement contract
feature enum

private:
activation client
refresh
server API
customer mapping
issuer
```

---

# 6. Critical rule

Do not publish a source file merely because it is technically useful.

Publish it when:

```text
it is part of the open-core contract
AND
it does not expose a commercial implementation that must remain private.
```
