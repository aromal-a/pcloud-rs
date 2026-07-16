//! Interactive terminal prompts for `pcloudc` login / unlock flows.
//!
//! # [`SecretPrompt`]
//!
//! A thin wrapper around stdin with three read modes:
//!
//! - [`SecretPrompt::read_line`] — normal visible echo (usernames,
//!   free-form values);
//! - [`SecretPrompt::read_secret`] — no echo, powered by the
//!   `rpassword` crate (passwords, auth tokens);
//! - [`SecretPrompt::read_masked`] — per-byte echo of `*` for each
//!   printable character typed, used for the masked 2FA / recovery
//!   code flow where the user benefits from a "did I actually hit six
//!   digits?" affordance without revealing the value on screen.
//!
//! # termios RAII guard
//!
//! The masked mode on Linux drops the terminal into non-canonical + no
//! echo via `tcsetattr(TCSANOW)`. Restoring the original termios is
//! managed by a private `Restore` struct whose `Drop` impl re-applies
//! the saved `termios` on **every** exit path — normal return, early
//! `Err(...)` propagation, or an unwinding panic. This guarantees the
//! user's shell is never left in `-icanon -echo` after `pcloudc`
//! exits, even if the caller interrupts the process mid-read.
//!
//! # Secret hygiene
//!
//! Results are returned as `String` to the caller, which is expected
//! to immediately wrap them in
//! [`pcloud_secret::secret_string::SecretString`] for any long-lived
//! storage. No bytes typed into a prompt are logged, written to
//! history, or surfaced in `PromptError`'s `Display` output.

// **PLATFORM:** unix (Linux + macOS)
// **GATING:** #[cfg(unix)] / #[cfg(not(unix))].

use std::io::{self, Write};

/// Read a plain (visible-echo) line from stdin. Use this for non-secret
/// interactive prompts such as paths, IDs, names, URLs, and email addresses.
/// Reserve [`SecretPrompt::read_secret`] / [`SecretPrompt::read_masked`] for
/// passwords, tokens, and passphrase inputs.
///
/// # Errors
///
/// - [`PromptError::Io`] on any stdin / stdout IO failure.
/// - [`PromptError::Eof`] when stdin closes before any byte is typed.
pub fn prompt_line(label: &str) -> Result<String, PromptError> {
    SecretPrompt::new(label).read_line()
}

/// Errors produced by [`SecretPrompt`] reads.
#[derive(Debug, thiserror::Error)]
pub enum PromptError {
    #[error("prompt IO failed: {0}")]
    Io(#[from] io::Error),
    /// User hit Ctrl-D (or stdin closed) before typing anything.
    /// The caller should treat this as a clean cancellation rather
    /// than an empty answer so that `pcloudc login` exits with
    /// [`crate::exit_code::ExitCode::Auth`] and a `login cancelled
    /// (EOF)` message, not a silent success with an empty username.
    #[error("end of input (Ctrl-D)")]
    Eof,
}

/// Interactive labelled prompt printed to stdout.
///
/// Construct once per field (`username`, `password`, `2FA code`, …)
/// and call the appropriate read method. The struct holds only the
/// prompt label — no typed bytes are kept between calls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretPrompt {
    /// Human-readable label printed before the `:` separator.
    pub label: String,
}

impl SecretPrompt {
    /// Construct a prompt with the given display label.
    #[must_use]
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
        }
    }

    /// Read a line from stdin with normal terminal echo. Returns
    /// [`PromptError::Eof`] on an immediate Ctrl-D so the caller can
    /// distinguish cancellation from an empty answer.
    ///
    /// # Errors
    ///
    /// - [`PromptError::Io`] on any stdin / stdout IO failure.
    /// - [`PromptError::Eof`] when stdin closes before any byte is
    ///   typed.
    pub fn read_line(&self) -> Result<String, PromptError> {
        let mut stdout = io::stdout();
        write!(stdout, "{}: ", self.label)?;
        stdout.flush()?;

        let mut value = String::new();
        let n = io::stdin().read_line(&mut value)?;
        // `read_line` returns Ok(0) at EOF *before* any byte was read.
        // That's Ctrl-D on an empty line — treat it as a distinct
        // cancellation signal rather than an empty string so the login
        // flow doesn't silently proceed with a blank username.
        if n == 0 {
            return Err(PromptError::Eof);
        }
        Ok(value.trim_end().to_owned())
    }

    /// Read a secret from stdin with echo disabled (standard
    /// password prompt). Falls back gracefully on non-TTY stdin.
    ///
    /// # Errors
    ///
    /// [`PromptError::Io`] on any IO failure inside `rpassword`.
    pub fn read_secret(&self) -> Result<String, PromptError> {
        let mut stdout = io::stdout();
        write!(stdout, "{}: ", self.label)?;
        stdout.flush()?;
        Ok(rpassword::read_password()?)
    }

    /// Like [`Self::read_secret`] but echoes `*` for each character typed. Used
    /// for inputs where the user benefits from visual feedback on their
    /// typing (2FA / recovery codes, confirmations) without revealing the
    /// value. Backspace / Ctrl-H erases one star; Ctrl-U kills the line;
    /// Enter terminates; Ctrl-C aborts with
    /// [`PromptError::Io`]`(Interrupted)`.
    ///
    /// The termios RAII guard (see module docs) guarantees the user's
    /// shell is restored to canonical / echo mode on every exit
    /// path, including panics.
    ///
    /// Falls back to [`Self::read_secret`] when stdin is not a TTY (piped input)
    /// so tests and scripted flows remain unchanged.
    ///
    /// # Errors
    ///
    /// - [`PromptError::Io`] for terminal / read failures, or when
    ///   the user hits Ctrl-C during the masked read.
    pub fn read_masked(&self) -> Result<String, PromptError> {
        let mut stdout = io::stdout();
        write!(stdout, "{}: ", self.label)?;
        stdout.flush()?;

        if !is_stdin_tty() {
            // No controlling terminal (pipe, tests). Drop through to the
            // plain rpassword read which handles non-TTY input gracefully.
            let value = rpassword::read_password()?;
            return Ok(value);
        }

        masked_tty_read()
    }
}

