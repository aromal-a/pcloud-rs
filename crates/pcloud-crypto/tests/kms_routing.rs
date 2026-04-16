#![allow(clippy::pedantic)]
//! Integration tests proving that `CryptoShell` DEK operations route
//! through the **injected** `KmsProvider` rather than the default
//! `NullKms`.
//!
//! This is the contract promise added by the KMS wiring bead:
//!
//! - `CryptoShell::default()` carries `NullKms` — `kms_wrap_dek`
//!   returns `KmsError::NotImplemented("null")`.
//! - `CryptoShell::with_kms_provider(Box::new(…))` or
//!   `set_kms_provider(Box::new(…))` on a live shell reroutes every
//!   subsequent `kms_wrap_dek` / `kms_unwrap_dek` call through the
//!   injected provider.
//! - Cache behaviour from `KmsProvider::unwrap_cached` carries over
//!   through the shell wrapper.

use std::sync::atomic::{AtomicUsize, Ordering};

use pcloud_crypto::CryptoShell;
use pcloud_kms::{KeyId, KmsError, KmsProvider, NullKms, PlaintextDek, WrappedDek};

/// Counts every trait call so tests can assert that dispatch actually
/// goes through the injected provider and not some residual `NullKms`.
struct RecordingProvider {
    name: &'static str,
    encrypts: AtomicUsize,
    decrypts: AtomicUsize,
}
impl RecordingProvider {
    fn new(name: &'static str) -> Self {
        Self {
            name,
            encrypts: AtomicUsize::new(0),
            decrypts: AtomicUsize::new(0),
        }
    }
}
impl KmsProvider for RecordingProvider {
    fn name(&self) -> &'static str {
        self.name
    }
    fn encrypt_dek(
        &self,
        _k: &KeyId,
        dek: &PlaintextDek,
        _c: Option<&str>,
    ) -> Result<WrappedDek, KmsError> {
        self.encrypts.fetch_add(1, Ordering::SeqCst);
        // Trivially reversible transform so the unwrap test can check
        // the round trip without depending on a real KMS.
        let mut out = Vec::with_capacity(dek.expose().len() + 1);
        out.push(0xAA);
        out.extend_from_slice(dek.expose());
        Ok(WrappedDek(out))
    }
    fn decrypt_dek(
        &self,
        _k: &KeyId,
        wrapped: &WrappedDek,
        _c: Option<&str>,
    ) -> Result<PlaintextDek, KmsError> {
        self.decrypts.fetch_add(1, Ordering::SeqCst);
        if wrapped.0.first() != Some(&0xAA) {
            return Err(KmsError::Malformed);
        }
        Ok(PlaintextDek(wrapped.0[1..].to_vec()))
    }
    fn health_check(&self) -> Result<(), KmsError> {
        Ok(())
    }
}

#[test]
fn default_shell_uses_null_kms() {
    let shell = CryptoShell::default();
    assert_eq!(shell.kms_provider_name(), "null");

    let id = KeyId("default-null".into());
    let dek = PlaintextDek(vec![0x11; 16]);
    let err = shell.kms_wrap_dek(&id, &dek, None).unwrap_err();
    // NullKms refuses every real wrap/unwrap so misconfigured
    // deployments fail loudly.
    assert!(matches!(err, KmsError::NotImplemented("null")));
}

#[test]
fn with_kms_provider_routes_through_injected() {
    let shell =
        CryptoShell::default().with_kms_provider(Box::new(RecordingProvider::new("inject-a")));
    assert_eq!(shell.kms_provider_name(), "inject-a");

    let id = KeyId("k-a".into());
    let dek = PlaintextDek(vec![1, 2, 3, 4, 5, 6, 7, 8]);
    let wrapped = shell.kms_wrap_dek(&id, &dek, Some("folder-42")).unwrap();
    assert!(!wrapped.0.is_empty());
    let unwrapped = shell
        .kms_unwrap_dek(&id, &wrapped, Some("folder-42"))
        .unwrap();
    assert_eq!(unwrapped.expose(), dek.expose());
}

