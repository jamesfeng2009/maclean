# 12 maclean Pro Architecture

## Objective
Evolve `maclean-core` into a reusable Storage Intelligence Engine without breaking the existing safety model.

## Target architecture

```text
maclean-core
├── scanner
│   ├── registry
│   ├── developer
│   ├── project
│   └── system
├── inventory
├── classification
├── intelligence
│   ├── growth
│   ├── forecast
│   ├── project_graph
│   └── reclaim_score
├── policy
├── cleanup
├── safety
├── restore
├── scheduler
├── entitlement
└── output
```

## New domain objects

### StorageEntity
Canonical storage fact.

### StorageObservation
Time-stamped observation of an entity.

### StorageRelation
Links entity → project/tool/artifact.

### CleanupCandidate
An entity plus deterministic action/safety evidence.

### CleanupPolicy
Versioned set of deterministic rules.

### CleanupRun
Immutable audit record.

### StorageForecast
Derived projection based on observations.

## Scanner contract

```rust
trait Scanner {
    fn id(&self) -> &'static str;
    fn version(&self) -> &'static str;
    fn scan(&self, ctx: &ScanContext) -> ScanResult;
}
```

Scanners do not delete.

## Action contract

```rust
trait CleanupExecutor {
    fn preview(&self, candidate: &CleanupCandidate) -> Preview;
    fn execute(&self, plan: &CleanupPlan) -> CleanupResult;
}
```

Executor must invoke the same safety gate used by GUI and CLI.

## Storage intelligence pipeline

```text
Scanner
 → Inventory
 → Normalize
 → Classify
 → Link to Project
 → Snapshot
 → Compare history
 → Calculate growth
 → Calculate reclaimable
 → Recommend
```

## Local database
Use SQLite for:
- observations
- projects
- cleanup runs
- policy versions
- local preferences

Do not persist raw file contents.
Store normalized paths only as needed.

## Migration from current code
1. Keep existing scanner implementations.
2. Wrap current results into `StorageEntity`.
3. Introduce `Observation`.
4. Add project linking.
5. Add classification.
6. Add history.
7. Add policy.
8. Add forecast.
9. Expose the same engine to GUI/CLI.

## API compatibility
Existing CLI commands remain.
Add:
- `inventory`
- `history`
- `growth`
- `forecast`
- `policy`

## Golden rule
**No Pro feature can bypass existing `sanitize_before_delete`, app protection or privilege gates.**

## Testing
Every scanner:
- fixture test
- size calculation test
- classification test
- safety test
- cleanup dry-run test
- regression test

## Performance
Scanner execution remains parallel.
History writes are batched.
UI receives incremental results.
