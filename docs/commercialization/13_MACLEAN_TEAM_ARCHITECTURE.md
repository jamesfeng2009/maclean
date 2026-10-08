# 13 maclean Team Architecture

## Launch condition
Do not build Team before:
- >1,000 desktop installs
- measurable Pro conversion
- recurring usage
- evidence that teams care about aggregate storage

## Architecture

```text
maclean Desktop
   ↓ signed device telemetry
Team API
   ↓
PostgreSQL
   ↓
Team Console
```

## Privacy-first device reporting
Default telemetry should contain:
- anonymous device ID
- app version
- OS version
- storage totals
- category aggregates
- reclaimable aggregates
- policy state

Never send:
- filenames
- full paths
- file contents
- project source
- AI prompts

unless explicitly enabled by an enterprise policy.

## Team entities
Organization
Member
Device
Policy
PolicyAssignment
StorageSnapshot
CleanupRun
AuditEvent

## Team dashboard
- fleet total storage
- fleet reclaimable
- top categories
- growth
- outlier devices
- policy compliance

## Policy model
Central policy → signed policy bundle → local validation → local execution.

The server never directly deletes files.

## Future
- SSO/SAML
- SCIM
- RBAC
- MDM integration
- Jamf integration
- enterprise reporting

These are P3 and must not delay desktop product-market fit.
