# 05 — Public Core Contracts

## Public domain

```rust
pub struct StorageEntity { ... }
pub struct StorageObservation { ... }
pub struct DeveloperProject { ... }
pub struct CleanupCandidate { ... }
pub struct CleanupPolicy { ... }
pub struct CleanupRun { ... }
```

## Public safety interface

```rust
pub trait SafetyGate {
    fn validate(&self, candidate: &CleanupCandidate) -> SafetyDecision;
}
```

## Public scanner interface

```rust
pub trait Scanner {
    fn scan(&self, context: &ScanContext) -> Result<Vec<StorageEntity>, ScanError>;
}
```

## Public cleanup interface

```rust
pub trait CleanupExecutor {
    fn preview(&self, candidates: &[CleanupCandidate]) -> CleanupPlan;
    fn execute(&self, plan: &CleanupPlan) -> Result<CleanupRun, CleanupError>;
}
```

## Pro extension

```rust
pub trait StorageIntelligence {
    fn analyze(&self, inventory: &Inventory) -> IntelligenceSummary;
}
```

Free implementation:

```rust
BasicStorageIntelligence
```

Pro implementation:

```text
private:
ProStorageIntelligence
```

The public contract must not expose private algorithms.

---

# Safety invariant

No intelligence implementation may directly delete files.

```text
Scanner
  ↓
Candidate
  ↓
SafetyGate
  ↓
Executor
```

AI must never bypass this chain.
