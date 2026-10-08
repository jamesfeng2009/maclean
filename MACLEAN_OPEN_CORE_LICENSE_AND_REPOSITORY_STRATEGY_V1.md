# MACLEAN Open Core License & Repository Strategy V1.0

> Status: **DECISION BASELINE**
>
> Project: `maclean`
>
> Scope: source-code licensing, repository topology, Cargo workspace boundaries, Free/Pro build boundaries, contributor rights, commercial code protection, and future Team/private modules.
>
> Date: 2026-10-07
>
> This document is an engineering/product governance baseline, not legal advice. Before publishing a final license or accepting external contributions, have counsel review the exact license, contributor agreement, trademark terms, and jurisdiction.

---

## 0. Executive Decision

`maclean` SHALL adopt an **Open-Core repository strategy**, not a fully open-source product strategy.

The repository MAY remain public, but **public visibility is not itself a grant of source-code reuse rights**. The repository must contain an explicit license before code is intentionally released as open source. GitHub notes that a public repository is viewable/forkable on GitHub, while an actual open-source license grants the broader rights to use, modify, and distribute the code. citeturn0search2turn0search15

### Final policy

| Area | Decision |
|---|---|
| Repository | Public `maclean` repository |
| Strategy | Open Core |
| Open-source core license | **Apache-2.0** |
| Commercial Pro code | Private repository/package |
| Team code | Private repository/package |
| Trademark | Separate `MACLEAN_TRADEMARK.md` policy; source license does not grant trademark rights |
| Free binary | Built from public/open core plus product shell |
| Pro binary | Built from open core + private Pro modules |
| Team binary | Built from open core + private Pro/Team modules |
| License server | Private |
| Entitlement/signing service | Private |
| Stripe integration | Private |
| Signing private keys | Never in repository |
| License issuer private keys | Never in repository |
| Customer/payment secrets | Never in repository |
| Contributor model | DCO first; CLA only if later required by relicensing/commercial policy |
| Future Team backend | Private |
| Premium scanner rules | Private unless deliberately promoted to open core |
| Safety primitives | Open |
| Basic scanners | Open |
| Basic CLI scan/inventory | Open |
| Advanced intelligence | Commercial |
| Forecast/history/policy automation | Commercial |
| Pro GUI workflows | Commercial |
| Team fleet/policy/audit | Commercial |

### Core principle

> **Open the mechanism; monetize the intelligence, automation, product experience, support, and commercial control plane.**

The purpose is not to make source code artificially scarce. The purpose is to keep the open project genuinely useful while preserving a defensible commercial layer.

---

# 1. What “Open Core” Means for maclean

`maclean` is not going to use the following model:

```text
100% source code
        ↓
MIT/Apache
        ↓
same source
        ↓
free binary
        ↓
try to charge anyway
```

That creates a weak commercial boundary.

Instead:

```text
                         MACLEAN
                            │
              ┌─────────────┴─────────────┐
              │                           │
        OPEN CORE                     PRIVATE
       Apache-2.0                    COMMERCIAL
              │                           │
   scanner primitives              Pro intelligence
   inventory model                 history
   safety primitives               forecast
   classification                  policy engine
   basic CLI                       automation
   JSON/JSONL                      premium rules
   basic project discovery         Pro GUI workflows
   basic cleanup engine            entitlement
                                   licensing
                                   billing
                                   Team
```

The commercial product MUST NOT be merely a disabled flag around code that is otherwise entirely present in the public repository.

The commercial boundary should exist structurally:

```text
public source
    ↓
maclean-core
    ↓
stable public contracts
    ↓
private commercial crates
    ↓
signed release artifacts
```

---

# 2. Important Licensing Principle

Open source does **not** mean “you cannot charge.”

The Open Source Initiative explicitly states that open-source software can be used commercially and can be sold. citeturn0search3

Therefore:

```text
Open source ≠ free of charge
```

But also:

```text
Open source = downstream users receive broad rights
```

For example, Apache-2.0 grants broad rights to reproduce, modify, distribute, and sublicense the covered work, subject to its conditions. citeturn0search5

Therefore maclean's commercial strategy must **not** depend on preventing someone from selling a binary containing the Apache-2.0 portion.

Instead, commercial differentiation comes from:

- private Pro implementation
- advanced intelligence
- premium rules
- automation
- history
- forecast
- product UX
- signed official builds
- updates
- support
- Team control plane
- hosted licensing/entitlement
- official branding/trademark
- commercial distribution convenience

---

# 3. Repository Topology

## 3.1 Public repository

Canonical public repository:

```text
github.com/jamesfeng2009/maclean
```

The public repository should contain:

```text
maclean/
├── crates/
│   ├── maclean-core/
│   ├── maclean-cli/
│   ├── maclean-types/
│   ├── maclean-storage/
│   └── maclean-platform/
│
├── apps/
│   └── maclean-free/
│
├── schemas/
│   ├── json/
│   └── migrations/
│
├── docs/
│   ├── architecture/
│   ├── safety/
│   ├── scanners/
│   └── development/
│
├── examples/
│
├── tests/
│
├── .github/
│   ├── workflows/
│   ├── ISSUE_TEMPLATE/
│   ├── CODEOWNERS
│   └── dependabot.yml
│
├── LICENSE
├── NOTICE
├── README.md
├── CONTRIBUTING.md
├── SECURITY.md
├── CODE_OF_CONDUCT.md
├── GOVERNANCE.md
├── DCO
└── Cargo.toml
```

GitHub recommends putting a detectable license file in the repository when a project is intentionally open source. citeturn0search0turn0search2

---

# 4. Public vs Private Repository Boundary

## 4.1 Public repository MUST contain

### Core domain

```text
crates/maclean-core/src/
├── domain/
├── scanner/
├── inventory/
├── classification/
├── safety/
├── cleanup/
├── restore/
└── output/
```

### Basic intelligence

Public:

```text
basic reclaimable calculation
basic classification
basic project discovery
basic storage aggregation
basic scan history format
```

The open project must be useful on its own.

---

## 4.2 Private repository MUST contain

Create a separate private repository:

```text
maclean-pro
```

