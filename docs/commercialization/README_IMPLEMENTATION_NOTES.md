# Implementation Notes

This package is intentionally product/engineering oriented. It is not a drop-in code patch.

## Current repository alignment
The current public repository documents:
- Rust + egui/eframe
- macOS primary platform
- 60+ scan categories
- CLI/GUI shared safety gate
- JSON/JSONL
- schedule
- backups/restore
- Ed25519 licensing
- CI gates

The commercial architecture therefore proposes an incremental refactor instead of rewriting the cleanup engine.

## First code modules to create
```text
crates/maclean-core/src/storage/
  entity.rs
  observation.rs
  classification.rs
  project.rs
  growth.rs
  forecast.rs

crates/maclean-core/src/policy/
  model.rs
  validator.rs
  planner.rs
  executor.rs

crates/maclean-core/src/intelligence/
  reclaim.rs
  explanations.rs
```

## Recommended SQLite tables
```text
storage_entities
storage_observations
developer_projects
storage_relations
cleanup_candidates
cleanup_policies
cleanup_policy_versions
cleanup_runs
cleanup_run_items
```

## First API contract
```text
scan → inventory
inventory → classify
inventory → project-link
inventory + history → growth
growth → forecast
candidate + policy → cleanup-plan
cleanup-plan → safety gate
safety gate → executor
executor → cleanup-run
```

## Important
Do not implement the Team cloud before the local domain model stabilizes.
