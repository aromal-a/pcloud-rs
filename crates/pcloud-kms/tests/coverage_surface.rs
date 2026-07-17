use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use pcloud_kms::{
    KeyId, KmsError, KmsProvider, NullKms, PlaintextDek, WrappedDek, evict_cached_dek,
};

struct EchoKms {
    decryptions: AtomicUsize,
}

impl KmsProvider for EchoKms {
    fn name(&self) -> &'static str {
        "coverage-echo"
    }

    fn encrypt_dek(
        &self,
        _key_id: &KeyId,
        dek: &PlaintextDek,
        _context: Option<&str>,
    ) -> Result<WrappedDek, KmsError> {
        Ok(WrappedDek(dek.expose().to_vec()))
    }

    fn decrypt_dek(
        &self,
        _key_id: &KeyId,
        wrapped: &WrappedDek,
        _context: Option<&str>,
    ) -> Result<PlaintextDek, KmsError> {
        self.decryptions.fetch_add(1, Ordering::Relaxed);
        Ok(PlaintextDek(wrapped.0.clone()))
    }

    fn health_check(&self) -> Result<(), KmsError> {
        Ok(())
    }
}

#[test]
fn public_value_types_null_provider_and_cache_are_observable() {
    let key = KeyId("coverage-key".to_owned());
    let plaintext = PlaintextDek(b"sensitive coverage bytes".to_vec());
    let wrapped = WrappedDek(plaintext.expose().to_vec());

    assert_eq!(key.to_string(), "coverage-key");
    assert_eq!(plaintext.clone_secret().expose(), plaintext.expose());
    let debug = format!("{plaintext:?}");
    assert!(debug.contains("<redacted>"));
    assert!(!debug.contains("sensitive coverage bytes"));

    let null = NullKms;
    assert_eq!(null.name(), "null");
    assert!(null.health_check().is_ok());
    assert!(matches!(
        null.encrypt_dek(&key, &plaintext, Some("folder")),
        Err(KmsError::NotImplemented("null"))
    ));
    assert!(matches!(
        null.decrypt_dek(&key, &wrapped, Some("folder")),
        Err(KmsError::NotImplemented("null"))
    ));
    assert!(matches!(
        null.unwrap_cached(&key, &wrapped, Some("folder"), Duration::from_secs(1)),
        Err(KmsError::NotImplemented("null"))
    ));

    let provider = EchoKms {
        decryptions: AtomicUsize::new(0),
    };
    assert_eq!(
        provider
            .encrypt_dek(&key, &plaintext, Some("folder"))
            .unwrap(),
        wrapped
    );
    assert!(provider.health_check().is_ok());
    assert_eq!(
        provider
            .unwrap_cached(&key, &wrapped, Some("folder"), Duration::from_secs(60))
            .unwrap()
            .expose(),
        plaintext.expose()
    );
    assert_eq!(
        provider
            .unwrap_cached(&key, &wrapped, Some("folder"), Duration::from_secs(60))
            .unwrap()
            .expose(),
        plaintext.expose()
    );
    assert_eq!(provider.decryptions.load(Ordering::Relaxed), 1);
    assert!(evict_cached_dek(
        "coverage-echo",
        &key,
        &wrapped,
        Some("folder")
    ));
    assert!(!evict_cached_dek(
        "coverage-echo",
        &key,
        &wrapped,
        Some("folder")
    ));
}

#[cfg(not(feature = "pkcs11"))]
#[test]
fn disabled_pkcs11_constructors_fail_loudly() {
    use pcloud_kms::Pkcs11Hsm;
    use pcloud_secret::secret_string::SecretString;

    assert!(matches!(
        Pkcs11Hsm::new(KeyId("slot=1;label=coverage".into())),
        Err(KmsError::NotImplemented(_))
    ));
    assert!(matches!(
        Pkcs11Hsm::new_from_module(
            "/does/not/exist.so",
            1,
            SecretString::new("1234".to_owned()),
            "coverage",
        ),
        Err(KmsError::NotImplemented(_))
    ));
}
