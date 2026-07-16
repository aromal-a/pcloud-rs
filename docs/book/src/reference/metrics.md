# Metrics and health HTTP endpoints

Prometheus export is an optional daemon build surface. It is disabled in the
default `pcloud-daemon` feature set and therefore must not be assumed present
in a normal source build or future package.

Build and run it explicitly:

```console
$ cargo build -p pcloud-daemon --features metrics
$ target/debug/pcloudd serve
```

The listener binds to `127.0.0.1:9353` by default and exposes:

- `GET /metrics` — Prometheus text exposition;
- `GET /health` — `200` while the daemon snapshot is clean, otherwise `503`;
- `GET /slo` — JSON SLO state when configured, otherwise `503`.

`PCLOUD_METRICS_PORT` changes the port. Wildcard binding requires both
`PCLOUD_METRICS_BIND_ALL=1` and the daemon's Development environment; production
configuration remains loopback-only. The listener limits concurrent clients,
caps request headers, applies read/write timeouts, and never exposes raw
credentials or request payloads.

Core metric families include:

| Metric | Type | Labels |
|---|---|---|
| `pcloud_request_count` | counter | `method`, `status` |
| `pcloud_request_latency_seconds` | histogram | `method` |
| `pcloud_auth_attempts_total` | counter | `result` |
| `pcloud_transfer_bytes_total` | counter | `direction` |
| `pcloud_crypto_lock_state` | gauge | none |
| `pcloud_sync_root_count` | gauge | none |
| `pcloud_ipc_connected_clients` | gauge | none |
| `pcloud_panic_count` | counter | none |
| `flush_latency_seconds` | histogram | write outcome labels |

Labels are sanitized and length-bounded to avoid unbounded cardinality and
secret leakage. The implementation sources are
`pcloud-observability::metrics`, `pcloud-observability::exporter`, and
`pcloud-daemon::metrics_server`.
