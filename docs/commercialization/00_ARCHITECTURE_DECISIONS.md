# maclean Commercialization V1 — Architecture Decisions

## ADR-001: maclean is not a generic cleaner
Decision: Developer Storage Intelligence & Automation.
Reason: direct developer-cleaner competition is already crowded.

## ADR-002: `maclean-core` remains the source of truth
GUI, CLI, scheduled jobs and future Team device agents consume the same domain engine.

## ADR-003: Scanner != Cleaner
Scanners only discover and classify. Executors perform actions through safety gates.

## ADR-004: AI != Safety
AI explains and summarizes. Deterministic code decides whether an action is permitted.

## ADR-005: Free scan is real
The free product must demonstrate the value of the intelligence layer.

## ADR-006: Lifetime desktop license first
Recurring Team value is deferred until proven.

## ADR-007: Local-first by default
No raw file contents, project source or paths are uploaded by default.

## ADR-008: Windows is secondary in the first commercial milestone
macOS developer product-market fit first. Windows requires real-device safety validation before equivalent marketing claims.

## ADR-009: History is local
Storage observations live locally by default. Team reporting is opt-in and aggregate-only.

## ADR-010: Safety invariants are non-negotiable
No monetization or feature flag may bypass:
- path validation
- protected application checks
- TOCTOU protection
- privilege boundaries
- restore/Trash semantics