#[cfg(unix)]
fn is_stdin_tty() -> bool {
    // SAFETY: `isatty(fd)` is a safe POSIX syscall with no preconditions
    // beyond passing a valid fd; 0 (STDIN) is always valid for a running
    // process. It sets errno but does not read or write user memory.
    // Available on both Linux and macOS (POSIX.1).
    unsafe { libc::isatty(0) == 1 }
}

#[cfg(not(unix))]
fn is_stdin_tty() -> bool {
    false
}

#[cfg(unix)]
fn masked_tty_read() -> Result<String, PromptError> {
    use std::io::Read;
    use std::mem::MaybeUninit;

    // Save current termios so we can restore on return.
    // SAFETY: `tcgetattr` writes into `termios` which we own via
    // MaybeUninit. Checking the return code is sufficient.
    let fd = 0;
    let mut original: MaybeUninit<libc::termios> = MaybeUninit::uninit();
    if unsafe { libc::tcgetattr(fd, original.as_mut_ptr()) } != 0 {
        // Fall back to plain read when we can't get termios (pipe-ish).
        return Ok(rpassword::read_password()?);
    }
    // SAFETY: `tcgetattr` returned success, so libc fully initialized the
    // `termios` value at `original`.
    let original = unsafe { original.assume_init() };

    // Build a raw-ish variant: disable canonical mode + echo.
    let mut raw = original;
    raw.c_lflag &= !(libc::ICANON | libc::ECHO);
    raw.c_cc[libc::VMIN] = 1;
    raw.c_cc[libc::VTIME] = 0;
    // SAFETY: `raw` is a valid `termios` value copied from `tcgetattr`; the
    // pointer is valid for the duration of this syscall and fd 0 is stdin.
    if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &raw) } != 0 {
        return Ok(rpassword::read_password()?);
    }

    // RAII guard to restore termios on any exit path, including panics.
    struct Restore {
        original: libc::termios,
    }
    impl Drop for Restore {
        fn drop(&mut self) {
            // SAFETY: `self.original` is a valid `libc::termios` saved via
            // `tcgetattr` earlier in the same function. `fd 0` (stdin) is
            // open for the duration of the process. `tcsetattr` does not
            // read or write any Rust memory beyond `self.original`.
            unsafe {
                let _ = libc::tcsetattr(0, libc::TCSANOW, &self.original);
            }
        }
    }
    let _guard = Restore { original };

    let mut stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut buf = [0u8; 1];
    let mut value = String::new();

    loop {
        if stdin.read(&mut buf)? == 0 {
            break; // EOF
        }
        match buf[0] {
            // Enter / newline terminates input.
            b'\r' | b'\n' => {
                writeln!(stdout)?;
                break;
            }
            // Backspace (DEL 0x7F) or Ctrl-H (0x08): erase one star.
            0x7F | 0x08 => {
                if value.pop().is_some() {
                    // Move cursor back, overwrite with space, back again.
                    write!(stdout, "\x08 \x08")?;
                    stdout.flush()?;
                }
            }
            // Ctrl-U: kill line.
            0x15 => {
                while value.pop().is_some() {
                    write!(stdout, "\x08 \x08")?;
                }
                stdout.flush()?;
            }
            // Ctrl-C: abort with an error so the caller surfaces it as a
            // cancellation.
            0x03 => {
                writeln!(stdout)?;
                return Err(PromptError::Io(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "user interrupt (Ctrl-C)",
                )));
            }
            // Ignore other control chars; only append printable ASCII.
            b if (0x20..=0x7E).contains(&b) => {
                value.push(b as char);
                write!(stdout, "*")?;
                stdout.flush()?;
            }
            _ => { /* swallow non-printable */ }
        }
    }
    Ok(value)
}

#[cfg(not(unix))]
fn masked_tty_read() -> Result<String, PromptError> {
    Ok(rpassword::read_password()?)
}
