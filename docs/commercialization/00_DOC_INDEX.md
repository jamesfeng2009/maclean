# maclean Commercialization V1 — Document Index

## Purpose
This package turns maclean from a developer cleaner into a **Developer Storage Intelligence & Automation** product.

## Product thesis
- CleanMyDev: developer-focused Mac cleaner.
- maclean: developer storage intelligence, lifecycle management, safe cleanup and automation.
- Core moat: explain *what* consumes storage, *why* it exists, *whether* it is regenerable, *when* it was last useful, and *how* to keep storage healthy over time.

## Recommended implementation order
1. 01_PRODUCT_POSITIONING.md
2. 02_COMPETITIVE_STRATEGY.md
3. 03_MONETIZATION_MODEL.md
4. 04_FREE_PRO_FEATURE_MATRIX.md
5. 05_DEVELOPER_STORAGE_PRD.md
6. 06_DEVELOPER_DASHBOARD_PRD.md
7. 07_SMART_CLEANUP_PRD.md
8. 08_AI_STORAGE_EXPLANATION_PRD.md
9. 12_MACLEAN_PRO_ARCHITECTURE.md
10. 09_LICENSE_AND_ENTITLEMENT_ARCHITECTURE.md
11. 10_STRIPE_PAYMENT_ARCHITECTURE.md
12. 11_LICENSE_SERVER_ARCHITECTURE.md
13. 13_MACLEAN_TEAM_ARCHITECTURE.md
14. 14_LANDING_PAGE_PRD.md
15. 15_SEO_GEO_AEO_STRATEGY.md
16. 16_GO_TO_MARKET_PLAN.md
17. 17_GITHUB_OPEN_CORE_STRATEGY.md
18. 18_PRICING_STRATEGY.md
19. 19_90_DAY_DEVELOPMENT_ROADMAP.md

## Added strategic document
- `MACLEAN_vs_CLEANMYDEV_POSITIONING_V1.md` is the single executive/competitive brief for product, engineering, marketing and founders.

## Existing repository facts used as constraints
The current repository already has:
- Rust + egui/eframe GUI
- macOS as the primary development/release platform
- 60+ scan categories
- Xcode, Docker, AI model/cache, package-manager and application cache scanners
- shared GUI/CLI cleanup safety gate
- CLI JSON/JSONL output and semantic exit codes
- scheduling, backups/restore, privilege handling
- Ed25519 license signing
- CI quality gates

These facts should be treated as current-state inputs, not as permission to preserve every existing UI or feature forever.

## Non-goals for V1
- Do not compete on generic consumer cleaning.
- Do not add dozens of low-value cache rules before improving intelligence.
- Do not build a cloud account requirement for desktop scanning.
- Do not make AI responsible for deletion decisions.
- Do not make subscription the only purchase option.
