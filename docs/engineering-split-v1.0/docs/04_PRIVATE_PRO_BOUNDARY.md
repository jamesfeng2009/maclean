# 04 — Private Pro Boundary

## Pro crates

```text
maclean-pro-intelligence
maclean-pro-history
maclean-pro-forecast
maclean-pro-policy
maclean-pro-automation
maclean-pro-ai
maclean-pro-entitlement
maclean-pro-ui
```

## Responsibilities

### Intelligence

```text
reclaim scoring
advanced classification
growth attribution
advanced project graph
risk-adjusted prioritization
```

### History

```text
long-term observations
growth deltas
historical analytics
```

### Forecast

```text
time-to-threshold
trend model
growth anomaly detection
```

### Policy

```text
policy validation
candidate selection
policy planner
conflict resolution
advanced dry-run
```

### Automation

```text
scheduler
execution orchestration
audit aggregation
automation state
```

### AI

```text
prompt templates
LLM/local-model adapters
structured explanation generation
provider configuration
AI UI
```

### Entitlement

```text
activation
deactivation
refresh
offline grace
feature gates
license lifecycle
```

### UI

```text
advanced dashboard
forecast views
policy editor
automation UI
AI explanation UI
```

---

# Private code rule

If a function answers:

> “Which storage should the product recommend cleaning, and why?”

it is likely Pro intelligence.

If a function answers:

> “How do I safely remove this approved candidate?”

it belongs in the open safety/core layer.

That distinction should be maintained aggressively.