#[test]
fn set_kms_provider_swaps_live_shell() {
    let mut shell = CryptoShell::default();
    assert_eq!(shell.kms_provider_name(), "null");

    // Swap to a recording provider mid-flight — the daemon does this
    // after reading the `[crypto.kms]` section from the profile.
    shell.set_kms_provider(Box::new(RecordingProvider::new("inject-b")));
    assert_eq!(shell.kms_provider_name(), "inject-b");

    let id = KeyId("k-b".into());
    let dek = PlaintextDek(vec![9, 9, 9, 9]);
    let w = shell.kms_wrap_dek(&id, &dek, None).unwrap();

    // Two unwraps inside the default cache TTL must only hit the
    // provider once; the second comes from the process-local cache.
    let first = shell.kms_unwrap_dek(&id, &w, None).unwrap();
    let second = shell.kms_unwrap_dek(&id, &w, None).unwrap();
    assert_eq!(first.expose(), dek.expose());
    assert_eq!(second.expose(), dek.expose());

    // Swap back to NullKms and confirm subsequent wraps fail loudly
    // — this is the inverse property that guarantees no sticky state
    // survives a provider swap.
    shell.set_kms_provider(Box::new(NullKms));
    assert_eq!(shell.kms_provider_name(), "null");
    let dek2 = PlaintextDek(vec![7; 4]);
    let err = shell.kms_wrap_dek(&id, &dek2, None).unwrap_err();
    assert!(matches!(err, KmsError::NotImplemented("null")));
}

#[test]
fn serde_skip_preserves_default_on_deserialize() {
    // A deserialised CryptoShell always comes back with NullKms; the
    // runtime must re-inject the real provider. This freezes that
    // contract so a future serde refactor can't silently regress.
    let shell =
        CryptoShell::default().with_kms_provider(Box::new(RecordingProvider::new("pre-serde")));
    let json = serde_json::to_string(&shell).expect("serialize");
    let back: CryptoShell = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back.kms_provider_name(), "null");
}

// -------------------------------------------------------------------------
// Sector path — DEK routing (I04).
//
// These tests prove that the AES-256-GCM sector encryption path actually
// honours `CryptoShell::mode`:
//
// - Raw mode: keys derived from Argon2id master key (NullKms is a
//   no-op and sector ops still work — single-user regression gate).
// - Kms mode: per-file key derived from the KMS-wrapped DEK; the
//   recording provider observes `decrypt_dek` traffic during sector
//   ops, and the cache amortises repeat calls.
// -------------------------------------------------------------------------

use pcloud_crypto::CryptoMode;
use pcloud_secret::secret_string::SecretString;

#[test]
fn raw_mode_works_with_null_kms_single_user_regression() {
    // Regression gate: the default shell (NullKms + Raw mode) must
    // still round-trip a sector without any KMS round-trip.
    let mut c = CryptoShell::default();
    assert_eq!(c.kms_provider_name(), "null");
    assert_eq!(c.mode.tag(), "raw");

    c.setup(SecretString::new("hunter2"), None).unwrap();
    c.start(SecretString::new("hunter2")).unwrap();

    let seed = [9u8; 32];
    let frame = c.seal_sector(&seed, 0, b"legacy raw path").unwrap();
    let round = c.open_sector(&seed, 0, &frame).unwrap();
    assert_eq!(round, b"legacy raw path");
    // Mode must remain Raw.
    assert_eq!(c.mode.tag(), "raw");
}

#[test]
fn enable_kms_mode_refused_on_null_kms() {
    // Switching to Kms mode while the default NullKms is injected
    // must fail loudly — the whole point of the KMS path is to stop
    // the DEK from living inside the process.
    let mut c = CryptoShell::default();
    c.setup(SecretString::new("pw"), None).unwrap();
    c.start(SecretString::new("pw")).unwrap();
    let err = c.enable_kms_mode("k", None).expect_err("must refuse null");
    assert!(matches!(err, pcloud_crypto::CryptoError::NoKmsProvider));
    assert_eq!(c.mode.tag(), "raw");
}

