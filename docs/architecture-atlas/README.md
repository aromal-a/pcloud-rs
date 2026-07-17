# pcloud-rs Architecture & Feature Atlas

Source-derived complete feature encyclopedia, architecture, and exhaustive
project inventory for implementers, library consumers, CLI/API users,
operators, security teams, and enterprise evaluators.

Generate, validate, and build:

```bash
python3 docs/architecture-atlas/tools/generate.py
python3 docs/architecture-atlas/tools/check_feature_coverage.py
python3 docs/architecture-atlas/tools/check_links.py
mdbook build docs/architecture-atlas
```

Serve for local or lab access:

```bash
mdbook serve docs/architecture-atlas \
  --hostname 0.0.0.0 \
  --port 12002
```

Hand-authored chapters live under `src/`. Files under `src/generated/` are
replaced by the generator. `check_feature_coverage.py` fails if a Cargo
package or flag lacks feature rationale, a canonical capability row is omitted,
or any required curated/generated feature chapter disappears from navigation.
