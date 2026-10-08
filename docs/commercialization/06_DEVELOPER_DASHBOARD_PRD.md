# 06 Developer Dashboard PRD

## Objective
Turn the first screen from a “clean button” into a storage control center.

## Information architecture

```text
Top
 ├─ Total Disk
 ├─ Free
 ├─ Developer Storage
 ├─ Reclaimable
 └─ 7-day Growth

Main
 ├─ Storage composition
 ├─ Developer categories
 ├─ Recent growth
 ├─ Projects
 └─ Recommended actions

Secondary
 ├─ History
 ├─ Policies
 └─ Safety Center
```

## Hero card

Example:
- Developer Storage: 231.4 GB
- Reclaimable: 174.2 GB
- 7-day growth: +38.1 GB
- Forecast: 23 days until low-space threshold

CTA:
`Review 174.2 GB`

## Category card
Each category shows:
- size
- percentage
- growth
- reclaimable
- confidence
- click-through

## Project drawer
Show:
- project path
- type
- last activity
- total footprint
- artifact footprint
- cleanup candidates
- “clean artifacts”
- “archive” future action

## Growth panel
Show top contributors:
- tool
- delta
- percentage of growth
- evidence

## Empty state
Never show “No junk”.
Show:
> Your developer storage looks healthy. We found no high-confidence cleanup candidates.

## Safety center
Visible before first cleanup:
- protected data rules
- Trash/restore behavior
- local processing
- permissions
- policy preview

## Performance
Initial dashboard target:
- first meaningful data < 2 seconds after cached inventory
- incremental scanner results stream into UI
- no blocking full-disk scan on app launch
