# 17 GitHub Open-Core Strategy

## Recommendation
Do not close the entire project immediately.

Use an open-core model.

## Open
`maclean-core`
- scanner interfaces
- safe classification framework
- core scanner definitions
- CLI basic scan
- JSON/JSONL contract
- safety primitives where practical

## Commercial
Potentially closed:
- advanced storage intelligence
- history/forecast UI
- premium scanner packs
- policy orchestration
- Pro entitlement integration
- Team features

## Why
Open source provides:
- developer trust
- SEO
- contributors
- scanner definitions
- reproducibility
- security review

Commercial layer provides:
- polished product
- support
- advanced intelligence
- automation
- licensing

## Repository structure

```text
maclean/
  crates/
    maclean-core/
    maclean-intelligence/
    maclean-policy/
  apps/
    maclean-desktop/
    maclean-cli/
  pro/
    entitlement/
    advanced-ui/
  docs/
```

If legal/licensing constraints require a different split, resolve the license before publishing code.

## Contribution model
Scanner contribution checklist:
- stable ID
- target paths
- classification
- safety evidence
- fixture
- platform guard
- explanation
- cleanup method

## Community roadmap
Public:
- scanner support
- safety fixes
- CLI
- platform compatibility

Commercial:
- product UX
- intelligence
- automation
- team

## Important
Do not falsely call closed components open source.
Publish exact license boundaries.
