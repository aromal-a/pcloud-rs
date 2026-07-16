# pcloud-rs Architecture Atlas

Source-derived architecture and exhaustive project inventory for implementers,
library consumers, CLI/API users, and operators.

Generate, validate, and build:

```bash
python3 docs/architecture-atlas/tools/generate.py
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
replaced by the generator.
