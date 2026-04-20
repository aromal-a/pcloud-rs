//! Mock-backed two-account end-to-end proof for the RSA-4096-OAEP
//! share-invitation wire path (bead `pcloud-rs-ncx.89-e2e`).
//!
//! # Purpose
//!
//! `ncx.89` landed the full Rust-side plumbing for the C-interop
//! share-invitation flow:
//!
//! * [`pcloud_crypto::share_rsa::wrap_share_invitation_b64`] — wraps
//!   the sharer's cached `SymKeyVer1` under the recipient's RSA-4096
//!   pubkey with OAEP-SHA1 (matches `pssl.c:718-740`).
//! * [`pcloud_proto::SharesApi::crypto_share_folder_rsa`] /
//!   [`pcloud_proto::SharesApi::crypto_account_team_share_rsa`] —
//!   attach the base64 ciphertext under the `sharedfolderkey` /
//!   `teamshare_key` wire parameter.
//! * [`pcloud_backends::SharesRuntime::crypto_share_folder_rsa`] /
//!   [`pcloud_backends::SharesRuntime::crypto_account_team_share_rsa`]
//!   — look up the cached sym key on the unlocked `CryptoShell`, call
//!   into the wrap primitive, and forward to the proto layer.
//!
//! Live two-pCloud-account verification is operator work (provision
//! two test accounts, optionally a team), tracked separately. This
//! file closes the loop that IS in-scope for an automated test:
//! mechanically prove, end-to-end, that
//!
//!   1. the sharer-side code path (primitive → api) produces a blob
//!      that is byte-reversible by the recipient's RSA private key,
//!   2. the wire request carries the blob under the correct parameter
//!      name (`sharedfolderkey` for user-share, `teamshare_key` for
//!      team-share),
//!   3. unwrapping with a *wrong* RSA private key fails deterministically
//!      (no padding-oracle leak, no silent success).
//!
//! The mock transport implements the exact same [`ProtocolTransport`]
//! and [`ApiServerHintConsumer`] traits the production `BinaryApiTransport`
//! does — no type-laundering, no fake that bypasses the compiler.
//!
//! # Why `SharesApi` rather than `SharesRuntime`
//!
//! `SharesRuntime` wraps `SharesApi<SharesTransportMode>` where the
//! transport is a closed enum (`Development` | `Network`). Adding a
//! third `Mock` variant just to drive this test would bleed test
//! apparatus into production code. The two backend methods under test
//! (`SharesRuntime::crypto_share_folder_rsa` /
//! `crypto_account_team_share_rsa`) are thin three-line delegates:
//!
//! ```ignore
//! let state = crypto.pclsync_compat_state.as_ref().ok_or(Locked)?;
//! let wrapped = share_rsa::wrap_share_invitation_b64(state, target, pub_blob)?;
//! self.api.crypto_share_folder_rsa(..., wrapped)
//! ```
//!
//! This test drives those same three steps explicitly against the
//! production `SharesApi` + production `wrap_share_invitation_b64`,
//! which is mechanically equivalent to calling through `SharesRuntime`
//! while keeping the transport injection type-safe.
//!
//! # Runtime cost
//!
//! Each test generates two RSA-4096 keypairs (the negative test
//! generates three). On a modern x86_64 that's ~3-6 s per test — this
//! is inherent to RSA-4096 keygen and matches the crypto crate's own
//! test suite.

// No feature gate — pcloud-backends always pulls pcloud-crypto with
// the default `pclsync-v2` feature active (see ../Cargo.toml), which
// is what brings in `share_rsa` and the RSA primitives this test
// exercises. The dev-dependencies also enable `test-helpers` so the
// `PclsyncCompatState::for_test` + `SymKeyVer1::duplicate` harnesses
// are reachable here.

use std::sync::{Arc, Mutex};

use base64::Engine;
use pcloud_crypto::{
    pclsync_compat_profile::{PclsyncCompatProfile, PclsyncCompatState},
    pclsync_rsa::{self, SymKeyVer1, generate_keypair, oaep_unwrap, serialize_pub_key_der},
    share_rsa::{self, ShareTarget},
};
use pcloud_model::shares::SharePermissions;
use pcloud_proto::{
    BinaryParamValue, EncodedRequest, SharesApi,
    auth_api::{ApiServerHintConsumer, ProtocolTransport},
    response::Value,
};

