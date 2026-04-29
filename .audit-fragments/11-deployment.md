## 11. Deployment & Operations

### Verdict

**STRONG PASS** with minor gaps. Linux systemd hardening is exceptional. macOS launchd and FreeBSD rc.d packaging present. Windows MSI with WinFSP integration ready. Configuration schema versioned with migrations. Prometheus metrics + alert rules shipped. Health checks (/livez, /readyz) present. SQLite migrations framework in place. Backup/restore CLI documented. FIPS posture documented as gap. Debian/RPM `.deb` build not yet wired to CI.

---

### 1. Linux systemd Packaging [EXCELLENT]

**File:** `packaging/systemd/pcloudd.service`

All required hardening directives present (lines 30–142):

| Directive | Value | Line |
|-----------|-------|------|
| User= | DynamicUser=yes (ephemeral) | 50 |
| ProtectSystem= | strict | 55 |
| ProtectHome= | tmpfs | 56 |
| PrivateTmp= | yes | 57 |
| ReadWritePaths= | /var/lib/pcloud-rs /var/log/pcloud-rs | 108 |
| MemoryMax= | 512M | 131 |
| RestartSec= | 5s | 40 |
| WatchdogSec= | 30s | 37 |
| NoNewPrivileges= | yes | 112 |
| CapabilityBoundingSet= | empty | 113 |

**Additional:** MemoryHigh=384M, CPUQuota=75%, TasksMax=256, LimitNOFILE=4096, RestrictAddressFamilies, SystemCallFilter with @mount blocked, ProtectKernelTunables/Modules/Logs/ControlGroups/Clock/Hostname, RestrictNamespaces, RestrictRealtime.

**Network & FUSE restrictions intentionally blocked** at source (lines 21–27); operators must install drop-ins (override.conf.example, override-fuse.conf.example).

---

### 2. macOS launchd Packaging [EXCELLENT]

**File:** `packaging/macos/com.pcloud.pcloudd.plist`

All required keys present:
- RunAtLoad: true (lines 62–63)
- KeepAlive: SuccessfulExit=false, Crashed=true (lines 65–71)
- ExitTimeOut: 30 seconds (line 75)
- UserName/GroupName: _pcloudd (lines 83–86)
- StandardOutPath/StandardErrorPath: separate log files (lines 91–95)
- ProcessType: Background (lines 88–89)
- ThrottleInterval: 10 (crash-loop protection, lines 80–81)

---

### 3. Windows Packaging [GOOD]

**Files:** `packaging/windows/wix/`, `packaging/signing/sign-windows.ps1`

Present:
- WiX MSI scaffolding with WinFSP 2.x bundled via CustomAction (README lines 28–46)
- WinFSP SHA256 verification and pinned versioning (lines 48–56)
- Authenticode signing script: signtool with SHA256 digest, timestamp URL (sign-windows.ps1 lines 59–67)
- EV certificate support for cloud HSM (lines 12–13)

**Gap:** No documented daemon-level error message if WinFSP missing at runtime.

---

### 4. FreeBSD Packaging [EXCELLENT]

**File:** `packaging/freebsd/pcloudd.rc`

Proper rc.d script:
- PROVIDE/REQUIRE/KEYWORD (lines 37–39)
- User privilege drop via `pcloudd_user` (line 49)
- **Kernel module preload:** `kldload -n fusefs` in `start_precmd()` (line 60)
- PID file tracking via rc.subr (lines 53–55)

---

### 5. Debian/RPM Packaging [GOOD]

**File:** `packaging/debian/nfpm.yaml`

Present:
- nfpm configuration (lines 36–94)
- Cross-compile support: amd64 (default), arm64 via NFPM_ARCH (lines 9–34)
- Binaries: /usr/bin/pcloudd, /usr/bin/pcloudc (lines 69–72)
- systemd unit inclusion (lines 73–77)
- Dependencies: libc6, libssl3, libsqlite3-0, libfuse3-3 (lines 55–60)
- Post-install/remove scripts (lines 87–88)

**Gap:** `.deb` build **not wired into CI** (lines 22–24; tracked pcloud-rs-s1p.69).

---

### 6. Configuration Schema & Migrations [EXCELLENT]

**Files:** `crates/pcloud-config/src/lib.rs`, `crates/pcloud-config/src/migrate.rs`

- Versioned envelope: v0→v1→v2 (migrate.rs lines 9–40)
- CURRENT_VERSION=2, MIN_SUPPORTED_VERSION=0 (lines 59, 62)
- Forward-only migrations; no downgrade (lines 19–22, 35–39)
- Every config module documented (lib.rs lines 18–32)
- Production TLS mandatory, auth vault opt-in, mode validation
- Example env file: `packaging/init/common/pcloudd.env.example` (39 lines)

---

### 7. Observability & Metrics [EXCELLENT]

