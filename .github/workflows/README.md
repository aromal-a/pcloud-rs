# GitHub Actions intentionally disabled

This repository does not run CI or release automation on GitHub-hosted
infrastructure.

The former workflow definitions are retained under
`.github/workflows-disabled/` as migration history only. GitHub does not load
workflow YAML from that directory.

Run the repository-owned pipeline instead:

```bash
cargo xtask ci
```

See `docs/local-cicd.md` for platform prerequisites, individual stages,
artifacts, and release operation.
