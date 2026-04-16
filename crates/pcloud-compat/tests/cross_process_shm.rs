#![allow(clippy::pedantic)]
//! Cross-process shm integration test.
//!
//! Spawns the `shm_peek` helper binary (built from this crate) and checks
//! that a payload written via [`ShmSegment::write`] is visible in a
//! second process. Ignored by default because it needs SysV IPC support
//! and the feature-gated binary; run with:
//!
//! ```bash
//! cargo test -p pcloud-compat --features legacy-shm -- --ignored
//! ```

#![cfg(all(target_os = "linux", feature = "legacy-shm"))]

// **PLATFORM:** all
// **GATING:** none (portable).

use std::io::Write as _;
use std::process::Command;

use pcloud_compat::shm_producer::ShmSegment;

#[test]
#[ignore = "requires SysV IPC and shm_peek helper; run with --ignored"]
fn second_process_reads_shm_payload() {
    let dir = std::env::temp_dir().join(format!("pcloud-compat-xproc-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let anchor = dir.join("data.db");
    std::fs::File::create(&anchor)
        .unwrap()
        .write_all(b"anchor")
        .unwrap();

    let payload = b"cross-process-compat-payload";
    let mut seg = ShmSegment::create(&anchor, 0o600).unwrap();
    seg.write(payload).unwrap();

    // Locate the helper binary that Cargo built for us.
    let bin = env!("CARGO_BIN_EXE_pcloud-compat-shm-peek");
    let out = Command::new(bin).arg(&anchor).output().expect("spawn peek");
    assert!(out.status.success(), "peek failed: {:?}", out);
    assert_eq!(out.stdout, payload);

    seg.mark_for_removal().unwrap();
    drop(seg);
    let _ = std::fs::remove_dir_all(&dir);
}
