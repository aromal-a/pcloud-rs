# Operations, deployment, tools, and cross-crate tests

This generated page covers **12** Git-visible files.

Kind summary: test: 9, YAML/config: 2, configuration: 1

| File | Kind | Source-derived role |
|---|---|---|
| [`deploy/prometheus/alerts.yml`](https://github.com/ezechiel203/pcloud-rs/blob/main/deploy/prometheus/alerts.yml) | YAML/config | T4.1 — Prometheus alert rules in tree. |
| [`ops/grafana/pcloud-rs-overview.json`](https://github.com/ezechiel203/pcloud-rs/blob/main/ops/grafana/pcloud-rs-overview.json) | configuration | Configuration used by the operations tests area. |
| [`ops/prometheus/pcloud-rs-alerts.yml`](https://github.com/ezechiel203/pcloud-rs/blob/main/ops/prometheus/pcloud-rs-alerts.yml) | YAML/config | Prometheus alerting rules for pcloud-rs daemon. |
| [`tests/README.md`](https://github.com/ezechiel203/pcloud-rs/blob/main/tests/README.md) | test | Workspace Test Roots |
| [`tests/dr_drill/README.md`](https://github.com/ezechiel203/pcloud-rs/blob/main/tests/dr_drill/README.md) | test | Disaster Recovery Drill Harness |
| [`tests/dr_drill/run.sh`](https://github.com/ezechiel203/pcloud-rs/blob/main/tests/dr_drill/run.sh) | test | DR drill driver. Runs every scenarios/*.sh, aggregates exit |
| [`tests/dr_drill/scenarios/_common.sh`](https://github.com/ezechiel203/pcloud-rs/blob/main/tests/dr_drill/scenarios/_common.sh) | test | shellcheck shell=bash |
| [`tests/dr_drill/scenarios/store_corruption.sh`](https://github.com/ezechiel203/pcloud-rs/blob/main/tests/dr_drill/scenarios/store_corruption.sh) | test | DR drill: SQLite store corruption. |
| [`tests/dr_drill/scenarios/sync_root_mass_eviction.sh`](https://github.com/ezechiel203/pcloud-rs/blob/main/tests/dr_drill/scenarios/sync_root_mass_eviction.sh) | test | DR drill: mass sync-root eviction. |
| [`tests/dr_drill/scenarios/vault_loss.sh`](https://github.com/ezechiel203/pcloud-rs/blob/main/tests/dr_drill/scenarios/vault_loss.sh) | test | DR drill: auth-vault loss. |
| [`tests/memprofile/gate_self_test.sh`](https://github.com/ezechiel203/pcloud-rs/blob/main/tests/memprofile/gate_self_test.sh) | test | tests/memprofile/gate_self_test.sh |
| [`tests/repro_build/diff_helper_self_test.sh`](https://github.com/ezechiel203/pcloud-rs/blob/main/tests/repro_build/diff_helper_self_test.sh) | test | diff_helper_self_test.sh |