Suggested structure:

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
├── apps/
│   └── maclean-pro/
│
├── premium-rules/
│
├── license-client/
├── release/
└── internal/
```

Private code MUST include:

- advanced storage intelligence
- growth modeling beyond basic snapshots
- forecasting
- reclaim scoring
- advanced project graph
- policy planner
- scheduled automation
- premium cleanup rules
- AI explanation integration
- Pro entitlement
- activation/deactivation
- license refresh
- paid feature gates
- commercial analytics that are not required for the open core
- official Pro UI
- release orchestration
- update channel logic

---

# 5. Future Team Repository

Do not put Team infrastructure into the public repository.

Create:

```text
maclean-team
```

Suggested structure:

```text
maclean-team/
├── services/
│   ├── api/
│   ├── entitlement/
│   ├── policy/
│   ├── device/
│   ├── audit/
│   └── billing/
│
├── packages/
│   ├── policy-schema/
│   ├── signed-policy/
│   └── team-contracts/
│
├── web/
│   └── dashboard/
│
└── infra/
```

Team remains deferred until there is evidence of demand.

Recommended trigger:

```text
> 1,000 active installs
AND
measurable Pro conversion
AND
real customers asking for fleet/policy/audit
```

Do not build the Team control plane merely because the architecture permits it.

---

# 6. Directory-Level License Matrix

| Directory / Component | Visibility | License / Control | Commercial Role |
|---|---:|---|---|
| `crates/maclean-types` | Public | Apache-2.0 | Stable contracts |
| `crates/maclean-core` | Public | Apache-2.0 | Core engine |
| `crates/maclean-storage` | Public | Apache-2.0 | Local persistence |
| `crates/maclean-platform` | Public | Apache-2.0 | OS adapters |
| `crates/maclean-cli` | Public | Apache-2.0 | Basic CLI |
| `apps/maclean-free` | Public | Apache-2.0 | Free/open product |
| `schemas/json` | Public | Apache-2.0 | Public contract |
| `schemas/migrations` | Public | Apache-2.0 | Public schema |
| basic scanners | Public | Apache-2.0 | Open ecosystem |
| safety gate | Public | Apache-2.0 | Trust/transparency |
| basic cleanup | Public | Apache-2.0 | Useful open core |
| advanced intelligence | Private | Proprietary | Pro |
| history | Private | Proprietary | Pro |
| forecast | Private | Proprietary | Pro |
| policy engine | Private | Proprietary | Pro |
| automation | Private | Proprietary | Pro |
| premium rules | Private | Proprietary | Pro |
| AI integration | Private | Proprietary | Pro |
| entitlement | Private | Proprietary | Pro |
| billing | Private | Proprietary | Pro |
| license issuer | Private | Proprietary | Commercial infrastructure |
| Team backend | Private | Proprietary | Team |
| Team dashboard | Private | Proprietary | Team |
| fleet policy | Private | Proprietary | Team |
| audit service | Private | Proprietary | Team |
| signing keys | Private / secret store | Secret | Security |
| Stripe secrets | Private / secret store | Secret | Security |

---

# 7. Cargo Workspace Strategy

The public workspace should compile independently.

## 7.1 Public workspace

Root:

```toml
[workspace]
members = [
    "crates/maclean-types",
    "crates/maclean-core",
    "crates/maclean-storage",
    "crates/maclean-platform",
    "crates/maclean-cli",
    "apps/maclean-free",
]
resolver = "2"
```

Public developers must be able to run:

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

without access to private repositories.

---

# 8. Commercial Workspace Composition

The commercial build uses a second workspace or private super-workspace.

Recommended:

```text
maclean-pro/
└── Cargo.toml
```

with:

```toml
[workspace]
members = [
    "../maclean/crates/maclean-types",
    "../maclean/crates/maclean-core",
    "../maclean/crates/maclean-storage",
    "../maclean/crates/maclean-platform",
    "../maclean-pro/crates/maclean-pro-intelligence",
    "../maclean-pro/crates/maclean-pro-history",
    "../maclean-pro/crates/maclean-pro-forecast",
    "../maclean-pro/crates/maclean-pro-policy",
    "../maclean-pro/crates/maclean-pro-automation",
    "../maclean-pro/crates/maclean-pro-entitlement",
    "../maclean-pro/apps/maclean-pro",
]
```

In actual CI, prefer a reproducible source checkout/submodule/package mechanism rather than relying on arbitrary relative paths.

---

# 9. Do NOT Put Private Code Behind a Public Cargo Feature

Avoid this:

```toml
[features]
pro = []
```

if the Pro implementation is still in the public repository.

Bad:

```text
public repository
└── src/pro/
    ├── forecast.rs
    ├── policy.rs
    └── automation.rs
```

with:

```bash
cargo build --features pro
```

That only hides the functionality at compile time.

It does not protect the source.

---

# 10. Correct Free / Pro Compilation Model

Use trait-based public interfaces.

Public:

```rust
pub trait StorageIntelligence {
    fn summarize(
        &self,
        inventory: &Inventory,
    ) -> IntelligenceSummary;
}
```

Free implementation:

```rust
pub struct BasicStorageIntelligence;
```

Private implementation:

```rust
pub struct ProStorageIntelligence;
```

The public core should define:

```text
contracts
    ↓
interfaces
    ↓
safe default implementation
```

The private layer supplies:

```text
advanced implementation
    ↓
Pro feature set
```

---

# 11. Recommended Cargo Dependency Graph

```text
                    ┌─────────────────────┐
                    │    maclean-types    │
                    └──────────┬──────────┘
                               │
             ┌─────────────────┼─────────────────┐
             │                 │                 │
             ▼                 ▼                 ▼
     maclean-storage    maclean-platform   maclean-core
             │                 │                 │
             └─────────────────┼─────────────────┘
                               │
                         maclean-cli
                               │
                         maclean-free
```

Private:

```text
maclean-types
      │
maclean-core
      │
      ├───────────────┐
      ▼               ▼
Pro Intelligence   Pro Policy
      │               │
      ├──────┬────────┤
      ▼      ▼        ▼
 Forecast  History  Automation
      │
      ▼
 Pro App
      │
      ▼
 Entitlement
