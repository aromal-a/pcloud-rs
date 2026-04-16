//! Unit tests for `account_backend`: exercises the typed happy-path and
//! error-classification behavior without live network. Only compiled
//! under `#[cfg(test)]`.

// **PLATFORM:** all
// **GATING:** none (portable).

#[cfg(test)]
mod tests {
    use pcloud_config::{ConfigProfile, Environment};
    use pcloud_secret::secret_string::SecretString;

    use crate::account_backend::AccountRuntime;

    #[test]
    fn development_account_runtime_lists_api_servers() {
        let root = std::env::temp_dir().join(format!(
            "pcloud-account-runtime-test-{}",
            std::process::id()
        ));
        let config = ConfigProfile::secure_defaults(root, Environment::Development);
        let runtime = AccountRuntime::from_config(&config);

        let servers = runtime
            .get_api_servers()
            .expect("development locations should parse");

        assert_eq!(servers.len(), 2);
        assert_eq!(servers[0].label, "Europe");
    }

    #[test]
    fn development_account_runtime_gets_promo() {
        let root = std::env::temp_dir().join(format!(
            "pcloud-account-runtime-promo-{}",
            std::process::id()
        ));
        let config = ConfigProfile::secure_defaults(root, Environment::Development);
        let runtime = AccountRuntime::from_config(&config);

        let promo = runtime
            .get_promo(SecretString::new("token"))
            .expect("promo call should succeed")
            .expect("promo should exist");

        assert_eq!(promo.width, 640);
        assert_eq!(promo.height, 480);
    }

    #[test]
    fn development_account_runtime_rejects_invalid_language() {
        let root = std::env::temp_dir().join(format!(
            "pcloud-account-runtime-language-{}",
            std::process::id()
        ));
        let config = ConfigProfile::secure_defaults(root, Environment::Development);
        let runtime = AccountRuntime::from_config(&config);

        let err = runtime
            .set_language(SecretString::new("token"), "zzz")
            .expect_err("invalid language should fail");

        assert!(err.to_string().contains("2000"));
    }

    #[test]
    fn development_account_runtime_supports_verification_and_password_utilities() {
        let root = std::env::temp_dir().join(format!(
            "pcloud-account-runtime-auth-utils-{}",
            std::process::id()
        ));
        let config = ConfigProfile::secure_defaults(root, Environment::Development);
        let runtime = AccountRuntime::from_config(&config);

        runtime
            .verify_email(SecretString::new("token"))
            .expect("verify email should succeed");
        runtime
            .verify_email_restricted("verify-token")
            .expect("restricted verify email should succeed");
        runtime
            .lost_password("alice@example.com")
            .expect("lost password should succeed");

        let changed = runtime
            .change_password(
                SecretString::new("token"),
                "old-pass",
                "new-pass",
                "Desktop",
            )
            .expect("change password should succeed");

        assert_eq!(changed.auth_token, "rotated-auth-token");
    }
}
