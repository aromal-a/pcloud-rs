#![allow(clippy::pedantic)]
//! Property tests for `SecretBytes` / `SecretString` zeroize invariants.
//!
//! These tests exercise the PUBLIC surface only and do not modify production
//! code. They complement the unit-level redaction/zeroize tests with randomized
//! coverage of lengths, contents, and constant-time equality.

// **PLATFORM:** all
// **GATING:** none (portable).

use pcloud_secret::secret_bytes::SecretBytes;
use pcloud_secret::secret_string::SecretString;
use pcloud_secret::{ExposeSecret, SecretMaterial};
use proptest::prelude::*;
use zeroize::Zeroize;

proptest! {
    /// Round-trip invariants: new-then-expose matches the input, and `Debug`
    /// never leaks the plaintext content.
    #[test]
    fn prop_secret_bytes_new_exposes_input(bytes in prop::collection::vec(any::<u8>(), 0..4096)) {
        let secret = SecretBytes::new(bytes.clone());
        prop_assert_eq!(secret.expose_secret(), bytes.as_slice());
        prop_assert_eq!(secret.expose_len(), bytes.len());
        let debug = format!("{secret:?}");
        prop_assert!(debug.contains("<redacted>"));
        // The formatted debug output must never contain the raw byte values
        // (restricted to non-empty non-trivial contents to avoid false hits).
        if bytes.len() > 4 {
            let hex: String = bytes.iter().take(8).map(|b| format!("{b:02x}")).collect();
            prop_assert!(!debug.to_lowercase().contains(&hex));
        }
    }

    #[test]
    fn prop_secret_string_new_exposes_input(s in ".{0,2048}") {
        let secret = SecretString::new(s.clone());
        prop_assert_eq!(secret.expose_secret(), s.as_str());
        prop_assert_eq!(secret.expose_len(), s.len());
        let debug = format!("{secret:?}");
        prop_assert!(debug.contains("<redacted>"));
        if !s.is_empty() && s.is_ascii() && s.len() > 4 {
            prop_assert!(!debug.contains(&s));
        }
    }

    /// Constant-time equality must equal structural equality.
    #[test]
    fn prop_secret_bytes_ct_eq_matches_bytes_eq(
        lhs in prop::collection::vec(any::<u8>(), 0..256),
        rhs in prop::collection::vec(any::<u8>(), 0..256),
    ) {
        let a = SecretBytes::new(lhs.clone());
        let b = SecretBytes::new(rhs.clone());
        prop_assert_eq!(a == b, lhs == rhs);
    }

    #[test]
    fn prop_secret_string_ct_eq_matches_string_eq(
        lhs in ".{0,128}",
        rhs in ".{0,128}",
    ) {
        let a = SecretString::new(lhs.clone());
        let b = SecretString::new(rhs.clone());
        prop_assert_eq!(a == b, lhs == rhs);
    }

    /// Explicit `zeroize()` empties the exposed buffer even before drop.
    /// Note: we can't observe post-drop memory in safe Rust, but we CAN
    /// verify that `Zeroize::zeroize` leaves the structure in a scrubbed
    /// state matching the `ZeroizeOnDrop` contract it derives from.
    #[test]
    fn prop_secret_bytes_zeroize_empties_exposed(bytes in prop::collection::vec(any::<u8>(), 1..256)) {
        let mut secret = SecretBytes::new(bytes);
        secret.zeroize();
        let exposed = secret.expose_secret();
        // zeroize on Vec<u8> truncates to len 0.
        prop_assert_eq!(exposed.len(), 0);
    }

    #[test]
    fn prop_secret_string_zeroize_empties_exposed(s in ".{1,128}") {
        let mut secret = SecretString::new(s);
        secret.zeroize();
        let exposed = secret.expose_secret();
        prop_assert_eq!(exposed.len(), 0);
    }

    /// `clone_secret` produces an equal but independently-owned secret.
    #[test]
    fn prop_secret_bytes_clone_secret_is_equal(bytes in prop::collection::vec(any::<u8>(), 0..256)) {
        let a = SecretBytes::new(bytes.clone());
        let b = a.clone_secret();
        prop_assert!(a == b);
        prop_assert_eq!(b.expose_secret(), bytes.as_slice());
    }

    #[test]
    fn prop_secret_string_clone_secret_is_equal(s in ".{0,128}") {
        let a = SecretString::new(s.clone());
        let b = a.clone_secret();
        prop_assert!(a == b);
        prop_assert_eq!(b.expose_secret(), s.as_str());
    }
}
