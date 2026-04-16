# Workspace Test Roots

Reserved integration surfaces:

- `integration/`: cross-crate correctness tests
- `replay/`: protocol replay fixtures and tests
- `fault/`: injected failure and recovery tests
- `legacy-import/`: migration/import tests against legacy state

Current live verification entry points:

- `crates/pcloud-daemon/tests/live_auth.rs`: ignored production-path auth checks that require explicit environment variables such as `PCLOUD_LIVE_AUTH_TOKEN`
