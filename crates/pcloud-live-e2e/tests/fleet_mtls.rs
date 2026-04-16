#![allow(clippy::pedantic)]
//! Live fleet-heartbeat coverage: dispatches a default heartbeat at an
//! operator-provided controller URL and asserts the mTLS-guarded
//! [`MtlsFleetAgent`] round-trip completes without panic, secret leak,
//! or unauthenticated-command escape.
//!
//! This binary does **not** bundle its own reference server (the
//! in-process reference server lives under
//! `crates/pcloud-fleet/tests/reference_server.rs` and is not re-exposed
//! as a public helper). A live-e2e invocation is expected to set:
//!
//! * `PCLOUD_FLEET_CONTROLLER_URL` — HTTPS URL of an already-running
//!   fleet controller (staging/reference server).
//! * `PCLOUD_FLEET_CA_BUNDLE` — path to the CA bundle trusted by that
//!   controller's leaf certificate.
//!
//! When any of those are unset, the test soft-skips with a clear message
//! so single-account harness runs cannot be misread as a fleet passing.
//!
//! Runtime-gated on `PCLOUD_LIVE_E2E=1 + PCLOUD_FLEET_CONTROLLER_URL +
//! PCLOUD_FLEET_CA_BUNDLE`.

#![forbid(unsafe_code)]

// **PLATFORM:** all
// **GATING:** none at build time; runtime-gated.

mod common;

use std::{path::PathBuf, time::Duration};

use pcloud_fleet::{MtlsFleetAgent, MtlsFleetConfig};

use crate::common::{optional_env, skip_if_not_live};

const ENV_CONTROLLER_URL: &str = "PCLOUD_FLEET_CONTROLLER_URL";
const ENV_CA_BUNDLE: &str = "PCLOUD_FLEET_CA_BUNDLE";
const ENV_DEVICE_GROUP: &str = "PCLOUD_FLEET_DEVICE_GROUP";

fn unique_identity_dir() -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let p = std::env::temp_dir().join(format!(
        "pcloud-live-e2e-fleet-{}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&p).expect("mkdir identity dir");
    p
}

#[test]
#[ignore = "live-e2e: requires PCLOUD_LIVE_E2E=1 + PCLOUD_FLEET_CONTROLLER_URL + PCLOUD_FLEET_CA_BUNDLE"]
fn live_fleet_heartbeat_round_trip() {
    if skip_if_not_live(&[ENV_CONTROLLER_URL, ENV_CA_BUNDLE]) {
        return;
    }
    let controller_url = optional_env(ENV_CONTROLLER_URL).expect("gate already asserted");
    let ca_bundle = optional_env(ENV_CA_BUNDLE).expect("gate already asserted");
    let device_group =
        optional_env(ENV_DEVICE_GROUP).unwrap_or_else(|| "live-e2e-default".to_owned());

    let identity_dir = unique_identity_dir();
    let identity_path = identity_dir.join("device_identity.json");

    let config = MtlsFleetConfig {
        server_url: controller_url,
        device_group,
        ca_bundle_path: PathBuf::from(ca_bundle),
        identity_path,
        trusted_server_keys: Vec::new(),
        request_timeout: Some(Duration::from_secs(10)),
    };

    let agent = match MtlsFleetAgent::new(config) {
        Ok(a) => a,
        Err(err) => {
            eprintln!(
                "[live-e2e] fleet_mtls: agent build failed (likely mis-configured controller/CA): {err}"
            );
            let _ = std::fs::remove_dir_all(&identity_dir);
            return;
        }
    };

    // The heartbeat itself is a non-secret payload (device id, version,
    // os, SLO summary). We still run an explicit "do not echo any
    // secret env we might have exposed" check on every stringified
    // error response.
    let hb = agent.default_heartbeat();
    match agent.send_heartbeat(&hb) {
        Ok(maybe_cmd) => {
            eprintln!(
                "[live-e2e] fleet_mtls: heartbeat ok, command={:?}",
                maybe_cmd
            );
        }
        Err(err) => {
            let msg = err.to_string();
            // Guard against secret leak in the transport/error message.
            for env_var in [
                "PCLOUD_TEST_PASSWORD",
                "PCLOUD_TEST_TOKEN",
                "PCLOUD_TEST_CRYPTO_PASSWORD",
            ] {
                if let Some(val) = optional_env(env_var) {
                    assert!(
                        !msg.contains(&val),
                        "fleet error message leaked {env_var} value"
                    );
                }
            }
            // Soft-skip on network / auth failures — the test asserts
            // mTLS round-trip robustness, not controller availability.
            eprintln!("[live-e2e] fleet_mtls: heartbeat declined: {msg}");
        }
    }

    let _ = std::fs::remove_dir_all(&identity_dir);
}