```

Team:

```text
Pro Client
    │
    ▼
Team SDK / Contracts
    │
    ▼
Private Team API
    │
    ├── Device
    ├── Policy
    ├── Audit
    ├── Billing
    └── Dashboard
```

---

# 12. What MUST Stay Open

The following are deliberately open because they increase trust and adoption.

## 12.1 Safety

Open:

```text
sanitize_before_delete
protected path detection
symlink checks
TOCTOU protection
Trash behavior
permission boundaries
app protection levels
```

This is strategically important.

Users should be able to inspect how maclean prevents destructive behavior.

---

## 12.2 Basic Scanner Architecture

Open:

```text
Scanner trait
ScannerRegistry
ScanResult
StorageEntity
StorageObservation
StorageRelation
```

Basic scanner implementations can be public.

Examples:

```text
Xcode
node_modules
Cargo target
Python venv
Docker basic cache
AI model inventory
Trash
large files
```

---

# 13. What SHOULD Stay Private

The following should remain private because they represent the product's commercial intelligence rather than basic filesystem mechanics.

## 13.1 Reclaim Intelligence

```text
reclaim_score
priority ranking
risk-adjusted reclaim value
confidence calculation
candidate ranking
```

---

## 13.2 Forecasting

```text
growth model
low-space prediction
time-to-threshold
growth anomaly detection
storage trend model
```

---

## 13.3 Policy Intelligence

```text
policy optimizer
candidate selection strategy
automatic policy recommendation
policy conflict resolution
advanced dry-run planner
```

---

## 13.4 Premium Rule Knowledge

The following can be private:

```text
premium scanner definitions
heuristic thresholds
advanced stale-project detection
advanced AI cache lifecycle rules
vendor-specific cleanup policies
enterprise policy packs
```

Basic scanner APIs remain public.

---

# 14. Free vs Pro Product Boundary

## Free

```text
Scan
Inventory
Basic classification
Basic project discovery
Basic reclaimable estimate
Basic growth snapshot
Basic CLI
JSON output
Safety checks
Preview cleanup
Basic cleanup where appropriate
```

## Pro

```text
Full cleanup workflow
Restore workflow
Smart Cleanup
Cleanup policies
Scheduled automation
Full history
Forecast
Advanced project intelligence
Reclaim scoring
Premium scanner/rule packs
Advanced CLI automation
AI storage explanation
Advanced JSON/JSONL analytics
```

## Team

```text
Device enrollment
Fleet dashboard
Central policy
Signed policy bundles
Aggregate storage intelligence
Audit logs
Team-level controls
SSO
Organization billing
```

---

# 15. Free Binary Must Not Be a Fake Trial

The Free edition should be genuinely useful.

Do not:

```text
scan
↓
show 174 GB reclaimable
↓
disable everything
↓
"BUY NOW"
```

Instead:

```text
Free
  ↓
understand storage
  ↓
inspect candidates
  ↓
perform safe/basic cleanup
  ↓
discover value
  ↓
upgrade when advanced automation is needed
```

This is more credible for a developer product.

---

# 16. Commercial Feature Gate Architecture

The gate belongs at the application/service layer.

Example:

```rust
pub enum Feature {
    BasicScan,
    BasicCleanup,
    AdvancedHistory,
    Forecast,
    SmartCleanup,
    ScheduledAutomation,
    PremiumRules,
    AiExplanation,
}
```

Then:

```rust
pub trait EntitlementProvider {
    fn allows(&self, feature: Feature) -> bool;
}
```

Free:

```rust
FreeEntitlementProvider
```

Pro:

```rust
LicenseEntitlementProvider
```

Team:

```rust
TeamEntitlementProvider
```

Do not put the authoritative entitlement decision solely in GUI code.

---

# 17. License Verification Architecture

The existing Ed25519 trust model should remain the cryptographic root.

Target:

```text
Purchase
   ↓
Payment provider
   ↓
Webhook
   ↓
Entitlement service
   ↓
License issuer
   ↓
Signed license
   ↓
maclean client
   ↓
Ed25519 verification
   ↓
Feature gates
```

Private signing key:

```text
NEVER:
Git
GitHub Actions repository secret printed to logs
Cargo source
Desktop application
Public documentation
```

Prefer:

```text
KMS/HSM
    ↓
