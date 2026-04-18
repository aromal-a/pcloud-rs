#![allow(clippy::pedantic)]
//! Live stub: `change_crypto_pass` end-to-end (crypto password rotation).
//!
//! Verifies the full `CryptoShell::change_password` chain:
//!   SendCryptoChangeUserPrivate → (out-of-band email confirmation) →
//!   CryptoChangePassword → verify decrypt still works with new pass.
//!
//! **Status:** stub — body is `todo!()` because the confirmation-code
//! delivery channel (email) is not programmatically addressable from a
//! test harness.  The test is provided so the gate exists in CI and can be
//! promoted once an OTP-injection mechanism is available.
//!
//! Tracking: bd-1du.10 / pcloud-rs-s1p.57 / audit-04 P2-8.
//!
//! Gate: `PCLOUD_LIVE_E2E=1` + valid credentials + `PCLOUD_TEST_CRYPTO_PASSWORD`.

#![forbid(unsafe_code)]

// **PLATFORM:** all
// **GATING:** none (portable at build time; runtime-gated).

mod common;

use crate::common::{ENV_CRYPTO_PASSWORD, skip_if_not_live};

/// Live end-to-end: rotate the crypto password and verify the vault
/// remains accessible with the new password.
///
/// TODO(bd-1du.10): Implement once an OTP-injection mechanism is available
/// (e.g. a mock SMTP server whose inbox the test harness can poll, or a
/// pre-shared OTP fixture on a CI-only account).  Until then the body is
/// a compile-time placeholder only.
#[test]
#[ignore = "live-e2e: requires PCLOUD_LIVE_E2E=1 + PCLOUD_TEST_CRYPTO_PASSWORD; body is todo!() — email-OTP not automatable"]
fn live_change_crypto_pass() {
    if skip_if_not_live(&[ENV_CRYPTO_PASSWORD]) {
        return;
    }

    // TODO(bd-1du.10): Replace todo!() with real test body:
    //   1. Authenticate + unlock crypto with ENV_CRYPTO_PASSWORD.
    //   2. Dispatch `Request::ChangeCryptoPassword { new_password }`.
    //   3. Assert ResponseStatus::Ok.
    //   4. Lock crypto, then unlock with new_password.
    //   5. Assert a previously-created encrypted file is still readable.
    //   6. Rotate back to ENV_CRYPTO_PASSWORD so the account is clean.
    todo!("email-OTP channel not automatable — see bd-1du.10 and pcloud-rs-s1p.57")
}
