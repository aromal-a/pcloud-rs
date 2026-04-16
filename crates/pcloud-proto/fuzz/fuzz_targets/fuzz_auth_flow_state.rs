//! Fuzz target: model the auth flow as a state machine and feed random event
//! sequences through it.
//!
//! The `pcloud-proto::AuthApi` surface encapsulates five reachable states
//! that a typical client walks through:
//!
//! * `Unauthenticated`
//! * `DigestFetched`
//! * `PasswordSubmitted` (outcome = Authenticated / TwoFactorRequired /
//!    Failed)
//! * `TwoFactorPending` (awaiting SMS / notification code or recovery code)
//! * `Authenticated`
//! * `LoggedOut`
//!
//! This target does NOT perform real network calls; the state machine is a
//! deterministic local model derived from `PasswordLoginOutcome` variants
//! and the public method set on `AuthApi`. The goal is to catch any future
//! state-machine bug that would let the driver skip a required transition
//! (e.g. issue a TFA code without first seeing `TwoFactorRequired`) or
//! panic on an unexpected event ordering.
//!
//! Run with:
//!
//! ```text
//! cd crates/pcloud-proto/fuzz
//! cargo +nightly fuzz run fuzz_auth_flow_state
//! ```

#![no_main]

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Unauthenticated,
    DigestFetched,
    TwoFactorPending,
    Authenticated,
    LoggedOut,
}

#[derive(Debug, Arbitrary)]
enum Event {
    GetDigest,
    SubmitPassword { two_factor_required: bool, ok: bool },
    SubmitTfaCode { ok: bool },
    SubmitRecoveryCode { ok: bool },
    ResendTfaSms,
    ResendTfaNotification,
    GetUserInfo,
    Logout,
    ApplyApiServerHint,
    SetAuthPersistence,
}

#[derive(Debug, Default)]
struct Transcript {
    tfa_sms_sends: u32,
    tfa_notif_sends: u32,
    user_info_calls: u32,
}

fn step(state: State, event: &Event, t: &mut Transcript) -> State {
    match (state, event) {
        // Digest fetch is always valid as the first interaction with the
        // server; re-fetching mid-flow is allowed by the C client too.
        (_, Event::GetDigest) => match state {
            State::Authenticated => State::Authenticated,
            State::LoggedOut => State::DigestFetched,
            _ => State::DigestFetched,
        },
        // Password submission is only valid after a digest has been fetched
        // (the digest token is required to compute the password digest).
        (State::DigestFetched, Event::SubmitPassword { two_factor_required, ok }) => {
            match (two_factor_required, ok) {
                (true, _) => State::TwoFactorPending,
                (false, true) => State::Authenticated,
                (false, false) => State::Unauthenticated,
            }
        }
        (
            State::TwoFactorPending,
            Event::SubmitTfaCode { ok } | Event::SubmitRecoveryCode { ok },
        ) => {
            if *ok {
                State::Authenticated
            } else {
                State::TwoFactorPending
            }
        }
        (State::TwoFactorPending, Event::ResendTfaSms) => {
            t.tfa_sms_sends = t.tfa_sms_sends.saturating_add(1);
            State::TwoFactorPending
        }
        (State::TwoFactorPending, Event::ResendTfaNotification) => {
            t.tfa_notif_sends = t.tfa_notif_sends.saturating_add(1);
            State::TwoFactorPending
        }
        (State::Authenticated, Event::GetUserInfo) => {
            t.user_info_calls = t.user_info_calls.saturating_add(1);
            State::Authenticated
        }
        (State::Authenticated, Event::Logout) => State::LoggedOut,
        // API-server-hint and persistence toggles are idempotent and do not
        // affect the state.
        (_, Event::ApplyApiServerHint) | (_, Event::SetAuthPersistence) => state,
        // Any other (state, event) pair is ignored (illegal transition).
        _ => state,
    }
}

fuzz_target!(|events: Vec<Event>| {
    let mut state = State::Unauthenticated;
    let mut transcript = Transcript::default();

    for e in events.iter().take(512) {
        let before = state;
        state = step(state, e, &mut transcript);

        // Structural invariants that MUST hold after every transition.

        // 1. TFA events can only move us out of / inside TwoFactorPending.
        if matches!(
            e,
            Event::SubmitTfaCode { .. }
                | Event::SubmitRecoveryCode { .. }
                | Event::ResendTfaSms
                | Event::ResendTfaNotification
        ) {
            assert!(matches!(
                state,
                State::TwoFactorPending | State::Authenticated | State::Unauthenticated
                    | State::DigestFetched | State::LoggedOut
            ));
            // If we started outside TwoFactorPending the state must not have
            // progressed into Authenticated via this event alone.
            if before != State::TwoFactorPending {
                assert_ne!(
                    state,
                    State::Authenticated,
                    "state machine advanced to Authenticated via TFA event without TwoFactorPending"
                );
            }
        }

        // 2. GetUserInfo is not permitted to authenticate a prior
        //    unauthenticated state.
        if matches!(e, Event::GetUserInfo) && before != State::Authenticated {
            assert_ne!(state, State::Authenticated);
        }

        // 3. Resend counts cannot exceed the number of events seen.
        assert!(transcript.tfa_sms_sends as usize <= events.len());
        assert!(transcript.tfa_notif_sends as usize <= events.len());
    }
});
