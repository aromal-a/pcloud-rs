//! Shell completion generation.
//!
//! We expose a `completion` subcommand that emits a shell completion script
//! for the requested shell using `clap_complete`. This does NOT replace the
//! legacy token parser — we build a parallel, descriptive `clap::Command`
//! tree that mirrors the documented subcommand list so shells get sensible
//! tab-completion.
//!
//! The completion script is printed to stdout. Typical use:
//!
//! ```text
//! pcloudc completion bash   > /etc/bash_completion.d/pcloudc
//! pcloudc completion zsh    > ~/.zfunc/_pcloudc
//! pcloudc completion fish   > ~/.config/fish/completions/pcloudc.fish
//! ```

// **PLATFORM:** all
// **GATING:** none (portable).

use std::io::Write;

use clap::{Arg, ArgAction, Command};
use clap_complete::{Shell, generate};

use crate::exit_code::EXIT_CODE_HELP;

/// Binary name used in generated completion scripts and version output.
pub const BIN_NAME: &str = "pcloudc";

/// Build the [`clap::Command`] tree that drives completion generation. This
/// mirrors the documented subcommand list in the hand-written help text so
/// users get consistent tab-completion, without actually routing commands
/// through clap at runtime.
#[must_use]
pub fn build_cli() -> Command {
    let sub = |name: &'static str, about: &'static str| Command::new(name).about(about);

    Command::new(BIN_NAME)
        .about("pCloud enterprise CLI")
        .after_help(EXIT_CODE_HELP)
        .disable_help_subcommand(true)
        .arg(
            Arg::new("json")
                .long("json")
                .action(ArgAction::SetTrue)
                .global(true)
                .help("Emit machine-readable JSON output"),
        )
        .arg(
            Arg::new("output")
                .long("output")
                .value_parser(["text", "json"])
                .global(true)
                .help("Output format (text or json)"),
        )
        .arg(
            Arg::new("quiet")
                .short('q')
                .long("quiet")
                .action(ArgAction::SetTrue)
                .global(true)
                .help("Suppress non-error stdout"),
        )
        .arg(
            Arg::new("verbose")
                .short('v')
                .long("verbose")
                .action(ArgAction::Count)
                .global(true)
                .help("Increase verbosity (-v, -vv, -vvv)"),
        )
        .subcommand(sub("status", "Show current sync state"))
        .subcommand(sub("health", "Daemon health check"))
        .subcommand(sub("pending", "Check for pending transfers"))
        .subcommand(sub("userinfo", "Show authenticated user info"))
        .subcommand(sub("pause", "Pause syncing"))
        .subcommand(sub("resume", "Resume syncing"))
        .subcommand(
            Command::new("sync")
                .about("Sync folder management")
                .subcommand(sub("list", "List sync folders"))
                .subcommand(sub("add", "Add a sync folder"))
                .subcommand(sub("remove", "Remove a sync folder"))
                // M-8.1: sync subcommands previously missing from completion tree.
                .subcommand(
                    sub(
                        "change-type",
                        "Change sync type for a registered sync folder",
                    )
                    .arg(
                        Arg::new("local-path")
                            .required(true)
                            .help("Local path of the existing sync root"),
                    )
                    .arg(
                        Arg::new("sync-type")
                            .required(true)
                            .value_parser(["two-way", "upload-only", "download-only"])
                            .help("New sync type"),
                    ),
                )
                .subcommand(
                    sub("localscan", "Force an immediate local-filesystem scan").arg(
                        Arg::new("local-path")
                            .required(false)
                            .help("Restrict scan to this local path (default: all roots)"),
                    ),
                )
                .subcommand(
                    sub("suggest", "Suggest folders suitable for syncing").arg(
                        Arg::new("limit")
                            .long("limit")
                            .value_parser(clap::value_parser!(u32))
                            .help("Maximum number of suggestions to return"),
                    ),
                )
                .subcommand(
                    sub(
                        "is-syncable",
                        "Check whether a local path can be added as a sync root",
                    )
                    .arg(
                        Arg::new("local-path")
                            .required(true)
                            .help("Local path to classify"),
                    ),
                )
                .subcommand(sub(
                    "status",
                    "Show current sync engine status and queue depth",
                )),
        )
        .subcommand(
            Command::new("crypto")
                .about("Crypto folder controls")
                .subcommand(sub("start", "Unlock crypto folder"))
                .subcommand(sub("stop", "Lock crypto folder"))
                .subcommand(sub("status", "Show crypto backend and lifecycle"))
                .subcommand(sub(
                    "reset",
                    "Wipe local crypto fingerprint and folder registry",
                ))
                .subcommand(sub(
                    "priv-key-flags",
                    "Return current crypto private-key flags",
                ))
                .subcommand(sub(
                    "send-change-private",
                    "Request server-side code to authorise a crypto password rotation",
                ))
                .subcommand(
                    sub("change-password", "Rotate the crypto passphrase").arg(
                        Arg::new("password-stdin")
                            .long("password-stdin")
                            .action(ArgAction::SetTrue)
                            .help("Read old crypto password from stdin"),
                    ),
                )
                .subcommand(
                    sub(
                        "change-password-unlocked",
                        "Rotate the crypto passphrase when the shell is already unlocked",
                    )
                    .arg(
                        Arg::new("password-stdin")
                            .long("password-stdin")
                            .action(ArgAction::SetTrue)
                            .help("Read new crypto password from stdin"),
                    ),
                )
                .subcommand(sub("hint", "Fetch the stored crypto passphrase hint"))
                .subcommand(
                    Command::new("setup")
                        .about(
                            "Set up the crypto profile. Choose a backend: pclsync-compat \
                             (default, interoperable with official pCloud apps) or enhanced \
                             (AES-256-GCM + Argon2id, NOT interoperable with any pCloud app).",
                        )
                        .arg(
                            clap::Arg::new("backend")
                                .long("backend")
                                .value_parser(["pclsync-compat", "enhanced"])
                                .help(
                                    "Crypto backend. 'enhanced' requires \
                                     --acknowledge-not-interop and locks you out of the official \
                                     pCloud apps.",
                                ),
                        )
                        .arg(
                            clap::Arg::new("acknowledge-not-interop")
                                .long("acknowledge-not-interop")
                                .num_args(0)
                                .help(
                                    "Acknowledge that --backend enhanced produces files that will \
                                     NOT open in the official pCloud apps.",
                                ),
                        )
                        .arg(
                            clap::Arg::new("hint")
                                .long("hint")
                                .help("Optional passphrase hint stored with the crypto metadata."),
                        ),
                )
                .subcommand(
                    // M-8.2: add positional folder-id and password-source flags.
                    sub(
                        "get-folder-key",
                        "Fetch + cache a folder's wrapped sym-key (debugging helper). \
                         SENSITIVE: output contains wrapped key material; \
                         do not log or share the response.",
                    )
                    .arg(
                        Arg::new("folder-id")
                            .required(true)
                            .value_parser(clap::value_parser!(u64))
                            .help("Numeric pCloud folder ID"),
                    )
                    .arg(
                        Arg::new("root")
                            .long("root")
                            .action(ArgAction::SetTrue)
                            .help("Treat the target as a crypto root folder"),
                    )
                    .arg(
                        Arg::new("password-stdin")
                            .long("password-stdin")
                            .action(ArgAction::SetTrue)
                            .help("Read crypto password from stdin"),
                    )
                    .arg(
                        Arg::new("password-env")
                            .long("password-env")
                            .value_name("VAR")
                            .help("Read crypto password from environment variable VAR"),
                    )
                    .arg(
                        Arg::new("allow-argv-password")
                            .long("allow-argv-password")
                            .value_name("PASSWORD")
                            .help(
                                "Supply password as a CLI argument. \
                                 WARNING: password is visible in /proc/self/cmdline and \
                                 shell history on Linux. Use --password-stdin for production.",
                            ),
                    ),
                )
                .subcommand(
                    // M-8.2: add positional file-id and password-source flags.
                    sub(
                        "get-file-key",
                        "Fetch + cache a file's wrapped sym-key (debugging helper). \
                         SENSITIVE: output contains wrapped key material; \
                         do not log or share the response.",
                    )
                    .arg(
                        Arg::new("file-id")
                            .required(true)
                            .value_parser(clap::value_parser!(u64))
                            .help("Numeric pCloud file ID"),
                    )
                    .arg(
                        Arg::new("root")
                            .long("root")
                            .action(ArgAction::SetTrue)
                            .help("Treat the target as a crypto root entry"),
                    )
                    .arg(
                        Arg::new("password-stdin")
                            .long("password-stdin")
                            .action(ArgAction::SetTrue)
                            .help("Read crypto password from stdin"),
                    )
                    .arg(
                        Arg::new("password-env")
                            .long("password-env")
                            .value_name("VAR")
                            .help("Read crypto password from environment variable VAR"),
                    )
                    .arg(
                        Arg::new("allow-argv-password")
                            .long("allow-argv-password")
                            .value_name("PASSWORD")
                            .help(
                                "Supply password as a CLI argument. \
                                 WARNING: password is visible in /proc/self/cmdline and \
                                 shell history on Linux. Use --password-stdin for production.",
                            ),
                    ),
                ),
        )
        .subcommand(
            Command::new("notifications")
                .about("Notification helpers")
                .subcommand(sub("list", "List account notifications"))
                .subcommand(sub("mark-read", "Mark notifications up to id as read")),
        )
        .subcommand(
            Command::new("session")
                .about("Session lifecycle helpers")
                .subcommand(sub("status", "Show session lifecycle details")),
        )
        .subcommand(
            Command::new("audit")
                .about("Audit-chain helpers")
                .subcommand(sub("verify", "Verify the tamper-evident audit chain")),
        )
        .subcommand(
            Command::new("publink")
                .about("Public-link helpers")
                .subcommand(sub("send", "Mail an existing public link to recipients")),
        )
        .subcommand(
            Command::new("folder")
                .about("Remote folder helpers")
                .subcommand(sub("create", "Create a remote folder by absolute path"))
                .subcommand(sub("id", "Resolve a remote folder path to its folder id"))
                .subcommand(sub("flags", "Read remote folder flags"))
                .subcommand(sub("owner", "Read remote folder owner id")),
        )
        .subcommand(
            Command::new("fs")
                .about("Filesystem helpers")
                .subcommand(sub("status", "Classify a local filesystem path")),
        )
        .subcommand(sub("login", "Begin interactive login"))
        .subcommand(sub("logout", "Logout and clear session"))
        .subcommand(sub("send-tfa-sms", "Resend 2FA SMS"))
        .subcommand(sub("send-tfa-notification", "Resend 2FA notification"))
        .subcommand(sub("submit-password", "Submit username + password"))
        .subcommand(sub("submit-auth", "Submit auth token"))
        .subcommand(sub("submit-tfa", "Submit 2FA code"))
        .subcommand(sub("submit-recovery", "Submit recovery code"))
        .subcommand(sub("unlock-crypto", "Unlock crypto"))
        .subcommand(sub("lock-crypto", "Lock crypto"))
        .subcommand(sub("authsave", "Toggle persistent auth"))
        .subcommand(sub("list-links", "List public links"))
        .subcommand(sub("list-upload-links", "List upload links"))
        .subcommand(sub("show-link", "Show a public link"))
        .subcommand(sub("delete-link", "Delete a public link"))
        .subcommand(sub("create-file-link", "Create file public link"))
        .subcommand(sub("create-folder-link", "Create folder public link"))
        .subcommand(sub("change-link-expire", "Change public link expiry"))
        .subcommand(sub("change-link-password", "Change public link password"))
        .subcommand(sub(
            "change-link-upload",
            "Change public link upload policy",
        ))
        .subcommand(sub("create-upload-link", "Create upload link"))
        .subcommand(sub("delete-upload-link", "Delete upload link"))
        .subcommand(sub("create-tree-link", "Create tree public link"))
        .subcommand(sub(
            "create-tree-link-from-paths",
            "Create a tree public link by resolving pCloud-drive paths daemon-side",
        ))
        .subcommand(sub("list-link-access", "List public link access"))
        .subcommand(sub("add-link-access", "Add public link access"))
        .subcommand(sub("remove-link-access", "Remove public link access"))
        .subcommand(sub("list-bookmarks", "List bookmarks"))
        .subcommand(sub("remove-bookmark", "Remove a bookmark"))
        .subcommand(sub("change-bookmark", "Modify a bookmark"))
        .subcommand(sub("list-incoming-shares", "List incoming shares"))
        .subcommand(sub("list-outgoing-shares", "List outgoing shares"))
        .subcommand(sub(
            "list-incoming-share-requests",
            "List incoming share requests",
        ))
        .subcommand(sub(
            "list-outgoing-share-requests",
            "List outgoing share requests",
        ))
        .subcommand(sub("list-contacts", "List contacts"))
        .subcommand(sub("list-myteams", "List my teams"))
        .subcommand(sub("share-folder", "Share a folder"))
        .subcommand(sub("cancel-share-request", "Cancel share request"))
        .subcommand(sub("decline-share-request", "Decline share request"))
        .subcommand(sub("accept-share-request", "Accept share request"))
        .subcommand(sub("remove-share", "Remove share"))
        .subcommand(sub("modify-share", "Modify share"))
        .subcommand(sub("account-stopshare", "Account-level stop share"))
        .subcommand(sub("account-modifyshare", "Account-level modify share"))
        .subcommand(sub("account-teamshare", "Account-level team share"))
        .subcommand(sub("stat", "Stat an absolute pCloud-drive path"))
        .subcommand(sub("reload", "Send SIGHUP to daemon for hot-reload"))
        .subcommand(sub(
            "drain",
            "Graceful-drain: flush pending uploads, then stop",
        ))
        .subcommand(sub(
            "slo",
            "Fetch canonical Service-Level Objective snapshot",
        ))
        .subcommand(
            Command::new("integrity")
                .about("Background integrity sweeper controls")
                .subcommand(sub("status", "Fetch sweeper progress JSON"))
                .subcommand(sub("run-once", "Trigger one sweeper cycle synchronously"))
                .subcommand(sub(
                    "skip",
                    "Append a glob pattern to the sweeper skip list",
                )),
        )
        .subcommand(
            Command::new("ha")
                .about("High-availability posture helpers")
                .subcommand(sub("status", "Return the Tier-2 HA posture")),
        )
        .subcommand(
            Command::new("audit-verifier")
                .about("Scheduled audit-verifier helpers")
                .subcommand(sub("status", "Return scheduled audit-verifier status")),
        )
        .subcommand(
            Command::new("upload")
                .about("Resumable upload session management")
                .subcommand(
                    sub("create", "Create a new upload session").arg(
                        Arg::new("local-path")
                            .required(true)
                            .help("Local file to upload"),
                    ),
                )
                .subcommand(sub("pause", "Pause an in-progress upload session"))
                .subcommand(sub("resume", "Resume a paused upload session"))
                .subcommand(sub("cancel", "Cancel and discard an upload session"))
                .subcommand(sub("list", "List active upload sessions")),
        )
        .subcommand(
            Command::new("conflict")
                .about("Sync conflict resolution helpers")
                .subcommand(sub("list", "List unresolved sync conflicts"))
                .subcommand(
                    sub("resolve", "Resolve a conflict").arg(
                        Arg::new("path")
                            .required(true)
                            .help("Conflicting local path"),
                    ),
                ),
        )
        .subcommand(
            Command::new("snapshot")
                .about("Daemon state snapshot helpers")
                .subcommand(
                    sub("create", "Create a GPG-encrypted snapshot")
                        .arg(Arg::new("path").required(true).help("Output path")),
                )
                .subcommand(
                    sub("restore", "Restore a snapshot")
                        .arg(Arg::new("path").required(true).help("Snapshot path")),
                )
                .subcommand(
                    sub("verify", "Verify a snapshot")
                        .arg(Arg::new("path").required(true).help("Snapshot path")),
                )
                .subcommand(
                    sub("prune", "Prune old snapshots in a directory").arg(
                        Arg::new("dir")
                            .required(true)
                            .help("Directory containing snapshots"),
                    ),
                ),
        )
        .subcommand(
            Command::new("verify")
                .about("Verify local sync state")
                .arg(Arg::new("path").required(true).help("Local path to verify"))
                .arg(
                    Arg::new("recursive")
                        .long("recursive")
                        .action(ArgAction::SetTrue)
                        .help("Recurse into subdirectories"),
                )
                .arg(
                    Arg::new("fix")
                        .long("fix")
                        .action(ArgAction::SetTrue)
                        .help("Attempt to repair inconsistencies"),
                ),
        )
        .subcommand(Command::new("migrate-from-c").about("Migrate state from the legacy C client"))
        .subcommand(
            Command::new("backup")
                .about("Backup and device helpers")
                .subcommand(sub("create", "Register a new backup"))
                .subcommand(sub("delete", "Delete a backup registration"))
                .subcommand(sub("stop-device", "Stop a backup device"))
                .subcommand(sub("delete-device", "Delete backup device local state"))
                .subcommand(
                    Command::new("snapshot-create").about("Create a GPG-encrypted backup snapshot"),
                )
                .subcommand(Command::new("snapshot-restore").about("Restore a backup snapshot"))
                .subcommand(Command::new("snapshot-verify").about("Verify a backup snapshot"))
                .subcommand(Command::new("snapshot-prune").about("Prune old backup snapshots")),
        )
        .subcommand(
            Command::new("download")
                .about("File download helpers")
                .subcommand(
                    sub("link", "Resolve the download URL for a remote file").arg(
                        Arg::new("file-id")
                            .required(true)
                            .value_parser(clap::value_parser!(u64))
                            .help("Numeric pCloud file ID"),
                    ),
                )
                .subcommand(
                    sub("file", "Download a remote file to a local path")
                        .arg(
                            Arg::new("file-id")
                                .required(true)
                                .value_parser(clap::value_parser!(u64))
                                .help("Numeric pCloud file ID"),
                        )
                        .arg(
                            Arg::new("local-path")
                                .required(true)
                                .help("Local destination path"),
                        ),
                ),
        )
        .subcommand(
            Command::new("account")
                .about("Account and auth management")
                .subcommand(sub(
                    "verify-email",
                    "Send a verification email for the active session",
                ))
                .subcommand(
                    sub(
                        "verify-email-restricted",
                        "Verify email via a restricted verify-token",
                    )
                    .arg(Arg::new("token").required(true).help("Verification token")),
                )
                .subcommand(
                    sub("lost-password", "Send a password-reset email").arg(
                        Arg::new("email")
                            .required(true)
                            .help("Account email address"),
                    ),
                )
                .subcommand(sub("change-password", "Change the account password"))
                .subcommand(
                    sub("register", "Register a new pCloud account")
                        .arg(
                            Arg::new("email")
                                .required(true)
                                .help("Email address for the new account"),
                        )
                        .arg(
                            Arg::new("accept-terms")
                                .long("accept-terms")
                                .action(ArgAction::SetTrue)
                                .help("Accept the pCloud Terms of Service"),
                        ),
                )
                .subcommand(sub(
                    "api-servers",
                    "List available pCloud API server regions",
                ))
                .subcommand(
                    sub("set-api-server", "Pin the daemon to a specific API region")
                        .arg(
                            Arg::new("location-id")
                                .required(true)
                                .help("Region location ID"),
                        )
                        .arg(
                            Arg::new("binapi")
                                .required(true)
                                .help("Binary API endpoint URL"),
                        ),
                )
                .subcommand(
                    sub("set-language", "Set the account language preference").arg(
                        Arg::new("lang")
                            .required(true)
                            .help("BCP-47 language tag (e.g. en, de, fr)"),
                    ),
                )
                .subcommand(sub("promo", "Fetch the promotional URL for this platform")),
        )
        .subcommand(
            sub(
                "log",
                "Show git-log-style revision history for a synced file",
            )
            .arg(
                Arg::new("path")
                    .required(true)
                    .help("Absolute pCloud-drive path"),
            )
            .arg(
                Arg::new("limit")
                    .long("limit")
                    .value_parser(clap::value_parser!(u32))
                    .help("Maximum number of revisions to return"),
            ),
        )
        // ncx.65: `diff` and `restore` are CLI-side stubs that always
        // exit `Unavailable` (pCloud's public API does not expose the
        // revision-diff / restore endpoints to third-party clients).
        // Hide them from tab-completion so operators do not discover a
        // command that cannot do what its name suggests. The handlers
        // remain wired in `app.rs` so existing scripts that call them
        // still receive the structured `Unavailable` response and the
        // tracker pointer, rather than a silent "unknown command"
        // surprise. Remove `.hide(true)` (and update docs) when the
        // upstream API exposes a revision API — follow-up tracked as
        // `pcloud-rs-07o`.
        .subcommand(
            sub(
                "diff",
                "Show diff between two file revisions (stub — Unavailable)",
            )
            .hide(true)
            .arg(
                Arg::new("path")
                    .required(true)
                    .help("Absolute pCloud-drive path"),
            )
            .arg(Arg::new("rev-a").required(true).help("First revision ID"))
            .arg(Arg::new("rev-b").required(true).help("Second revision ID")),
        )
        .subcommand(
            sub(
                "restore",
                "Restore a file to a specific revision (stub — Unavailable)",
            )
            .hide(true)
            .arg(
                Arg::new("path")
                    .required(true)
                    .help("Absolute pCloud-drive path"),
            )
            .arg(Arg::new("rev").required(true).help("Target revision ID")),
        )
        .subcommand(sub("mount", "Mount the pCloud filesystem"))
        .subcommand(sub("unmount", "Unmount the pCloud filesystem"))
        .subcommand(sub("start", "Spawn pcloudd in the background"))
        .subcommand(sub("finalize", "Stop daemon and exit"))
        .subcommand(sub("doctor", "Run self-diagnostic checks"))
        .subcommand(
            Command::new("completion")
                .about("Generate shell completion script")
                .arg(
                    Arg::new("shell")
                        .required(true)
                        .value_parser(clap::value_parser!(Shell))
                        .help("Target shell"),
                ),
        )
}

