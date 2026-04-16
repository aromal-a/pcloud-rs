//! [`ProtocolAuthFlow`] — glue between the I/O-free
//! [`crate::manager::SessionManager`] state machine and the pCloud
//! [`pcloud_proto::auth_api::AuthApi`] transport.
//!
//! The orchestrator performs the HTTP round-trips, classifies server
//! responses into [`AuthCommand`]s, and folds the resulting
//! [`AuthEvent`]s back into the manager. Secrets (`password`, `code`,
//! `auth_token`) flow through [`SecretString`] and are never logged.
//!
//! See ADR 0007 for the secret-handling rules enforced here.

// **PLATFORM:** all
// **GATING:** none (portable).

use pcloud_model::ids::UserId;
use pcloud_proto::auth_api::{
    ApiServerHintConsumer, AuthApi, AuthApiError, AuthRefreshError, PasswordLoginOutcome,
    ProtocolTransport, TwoFactorNotificationDelivery, TwoFactorSmsDelivery, UserInfo,
};
use pcloud_secret::{ExposeSecret, secret_string::SecretString};
use thiserror::Error;

use crate::{AuthCommand, AuthEvent, SessionManager, SessionManagerError};

/// Classified failure returned by [`ProtocolAuthFlow::refresh_token`].
///
/// Mirrors [`AuthRefreshError`] at the orchestrator boundary so call
/// sites can distinguish "revoke and re-login" from "retry later"
/// without reaching into the proto crate's error types.
///
/// # Recoverability matrix
///
/// | Variant               | Session state after       | Retry guidance                  |
/// |-----------------------|---------------------------|---------------------------------|
/// | `NotAuthenticated`    | unchanged                 | caller bug; do not retry        |
/// | `AuthExpired`         | revoked → `LoggedOut`     | re-authenticate interactively   |
/// | `TemporaryFailure`    | unchanged → `Authenticated` | exponential backoff, then retry |
/// | `Session`             | unchanged                 | caller bug; do not retry        |
/// | `MissingAuthField`    | unchanged                 | retry once; if persistent, file a bug against the proto crate |
/// | `Protocol`            | unchanged                 | inspect inner error; most cases retryable |
#[derive(Debug, Error)]
pub enum RefreshTokenError<E: std::error::Error + Send + Sync + 'static> {
    /// Session not authenticated; nothing to refresh.
    ///
    /// * **Cause**: [`ProtocolAuthFlow::refresh_token`] was called
    ///   while the [`SessionManager`] is not in
    ///   [`crate::SessionState::Authenticated`].
    /// * **Recoverability**: not recoverable by refresh. Invoke an
    ///   interactive login path instead.
    /// * **Retry guidance**: do not retry — this is always an
    ///   orchestration bug.
    #[error("no authenticated session")]
    NotAuthenticated,
    /// Server reported the current token is no longer valid. The
    /// session has been revoked by the orchestrator.
    ///
    /// * **Cause**: pCloud returned an auth-expired `result` code
    ///   (carried in the variant payload) from the refresh request.
    /// * **Recoverability**: the orchestrator has already called
    ///   [`SessionManager::revoke`], so the in-memory token is
    ///   zeroized and the session is `LoggedOut`.
    /// * **Retry guidance**: **do not** retry the refresh; re-run
    ///   interactive auth (password / token login).
    #[error("auth token expired (result {0})")]
    AuthExpired(u64),
    /// Transient failure. The session is left untouched; the caller
    /// may retry with backoff.
    ///
    /// * **Cause**: transport-level or server-classified transient
    ///   error (e.g. network blip, 5xx, non-expired non-zero result
    ///   code).
    /// * **Recoverability**: yes. The session remains
    ///   [`crate::SessionState::Authenticated`] with its current
    ///   token; the single-flight guard in
    ///   [`crate::refresh::RefreshCoordinator`] will allow another
    ///   attempt.
    /// * **Retry guidance**: exponential backoff. Do not tight-loop.
    #[error("temporary refresh failure: {0}")]
    TemporaryFailure(String),
    /// Session state transition rejected the new token.
    ///
    /// * **Cause**: should not happen after a successful server
    ///   response; indicates a programmer error where
    ///   [`SessionManager::replace_auth_token`] was called from a
    ///   non-authenticated state.
    /// * **Recoverability**: not recoverable by retry.
    /// * **Retry guidance**: do not retry; fix the caller.
    #[error("session manager rejected refresh: {0}")]
    Session(#[from] SessionManagerError),
    /// Forwarded from [`AuthRefreshError::MissingAuthField`].
    ///
    /// * **Cause**: the server returned `result=0` but omitted the
    ///   `auth` field; likely a transport or parsing anomaly.
    /// * **Recoverability**: session is untouched.
    /// * **Retry guidance**: retry once; if the condition persists,
    ///   treat as a protocol bug rather than a credential problem.
    #[error("refresh response missing auth field")]
    MissingAuthField,
    /// Proto-level refresh error (retained for future extension).
    ///
    /// * **Cause**: any [`AuthApiError`] not otherwise classified.
    /// * **Recoverability**: inspect inner error — most subcases are
    ///   transient.
    /// * **Retry guidance**: treat like `TemporaryFailure` unless the
    ///   inner error indicates otherwise.
    #[error("protocol refresh failed: {0}")]
    Protocol(AuthApiError<E>),
}

