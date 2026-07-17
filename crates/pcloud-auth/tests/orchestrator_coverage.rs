#![allow(clippy::pedantic)]
//! Public-API coverage for protocol-backed authentication orchestration.

use std::{
    collections::VecDeque,
    io,
    sync::{Arc, Mutex},
};

use pcloud_auth::{AuthEvent, ProtocolAuthFlow, SessionManager, SessionState};
use pcloud_proto::{
    AuthApi, EncodedRequest,
    auth_api::{ApiServerHintConsumer, ProtocolTransport},
    response::Value,
};
use pcloud_secret::secret_string::SecretString;

#[derive(Debug, Clone)]
struct ScriptedTransport {
    responses: Arc<Mutex<VecDeque<Result<Value, io::ErrorKind>>>>,
}

impl ScriptedTransport {
    fn new(responses: impl IntoIterator<Item = Value>) -> Self {
        Self {
            responses: Arc::new(Mutex::new(
                responses.into_iter().map(Ok).collect::<VecDeque<_>>(),
            )),
        }
    }

    fn failing(kind: io::ErrorKind) -> Self {
        Self {
            responses: Arc::new(Mutex::new(VecDeque::from([Err(kind)]))),
        }
    }
}

impl ProtocolTransport for ScriptedTransport {
    type Error = io::Error;

    fn execute(&self, _request: &EncodedRequest) -> Result<Value, Self::Error> {
        self.responses
            .lock()
            .expect("scripted responses should lock")
            .pop_front()
            .expect("test supplied enough responses")
            .map_err(|kind| io::Error::new(kind, "scripted transport failure"))
    }
}

impl ApiServerHintConsumer for ScriptedTransport {
    fn apply_api_server_hint(&self, _api_server: &str) {}
}

fn hash(fields: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
    Value::Hash(
        fields
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect(),
    )
}

fn challenge_session() -> SessionManager {
    let mut session = SessionManager::new();
    session.issue_two_factor_challenge(SecretString::new("challenge-token"), false);
    session
}

fn flow(responses: impl IntoIterator<Item = Value>) -> ProtocolAuthFlow<ScriptedTransport> {
    ProtocolAuthFlow::new(AuthApi::new(ScriptedTransport::new(responses)))
}

#[test]
fn submitted_two_factor_code_covers_success_rechallenge_and_soft_failure() {
    let mut session = challenge_session();
    let authenticated = flow([
        hash([
            ("result", Value::Number(0)),
            ("auth", Value::String("authenticated-token".to_owned())),
            ("userid", Value::Number(73)),
        ]),
        hash([
            ("result", Value::Number(0)),
            ("userid", Value::Number(73)),
            ("email", Value::String("coverage@example.test".to_owned())),
        ]),
    ])
    .submit_two_factor_code(&mut session, SecretString::new("123456"), true, false)
    .expect("valid two-factor code should authenticate");
    assert!(matches!(authenticated, AuthEvent::LoginSucceeded { .. }));
    assert_eq!(session.snapshot().state, SessionState::Authenticated);
    assert_eq!(
        session.snapshot().email.as_deref(),
        Some("coverage@example.test")
    );

    let mut session = challenge_session();
    let rechallenge = flow([hash([
        ("result", Value::Number(2000)),
        (
            "tfa_token",
            Value::String("replacement-challenge".to_owned()),
        ),
        ("trustdevice", Value::Bool(true)),
    ])])
    .submit_two_factor_code(&mut session, SecretString::new("234567"), false, true)
    .expect("server may replace a two-factor challenge");
    assert_eq!(rechallenge, AuthEvent::TwoFactorChallengeIssued);
    assert_eq!(session.snapshot().state, SessionState::TwoFactorRequired);

    let mut session = challenge_session();
    let rejected = flow([hash([
        ("result", Value::Number(2001)),
        ("error", Value::String("wrong code".to_owned())),
    ])])
    .submit_two_factor_code(&mut session, SecretString::new("345678"), false, false)
    .expect("soft rejection should remain an auth event");
    assert!(matches!(rejected, AuthEvent::LoginFailed { .. }));
    assert!(session.snapshot().pending_challenge.is_some());
}

#[test]
fn submitted_two_factor_code_covers_missing_challenge_and_protocol_failure() {
    let mut missing = SessionManager::new();
    assert!(
        flow([])
            .submit_two_factor_code(&mut missing, SecretString::new("000000"), false, false,)
            .is_err()
    );

    let transport = ScriptedTransport::failing(io::ErrorKind::ConnectionReset);
    let mut session = challenge_session();
    assert!(
        ProtocolAuthFlow::new(AuthApi::new(transport))
            .submit_two_factor_code(&mut session, SecretString::new("456789"), false, false,)
            .is_err()
    );
    assert_eq!(session.snapshot().state, SessionState::AuthFailed);
    assert!(session.snapshot().pending_challenge.is_none());
}

#[test]
fn password_plus_code_covers_all_server_outcomes() {
    let digest = || {
        hash([
            ("result", Value::Number(0)),
            ("digest", Value::String("coverage-digest".to_owned())),
        ])
    };

    let mut session = challenge_session();
    let authenticated = flow([
        digest(),
        hash([
            ("result", Value::Number(0)),
            ("auth", Value::String("combined-token".to_owned())),
            ("userid", Value::Number(91)),
        ]),
        hash([
            ("result", Value::Number(0)),
            ("userid", Value::Number(91)),
            ("email", Value::String("combined@example.test".to_owned())),
        ]),
    ])
    .submit_two_factor_code_with_password(
        &mut session,
        "combined@example.test".to_owned(),
        SecretString::new("password"),
        SecretString::new("567890"),
    )
    .expect("combined login should authenticate");
    assert!(matches!(authenticated, AuthEvent::LoginSucceeded { .. }));

    let mut session = challenge_session();
    let rechallenge = flow([
        digest(),
        hash([
            ("result", Value::Number(2297)),
            ("trustdevice", Value::Bool(false)),
        ]),
    ])
    .submit_two_factor_code_with_password(
        &mut session,
        "rechallenge@example.test".to_owned(),
        SecretString::new("password"),
        SecretString::new("678901"),
    )
    .expect("combined login may still require another challenge");
    assert_eq!(rechallenge, AuthEvent::TwoFactorChallengeIssued);

    let mut session = challenge_session();
    let failed = flow([
        digest(),
        hash([
            ("result", Value::Number(2003)),
            ("error", Value::String("invalid credentials".to_owned())),
        ]),
    ])
    .submit_two_factor_code_with_password(
        &mut session,
        "failed@example.test".to_owned(),
        SecretString::new("password"),
        SecretString::new("789012"),
    )
    .expect("hard rejection should be represented by an auth event");
    assert!(matches!(failed, AuthEvent::LoginFailed { .. }));
    assert_eq!(session.snapshot().state, SessionState::AuthFailed);

    let mut session = challenge_session();
    assert!(
        flow([digest(), Value::String("malformed".to_owned())])
            .submit_two_factor_code_with_password(
                &mut session,
                "malformed@example.test".to_owned(),
                SecretString::new("password"),
                SecretString::new("890123"),
            )
            .is_err()
    );
    assert_eq!(session.snapshot().state, SessionState::AuthFailed);
}
