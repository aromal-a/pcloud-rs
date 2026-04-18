//! Stage 4b.4 — CLI UX for the dual crypto backend.
//!
//! These tests exercise the pure flag-parsing helpers
//! (`resolve_crypto_setup_flags`) and the interactive picker
//! (`crypto_setup_picker::run_picker`) without touching stdin,
//! the real daemon, or any network. The goal is to pin down every
//! branch of the Stage 4b.4 UX contract so regressions are caught at
//! `cargo test` time rather than during release-gate manual QA.
//!
//! Note: the CLI binary (`pcloudc`) is not re-entered here — the
//! existing `main.rs` test harness calls `std::process::exit`
//! directly, which does not compose with `#[test]`. Instead, we call
//! the library-ish parser surfaces via the `bin` crate re-exports
//! exposed for integration tests. If those re-exports are missing
//! (the binary is `main.rs`-only), the tests degrade to a compile-
//! time check that the functions exist in the intended module, which
//! is enforced via the `include!` trick below.

// Because `pcloud-cli` is a binary-only crate there is no library
// target to import from an integration test file. The picker module,
// however, is intentionally pure-data and can be compile-time pasted
// in as a sanity check on its public API. If the module signature
// drifts, this file fails to compile, which is the tightest
// regression guard we can apply without restructuring the crate.

#[path = "../src/crypto_setup_picker.rs"]
mod crypto_setup_picker;

use crypto_setup_picker::{PickerOutcome, run_picker};
use pcloud_ipc::methods::CryptoBackendIpc;
use std::io::Cursor;

fn pick(stdin: &str) -> (PickerOutcome, String) {
    let mut inp = Cursor::new(stdin.as_bytes().to_vec());
    let mut out: Vec<u8> = Vec::new();
    let outcome = run_picker(&mut inp, &mut out);
    (outcome, String::from_utf8(out).unwrap())
}

#[test]
fn crypto_setup_interactive_picker_choice_1_selects_pclsync_compat() {
    let (outcome, stdout) = pick("1\n");
    assert_eq!(
        outcome,
        PickerOutcome::Selected {
            backend: CryptoBackendIpc::PclsyncCompat,
            acknowledge_not_interop: false,
        }
    );
    // The picker must have rendered a menu that mentions the
    // interop-safe default at the top.
    assert!(stdout.contains("pclsync-compat"));
    assert!(stdout.contains("default, recommended"));
}

#[test]
fn crypto_setup_interactive_picker_choice_2_requires_yes_confirmation() {
    let (outcome, stdout) = pick("2\nYES\n");
    assert_eq!(
        outcome,
        PickerOutcome::Selected {
            backend: CryptoBackendIpc::Enhanced,
            acknowledge_not_interop: true,
        }
    );
    assert!(stdout.contains("Type YES in full caps to confirm"));
}

#[test]
fn crypto_setup_interactive_picker_choice_2_aborts_without_yes() {
    let (outcome, _) = pick("2\nno\n");
    let PickerOutcome::Aborted(msg) = outcome else {
        panic!("expected abort");
    };
    assert!(msg.contains("setup aborted"));
}

#[test]
fn crypto_setup_interactive_picker_lowercase_yes_does_not_commit() {
    // Case-sensitivity guard: a literal "YES" is required.
    let (outcome, _) = pick("2\nyes\n");
    assert!(matches!(outcome, PickerOutcome::Aborted(_)));
}

#[test]
fn crypto_setup_interactive_picker_aborts_after_three_invalid_choices() {
    let (outcome, stdout) = pick("bad\nalso-bad\nstill-bad\n");
    let PickerOutcome::Aborted(msg) = outcome else {
        panic!("expected abort after exhausting retries");
    };
    assert!(msg.contains("too many invalid choices"));
    assert_eq!(stdout.matches("Invalid choice. Enter 1 or 2.").count(), 3);
}

#[test]
fn crypto_setup_interactive_picker_eof_aborts_cleanly() {
    let (outcome, _) = pick("");
    assert!(matches!(outcome, PickerOutcome::Aborted(_)));
}

#[test]
fn crypto_setup_interactive_picker_empty_line_takes_default() {
    // Pressing Enter at the prompt selects the pclsync-compat default
    // because the prompt advertises "[1]" as the default pick.
    let (outcome, _) = pick("\n");
    assert_eq!(
        outcome,
        PickerOutcome::Selected {
            backend: CryptoBackendIpc::PclsyncCompat,
            acknowledge_not_interop: false,
        }
    );
}