**Files:** `crates/pcloud-observability/src/metrics.rs`, `ops/prometheus/pcloud-rs-alerts.yml`, `crates/pcloud-daemon/src/metrics_server.rs`

**Metrics exported:**
- pcloud_request_count{method, status} (counter)
- pcloud_request_latency_seconds{method} (histogram)
- pcloud_auth_attempts_total{result} (counter)
- pcloud_transfer_bytes_total{direction} (counter)
- pcloud_crypto_lock_state (gauge)
- pcloud_sync_root_count (gauge)
- pcloud_ipc_connected_clients (gauge)
- pcloud_panic_count (counter)

**Label sanitization:** Disallowed chars → opaque "invalid" (metrics.rs lines 40–46); no PII; 64-char cap.

**Alert rules shipped:** 7 rules (PcloudAuthAttemptSpike, PcloudHighAuthFailureRate, PcloudHighRequestLatency, PcloudCryptoUnexpectedlyLocked, PcloudIpcClientsExhausted, PcloudPanicDetected, PcloudTransferStale; lines 30–152).

**Endpoint security:** Binds 127.0.0.1 by default; wildcard requires Environment=Development + env var.

**Gap:** No Grafana dashboards shipped.

---

### 8. Health Checks [EXCELLENT]

**File:** `crates/pcloud-daemon/src/health_server.rs`

- **GET /livez** — always 200 OK (liveness probe)
- **GET /readyz** — 200 OK while Running, 503 during drain (readiness probe)
- Binds 127.0.0.1 only (line 15)
- Disabled by default (http_port=0); enabled via config [health] (lines 21–22)
- Port range [1024, 65535] enforced (line 66)
- MAX_CONCURRENT_HEALTH_CONNECTIONS=32 (line 55)

---

### 9. Upgrade Path [GOOD]

- Config migrations: v0→v1→v2 forward-only (migrate.rs)
- Backup/restore CLI: `pcloudc snapshot {create,verify,restore,prune}` (backup-snapshots.md lines 4–11)
- Pipeline: tar → zstd → SHA3-256 sidecar (lines 22–35)
- Zstd level tuning (1–22, default 3; lines 55–60)

**Gap:** Journal schema versioning not found; in-place restart semantics not documented.

---

### 10. Backup/Restore Documentation [GOOD]

**File:** `docs/book/src/operations/backup-snapshots.md`

Documents snapshot CLI tool (create, verify, restore, prune). Directory-level backup guidance for vault, SQLite, journal lacking.

---

### 11. Resource Limits [EXCELLENT]

**File:** `packaging/systemd/pcloudd.service`

- MemoryMax=512M, MemoryHigh=384M, CPUQuota=75%, TasksMax=256 (lines 131–134)
- LimitNOFILE=4096, LimitNPROC=256, LimitCORE=0 (lines 135–137)
- Server override documented (README line 74): MemoryMax=4G drop-in

---

### 12. Observability Endpoint Security [EXCELLENT]

**Metrics:** binds 127.0.0.1; wildcard requires dev environment + env var.
**Health:** binds 127.0.0.1 only; no wildcard option.
**Auth:** no auth layer (localhost-only access; acceptable).

---

### 13. FIPS Posture [MEDIUM GAP]

**Reference:** AUDIT_REPORT.md section 13.4

- **No FIPS claim** shipped
- Primitives: AES-256-GCM, SHA-256, SHA-512, HMAC, PBKDF2 (NIST-approved)
- **Argon2id** NOT FIPS-140-3 approved

**Gap:** No runtime FIPS mode switch. AUDIT_REPORT recommends `CryptoPolicy::fips_mode` gate to swap Argon2id → PBKDF2-HMAC-SHA-512.

---

### 14. Reproducible Builds [GOOD]

**Files:** `Cargo.toml`, `.github/workflows/release.yml`

- SOURCE_DATE_EPOCH extraction (release.yml line 43)
- cargo-auditable for SBOM embedding (lines 34–44)
- SHA-256 digest computation (lines 46–51)

**Gap:** Deterministic strip not verified.

---

### Summary

| Area | Status | Gap | Severity |
|------|--------|-----|----------|
| Linux systemd | ✓ | None | — |
| macOS launchd | ✓ | None | — |
| Windows MSI | Good | WinFSP error handling | LOW |
| FreeBSD rc.d | ✓ | None | — |
| Debian/RPM | Good | .deb CI integration | MEDIUM |
| Config schema | ✓ | None | — |
| Prometheus | ✓ | No dashboards | LOW |
| Health checks | ✓ | None | — |
| Upgrade path | Good | Journal versioning | MEDIUM |
| FIPS | MEDIUM | No runtime switch | HIGH |
| Repro builds | Good | Deterministic strip | LOW |

**Recommendation:** Implement optional `[crypto] fips_mode` gate for FIPS-140-3 deployments. Complete `.deb` CI integration. All tier-1 platforms hardened and production-ready.
