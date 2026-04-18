//! Interactive crypto-backend picker for `pcloudc crypto setup`.
//!
//! This module encapsulates the UX for selecting between the
//! pcloudcom-interoperable backend (`pclsync-compat`) and the stricter
//! but non-interoperable `enhanced` backend. It is split into its own
//! module so unit tests can feed synthetic stdin / capture stdout
//! without touching the real terminal.
//!
//! The picker is only ever reached when the user invoked
//! `crypto setup` without `--backend` AND stdin is a tty; the
//! non-interactive scripted path rejects the command with
//! [`crate::exit_code::ExitCode::Usage`] before the picker is
//! entered. See Stage 4b.4 in `docs/CRYPTO-BACKEND-PLAN.md`.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::io::{BufRead, Write};

use pcloud_ipc::methods::CryptoBackendIpc;

/// Outcome of an interactive picker run.
///
/// `Selected` carries the user's choice plus the implicit
/// `acknowledge_not_interop` flag (always `true` when the user
/// explicitly confirmed `YES` for the `enhanced` backend, `false`
/// otherwise — the flag is inert for `PclsyncCompat`).
///
/// `Aborted` carries a short human-readable reason printed by the
/// caller before it returns a non-zero exit code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickerOutcome {
    /// User picked a backend. For the `enhanced` branch the
    /// `acknowledge_not_interop` flag is `true` (YES was typed).
    Selected {
        backend: CryptoBackendIpc,
        acknowledge_not_interop: bool,
    },
    /// User aborted (declined YES, invalid input three times, or EOF).
    Aborted(String),
}

/// Render the backend-picker prompt and read a single choice from
/// `input`. Returns [`PickerOutcome::Selected`] on a valid pick or
/// [`PickerOutcome::Aborted`] on an invalid one (propagated from
/// [`ask_enhanced_confirmation`] when the user picked branch 2).
///
/// Uses up to [`MAX_RETRIES`] attempts before aborting; this matches
/// the wording in the Stage 4b.4 CLI spec: "Invalid choice. Enter 1 or
/// 2." and "max 3 retries, then abort".
pub const MAX_RETRIES: u32 = 3;

/// Main picker entry point. Writes the menu + prompt to `output`,
/// reads the user's answer from `input`, and returns a
/// [`PickerOutcome`]. `input` is consumed line-by-line so the same
/// picker test can feed the primary choice and the `YES`/abort
/// confirmation through a single reader.
pub fn run_picker<R: BufRead, W: Write>(input: &mut R, output: &mut W) -> PickerOutcome {
    let menu = concat!(
        "Choose a crypto backend:\n",
        "\n",
        "  1. pclsync-compat  (default, recommended)\n",
        "       - Byte-compatible with the official pCloud apps\n",
        "       - Files you encrypt here will decrypt in any pCloud client\n",
        "\n",
        "  2. enhanced\n",
        "       - Stricter crypto (AES-256-GCM + Argon2id)\n",
        "       - NOT interoperable with official pCloud apps\n",
        "       - Files encrypted here will NOT decrypt in pCloud desktop, web, iOS, or Android\n",
        "\n",
    );
    let _ = output.write_all(menu.as_bytes());
    let _ = output.flush();

    for _ in 0..MAX_RETRIES {
        let _ = output.write_all(b"Enter 1 or 2 [1]: ");
        let _ = output.flush();
        let choice = match read_line_trimmed(input) {
            Some(s) => s,
            None => {
                return PickerOutcome::Aborted("setup aborted (EOF on stdin)".to_owned());
            }
        };
        match choice.as_str() {
            "" | "1" => {
                return PickerOutcome::Selected {
                    backend: CryptoBackendIpc::PclsyncCompat,
                    acknowledge_not_interop: false,
                };
            }
            "2" => return ask_enhanced_confirmation(input, output),
            _ => {
                let _ = output.write_all(b"Invalid choice. Enter 1 or 2.\n");
                let _ = output.flush();
            }
        }
    }
    PickerOutcome::Aborted("setup aborted (too many invalid choices)".to_owned())
}