license signing service
```

---

# 18. License Payload

Recommended canonical payload:

```json
{
  "schema_version": 1,
  "license_id": "uuid",
  "product": "maclean",
  "plan": "pro",
  "issued_at": "2026-10-07T00:00:00Z",
  "expires_at": null,
  "seats": 1,
  "machine_limit": 2,
  "customer_ref": "opaque-reference",
  "features": [
    "advanced_history",
    "forecast",
    "smart_cleanup",
    "automation"
  ],
  "key_id": "license-signing-key-01"
}
```

The payload should contain no:

```text
credit card
payment token
Stripe secret
raw filesystem paths
file contents
source code
private API credentials
```

---

# 19. Public Contract vs Private Implementation

Public contracts should remain stable.

Examples:

```text
StorageEntity
StorageObservation
DeveloperProject
CleanupCandidate
CleanupPolicy
CleanupRun
```

Private implementations can evolve independently.

This gives:

```text
public contract stability
+
private implementation freedom
```

The public API should describe **what** the engine can represent.

The private layer owns advanced logic describing **how** commercial intelligence is derived.

---

# 20. SQLite Strategy

The core SQLite schema may remain public.

Open:

```text
storage_entities
storage_observations
developer_projects
storage_relations
cleanup_candidates
cleanup_runs
```

Commercial tables can remain private if they contain proprietary product intelligence.

Examples:

```text
reclaim_scores
forecast_models
policy_recommendations
premium_rule_state
pro_feature_usage
```

However, do not unnecessarily hide generic persistence primitives.

The rule is:

> Hide proprietary semantics, not ordinary database engineering.

---

# 21. CLI Strategy

Public CLI:

```bash
maclean scan
maclean list
maclean check-disk
maclean apps
maclean startup
maclean backups
maclean restore
maclean log
```

Public structured output:

```bash
maclean scan --format json
maclean scan --format jsonl
```

Pro commands:

```bash
maclean history
maclean growth
maclean forecast
maclean policy list
maclean policy validate
maclean policy dry-run
maclean policy apply
maclean automation
```

The CLI contract remains public where practical.

The Pro implementation remains private.

---

# 22. GUI Strategy

Public/open:

```text
scan progress
inventory viewer
basic category view
basic candidate review
safety center
basic cleanup
```

Private Pro:

```text
advanced dashboard
growth analytics
forecast
project intelligence drawer
smart cleanup builder
policy editor
automation scheduler
advanced restore UX
AI explanation UI
```

This allows the public project to demonstrate real product quality without exposing the entire commercial roadmap.

---

# 23. GitHub Repository Policy

## 23.1 Public repository

Enable:

```text
Issues
Discussions
Pull Requests
Security Advisories
Dependabot
CODEOWNERS
```

Consider disabling direct pushes to `main`.

Required:

```text
branch protection
CI required
review required
secret scanning
dependency review
```

GitHub supports repository contribution guidance such as `CONTRIBUTING`, `CODE_OF_CONDUCT`, `SECURITY`, and `CODEOWNERS`. citeturn0search11turn0search10

---

# 24. Public Repository Does NOT Mean Everything Must Be Public

The following can remain private:

```text
maclean-pro
maclean-team
license-server
billing-service
release-infrastructure
signing infrastructure
premium rule source
private customer integrations
internal security tooling
commercial roadmap
```

The public repository is the community-facing project.

The private repositories are the commercial product.

---

# 25. Git History Security Audit — REQUIRED BEFORE OPEN-SOURCING

Because the repository was previously private, do NOT simply add `LICENSE` and announce it as open source.

First audit:

```text
git history
branches
tags
deleted files
GitHub Actions
release artifacts
issues
PRs
workflow logs
```

Search for:

```text
sk-
AKIA
AWS
STRIPE
PRIVATE_KEY
BEGIN OPENSSH PRIVATE KEY
BEGIN PRIVATE KEY
BEGIN RSA PRIVATE KEY
LICENSE_SECRET
JWT_SECRET
DATABASE_URL
TOKEN
PASSWORD
API_KEY
```

Also inspect:

```text
target/
dist/
release/
.env
.env.*
*.pem
*.key
*.p12
*.mobileprovision
```

If a secret was ever committed:

```text
rotate secret
↓
revoke old secret
↓
rewrite history if appropriate
↓
verify repository
↓
only then open source
```

Removing the latest file is not enough.

---

# 26. GitHub Repository Visibility Decision

Current recommendation:

```text
PUBLIC
```

but only after:

```text
secret audit
license boundary audit
dependency license audit
git history audit
release artifact audit
```

If the audit discovers commercially sensitive source or secrets that cannot be separated cleanly:

```text
PRIVATE
    ↓
split repositories
    ↓
clean public repository
    ↓
publish
```

Do not let the fact that the repository is currently public force the open-source decision.

---

# 27. License Choice

## 27.1 Primary recommendation: Apache-2.0

Use:

```text
Apache License 2.0
SPDX: Apache-2.0
```

for the open core.

Reason:

- permissive
- widely understood
- commercial friendly
- patent grant
- strong ecosystem compatibility
- low friction for Rust developers
- compatible with open-core strategy

OSI lists Apache-2.0 as an approved open-source license. citeturn0search4turn0search5

---

# 28. Why Not MIT as the Default?

MIT is also valid and extremely simple.

However, Apache-2.0 is preferred for maclean because of its explicit patent grant and more complete legal framework.

MIT permits commercial use, modification, distribution, sublicensing, and sale with minimal conditions. citeturn0search7

For maclean:

```text
MIT       = simpler
Apache-2  = stronger long-term project baseline
```

Choose Apache-2.0.

---

# 29. Why Not GPL/AGPL?

GPL/AGPL are legitimate open-source licenses.

They are not the preferred baseline here because maclean wants:

```text
broad adoption
commercial embedding
commercial tooling integrations
low-friction ecosystem use
```

The project is a developer utility, not a server-side network service where AGPL reciprocity is central.

Therefore:

```text
Apache-2.0
```

is the default.

---

# 30. Source-Available Alternative

If the commercial goal changes and you decide that some source should be visible but not commercially reusable, use a **separate source-available license** for that component.

Do not call such code “open source” in the OSI sense.

The distinction must be explicit:

```text
Apache-2.0
= Open Source

Custom commercial/source-available license
= Source Available
```

Do not mix ambiguous terminology in README marketing.

---

# 31. Dual Licensing

Do NOT use dual licensing for the whole project initially.

Avoid:

```text
maclean
├── MIT
└── Commercial License
```

for the same source unless there is a concrete reason.

The recommended structure is:

```text
Open Core
└── Apache-2.0

Pro
└── Proprietary

Team
└── Proprietary
```

Dual licensing can be introduced later for a specific core component if there is a business/legal reason.

---

# 32. Contributor Policy

Initial model:

```text
DCO + Apache-2.0
```

Contributor signs off commits with:

```text
Signed-off-by: Name <email>
```

This is lower friction than requiring a CLA for every contributor.

---

# 33. When to Introduce a CLA

Introduce a CLA if one of these becomes true:

```text
multiple external corporate contributors
planned relicensing
commercial redistribution of community contributions
need for explicit copyright assignment/license grant
large ecosystem
```

Until then:

```text
DCO
```

is sufficient operationally, subject to legal review.

---

# 34. Contributor Rule

Every contribution must satisfy:

```text
I wrote it
OR
I have the right to submit it
```

Never accept:

```text
copied proprietary scanner
copied vendor source
AI-generated code with unclear provenance
code containing third-party license violations
```

The project should maintain:

```text
CONTRIBUTING.md
DCO
NOTICE
SECURITY.md
```

---

# 35. Copyright Ownership

For code authored by the project owner:

```text
Copyright © 2026 maclean contributors
```

Do not casually transfer copyright ownership.

For future company formation:

```text
Founder / current owner
        ↓
IP assignment
        ↓
Company
        ↓
