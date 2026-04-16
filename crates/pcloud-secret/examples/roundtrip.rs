#![allow(clippy::pedantic)]
//! Demonstrates a `SecretString` round-trip: construct, expose, audit-visible
//! duplicate, constant-time compare, and observe redacted `Debug` output.
//!
//! The actual zeroize-on-drop step cannot be directly observed from safe Rust
//! (the backing buffer is scrubbed after `Drop` releases ownership of it),
//! but the wrapper derives `ZeroizeOnDrop` and the redacted `Debug` impl is
//! proof that the inner bytes never flow through a formatter.
//!
//! Run with: `cargo run -p pcloud-secret --example roundtrip`

// **PLATFORM:** all
// **GATING:** none (portable).

use pcloud_secret::secret_string::SecretString;
use pcloud_secret::{ExposeSecret, SecretMaterial};

fn main() {
    // Construct a secret from an owned String. The input String is moved into
    // the wrapper and scrubbed on Drop.
    let token = SecretString::new(String::from("hunter2-auth-token"));

    // Debug output is redacted: the inner bytes never hit stdout.
    println!("debug:   {token:?}");
    println!("length:  {}", token.expose_len());

    // expose_secret returns a borrowed view; do not store it in a long-lived
    // String or Vec<u8>.
    let exposed_prefix: String = token.expose_secret().chars().take(4).collect();
    println!("prefix:  {exposed_prefix}****");

    // Cloning is deliberately audit-visible: no `Clone` derive, only the
    // explicit `clone_secret` method.
    let dup = token.clone_secret();

    // PartialEq is constant-time via subtle::ConstantTimeEq.
    assert_eq!(token, dup, "duplicated secret must compare equal");

    // A different secret must not compare equal.
    let other = SecretString::new("different");
    assert_ne!(token, other);

    // Dropping here scrubs both backing buffers.
    drop(token);
    drop(dup);
    drop(other);

    println!("ok: round-trip + zeroize-on-drop complete");
}