/// Second-stage confirmation for the `enhanced` branch. Prints the
/// irreversibility warning and requires the literal string `YES`
/// (case-sensitive) to commit; anything else aborts with the exact
/// "setup aborted" message documented in the Stage 4b.4 spec.
fn ask_enhanced_confirmation<R: BufRead, W: Write>(input: &mut R, output: &mut W) -> PickerOutcome {
    let warning = concat!(
        "\n",
        "This choice means:\n",
        "\n",
        "  * Your encrypted files will NOT open in the official pCloud apps.\n",
        "  * You will only be able to access them through pcloud-rs.\n",
        "  * This cannot be reversed after setup without re-encrypting all data.\n",
        "\n",
        "Type YES in full caps to confirm: ",
    );
    let _ = output.write_all(warning.as_bytes());
    let _ = output.flush();
    match read_line_trimmed(input) {
        Some(answer) if answer == "YES" => PickerOutcome::Selected {
            backend: CryptoBackendIpc::Enhanced,
            acknowledge_not_interop: true,
        },
        _ => PickerOutcome::Aborted("setup aborted".to_owned()),
    }
}

/// Read one line from `input`, stripping the trailing newline. Returns
/// `None` on EOF so callers can distinguish Ctrl-D from an empty
/// answer (empty answer still comes back as `Some("")`).
fn read_line_trimmed<R: BufRead>(input: &mut R) -> Option<String> {
    let mut buf = String::new();
    match input.read_line(&mut buf) {
        Ok(0) => None,
        Ok(_) => Some(buf.trim_end_matches(['\r', '\n']).to_owned()),
        Err(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn pick(stdin: &str) -> (PickerOutcome, String) {
        let mut inp = Cursor::new(stdin.as_bytes().to_vec());
        let mut out: Vec<u8> = Vec::new();
        let outcome = run_picker(&mut inp, &mut out);
        (outcome, String::from_utf8(out).unwrap())
    }

    #[test]
    fn choice_1_selects_pclsync_compat() {
        let (outcome, _) = pick("1\n");
        assert_eq!(
            outcome,
            PickerOutcome::Selected {
                backend: CryptoBackendIpc::PclsyncCompat,
                acknowledge_not_interop: false,
            }
        );
    }

    #[test]
    fn empty_selects_pclsync_compat_default() {
        let (outcome, _) = pick("\n");
        assert_eq!(
            outcome,
            PickerOutcome::Selected {
                backend: CryptoBackendIpc::PclsyncCompat,
                acknowledge_not_interop: false,
            }
        );
    }

    #[test]
    fn choice_2_requires_yes_in_full_caps() {
        let (outcome, _) = pick("2\nYES\n");
        assert_eq!(
            outcome,
            PickerOutcome::Selected {
                backend: CryptoBackendIpc::Enhanced,
                acknowledge_not_interop: true,
            }
        );
    }

    #[test]
    fn choice_2_aborts_when_user_declines() {
        let (outcome, _) = pick("2\nno\n");
        matches!(outcome, PickerOutcome::Aborted(_));
        let PickerOutcome::Aborted(msg) = outcome else {
            panic!("expected abort");
        };
        assert!(msg.contains("setup aborted"));
    }

    #[test]
    fn choice_2_aborts_on_lowercase_yes() {
        // Spec is strict: only literal "YES" commits.
        let (outcome, _) = pick("2\nyes\n");
        assert!(matches!(outcome, PickerOutcome::Aborted(_)));
    }

    #[test]
    fn invalid_choices_reprompt_up_to_three_times_then_abort() {
        let (outcome, out) = pick("foo\nbar\nbaz\n");
        let PickerOutcome::Aborted(msg) = outcome else {
            panic!("expected abort after 3 invalid choices");
        };
        assert!(msg.contains("too many invalid choices"));
        // Three "Invalid choice" error lines were printed.
        assert_eq!(out.matches("Invalid choice. Enter 1 or 2.").count(), 3);
    }

    #[test]
    fn menu_contains_interop_warning_for_enhanced_branch() {
        let (_, out) = pick("1\n");
        assert!(out.contains("NOT interoperable with official pCloud apps"));
        assert!(out.contains("pclsync-compat"));
    }
}