maclean
```

This should be handled legally, not through an informal README statement.

---

# 36. Trademark Strategy

The source license does not automatically grant permission to use the project's trademarks as if they were the official product.

Reserve:

```text
maclean
Maclean
maclean logo
```

as brand assets.

Future policy:

```text
Fork:
allowed under Apache-2.0

Use source:
allowed under Apache-2.0

Call fork "maclean":
not automatically authorized

Claim official maclean:
not authorized
```

Create later:

```text
TRADEMARK_POLICY.md
```

---

# 37. Preventing Fake Official Builds

Official releases should be distinguishable.

Use:

```text
Official maclean
```

for official signed releases.

Third-party forks should not be able to imply:

```text
official maclean release
```

through branding.

Use release signing:

```text
macOS notarization
Windows Authenticode
release checksums
signed release metadata
```

---

# 38. Commercial Code Protection

Private code must not leak through:

```text
debug symbols
release artifacts
source maps
CI logs
crash reports
generated documentation
public benchmark output
test fixtures
example repositories
Docker images
```

Build pipeline:

```text
private source
    ↓
private CI
    ↓
release build
    ↓
strip/debug policy
    ↓
sign
    ↓
notarize
    ↓
publish
```

---

# 39. Cargo Package Protection

Do not accidentally publish private crates to crates.io.

Private Cargo packages should use:

```toml
[package]
publish = false
```

where applicable.

For private dependencies:

```text
private Git repository
or
private registry
```

Do not rely on an unpublished crate name alone as a security boundary.

---

# 40. Dependency Policy

Open core dependencies:

```text
prefer permissive licenses
avoid incompatible copyleft where unnecessary
pin/review security-sensitive dependencies
```

Maintain an SBOM/license inventory.

CI should check:

```text
cargo deny
dependency advisories
license compatibility
banned licenses
```

Recommended policy file:

```text
deny.toml
```

---

# 41. Third-Party License Boundary

A public Apache-2.0 repository does NOT mean every dependency becomes Apache-2.0.

Each dependency retains its own license.

Maintain:

```text
THIRD_PARTY_NOTICES.md
```

when required.

Never copy third-party source into the repository without preserving the applicable license/notice.

---

# 42. Build Matrix

## Public CI

```text
macOS Apple Silicon
macOS Intel
Windows x64
Linux check/build where useful
```

However:

```text
compile support ≠ product support
```

Product support remains explicitly documented.

Current platform priority:

```text
P0  macOS Apple Silicon
P1  macOS Intel
P1  Windows x64
P2  Windows ARM64
```

Intel and Windows should not be marketed as fully supported until real-device smoke tests pass.

---

# 43. Free Release Pipeline

```text
GitHub public repo
        ↓
GitHub Actions
        ↓
cargo test
cargo clippy
cargo build
        ↓
free artifact
        ↓
optional signed public release
```

No private credentials required.

---

# 44. Pro Release Pipeline

```text
Public maclean revision
        +
Private maclean-pro revision
        ↓
Private CI
        ↓
integration tests
        ↓
license integration tests
        ↓
build
        ↓
sign
        ↓
notarize
        ↓
release
```

The public repository should never be able to build the complete Pro product merely by toggling a local flag.

---

# 45. Team Release Pipeline

```text
Public core
      +
Private Pro
      +
Private Team
      ↓
Private CI
      ↓
integration
      ↓
security
      ↓
fleet/policy tests
      ↓
signed release
```

---

# 46. Public API Stability Policy

Once published, these contracts should be treated as compatibility-sensitive:

```text
StorageEntity
StorageObservation
DeveloperProject
CleanupCandidate
CleanupPolicy
CleanupRun
CLI JSON envelope
JSON schemas
SQLite migration semantics
```

Breaking changes require:

```text
contract version bump
migration
release notes
compatibility documentation
```

---

# 47. JSON Contract Versioning

Use:

```json
{
  "contract_version": "1",
  "request_id": "uuid",
  "command": "inventory",
  "status": "ok",
  "data": {}
}
```

Do not silently change field meanings.

Prefer:

```text
add field
```

over:

```text
rename field
```

For breaking changes:

```text
contract_version: 2
```

---

# 48. Commercial API Boundary

Private Pro code may consume public contracts:

```text
maclean-types
maclean-core
schemas
```

but public code must never depend on:

```text
maclean-pro
license-server
team-api
```

This preserves:

```text
public build independence
```

---

# 49. AI Boundary

AI code remains private.

Public core may expose:

```rust
pub struct ExplanationFacts {
    ...
}
```

Private Pro owns:

```text
prompt templates
provider adapters
local model adapters
ranking
response normalization
AI product UX
```

AI MUST NOT:

```text
delete files directly
override safety
bypass policy validation
upload raw file contents by default
```

Deterministic safety code remains authoritative.

---

# 50. Team Privacy Boundary

Team service should receive only what the customer enables.

Default:

```text
aggregate bytes
category totals
device health
policy status
cleanup statistics
```

Do NOT default-upload:

```text
file contents
source code
full filenames
full paths
AI prompts
private documents
Git repository contents
```

The local agent remains authoritative for file operations.

---

# 51. Team Policy Model

Future Team architecture:

```text
Admin
  ↓
Team policy
  ↓
Signed policy bundle
  ↓
Device
  ↓
Local validation
  ↓
Local safety gate
  ↓
Local execution
  ↓
Audit event
```

Server MUST NOT directly issue arbitrary filesystem delete commands.

---

# 52. Open-Core Security Model

Open source should improve trust.

Therefore publish:

```text
safety model
threat model
cleanup invariants
permission model
data handling
basic scanner logic
```

Keep private:

```text
commercial heuristics
license issuance
billing
customer management
premium rules
Team backend
```

This produces a strong message:

> “You can inspect how maclean protects your files. You do not need to trust a black-box deletion engine.”

---

# 53. What Competitors Can Copy

Assume competitors can copy:

```text
scanner paths
basic cache categories
basic UI concepts
basic CLI commands
basic JSON schema
basic cleanup patterns
```

Do not build the business around secrecy of these items.

The moat is:

```text
Storage Intelligence model
        +
Project graph
        +
Lifecycle model
        +
Growth history
        +
Forecasting
        +