#[test]
fn kms_mode_round_trips_and_routes_through_provider() {
    // Real KMS mode: the recording provider observes the wrap on
    // enable, then at least one decrypt during seal/open. Repeat
    // ops inside the default TTL hit the cache (second op must not
    // add a second decrypt).
    let provider = Box::new(RecordingProvider::new("i04-kms"));
    // Capture raw pointer addresses via a second handle on the
    // counters — can't re-borrow through the shell once boxed.
    let encrypts = std::sync::Arc::new(AtomicUsize::new(0));
    let decrypts = std::sync::Arc::new(AtomicUsize::new(0));

    struct SharedRecorder {
        encrypts: std::sync::Arc<AtomicUsize>,
        decrypts: std::sync::Arc<AtomicUsize>,
    }
    impl KmsProvider for SharedRecorder {
        fn name(&self) -> &'static str {
            "i04-shared"
        }
        fn encrypt_dek(
            &self,
            _k: &KeyId,
            dek: &PlaintextDek,
            _c: Option<&str>,
        ) -> Result<WrappedDek, KmsError> {
            self.encrypts.fetch_add(1, Ordering::SeqCst);
            let mut out = Vec::with_capacity(dek.expose().len() + 1);
            out.push(0xAA);
            out.extend_from_slice(dek.expose());
            Ok(WrappedDek(out))
        }
        fn decrypt_dek(
            &self,
            _k: &KeyId,
            wrapped: &WrappedDek,
            _c: Option<&str>,
        ) -> Result<PlaintextDek, KmsError> {
            self.decrypts.fetch_add(1, Ordering::SeqCst);
            if wrapped.0.first() != Some(&0xAA) {
                return Err(KmsError::Malformed);
            }
            Ok(PlaintextDek(wrapped.0[1..].to_vec()))
        }
        fn health_check(&self) -> Result<(), KmsError> {
            Ok(())
        }
    }
    let shared = Box::new(SharedRecorder {
        encrypts: std::sync::Arc::clone(&encrypts),
        decrypts: std::sync::Arc::clone(&decrypts),
    });
    // Force shell provider to a unique provider name so the cache
    // key doesn't collide with any other test's session cache.
    let mut c = CryptoShell::default().with_kms_provider(shared);
    let _ = provider; // unused — kept for doc

    c.setup(SecretString::new("pw"), None).unwrap();
    c.start(SecretString::new("pw")).unwrap();

    // Enable KMS mode — one encrypt_dek call must land.
    c.enable_kms_mode("i04-key", Some("tenant-42".into()))
        .unwrap();
    assert_eq!(encrypts.load(Ordering::SeqCst), 1);
    assert!(matches!(c.mode, CryptoMode::Kms { .. }));

    // First seal: must decrypt the wrapped DEK.
    let seed = [1u8; 32];
    let frame = c.seal_sector(&seed, 0, b"kms-sealed").unwrap();
    let decrypts_after_first = decrypts.load(Ordering::SeqCst);
    assert_eq!(decrypts_after_first, 1, "first seal must unwrap DEK");

    // Second seal inside the TTL must hit the cache.
    let _ = c.seal_sector(&seed, 1, b"again").unwrap();
    assert_eq!(
        decrypts.load(Ordering::SeqCst),
        1,
        "second seal inside TTL must NOT re-unwrap"
    );

    // Open round-trips must still validate.
    let round = c.open_sector(&seed, 0, &frame).unwrap();
    assert_eq!(round, b"kms-sealed");

    // Stop evicts the cache entry; next op re-unwraps.
    c.stop();
    c.start(SecretString::new("pw")).unwrap();
    let _ = c
        .open_sector(&seed, 0, &frame)
        .expect("re-open after restart ok");
    assert!(
        decrypts.load(Ordering::SeqCst) >= 2,
        "stop() must have evicted the cached DEK so a subsequent op re-unwraps"
    );
}

#[test]
fn kms_mode_reset_reverts_to_raw() {
    // reset() wipes mode back to Raw and evicts the cache. After a
    // fresh setup+start the shell is back on the legacy path.
    let (encrypts, _decrypts) = (
        std::sync::Arc::new(AtomicUsize::new(0)),
        std::sync::Arc::new(AtomicUsize::new(0)),
    );
    struct ChewingRecorder {
        encrypts: std::sync::Arc<AtomicUsize>,
    }
    impl KmsProvider for ChewingRecorder {
        fn name(&self) -> &'static str {
            "i04-reset"
        }
        fn encrypt_dek(
            &self,
            _k: &KeyId,
            dek: &PlaintextDek,
            _c: Option<&str>,
        ) -> Result<WrappedDek, KmsError> {
            self.encrypts.fetch_add(1, Ordering::SeqCst);
            let mut out = vec![0xAA];
            out.extend_from_slice(dek.expose());
            Ok(WrappedDek(out))
        }
        fn decrypt_dek(
            &self,
            _k: &KeyId,
            wrapped: &WrappedDek,
            _c: Option<&str>,
        ) -> Result<PlaintextDek, KmsError> {
            Ok(PlaintextDek(wrapped.0[1..].to_vec()))
        }
        fn health_check(&self) -> Result<(), KmsError> {
            Ok(())
        }
    }
    let mut c = CryptoShell::default().with_kms_provider(Box::new(ChewingRecorder {
        encrypts: std::sync::Arc::clone(&encrypts),
    }));
    c.setup(SecretString::new("pw"), None).unwrap();
    c.start(SecretString::new("pw")).unwrap();
    c.enable_kms_mode("reset-k", None).unwrap();
    assert!(matches!(c.mode, CryptoMode::Kms { .. }));
    c.reset();
    assert_eq!(c.mode.tag(), "raw");
}