// ============================================================================
// Mock transport
// ============================================================================

/// Mock [`ProtocolTransport`] that captures every encoded request and
/// returns a canned `sharefolder` / `account_teamshare` OK response
/// (`result = 0`, `sharerequestid = 777`).
///
/// The capture buffer is shared via `Arc<Mutex<_>>` so the test can
/// introspect the wire request after the API call returns — this is
/// how we prove the `sharedfolderkey` / `teamshare_key` parameter
/// carries the RSA-OAEP ciphertext the sharer produced.
#[derive(Debug, Default, Clone)]
struct MockTransport {
    captured: Arc<Mutex<Vec<EncodedRequest>>>,
}

impl MockTransport {
    fn requests(&self) -> Vec<EncodedRequest> {
        self.captured.lock().unwrap().clone()
    }
}

/// Error type for the mock — must satisfy the `Error + Send + Sync +
/// 'static` bound the [`ProtocolTransport`] trait imposes.
#[derive(Debug, thiserror::Error)]
#[error("mock transport error")]
struct MockTransportError;

impl ProtocolTransport for MockTransport {
    type Error = MockTransportError;

    fn execute(&self, request: &EncodedRequest) -> Result<Value, Self::Error> {
        self.captured.lock().unwrap().push(request.clone());
        // Canned success response — matches the shape both
        // `crypto_share_folder_rsa` and `crypto_account_team_share_rsa`
        // expect (`result = 0`, optional `sharerequestid`).
        Ok(Value::Hash(vec![
            ("result".to_string(), Value::Number(0)),
            ("sharerequestid".to_string(), Value::Number(777)),
        ]))
    }
}

impl ApiServerHintConsumer for MockTransport {
    fn apply_api_server_hint(&self, _api_server: &str) {
        // No-op; mock does not route.
    }
}

// ============================================================================
// Helpers
// ============================================================================

/// Build a deterministic synthetic `SymKeyVer1` so the recipient-side
/// unwrap can compare byte-for-byte against the original.
fn test_sym_key(aes_seed: u8, hmac_seed: u8) -> SymKeyVer1 {
    let mut sym = SymKeyVer1::new(0);
    for (i, b) in sym.aes_key.iter_mut().enumerate() {
        *b = aes_seed.wrapping_add(i as u8);
    }
    for (i, b) in sym.hmac_key.iter_mut().enumerate() {
        *b = hmac_seed.wrapping_add(i as u8);
    }
    sym
}

/// Build the on-wire `pub_key_ver1` blob exactly the way the pCloud
/// server returns it from `crypto_getpubkey` — 8-byte LE header
/// (`type=1`, `flags=0`) followed by PKCS#1 DER of the RSA-4096 pubkey.
fn pub_blob_for(pubkey: &rsa::RsaPublicKey) -> Vec<u8> {
    let der = serialize_pub_key_der(pubkey).expect("serialize pub DER");
    PclsyncCompatProfile::build_pub_blob(0, &der)
}

/// Extract a named string parameter from a captured [`EncodedRequest`].
/// Mirrors the `string_param` helper the backend uses internally so
/// the test reads the same way production code does.
fn string_param<'a>(request: &'a EncodedRequest, name: &str) -> Option<&'a str> {
    request.params.iter().find_map(|p| {
        if p.name == name {
            match &p.value {
                BinaryParamValue::String(s) => Some(s.as_str()),
                _ => None,
            }
        } else {
            None
        }
    })
}

fn number_param(request: &EncodedRequest, name: &str) -> Option<u64> {
    request.params.iter().find_map(|p| {
        if p.name == name {
            match &p.value {
                BinaryParamValue::Number(n) => Some(*n),
                _ => None,
            }
        } else {
            None
        }
    })
}

/// Base64-decode a captured `sharedfolderkey` / `teamshare_key` param
/// back to the 512-byte RSA-OAEP ciphertext.
fn decode_b64(s: &str) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .expect("base64 decode of captured wire param")
}

// ============================================================================
// Tests
// ============================================================================