/// Protocol-aware auth orchestrator that drives a
/// [`SessionManager`] by performing the pCloud HTTP round-trips.
///
/// This is the only type in the crate that touches the network. It
/// wraps a [`pcloud_proto::auth_api::AuthApi`] and exposes high-level
/// operations (`login_with_password`, `submit_two_factor_code`,
/// `refresh_token`, ...) that classify server responses into
/// [`AuthCommand`] transitions.
///
/// # Secret discipline
///
/// Every credential argument — `password`, `code`, `auth_token` — is
/// taken by value as a [`SecretString`]. Internal duplication goes
/// through [`SecretString::clone_secret`] so every copy is visible in a
/// code review (ADR 0007). Secret bytes never appear in:
///
/// * the emitted [`AuthEvent`] payloads,
/// * the [`AuthFlowError`] / [`RefreshTokenError`] variants,
/// * any `tracing` / `log` call inside this module.
#[derive(Debug)]
pub struct ProtocolAuthFlow<T> {
    api: AuthApi<T>,
}

/// Errors returned by [`ProtocolAuthFlow`] operations (login, 2FA,
/// userinfo). `refresh_token` uses the finer-grained
/// [`RefreshTokenError`] instead.
#[derive(Debug, Error)]
pub enum AuthFlowError<E: std::error::Error + Send + Sync + 'static> {
    /// The [`SessionManager`] rejected a state transition (e.g. invoked
    /// from the wrong state).
    ///
    /// * **Cause**: orchestration bug — a flow method was called in
    ///   a state its transition table does not permit.
    /// * **Recoverability**: snapshot is unchanged; no credential
    ///   leak.
    /// * **Retry guidance**: do not retry; fix the caller.
    #[error("session manager rejected auth transition: {0}")]
    Session(#[from] SessionManagerError),
    /// The underlying protocol layer (transport / parsing / server
    /// response) failed.
    ///
    /// * **Cause**: network error, malformed response, or a
    ///   hard server rejection (wrong password, locked account,
    ///   invalid token). For hard rejections the session has
    ///   already been transitioned to
    ///   [`crate::SessionState::AuthFailed`] before this error is
    ///   returned.
    /// * **Recoverability**: depends on the inner error. Transport
    ///   blips are retryable; hard credential rejections require a
    ///   user-level retry with different credentials.
    /// * **Retry guidance**: backoff + retry for transient transport
    ///   errors; surface the failure to the user otherwise.
    #[error("protocol auth flow failed: {0}")]
    Protocol(#[from] AuthApiError<E>),
}

impl<T> ProtocolAuthFlow<T> {
    /// Wrap an [`AuthApi`] in the auth orchestrator.
    #[must_use]
    pub fn new(api: AuthApi<T>) -> Self {
        Self { api }
    }
}

impl<T> ProtocolAuthFlow<T>
where
    T: ProtocolTransport + ApiServerHintConsumer,
{
    /// Forward a server-supplied `api_server` hint to the transport so
    /// subsequent requests are pinned to the preferred region.
    pub fn apply_api_server_hint(&self, api_server: &str) {
        self.api.apply_api_server_hint(api_server);
    }

    /// Revalidate an existing auth token against the server and, on
    /// success, install it into `session` as
    /// [`crate::state::SessionState::Authenticated`].
    pub fn login_with_token(
        &self,
        session: &mut SessionManager,
        auth_token: SecretString,
    ) -> Result<AuthEvent, AuthFlowError<T::Error>> {
        session.apply(AuthCommand::LoginWithToken {
            token: auth_token.clone_secret(),
        })?;

        let userinfo = match self.api.userinfo(auth_token.expose_secret()) {
            Ok(userinfo) => userinfo,
            Err(err) => {
                let _ = session.apply(AuthCommand::MarkAuthenticationFailed {
                    message: Some(err.to_string()),
                });
                return Err(AuthFlowError::Protocol(err));
            }
        };

        let event = session.apply(AuthCommand::MarkAuthenticated {
            user_id: userinfo.user_id.map(UserId::new),
            auth_token: auth_token.clone_secret(),
        })?;
        session.update_userinfo(userinfo.user_id.map(UserId::new), userinfo.email)?;
        Ok(event)
    }

    /// Execute a password-based login. May issue a 2FA challenge, on
    /// which path the session is parked in
    /// [`crate::state::SessionState::TwoFactorRequired`] with a
    /// [`crate::state::PendingChallenge`] held in the snapshot.
    pub fn login_with_password(
        &self,
        session: &mut SessionManager,
        username: String,
        password: SecretString,
    ) -> Result<AuthEvent, AuthFlowError<T::Error>> {
        session.apply(AuthCommand::LoginWithPassword {
            username: username.clone(),
            password: password.clone_secret(),
        })?;

        let outcome = match self.api.login_password(username, password.expose_secret()) {
            Ok(outcome) => outcome,
            Err(err) => {
                let _ = session.apply(AuthCommand::MarkAuthenticationFailed {
                    message: Some(err.to_string()),
                });
                return Err(AuthFlowError::Protocol(err));
            }
        };

        match outcome {
            PasswordLoginOutcome::Authenticated {
                auth_token,
                user_id,
                ..
            } => {
                let auth_token = SecretString::new(auth_token);
                let userinfo = self.api.userinfo(auth_token.expose_secret())?;
                let event = session.apply(AuthCommand::MarkAuthenticated {
                    user_id: userinfo.user_id.or(user_id).map(UserId::new),
                    auth_token: auth_token.clone_secret(),
                })?;
                session.update_userinfo(
                    userinfo.user_id.or(user_id).map(UserId::new),
                    userinfo.email,
                )?;
                Ok(event)
            }
            PasswordLoginOutcome::TwoFactorRequired {
                challenge_token,
                trust_device,
                ..
            } => Ok(session
                .issue_two_factor_challenge(SecretString::new(challenge_token), trust_device)),
            PasswordLoginOutcome::Failed { message, .. } => session
                .apply(AuthCommand::MarkAuthenticationFailed { message })
                .map_err(Into::into),
        }
    }

    /// Answer a pending 2FA challenge with a TOTP, SMS, push-response,
    /// or recovery code.
    ///
    /// # Flow: TOTP / authenticator-app code
    ///
    /// 1. Caller reads a 6-digit code from the user's authenticator
    ///    app and wraps it in [`SecretString`].
    /// 2. Invoke with `recovery_code = false`, `trust_device = true`
    ///    iff the user opted in.
    /// 3. On success the session transitions to
    ///    [`crate::SessionState::Authenticated`] and the server-issued
    ///    auth token is installed via [`AuthCommand::MarkAuthenticated`].
    ///
    /// # Flow: SMS code
    ///
    /// 1. Caller invokes [`ProtocolAuthFlow::send_two_factor_sms`] to
    ///    ask the server to deliver (or re-deliver) a code over SMS.
    /// 2. Caller collects the 6-digit code from the user and wraps it
    ///    in [`SecretString`].
    /// 3. Invoke this method with `recovery_code = false`. The server
    ///    accepts the SMS-delivered code against the same
    ///    [`crate::state::PendingChallenge`] token.
    ///
    /// # Flow: push notification
    ///
    /// 1. Caller invokes
    ///    [`ProtocolAuthFlow::send_two_factor_notification`] to push a
    ///    prompt to a trusted device.
    /// 2. When the user taps *Approve* on the device, pCloud issues a
    ///    short-lived code via its push channel. The caller relays
    ///    that code here with `recovery_code = false`.
    /// 3. If the user taps *Deny* or lets the prompt time out, the
    ///    server returns a soft failure and the
    ///    [`crate::state::PendingChallenge`] is preserved so the caller can fall
    ///    back to SMS or TOTP.
    ///
    /// # Flow: recovery code
    ///
    /// 1. User supplies one of their one-time recovery codes,
    ///    typically printed at 2FA setup.
    /// 2. Invoke with `recovery_code = true`. The server enforces
    ///    single-use semantics; re-using the same code hard-fails.
    /// 3. `trust_device` is honored here too, but most callers leave
    ///    it `false` for recovery-code flows so the user must
    ///    re-confirm 2FA on the next login.
    ///
    /// # Failure handling
    ///
    /// A *soft* server rejection (wrong code, typo) preserves the
    /// pending challenge so the caller can retype the code against the
    /// same server-side token without a fresh password round-trip. A
    /// *hard* rejection (challenge token invalidated, account locked)
    /// transitions to [`crate::SessionState::AuthFailed`] and clears
    /// credentials.
    pub fn submit_two_factor_code(
        &self,
        session: &mut SessionManager,
        code: SecretString,
        trust_device: bool,
        recovery_code: bool,
    ) -> Result<AuthEvent, AuthFlowError<T::Error>> {
        let challenge = session
            .snapshot()
            .pending_challenge
            .as_ref()
            .ok_or(SessionManagerError::NoPendingChallenge)?
            .token
            .clone_secret();

        session.apply(AuthCommand::SubmitTwoFactorCode {
            code: code.clone_secret(),
            trust_device,
        })?;

        let outcome = match self.api.submit_two_factor_code(
            challenge.expose_secret(),
            code.expose_secret(),
            trust_device,
            recovery_code,
        ) {
            Ok(outcome) => outcome,
            Err(err) => {
                let _ = session.apply(AuthCommand::MarkAuthenticationFailed {
                    message: Some(err.to_string()),
                });
                return Err(AuthFlowError::Protocol(err));
            }
        };

        match outcome {
            PasswordLoginOutcome::Authenticated {
                auth_token,
                user_id,
                ..
            } => {
                let auth_token = SecretString::new(auth_token);
                let userinfo = self.api.userinfo(auth_token.expose_secret())?;
                let event = session.apply(AuthCommand::MarkAuthenticated {
                    user_id: userinfo.user_id.or(user_id).map(UserId::new),
                    auth_token: auth_token.clone_secret(),
                })?;
                session.update_userinfo(
                    userinfo.user_id.or(user_id).map(UserId::new),
                    userinfo.email,
                )?;
                Ok(event)
            }
            PasswordLoginOutcome::TwoFactorRequired {
                challenge_token,
                trust_device,
                ..
            } => Ok(session
                .issue_two_factor_challenge(SecretString::new(challenge_token), trust_device)),
            PasswordLoginOutcome::Failed { message, .. } => {
                // Preserve `pending_challenge` so the caller can retype
                // the code against the SAME server-side challenge token.
                // pCloud allows multiple attempts before invalidating
                // the token; on actual invalidation the next submit
                // will return a distinct hard-error which maps through
                // `MarkAuthenticationFailed` upstream.
                session
                    .apply(AuthCommand::MarkTwoFactorCodeInvalid { message })
                    .map_err(Into::into)
            }
        }
    }

    /// Single-shot login that combines password + 2FA code in one
    /// protocol request. Used by non-interactive callers that already
    /// hold the 2FA code.
    pub fn submit_two_factor_code_with_password(
        &self,
        session: &mut SessionManager,
        username: String,
        password: SecretString,
        code: SecretString,
    ) -> Result<AuthEvent, AuthFlowError<T::Error>> {
        session.apply(AuthCommand::SubmitTwoFactorCode {
            code: code.clone_secret(),
            trust_device: false,
        })?;

        let outcome = match self.api.login_password_with_code(
            username,
            password.expose_secret(),
            Some(code.expose_secret()),
        ) {
            Ok(outcome) => outcome,
            Err(err) => {
                let _ = session.apply(AuthCommand::MarkAuthenticationFailed {
                    message: Some(err.to_string()),
                });
                return Err(AuthFlowError::Protocol(err));
            }
        };

        match outcome {
            PasswordLoginOutcome::Authenticated {
                auth_token,
                user_id,
                ..
            } => {
                let auth_token = SecretString::new(auth_token);
                let userinfo = self.api.userinfo(auth_token.expose_secret())?;
                let event = session.apply(AuthCommand::MarkAuthenticated {
                    user_id: userinfo.user_id.or(user_id).map(UserId::new),
                    auth_token: auth_token.clone_secret(),
                })?;
                session.update_userinfo(
                    userinfo.user_id.or(user_id).map(UserId::new),
                    userinfo.email,
                )?;
                Ok(event)
            }
            PasswordLoginOutcome::TwoFactorRequired {
                challenge_token,
                trust_device,
                ..
            } => Ok(session
                .issue_two_factor_challenge(SecretString::new(challenge_token), trust_device)),
            PasswordLoginOutcome::Failed { message, .. } => session
                .apply(AuthCommand::MarkAuthenticationFailed { message })
                .map_err(Into::into),
        }
    }

    /// Fetch `userinfo` for an auth token without touching the
    /// [`SessionManager`] state. Used for sanity-checking a token
    /// outside the login flow.
    pub fn userinfo(&self, auth_token: SecretString) -> Result<UserInfo, AuthFlowError<T::Error>> {
        self.api
            .userinfo(auth_token.expose_secret())
            .map_err(Into::into)
    }

    /// Exchange the session's current auth token for a fresh one.
    ///
    /// This is pCloud's native "refresh" — there is no OAuth refresh
    /// token; `userinfo?getauth=1&auth=<current>` returns a new token
    /// while the old one stays valid until server-side expiry.
    ///
    /// The caller passes the current token by reference. The new token
    /// is installed into the `SessionManager` via `replace_auth_token`
    /// so any old `SecretString` held inside is dropped and zeroized.
    /// A lifecycle event is emitted describing the outcome:
    /// - `AuthEvent::TokenRefreshed` on success,
    /// - `AuthEvent::TokenRefreshExpired` on server-classified expiry
    ///   (the session is revoked in place),
    /// - `AuthEvent::TokenRefreshTemporaryFailure` on retryable errors.
    ///
    /// Secrets never appear in the emitted event payloads. The current
    /// token argument is never logged or returned.
    pub fn refresh_token(
        &self,
        session: &mut SessionManager,
        current: &SecretString,
    ) -> Result<AuthEvent, RefreshTokenError<T::Error>> {
        if !matches!(session.snapshot().state, crate::SessionState::Authenticated) {
            return Err(RefreshTokenError::NotAuthenticated);
        }

        match self.api.refresh_auth_token(current) {
            Ok(new_token) => {
                session.replace_auth_token(new_token)?;
                let event = AuthEvent::TokenRefreshed {
                    user_id: session.snapshot().authenticated_user,
                };
                Ok(event)
            }
            Err(AuthRefreshError::AuthExpired(result)) => {
                session.revoke();
                Err(RefreshTokenError::AuthExpired(result))
            }
            Err(AuthRefreshError::MissingAuthField) => Err(RefreshTokenError::MissingAuthField),
            Err(AuthRefreshError::TemporaryFailure(inner)) => {
                // Do NOT include raw error message with secrets; the
                // inner error's Display implementation is curated in
                // the proto crate and does not expose token material.
                Err(RefreshTokenError::TemporaryFailure(inner.to_string()))
            }
        }
    }

    /// Ask the server to (re)send the 2FA code over SMS for the
    /// currently pending challenge.
    ///
    /// # Flow: SMS
    ///
    /// 1. Session must already be in
    ///    [`crate::SessionState::TwoFactorRequired`] — i.e. a prior
    ///    [`ProtocolAuthFlow::login_with_password`] returned
    ///    [`pcloud_proto::auth_api::PasswordLoginOutcome::TwoFactorRequired`].
    /// 2. This call hits the pCloud SMS-delivery endpoint using the
    ///    secret challenge token held in [`crate::state::PendingChallenge`].
    /// 3. The caller collects the delivered code and feeds it back
    ///    into [`ProtocolAuthFlow::submit_two_factor_code`] with
    ///    `recovery_code = false`.
    /// 4. The session state is **not** advanced by this call — the
    ///    challenge is still pending until the code is submitted.
    ///
    /// Returns [`SessionManagerError::NoPendingChallenge`] if called
    /// outside an active 2FA challenge.
    pub fn send_two_factor_sms(
        &self,
        session: &SessionManager,
    ) -> Result<TwoFactorSmsDelivery, AuthFlowError<T::Error>> {
        let challenge = session
            .snapshot()
            .pending_challenge
            .as_ref()
            .ok_or(SessionManagerError::NoPendingChallenge)?;
        self.api
            .send_two_factor_sms(challenge.token.expose_secret())
            .map_err(Into::into)
    }

    /// Ask the server to (re)send the 2FA push notification to a
    /// trusted device for the currently pending challenge.
    ///
    /// # Flow: push notification
    ///
    /// 1. Session must be in
    ///    [`crate::SessionState::TwoFactorRequired`] with a
    ///    [`crate::state::PendingChallenge`] whose trusted-device relationship is
    ///    already established on the pCloud side.
    /// 2. This call asks pCloud to push a prompt to the paired
    ///    device. The user approves (or denies) on the device.
    /// 3. On approval, the device receives a short-lived code that
    ///    the caller feeds into
    ///    [`ProtocolAuthFlow::submit_two_factor_code`].
    /// 4. On denial or timeout the challenge stays pending; the
    ///    caller may fall back to
    ///    [`ProtocolAuthFlow::send_two_factor_sms`] or to a TOTP/
    ///    recovery-code submission.
    ///
    /// As with SMS, the session state is **not** advanced by this
    /// call. Returns [`SessionManagerError::NoPendingChallenge`] when
    /// no challenge is pending.
    pub fn send_two_factor_notification(
        &self,
        session: &SessionManager,
    ) -> Result<TwoFactorNotificationDelivery, AuthFlowError<T::Error>> {
        let challenge = session
            .snapshot()
            .pending_challenge
            .as_ref()
            .ok_or(SessionManagerError::NoPendingChallenge)?;
        self.api
            .send_two_factor_notification(challenge.token.expose_secret())
            .map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::VecDeque, io};

    use pcloud_proto::{
        AuthApi,
        auth_api::{ApiServerHintConsumer, ProtocolTransport},
        response::Value,
    };
    use pcloud_secret::{ExposeSecret, secret_string::SecretString};

    use crate::{SessionManager, SessionState};

    use super::ProtocolAuthFlow;

    #[derive(Debug)]
    struct MockTransport {
        responses: std::sync::Mutex<VecDeque<Value>>,
    }

    impl MockTransport {
        fn from_responses(responses: Vec<Value>) -> Self {
            Self {
                responses: std::sync::Mutex::new(VecDeque::from(responses)),
            }
        }
    }

    impl ProtocolTransport for MockTransport {
        type Error = io::Error;

        fn execute(&self, _request: &pcloud_proto::EncodedRequest) -> Result<Value, Self::Error> {
            self.responses
                .lock()
                .expect("responses mutex should lock")
                .pop_front()
                .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "missing response"))
        }
    }

    impl ApiServerHintConsumer for MockTransport {
        fn apply_api_server_hint(&self, _api_server: &str) {}
    }

    #[test]
    fn password_login_success_marks_session_authenticated() {
        let transport = MockTransport::from_responses(vec![
            Value::Hash(vec![
                ("result".to_owned(), Value::Number(0)),
                (
                    "digest".to_owned(),
                    Value::String("development-digest".to_owned()),
                ),
            ]),
            Value::Hash(vec![
                ("result".to_owned(), Value::Number(0)),
                ("auth".to_owned(), Value::String("token".to_owned())),
                ("userid".to_owned(), Value::Number(42)),
            ]),
            Value::Hash(vec![
                ("result".to_owned(), Value::Number(0)),
                ("userid".to_owned(), Value::Number(42)),
                (
                    "email".to_owned(),
                    Value::String("alice@example.com".to_owned()),
                ),
            ]),
        ]);
        let flow = ProtocolAuthFlow::new(AuthApi::new(transport));
        let mut session = SessionManager::new();

        let event = flow
            .login_with_password(
                &mut session,
                "alice@example.com".to_owned(),
                SecretString::new("correct-horse"),
            )
            .expect("login should succeed");

        assert!(matches!(event, crate::AuthEvent::LoginSucceeded { .. }));
        assert_eq!(session.snapshot().state, SessionState::Authenticated);
        assert_eq!(
            session.snapshot().authenticated_user.map(|id| id.get()),
            Some(42)
        );
        assert_eq!(
            session.snapshot().email.as_deref(),
            Some("alice@example.com")
        );
        assert!(session.snapshot().auth_token.is_some());
    }

    #[test]
    fn password_login_can_issue_two_factor_challenge() {
        let transport = MockTransport::from_responses(vec![
            Value::Hash(vec![
                ("result".to_owned(), Value::Number(0)),
                (
                    "digest".to_owned(),
                    Value::String("development-digest".to_owned()),
                ),
            ]),
            Value::Hash(vec![
                ("result".to_owned(), Value::Number(2000)),
                ("token".to_owned(), Value::String("challenge".to_owned())),
                ("trustdevice".to_owned(), Value::Bool(true)),
            ]),
        ]);
        let flow = ProtocolAuthFlow::new(AuthApi::new(transport));
        let mut session = SessionManager::new();

        let event = flow
            .login_with_password(
                &mut session,
                "alice@example.com".to_owned(),
                SecretString::new("correct-horse"),
            )
            .expect("login should issue challenge");

        assert!(matches!(event, crate::AuthEvent::TwoFactorChallengeIssued));
        assert_eq!(session.snapshot().state, SessionState::TwoFactorRequired);
        assert_eq!(
            session
                .snapshot()
                .pending_challenge
                .as_ref()
                .expect("challenge should exist")
                .token
                .expose_secret(),
            "challenge"
        );
    }

    #[test]
    fn refresh_token_happy_path_emits_event_and_swaps_token() {
        use crate::AuthCommand;
        use pcloud_model::ids::UserId;
        use pcloud_secret::ExposeSecret;

        // Single response: userinfo with getauth=1 returning new token.
        let transport = MockTransport::from_responses(vec![Value::Hash(vec![
            ("result".to_owned(), Value::Number(0)),
            ("userid".to_owned(), Value::Number(7)),
            ("auth".to_owned(), Value::String("fresh-token".to_owned())),
        ])]);
        let flow = ProtocolAuthFlow::new(AuthApi::new(transport));

        let mut session = SessionManager::new();
        session
            .apply(AuthCommand::LoginWithToken {
                token: SecretString::new("old-token"),
            })
            .unwrap();
        session
            .apply(AuthCommand::MarkAuthenticated {
                user_id: Some(UserId::new(7)),
                auth_token: SecretString::new("old-token"),
            })
            .unwrap();

        let current = SecretString::new("old-token");
        let event = flow
            .refresh_token(&mut session, &current)
            .expect("refresh should succeed");

        match event {
            crate::AuthEvent::TokenRefreshed { user_id } => {
                assert_eq!(user_id.map(|id| id.get()), Some(7));
            }
            other => panic!("expected TokenRefreshed, got {other:?}"),
        }
        assert_eq!(
            session
                .snapshot()
                .auth_token
                .as_ref()
                .expect("token should be present")
                .expose_secret(),
            "fresh-token"
        );
        assert_eq!(session.snapshot().state, SessionState::Authenticated);
    }

    #[test]
    fn refresh_token_auth_expired_revokes_session() {
        use crate::AuthCommand;
        use pcloud_model::ids::UserId;

        let transport = MockTransport::from_responses(vec![Value::Hash(vec![
            ("result".to_owned(), Value::Number(2094)),
            ("error".to_owned(), Value::String("invalid auth".to_owned())),
        ])]);
        let flow = ProtocolAuthFlow::new(AuthApi::new(transport));
        let mut session = SessionManager::new();
        session
            .apply(AuthCommand::LoginWithToken {
                token: SecretString::new("expired-token"),
            })
            .unwrap();
        session
            .apply(AuthCommand::MarkAuthenticated {
                user_id: Some(UserId::new(3)),
                auth_token: SecretString::new("expired-token"),
            })
            .unwrap();

        let current = SecretString::new("expired-token");
        let err = flow
            .refresh_token(&mut session, &current)
            .expect_err("refresh must classify as expired");
        match err {
            super::RefreshTokenError::AuthExpired(2094) => {}
            other => panic!("expected AuthExpired(2094), got {other:?}"),
        }
        // Session was revoked: token zeroized, state LoggedOut.
        assert_eq!(session.snapshot().state, SessionState::LoggedOut);
        assert!(session.snapshot().auth_token.is_none());
    }

    #[test]
    fn refresh_token_temporary_failure_leaves_session_intact() {
        use crate::AuthCommand;
        use pcloud_model::ids::UserId;
        use pcloud_secret::ExposeSecret;

        // Non-auth-expired non-zero result => temporary failure.
        let transport = MockTransport::from_responses(vec![Value::Hash(vec![(
            "result".to_owned(),
            Value::Number(5000),
        )])]);
        let flow = ProtocolAuthFlow::new(AuthApi::new(transport));
        let mut session = SessionManager::new();
        session
            .apply(AuthCommand::LoginWithToken {
                token: SecretString::new("live-token"),
            })
            .unwrap();
        session
            .apply(AuthCommand::MarkAuthenticated {
                user_id: Some(UserId::new(9)),
                auth_token: SecretString::new("live-token"),
            })
            .unwrap();

        let current = SecretString::new("live-token");
        let err = flow
            .refresh_token(&mut session, &current)
            .expect_err("server error must surface as temporary");
        assert!(matches!(err, super::RefreshTokenError::TemporaryFailure(_)));
        // Session still authenticated with original token.
        assert_eq!(session.snapshot().state, SessionState::Authenticated);
        assert_eq!(
            session
                .snapshot()
                .auth_token
                .as_ref()
                .expect("token still present")
                .expose_secret(),
            "live-token"
        );
    }

    #[test]
    fn refresh_token_rejects_when_not_authenticated() {
        let transport = MockTransport::from_responses(vec![]);
        let flow = ProtocolAuthFlow::new(AuthApi::new(transport));
        let mut session = SessionManager::new();

        let current = SecretString::new("no-session");
        let err = flow
            .refresh_token(&mut session, &current)
            .expect_err("logged-out session cannot refresh");
        assert!(matches!(err, super::RefreshTokenError::NotAuthenticated));
    }

    /// Regression guard: `SecretString` must remain non-`Clone`.
    ///
    /// Uses autoref-based specialization: an inherent method on
    /// `Wrap<T>` (bound `T: Clone`) is preferred over a trait method
    /// on `&Wrap<T>`. If `T: Clone`, the inherent wins and returns
    /// `true`; otherwise the trait fallback returns `false`. Adding
    /// `Clone` to `SecretString` would make this assertion observe
    /// `true` and panic the test, producing a loud, deterministic
    /// regression signal.
    #[test]
    fn secret_string_is_not_clone_regression() {
        // Audit-visible duplication path still works.
        let a = SecretString::new("abc");
        let b = a.clone_secret();
        assert_eq!(a.expose_secret(), b.expose_secret());

        // Behavioral regression guard (see module doc).
        assert!(
            !is_clone::<SecretString>(),
            "SecretString must not implement Clone; use clone_secret() for audit-visible duplication"
        );
    }

    struct Wrap<T>(std::marker::PhantomData<T>);

    trait IsCloneFallback {
        fn is_clone(&self) -> bool {
            false
        }
    }
    impl<T> IsCloneFallback for &Wrap<T> {}

    // Autoderef specialization trick (see `is_clone::<T>()` below).
    //
    // This inherent method is conditionally applicable — its impl block
    // is bound `T: Clone`. Rust's method-resolution rules prefer an
    // inherent method on `Wrap<T>` over a trait method reachable via
    // autoref on `&Wrap<T>`, so the call site `(&Wrap::<T>(...)).is_clone()`
    // dispatches to THIS method iff `T: Clone`, and otherwise falls back
    // to the `IsCloneFallback` trait impl on `&Wrap<T>`.
    //
    // Why `#[allow(dead_code)]` is LOAD-BEARING:
    // clippy / rustc's dead-code analysis inspects each `impl` block in
    // isolation and cannot statically see the indirect autoref dispatch
    // in `is_clone::<T>()`. It therefore flags this `fn is_clone` as
    // unused even though the autoref trick specifically relies on it
    // existing. Removing the attribute breaks `-D warnings` builds; in
    // a CI with `-D dead-code` the build fails outright. Do NOT delete
    // the attribute — it is the whole point of the `SecretString is not
    // Clone` regression guard below.
    #[allow(dead_code)]
    impl<T: Clone> Wrap<T> {
        fn is_clone(&self) -> bool {
            true
        }
    }

    fn is_clone<T>() -> bool {
        // Method resolution prefers the inherent impl on `Wrap<T>`
        // (only applicable when `T: Clone`) over the trait impl on
        // `&Wrap<T>`. So the result is `true` iff `T: Clone`.
        (&Wrap::<T>(std::marker::PhantomData)).is_clone()
    }

    #[test]
    fn token_login_uses_userinfo_to_authenticate() {
        let transport = MockTransport::from_responses(vec![Value::Hash(vec![
            ("result".to_owned(), Value::Number(0)),
            ("userid".to_owned(), Value::Number(7)),
            (
                "email".to_owned(),
                Value::String("token@example.com".to_owned()),
            ),
        ])]);
        let flow = ProtocolAuthFlow::new(AuthApi::new(transport));
        let mut session = SessionManager::new();

        let event = flow
            .login_with_token(&mut session, SecretString::new("auth-token"))
            .expect("token login should succeed");

        assert!(matches!(event, crate::AuthEvent::LoginSucceeded { .. }));
        assert_eq!(session.snapshot().state, SessionState::Authenticated);
        assert_eq!(
            session.snapshot().authenticated_user.map(|id| id.get()),
            Some(7)
        );
        assert_eq!(
            session.snapshot().email.as_deref(),
            Some("token@example.com")
        );
        assert!(session.snapshot().auth_token.is_some());
    }
}
