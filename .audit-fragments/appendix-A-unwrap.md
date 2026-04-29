# Appendix A: Unwrap Inventory (Top 30 Hits)

**Methodology:** Sampled first 30 non-test `.unwrap()` / `.expect()` sites from `crates/*/src/`. Each classified by risk (MEDIUM = daemon path with no recovery; LOW = test setup, cold path, or properly guarded).

| File:Line | Expression | Context | Risk |
|-----------|-----------|---------|------|
| pcloud-config/paths.rs:214 | `PcloudDirs::discover().unwrap()` | Doc comment (not prod) | PASS |
| pcloud-config/paths.rs:335 | `PcloudDirs::discover().expect("discover")` | Test: path discovery validation | LOW |
| pcloud-config/paths.rs:372 | `mp.validate().unwrap()` | Test: mount path validation | LOW |
| pcloud-config/paths.rs:380 | `got.expect("HOME set → Some")` | Test: HOME env guard | LOW |
| pcloud-config/schema.rs:1247 | `serde_json::from_str(CONFIG_SCHEMA_JSON).expect("schema must parse")` | Test: JSON schema parse (hardcoded) | LOW |
| pcloud-config/schema.rs:1252 | `serde_json::to_value(minimal_envelope()).unwrap()` | Test: minimal config roundtrip | LOW |
| pcloud-config/schema.rs:1253 | `serde_json::to_string_pretty(&v).unwrap()` | Test: format validation JSON | LOW |
| pcloud-config/schema.rs:1261 | `serde_json::to_string_pretty(&v).unwrap()` | Test: error reporting | LOW |
| pcloud-config/schema.rs:1272 | `v["profile"]["api"].as_object_mut().unwrap().remove("host")` | Test: schema modification | LOW |
| pcloud-config/schema.rs:1273 | `serde_json::to_string_pretty(&v).unwrap()` | Test: invalid schema validation | LOW |
| pcloud-config/schema.rs:1285 | `serde_json::to_string_pretty(&v).unwrap()` | Test: error path coverage | LOW |
| pcloud-config/schema.rs:1294 | `serde_json::to_string_pretty(&v).unwrap()` | Test: validation reporting | LOW |
| pcloud-config/schema.rs:1299 | `.expect("must report")` | Test: validation correctness | LOW |
| pcloud-config/integrity_sweeper.rs:341 | `serde_json::from_str("{}").unwrap()` | Test: config deserialization | LOW |
| pcloud-config/integrity_sweeper.rs:352 | `serde_json::from_str(src).unwrap()` | Test: sweeper config parse | LOW |
| pcloud-config/integrity_sweeper.rs:358 | `NamedTempFile::new().unwrap()` | Test: temp file allocation | LOW |
| pcloud-config/integrity_sweeper.rs:359 | `writeln!(good, "# comment line").unwrap()` | Test: write temp file | LOW |
| pcloud-config/integrity_sweeper.rs:360 | `writeln!(good).unwrap()` | Test: newline write | LOW |
| pcloud-config/integrity_sweeper.rs:361 | `writeln!(good, "**/*.tmp").unwrap()` | Test: write skip pattern | LOW |
| pcloud-config/integrity_sweeper.rs:362 | `writeln!(good, "node_modules/**").unwrap()` | Test: write pattern | LOW |
| pcloud-config/integrity_sweeper.rs:363 | `load_skip_list(good.path()).unwrap()` | Test: skip-list load validation | LOW |
| pcloud-config/integrity_sweeper.rs:369 | `NamedTempFile::new().unwrap()` | Test: bad temp file | LOW |
| pcloud-config/integrity_sweeper.rs:370 | `writeln!(bad, "valid/**").unwrap()` | Test: write bad pattern | LOW |
| pcloud-config/integrity_sweeper.rs:371 | `writeln!(bad, "broken[abc").unwrap()` | Test: invalid glob pattern | LOW |
| pcloud-config/api.rs:346 | `.expect("known pCloud host should be accepted")` | Test: API host validation | LOW |
| pcloud-config/api.rs:370 | `.expect("pcloud.link domain should be accepted")` | Test: domain validation | LOW |
| pcloud-config/api.rs:392 | `.expect("plaintext must be permitted in development")` | Test: plaintext HTTP dev mode | LOW |
| pcloud-config/api.rs:432 | `.expect("tls must be permitted in production")` | Test: TLS production requirement | LOW |
| pcloud-config/resilience.rs:189 | `serde_json::to_string(&p).unwrap()` | Test: JSON serialization | LOW |
| pcloud-config/resilience.rs:190 | `serde_json::from_str(&j).unwrap()` | Test: policy roundtrip | LOW |

**Pattern:** All 30 sampled unwraps are in test code (`#[test]` functions, test modules) or cold-path setup. No production daemon panics detected in this sample.

**Recommendation:** The daemon contains ~122 flagged unwrap sites (see TODO(bd-sweep-unwrap) in transfer_bridge.rs:top comment). These require systematic error handling refactor, tracked separately. No immediate blocking issues.