/// E2E-1: folder-share — account A wraps a folder `SymKeyVer1` under
/// account B's pubkey, the wire request is built through the real
/// `SharesApi`, and account B recovers the original sym key bit-identical
/// by unwrapping the captured `sharedfolderkey` param with their private
/// key.
#[test]
fn folder_share_rsa_two_account_roundtrip() {
    // Account A (sharer) — generates a keypair purely so the shell
    // harness can be constructed; the wrap path never touches it.
    let a_kp = generate_keypair().expect("keygen sharer");
    let (a_priv, _a_pub) = a_kp.into_parts();

    // Account B (recipient) — the sym-key wrap targets this pubkey.
    let b_kp = generate_keypair().expect("keygen recipient");
    let (b_priv, b_pub) = b_kp.into_parts();

    // A caches a synthetic folder sym-key under folder id 42.
    let original = test_sym_key(0x11, 0x77);
    let mut a_state = PclsyncCompatState::for_test(a_priv);
    a_state.cache_folder_key(42, original.duplicate());

    // B publishes its pubkey as a `pub_key_ver1` blob (this is exactly
    // what `crypto_getpubkey(userid=B)` would return).
    let b_pub_blob = pub_blob_for(&b_pub);

    // --- Step 1 (exercises the exact primitive SharesRuntime calls). ---
    let wrapped_b64 = share_rsa::wrap_share_invitation_b64(
        &a_state,
        ShareTarget::Folder(42),
        &b_pub_blob,
    )
    .expect("wrap succeeds");

    // Wire shape sanity — the base64 must be 684 chars for RSA-4096.
    assert_eq!(wrapped_b64.len(), 684, "RSA-4096 OAEP b64 length contract");

    // --- Step 2: drive the real SharesApi against a mock transport. ---
    let mock = MockTransport::default();
    let api = SharesApi::new(mock.clone());
    let result = api
        .crypto_share_folder_rsa(
            "auth-token",
            42,
            "shared-folder-name",
            "b@example.com",
            "invitation message",
            SharePermissions::from_bits(3),
            Some("hint".into()),
            wrapped_b64.clone(),
        )
        .expect("api call returns ok from mock");
    assert_eq!(result.share_request_id, Some(777));

    // --- Step 3: inspect the captured wire request. ---
    let reqs = mock.requests();
    assert_eq!(reqs.len(), 1, "exactly one wire request emitted");
    let req = &reqs[0];
    assert_eq!(req.frame.command, "sharefolder");
    assert_eq!(number_param(req, "folderid"), Some(42));
    let sfk = string_param(req, "sharedfolderkey")
        .expect("sharedfolderkey parameter present on the wire");
    assert_eq!(
        sfk, wrapped_b64,
        "backend attaches the ciphertext verbatim under sharedfolderkey"
    );

    // --- Step 4: account B decrypts with their private key. ---
    let ct = decode_b64(sfk);
    assert_eq!(
        ct.len(),
        pclsync_rsa::PCLSYNC_RSA_BYTES,
        "RSA-4096 ciphertext length = modulus bytes"
    );
    let recovered = oaep_unwrap(&b_priv, &ct).expect("B unwraps with their priv key");

    // Byte equality on every field (aes_key + hmac_key + sym_type + flags).
    assert!(
        bool::from(recovered.ct_eq(&original)),
        "recovered SymKeyVer1 ≠ original — E2E round-trip broken"
    );
    assert_eq!(recovered.aes_key, original.aes_key);
    assert_eq!(recovered.hmac_key, original.hmac_key);
    assert_eq!(recovered.sym_type, original.sym_type);
    assert_eq!(recovered.flags, original.flags);
}

