# 07 Smart Cleanup PRD

## Goal
Provide policy-driven cleanup without turning maclean into a dangerous auto-delete utility.

## Policy lifecycle
Draft → Validate → Dry Run → Review → Enable → Execute → Audit → Restore

## Policy example

```yaml
name: weekly-developer-safe
schedule: weekly
rules:
  - target: xcode.derived_data
    max_age_days: 30
  - target: node_modules
    condition: project_inactive_days >= 90
  - target: docker.build_cache
    min_reclaim_gb: 5
protected:
  - git_repositories
  - ai_models
  - user_documents
```

## Dry-run output
Must show:
- candidate count
- reclaimable bytes
- blocked candidates
- warnings
- exact paths
- cleanup method

## Safety
- reuse existing `sanitize_before_delete`
- revalidate immediately before action
- no policy may bypass safety gates
- no destructive candidate is auto-selected
- policy version stored with every run

## CLI
Proposed:
- `maclean policy list`
- `maclean policy validate <file>`
- `maclean policy dry-run <name> --format jsonl`
- `maclean policy apply <name> --yes`
- `maclean policy history`

## Scheduled runs
Existing schedule support becomes policy execution.
Every run writes:
- policy_id
- policy_version
- timestamp
- machine_id local hash
- candidates
- succeeded
- failed
- blocked
- reclaimable bytes

## Undo
Undo should reference the existing backup/restore model. Where the underlying operation is inherently irreversible, the UI must say so before confirmation.
