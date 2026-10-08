# 05 Developer Storage PRD

## Goal
Create a canonical, evidence-based model of developer-generated storage.

## Storage classes
- active
- regenerable
- downloadable
- stale
- user_data
- destructive
- unknown

## Entity model
`StorageEntity`
- id
- path
- logical_category
- tool
- project_id
- size_bytes
- file_count
- last_modified_at
- last_accessed_at: optional
- detected_usage_evidence
- regenerability
- risk
- recommended_action
- cleanup_method
- restore_capability
- scanner_id
- scanner_version
- confidence

## Project model
`DeveloperProject`
- id
- root_path
- project_type
- detected_tools
- git_remote_hash: optional, never raw remote by default
- last_activity_at
- total_size
- reclaimable_size
- active_size
- artifact_size

## Scanner contract
Every scanner must return:
1. stable scanner id
2. entities
3. evidence
4. risk classification
5. cleanup operation
6. explainability metadata
7. deterministic test fixture

## Classification rules
Example:
- `node_modules`: regenerable if package manifest + lockfile exists
- Xcode DerivedData: regenerable
- Git repository source: user_data
- Ollama model: downloadable/destructive, not regenerable
- project archive: user_data unless explicit cleanup policy
- Docker build cache: regenerable
- Docker named volume: user_data unless proven ephemeral

## Safety invariant
No scanner may directly delete files. It only proposes `CleanupCandidate`s.

## Acceptance
Every candidate must be explainable in one sentence:
> “This is X, it occupies Y, it was last observed Z, and deleting it causes Q.”