/// Write a completion script for the requested shell to `out`.
pub fn generate_completion<W: Write>(shell: Shell, out: &mut W) {
    let mut cmd = build_cli();
    generate(shell, &mut cmd, BIN_NAME, out);
}

/// Parse a shell name from a user-provided string. Returns `None` for unknown
/// values; callers should surface a usage error.
#[must_use]
pub fn parse_shell(name: &str) -> Option<Shell> {
    match name.to_ascii_lowercase().as_str() {
        "bash" => Some(Shell::Bash),
        "zsh" => Some(Shell::Zsh),
        "fish" => Some(Shell::Fish),
        "elvish" => Some(Shell::Elvish),
        "powershell" | "pwsh" => Some(Shell::PowerShell),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gen_script(shell: Shell) -> String {
        let mut buf: Vec<u8> = Vec::new();
        generate_completion(shell, &mut buf);
        String::from_utf8(buf).expect("completion output must be UTF-8")
    }

    #[test]
    fn build_cli_has_expected_top_level_subcommands() {
        let cmd = build_cli();
        let names: Vec<&str> = cmd.get_subcommands().map(Command::get_name).collect();
        for expected in [
            "status",
            "health",
            "sync",
            "crypto",
            "completion",
            "finalize",
            "stat",
            "reload",
            "drain",
            "slo",
            "integrity",
            "ha",
            "audit-verifier",
            "upload",
            "conflict",
            "snapshot",
            "verify",
            "migrate-from-c",
            "backup",
        ] {
            assert!(names.contains(&expected), "missing subcommand: {expected}");
        }
    }

    #[test]
    fn bash_completion_non_empty() {
        let s = gen_script(Shell::Bash);
        assert!(!s.is_empty());
        assert!(s.contains("pcloudc"));
    }

    #[test]
    fn zsh_completion_non_empty() {
        let s = gen_script(Shell::Zsh);
        assert!(!s.is_empty());
        assert!(s.contains("pcloudc"));
    }

    #[test]
    fn fish_completion_non_empty() {
        let s = gen_script(Shell::Fish);
        assert!(!s.is_empty());
        assert!(s.contains("pcloudc"));
    }

    #[test]
    fn elvish_completion_non_empty() {
        let s = gen_script(Shell::Elvish);
        assert!(!s.is_empty());
    }

    #[test]
    fn powershell_completion_non_empty() {
        let s = gen_script(Shell::PowerShell);
        assert!(!s.is_empty());
        assert!(s.contains("pcloudc"));
    }

    #[test]
    fn parse_shell_names() {
        assert_eq!(parse_shell("bash"), Some(Shell::Bash));
        assert_eq!(parse_shell("ZSH"), Some(Shell::Zsh));
        assert_eq!(parse_shell("fish"), Some(Shell::Fish));
        assert_eq!(parse_shell("elvish"), Some(Shell::Elvish));
        assert_eq!(parse_shell("powershell"), Some(Shell::PowerShell));
        assert_eq!(parse_shell("pwsh"), Some(Shell::PowerShell));
        assert_eq!(parse_shell("nu"), None);
    }
}
