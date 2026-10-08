# 02 Competitive Strategy

## Competitor tiers

### Tier A — Consumer cleaners
Examples: CleanMyMac-class products.
Threat: brand, polish, broad consumer scope.
Response: do not compete on breadth.

### Tier B — Developer cleaners
Examples: CleanMyDev, Storage Cleaner.
Threat: direct audience overlap.
Response: move one abstraction level upward.

### Tier C — Free/open-source utilities
Examples: MangoDisk and similar.
Threat: zero price.
Response: make intelligence, policy and automation the paid value.

### Tier D — Scripts
Threat: free, powerful, developer-native.
Response: provide a safer, observable, reusable policy engine.

## Strategic position

```text
Generic Cleaner
      ↓
Developer Cleaner
      ↓
Developer Storage Intelligence  ← maclean
      ↓
Developer Storage Management
      ↓
Team/Fleet Governance
```

## Competitive response rules

When competitor adds a scanner:
- add support only if it materially improves coverage.
- do not make scanner count the roadmap driver.

When competitor adds an AI feature:
- focus on deterministic evidence and local explainability.
- AI may summarize; policy engine decides.

When competitor lowers price:
- do not price-match.
- increase perceived product category and value.

When competitor adds automation:
- differentiate with policy preview, dry-run, audit log and deterministic safety gates.

## Battlecard

### “Why not CleanMyDev?”
Answer:
> If you only need a developer cleaner, it may be enough. maclean is designed for developers who want to understand storage over time, group it by project, forecast growth and automate safe cleanup.

### “Why not a shell script?”
Answer:
> Scripts are powerful but usually lack inventory history, safety classification, dry-run policy, restore records and a consistent GUI/CLI contract.

### “Why not MangoDisk?”
Answer:
> MangoDisk is a broad free cleaner. maclean focuses on the lifecycle of developer-generated storage and turns the cleanup engine into an observable, policy-driven system.

## Defensive moat
1. canonical storage entity model
2. local usage history
3. project graph
4. deterministic safety engine
5. policy engine
6. CLI/GUI parity
7. developer-specific telemetry without sending raw data off-device
8. community-contributed scanner definitions
