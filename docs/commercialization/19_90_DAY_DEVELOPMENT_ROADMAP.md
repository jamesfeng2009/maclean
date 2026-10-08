# 19 90-Day Development Roadmap

## Sprint 0 — Days 1–7
### Foundation
- freeze current cleanup behavior
- create domain ADR
- define StorageEntity
- define StorageObservation
- define CleanupCandidate
- define Policy
- establish regression fixtures
- document current safety invariants

Exit:
- existing tests green
- no change in deletion semantics

## Sprint 1 — Days 8–21
### Developer Storage Inventory
- wrap current scanners
- normalized category IDs
- classification
- project detection
- reclaimable calculation
- SQLite observation store

Exit:
- dashboard can show developer storage and reclaimable storage.

## Sprint 2 — Days 22–35
### Dashboard
- hero storage card
- category breakdown
- project drawer
- recent growth
- review flow
- Safety Center

Exit:
- a new user can understand disk usage without reading docs.

## Sprint 3 — Days 36–49
### History / Forecast
- snapshots
- delta engine
- growth ranking
- low-space forecast
- local history retention

Exit:
- “what changed?” and “when will disk fill?” work deterministically.

## Sprint 4 — Days 50–63
### Smart Cleanup
- policy schema
- validator
- dry run
- policy execution
- audit history
- existing safety gate integration

Exit:
- scheduled policy can clean only high-confidence candidates.

## Sprint 5 — Days 64–72
### Pro / Entitlement
- feature gates
- license server
- Stripe webhook adapter
- activation
- deactivation
- offline grace
- support tooling

Exit:
- end-to-end purchase → license → activation.

## Sprint 6 — Days 73–80
### AI Explanation
- structured explanation adapter
- local template fallback
- optional provider
- no AI-controlled deletion
- evaluation fixtures

Exit:
- every supported candidate gets a safe explanation.

## Sprint 7 — Days 81–90
### Launch
- landing page
- pricing
- privacy/terms/refund
- notarized build
- crash/error telemetry only if explicitly privacy-safe
- documentation
- GitHub README
- launch content

## Engineering priorities
P0:
- storage model
- dashboard
- classification
- safety
- policy
- history

P1:
- forecast
- AI explanation
- Pro licensing

P2:
- Team

## Release gates
1. zero known unsafe deletion regressions
2. deterministic CLI output
3. offline scan works
4. paid feature gating works without weakening safety
5. license server outage does not brick activated users
6. uninstall leaves no unexpected privileged process
7. privacy review complete