/// E2E-2: account-team-share — same flow but targets the team share
/// wire variant. Confirms the ciphertext is carried under the
/// `teamshare_key` parameter (not `sharedfolderkey`) and the command
/// name is `account_teamshare` (not `sharefolder`).
#[test]
fn account_team_share_rsa_two_account_roundtrip() {
    let a_kp = generate_keypair().expect("keygen sharer");
    let (a_priv, _) = a_kp.into_parts();

    // The "recipient" here stands in for the team's shared pubkey
    // (`crypto_getpubkey(teamid=T)` in production).
    let team_kp = generate_keypair().expect("keygen team");
    let (team_priv, team_pub) = team_kp.into_parts();

    let original = test_sym_key(0x55, 0xAA);
    let mut a_state = PclsyncCompatState::for_test(a_priv);
    a_state.cache_folder_key(7, original.duplicate());
    let team_pub_blob = pub_blob_for(&team_pub);

    let wrapped_b64 = share_rsa::wrap_share_invitation_b64(
        &a_state,
        ShareTarget::Folder(7),
        &team_pub_blob,
    )
    .expect("wrap team succeeds");

    let mock = MockTransport::default();
    let api = SharesApi::new(mock.clone());
    let result = api
        .crypto_account_team_share_rsa(
            "auth-token",
            7,
            "team-share-name",
            9, // team id
            "team invitation",
            SharePermissions::from_bits(27),
            Some("team-hint".into()),
            wrapped_b64.clone(),
        )
        .expect("team api call returns ok");
    assert_eq!(result.share_request_id, Some(777));

    let reqs = mock.requests();
    assert_eq!(reqs.len(), 1);
    let req = &reqs[0];
    assert_eq!(req.frame.command, "account_teamshare");
    assert_eq!(number_param(req, "folderid"), Some(7));
    assert_eq!(number_param(req, "teamid"), Some(9));
    // Critical: team-share uses `teamshare_key`, NOT `sharedfolderkey`.
    assert!(
        string_param(req, "sharedfolderkey").is_none(),
        "team-share path must not emit sharedfolderkey"
    );
    let tsk = string_param(req, "teamshare_key")
        .expect("teamshare_key parameter present on the wire");
    assert_eq!(tsk, wrapped_b64);

    // Team unwraps with their private key.
    let recovered =
        oaep_unwrap(&team_priv, &decode_b64(tsk)).expect("team unwraps with team priv");
    assert!(bool::from(recovered.ct_eq(&original)));
    assert_eq!(recovered.aes_key, original.aes_key);
    assert_eq!(recovered.hmac_key, original.hmac_key);
}

/// E2E-3 (negative): a wrong private key MUST NOT unwrap.
///
/// Account A wraps the sym key under account B's pubkey; account C
/// (a third, unrelated keypair) tries to unwrap with their own private
/// key. OAEP must fail — this is the property that makes the
/// share-invitation flow a capability (only the holder of the matching
/// priv key can recover the sym key). If this ever silently succeeds,
/// the whole invitation flow is broken.
#[test]
fn wrong_private_key_cannot_unwrap_share_invitation() {
    let a_kp = generate_keypair().expect("keygen sharer");
    let (a_priv, _) = a_kp.into_parts();
    let b_kp = generate_keypair().expect("keygen intended recipient");
    let (_b_priv, b_pub) = b_kp.into_parts();
    // Third party — has no legitimate relationship to this invitation.
    let c_kp = generate_keypair().expect("keygen attacker");
    let (c_priv, _c_pub) = c_kp.into_parts();

    let original = test_sym_key(0xCC, 0x33);
    let mut a_state = PclsyncCompatState::for_test(a_priv);
    a_state.cache_folder_key(100, original.duplicate());
    let b_pub_blob = pub_blob_for(&b_pub);

    let wrapped_b64 = share_rsa::wrap_share_invitation_b64(
        &a_state,
        ShareTarget::Folder(100),
        &b_pub_blob,
    )
    .expect("wrap against B's pubkey succeeds");

    // Drive through the API to match the exact wire path.
    let mock = MockTransport::default();
    let api = SharesApi::new(mock.clone());
    api.crypto_share_folder_rsa(
        "auth-token",
        100,
        "capability-test",
        "b@example.com",
        "",
        SharePermissions::from_bits(3),
        None,
        wrapped_b64,
    )
    .expect("api mock always succeeds");

    let reqs = mock.requests();
    let req = &reqs[0];
    let sfk = string_param(req, "sharedfolderkey").unwrap();
    let ct = decode_b64(sfk);

    // Account C tries to unwrap. MUST fail — OAEP is randomized and
    // tied to the modulus; a wrong priv key cannot produce a valid
    // `sym_key_ver1` plaintext.
    let err =
        oaep_unwrap(&c_priv, &ct).expect_err("wrong priv key must not unwrap invitation");
    // Error variant is deliberately opaque (no padding-oracle leak) —
    // the exact variant is either `Oaep` or `WrongSymKeyLen` depending
    // on whether the random-looking plaintext happens to pass the
    // OAEP decode. We accept any error and only reject silent success.
    drop(err); // explicit: we do not introspect the variant.
}