Policy automation
        +
Safety reputation
        +
Distribution
        +
Brand
```

---

# 54. The Real Commercial Moat

Target internal model:

```text
Entity
  ↓
Project
  ↓
Tool
  ↓
Artifact
  ↓
Lifecycle
  ↓
Usage
  ↓
Regenerability
  ↓
Risk
  ↓
Action
```

This is the knowledge graph behind maclean.

The open source engine can expose the mechanics.

The commercial product can continuously improve the intelligence layer.

---

# 55. Repository Naming

Recommended:

```text
maclean
maclean-pro
maclean-team
maclean-license-server
```

Avoid:

```text
maclean-secret
maclean-private-secret
maclean-hidden
```

Use business/domain semantics rather than secrecy semantics.

---

# 56. Branch Strategy

Public:

```text
main
develop (optional)
feature/*
release/*
```

Protect:

```text
main
```

Private:

```text
main
release/*
feature/*
security/*
```

Commercial code should never be merged into the public repository merely to simplify release engineering.

---

# 57. Git Submodule Strategy

Do NOT put private Pro code into the public repository as a Git submodule.

Why:

```text
public repo
   ↓
submodule URL
   ↓
private repository
```

This reveals the commercial repository topology and complicates builds.

Prefer:

```text
private super-workspace
```

or:

```text
private package/registry
```

---

# 58. Monorepo vs Multi-Repo Decision

Final decision:

```text
Public Open Core:
    maclean

Private Commercial:
    maclean-pro

Future Team:
    maclean-team
```

This is preferred over one public monorepo with hidden directories.

Reason:

```text
clear license boundaries
clear CI boundaries
clear access control
clear contributor expectations
lower accidental leakage risk
```

---

# 59. Documentation Boundary

Public docs:

```text
architecture overview
scanner development
safety model
CLI reference
JSON schemas
contribution guide
security policy
```

Private docs:

```text
Pro architecture
pricing
entitlement internals
license server
billing
premium heuristics
Team architecture
customer playbooks
commercial roadmap
```

---

# 60. README Positioning

The public README should say:

```text
maclean is an open-core developer storage intelligence engine and desktop utility.

The open core provides scanning, inventory, classification, safety primitives,
basic cleanup, and structured CLI output.

Advanced intelligence, automation, forecasting, premium rules, licensing,
and Team capabilities are commercial components.
```

Do not pretend the entire product is open source.

---

# 61. Open-Core Product Naming

Use:

```text
maclean Open Core
maclean Free
maclean Pro
maclean Team
```

Avoid:

```text
maclean Community Edition
maclean Enterprise Edition
```

until Team/enterprise features actually exist.

---

# 62. Pricing Relationship

The source license and product price are independent.

Recommended:

```text
Open Core       $0 source
Free            $0 binary
Pro             $29.99 lifetime
Founding Pro    $19.99 lifetime
Team            future subscription
```

Do not imply:

```text
Apache source
=
free official Pro
```

They are different products.

---

# 63. Why a Competitor Cannot Simply Take maclean

They can take Apache-licensed code.

That is allowed.

They cannot automatically take:

```text
maclean trademark
official release identity
private Pro code
private Team backend
commercial license service
premium rules
official support
```

Therefore the business is not based on:

```text
"nobody can copy our code"
```

It is based on:

```text
"the best official product is continuously better than the base engine."
```

---

# 64. Contribution Funnel

Public contributor flow:

```text
Issue
  ↓
Discussion
  ↓
PR
  ↓
DCO
  ↓
CI
  ↓
CODEOWNERS review
  ↓
merge
  ↓
release
```

Contributors should primarily work on:

```text
scanner support
platform compatibility
bug fixes
performance
tests
safety
documentation
CLI
basic integrations
```

Commercial contributors may work privately on:

```text
Pro intelligence
policy
automation
billing
Team
```

---

# 65. Security Disclosure

Public repository:

```text
SECURITY.md
```

Security issues should not be discussed publicly before mitigation.

Especially:

```text
path traversal
privilege escalation
unsafe deletion
symlink race
license bypass
secret leakage
remote execution
```

---

# 66. License Bypass Is Not a Security Boundary

The Pro license system should be designed so that bypassing entitlement does not compromise user safety.

Good:

```text
license bypass
→ unlocks feature
→ commercial loss
```

Bad:

```text
license bypass
→ changes filesystem safety
→ arbitrary deletion
```

Safety must remain independent.

---

# 67. Offline License Behavior

Existing users should not be bricked by a temporary licensing outage.

Recommended:

```text
activation
   ↓
signed local entitlement
   ↓
offline verification
   ↓
grace period
   ↓
periodic refresh
```

The client verifies signatures locally.

The server remains the entitlement authority, not the runtime dependency for every cleanup operation.

---

# 68. Source Distribution Policy

Open source release:

```text
source
+
license
+
notice
+
build instructions
```

Commercial release:

```text
signed binary
+
EULA
+
privacy policy
+
commercial license
+
support policy
```

Do not confuse:

```text
source license
```

with:

```text
binary EULA
```

They are separate legal instruments.

---

# 69. EULA

Pro should have a separate EULA covering:

```text
license grant
installation limits
activation
support
updates
refunds
warranty disclaimer
liability
acceptable use
termination
```

The Apache-2.0 license applies only to the open-source components.

---

# 70. Privacy Policy

Because maclean is a local-first utility, the privacy position should be strong:

```text
basic scan:
local

filesystem data:
local

raw file contents:
not uploaded by default

Team:
explicit opt-in aggregate telemetry/control plane
```

This is part of the product trust moat.

---

# 71. Release Artifact Naming

Recommended:

```text
maclean-free-macos-arm64
maclean-free-macos-x86_64
maclean-free-windows-x86_64

maclean-pro-macos-arm64
maclean-pro-macos-x86_64
maclean-pro-windows-x86_64
```

Avoid exposing private crate names in end-user artifacts.

---

# 72. Versioning

Use one product version:

```text
maclean 1.0.0
```

Internally:

```text
core_version
pro_version
team_version
contract_version
license_schema_version
```

can evolve independently.

Example:

```json
{
  "product_version": "1.4.0",
  "core_version": "1.4.0",
  "contract_version": 1,
  "license_schema_version": 2
}
```

---

# 73. Migration Plan From Current Repository

## Phase 0 — Freeze

Do not immediately publish a license.

Create:

```text
OPEN_CORE_MIGRATION branch
```

---

## Phase 1 — Audit

Audit:

```text
Git history
dependencies
secrets
license compatibility
CI
release artifacts
```

---

## Phase 2 — Classify

Every source directory gets:

```text
OPEN
PRIVATE
MOVE
DELETE
REWRITE
```

classification.

---

## Phase 3 — Extract Contracts

Create:

```text
maclean-types
```

Move stable domain types there.

---

## Phase 4 — Extract Core

Create:

```text
maclean-core
```

Move:

```text
scanner
inventory
classification
safety
basic cleanup
```

---

## Phase 5 — Create Private Pro

Create:

```text
maclean-pro
```

Move:

```text
forecast
advanced history
policy
automation
advanced intelligence
premium rules
entitlement
```

---

## Phase 6 — Rebuild Free

Public:

```bash
cargo build --workspace
```

must produce a useful Free application.

---

## Phase 7 — Rebuild Pro

Private CI:

```text
public core
+
private Pro
=
Pro release
```

---

# 74. Existing maclean Code Mapping

Based on the current repository architecture:

## Keep open

```text
cache_registry.rs
dev_cache.rs
app_cache.rs
apfs.rs
large_files.rs
uninstall.rs
residual_match.rs
windows_apps.rs
```

where they are basic scanner/safety implementations.

Also keep open:

```text
sanitize_before_delete
protected path handling
TOCTOU checks
symlink checks
Trash behavior
```

---

## Candidates for private extraction

If currently present as reusable commercial logic, move:

```text
advanced dashboard aggregation
growth analytics
forecasting
advanced reclaim ranking
policy orchestration
scheduled smart cleanup
premium rule selection
AI explanation
commercial entitlement
license service
billing
```

Do not prematurely privatize basic scanner code merely because it is useful.

---

# 75. Existing License System

Current Ed25519 license verification is a good foundation.

Keep:

```text
public verification key
license parser
local signature verification
feature gate interfaces
```

in the client as needed.

Keep private:

```text
private signing key
issuer
activation service
refresh service
deactivation service
customer mapping
billing integration
```

---

# 76. Stripe Boundary

Stripe belongs entirely outside the public core.

Architecture:

```text
Website
  ↓
Checkout
  ↓
Stripe
  ↓
Webhook
  ↓
Billing Adapter
  ↓
Entitlement Service
  ↓
License Issuer
```

Never put:

```text
Stripe secret
Webhook signing secret
customer payment metadata
```

in public source.

---

# 77. Future Team Boundary

Team is a separate business layer.

Public:

```text
local policy schema
local executor
safety primitives
```

Private:

```text
organization
device
policy distribution
audit
billing
SSO
dashboard
fleet analytics
```

---

# 78. What Happens If Someone Forks maclean?

Expected and acceptable.

They may:

```text
fork
modify
build
redistribute
sell
```

the Apache-2.0 portions, subject to the license terms.

They may not automatically:

```text
claim official maclean
use private Pro source
use Team infrastructure
use private signing keys
represent themselves as official
```

The project should treat successful forks as ecosystem validation rather than a failure.

---

# 79. What Happens If Someone Sells a Fork?

This is allowed for Apache-2.0-covered code.

Do not fight the license.

Compete through:

```text
better UX
better intelligence
better rules
better support
better updates
better integrations
better brand
```

If this is unacceptable commercially, do not place that component under an open-source license.

This is the fundamental strategic decision.

---

# 80. When NOT to Open a Component

Do not open a component if:

```text
it is the primary direct source of Pro revenue
AND
it is easy to substitute
AND
its source disclosure would materially reduce pricing power
AND
there is no ecosystem benefit from opening it
```

Keep it private.

---

# 81. When to Open a Component

Open a component if:

```text
it improves trust
OR
it improves adoption
OR
it improves contribution
OR
it creates ecosystem integrations
OR
it establishes a standard
OR
it is not a meaningful commercial moat
```

Examples:

```text
scanner API
storage schema
safety model
CLI JSON contract
basic scanners
```

---

# 82. Decision Matrix

| Component | Open? | Why |
|---|---:|---|
| Scanner trait | YES | Ecosystem |
| Basic scanners | YES | Contribution |
| Safety engine | YES | Trust |
| StorageEntity | YES | Contract |
| SQLite core schema | YES | Transparency |
| CLI JSON | YES | Integration |
| Basic project discovery | YES | Adoption |
| Basic cleanup | YES | Useful product |
| Advanced reclaim score | NO | Intelligence |
| Forecast | NO | Differentiation |
| Smart policy | NO | Automation |
| Premium rules | NO | Knowledge |
| AI integration | NO | Commercial UX |
| Entitlement | NO | Revenue |
| Billing | NO | Security |
| License issuer | NO | Security |
| Team backend | NO | SaaS |
| Team dashboard | NO | SaaS |

---

# 83. Governance

Project owner retains final authority over:

```text
license
trademark
release
security
commercial boundary
roadmap
breaking API changes
```

Community may influence:

```text
bugs
features
scanner additions
documentation
platform support
```

This avoids governance ambiguity during the commercial phase.

---

# 84. Required Public Files

Before declaring the repository officially open source, add:

```text
LICENSE
NOTICE
README.md
CONTRIBUTING.md
SECURITY.md
CODE_OF_CONDUCT.md
GOVERNANCE.md
DCO
TRADEMARK_POLICY.md
THIRD_PARTY_NOTICES.md
```

Some may be introduced incrementally, but `LICENSE` is mandatory for the intended open-source grant.

---

# 85. Required Private Repositories

Create:

```text
maclean-pro
maclean-license-server
```

Immediately.

Create later:

```text
maclean-team
```

when Team demand is validated.

---

# 86. Required GitHub Controls

Public:

```text
branch protection
CODEOWNERS
secret scanning
Dependabot
dependency review
required CI
release permissions
```

Private:

```text
same baseline
+
restricted organization access
+
protected environments
+
production signing approval
```

---

# 87. Required CI Checks

Public:

```text
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --workspace
cargo deny check
```

Plus platform builds.

Private:

```text
all public checks
+
Pro integration tests
+
license tests
+
release signing
+
notarization
+
commercial regression tests
```

---

# 88. License Test Suite

Add tests for:

```text
valid license
expired license
invalid signature
wrong product
wrong plan
wrong machine
offline grace
clock rollback
unknown feature
future schema
revoked license
```

The entitlement layer must fail closed for Pro features without affecting basic safety.

---

# 89. Commercial Boundary Test

CI should verify:

```text
public repository contains no private crate references
public build does not require private credentials
public build does not fetch private repositories
public artifacts do not contain Pro source
```

Private CI verifies:

```text
Pro contains expected commercial modules
Pro build has entitlement enforcement
Team build has Team enforcement
```

---

# 90. Accidental Leakage Prevention

Add CI rules for:

```text
grep private package names
grep Stripe secret patterns
grep license private key patterns
grep Team service URLs
grep internal repository URLs
```

Use secret scanners as a second line of defense.

---

# 91. Public Release Rule

A public release is allowed only if:

```text
LICENSE correct
+
secret scan clean
+
dependency audit clean
+
public build reproducible
+
no private dependency
+
no private artifact
```

---

# 92. Commercial Release Rule

A Pro release is allowed only if:

```text
core revision pinned
+
Pro revision pinned
+
license tests pass
+
signing succeeds
+
notarization succeeds
+
smoke tests pass
+
upgrade path verified
```

---

# 93. Long-Term Architecture

Final target:

```text
                         maclean
                            │
             ┌──────────────┴──────────────┐
             │                             │
        OPEN CORE                     COMMERCIAL
        Apache-2.0                    Proprietary
             │                             │
     ┌───────┼───────┐             ┌───────┼────────┐
     │       │       │             │       │        │
  Scanner  Safety  CLI           Pro     License   Billing
     │       │       │             │
     └───────┴───────┘             │
             │                      │
             └──────────┬───────────┘
                        │
                       Free
                        │
                       Pro
                        │
                       Team
```

---

# 94. Strategic Position

The product should be described as:

> **maclean is an open-core Developer Storage Intelligence platform.**

Not:

> “an open-source CleanMyDev clone.”

Not:

> “a free disk cleaner.”

Not:

> “a closed-source disk cleaner with a GitHub mirror.”

The open core is the foundation.

The commercial product is the intelligence layer.

---

# 95. Final Decisions — Frozen for V1

The following decisions are considered **FROZEN** unless a deliberate architecture review changes them:

### License

```text
Open Core → Apache-2.0
Pro → Proprietary
Team → Proprietary
```

### Repository

```text
maclean       → Public
maclean-pro   → Private
license-server→ Private
maclean-team  → Future Private
```

### Cargo

```text
Public workspace → independent build
Private workspace → composes public core + Pro
```

### Free

```text
Scan
Inventory
Classification
Safety
Basic cleanup
Basic CLI
Basic JSON
```

### Pro

```text
Intelligence
History
Forecast
Policy
Automation
Premium rules
AI
Entitlement
```

### Team

```text
Fleet
Policy
Audit
SSO
Central dashboard
Billing
```

### Security

```text
Private signing key → never source-controlled
Stripe secrets → never public
Customer secrets → never public
```

### Contributors

```text
DCO first
CLA later if legally/business required
```

### Brand

```text
Apache source rights ≠ maclean trademark rights
```

---

# 96. Implementation Checklist

## Repository

- [ ] Audit entire Git history
- [ ] Audit branches/tags
- [ ] Audit GitHub Actions
- [ ] Audit release artifacts
- [ ] Audit dependencies
- [ ] Add `LICENSE`
- [ ] Add `NOTICE`
- [ ] Add `CONTRIBUTING.md`
- [ ] Add `SECURITY.md`
- [ ] Add `CODE_OF_CONDUCT.md`
- [ ] Add `GOVERNANCE.md`
- [ ] Add DCO
- [ ] Add trademark policy

## Cargo

- [ ] Create `maclean-types`
- [ ] Create `maclean-core`
- [ ] Create `maclean-storage`
- [ ] Create `maclean-platform`
- [ ] Create `maclean-cli`
- [ ] Create `maclean-free`
- [ ] Create private `maclean-pro`

## Commercial

- [ ] Move Pro intelligence
- [ ] Move forecast
- [ ] Move policy
- [ ] Move automation
- [ ] Move premium rules
- [ ] Move entitlement
- [ ] Move billing
- [ ] Create private license server

## Security

- [ ] Rotate any historical secrets
- [ ] Verify Ed25519 private key never existed in public history
- [ ] Verify Stripe secrets never existed in public history
- [ ] Configure secret scanning
- [ ] Configure dependency scanning
- [ ] Configure protected release environments

## Release

- [ ] Public build works without private repo
- [ ] Pro build works from pinned public revision
- [ ] macOS Apple Silicon smoke test
- [ ] macOS Intel smoke test
- [ ] Windows x64 smoke test
- [ ] Signing
- [ ] Notarization
- [ ] Checksums
- [ ] Release notes

---

# 97. Final Recommendation to the Project Owner

Do **not** make the entire existing `maclean` repository Apache-2.0 immediately.

First:

```text
audit
  ↓
split
  ↓
extract public contracts
  ↓
extract open core
  ↓
extract private Pro
  ↓
verify public build
  ↓
add Apache-2.0
  ↓
publish
```

The current repository being public only because it was temporarily made public for development access does **not** obligate the project to remain fully open source.

If the codebase cannot yet be cleanly separated, keeping the repository private until the split is complete is safer than publishing the entire product under an irreversible permissive license.

The final business model should be:

```text
OPEN SOURCE
    ↓
trust + distribution + contributors + SEO
    ↓
FREE
    ↓
adoption
    ↓
PRO
    ↓
intelligence + automation + convenience
    ↓
TEAM
    ↓
fleet + policy + audit + SaaS
```

That is the recommended V1 governance model for maclean.
