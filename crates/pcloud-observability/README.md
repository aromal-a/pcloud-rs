# pcloud-observability

Metrics, tracing, and structured-log helpers for pcloud-rs.

## What this crate does

- Hosts the metric-family definitions used across the daemon.
- Ships an optional Prometheus text exporter.
- Ships an optional JSON log sink for structured log ingestion.

## Public API entry points

- `Metrics`, `MetricFamily`, `log::JsonSink`.
- `exporter::render` (feature `prometheus-exporter`).

## Features

- `prometheus-exporter` — enables the Prometheus text exporter. OFF by default.
- `json-logs` — enables the structured JSON log sink. OFF by default.

## Usage

```rust,no_run
use pcloud_observability::Metrics;

let m = Metrics::new();
let _ = m.snapshot();
```

## License

Dual-licensed under `MIT OR Apache-2.0`.

---

See also: [mdBook crate map](../../docs/book/src/architecture/crate-map.md).
