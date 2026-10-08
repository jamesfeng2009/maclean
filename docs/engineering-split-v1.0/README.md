# maclean Open Core Engineering Split V1.0

This package turns the `MACLEAN_OPEN_CORE_LICENSE_AND_REPOSITORY_STRATEGY_V1.md`
decision into an engineering migration plan.

It contains:

- repository/module classification: `OPEN / PRIVATE / MOVE / REWRITE`
- proposed public `maclean-core` Cargo workspace
- proposed private `maclean-pro` Cargo workspace
- public/private dependency rules
- migration script
- repository audit script
- post-migration validation script
- Cargo manifests and Rust skeletons
- CODEOWNERS and CI boundary examples

## Important

This package is designed to be applied to the existing `maclean` repository.
It does not silently delete or overwrite source code.

The migration script defaults to **DRY RUN**.

Run:

```bash
./scripts/maclean-open-core-migrate.sh /path/to/maclean
```

Only after reviewing the report:

```bash
./scripts/maclean-open-core-migrate.sh /path/to/maclean --apply
```

Then validate:

```bash
./scripts/maclean-open-core-validate.sh /path/to/maclean
```

The exact repository cannot be cloned from this execution environment, so the
mapping is based on the repository structure already established during the
maclean architecture review. The audit script discovers any additional files
and forces an explicit classification before final publication.
