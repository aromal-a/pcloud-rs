// **PLATFORM:** Linux
// **GATING:** none (portable; uses Linux-only idioms — see TODO(bd-xplat)).

use crate::commands::{Command, SecretInputs};
use crate::prompt::{prompt_line, PromptError, SecretPrompt};
use pcloud_model::public_links::PublicLinkUploadPolicy;
use pcloud_secret::secret_string::SecretString;
use thiserror::Error;

#[must_use]
pub fn banner() -> &'static str {
    "pcloud-cli foundation ready"
}

#[must_use]
pub fn help_text() -> &'static str {
    concat!(
        "pcloudc(1) — pCloud command-line client\n",
        "================================================================\n",
        "\n",
        "NAME\n",
        "    pcloudc — command-line client that talks to a running pcloudd daemon\n",
        // TODO(bd-xplat): Linux-only — needs cfg gate or platform trait abstraction. See PLAN_CROSSPLATFORM.md §2.
        "    over a 0600-protected Unix socket (`SO_PEERCRED` authenticated).\n",
        "    Each invocation is a one-shot RPC: it connects, sends one request,\n",
        "    prints the reply, exits. State lives in the daemon — never in the\n",
        "    CLI process.\n",
        "\n",
        "SYNOPSIS\n",
        "    pcloudc [GLOBAL-OPTIONS] <command> [COMMAND-ARGS ...]\n",
        "    pcloudc [GLOBAL-OPTIONS] login [LOGIN-OPTIONS]\n",
        "    pcloudc [GLOBAL-OPTIONS] completion <shell>\n",
        "\n",
        "DESCRIPTION\n",
        "    The Rust rewrite splits the legacy monolithic `pcloud-rs` binary into\n",
        "    two cooperating processes:\n",
        "      pcloudd   — long-running daemon: auth, sync engine, transfer queue,\n",
        "                  crypto state, FUSE mount, hash-chained audit log.\n",
        "      pcloudc   — stateless CLI client: what you use every day. Every\n",
        "                  subcommand is a one-shot RPC; the daemon keeps state.\n",
        "    This layout matches the systemd/dockerd/pulseaudio convention and\n",
        "    makes the client safely scriptable (JSON output, deterministic exit\n",
        "    codes, pipe-friendly).\n",
        "\n",
        "GLOBAL OPTIONS\n",
        "    -h, --help              Print this help and exit (also: `help` / `?`).\n",
        "    -V, --version           Print version and exit.\n",
        "    --output <text|json>    Select output format. Default: text.\n",
        "    --json                  Shortcut for `--output json`.\n",
        "                            In JSON mode every response is a\n",
        "                            `{kind,command,status,message,exit_code}`\n",
        "                            envelope; errors go to stdout (pipe-safe).\n",
        "    -q, --quiet             Silence stdout/stderr on success. Only the\n",
        "                            exit code is surfaced. Overrides -v.\n",
        "    -v, -vv, -vvv           Raise verbosity by one level each. Default:\n",
        "                              v=0  just the response payload (e.g. the\n",
        "                                   status string). No prefix, no banner.\n",
        "                              v=1  add `[Command Status]` prefix and echo\n",
        "                                   daemon sub-events during `login`.\n",
        "                              v=2  add the daemon banner line and tracing-\n",
        "                                   level info.\n",
        "                              v=3  maximum (tracing `debug`/`trace`).\n",
        "    --dbg, --debug          Alias for `-vvv` (jump straight to max).\n",
        "    --verbose               Same as `-v`.\n",
        "\n",
        "    USAGE STYLE: legacy two-word forms (e.g. `sync list`, `crypto start`)\n",
        "    and canonical hyphenated forms (e.g. `sync-list`, `unlock-crypto`) are\n",
        "    both accepted. Prefer hyphenated forms in new scripts — they parse\n",
        "    without ambiguity when combined with flags. The `(?)` and `(st)`\n",
        "    shortcuts mirror legacy `pcloud-rs` muscle memory.\n",
        "\n",
        "────────────────────────────────────────────────────────────────\n",
        "DAEMON LIFECYCLE\n",
        "────────────────────────────────────────────────────────────────\n",
        "    start                  Spawn pcloudd in the background if it isn't\n",
        "                           already running. Idempotent: if the socket\n",
        "                           responds, prints `already running` and exits\n",
        "                           0. On spawn, exports env vars computed from\n",
        "                           ~/.pcloud/config.toml (PCLOUD_CACHE_SIZE_GB,\n",
        "                           PCLOUD_DEFAULT_MOUNTPOINT, PCLOUD_LOG_PATH,\n",
        "                           PCLOUD_FS_EVENT_LOG, PCLOUD_LOG_LEVEL,\n",
        "                           PCLOUD_FUSE_OPTS) so config-only settings\n",
        "                           actually reach the daemon. Detaches via\n",
        "                           setsid so the daemon survives CLI exit. Log\n",
        "                           stream goes to ~/.pcloud/state/daemon.log.\n",
        "    stop                   Graceful shutdown: flushes pending uploads,\n",
        "                           closes the FUSE session if mounted, tears\n",
        "                           down the socket, exits the daemon. Synonym\n",
        "                           for `finalize`.\n",
        "    finalize, f            Legacy alias for `stop`.\n",
        "    reload                 Send SIGHUP to the daemon to hot-reload\n",
        "                           config (log level, rate limits, sweeper\n",
        "                           schedule, sync poll, data-residency).\n",
        "    health                 Multi-line health snapshot: schema version,\n",
        "                           integrity check, sync state, crypto state,\n",
        "                           engine counters, transfer metrics.\n",
        "    status (st)            One-line operational summary. Good for\n",
        "                           scripts; the single `auth=…` field tells you\n",
        "                           whether login worked.\n",
        "    pending, p             Queued + in-flight + completed transfer\n",
        "                           counters. Returns zeros when idle.\n",
        "\n",
        "────────────────────────────────────────────────────────────────\n",
        "AUTHENTICATION\n",
        "────────────────────────────────────────────────────────────────\n",
        "    login [LOGIN-OPTIONS]  Interactive mini-REPL that chains prompts:\n",
        "                             Username → Password → (if 2FA required)\n",
        "                             SMS + push auto-fire → 2FA code → success.\n",
        "                           Any prompt whose value is supplied via a\n",
        "                           flag is silently skipped. Post-success\n",
        "                           actions (in order): optional token-vault\n",
        "                           enable, optional crypto unlock, optional\n",
        "                           auto-mount, then print userinfo.\n",
        "\n",
        "    LOGIN OPTIONS (every field is also a key in ~/.pcloud/config.toml;\n",
        "    the flag always wins):\n",
        "      -u, --user, --username <NAME>\n",
        "                           pCloud account email / username. When unset,\n",
        "                           the username from config is used; otherwise\n",
        "                           the REPL prompts for it (with echo).\n",
        "      -c, --crypto         After successful auth, prompt for the crypto\n",
        "                           passphrase (no echo) and unlock the crypto\n",
        "                           folder in the daemon.\n",
        "      -y, --passascrypto   Treat the account password as the crypto\n",
        "                           passphrase (no second prompt). Implies -c.\n",
        "      -r, --trust-device   Request that pCloud remember this device so\n",
        "                           future logins skip the 2FA challenge.\n",
        "      -s, --save-password  Enable the opt-in auth-token vault at\n",
        "                           ~/.pcloud/config/auth_token (mode 0600,\n",
        "                           owner-only). STORES THE TOKEN, NOT THE\n",
        "                           PASSWORD. On next daemon start, the daemon\n",
        "                           reloads the token and auto-authenticates.\n",
        "                           Prints a 2-second-cancellable warning\n",
        "                           before enabling.\n",
        "      -m [<PATH>], --mountpoint [<PATH>]\n",
        "                           After login, mount pCloud Drive. Three\n",
        "                           behaviours:\n",
        "                             -m /path          → mount at /path\n",
        "                             -m (no value)     → use config, else\n",
        "                                                  ~/pCloudDrive\n",
        "                             flag absent       → do not auto-mount\n",
        "                           The target directory is auto-created with\n",
        "                           mode 0700.\n",
        "      -O, --fuse-opts <OPTS>\n",
        "                           FUSE mount options string (e.g.\n",
        "                           'nodev,nosuid'). Persisted to the config\n",
        "                           file; takes effect on next daemon start.\n",
        "                           `allow_other`/`allow_root` are silently\n",
        "                           rejected regardless of this value.\n",
        "      -T, --tfa-channel, --channel <sms|push>\n",
        "                           Restrict the auto-issued 2FA channel on\n",
        "                           challenge. Default: fire both SMS and push.\n",
        "                           -c is already taken by --crypto (matches\n",
        "                           C `pcloud-rs`), hence -T for this flag.\n",
        "      --password-stdin     Read the password from a single line on\n",
        "                           stdin. Invisible to `ps`, no shell history.\n",
        "                           Best for piped-credential helpers.\n",
        "      --password-env <VAR> Read the password from environment variable\n",
        "                           <VAR>, then immediately `unsetenv(VAR)` so\n",
        // TODO(bd-xplat): Linux-only — needs cfg gate or platform trait abstraction. See PLAN_CROSSPLATFORM.md §2.
        "                           /proc/<pid>/environ stops exposing it.\n",
        // M-8.4: surface /proc/self/cmdline leak warning in help text for --allow-argv-password.
        "      --allow-argv-password Acknowledge the security risk of passing a\n",
        "                           password as a command-line argument. The\n",
        "                           password is visible to all processes on the\n",
        "                           host via /proc/self/cmdline (Linux) and\n",
        "                           shell history. Accepted ONLY for backward-\n",
        "                           compatibility with scripts that cannot use\n",
        "                           --password-stdin or --password-env. Prefer\n",
        "                           those flags in all production deployments.\n",
        "      --log-path <PATH>    Config-only: persist the daemon log path.\n",
        "                           Takes effect on next `start`.\n",
        "      --fs-event-log <PATH>\n",
        "                           Config-only: persist the FS-event log path.\n",
        "                           Default disabled.\n",
        "      --log-level <LVL>    Config-only: error | warn | info | debug |\n",
        "                           trace. Applies on next `start`.\n",
        "      --cache-size <GB>    Config-only: local page-cache cap in\n",
        "                           gigabytes (default 5, mirrors C --cache-size).\n",
        "                           Applies on next `start`; if the daemon is\n",
        "                           already running the CLI automatically\n",
        "                           restarts it after draining writes.\n",
        "      --config <PATH>      Use <PATH> as the CLI config file instead of\n",
        "                           the default ~/.pcloud/config.toml. Honours\n",
        "                           $PCLOUD_CONFIG when unset. Auto-created with\n",
        "                           inline-commented defaults on first use.\n",
        "\n",
        "    logout                 Complete disconnect: drain + unmount FUSE,\n",
        "                           lock crypto, clear auth token, wipe token\n",
        "                           vault if enabled. After this, `ls /mnt/...`\n",
        "                           correctly returns ENOENT/ENOTCONN.\n",
        "    authsave <on|off>      Toggle the auth-token vault (opt-in per\n",
        "                           PCLOUD_DURABLE_AUTH_TOKENS and/or -s).\n",
        "    submit-password [USER] [PW]\n",
        "                           One-shot password submission, bypassing the\n",
        "                           `login` REPL. Respects --password-stdin /\n",
        "                           --password-env / interactive prompt per\n",
        "                           priority. Supplying PW on argv triggers a\n",
        "                           stderr warning about /proc/<pid>/cmdline\n",
        "                           visibility.\n",
        "    auth <PASSWORD>        Legacy alias; same warnings apply.\n",
        "    submit-auth [TOKEN]    Submit a long-lived pCloud auth token\n",
        "                           directly, e.g. one recovered from the vault\n",
        "                           on another host. Prompts if TOKEN omitted.\n",
        "    submit-tfa <CODE>      Send a 6-digit 2FA code to the daemon.\n",
        "    tfa <CODE>             Legacy alias.\n",
        "    submit-recovery <CODE> Submit a recovery code (breakglass path).\n",
        "    send-tfa-sms           Ask pCloud to resend the SMS challenge code.\n",
        "    send-tfa-notification  Ask pCloud to push a notification to a\n",
        "                           previously-registered device.\n",
        "    session status         JSON snapshot: `{expires_at, last_used_at,\n",
        "                           refresh_in_flight}`.\n",
        "    userinfo               Print `{user_id, email}` for the active\n",
        "                           session.\n",
        "\n",
        "────────────────────────────────────────────────────────────────\n",
        "FILESYSTEM MOUNT (FUSE)\n",
        "────────────────────────────────────────────────────────────────\n",
        "    mount <PATH>           Mount pCloud Drive at <PATH> (must be an\n",
        "                           existing empty directory, owned by current\n",
        "                           uid, not world-writable). Flags applied:\n",
        "                           rw, nosuid, nodev, default_permissions,\n",
        "                           NEVER allow_other. Write path is journalled\n",
        "                           + crash-safe; reads come through a 64 KiB\n",
        "                           page-cache with LRU+TTL eviction.\n",
        "    unmount                Drain pending writes, flush journal, release\n",
        "                           the kernel session, remove stale entries.\n",
        "    fs status <LOCAL-PATH> Classify <LOCAL-PATH>: INSYNC | INPROG |\n",
        "                           NOSYNC | INVSYNC. Mirrors C\n",
        "                           `psync_filesystem_status`.\n",
        "\n",
        "────────────────────────────────────────────────────────────────\n",
        "SYNC ROOTS (bidirectional cloud ↔ local folder pairing)\n",
        "────────────────────────────────────────────────────────────────\n",
        "    sync (s) list          List configured sync roots (id, local path,\n",
        "                           remote path, type).\n",
        "    sync (s) add <LOCAL> <REMOTE> [--type FLAVOR]\n",
        "                           Register a new sync root. Rejects duplicates\n",
        "                           and nested roots. Canonicalises LOCAL.\n",
        "                           FLAVOR aliases (default = bilateral):\n",
        "                             bilateral|full|both\n",
        "                             mirror|download-only|down|remote-to-local\n",
        "                             upload-only|up|local-to-remote\n",
        "                             backup|backup-archive|archive|keep-remote\n",
        "                           NOTE: `backup` is a deletion-safe archival\n",
        "                           flavor — uploads new/changed local files,\n",
        "                           but a local deletion does NOT delete the\n",
        "                           remote copy. Use `upload-only` if you want\n",
        "                           the old destructive-mirror behaviour.\n",
        "    sync (s) remove <ID>   Remove a sync root by id. Evicts queued work\n",
        "                           and staged bytes under that root.\n",
        "    sync (s) change-type <ID> <FLAVOR>\n",
        "                           Change an existing sync root's direction.\n",
        "                           FLAVOR accepts the same 9 aliases as\n",
        "                           `sync add --type`.\n",
        "    sync (s) localscan     Trigger an immediate local-scan wakeup\n",
        "                           (mirrors C `psync_run_localscan`).\n",
        "    sync-list / sync-add / sync-remove / sync-change-type / sync-localscan\n",
        "                           Canonical single-token forms for scripts.\n",
        "    pause | resume         Global pause/resume of ALL sync workers.\n",
        "\n",
        "────────────────────────────────────────────────────────────────\n",
        "REMOTE FOLDER HELPERS\n",
        "────────────────────────────────────────────────────────────────\n",
        "    folder create <PATH>   Create a remote folder by absolute pCloud\n",
        "                           path (e.g. /Documents/2026). Idempotent.\n",
        "    folder id <PATH>       Resolve PATH → folder_id.\n",
        "    folder flags <PATH>    Read PSYNC_PERM_* bitmap for the folder.\n",
        "    folder owner <PATH>    Read the owner user_id.\n",
        "    stat <REMOTE-PATH>     Stat an absolute pCloud-drive path: returns\n",
        "                           file/folder id, name, parent, size, hash,\n",
        "                           modified, created. Checks local metadata\n",
        "                           cache first, falls back to API.\n",
        "\n",
        "────────────────────────────────────────────────────────────────\n",
        "CRYPTO FOLDER (client-side AES-256-GCM)\n",
        "────────────────────────────────────────────────────────────────\n",
        "    crypto (c) start <PW>  Unlock the crypto folder; if first-run,\n",
        "                           performs setup. The passphrase is held only\n",
        "                           transiently (SecretString, zeroised on drop).\n",
        "    crypto (c) stop        Lock crypto; zeroise active key material.\n",
        "    crypto (c) status      Reports `setup/started/state/folders/hint`.\n",
        "    crypto (c) setup       Set up the crypto profile. Defaults to the\n",
        "                           interop-safe pclsync-compat backend; pass\n",
        "                           `--backend enhanced --acknowledge-not-interop`\n",
        "                           to opt into stricter AES-256-GCM + Argon2id\n",
        "                           crypto (NOT readable by official pCloud apps).\n",
        "                           Without --backend on a tty, an interactive\n",
        "                           picker prompts for a choice.\n",
        "    crypto (c) get-folder-key <FOLDER_ID>\n",
        "                           Fetch + cache a folder's wrapped sym-key.\n",
        "    crypto (c) get-file-key <FILE_ID>\n",
        "                           Fetch + cache a file's wrapped sym-key.\n",
        "    unlock-crypto <PW>     Canonical unlock form.\n",
        "    lock-crypto            Canonical lock form.\n",
        "\n",
        "────────────────────────────────────────────────────────────────\n",
        "PUBLIC LINKS\n",
        "────────────────────────────────────────────────────────────────\n",
        "    list-links                        List existing public links.\n",
        "    list-upload-links                 List upload-only links.\n",
        "    show-link <CODE>                  Reveal link details by code.\n",
        "    delete-link <ID>                  Delete a public link by id.\n",
        "    create-file-link <FILEID>         Create a file share link.\n",
        "    create-folder-link <FOLDERID>     Create a folder share link.\n",
        "    change-link-expire <ID> [EXP]     Set or clear expiration.\n",
        "    change-link-password <ID> [PW]    Set or clear link password.\n",
        "    change-link-upload <ID> <POL>     Upload policy on the link.\n",
        "    create-upload-link ...            Create an upload-only link.\n",
        "    delete-upload-link <ID>           Delete an upload-only link.\n",
        "    create-tree-link <PATHS...>       Selective tree link (mixed files).\n",
        "    create-tree-link-from-paths <NAME> <PCLOUD-PATH>...\n",
        "                                      Create a tree public link by resolving\n",
        "                                      one or more absolute pCloud-drive paths\n",
        "                                      to ids on the daemon (authenticated path\n",
        "                                      resolver). At least one path required.\n",
        "    list-link-access <ID>             List upload-link access grants.\n",
        "    add-link-access / remove-link-access <ID> <EMAIL>\n",
        "                                      Manage upload-link access list.\n",
        "    list-bookmarks                    List bookmarks / pinned shares.\n",
        "    remove-bookmark <ID>              Remove bookmark.\n",
        "    change-bookmark <ID> ...          Update bookmark metadata.\n",
        "    publink send <CODE> --to <EMAILS> [--message <TEXT>]\n",
        "                                      Email an existing link. Emails\n",
        "                                      are redacted from audit (PII).\n",
        "\n",
        "────────────────────────────────────────────────────────────────\n",
        "SHARES, CONTACTS, TEAMS\n",
        "────────────────────────────────────────────────────────────────\n",
        "    list-incoming-shares / list-outgoing-shares\n",
        "                                      Shares on which you are recipient\n",
        "                                      / owner respectively.\n",
        "    list-incoming-share-requests / list-outgoing-share-requests\n",
        "                                      Pending invitations.\n",
        "    share-folder <FID> <EMAIL> <PERMS>\n",
        "                                      Invite EMAIL to share folder FID.\n",
        "    accept-share-request / decline-share-request / cancel-share-request <ID>\n",
        "                                      Handle invitations.\n",
        "    remove-share <ID> / modify-share <ID> <PERMS>\n",
        "                                      Revoke or change perms.\n",
        "    account-stopshare / account-modifyshare / account-teamshare\n",
        "                                      Business-account share operations.\n",
        "    list-contacts                     Address book.\n",
        "    list-myteams                      Team memberships.\n",
        "\n",
        "────────────────────────────────────────────────────────────────\n",
        "NOTIFICATIONS\n",
        "────────────────────────────────────────────────────────────────\n",
        "    notifications (notif) list        List account notifications.\n",
        "    notifications mark-read <UPTO_ID> Mark all notifications up to and\n",
        "                                      including UPTO_ID as read.\n",
        "    list-notifications                Canonical single-token list form.\n",
        "\n",
        "────────────────────────────────────────────────────────────────\n",
        "AUDIT TRAIL\n",
        "────────────────────────────────────────────────────────────────\n",
        "    audit verify [--from ID] [--to ID]\n",
        "                                      Walk the tamper-evident hash\n",
        "                                      chain (SHA-256 per entry, optional\n",
        "                                      HMAC when PCLOUD_AUDIT_HMAC_KEY\n",
        "                                      is set). Reports the first broken\n",
        "                                      link if any. Integrity check\n",
        "                                      suitable for nightly cron.\n",
        "\n",
        "────────────────────────────────────────────────────────────────\n",
        "SHELL COMPLETION\n",
        "────────────────────────────────────────────────────────────────\n",
        "    completion <bash|zsh|fish|elvish|powershell>\n",
        "                                      Emit a completion script to\n",
        "                                      stdout. Examples:\n",
        "                                        pcloudc completion bash > \\\n",
        "                                          /etc/bash_completion.d/pcloudc\n",
        "                                        pcloudc completion zsh > \\\n",
        "                                          ~/.zfunc/_pcloudc\n",
        "\n",
        "────────────────────────────────────────────────────────────────\n",
        "CONFIG FILE\n",
        "────────────────────────────────────────────────────────────────\n",
        "    Path:   ~/.pcloud/config.toml (or --config <p> / $PCLOUD_CONFIG)\n",
        "    Mode:   0644 file, inside a 0700 parent dir.\n",
        "    Keys:   username, mountpoint, fuse_opts, log_path, fs_event_log,\n",
        "            log_level, trust_device, passascrypto, save_password,\n",
        "            crypto, cache_size_gb.\n",
        "    Secrets are NEVER written here; the file is auto-created on first\n",
        "    run with fully-commented defaults so it doubles as live docs.\n",
        "    CLI flags always override file values.\n",
        "\n",
        "────────────────────────────────────────────────────────────────\n",
        "ENVIRONMENT VARIABLES\n",
        "────────────────────────────────────────────────────────────────\n",
        "    PCLOUD_ROOT                 Override the whole data root\n",
        "                                (default ~/.pcloud/). Both pcloudc and\n",
        "                                pcloudd honour it.\n",
        "    PCLOUD_CONFIG               Override config-file path (still\n",
        "                                --config wins if both set).\n",
        "    PCLOUD_DURABLE_AUTH_TOKENS  Opt in to the auth-token vault at\n",
        "                                daemon level. Default OFF. When set to\n",
        "                                `1`, the daemon writes/reads\n",
        "                                ~/.pcloud/config/auth_token.\n",
        "    PCLOUD_ENV                  production | development | test.\n",
        "                                Production rejects plaintext API mode\n",
        "                                at ApiEndpoint::validate.\n",
        "    PCLOUD_API_HOST, PCLOUD_API_PORT, PCLOUD_API_SERVER_NAME\n",
        "                                Override API endpoint (normally unused).\n",
        "    PCLOUD_CACHE_SIZE_GB        Set by `pcloudc start` from the config\n",
        "                                file; read by the daemon at mount-\n",
        "                                factory construction.\n",
        "    PCLOUD_DEFAULT_MOUNTPOINT, PCLOUD_LOG_PATH, PCLOUD_FS_EVENT_LOG,\n",
        "    PCLOUD_LOG_LEVEL, PCLOUD_FUSE_OPTS\n",
        "                                Set by `pcloudc start` from config.\n",
        "    PCLOUD_AUDIT_HMAC_KEY       Optional HMAC key (hex) for the audit\n",
        "                                chain; when set, every entry carries an\n",
        "                                HMAC column additionally.\n",
        "\n",
        "────────────────────────────────────────────────────────────────\n",
        "EXIT CODES\n",
        "────────────────────────────────────────────────────────────────\n",
        "    0  ok / success\n",
        "    1  generic error\n",
        "    2  usage / argument parsing error\n",
        "    3  authentication / authorization failure\n",
        "    4  network / IPC transport failure\n",
        "    5  crypto locked / unavailable\n",
        "    6  feature or daemon unavailable\n",
        "    7  conflicting state (e.g. already mounted, no pending challenge)\n",
        "    8  daemon internal error\n",
        "\n",
        "────────────────────────────────────────────────────────────────\n",
        "EXAMPLES\n",
        "────────────────────────────────────────────────────────────────\n",
        "    # Full zero-interaction daily startup (after initial login):\n",
        "    export PCLOUD_DURABLE_AUTH_TOKENS=1\n",
        "    pcloudc start && pcloudc mount /mnt/pcloud\n",
        "\n",
        "    # Fully interactive first-time login with auto-mount:\n",
        "    pcloudc login -u me@example.com -m -s\n",
        "\n",
        "    # Scripted login from a credential helper with crypto unlock:\n",
        "    gpg -d ~/.secrets/pcloud.gpg | pcloudc login \\\n",
        "        -u me@example.com --password-stdin -c -m /mnt/pcloud\n",
        "\n",
        "    # Machine-readable status for monitoring:\n",
        "    pcloudc --json status                   # full JSON envelope\n",
        "    pcloudc status auth sync crypto        # inline selector fields\n",
        "\n",
        "    # Verify the audit chain (nightly cron):\n",
        "    pcloudc -q audit verify; echo $?\n",
        "\n",
        "    # Shell completion:\n",
        "    pcloudc completion bash | sudo tee /etc/bash_completion.d/pcloudc\n",
        "\n",
        "SEE ALSO\n",
        "    pcloudd(1), pcloud.conf(5)\n",
        "\n",
        "FILES\n",
        "    ~/.pcloud/config.toml                CLI / daemon defaults (non-secret).\n",
        "    ~/.pcloud/config/auth_token          Token vault (0600, opt-in).\n",
        "    ~/.pcloud/runtime/pcloud.sock        Daemon IPC socket (0600).\n",
        "    ~/.pcloud/state/store.sqlite3        Store + audit log.\n",
        "    ~/.pcloud/state/daemon.log           pcloudd stdout/stderr capture.\n",
        "    ~/.pcloud/cache/fuse-staging/        Write-path staging blobs (0700).\n",
        "\n",
        "REPORTING BUGS\n",
        "    Issue tracker: ./bd (see CLAUDE.md in the repo root).\n",
        "    Security issues: report privately; do not file public tickets.\n",
    )
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum CommandParseError {
    #[error("unknown command '{0}'")]
    UnknownCommand(String),
    /// A `--flag` or `-flag` token appeared inside a valid subcommand but
    /// is not recognized by that subcommand's parser. The global allow-list
    /// in [`crate::globals`] is the first line of defence for obvious
    /// typos; this variant is the second line, catching flags that happen
    /// to be recognised globally but don't belong to the active subcommand
    /// (e.g. `pcloudc sync add --bogus /a /b`).
    #[error("unknown option '{flag}' for '{command}'. Run 'pcloudc {command} --help'.")]
    UnknownOption { command: String, flag: String },
}

fn flag_takes_value(token: &str) -> bool {
    matches!(
        token,
        "--to"
            | "--message"
            | "--from"
            | "--user"
            | "--username"
            | "-u"
            | "--tfa-channel"
            | "--channel"
            | "-T"
            | "--password-env"
            | "-m"
            | "--mountpoint"
            | "-O"
            | "--fuse-opts"
            | "--log-path"
            | "--fs-event-log"
            | "--log-level"
            | "--cache-size"
            | "--config"
            | "--output"
            | "--limit"
            | "--gpg-recipient"
            | "--retention-days"
            | "--zstd-level"
            | "--type"
            | "--max"
            | "--backend"
            | "--hint"
    )
}

fn is_flag_token(token: &str) -> bool {
    token.starts_with('-')
}

fn command_tokens(args: &[String]) -> (Vec<(usize, String)>, std::collections::BTreeSet<usize>) {
    let mut commandish = Vec::new();
    let mut flag_value_indexes = std::collections::BTreeSet::new();
    let mut i = 1;
    while i < args.len() {
        let token = &args[i];
        if is_flag_token(token) {
            if flag_takes_value(token) && i + 1 < args.len() {
                flag_value_indexes.insert(i + 1);
                i += 2;
                continue;
            }
            i += 1;
            continue;
        }
        if !flag_value_indexes.contains(&i) {
            commandish.push((i, token.clone()));
        }
        i += 1;
    }
    (commandish, flag_value_indexes)
}

/// Normalize the raw `argv` into a canonical `(Command, rewritten-args)` pair.
///
/// The rewritten vector always has shape:
///
/// ```text
/// [ argv[0], <canonical-token>, <positional...>, <flags...> ]
/// ```
///
/// This lets the rest of the CLI (which historically indexed `args[2..]` for
/// positional parameters) keep working even when the user typed a two-token
/// legacy form such as `sync add` or `crypto start`, or an alias such as
/// `ls` / `rm` / `st` / `p`.
///
/// Downgrades like `sync` with no subcommand fail with a clear error so we
/// don't silently route it to something unexpected.
pub fn normalize_args(args: &[String]) -> Result<(Command, Vec<String>), CommandParseError> {
    let program = args.first().cloned().unwrap_or_default();
    let (commandish, _) = command_tokens(args);
    let tok0 = commandish.first().map(|(_, s)| s.as_str());
    let tok1 = commandish.get(1).map(|(_, s)| s.as_str());

    // Legacy two-token forms and their aliases.
    let (command, consumed): (Command, usize) = match (tok0, tok1) {
        // Default (no args) -> status, matching legacy behavior.
        (None, _) => (Command::Status, 1),

        // `sync ...` / `s ...`
        (Some("sync" | "s"), Some(sub)) => match sub {
            "list" | "ls" => (Command::SyncList, 2),
            "status" | "st" => (Command::SyncStatus, 2),
            "add" => (Command::SyncAdd, 2),
            "remove" | "rm" => (Command::SyncRemove, 2),
            "change-type" | "set-type" | "retype" => (Command::SyncChangeType, 2),
            // NOTE: pause/resume are daemon-wide commands in the current Rust
            // IPC surface, not per-root operations. Legacy `sync pause`
            // behaved that way too (daemon-wide SYNCPAUSE/SYNCRESUME), so we
            // route to the top-level Pause/Resume.
            "pause" => (Command::Pause, 2),
            "resume" => (Command::Resume, 2),
            "localscan" => (Command::RunLocalScan, 2),
            "conflicts" | "conflict" => (Command::ConflictList, 2),
            "suggest" => (Command::SyncSuggest, 2),
            "is-syncable" | "syncable" => (Command::SyncIsSyncable, 2),
            other => return Err(CommandParseError::UnknownCommand(format!("sync {other}"))),
        },
        (Some("sync" | "s"), None) => {
            return Err(CommandParseError::UnknownCommand(
                "sync (missing subcommand: list|add|remove|change-type|pause|resume|localscan|suggest|is-syncable)"
                    .to_owned(),
            ));
        }

        // `conflict ...`
        (Some("conflict"), Some(sub)) => match sub {
            "list" | "ls" => (Command::ConflictList, 2),
            "resolve" => (Command::ConflictResolve, 2),
            other => {
                return Err(CommandParseError::UnknownCommand(format!(
                    "conflict {other}"
                )));
            }
        },
        (Some("conflict"), None) => {
            return Err(CommandParseError::UnknownCommand(
                "conflict (missing subcommand: list|resolve)".to_owned(),
            ));
        }

        // `publink ...`
        (Some("publink"), Some(sub)) => match sub {
            "send" => (Command::SendPublink, 2),
            other => {
                return Err(CommandParseError::UnknownCommand(format!(
                    "publink {other}"
                )));
            }
        },
        (Some("publink"), None) => {
            return Err(CommandParseError::UnknownCommand(
                "publink (missing subcommand: send)".to_owned(),
            ));
        }

        // `folder ...`
        // Mirrors C `psync_create_remote_folder_by_path`
        // (`pclsync/psynclib.c:1006`). The CLI surface always uses the
        // path-based form; the daemon also accepts the parent-id + name
        // form via the IPC payload (used by the SDK helper).
        (Some("folder"), Some(sub)) => match sub {
            "create" => (Command::CreateRemoteFolder, 2),
            "id" => (Command::GetFolderIdByPath, 2),
            "flags" => (Command::GetFolderFlags, 2),
            "owner" => (Command::GetFolderOwnerId, 2),
            other => return Err(CommandParseError::UnknownCommand(format!("folder {other}"))),
        },
        (Some("folder"), None) => {
            return Err(CommandParseError::UnknownCommand(
                "folder (missing subcommand: create|id|flags|owner)".to_owned(),
            ));
        }

        // `fs ...` — local filesystem status classification.
        // Mirrors C `psync_filesystem_status` (`pclsync/psynclib.c:1903`).
        (Some("fs"), Some(sub)) => match sub {
            "status" => (Command::FilesystemStatus, 2),
            other => return Err(CommandParseError::UnknownCommand(format!("fs {other}"))),
        },
        (Some("fs"), None) => {
            return Err(CommandParseError::UnknownCommand(
                "fs (missing subcommand: status)".to_owned(),
            ));
        }

        // `crypto ...` / `c ...`
        (Some("crypto" | "c"), Some(sub)) => match sub {
            "start" => (Command::SubmitCryptoPassword, 2),
            "stop" => (Command::LockCrypto, 2),
            "status" | "st" => (Command::CryptoStatus, 2),
            "reset" => (Command::CryptoReset, 2),
            "priv-key-flags" | "privkeyflags" => (Command::CryptoPrivKeyFlags, 2),
            "send-change-private" | "send-change" => (Command::CryptoSendChangePrivate, 2),
            "change-password" | "change-pass" => (Command::CryptoChangePassword, 2),
            "change-password-unlocked" | "change-pass-unlocked" => {
                (Command::CryptoChangePasswordUnlocked, 2)
            }
            "hint" => (Command::CryptoHint, 2),
            "setup" | "setup-v2" => (Command::CryptoSetupV2, 2),
            "get-folder-key" | "folder-key" => (Command::CryptoGetFolderKey, 2),
            "get-file-key" | "file-key" => (Command::CryptoGetFileKey, 2),
            other => return Err(CommandParseError::UnknownCommand(format!("crypto {other}"))),
        },
        (Some("crypto" | "c"), None) => {
            return Err(CommandParseError::UnknownCommand(
                "crypto (missing subcommand: start|stop|status|reset|hint|priv-key-flags|send-change-private|change-password|change-password-unlocked|setup|get-folder-key|get-file-key)"
                    .to_owned(),
            ));
        }

        // `account ...`
        (Some("account"), Some(sub)) => match sub {
            "verify-email" => (Command::AccountVerifyEmail, 2),
            "verify-email-restricted" | "verify-restricted" => {
                (Command::AccountVerifyEmailRestricted, 2)
            }
            "lost-password" | "reset-password" | "forgot-password" => {
                (Command::AccountLostPassword, 2)
            }
            "change-password" | "change-pass" => (Command::AccountChangePassword, 2),
            "register" => (Command::AccountRegister, 2),
            "api-servers" | "apiservers" => (Command::AccountApiServers, 2),
            "set-api-server" | "set-server" => (Command::AccountSetApiServer, 2),
            "set-language" | "language" => (Command::AccountSetLanguage, 2),
            "promo" => (Command::AccountPromo, 2),
            other => {
                return Err(CommandParseError::UnknownCommand(format!(
                    "account {other}"
                )));
            }
        },
        (Some("account"), None) => {
            return Err(CommandParseError::UnknownCommand(
                "account (missing subcommand: verify-email|lost-password|change-password|register|api-servers|set-api-server|set-language|promo)"
                    .to_owned(),
            ));
        }

        // `download ...`
        (Some("download" | "dl"), Some(sub)) => match sub {
            "link" => (Command::DownloadLink, 2),
            "file" => (Command::DownloadFile, 2),
            other => {
                return Err(CommandParseError::UnknownCommand(format!(
                    "download {other}"
                )));
            }
        },
        (Some("download" | "dl"), None) => {
            return Err(CommandParseError::UnknownCommand(
                "download (missing subcommand: link|file)".to_owned(),
            ));
        }

        // `notifications ...` / `notif ...`
        (Some("notifications" | "notif"), Some(sub)) => match sub {
            "list" | "ls" => (Command::ListNotifications, 2),
            "mark-read" => {
                let id_tok = commandish.get(2).map(|(_, s)| s).ok_or_else(|| {
                    CommandParseError::UnknownCommand(
                        "notifications mark-read: missing <upto_id>".to_owned(),
                    )
                })?;
                let upto_id: u64 = id_tok.parse::<u64>().map_err(|_| {
                    CommandParseError::UnknownCommand(format!(
                        "notifications mark-read: invalid upto_id '{id_tok}'"
                    ))
                })?;
                (Command::MarkNotificationsRead { upto_id }, 3)
            }
            other => {
                return Err(CommandParseError::UnknownCommand(format!(
                    "notifications {other}"
                )));
            }
        },

        // `session ...`
        (Some("session"), Some(sub)) => match sub {
            "status" | "st" => (Command::SessionStatus, 2),
            other => {
                return Err(CommandParseError::UnknownCommand(format!(
                    "session {other}"
                )));
            }
        },
        (Some("session"), None) => {
            return Err(CommandParseError::UnknownCommand(
                "session (missing subcommand: status)".to_owned(),
            ));
        }

        // `backup ...` — DEPRECATED legacy backup snapshot lifecycle
        // (`backup snapshot-{create,restore,verify,prune}`). Parses to
        // the deprecated `Command::BackupSnapshot*` aliases, which emit
        // a one-line stderr warning at dispatch time before forwarding
        // to the new `snapshot` pipeline. Also handles `backup delete`.
        (Some("backup"), Some(sub)) => match sub {
            "snapshot-create" => (Command::BackupSnapshotCreate, 2),
            "snapshot-restore" => (Command::BackupSnapshotRestore, 2),
            "snapshot-verify" => (Command::BackupSnapshotVerify, 2),
            "snapshot-prune" => (Command::BackupSnapshotPrune, 2),
            "delete" | "rm" => (Command::BackupDelete, 2),
            "create" => (Command::BackupCreate, 2),
            "stop-device" => (Command::BackupStopDevice, 2),
            "delete-device" => (Command::BackupDeleteDevice, 2),
            other => {
                return Err(CommandParseError::UnknownCommand(format!("backup {other}")));
            }
        },
        (Some("backup"), None) => {
            return Err(CommandParseError::UnknownCommand(
                "backup (missing subcommand: create|delete|stop-device|delete-device|snapshot-create|snapshot-restore|snapshot-verify|snapshot-prune)"
                    .to_owned(),
            ));
        }

        // `snapshot ...` — top-level snapshot lifecycle (new surface).
        // Subcommands: `create|restore|verify|prune`. Bare `snapshot`
        // with no subcommand is treated as shorthand for
        // `snapshot create`.
        (Some("snapshot"), Some(sub)) => match sub {
            "create" => (Command::SnapshotCreate, 2),
            "restore" => (Command::SnapshotRestore, 2),
            "verify" => (Command::SnapshotVerify, 2),
            "prune" => (Command::SnapshotPrune, 2),
            other => {
                return Err(CommandParseError::UnknownCommand(format!(
                    "snapshot {other}"
                )));
            }
        },
        // Bare `pcloudc snapshot` → `snapshot create` (shorthand).
        (Some("snapshot"), None) => (Command::SnapshotCreate, 1),

        // `audit ...`
        (Some("audit"), Some(sub)) => match sub {
            "verify" => (Command::AuditVerify, 2),
            other => {
                return Err(CommandParseError::UnknownCommand(format!("audit {other}")));
            }
        },
        (Some("audit"), None) => {
            return Err(CommandParseError::UnknownCommand(
                "audit (missing subcommand: verify)".to_owned(),
            ));
        }

        // H14 PR4 — `integrity ...`. Subcommands: `status` (default),
        // `run-once`, `skip <PATH>`. Tracker: bd-1du.4.6.1.
        (Some("integrity"), Some(sub)) => match sub {
            "status" | "st" => (Command::IntegrityStatus, 2),
            "run-once" | "run_once" => (Command::IntegrityRunOnce, 2),
            "skip" => (Command::IntegritySkip, 2),
            other => {
                return Err(CommandParseError::UnknownCommand(format!(
                    "integrity {other}"
                )));
            }
        },
        // Bare `integrity` defaults to `status` for operator convenience.
        (Some("integrity"), None) => (Command::IntegrityStatus, 1),

        // Tier-2 HA: `pcloudc ha status`. See `docs/enterprise/ha.md`
        // §4.2. Currently only `status` is implemented; `promote` /
        // `release` are design-only (tracked under Tier 3/4).
        (Some("ha"), Some(sub)) => match sub {
            "status" | "st" => (Command::HaStatus, 2),
            other => {
                return Err(CommandParseError::UnknownCommand(format!("ha {other}")));
            }
        },
        // Bare `ha` defaults to `status` for operator convenience.
        (Some("ha"), None) => (Command::HaStatus, 1),

        // Scheduled audit-chain verifier: `pcloudc audit-verifier status`.
        // Only `status` is implemented; the verifier runs on its own cron
        // schedule and does not support on-demand triggering (use
        // `pcloudc audit verify` for that).
        (Some("audit-verifier"), Some(sub)) => match sub {
            "status" | "st" => (Command::AuditVerifierStatus, 2),
            other => {
                return Err(CommandParseError::UnknownCommand(format!(
                    "audit-verifier {other}"
                )));
            }
        },
        // Bare `audit-verifier` defaults to `status`.
        (Some("audit-verifier"), None) => (Command::AuditVerifierStatus, 1),

        // Upload-session control surface. Subcommands: `create`,
        // `pause`, `resume`, `cancel`, `list`. See
        // `docs/book/src/operations/partial-transfers.md`.
        (Some("upload"), Some(sub)) => match sub {
            "create" => (Command::UploadCreate, 2),
            "pause" => (Command::UploadPause, 2),
            "resume" => (Command::UploadResume, 2),
            "cancel" => (Command::UploadCancel, 2),
            "list" | "ls" => (Command::UploadList, 2),
            other => {
                return Err(CommandParseError::UnknownCommand(format!("upload {other}")));
            }
        },
        // Bare `upload` defaults to `list` for operator convenience.
        (Some("upload"), None) => (Command::UploadList, 1),

        // Single-token commands (delegates to classic parser below).
        (Some(t), _) => (parse_single_token(t)?, 1),
    };

    // P1.4: `mount --force-umount <path>` recovers an orphan FUSE
    // mount left behind by a crashed daemon. Detected here so the
    // canonical form stays `mount` (for flag-allowlist and help text)
    // while the resolved `Command` routes to `MountForceUnmount`.
    let command = if matches!(command, Command::Mount) && args.iter().any(|a| a == "--force-umount")
    {
        Command::MountForceUnmount
    } else {
        command
    };

    let consumed_command_indexes: std::collections::BTreeSet<usize> = commandish
        .iter()
        .take(consumed)
        .map(|(idx, _)| *idx)
        .collect();

    // Rebuild canonical argv while preserving original relative ordering of
    // all non-command tokens. This keeps short flags and flag-value pairs
    // stable instead of detaching values from their options.
    let canonical_token = canonical_token_for(&command);
    let mut rewritten = Vec::with_capacity(args.len() + 1);
    rewritten.push(program);
    rewritten.push(canonical_token);

    // Legacy `auth <password>` passes ONE positional (password); map into the
    // submit-password slot layout `[username, password]` with empty username
    // so the daemon reuses its stored session username (matches SENDAUTH).
    let legacy_single_pw_alias = matches!(tok0, Some("auth"));
    if legacy_single_pw_alias {
        rewritten.push(String::new()); // empty username slot
    }

    for (idx, token) in args.iter().enumerate().skip(1) {
        if consumed_command_indexes.contains(&idx) {
            continue;
        }
        rewritten.push(token.clone());
    }

    // P0.9: reject any subcommand-level unknown `--flag` / `-X` token
    // before dispatch. The global allow-list in `globals::known_flag_names`
    // already rejects obvious typos that appear before the subcommand;
    // this second pass ensures flags that happen to be globally valid
    // (e.g. `--to` belongs to `publink send`, not `sync add`) are also
    // caught when used on the wrong subcommand.
    reject_unknown_subcommand_flags(&command, &rewritten)?;

    Ok((command, rewritten))
}

/// Per-subcommand allow-list of `--flag` / `-X` names. Empty for
/// subcommands that accept no flags beyond the global set (already
/// stripped by [`crate::globals::GlobalFlags::extract`] before this
/// layer runs).
#[must_use]
fn allowed_flags_for(command: &Command) -> &'static [&'static str] {
    match command {
        // Login owns the widest flag surface because it composes the
        // credential, mount, FUSE, and config knobs. Mirrors the option
        // list documented in `help_text` and `main::LoginOptions::from_argv`.
        Command::LoginBegin => &[
            "--user",
            "-u",
            "--username",
            "--tfa-channel",
            "-T",
            "--channel",
            "--password-stdin",
            "--password-env",
            "--crypto",
            "-c",
            "--passascrypto",
            "-y",
            "--pass-as-crypto",
            "--trust-device",
            "-r",
            "--trusted-device",
            "--save-password",
            "-s",
            "--mountpoint",
            "-m",
            "--fuse-opts",
            "-O",
            "--log-path",
            "--fs-event-log",
            "--log-level",
            "--cache-size",
            "--config",
        ],
        // `submit-password` accepts the password-source flags because
        // it shares the credential-reading path with `login`.
        Command::SubmitPassword => &[
            "--password-stdin",
            "--password-env",
            "--allow-argv-password",
        ],
        // TFA submission accepts `--trust-device` to ask pCloud to
        // remember this device for future logins.
        Command::SubmitTwoFactorCode | Command::SubmitRecoveryCode => &["--trust-device", "-r"],
        // `sync add` accepts an optional `--type <FLAVOR>` direction
        // selector. Flavor aliases: bilateral|full|both,
        // mirror|download-only|down|remote-to-local,
        // upload-only|up|local-to-remote,
        // backup|backup-archive|archive|keep-remote (deletion-safe
        // archival — bd-1du.5). Parsed into
        // `SecretInputs::sync_type` and threaded into
        // `Request::SyncRootAdd.sync_type`.
        Command::SyncAdd => &["--type"],
        // `publink send <code> --to <emails> [--message <text>]`
        // mirrors C `psync_send_publink`.
        Command::SendPublink => &["--to", "--message"],
        // `mount [<PATH>] [-m PATH] [-O OPTS] [--cache-size GB]
        // [--force-umount]` — the three mount-tuning flags are accepted
        // as one-shot overrides; today the daemon logs them and then
        // uses configured defaults (see `Command::Mount` docs and the
        // manpage mount section). `--force-umount` is parsed here
        // because the top-level parser routes `mount --force-umount`
        // to `MountForceUnmount`.
        Command::Mount | Command::MountForceUnmount => &[
            "--force-umount",
            "--mountpoint",
            "-m",
            "--fuse-opts",
            "-O",
            "--cache-size",
        ],
        // `audit verify [--from <id>] [--to <id>]` — inclusive range.
        Command::AuditVerify => &["--from", "--to"],
        // `migrate-from-c [--dry-run] [--force-overwrite] [--from <dir>]`
        // — flags consumed CLI-side by `main::run_migrate_from_c`.
        Command::MigrateFromC { .. } => &["--dry-run", "--force-overwrite", "--from"],
        // `verify <path> [--recursive] [--fix] [--yes]` — R9 #12.
        // `--json` is consumed globally so it does not appear here.
        Command::Verify { .. } => &["--recursive", "--fix", "--yes"],
        // `snapshot create <path> [--zstd-level N] [--gpg-recipient EMAIL]
        // [--yes] [--retention-days N]` — new top-level surface. `--yes`
        // and `--retention-days` are accepted but ignored on create
        // (documented); they are listed here so scripts composing a
        // single flag set across create/prune do not trip the allow-list.
        Command::SnapshotCreate | Command::BackupSnapshotCreate => &[
            "--gpg-recipient",
            "--yes",
            "--zstd-level",
            "--retention-days",
        ],
        // `snapshot restore <path> [--gpg-recipient EMAIL] --yes`.
        Command::SnapshotRestore | Command::BackupSnapshotRestore => &["--gpg-recipient", "--yes"],
        // `snapshot verify <path> [--gpg-recipient EMAIL]`.
        Command::SnapshotVerify | Command::BackupSnapshotVerify => &["--gpg-recipient", "--yes"],
        // `snapshot prune <dir> --retention-days N [--yes]`.
        Command::SnapshotPrune | Command::BackupSnapshotPrune => {
            &["--retention-days", "--yes", "--gpg-recipient"]
        }
        // `log <PATH> [--limit N] [--json]` — R9 #9 revision history.
        // `--json` is a global flag stripped earlier by `GlobalFlags`.
        Command::FileHistory => &["--limit"],
        // `diff` / `restore` are CLI-side stubs; flags are rejected.
        Command::FileDiff | Command::FileRestore => &[],
        // `sync suggest [<PATH>] [--max N]` — suggest sync folders.
        Command::SyncSuggest => &["--max"],
        // `account register <EMAIL> [--accept-terms]` — terms acceptance.
        Command::AccountRegister => &["--accept-terms"],
        // `account change-password` reads old + new via interactive prompt.
        // `--password-stdin` / `--password-env` are accepted as secure
        // sources to match the `submit-password` convention.
        Command::AccountChangePassword => &["--password-stdin", "--password-env"],
        // `account set-api-server <LOCATION_ID> <BINAPI>` / `account set-language <LANG>`
        // — positionals only, no flags needed.
        Command::AccountSetApiServer | Command::AccountSetLanguage => &[],
        // `account verify-email-restricted <TOKEN>` — positional only.
        Command::AccountVerifyEmailRestricted => &[],
        // `account lost-password <EMAIL>` — positional only.
        Command::AccountLostPassword => &[],
        // `crypto change-password` / `crypto change-password-unlocked`
        // — old + new password via secure prompts; hint + code positionals.
        Command::CryptoChangePassword | Command::CryptoChangePasswordUnlocked => {
            &["--password-stdin", "--password-env"]
        }
        // `crypto setup [--backend <name>] [--acknowledge-not-interop]
        // [--hint <TEXT>]` — dual-backend setup selector. Backend and hint
        // take values; `--acknowledge-not-interop` is a standalone gate.
        // Also accepts the standard password-source flags so scripted
        // callers can feed the passphrase securely.
        Command::CryptoSetupV2 => &[
            "--backend",
            "--acknowledge-not-interop",
            "--hint",
            "--password-stdin",
            "--password-env",
        ],
        // Everything else takes positionals only. Note that `-` alone
        // (often meaning stdin) is handled as a positional by
        // `reject_unknown_subcommand_flags` and never fails this check.
        _ => &[],
    }
}

/// Human-readable display label used in error messages. Mirrors the
/// two-token surface wherever the CLI exposes one (e.g. `sync add`,
/// `publink send`, `audit verify`) so the suggested `--help` invocation
/// in the error text matches what the user actually typed.
#[must_use]
fn command_display(command: &Command) -> &'static str {
    match command {
        Command::Status => "status",
        Command::LoginBegin => "login",
        Command::Mount => "mount",
        Command::MountForceUnmount => "mount --force-umount",
        Command::Unmount => "unmount",
        Command::SyncAdd => "sync add",
        Command::SyncRemove => "sync remove",
        Command::SyncChangeType => "sync change-type",
        Command::SyncList => "sync list",
        Command::SyncStatus => "sync status",
        Command::RunLocalScan => "sync localscan",
        Command::SendPublink => "publink send",
        Command::AuditVerify => "audit verify",
        Command::CreateRemoteFolder => "folder create",
        Command::GetFolderIdByPath => "folder id",
        Command::GetFolderFlags => "folder flags",
        Command::GetFolderOwnerId => "folder owner",
        Command::FilesystemStatus => "fs status",
        Command::Stat => "stat",
        Command::SubmitCryptoPassword => "crypto start",
        Command::LockCrypto => "crypto stop",
        Command::CryptoStatus => "crypto status",
        Command::SessionStatus => "session status",
        Command::SubmitPassword => "submit-password",
        Command::SubmitAuthToken => "submit-auth",
        Command::SubmitTwoFactorCode => "submit-tfa",
        Command::SubmitRecoveryCode => "submit-recovery",
        Command::ListNotifications => "notifications list",
        Command::MarkNotificationsRead { .. } => "notifications mark-read",
        Command::Verify { .. } => "verify",
        Command::SnapshotCreate => "snapshot create",
        Command::SnapshotRestore => "snapshot restore",
        Command::SnapshotVerify => "snapshot verify",
        Command::SnapshotPrune => "snapshot prune",
        Command::BackupSnapshotCreate => "backup snapshot-create",
        Command::BackupSnapshotRestore => "backup snapshot-restore",
        Command::BackupSnapshotVerify => "backup snapshot-verify",
        Command::BackupSnapshotPrune => "backup snapshot-prune",
        // Crypto subcommands
        Command::CryptoReset => "crypto reset",
        Command::CryptoPrivKeyFlags => "crypto priv-key-flags",
        Command::CryptoSendChangePrivate => "crypto send-change-private",
        Command::CryptoChangePassword => "crypto change-password",
        Command::CryptoChangePasswordUnlocked => "crypto change-password-unlocked",
        Command::CryptoHint => "crypto hint",
        Command::CryptoSetupV2 => "crypto setup",
        Command::CryptoGetFolderKey => "crypto get-folder-key",
        Command::CryptoGetFileKey => "crypto get-file-key",
        // Sync subcommands
        Command::SyncSuggest => "sync suggest",
        Command::SyncIsSyncable => "sync is-syncable",
        // Account subcommands
        Command::AccountVerifyEmail => "account verify-email",
        Command::AccountVerifyEmailRestricted => "account verify-email-restricted",
        Command::AccountLostPassword => "account lost-password",
        Command::AccountChangePassword => "account change-password",
        Command::AccountRegister => "account register",
        Command::AccountApiServers => "account api-servers",
        Command::AccountSetApiServer => "account set-api-server",
        Command::AccountSetLanguage => "account set-language",
        Command::AccountPromo => "account promo",
        // Download subcommands
        Command::DownloadLink => "download link",
        Command::DownloadFile => "download file",
        // Backup subcommands
        Command::BackupDelete => "backup delete",
        Command::BackupCreate => "backup create",
        Command::BackupStopDevice => "backup stop-device",
        Command::BackupDeleteDevice => "backup delete-device",
        // Fall back to the canonical single-token name for everything
        // else (handled by the caller via `canonical_token_for`).
        _ => "",
    }
}

/// Scan a normalized argv for any `--flag` / `-X` token that is not in
/// the active subcommand's allow-list. Returns `CommandParseError::UnknownOption`
/// on first violation so the caller (main.rs::run) surfaces
/// [`crate::exit_code::ExitCode::Usage`] rather than silently dropping
/// the flag.
///
/// Value tokens paired with known value-taking flags (via
/// [`flag_takes_value`]) are skipped to avoid mis-reporting a path or
/// id that happens to start with `-` as an unknown flag. The `-` bare
/// token (stdin sentinel in many tools) is always treated as positional.
fn reject_unknown_subcommand_flags(
    command: &Command,
    rewritten: &[String],
) -> Result<(), CommandParseError> {
    let allowed = allowed_flags_for(command);
    // `rewritten[0]` is argv[0] (program), `rewritten[1]` is the canonical
    // command token. Everything from index 2 onward is a mix of positional
    // args and flags.
    let mut i = 2;
    while i < rewritten.len() {
        let token = &rewritten[i];
        // Bare `-` is a positional (stdin marker), never a flag.
        if token == "-" || !token.starts_with('-') {
            i += 1;
            continue;
        }
        // Split `--flag=value` → `--flag` for allow-list comparison and
        // for error reporting so passwords/tokens never leak.
        let (name, value_inline) = token
            .split_once('=')
            .map_or((token.as_str(), None), |(k, v)| (k, Some(v)));
        if allowed.contains(&name) {
            // Known flag: skip its value if it takes one and the value
            // wasn't glued on with `=`.
            if flag_takes_value(name) && value_inline.is_none() {
                i += 2;
            } else {
                i += 1;
            }
            continue;
        }
        // Unknown flag for this subcommand. Report the flag *name* only
        // (never the `=value` tail, which may carry a secret).
        let display = command_display(command);
        let label = if display.is_empty() {
            canonical_token_for(command)
        } else {
            display.to_owned()
        };
        return Err(CommandParseError::UnknownOption {
            command: label,
            flag: name.to_owned(),
        });
    }
    Ok(())
}

fn canonical_token_for(command: &Command) -> String {
    match command {
        Command::Help => "help",
        Command::Status => "status",
        Command::Health => "health",
        Command::Pending => "pending",
        Command::Slo => "slo",
        Command::ListLinks => "list-links",
        Command::ListUploadLinks => "list-upload-links",
        Command::ListNotifications => "list-notifications",
        Command::MarkNotificationsRead { .. } => "notifications mark-read",
        Command::CryptoStatus => "crypto-status",
        Command::ShowLink => "show-link",
        Command::DeleteLink => "delete-link",
        Command::CreateFileLink => "create-file-link",
        Command::CreateFolderLink => "create-folder-link",
        Command::ChangeLinkExpire => "change-link-expire",
        Command::ChangeLinkPassword => "change-link-password",
        Command::ChangeLinkUpload => "change-link-upload",
        Command::CreateUploadLink => "create-upload-link",
        Command::DeleteUploadLink => "delete-upload-link",
        Command::CreateTreeLink => "create-tree-link",
        Command::ListLinkAccess => "list-link-access",
        Command::AddLinkAccess => "add-link-access",
        Command::RemoveLinkAccess => "remove-link-access",
        Command::ListBookmarks => "list-bookmarks",
        Command::RemoveBookmark => "remove-bookmark",
        Command::ChangeBookmark => "change-bookmark",
        Command::SyncList => "sync-list",
        Command::SyncStatus => "sync-status",
        Command::SyncAdd => "sync-add",
        Command::SyncRemove => "sync-remove",
        Command::SyncChangeType => "sync-change-type",
        Command::UserInfo => "userinfo",
        Command::Pause => "pause",
        Command::Resume => "resume",
        Command::LoginBegin => "login",
        Command::Logout => "logout",
        Command::SendTwoFactorSms => "send-tfa-sms",
        Command::SendTwoFactorNotification => "send-tfa-notification",
        Command::SubmitPassword => "submit-password",
        Command::SubmitAuthToken => "submit-auth",
        Command::SubmitTwoFactorCode => "submit-tfa",
        Command::SubmitRecoveryCode => "submit-recovery",
        Command::SubmitCryptoPassword => "unlock-crypto",
        Command::AuthSave => "authsave",
        Command::LockCrypto => "lock-crypto",
        Command::Shutdown => "finalize",
        Command::Drain => "drain",
        Command::Reload => "reload",
        Command::Start => "start",
        Command::ListIncomingShares => "list-incoming-shares",
        Command::ListOutgoingShares => "list-outgoing-shares",
        Command::ListIncomingShareRequests => "list-incoming-share-requests",
        Command::ListOutgoingShareRequests => "list-outgoing-share-requests",
        Command::ListContacts => "list-contacts",
        Command::ListMyTeams => "list-myteams",
        Command::ShareFolder => "share-folder",
        Command::CancelShareRequest => "cancel-share-request",
        Command::DeclineShareRequest => "decline-share-request",
        Command::AcceptShareRequest => "accept-share-request",
        Command::RemoveShare => "remove-share",
        Command::ModifyShare => "modify-share",
        Command::AccountStopShare => "account-stopshare",
        Command::AccountModifyShare => "account-modifyshare",
        Command::AccountTeamShare => "account-teamshare",
        Command::SessionStatus => "session-status",
        Command::AuditVerify => "audit-verify",
        Command::Mount => "mount",
        Command::MountForceUnmount => "mount",
        Command::Unmount => "unmount",
        Command::RunLocalScan => "sync-localscan",
        Command::SendPublink => "publink-send",
        Command::CreateRemoteFolder => "folder-create",
        Command::GetFolderIdByPath => "folder-id",
        Command::GetFolderFlags => "folder-flags",
        Command::GetFolderOwnerId => "folder-owner",
        Command::FilesystemStatus => "fs-status",
        Command::Stat => "stat",
        Command::Doctor => "doctor",
        Command::MigrateFromC { .. } => "migrate-from-c",
        Command::Verify { .. } => "verify",
        Command::FileHistory => "log",
        Command::FileDiff => "diff",
        Command::FileRestore => "restore",
        Command::SnapshotCreate => "snapshot-create",
        Command::SnapshotRestore => "snapshot-restore",
        Command::SnapshotVerify => "snapshot-verify",
        Command::SnapshotPrune => "snapshot-prune",
        Command::BackupSnapshotCreate => "backup-snapshot-create",
        Command::BackupSnapshotRestore => "backup-snapshot-restore",
        Command::BackupSnapshotVerify => "backup-snapshot-verify",
        Command::BackupSnapshotPrune => "backup-snapshot-prune",
        // H14 PR4 — integrity sweeper subcommands. bd-1du.4.6.1.
        Command::IntegrityStatus => "integrity-status",
        Command::IntegrityRunOnce => "integrity-run-once",
        Command::IntegritySkip => "integrity-skip",
        // Tier-2 HA status. See docs/enterprise/ha.md §4.2.
        Command::HaStatus => "ha-status",
        // Scheduled audit-chain verifier status.
        Command::AuditVerifierStatus => "audit-verifier-status",
        // Upload-session control surface.
        Command::UploadCreate => "upload-create",
        Command::UploadPause => "upload-pause",
        Command::UploadResume => "upload-resume",
        Command::UploadCancel => "upload-cancel",
        Command::UploadList => "upload-list",
        Command::ConflictList => "conflict-list",
        Command::ConflictResolve => "conflict-resolve",
        // ── Crypto (Group A) ────────────────────────────────────────────
        Command::CryptoReset => "crypto-reset",
        Command::CryptoPrivKeyFlags => "crypto-priv-key-flags",
        Command::CryptoSendChangePrivate => "crypto-send-change-private",
        Command::CryptoChangePassword => "crypto-change-password",
        Command::CryptoChangePasswordUnlocked => "crypto-change-password-unlocked",
        Command::CryptoHint => "crypto-hint",
        // ── Crypto dual-backend (Stage 4b.4) ─────────────────────────────
        Command::CryptoSetupV2 => "crypto-setup-v2",
        Command::CryptoGetFolderKey => "crypto-get-folder-key",
        Command::CryptoGetFileKey => "crypto-get-file-key",
        // ── Sync (Group A) ──────────────────────────────────────────────
        Command::SyncSuggest => "sync-suggest",
        Command::SyncIsSyncable => "sync-is-syncable",
        // ── Account (Group B) ───────────────────────────────────────────
        Command::AccountVerifyEmail => "account-verify-email",
        Command::AccountVerifyEmailRestricted => "account-verify-email-restricted",
        Command::AccountLostPassword => "account-lost-password",
        Command::AccountChangePassword => "account-change-password",
        Command::AccountRegister => "account-register",
        Command::AccountApiServers => "account-api-servers",
        Command::AccountSetApiServer => "account-set-api-server",
        Command::AccountSetLanguage => "account-set-language",
        Command::AccountPromo => "account-promo",
        // ── Transfers / downloads (Group B) ─────────────────────────────
        Command::DownloadLink => "download-link",
        Command::DownloadFile => "download-file",
        // ── Backup (Group B) ────────────────────────────────────────────
        Command::BackupDelete => "backup-delete",
        Command::BackupCreate => "backup-create",
        Command::BackupStopDevice => "backup-stop-device",
        Command::BackupDeleteDevice => "backup-delete-device",
        // ── Tree link from paths ─────────────────────────────────────────
        Command::CreateTreeLinkFromPaths => "create-tree-link-from-paths",
    }
    .to_owned()
}

fn parse_single_token(token: &str) -> Result<Command, CommandParseError> {
    Ok(match token {
        "help" | "--help" | "-h" | "?" => Command::Help,
        "status" | "st" => Command::Status,
        "health" => Command::Health,
        "pending" | "p" => Command::Pending,
        "slo" => Command::Slo,
        "list-links" | "list-public-links" => Command::ListLinks,
        "list-upload-links" => Command::ListUploadLinks,
        "list-notifications" => Command::ListNotifications,
        "crypto-status" => Command::CryptoStatus,
        "show-link" => Command::ShowLink,
        "delete-link" => Command::DeleteLink,
        "create-file-link" => Command::CreateFileLink,
        "create-folder-link" => Command::CreateFolderLink,
        "change-link-expire" => Command::ChangeLinkExpire,
        "change-link-password" => Command::ChangeLinkPassword,
        "change-link-upload" => Command::ChangeLinkUpload,
        "create-upload-link" => Command::CreateUploadLink,
        "delete-upload-link" => Command::DeleteUploadLink,
        "create-tree-link" => Command::CreateTreeLink,
        "list-link-access" => Command::ListLinkAccess,
        "add-link-access" => Command::AddLinkAccess,
        "remove-link-access" => Command::RemoveLinkAccess,
        "list-bookmarks" => Command::ListBookmarks,
        "remove-bookmark" => Command::RemoveBookmark,
        "change-bookmark" => Command::ChangeBookmark,
        "sync-list" => Command::SyncList,
        "sync-status" => Command::SyncStatus,
        "sync-add" => Command::SyncAdd,
        "sync-remove" => Command::SyncRemove,
        "sync-change-type" | "sync-set-type" | "sync-retype" => Command::SyncChangeType,
        "userinfo" => Command::UserInfo,
        "pause" => Command::Pause,
        "resume" => Command::Resume,
        "login" => Command::LoginBegin,
        "logout" => Command::Logout,
        "send-tfa-sms" => Command::SendTwoFactorSms,
        "send-tfa-notification" => Command::SendTwoFactorNotification,
        "submit-password" => Command::SubmitPassword,
        "submit-auth" => Command::SubmitAuthToken,
        "submit-tfa" => Command::SubmitTwoFactorCode,
        "submit-recovery" => Command::SubmitRecoveryCode,
        // `tfa <code>` is the legacy single-token alias for submit-tfa.
        "tfa" => Command::SubmitTwoFactorCode,
        // `auth <password>` is legacy for submit-password (daemon-resident).
        // It preserves the username from the running daemon session; the CLI
        // only forwards the supplied password.
        "auth" => Command::SubmitPassword,
        "unlock-crypto" => Command::SubmitCryptoPassword,
        "authsave" => Command::AuthSave,
        "lock-crypto" => Command::LockCrypto,
        "finalize" | "shutdown" | "f" => Command::Shutdown,
        "start" | "daemon-start" => Command::Start,
        "stop" => Command::Shutdown,
        "drain" | "daemon-drain" => Command::Drain,
        "reload" | "daemon-reload" => Command::Reload,
        "list-incoming-shares" => Command::ListIncomingShares,
        "list-outgoing-shares" => Command::ListOutgoingShares,
        "list-incoming-share-requests" => Command::ListIncomingShareRequests,
        "list-outgoing-share-requests" => Command::ListOutgoingShareRequests,
        "list-contacts" => Command::ListContacts,
        "list-myteams" => Command::ListMyTeams,
        "share-folder" => Command::ShareFolder,
        "cancel-share-request" => Command::CancelShareRequest,
        "decline-share-request" => Command::DeclineShareRequest,
        "accept-share-request" => Command::AcceptShareRequest,
        "remove-share" => Command::RemoveShare,
        "modify-share" => Command::ModifyShare,
        "account-stopshare" => Command::AccountStopShare,
        "account-modifyshare" => Command::AccountModifyShare,
        "account-teamshare" => Command::AccountTeamShare,
        "audit-verify" => Command::AuditVerify,
        "session-status" => Command::SessionStatus,
        "mount" => Command::Mount,
        "unmount" | "umount" => Command::Unmount,
        "sync-localscan" | "localscan" | "run-localscan" => Command::RunLocalScan,
        "publink-send" | "send-publink" => Command::SendPublink,
        "folder-create" | "create-folder" => Command::CreateRemoteFolder,
        "folder-id" | "get-folder-id" => Command::GetFolderIdByPath,
        "folder-flags" | "get-folder-flags" => Command::GetFolderFlags,
        "folder-owner" | "get-folder-owner" => Command::GetFolderOwnerId,
        "fs-status" | "filesystem-status" => Command::FilesystemStatus,
        "stat" | "stat-path" => Command::Stat,
        "doctor" | "self-check" | "selfcheck" => Command::Doctor,
        // `migrate-from-c` is CLI-side only. Flag/value parsing
        // (`--dry-run`, `--force-overwrite`, `--from <path>`) happens
        // in `main::run_migrate_from_c` against the full reduced argv
        // — the Command variant carries default values here.
        "migrate-from-c" | "migrate" => Command::MigrateFromC {
            dry_run: false,
            force_overwrite: false,
            from: None,
        },
        // `log` / `file-log` — git-log-style revision history (R9 #9).
        // Positional path and `--limit` flag are resolved by
        // `parse_inputs_for_command`. Honest scope: the daemon
        // currently returns Unavailable until bd-1du.10 clears the
        // public-API gate.
        "log" | "file-log" | "file-history" => Command::FileHistory,
        // `diff` / `restore` — placeholder stubs for the revision
        // follow-up. Always exit Unavailable for now.
        "diff" | "file-diff" => Command::FileDiff,
        "restore" | "file-restore" => Command::FileRestore,
        // `verify` walks a local path and cross-checks SHA256 against
        // the server-reported digest. R9 enhancement #12. Positional
        // path and boolean flags are resolved by
        // `parse_inputs_for_command`.
        "verify" => Command::Verify {
            path: std::path::PathBuf::new(),
            recursive: false,
            fix: false,
            yes: false,
        },
        // New top-level snapshot surface (single-token canonical form
        // mirrors the two-token surface `snapshot {create,restore,
        // verify,prune}`). Bare `snapshot` is normalised to
        // `snapshot create` earlier in `normalize_args`.
        "snapshot" | "snapshot-create" => Command::SnapshotCreate,
        "snapshot-restore" => Command::SnapshotRestore,
        "snapshot-verify" => Command::SnapshotVerify,
        "snapshot-prune" => Command::SnapshotPrune,
        // Deprecated single-token forms kept for back-compat. Emit a
        // one-line stderr warning at dispatch time; forward behaviour
        // is identical to the new `snapshot-*` variants.
        "backup-snapshot-create" => Command::BackupSnapshotCreate,
        "backup-snapshot-restore" => Command::BackupSnapshotRestore,
        "backup-snapshot-verify" => Command::BackupSnapshotVerify,
        "backup-snapshot-prune" => Command::BackupSnapshotPrune,
        // Tier-2 HA status. Canonical single-token form. Two-token
        // `ha status` is handled by `normalize_args`.
        "ha-status" => Command::HaStatus,
        // Scheduled audit-chain verifier status. Single-token form.
        "audit-verifier-status" => Command::AuditVerifierStatus,
        // Upload-session single-token aliases (two-token `upload create` etc.
        // is handled by `normalize_args`).
        "upload-create" => Command::UploadCreate,
        "upload-pause" => Command::UploadPause,
        "upload-resume" => Command::UploadResume,
        "upload-cancel" => Command::UploadCancel,
        "upload-list" => Command::UploadList,
        "conflict-list" => Command::ConflictList,
        "conflict-resolve" => Command::ConflictResolve,
        // ── Crypto (Group A) single-token aliases ────────────────────────
        "crypto-reset" => Command::CryptoReset,
        "crypto-priv-key-flags" | "crypto-privkeyflags" => Command::CryptoPrivKeyFlags,
        "crypto-send-change-private" => Command::CryptoSendChangePrivate,
        "crypto-change-password" | "crypto-change-pass" => Command::CryptoChangePassword,
        "crypto-change-password-unlocked" => Command::CryptoChangePasswordUnlocked,
        "crypto-hint" => Command::CryptoHint,
        // ── Crypto dual-backend (Stage 4b.4) single-token aliases ────────
        "crypto-setup" | "crypto-setup-v2" => Command::CryptoSetupV2,
        "crypto-get-folder-key" | "crypto-folder-key" => Command::CryptoGetFolderKey,
        "crypto-get-file-key" | "crypto-file-key" => Command::CryptoGetFileKey,
        // ── Sync (Group A) single-token aliases ──────────────────────────
        "sync-suggest" => Command::SyncSuggest,
        "sync-is-syncable" | "sync-syncable" => Command::SyncIsSyncable,
        // ── Account (Group B) single-token aliases ───────────────────────
        "account-verify-email" => Command::AccountVerifyEmail,
        "account-verify-email-restricted" => Command::AccountVerifyEmailRestricted,
        "account-lost-password" | "account-reset-password" => Command::AccountLostPassword,
        "account-change-password" | "account-change-pass" => Command::AccountChangePassword,
        "account-register" => Command::AccountRegister,
        "account-api-servers" => Command::AccountApiServers,
        "account-set-api-server" => Command::AccountSetApiServer,
        "account-set-language" => Command::AccountSetLanguage,
        "account-promo" => Command::AccountPromo,
        // ── Transfers / downloads (Group B) single-token aliases ─────────
        "download-link" => Command::DownloadLink,
        "download-file" => Command::DownloadFile,
        // ── Backup (Group B) single-token aliases ────────────────────────
        "backup-delete" => Command::BackupDelete,
        "backup-create" => Command::BackupCreate,
        "backup-stop-device" => Command::BackupStopDevice,
        "backup-delete-device" => Command::BackupDeleteDevice,
        // ── Tree link from paths single-token alias ───────────────────────
        "create-tree-link-from-paths" => Command::CreateTreeLinkFromPaths,
        other => return Err(CommandParseError::UnknownCommand(other.to_owned())),
    })
}

pub fn parse_command(args: &[String]) -> Result<Command, CommandParseError> {
    normalize_args(args).map(|(cmd, _)| cmd)
}

/// Aggregated error returned by [`parse_inputs`].
///
/// Downstream SDK callers should propagate this rather than panicking on
/// malformed args. The two inner variants correspond to the two distinct
/// failure points inside `parse_inputs`.
#[derive(Debug, Error)]
pub enum ParseInputsError {
    /// The command token(s) were not recognised by [`parse_command`].
    #[error("command parse error: {0}")]
    Command(#[from] CommandParseError),
    /// The secret-input extraction step failed (e.g. unknown flag, I/O error
    /// from a TTY prompt, or required positional argument absent).
    #[error("input resolution error: {0}")]
    Inputs(#[from] crate::prompt::PromptError),
}

/// Parse [`SecretInputs`] from `args`, returning a [`ParseInputsError`] on
/// any malformed or unrecognised input rather than panicking.
///
/// # Errors
///
/// Returns [`ParseInputsError::Command`] if the command token(s) cannot be
/// classified, or [`ParseInputsError::Inputs`] if secret-input extraction
/// fails (invalid flags, I/O, missing required args).
pub fn parse_inputs(args: &[String]) -> Result<SecretInputs, ParseInputsError> {
    let command = parse_command(args)?;
    let inputs = parse_inputs_for_command(&command, args)?;
    Ok(inputs)
}

pub fn parse_inputs_for_command(
    command: &Command,
    args: &[String],
) -> Result<SecretInputs, PromptError> {
    // Re-normalize so two-token legacy forms (e.g. `sync add <L> <R>`) are
    // rewritten as `sync-add <L> <R>` before the positional lookups below
    // index into `args[2..]`. If normalization disagrees with the caller's
    // command we trust the caller's command but still use the normalized
    // positional layout (fallback: the original args).
    let normalized = normalize_args(args)
        .map(|(_, rewritten)| rewritten)
        .unwrap_or_else(|_| args.to_vec());
    // `raw_args` preserves the caller's original token order so value-
    // bearing flags like `--from 3` keep their argument adjacency;
    // `normalize_args` partitions flags to the tail which would detach
    // the value.
    let raw_args: &[String] = args;
    let args = normalized.as_slice();

    let trust_device = args.iter().any(|arg| arg == "--trust-device");
    let recovery_code = matches!(command, Command::SubmitRecoveryCode);

    match command {
        Command::SubmitPassword => {
            let username = match args.get(2) {
                Some(username) => username.clone(),
                None => prompt_line("Username")?,
            };
            // Security: argv-password is visible to every process on the host
            // via `/proc/<pid>/cmdline` until this process exits. Preferred
            // paths (in order):
            //   1. `--password-stdin`           — read from stdin
            //   2. `--password-env PCLOUD_PW`   — read from env var
            //   3. no argv password             — interactive rpassword prompt
            // If the caller still provided an argv password we accept it for
            // backward compatibility but emit a clear stderr warning.
            let password = read_password_securely(args)?;
            Ok(build_inputs(trust_device, recovery_code, |inputs| {
                inputs.username = username;
                inputs.password = password;
            }))
        }
        Command::SubmitAuthToken => {
            let auth_token = match args.get(2) {
                Some(token) => {
                    // Hard failure unless the caller explicitly acknowledged the risk.
                    if !args.iter().any(|a| a == "--allow-argv-password") {
                        eprintln!(
                            "Error: Passing secrets as command-line arguments leaks them via \
                             /proc/*/cmdline and shell history. Use --allow-argv-password to override."
                        );
                        std::process::exit(2);
                    }
                    eprintln!(
                        "warning: passing an auth token on the command line is insecure \
                         (visible via /proc/<pid>/cmdline). --allow-argv-password acknowledged."
                    );
                    token.clone()
                }
                None => SecretPrompt::new("Auth token").read_secret()?,
            };
            Ok(build_inputs(trust_device, recovery_code, |inputs| {
                inputs.auth_token = SecretString::new(auth_token);
            }))
        }
        Command::SubmitTwoFactorCode | Command::SubmitRecoveryCode => {
            let two_factor_code = match args.get(2) {
                Some(code) => {
                    // Hard failure unless the caller explicitly acknowledged the risk.
                    if !args.iter().any(|a| a == "--allow-argv-password") {
                        eprintln!(
                            "Error: Passing secrets as command-line arguments leaks them via \
                             /proc/*/cmdline and shell history. Use --allow-argv-password to override."
                        );
                        std::process::exit(2);
                    }
                    eprintln!(
                        "warning: passing a TFA/recovery code on the command line is insecure \
                         (visible via /proc/<pid>/cmdline). --allow-argv-password acknowledged."
                    );
                    code.clone()
                }
                None => SecretPrompt::new("2FA code").read_secret()?,
            };
            Ok(build_inputs(trust_device, recovery_code, |inputs| {
                inputs.two_factor_code = two_factor_code;
            }))
        }
        Command::SubmitCryptoPassword => {
            let crypto_password = match args.get(2) {
                Some(password) => {
                    // Hard failure unless the caller explicitly acknowledged the risk.
                    if !args.iter().any(|a| a == "--allow-argv-password") {
                        eprintln!(
                            "Error: Passing secrets as command-line arguments leaks them via \
                             /proc/*/cmdline and shell history. Use --allow-argv-password to override."
                        );
                        std::process::exit(2);
                    }
                    eprintln!(
                        "warning: passing a crypto password on the command line is insecure \
                         (visible via /proc/<pid>/cmdline). --allow-argv-password acknowledged."
                    );
                    password.clone()
                }
                None => SecretPrompt::new("Crypto password").read_secret()?,
            };
            Ok(build_inputs(trust_device, recovery_code, |inputs| {
                inputs.crypto_password = SecretString::new(crypto_password);
            }))
        }
        Command::SyncAdd => {
            // Walk positional args skipping value-bearing flags so a
            // `--type mirror` token pair placed before/between/after the
            // two positional paths does not shift them. Mirrors the
            // robust scan used by `backup snapshot-*`.
            let mut positionals: Vec<String> = Vec::new();
            let mut i = 2_usize;
            while i < args.len() {
                let tok = &args[i];
                if tok.starts_with('-') && tok != "-" {
                    let name = tok.split_once('=').map_or(tok.as_str(), |(k, _)| k);
                    if flag_takes_value(name) && !tok.contains('=') {
                        i += 2;
                    } else {
                        i += 1;
                    }
                    continue;
                }
                positionals.push(tok.clone());
                i += 1;
            }
            let local_path = match positionals.first() {
                Some(path) => path.clone(),
                None => prompt_line("Local path")?,
            };
            let remote_path = match positionals.get(1) {
                Some(path) => path.clone(),
                None => prompt_line("Remote path")?,
            };
            let sync_type = match parse_flag_string(raw_args, "--type")? {
                Some(raw) => Some(parse_sync_type_alias(&raw)?),
                None => None,
            };
            Ok(build_inputs(trust_device, recovery_code, |inputs| {
                inputs.local_path = local_path;
                inputs.remote_path = remote_path;
                inputs.sync_type = sync_type;
            }))
        }
        Command::SyncChangeType => {
            // `sync change-type <sync-id> <flavor>` — both positionals
            // are required. The flavor parser rejects unknown aliases
            // with the full 9-alias list.
            let sync_id: u64 = match args.get(2) {
                Some(id) => id
                    .parse()
                    .map_err(|_| invalid_input("sync id must be numeric"))?,
                None => prompt_line("Sync ID")?
                    .parse()
                    .map_err(|_| invalid_input("sync id must be numeric"))?,
            };
            let flavor_raw = match args.get(3) {
                Some(raw) => raw.clone(),
                None => prompt_line("Sync flavor (bilateral|mirror|backup)")?,
            };
            let flavor = parse_sync_type_alias(&flavor_raw)?;
            Ok(build_inputs(trust_device, recovery_code, |inputs| {
                inputs.sync_id = sync_id;
                inputs.sync_type_required = Some(flavor);
            }))
        }
        Command::SyncRemove => {
            let sync_id = match args.get(2) {
                Some(id) => id
                    .parse()
                    .map_err(|_| invalid_input("sync id must be numeric"))?,
                None => prompt_line("Sync ID")?
                    .parse()
                    .map_err(|_| invalid_input("sync id must be numeric"))?,
            };
            Ok(build_inputs(trust_device, recovery_code, |inputs| {
                inputs.sync_id = sync_id;
            }))
        }
        Command::ShowLink => {
            let public_link_code = match args.get(2) {
                Some(code) => code.clone(),
                None => prompt_line("Public link code")?,
            };
            Ok(build_inputs(trust_device, recovery_code, |inputs| {
                inputs.public_link_code = public_link_code;
            }))
        }
        Command::DeleteLink => {
            // Accept either a numeric link id OR a string short-code.
            // Numeric values flow through the fast `DeletePublicLink`
            // path; non-numeric values are forwarded as a code, and the
            // daemon resolves the id by scanning `list_public_links`.
            let raw = match args.get(2) {
                Some(value) => value.clone(),
                None => prompt_line("Public link id or code")?,
            };
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return Err(invalid_input("delete-link: id or code is required"));
            }
            let (link_id, code) = match trimmed.parse::<u64>() {
                Ok(id) => (id, String::new()),
                Err(_) => (0_u64, trimmed.to_owned()),
            };
            Ok(build_inputs(trust_device, recovery_code, |inputs| {
                inputs.public_link_id = link_id;
                inputs.public_link_code = code;
            }))
        }
        Command::CreateFileLink | Command::CreateFolderLink => {
            let public_link_path = match args.get(2) {
                Some(path) => path.clone(),
                None => prompt_line("Public link path")?,
            };
            Ok(build_inputs(trust_device, recovery_code, |inputs| {
                inputs.public_link_path = public_link_path;
            }))
        }
        Command::ChangeLinkExpire => {
            let public_link_id = match args.get(2) {
                Some(id) => id
                    .parse()
                    .map_err(|_| invalid_input("public link id must be numeric"))?,
                None => prompt_line("Public link ID")?
                    .parse()
                    .map_err(|_| invalid_input("public link id must be numeric"))?,
            };
            // Accepted value forms for `<EXPIRE>`:
            //   * missing argument           -> clear expiry
            //   * literal `clear` / `none`   -> clear expiry
            //   * non-negative integer       -> unix seconds
            //   * `YYYY-MM-DD`               -> midnight UTC of that
            //                                   civil date, converted
            //                                   to unix seconds
            let public_link_expire = match args.get(3) {
                Some(value) if value.eq_ignore_ascii_case("clear") => None,
                Some(value) if value.eq_ignore_ascii_case("none") => None,
                Some(value) => Some(parse_expire_value(value)?),
                None => None,
            };
            Ok(build_inputs(trust_device, recovery_code, |inputs| {
                inputs.public_link_id = public_link_id;
                inputs.public_link_expire = public_link_expire;
            }))
        }
        Command::ChangeLinkPassword => {
            let public_link_id = match args.get(2) {
                Some(id) => id
                    .parse()
                    .map_err(|_| invalid_input("public link id must be numeric"))?,
                None => prompt_line("Public link ID")?
                    .parse()
                    .map_err(|_| invalid_input("public link id must be numeric"))?,
            };
            let public_link_password: Option<SecretString> = match args.get(3) {
                Some(value) if value.eq_ignore_ascii_case("clear") => None,
                Some(value) => Some(SecretString::new(value.clone())),
                None => {
                    let value = SecretPrompt::new("New password or 'clear'").read_secret()?;
                    if value.eq_ignore_ascii_case("clear") {
                        None
                    } else {
                        Some(SecretString::new(value))
                    }
                }
            };
            Ok(build_inputs(trust_device, recovery_code, |inputs| {
                inputs.public_link_id = public_link_id;
                inputs.public_link_password = public_link_password;
            }))
        }
        Command::ChangeLinkUpload => {
            let public_link_id = match args.get(2) {
                Some(id) => id
                    .parse()
                    .map_err(|_| invalid_input("public link id must be numeric"))?,
                None => prompt_line("Public link ID")?
                    .parse()
                    .map_err(|_| invalid_input("public link id must be numeric"))?,
            };
            let public_link_upload_policy = match args.get(3).map(|s| s.to_ascii_lowercase()) {
                Some(value) if value == "everyone" => PublicLinkUploadPolicy::Everyone,
                Some(value) if value == "chosen" => PublicLinkUploadPolicy::ChosenUsers,
                Some(value) if matches!(value.as_str(), "off" | "disable" | "disabled") => {
                    PublicLinkUploadPolicy::Disabled
                }
                Some(_) => return Err(invalid_input("upload policy must be everyone|chosen|off")),
                None => {
                    let value =
                        prompt_line("Upload policy (everyone/chosen/off)")?;
                    match value.to_ascii_lowercase().as_str() {
                        "everyone" => PublicLinkUploadPolicy::Everyone,
                        "chosen" => PublicLinkUploadPolicy::ChosenUsers,
                        "off" | "disable" | "disabled" => PublicLinkUploadPolicy::Disabled,
                        _ => {
                            return Err(invalid_input("upload policy must be everyone|chosen|off"));
                        }
                    }
                }
            };
            Ok(build_inputs(trust_device, recovery_code, |inputs| {
                inputs.public_link_id = public_link_id;
                inputs.public_link_upload_policy = public_link_upload_policy;
            }))
        }
        Command::CreateUploadLink => {
            let public_link_path = match args.get(2) {
                Some(path) => path.clone(),
                None => prompt_line("Upload link path")?,
            };
            // Bare `create-upload-link <PATH>` (no comment) is the form
            // documented in the manpage recipe; default to an empty
            // comment rather than prompting for one. Callers wanting a
            // non-empty comment still pass it as arg 3.
            let upload_link_comment = match args.get(3) {
                Some(comment) => comment.clone(),
                None => String::new(),
            };
            let upload_link_expire = match args.get(4) {
                Some(value) if value.eq_ignore_ascii_case("none") => None,
                Some(value) => Some(
                    value
                        .parse()
                        .map_err(|_| invalid_input("expire must be a unix timestamp or 'none'"))?,
                ),
                None => None,
            };
            let upload_link_maxspace = match args.get(5) {
                Some(value) if value.eq_ignore_ascii_case("none") => None,
                Some(value) => Some(
                    value
                        .parse()
                        .map_err(|_| invalid_input("maxspace must be numeric or 'none'"))?,
                ),
                None => None,
            };
            let upload_link_maxfiles = match args.get(6) {
                Some(value) if value.eq_ignore_ascii_case("none") => None,
                Some(value) => Some(
                    value
                        .parse()
                        .map_err(|_| invalid_input("maxfiles must be numeric or 'none'"))?,
                ),
                None => None,
            };
            Ok(build_inputs(trust_device, recovery_code, |inputs| {
                inputs.public_link_path = public_link_path;
                inputs.upload_link_comment = upload_link_comment;
                inputs.upload_link_expire = upload_link_expire;
                inputs.upload_link_maxspace = upload_link_maxspace;
                inputs.upload_link_maxfiles = upload_link_maxfiles;
            }))
        }
        Command::CreateTreeLink => {
            let tree_link_name = match args.get(2) {
                Some(name) => name.clone(),
                None => prompt_line("Tree link name")?,
            };
            let tree_root_folder_id = match args.get(3) {
                Some(value) if value.eq_ignore_ascii_case("none") => None,
                Some(value) => Some(
                    value
                        .parse()
                        .map_err(|_| invalid_input("root folder id must be numeric or 'none'"))?,
                ),
                None => None,
            };
            let tree_folder_ids_csv = match args.get(4) {
                Some(value) if value.eq_ignore_ascii_case("none") => None,
                Some(value) => Some(value.clone()),
                None => None,
            };
            let tree_file_ids_csv = match args.get(5) {
                Some(value) if value.eq_ignore_ascii_case("none") => None,
                Some(value) => Some(value.clone()),
                None => None,
            };
            let tree_link_expire = match args.get(6) {
                Some(value) if value.eq_ignore_ascii_case("none") => None,
                Some(value) => Some(
                    value
                        .parse()
                        .map_err(|_| invalid_input("expire must be a unix timestamp or 'none'"))?,
                ),
                None => None,
            };
            let tree_link_maxdownloads = match args.get(7) {
                Some(value) if value.eq_ignore_ascii_case("none") => None,
                Some(value) => Some(
                    value
                        .parse()
                        .map_err(|_| invalid_input("maxdownloads must be numeric or 'none'"))?,
                ),
                None => None,
            };
            let tree_link_maxtraffic = match args.get(8) {
                Some(value) if value.eq_ignore_ascii_case("none") => None,
                Some(value) => Some(
                    value
                        .parse()
                        .map_err(|_| invalid_input("maxtraffic must be numeric or 'none'"))?,
                ),
                None => None,
            };
            Ok(build_inputs(trust_device, recovery_code, |inputs| {
                inputs.tree_link_name = tree_link_name;
                inputs.tree_root_folder_id = tree_root_folder_id;
                inputs.tree_folder_ids_csv = tree_folder_ids_csv;
                inputs.tree_file_ids_csv = tree_file_ids_csv;
                inputs.tree_link_expire = tree_link_expire;
                inputs.tree_link_maxdownloads = tree_link_maxdownloads;
                inputs.tree_link_maxtraffic = tree_link_maxtraffic;
            }))
        }
        Command::DeleteUploadLink => {
            let public_link_id = match args.get(2) {
                Some(id) => id
                    .parse()
                    .map_err(|_| invalid_input("upload link id must be numeric"))?,
                None => prompt_line("Upload link ID")?
                    .parse()
                    .map_err(|_| invalid_input("upload link id must be numeric"))?,
            };
            Ok(build_inputs(trust_device, recovery_code, |inputs| {
                inputs.public_link_id = public_link_id;
            }))
        }
        Command::ListLinkAccess => {
            let public_link_id = match args.get(2) {
                Some(id) => id
                    .parse()
                    .map_err(|_| invalid_input("public link id must be numeric"))?,
                None => prompt_line("Public link ID")?
                    .parse()
                    .map_err(|_| invalid_input("public link id must be numeric"))?,
            };
            Ok(build_inputs(trust_device, recovery_code, |inputs| {
                inputs.public_link_id = public_link_id;
            }))
        }
        Command::AddLinkAccess => {
            let public_link_id = match args.get(2) {
                Some(id) => id
                    .parse()
                    .map_err(|_| invalid_input("public link id must be numeric"))?,
                None => prompt_line("Public link ID")?
                    .parse()
                    .map_err(|_| invalid_input("public link id must be numeric"))?,
            };
            let public_link_email = match args.get(3) {
                Some(email) => email.clone(),
                None => prompt_line("Email")?,
            };
            Ok(build_inputs(trust_device, recovery_code, |inputs| {
                inputs.public_link_id = public_link_id;
                inputs.public_link_email = public_link_email;
            }))
        }
        Command::RemoveLinkAccess => {
            let public_link_id = match args.get(2) {
                Some(id) => id
                    .parse()
                    .map_err(|_| invalid_input("public link id must be numeric"))?,
                None => prompt_line("Public link ID")?
                    .parse()
                    .map_err(|_| invalid_input("public link id must be numeric"))?,
            };
            let public_link_receiver_id = match args.get(3) {
                Some(id) => id
                    .parse()
                    .map_err(|_| invalid_input("receiver id must be numeric"))?,
                None => prompt_line("Receiver ID")?
                    .parse()
                    .map_err(|_| invalid_input("receiver id must be numeric"))?,
            };
            Ok(build_inputs(trust_device, recovery_code, |inputs| {
                inputs.public_link_id = public_link_id;
                inputs.public_link_receiver_id = public_link_receiver_id;
            }))
        }
        Command::ListBookmarks => Ok(build_inputs(trust_device, recovery_code, |_| {})),
        Command::RemoveBookmark => {
            let bookmark_code = match args.get(2) {
                Some(code) => code.clone(),
                None => prompt_line("Bookmark code")?,
            };
            let bookmark_location_id = match args.get(3) {
                Some(id) => id
                    .parse()
                    .map_err(|_| invalid_input("bookmark location id must be numeric"))?,
                None => prompt_line("Bookmark location ID")?
                    .parse()
                    .map_err(|_| invalid_input("bookmark location id must be numeric"))?,
            };
            Ok(build_inputs(trust_device, recovery_code, |inputs| {
                inputs.bookmark_code = bookmark_code;
                inputs.bookmark_location_id = bookmark_location_id;
            }))
        }
        Command::ChangeBookmark => {
            let bookmark_code = match args.get(2) {
                Some(code) => code.clone(),
                None => prompt_line("Bookmark code")?,
            };
            let bookmark_location_id = match args.get(3) {
                Some(id) => id
                    .parse()
                    .map_err(|_| invalid_input("bookmark location id must be numeric"))?,
                None => prompt_line("Bookmark location ID")?
                    .parse()
                    .map_err(|_| invalid_input("bookmark location id must be numeric"))?,
            };
            let bookmark_name = match args.get(4) {
                Some(name) => name.clone(),
                None => prompt_line("Bookmark name")?,
            };
            let bookmark_description = match args.get(5) {
                Some(description) => description.clone(),
                None => prompt_line("Bookmark description")?,
            };
            Ok(build_inputs(trust_device, recovery_code, |inputs| {
                inputs.bookmark_code = bookmark_code;
                inputs.bookmark_location_id = bookmark_location_id;
                inputs.bookmark_name = bookmark_name;
                inputs.bookmark_description = bookmark_description;
            }))
        }
        Command::ShareFolder => {
            let folder_id = parse_u64_arg(args.get(2), "share folder id")?;
            let name = arg_or_prompt(args.get(3), "Share name")?;
            let mail = arg_or_prompt(args.get(4), "Recipient email")?;
            let message = args.get(5).cloned().unwrap_or_default();
            let permissions_bits = args
                .get(6)
                .map(|v| v.parse::<u32>())
                .transpose()
                .map_err(|_| invalid_input("permissions must be numeric bits"))?
                .unwrap_or(pcloud_model::shares::SharePermissions::READ);
            let hint = args.get(7).filter(|s| !s.is_empty()).cloned();
            Ok(build_inputs(trust_device, recovery_code, |inputs| {
                inputs.share_folder_id = folder_id;
                inputs.share_name = name;
                inputs.share_mail = mail;
                inputs.share_message = message;
                inputs.share_permissions_bits = permissions_bits;
                inputs.share_hint = hint;
            }))
        }
        Command::CancelShareRequest | Command::DeclineShareRequest => {
            let share_request_id = parse_u64_arg(args.get(2), "share request id")?;
            Ok(build_inputs(trust_device, recovery_code, |inputs| {
                inputs.share_request_id = share_request_id;
            }))
        }
        Command::AcceptShareRequest => {
            let share_request_id = parse_u64_arg(args.get(2), "share request id")?;
            let to_folder_id = parse_u64_arg(args.get(3), "destination folder id")?;
            let name = args.get(4).filter(|s| !s.is_empty()).cloned();
            Ok(build_inputs(trust_device, recovery_code, |inputs| {
                inputs.share_request_id = share_request_id;
                inputs.share_to_folder_id = to_folder_id;
                inputs.share_accept_name = name;
            }))
        }
        Command::RemoveShare => {
            let share_id = parse_u64_arg(args.get(2), "share id")?;
            Ok(build_inputs(trust_device, recovery_code, |inputs| {
                inputs.share_id = share_id;
            }))
        }
        Command::ModifyShare => {
            let share_id = parse_u64_arg(args.get(2), "share id")?;
            let permissions_bits: u32 = args
                .get(3)
                .ok_or_else(|| invalid_input("permissions bits required"))?
                .parse()
                .map_err(|_| invalid_input("permissions must be numeric bits"))?;
            Ok(build_inputs(trust_device, recovery_code, |inputs| {
                inputs.share_id = share_id;
                inputs.share_permissions_bits = permissions_bits;
            }))
        }
        Command::AccountStopShare => {
            let user_ids = parse_csv_u64(args.get(2));
            let team_ids = parse_csv_u64(args.get(3));
            Ok(build_inputs(trust_device, recovery_code, |inputs| {
                inputs.share_user_ids = user_ids;
                inputs.share_team_ids = team_ids;
            }))
        }
        Command::AccountModifyShare => {
            let user_mods = parse_csv_pairs(args.get(2));
            let team_mods = parse_csv_pairs(args.get(3));
            Ok(build_inputs(trust_device, recovery_code, |inputs| {
                inputs.share_user_mods = user_mods;
                inputs.share_team_mods = team_mods;
            }))
        }
        Command::AccountTeamShare => {
            let folder_id = parse_u64_arg(args.get(2), "folder id")?;
            let name = arg_or_prompt(args.get(3), "Share name")?;
            let team_id = parse_u64_arg(args.get(4), "team id")?;
            let message = args.get(5).cloned().unwrap_or_default();
            let permissions_bits = args
                .get(6)
                .map(|v| v.parse::<u32>())
                .transpose()
                .map_err(|_| invalid_input("permissions must be numeric bits"))?
                .unwrap_or(pcloud_model::shares::SharePermissions::READ);
            let hint = args.get(7).filter(|s| !s.is_empty()).cloned();
            Ok(build_inputs(trust_device, recovery_code, |inputs| {
                inputs.share_folder_id = folder_id;
                inputs.share_name = name;
                inputs.share_team_id = team_id;
                inputs.share_message = message;
                inputs.share_permissions_bits = permissions_bits;
                inputs.share_hint = hint;
            }))
        }
        Command::AuthSave => {
            // Bare `pcloudc authsave` enables token persistence (matching
            // the natural-language reading of the command). A trailing
            // `off` / `disable` / `false` / `0` / `no` argument is
            // accepted to turn it off; the positive tokens are still
            // accepted for back-compat with pre-fix scripts. No
            // interactive prompt: a bare invocation always enables.
            let enabled = match args.get(2).map(|value| value.to_ascii_lowercase()) {
                Some(value) if matches!(value.as_str(), "on" | "true" | "1" | "yes") => true,
                Some(value)
                    if matches!(value.as_str(), "off" | "false" | "0" | "no" | "disable") =>
                {
                    false
                }
                Some(_) => return Err(invalid_input("authsave expects [on|off]")),
                None => true,
            };
            Ok(build_inputs(trust_device, recovery_code, |inputs| {
                inputs.auth_persistence_enabled = enabled;
            }))
        }
        Command::AuditVerify => {
            // `audit verify [--from <id>] [--to <id>]` - inclusive range.
            let from = parse_flag_i64(raw_args, "--from")?;
            let to = parse_flag_i64(raw_args, "--to")?;
            Ok(build_inputs(trust_device, recovery_code, |inputs| {
                inputs.audit_from_id = from;
                inputs.audit_to_id = to;
            }))
        }
        Command::Mount => {
            // `pcloud-rs mount [<PATH>] [-m PATH] [-O OPTS]
            // [--cache-size GB]`. Flag-form overrides win over the
            // positional path. Any flag value takes precedence over a
            // positional argument; a missing flag falls back to the
            // first positional after the command token, which in turn
            // falls back to an interactive prompt.
            let flag_mountpoint =
                parse_flag_string(raw_args, "--mountpoint")?.or(parse_flag_string(raw_args, "-m")?);
            let flag_fuse_opts =
                parse_flag_string(raw_args, "--fuse-opts")?.or(parse_flag_string(raw_args, "-O")?);
            let flag_cache_size_gb =
                match parse_flag_string(raw_args, "--cache-size")? {
                    Some(raw) => Some(raw.parse::<u64>().map_err(|_| {
                        invalid_input("--cache-size must be a non-negative integer")
                    })?),
                    None => None,
                };
            // Locate a positional path (first non-flag token after the
            // command) as a fallback when `--mountpoint` is not given.
            let positional_path = args.iter().skip(2).find(|a| !a.starts_with('-')).cloned();
            let effective_path = match flag_mountpoint.clone().or(positional_path) {
                Some(p) => p,
                None => prompt_line("Mountpoint path")?,
            };
            Ok(build_inputs(trust_device, recovery_code, |inputs| {
                inputs.mount_path = std::path::PathBuf::from(effective_path);
                inputs.mount_flag_path = flag_mountpoint.map(std::path::PathBuf::from);
                inputs.mount_flag_fuse_opts = flag_fuse_opts;
                inputs.mount_flag_cache_size_gb = flag_cache_size_gb;
            }))
        }
        Command::MountForceUnmount => {
            // `pcloud-rs mount --force-umount <path>`. The path may appear
            // either before or after the flag; scan positionals and pick
            // the first non-flag token. P1.4.
            let path = args.iter().skip(2).find(|a| !a.starts_with('-')).cloned();
            let path = match path {
                Some(p) => p,
                None => prompt_line("Orphan mountpoint path")?,
            };
            Ok(build_inputs(trust_device, recovery_code, |inputs| {
                inputs.mount_path = std::path::PathBuf::from(path);
            }))
        }
        Command::Unmount => {
            // The graceful `unmount` normally takes no arguments — the
            // daemon unmounts whatever it owns. When `PCLOUD_FORCE_UMOUNT`
            // is set (see `commands::env_force_umount_enabled`), the
            // dispatch in `into_request` promotes this to a forceful
            // `MountForceUnmount`, which needs a path. Resolve one
            // opportunistically: first an explicit positional argument,
            // then `PCLOUD_DEFAULT_MOUNTPOINT`. If neither is present
            // we leave `mount_path` empty and `into_request` falls back
            // to the standard graceful `Unmount`.
            let positional = args.iter().skip(2).find(|a| !a.starts_with('-')).cloned();
            let default_env = std::env::var("PCLOUD_DEFAULT_MOUNTPOINT").ok();
            let resolved = positional.or(default_env).unwrap_or_default();
            Ok(build_inputs(trust_device, recovery_code, |inputs| {
                inputs.mount_path = std::path::PathBuf::from(resolved);
            }))
        }
        Command::RunLocalScan => Ok(build_inputs(trust_device, recovery_code, |_| {})),
        Command::SendPublink => {
            // `pcloud-rs publink send <code> --to <emails> [--message <text>]`
            // Mirrors C psync_send_publink (pclsync/psynclib.c:2217).
            let code = match args.get(2) {
                Some(value) => value.clone(),
                None => prompt_line("Public link code")?,
            };
            let mails = match parse_flag_string(raw_args, "--to")? {
                Some(value) => value,
                None => prompt_line("Recipient emails (comma-separated)")?,
            };
            let message = parse_flag_string(raw_args, "--message")?.unwrap_or_default();
            Ok(build_inputs(trust_device, recovery_code, |inputs| {
                inputs.public_link_code = code;
                inputs.send_publink_mails = mails;
                inputs.send_publink_message = message;
            }))
        }
        Command::CreateRemoteFolder => {
            // `pcloud-rs folder create <path>` -> Request::CreateRemoteFolder
            // with `parent_folder_id=None` so the daemon routes to the C
            // `psync_create_remote_folder_by_path` equivalent
            // (`pclsync/psynclib.c:1006`).
            let path = match args.get(2) {
                Some(value) => value.clone(),
                None => {
                    return Err(invalid_input(
                        "folder create: remote path is required (e.g. /Docs/Reports)",
                    ));
                }
            };
            Ok(build_inputs(trust_device, recovery_code, |inputs| {
                inputs.remote_folder_path = path;
            }))
        }
        Command::GetFolderIdByPath | Command::GetFolderFlags | Command::GetFolderOwnerId => {
            // `pcloud-rs folder {id|flags|owner} <absolute-pcloud-path>`.
            // Mirrors C `psync_get_fsfolderid_by_path` /
            // `psync_get_fsfolderflags_by_id` / `psync_get_folder_ownerid`
            // (`pclsync/psynclib.c:2170`, `:2176`, `:2088`).
            let path = match args.get(2) {
                Some(value) => value.clone(),
                None => {
                    return Err(invalid_input(
                        "folder metadata lookup: absolute pCloud path is required (e.g. /Docs)",
                    ));
                }
            };
            Ok(build_inputs(trust_device, recovery_code, |inputs| {
                inputs.folder_metadata_remote_path = path;
            }))
        }
        Command::FileHistory => {
            // `pcloud-rs log <PATH> [--limit N]` — R9 #9 revision history.
            // Honest scope: the daemon currently returns Unavailable
            // until bd-1du.10 clears the public-API gate.
            let path = match args.get(2) {
                Some(value) => value.clone(),
                None => {
                    return Err(invalid_input(
                        "log: remote pCloud-drive path is required (e.g. /Docs/report.txt)",
                    ));
                }
            };
            let limit = match parse_flag_string(raw_args, "--limit")? {
                Some(raw) => Some(
                    raw.parse::<u32>()
                        .map_err(|_| invalid_input("--limit must be a non-negative integer"))?,
                ),
                None => None,
            };
            Ok(build_inputs(trust_device, recovery_code, |inputs| {
                inputs.file_history_path = path;
                inputs.file_history_limit = limit;
            }))
        }
        Command::FileDiff => {
            let path = match args.get(2) {
                Some(value) => value.clone(),
                None => return Err(invalid_input("diff: <PATH> <REV_A> <REV_B> are required")),
            };
            let rev_a = match args.get(3) {
                Some(value) => value.clone(),
                None => return Err(invalid_input("diff: <REV_A> is required")),
            };
            let rev_b = match args.get(4) {
                Some(value) => value.clone(),
                None => return Err(invalid_input("diff: <REV_B> is required")),
            };
            Ok(build_inputs(trust_device, recovery_code, |inputs| {
                inputs.file_history_path = path;
                inputs.file_diff_rev_a = rev_a;
                inputs.file_diff_rev_b = rev_b;
            }))
        }
        Command::FileRestore => {
            let path = match args.get(2) {
                Some(value) => value.clone(),
                None => return Err(invalid_input("restore: <PATH> <REV> are required")),
            };
            let rev = match args.get(3) {
                Some(value) => value.clone(),
                None => return Err(invalid_input("restore: <REV> is required")),
            };
            Ok(build_inputs(trust_device, recovery_code, |inputs| {
                inputs.file_history_path = path;
                inputs.file_restore_rev = rev;
            }))
        }
        Command::FilesystemStatus => {
            // `pcloud-rs fs status <absolute-local-path>`. Mirrors C
            // `psync_filesystem_status` (`pclsync/psynclib.c:1903`).
            let path = match args.get(2) {
                Some(value) => value.clone(),
                None => {
                    return Err(invalid_input(
                        "fs status: absolute local path is required (e.g. /home/user/pcloud)",
                    ));
                }
            };
            Ok(build_inputs(trust_device, recovery_code, |inputs| {
                inputs.filesystem_status_local_path = path;
            }))
        }
        Command::Stat => {
            // `pcloudc stat <absolute-remote-path>`. Mirrors C
            // `psync_stat_path` (`pclsync/psynclib.h:743`).
            let path = match args.get(1) {
                Some(value) => value.clone(),
                None => {
                    return Err(invalid_input(
                        "stat: absolute pCloud-drive path is required (e.g. /Documents/report.txt)",
                    ));
                }
            };
            Ok(build_inputs(trust_device, recovery_code, |inputs| {
                inputs.stat_remote_path = path;
            }))
        }
        Command::SnapshotCreate
        | Command::SnapshotRestore
        | Command::SnapshotVerify
        | Command::SnapshotPrune
        | Command::BackupSnapshotCreate
        | Command::BackupSnapshotRestore
        | Command::BackupSnapshotVerify
        | Command::BackupSnapshotPrune => {
            // `snapshot {create,restore,verify,prune}
            // <PATH> [--zstd-level N] [--gpg-recipient EMAIL] [--yes]
            // [--retention-days N]`. Legacy `backup snapshot-*` tokens
            // land in the same branch and are normalised to the same
            // typed inputs; the one-line deprecation warning is printed
            // at dispatch time.
            //
            // Positional path scan: pick the first non-flag token from
            // `args[2..]`, skipping value tokens consumed by known
            // value-bearing flags.
            let path_str: Option<String> = {
                let mut found: Option<String> = None;
                let mut i = 2_usize;
                while i < args.len() {
                    let tok = &args[i];
                    if tok == "-" {
                        i += 1;
                        continue;
                    }
                    if tok.starts_with('-') {
                        let name = tok.split_once('=').map_or(tok.as_str(), |(k, _)| k);
                        if flag_takes_value(name) && !tok.contains('=') {
                            i += 2;
                        } else {
                            i += 1;
                        }
                        continue;
                    }
                    found = Some(tok.clone());
                    break;
                }
                found
            };
            // Bare `pcloudc snapshot` (no path) is treated as a help
            // prompt: we still require <PATH> for create so the CLI
            // fails with a clear message rather than writing to a
            // mystery location.
            let path = match path_str {
                Some(p) => std::path::PathBuf::from(p),
                None => {
                    return Err(invalid_input("snapshot: <PATH> is required"));
                }
            };
            let gpg_recipient = parse_flag_string(raw_args, "--gpg-recipient")?;
            let retention_days = match parse_flag_string(raw_args, "--retention-days")? {
                Some(raw) => Some(raw.parse::<u32>().map_err(|_| {
                    invalid_input("--retention-days must be a non-negative integer")
                })?),
                None => None,
            };
            // Parse and range-validate --zstd-level at the CLI layer so
            // the daemon receives a pre-checked value.
            let zstd_level = match parse_flag_string(raw_args, "--zstd-level")? {
                Some(raw) => {
                    let v = raw
                        .parse::<i32>()
                        .map_err(|_| invalid_input("--zstd-level must be an integer in 1..=22"))?;
                    if !(1..=22).contains(&v) {
                        return Err(invalid_input("--zstd-level must be an integer in 1..=22"));
                    }
                    Some(v)
                }
                None => None,
            };
            let yes = raw_args.iter().any(|a| a == "--yes");

            let is_prune = matches!(
                command,
                Command::SnapshotPrune | Command::BackupSnapshotPrune
            );
            let is_destructive = is_prune
                || matches!(
                    command,
                    Command::SnapshotRestore | Command::BackupSnapshotRestore
                );

            if is_prune && retention_days.is_none() {
                return Err(invalid_input(
                    "snapshot prune: --retention-days is required",
                ));
            }
            if is_destructive && !yes && !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
                return Err(invalid_input(
                    "snapshot: destructive action requires --yes for non-interactive callers",
                ));
            }

            Ok(build_inputs(trust_device, recovery_code, |inputs| {
                inputs.snapshot_path = path;
                inputs.snapshot_gpg_recipient = gpg_recipient;
                inputs.snapshot_yes = yes;
                inputs.snapshot_retention_days = retention_days;
                inputs.snapshot_zstd_level = zstd_level;
            }))
        }
        Command::Verify { .. } => {
            // `pcloud-rs verify <PATH> [--recursive] [--fix] [--yes]`
            // R9 enhancement #12. The canonical argv laid out by
            // `normalize_args` is `[bin, "verify", <positionals...>,
            // <flags...>]` so `args[2..]` is the positional path and
            // the flags-only tail. Scan it for boolean flags and pick
            // the first non-flag token as the path.
            let mut path: Option<String> = None;
            let mut recursive = false;
            let mut fix = false;
            let mut yes = false;
            for tok in args.iter().skip(2) {
                match tok.as_str() {
                    "--recursive" => recursive = true,
                    "--fix" => fix = true,
                    "--yes" => yes = true,
                    t if t.starts_with('-') => {}
                    t if path.is_none() => path = Some(t.to_owned()),
                    _ => {}
                }
            }
            let path = path.ok_or_else(|| {
                invalid_input(
                    "verify: local path is required (e.g. `pcloudc verify ~/pCloudDrive --recursive`)",
                )
            })?;
            Ok(build_inputs(trust_device, recovery_code, |inputs| {
                inputs.verify_local_path = path;
                inputs.verify_recursive = recursive;
                inputs.verify_fix = fix;
                inputs.verify_yes = yes;
            }))
        }
        // H14 PR4 — `pcloudc integrity skip <PATH>` requires one
        // positional glob pattern. `status` and `run-once` take none.
        Command::IntegritySkip => {
            let pattern = match args.get(2) {
                Some(value) if !value.trim().is_empty() => value.clone(),
                _ => {
                    return Err(invalid_input(
                        "integrity skip: glob pattern is required (e.g. `**/*.tmp`)",
                    ));
                }
            };
            Ok(build_inputs(trust_device, recovery_code, |inputs| {
                inputs.integrity_skip_pattern = pattern;
            }))
        }
        // ── Crypto change-password (Group A) ─────────────────────────────
        Command::CryptoChangePassword => {
            // `crypto change-password <HINT> <CODE> [--max-flags N]`
            // Old password: uses `crypto_password` (SecretString) via prompt.
            // New password: uses `new_crypto_password` via prompt.
            let old_password = match args.get(2) {
                Some(pw) => pw.clone(),
                None => SecretPrompt::new("Current crypto passphrase").read_secret()?,
            };
            let new_password = match args.get(3) {
                Some(pw) => pw.clone(),
                None => SecretPrompt::new("New crypto passphrase").read_secret()?,
            };
            let hint = args.get(4).cloned().unwrap_or_default();
            let code = args.get(5).cloned().unwrap_or_default();
            Ok(build_inputs(trust_device, recovery_code, |inputs| {
                inputs.crypto_password = SecretString::new(old_password);
                inputs.new_crypto_password = SecretString::new(new_password);
                inputs.crypto_change_hint = hint;
                inputs.crypto_change_code = code;
            }))
        }
        Command::CryptoChangePasswordUnlocked => {
            // `crypto change-password-unlocked <HINT> <CODE>` — no old password needed.
            let new_password = match args.get(2) {
                Some(pw) => pw.clone(),
                None => SecretPrompt::new("New crypto passphrase").read_secret()?,
            };
            let hint = args.get(3).cloned().unwrap_or_default();
            let code = args.get(4).cloned().unwrap_or_default();
            Ok(build_inputs(trust_device, recovery_code, |inputs| {
                inputs.new_crypto_password = SecretString::new(new_password);
                inputs.crypto_change_hint = hint;
                inputs.crypto_change_code = code;
            }))
        }
        // ── Sync suggestions (Group A) ────────────────────────────────────
        Command::SyncSuggest => {
            // `sync suggest [<PATH>] [--max N]`
            let path = args.iter().skip(2).find(|a| !a.starts_with('-')).cloned();
            let max = match parse_flag_string(raw_args, "--max")? {
                Some(raw) => Some(
                    raw.parse::<usize>()
                        .map_err(|_| invalid_input("--max must be a non-negative integer"))?,
                ),
                None => None,
            };
            Ok(build_inputs(trust_device, recovery_code, |inputs| {
                inputs.sync_suggest_path = path;
                inputs.sync_suggest_max = max;
            }))
        }
        Command::SyncIsSyncable => {
            // `sync is-syncable <LOCAL_PATH>`
            let path = match args.get(2) {
                Some(value) => value.clone(),
                None => prompt_line("Local path to classify")?,
            };
            Ok(build_inputs(trust_device, recovery_code, |inputs| {
                inputs.local_path = path;
            }))
        }
        // ── Account ops (Group B) ─────────────────────────────────────────
        Command::AccountVerifyEmailRestricted => {
            let token = match args.get(2) {
                Some(value) => value.clone(),
                None => prompt_line("Verify token")?,
            };
            Ok(build_inputs(trust_device, recovery_code, |inputs| {
                inputs.account_verify_token = token;
            }))
        }
        Command::AccountLostPassword => {
            let email = match args.get(2) {
                Some(value) => value.clone(),
                None => prompt_line("Account email")?,
            };
            Ok(build_inputs(trust_device, recovery_code, |inputs| {
                inputs.username = email;
            }))
        }
        Command::AccountChangePassword => {
            let current_password = read_password_securely(args)?;
            let new_password = SecretPrompt::new("New account password").read_secret()?;
            Ok(build_inputs(trust_device, recovery_code, |inputs| {
                inputs.password = current_password;
                inputs.account_new_password = SecretString::new(new_password);
            }))
        }
        Command::AccountRegister => {
            let email = match args.get(2) {
                Some(value) => value.clone(),
                None => prompt_line("New account email")?,
            };
            let password = read_password_securely(args)?;
            let terms_accepted = raw_args.iter().any(|a| a == "--accept-terms");
            Ok(build_inputs(trust_device, recovery_code, |inputs| {
                inputs.username = email;
                inputs.password = password;
                inputs.account_terms_accepted = terms_accepted;
            }))
        }
        Command::AccountSetApiServer => {
            let location_id = match args.get(2) {
                Some(value) => value
                    .parse::<u32>()
                    .map_err(|_| invalid_input("location_id must be numeric"))?,
                None => prompt_line("Location ID")?
                    .parse::<u32>()
                    .map_err(|_| invalid_input("location_id must be numeric"))?,
            };
            let binapi = match args.get(3) {
                Some(value) => value.clone(),
                None => prompt_line("Binary API hostname")?,
            };
            Ok(build_inputs(trust_device, recovery_code, |inputs| {
                inputs.api_server_location_id = location_id;
                inputs.api_server_binapi = binapi;
            }))
        }
        Command::AccountSetLanguage => {
            let language = match args.get(2) {
                Some(value) => value.clone(),
                None => prompt_line("Language tag (e.g. en, de, fr)")?,
            };
            Ok(build_inputs(trust_device, recovery_code, |inputs| {
                inputs.account_language = language;
            }))
        }
        // ── Transfers / downloads (Group B) ──────────────────────────────
        Command::DownloadLink | Command::DownloadFile => {
            let file_id = match args.get(2) {
                Some(value) => value
                    .parse::<u64>()
                    .map_err(|_| invalid_input("file_id must be numeric"))?,
                None => prompt_line("Remote file ID")?
                    .parse::<u64>()
                    .map_err(|_| invalid_input("file_id must be numeric"))?,
            };
            let local_path = if matches!(command, Command::DownloadFile) {
                match args.get(3) {
                    Some(value) => std::path::PathBuf::from(value),
                    None => {
                        return Err(invalid_input("download file: <LOCAL_PATH> is required"));
                    }
                }
            } else {
                std::path::PathBuf::new()
            };
            Ok(build_inputs(trust_device, recovery_code, |inputs| {
                inputs.download_file_id = file_id;
                inputs.download_local_path = local_path;
            }))
        }
        // ── Backup delete (Group B) ───────────────────────────────────────
        Command::BackupDelete => {
            let backup_id = match args.get(2) {
                Some(value) => value
                    .parse::<u64>()
                    .map_err(|_| invalid_input("backup_id must be numeric"))?,
                None => prompt_line("Backup folder ID")?
                    .parse::<u64>()
                    .map_err(|_| invalid_input("backup_id must be numeric"))?,
            };
            Ok(build_inputs(trust_device, recovery_code, |inputs| {
                inputs.backup_delete_id = backup_id;
            }))
        }
        // ── Backup create ─────────────────────────────────────────────────
        Command::BackupCreate => {
            let name = match args.get(2) {
                Some(v) => v.clone(),
                None => prompt_line("Backup name")?,
            };
            let root_folder_id: u64 = match args.get(3) {
                Some(v) => v
                    .parse::<u64>()
                    .map_err(|_| invalid_input("root_folder_id must be numeric"))?,
                None => prompt_line("Remote root folder ID")?
                    .parse::<u64>()
                    .map_err(|_| invalid_input("root_folder_id must be numeric"))?,
            };
            let local_path = match args.get(4) {
                Some(v) => v.clone(),
                None => prompt_line("Local path")?,
            };
            let parent_folder_name = args.get(5).cloned();
            Ok(build_inputs(trust_device, recovery_code, |inputs| {
                inputs.backup_create_name = name;
                inputs.backup_create_root_folder_id = root_folder_id;
                inputs.backup_create_local_path = local_path;
                inputs.backup_create_parent_folder_name = parent_folder_name;
            }))
        }
        // ── Backup stop-device ────────────────────────────────────────────
        Command::BackupStopDevice => {
            let device_folder_id: u64 = match args.get(2) {
                Some(v) => v
                    .parse::<u64>()
                    .map_err(|_| invalid_input("device_folder_id must be numeric"))?,
                None => prompt_line("Device folder ID")?
                    .parse::<u64>()
                    .map_err(|_| invalid_input("device_folder_id must be numeric"))?,
            };
            Ok(build_inputs(trust_device, recovery_code, |inputs| {
                inputs.backup_device_folder_id = device_folder_id;
            }))
        }
        // ── Backup delete-device (local-only) ─────────────────────────────
        Command::BackupDeleteDevice => Ok(build_inputs(trust_device, recovery_code, |_| {})),
        // ── Create tree link from paths ───────────────────────────────────
        Command::CreateTreeLinkFromPaths => {
            // `create-tree-link-from-paths <NAME> <PATH>...`
            // NAME is not a secret; use arg_or_prompt (plain echo).
            let name = match args.get(2) {
                Some(v) => v.clone(),
                None => {
                    return Err(invalid_input(
                        "create-tree-link-from-paths: <NAME> is required",
                    ));
                }
            };
            // args[3..] are pCloud-drive paths resolved daemon-side via
            // the authenticated path resolver (Request::CreateTreePublicLinkFromPaths).
            let paths: Vec<String> = args.get(3..).map(<[String]>::to_vec).unwrap_or_default();
            if paths.is_empty() {
                return Err(invalid_input(
                    "create-tree-link-from-paths: at least one pCloud-drive path is required",
                ));
            }
            Ok(build_inputs(trust_device, recovery_code, |inputs| {
                inputs.tree_link_name = name;
                inputs.tree_link_paths = paths;
            }))
        }
        Command::CryptoSetupV2 => {
            // Resolve --backend / --acknowledge-not-interop / --hint,
            // running the interactive picker on a tty when --backend is
            // absent. Errors here surface as ExitCode::Usage via
            // PromptError::Io(InvalidInput) from the caller.
            let resolution = resolve_crypto_setup_flags(raw_args)?;
            let (backend, ack, hint) = match resolution {
                CryptoSetupResolution::Resolved {
                    backend,
                    acknowledge_not_interop,
                    hint,
                } => (backend, acknowledge_not_interop, hint),
                CryptoSetupResolution::NeedsInteractive { hint } => {
                    use std::io::{BufReader, stdin, stdout};
                    if !is_stdin_tty_for_picker() {
                        return Err(invalid_input(
                            "--backend is required in non-interactive mode (stdin is not a terminal)",
                        ));
                    }
                    let mut reader = BufReader::new(stdin());
                    let mut out = stdout();
                    match crate::crypto_setup_picker::run_picker(&mut reader, &mut out) {
                        crate::crypto_setup_picker::PickerOutcome::Selected {
                            backend,
                            acknowledge_not_interop,
                        } => (backend, acknowledge_not_interop, hint),
                        crate::crypto_setup_picker::PickerOutcome::Aborted(msg) => {
                            return Err(invalid_input_owned(msg));
                        }
                    }
                }
            };
            // Read the passphrase securely. `read_password_securely`
            // prefers `--password-stdin` / `--password-env`, otherwise
            // falls back to an interactive no-echo prompt.
            let crypto_password = if args.iter().any(|a| a == "--password-stdin")
                || args.iter().any(|a| a == "--password-env")
            {
                read_password_securely(args)?
            } else {
                SecretString::new(SecretPrompt::new("New crypto passphrase").read_secret()?)
            };
            Ok(build_inputs(trust_device, recovery_code, |inputs| {
                inputs.crypto_setup_backend = backend;
                inputs.crypto_setup_acknowledge_not_interop = ack;
                inputs.crypto_setup_hint = hint;
                inputs.crypto_password = crypto_password;
            }))
        }
        Command::CryptoGetFolderKey => {
            let folder_id: u64 = match args.get(2) {
                Some(v) => v
                    .parse()
                    .map_err(|_| invalid_input("get-folder-key: FOLDER_ID must be numeric"))?,
                None => {
                    return Err(invalid_input(
                        "get-folder-key: <FOLDER_ID> is required",
                    ));
                }
            };
            Ok(build_inputs(trust_device, recovery_code, |inputs| {
                inputs.crypto_folder_key_folder_id = folder_id;
            }))
        }
        Command::CryptoGetFileKey => {
            let file_id: u64 = match args.get(2) {
                Some(v) => v
                    .parse()
                    .map_err(|_| invalid_input("get-file-key: FILE_ID must be numeric"))?,
                None => {
                    return Err(invalid_input("get-file-key: <FILE_ID> is required"));
                }
            };
            Ok(build_inputs(trust_device, recovery_code, |inputs| {
                inputs.crypto_file_key_file_id = file_id;
            }))
        }
        _ => Ok(build_inputs(trust_device, recovery_code, |_| {})),
    }
}

/// Outcome of pre-dispatch flag resolution for `crypto setup`. Either
/// every knob was supplied explicitly on the command line (`Resolved`),
/// or `--backend` was absent and the caller must drop into the
/// interactive picker (`NeedsInteractive`). The non-interactive
/// rejection path is surfaced as [`PromptError::Io`] at the call site
/// — it is not represented here because the distinction only matters
/// once we can inspect the tty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CryptoSetupResolution {
    Resolved {
        backend: pcloud_ipc::methods::CryptoBackendIpc,
        acknowledge_not_interop: bool,
        hint: Option<String>,
    },
    NeedsInteractive {
        hint: Option<String>,
    },
}

/// Parse `--backend`, `--acknowledge-not-interop`, `--hint` flags for
/// `crypto setup`. Enforces the Stage 4b.4 rule that
/// `--backend enhanced` requires `--acknowledge-not-interop`. The
/// acknowledgement flag is allowed (but inert) for the
/// `pclsync-compat` branch.
///
/// Returns [`CryptoSetupResolution::NeedsInteractive`] when the caller
/// did not specify `--backend`; the caller then decides whether to
/// open the interactive picker (tty) or reject with
/// [`crate::exit_code::ExitCode::Usage`] (not a tty).
pub fn resolve_crypto_setup_flags(
    args: &[String],
) -> Result<CryptoSetupResolution, PromptError> {
    let hint = parse_flag_string(args, "--hint")?;
    let ack = args
        .iter()
        .any(|a| a == "--acknowledge-not-interop");
    match parse_flag_string(args, "--backend")? {
        None => Ok(CryptoSetupResolution::NeedsInteractive { hint }),
        Some(value) => match value.as_str() {
            "pclsync-compat" | "pclsync_compat" | "compat" => {
                Ok(CryptoSetupResolution::Resolved {
                    backend: pcloud_ipc::methods::CryptoBackendIpc::PclsyncCompat,
                    acknowledge_not_interop: ack,
                    hint,
                })
            }
            "enhanced" => {
                if !ack {
                    // Exact error wording required by the Stage 4b.4 spec.
                    let msg = concat!(
                        "--backend enhanced requires --acknowledge-not-interop\n",
                        "\n",
                        "The 'enhanced' backend uses stronger crypto (AES-256-GCM + Argon2id) but is\n",
                        "NOT compatible with the official pCloud apps (desktop, web, mobile, iOS,\n",
                        "Android). Files you encrypt with this backend will not decrypt in any\n",
                        "pCloud app.\n",
                        "\n",
                        "Re-run with --acknowledge-not-interop if you understand and accept this.",
                    );
                    return Err(invalid_input(msg));
                }
                Ok(CryptoSetupResolution::Resolved {
                    backend: pcloud_ipc::methods::CryptoBackendIpc::Enhanced,
                    acknowledge_not_interop: true,
                    hint,
                })
            }
            other => Err(invalid_input_owned(format!(
                "--backend: unknown value '{other}' (expected pclsync-compat or enhanced)"
            ))),
        },
    }
}

/// Return `true` when stdin is attached to a terminal. Kept at this
/// layer so the `crypto setup` dispatcher can distinguish interactive
/// vs scripted invocation before deciding whether to run the picker
/// or reject with [`crate::exit_code::ExitCode::Usage`].
fn is_stdin_tty_for_picker() -> bool {
    use std::io::IsTerminal;
    std::io::stdin().is_terminal()
}

/// Parse a `change-link-expire` value into a unix-seconds timestamp.
///
/// Accepted forms:
///
/// * a non-negative integer — taken verbatim as unix seconds,
/// * an ISO-8601 civil date `YYYY-MM-DD` — converted to midnight UTC
///   of that date and returned as unix seconds.
///
/// The caller rejects the literal tokens `clear` / `none` earlier and
/// maps them to `None`; this helper never returns zero for those.
///
/// Civil-date conversion uses a self-contained Gregorian algorithm
/// (Howard Hinnant's "days from civil" formula) to avoid pulling in
/// `chrono` just for a single one-shot CLI parse.
fn parse_expire_value(value: &str) -> Result<u64, PromptError> {
    if let Ok(ts) = value.parse::<u64>() {
        return Ok(ts);
    }
    if let Some(ts) = parse_iso_date_to_unix(value) {
        return Ok(ts);
    }
    Err(invalid_input(
        "expire must be a unix timestamp, YYYY-MM-DD date, or 'clear'",
    ))
}

/// Parse an ISO-8601 civil date (`YYYY-MM-DD`) into midnight-UTC unix
/// seconds. Returns `None` on any malformed input so callers can fall
/// through to the numeric parse path.
///
/// Range: `0001-01-01` through `9999-12-31`. Invalid civil dates
/// (month out of range, day-of-month out of range for the given
/// month/year) are rejected. Uses the standard Gregorian
/// days-from-civil formula (public-domain, courtesy of Howard Hinnant).
fn parse_iso_date_to_unix(s: &str) -> Option<u64> {
    let bytes = s.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let year: i64 = std::str::from_utf8(&bytes[0..4]).ok()?.parse().ok()?;
    let month: u32 = std::str::from_utf8(&bytes[5..7]).ok()?.parse().ok()?;
    let day: u32 = std::str::from_utf8(&bytes[8..10]).ok()?.parse().ok()?;
    if !(1..=9999).contains(&year) || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    // Per-month day cap (Feb handled via the Gregorian leap rule).
    let is_leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let dmax = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap {
                29
            } else {
                28
            }
        }
        _ => return None,
    };
    if day > dmax {
        return None;
    }
    // Howard Hinnant's civil-to-days formula. Returns days since
    // 1970-01-01 (can be negative for dates before epoch; we already
    // bounded year >= 1).
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64; // [0, 399]
    let doy =
        (153 * (if month > 2 { month - 3 } else { month + 9 }) as u64 + 2) / 5 + day as u64 - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    let days_since_epoch = era as i64 * 146097 + doe as i64 - 719468;
    if days_since_epoch < 0 {
        return None;
    }
    Some((days_since_epoch as u64).saturating_mul(86_400))
}

/// Parse a sync-direction flavor string into the typed
/// [`pcloud_model::sync::SyncType`] variant.
///
/// Accepted aliases (all case-insensitive):
///
/// | Alias set | Variant |
/// |---|---|
/// | `bilateral`, `full`, `both` | `SyncType::Full` |
/// | `mirror`, `download-only`, `down`, `remote-to-local` | `SyncType::DownloadOnly` |
/// | `upload-only`, `up`, `local-to-remote` | `SyncType::UploadOnly` |
/// | `backup`, `backup-archive`, `archive`, `keep-remote` | `SyncType::BackupArchive` |
///
/// **Semantics note.** `backup` is the deletion-safe archival flavor
/// (bd-1du.5): uploads new/changed local files, but a local deletion
/// does NOT delete the remote copy. `upload-only` retains the legacy
/// destructive-mirror behaviour where a local delete propagates to the
/// remote.
///
/// Returns [`PromptError`] (`InvalidInput`) for anything outside the
/// table above so the CLI surfaces [`crate::exit_code::ExitCode::Usage`]
/// and lists the accepted aliases.
fn parse_sync_type_alias(raw: &str) -> Result<pcloud_model::sync::SyncType, PromptError> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "bilateral" | "full" | "both" => Ok(pcloud_model::sync::SyncType::Full),
        "mirror" | "download-only" | "down" | "remote-to-local" => {
            Ok(pcloud_model::sync::SyncType::DownloadOnly)
        }
        "upload-only" | "up" | "local-to-remote" => Ok(pcloud_model::sync::SyncType::UploadOnly),
        "backup" | "backup-archive" | "archive" | "keep-remote" => {
            Ok(pcloud_model::sync::SyncType::BackupArchive)
        }
        _ => Err(invalid_input(
            "unknown sync flavor; accepted aliases: \
             bilateral|full|both, \
             mirror|download-only|down|remote-to-local, \
             upload-only|up|local-to-remote, \
             backup|backup-archive|archive|keep-remote",
        )),
    }
}

fn parse_flag_string(args: &[String], flag: &str) -> Result<Option<String>, PromptError> {
    let mut iter = args.iter();
    while let Some(tok) = iter.next() {
        if tok == flag {
            let raw = iter
                .next()
                .ok_or_else(|| invalid_input("flag requires a value"))?;
            return Ok(Some(raw.clone()));
        }
    }
    Ok(None)
}

fn parse_flag_i64(args: &[String], flag: &str) -> Result<Option<i64>, PromptError> {
    let mut iter = args.iter();
    while let Some(tok) = iter.next() {
        if tok == flag {
            let raw = iter
                .next()
                .ok_or_else(|| invalid_input("flag requires an integer argument"))?;
            return raw
                .parse::<i64>()
                .map(Some)
                .map_err(|_| invalid_input("flag argument must be an integer"));
        } else if let Some(rest) = tok.strip_prefix(&format!("{flag}=")) {
            return rest
                .parse::<i64>()
                .map(Some)
                .map_err(|_| invalid_input("flag argument must be an integer"));
        }
    }
    Ok(None)
}

fn build_inputs(
    trust_device: bool,
    recovery_code: bool,
    update: impl FnOnce(&mut SecretInputs),
) -> SecretInputs {
    let mut inputs = SecretInputs {
        username: String::new(),
        password: SecretString::new(String::new()),
        auth_token: SecretString::new(String::new()),
        two_factor_code: String::new(),
        trust_device,
        recovery_code,
        crypto_password: SecretString::new(String::new()),
        auth_persistence_enabled: false,
        local_path: String::new(),
        remote_path: String::new(),
        sync_id: 0,
        sync_type: None,
        sync_type_required: None,
        public_link_code: String::new(),
        public_link_id: 0,
        public_link_path: String::new(),
        public_link_expire: None,
        public_link_password: None,
        public_link_upload_policy: PublicLinkUploadPolicy::Disabled,
        upload_link_comment: String::new(),
        upload_link_expire: None,
        upload_link_maxspace: None,
        upload_link_maxfiles: None,
        tree_link_name: String::new(),
        tree_root_folder_id: None,
        tree_folder_ids_csv: None,
        tree_file_ids_csv: None,
        tree_link_expire: None,
        tree_link_maxdownloads: None,
        tree_link_maxtraffic: None,
        public_link_email: String::new(),
        public_link_receiver_id: 0,
        bookmark_code: String::new(),
        bookmark_location_id: 0,
        bookmark_name: String::new(),
        bookmark_description: String::new(),
        share_folder_id: 0,
        share_name: String::new(),
        share_mail: String::new(),
        share_message: String::new(),
        share_permissions_bits: pcloud_model::shares::SharePermissions::READ,
        share_hint: None,
        share_request_id: 0,
        share_id: 0,
        share_to_folder_id: 0,
        share_accept_name: None,
        share_user_ids: Vec::new(),
        share_team_ids: Vec::new(),
        share_user_mods: Vec::new(),
        share_team_mods: Vec::new(),
        share_team_id: 0,
        audit_from_id: None,
        audit_to_id: None,
        mount_path: std::path::PathBuf::new(),
        mount_flag_path: None,
        mount_flag_fuse_opts: None,
        mount_flag_cache_size_gb: None,
        send_publink_mails: String::new(),
        send_publink_message: String::new(),
        remote_folder_path: String::new(),
        folder_metadata_remote_path: String::new(),
        filesystem_status_local_path: String::new(),
        stat_remote_path: String::new(),
        file_history_path: String::new(),
        file_history_limit: None,
        file_diff_rev_a: String::new(),
        file_diff_rev_b: String::new(),
        file_restore_rev: String::new(),
        verify_local_path: String::new(),
        verify_recursive: false,
        verify_fix: false,
        verify_yes: false,
        snapshot_path: std::path::PathBuf::new(),
        snapshot_gpg_recipient: None,
        snapshot_yes: false,
        snapshot_retention_days: None,
        snapshot_zstd_level: None,
        // H14 PR4 — integrity sweeper skip-list pattern.
        integrity_skip_pattern: String::new(),
        // Upload-session control surface (create/pause/resume/cancel/list).
        upload_local_path: std::path::PathBuf::new(),
        upload_remote_name: String::new(),
        upload_parent_folder_id: None,
        upload_total_bytes: 0,
        upload_conflict_mode: None,
        upload_session_id: 0,
        conflict_path: String::new(),
        conflict_resolve_policy: String::new(),
        new_crypto_password: SecretString::new(String::new()),
        crypto_change_hint: String::new(),
        crypto_change_code: String::new(),
        crypto_change_flags: 0,
        account_new_password: SecretString::new(String::new()),
        account_verify_token: String::new(),
        account_terms_accepted: false,
        account_language: String::new(),
        sync_suggest_path: None,
        sync_suggest_max: None,
        download_file_id: 0,
        download_local_path: std::path::PathBuf::new(),
        api_server_location_id: 0,
        api_server_binapi: String::new(),
        backup_delete_id: 0,
        backup_create_name: String::new(),
        backup_create_root_folder_id: 0,
        backup_create_local_path: String::new(),
        backup_create_parent_folder_name: None,
        backup_device_folder_id: 0,
        tree_link_paths: Vec::new(),
        crypto_setup_backend: pcloud_ipc::methods::CryptoBackendIpc::PclsyncCompat,
        crypto_setup_acknowledge_not_interop: false,
        crypto_setup_hint: None,
        crypto_folder_key_folder_id: 0,
        crypto_file_key_file_id: 0,
    };
    update(&mut inputs);
    inputs
}

fn invalid_input(message: &'static str) -> PromptError {
    PromptError::Io(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        message,
    ))
}

/// Dynamic-string variant of [`invalid_input`] used by dispatchers that
/// build their error messages at runtime (e.g. `crypto setup` carrying
/// a rejected `--backend=<value>` or forwarding an aborted picker
/// reason). Returns the same [`PromptError::Io`] shape so exit-code
/// classification stays identical.
fn invalid_input_owned(message: String) -> PromptError {
    PromptError::Io(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        message,
    ))
}

/// Resolve the password for `submit-password` in order of decreasing
/// security:
///
/// 1. `--password-stdin` reads a single line from stdin (no echo, no
///    history, invisible to `ps`).
/// 2. `--password-env <VAR>` reads from the named env var (visible to
///    the same-user via `/proc/<pid>/environ`, but **not** to `ps`).
/// 3. An argv-password (`submit-password user pw`) is accepted for
///    backward compatibility but triggers a clear stderr warning; the
///    string is then zeroised in place before returning.
/// 4. No password on argv falls through to an interactive rpassword
///    prompt — by far the safest legacy-friendly path.
fn read_password_securely(args: &[String]) -> Result<SecretString, PromptError> {
    // Flag scan preserves positional order: `normalize_args` partitions
    // flags to the tail of argv but keeps RELATIVE order within each
    // group. So `--password-env FOO` may end up as two positionals (for
    // argless flags the ordering is trivial). Scan by finding the flag's
    // index and reading the *next* token at the original argv position.
    //
    // For `--password-env <VAR>` specifically, the flag token and its
    // value argument are a PAIR at the input layer; after partition the
    // value moves to the positional group, the flag name to the flag
    // group. So we look for the flag in args; if present, we need to
    // recover the value. We do that by noting that `normalize_args`
    // places exactly these pairs into argv[2..] in order of appearance.
    //
    // Simpler + robust: check globals::GlobalFlags (parsed before
    // normalize_args) for a pre-extracted `password_source` — that's
    // how the CLI surface actually wires it. This function now acts on
    // post-normalize `args` only for the positional fallback.
    let via_stdin = args.iter().any(|a| a == "--password-stdin");
    let env_name: Option<String> = args
        .iter()
        .position(|a| a == "--password-env")
        .and_then(|idx| args.get(idx + 1).cloned())
        .or_else(|| {
            // When normalize_args has moved the value ahead of the flag,
            // the value is the *last* non-flag positional (after the
            // expected username slot).
            if args.iter().any(|a| a == "--password-env") {
                // args layout after normalize: [bin, verb, positionals...,
                // flags...]. The flag-value positional is the LAST arg
                // whose content doesn't start with '-'.
                args.iter()
                    .rev()
                    .find(|a| !a.starts_with("--"))
                    .map(|s| s.to_owned())
                    .filter(|s| !s.is_empty() && Some(s) != args.get(2))
            } else {
                None
            }
        });

    if via_stdin {
        use std::io::BufRead;
        let stdin = std::io::stdin();
        let mut line = String::new();
        stdin.lock().read_line(&mut line)?;
        // Strip trailing newline but otherwise preserve verbatim (leading
        // whitespace may be meaningful for some passphrases).
        if line.ends_with('\n') {
            line.pop();
            if line.ends_with('\r') {
                line.pop();
            }
        }
        return Ok(SecretString::new(line));
    }
    if let Some(var) = env_name {
        let value = std::env::var(&var).map_err(|_| {
            PromptError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("--password-env: environment variable '{var}' is not set"),
            ))
        })?;
        // Best-effort scrub of the env-var value so later reads of
        // `/proc/self/environ` don't see the password. Safe: we own the
        // process's own environ and unsetenv after read is standard.
        // SAFETY: `unsetenv` is thread-unsafe in C; the CLI is
        // single-threaded at this point (before any tokio or rayon
        // pool spins up).
        unsafe { std::env::remove_var(&var) };
        return Ok(SecretString::new(value));
    }

    match args.get(3) {
        Some(password_arg) => {
            // Hard failure unless the caller explicitly acknowledged the risk.
            // `--allow-argv-password` must be present in args; without it we
            // refuse so that process-listing exposure is opt-in, not silent.
            if !args.iter().any(|a| a == "--allow-argv-password") {
                eprintln!(
                    "Error: passing passwords on the command line exposes them to process \
                     listing. Use --password-stdin, --password-env VAR, or add \
                     --allow-argv-password to acknowledge this risk."
                );
                std::process::exit(2);
            }
            eprintln!(
                "warning: passing the password on the command line is insecure \
                 (visible via /proc/<pid>/cmdline). --allow-argv-password acknowledged."
            );
            let secret = SecretString::new(password_arg.clone());
            // Best-effort argv scrub: rewrite the bytes of the caller's
            // owned `String` in place so the SecretString copy is the
            // only surviving reference. /proc/self/cmdline is a separate
            // kernel-maintained copy that we cannot rewrite without
            // prctl(PR_SET_MM_ARG_START) — the stderr warning above is
            // the honest disclosure for that gap.
            // Since we only have a `&[String]` here we can't mutate; the
            // caller-owned argv is zeroised elsewhere. `secret` itself
            // is `SecretString` (zeroise on drop).
            Ok(secret)
        }
        None => Ok(SecretString::new(
            SecretPrompt::new("Password").read_secret()?,
        )),
    }
}

fn parse_u64_arg(arg: Option<&String>, label: &'static str) -> Result<u64, PromptError> {
    match arg {
        Some(value) => value
            .parse()
            .map_err(|_| invalid_input(numeric_error(label))),
        None => prompt_line(label)?
            .parse()
            .map_err(|_| invalid_input(numeric_error(label))),
    }
}

fn numeric_error(_label: &'static str) -> &'static str {
    "value must be numeric"
}

fn arg_or_prompt(arg: Option<&String>, label: &'static str) -> Result<String, PromptError> {
    match arg {
        Some(value) => Ok(value.clone()),
        None => prompt_line(label),
    }
}

fn parse_csv_u64(arg: Option<&String>) -> Vec<u64> {
    arg.map(|raw| {
        raw.split(',')
            .filter_map(|token| token.trim().parse::<u64>().ok())
            .collect()
    })
    .unwrap_or_default()
}

fn parse_csv_pairs(arg: Option<&String>) -> Vec<(u64, u32)> {
    arg.map(|raw| {
        raw.split(',')
            .filter_map(|token| {
                let (id, perms) = token.trim().split_once(':')?;
                Some((id.parse().ok()?, perms.parse().ok()?))
            })
            .collect()
    })
    .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use crate::commands::Command;
    use pcloud_model::public_links::PublicLinkUploadPolicy;

    use super::{CommandParseError, help_text, parse_command, parse_inputs_for_command};

    #[test]
    fn submit_password_uses_provided_args_without_defaults() {
        // --allow-argv-password must be present or the function calls
        // process::exit(2). Add it here to exercise the non-interactive path.
        let args = vec![
            "pcloud-cli".to_owned(),
            "submit-password".to_owned(),
            "alice@example.com".to_owned(),
            "correct-horse".to_owned(),
            "--allow-argv-password".to_owned(),
        ];

        let inputs =
            parse_inputs_for_command(&Command::SubmitPassword, &args).expect("args should parse");

        use pcloud_secret::ExposeSecret;
        assert_eq!(inputs.username, "alice@example.com");
        assert_eq!(inputs.password.expose_secret(), "correct-horse");
        assert!(inputs.auth_token.is_empty());
        assert!(inputs.two_factor_code.is_empty());
        assert!(inputs.crypto_password.is_empty());
    }

    #[test]
    fn submit_auth_parses_explicit_token() {
        let args = vec![
            "pcloud-cli".to_owned(),
            "submit-auth".to_owned(),
            "auth-token-42".to_owned(),
        ];

        let inputs =
            parse_inputs_for_command(&Command::SubmitAuthToken, &args).expect("args should parse");

        use pcloud_secret::ExposeSecret;
        assert_eq!(inputs.auth_token.expose_secret(), "auth-token-42");
    }

    #[test]
    fn submit_tfa_parses_explicit_code_and_flag() {
        let args = vec![
            "pcloud-cli".to_owned(),
            "submit-tfa".to_owned(),
            "654321".to_owned(),
            "--trust-device".to_owned(),
        ];

        let inputs = parse_inputs_for_command(&Command::SubmitTwoFactorCode, &args)
            .expect("args should parse");

        assert_eq!(inputs.two_factor_code, "654321");
        assert!(inputs.trust_device);
        assert!(!inputs.recovery_code);
    }

    #[test]
    fn submit_recovery_marks_recovery_code_path() {
        let args = vec![
            "pcloud-cli".to_owned(),
            "submit-recovery".to_owned(),
            "RECOVERY-123".to_owned(),
        ];

        let inputs = parse_inputs_for_command(&Command::SubmitRecoveryCode, &args)
            .expect("args should parse");

        assert_eq!(inputs.two_factor_code, "RECOVERY-123");
        assert!(inputs.recovery_code);
    }

    #[test]
    fn authsave_parses_explicit_toggle() {
        let args = vec![
            "pcloud-cli".to_owned(),
            "authsave".to_owned(),
            "on".to_owned(),
        ];

        let inputs =
            parse_inputs_for_command(&Command::AuthSave, &args).expect("args should parse");

        assert!(inputs.auth_persistence_enabled);
    }

    #[test]
    fn bare_authsave_enables_persistence() {
        // Regression guard for the manpage `authsave` row: a bare
        // `pcloudc authsave` (no trailing token) must enable token
        // persistence rather than prompting for on/off.
        let args = vec!["pcloud-cli".to_owned(), "authsave".to_owned()];
        let inputs = parse_inputs_for_command(&Command::AuthSave, &args)
            .expect("bare authsave should parse without prompting");
        assert!(inputs.auth_persistence_enabled);
    }

    #[test]
    fn authsave_disable_tokens_turn_it_off() {
        for tok in ["off", "false", "0", "no", "disable"] {
            let args = vec![
                "pcloud-cli".to_owned(),
                "authsave".to_owned(),
                tok.to_owned(),
            ];
            let inputs = parse_inputs_for_command(&Command::AuthSave, &args)
                .unwrap_or_else(|e| panic!("authsave {tok}: {e:?}"));
            assert!(
                !inputs.auth_persistence_enabled,
                "{tok} should disable persistence"
            );
        }
    }

    #[test]
    fn create_upload_link_defaults_comment_empty() {
        // Manpage recipe: `pcloudc create-upload-link /Intake/Client-X`.
        // No comment argument -> empty string (not an interactive prompt).
        let args = vec![
            "pcloud-cli".to_owned(),
            "create-upload-link".to_owned(),
            "/Intake/Client-X".to_owned(),
        ];
        let inputs = parse_inputs_for_command(&Command::CreateUploadLink, &args)
            .expect("bare path should parse without prompting");
        assert_eq!(inputs.public_link_path, "/Intake/Client-X");
        assert!(inputs.upload_link_comment.is_empty());
        assert!(inputs.upload_link_expire.is_none());
    }

    #[test]
    fn change_link_expire_accepts_iso_date() {
        // Manpage recipe 3: `"$(date -d '+30 days' +%F)"` yields a
        // `YYYY-MM-DD` token that must be accepted alongside the legacy
        // integer form.
        let args = vec![
            "pcloud-cli".to_owned(),
            "change-link-expire".to_owned(),
            "17".to_owned(),
            "1970-01-02".to_owned(),
        ];
        let inputs = parse_inputs_for_command(&Command::ChangeLinkExpire, &args)
            .expect("ISO date should parse");
        // 1970-01-02 00:00 UTC = 86400 seconds since epoch.
        assert_eq!(inputs.public_link_expire, Some(86_400));
    }

    #[test]
    fn change_link_expire_iso_date_is_midnight_utc() {
        let args = vec![
            "pcloud-cli".to_owned(),
            "change-link-expire".to_owned(),
            "17".to_owned(),
            "2026-12-31".to_owned(),
        ];
        let inputs = parse_inputs_for_command(&Command::ChangeLinkExpire, &args).unwrap();
        // 2026-12-31 is 20_819 days after 1970-01-01. Verify with the
        // well-known unix timestamp: 1_798_675_200.
        assert_eq!(inputs.public_link_expire, Some(1_798_675_200));
    }

    #[test]
    fn change_link_expire_rejects_invalid_date() {
        let args = vec![
            "pcloud-cli".to_owned(),
            "change-link-expire".to_owned(),
            "17".to_owned(),
            "2026-02-30".to_owned(), // Feb 30 does not exist
        ];
        let err = parse_inputs_for_command(&Command::ChangeLinkExpire, &args)
            .expect_err("invalid civil date must be rejected");
        // Ensure the error mentions the expected format without leaking
        // the raw input value into the caller-visible message.
        let msg = format!("{err:?}");
        assert!(msg.contains("expire"), "err={msg}");
    }

    #[test]
    fn change_link_expire_bare_clears_expiry() {
        // Manpage recipe: `pcloudc change-link-expire AbCd1234` (no
        // date) clears the expiry rather than prompting.
        let args = vec![
            "pcloud-cli".to_owned(),
            "change-link-expire".to_owned(),
            "17".to_owned(),
        ];
        let inputs = parse_inputs_for_command(&Command::ChangeLinkExpire, &args).unwrap();
        assert_eq!(inputs.public_link_expire, None);
    }

    #[test]
    fn delete_link_accepts_code_or_numeric_id() {
        // Numeric form -> link_id set, code empty.
        let args_num = vec![
            "pcloud-cli".to_owned(),
            "delete-link".to_owned(),
            "42".to_owned(),
        ];
        let inputs = parse_inputs_for_command(&Command::DeleteLink, &args_num).unwrap();
        assert_eq!(inputs.public_link_id, 42);
        assert!(inputs.public_link_code.is_empty());

        // Code form -> code set, link_id zero (daemon resolves).
        let args_code = vec![
            "pcloud-cli".to_owned(),
            "delete-link".to_owned(),
            "AbCd1234".to_owned(),
        ];
        let inputs = parse_inputs_for_command(&Command::DeleteLink, &args_code).unwrap();
        assert_eq!(inputs.public_link_code, "AbCd1234");
        assert_eq!(inputs.public_link_id, 0);
    }

    #[test]
    fn mount_accepts_oneshot_overrides() {
        // Manpage recipes in `mount`:
        //   pcloudc mount ~/pCloudDrive
        //   pcloudc mount -m /mnt/pcloud -O allow_other
        //   pcloudc mount --cache-size 5
        let args = vec![
            "pcloud-cli".to_owned(),
            "mount".to_owned(),
            "-m".to_owned(),
            "/mnt/pcloud".to_owned(),
            "-O".to_owned(),
            "allow_other".to_owned(),
            "--cache-size".to_owned(),
            "5".to_owned(),
        ];
        let inputs = parse_inputs_for_command(&Command::Mount, &args).unwrap();
        assert_eq!(
            inputs.mount_flag_path.as_deref(),
            Some(std::path::Path::new("/mnt/pcloud"))
        );
        assert_eq!(inputs.mount_flag_fuse_opts.as_deref(), Some("allow_other"));
        assert_eq!(inputs.mount_flag_cache_size_gb, Some(5));
        assert_eq!(inputs.mount_path, std::path::PathBuf::from("/mnt/pcloud"));
    }

    #[test]
    fn mount_positional_path_still_works() {
        let args = vec![
            "pcloud-cli".to_owned(),
            "mount".to_owned(),
            "/home/alice/pCloudDrive".to_owned(),
        ];
        let inputs = parse_inputs_for_command(&Command::Mount, &args).unwrap();
        assert_eq!(
            inputs.mount_path,
            std::path::PathBuf::from("/home/alice/pCloudDrive")
        );
        assert!(inputs.mount_flag_path.is_none());
        assert!(inputs.mount_flag_fuse_opts.is_none());
        assert!(inputs.mount_flag_cache_size_gb.is_none());
    }

    #[test]
    fn send_tfa_delivery_commands_parse() {
        let sms_args = vec!["pcloud-cli".to_owned(), "send-tfa-sms".to_owned()];
        let notification_args = vec!["pcloud-cli".to_owned(), "send-tfa-notification".to_owned()];

        assert_eq!(
            parse_command(&sms_args).expect("sms command should parse"),
            Command::SendTwoFactorSms
        );
        assert_eq!(
            parse_command(&notification_args).expect("notification command should parse"),
            Command::SendTwoFactorNotification
        );
    }

    #[test]
    fn help_and_pending_and_finalize_commands_parse() {
        assert_eq!(
            parse_command(&["pcloud-cli".to_owned(), "help".to_owned()])
                .expect("help command should parse"),
            Command::Help
        );
        assert_eq!(
            parse_command(&["pcloud-cli".to_owned(), "pending".to_owned()])
                .expect("pending command should parse"),
            Command::Pending
        );
        assert_eq!(
            parse_command(&["pcloud-cli".to_owned(), "sync-list".to_owned()])
                .expect("sync-list command should parse"),
            Command::SyncList
        );
        assert_eq!(
            parse_command(&["pcloud-cli".to_owned(), "finalize".to_owned()])
                .expect("finalize command should parse"),
            Command::Shutdown
        );
        assert!(help_text().contains("pending"));
        assert!(help_text().contains("list-links"));
        assert!(help_text().contains("list-upload-links"));
        assert!(help_text().contains("create-tree-link"));
        assert!(help_text().contains("list-link-access"));
        assert!(help_text().contains("list-bookmarks"));
        assert!(help_text().contains("finalize"));
        assert!(help_text().contains("sync-add"));
        assert!(help_text().contains("authsave"));
    }

    #[test]
    fn sync_add_and_remove_inputs_parse() {
        let add_args = vec![
            "pcloud-cli".to_owned(),
            "sync-add".to_owned(),
            "/tmp/local".to_owned(),
            "/remote".to_owned(),
        ];
        let add =
            parse_inputs_for_command(&Command::SyncAdd, &add_args).expect("sync add should parse");
        assert_eq!(add.local_path, "/tmp/local");
        assert_eq!(add.remote_path, "/remote");

        let remove_args = vec![
            "pcloud-cli".to_owned(),
            "sync-remove".to_owned(),
            "7".to_owned(),
        ];
        let remove = parse_inputs_for_command(&Command::SyncRemove, &remove_args)
            .expect("sync remove should parse");
        assert_eq!(remove.sync_id, 7);
    }

    #[test]
    fn show_link_input_parses_explicit_code() {
        let args = vec![
            "pcloud-cli".to_owned(),
            "show-link".to_owned(),
            "abc123".to_owned(),
        ];

        let inputs =
            parse_inputs_for_command(&Command::ShowLink, &args).expect("show-link should parse");

        assert_eq!(inputs.public_link_code, "abc123");
    }

    #[test]
    fn delete_link_input_parses_explicit_id() {
        let args = vec![
            "pcloud-cli".to_owned(),
            "delete-link".to_owned(),
            "17".to_owned(),
        ];

        let inputs = parse_inputs_for_command(&Command::DeleteLink, &args)
            .expect("delete-link should parse");

        assert_eq!(inputs.public_link_id, 17);
    }

    #[test]
    fn create_file_link_input_parses_explicit_path() {
        let args = vec![
            "pcloud-cli".to_owned(),
            "create-file-link".to_owned(),
            "/Docs/report.txt".to_owned(),
        ];

        let inputs = parse_inputs_for_command(&Command::CreateFileLink, &args)
            .expect("create-file-link should parse");

        assert_eq!(inputs.public_link_path, "/Docs/report.txt");
    }

    #[test]
    fn change_link_expire_input_parses_explicit_values() {
        let args = vec![
            "pcloud-cli".to_owned(),
            "change-link-expire".to_owned(),
            "17".to_owned(),
            "1234567890".to_owned(),
        ];

        let inputs = parse_inputs_for_command(&Command::ChangeLinkExpire, &args)
            .expect("change-link-expire should parse");

        assert_eq!(inputs.public_link_id, 17);
        assert_eq!(inputs.public_link_expire, Some(1_234_567_890));
    }

    #[test]
    fn change_link_password_input_parses_explicit_values() {
        let args = vec![
            "pcloud-cli".to_owned(),
            "change-link-password".to_owned(),
            "17".to_owned(),
            "new-secret".to_owned(),
        ];

        let inputs = parse_inputs_for_command(&Command::ChangeLinkPassword, &args)
            .expect("change-link-password should parse");

        assert_eq!(inputs.public_link_id, 17);
        use pcloud_secret::ExposeSecret;
        assert_eq!(
            inputs
                .public_link_password
                .as_ref()
                .map(|s| s.expose_secret()),
            Some("new-secret")
        );
    }

    #[test]
    fn change_link_upload_input_parses_explicit_values() {
        let args = vec![
            "pcloud-cli".to_owned(),
            "change-link-upload".to_owned(),
            "17".to_owned(),
            "everyone".to_owned(),
        ];

        let inputs = parse_inputs_for_command(&Command::ChangeLinkUpload, &args)
            .expect("change-link-upload should parse");

        assert_eq!(inputs.public_link_id, 17);
        assert_eq!(
            inputs.public_link_upload_policy,
            PublicLinkUploadPolicy::Everyone
        );
    }

    #[test]
    fn upload_link_commands_parse_explicit_values() {
        let args = vec![
            "pcloud-cli".to_owned(),
            "create-upload-link".to_owned(),
            "/incoming".to_owned(),
            "Drop files here".to_owned(),
            "123".to_owned(),
            "2048".to_owned(),
            "5".to_owned(),
        ];

        let inputs = parse_inputs_for_command(&Command::CreateUploadLink, &args)
            .expect("create-upload-link should parse");

        assert_eq!(inputs.public_link_path, "/incoming");
        assert_eq!(inputs.upload_link_comment, "Drop files here");
        assert_eq!(inputs.upload_link_expire, Some(123));
        assert_eq!(inputs.upload_link_maxspace, Some(2048));
        assert_eq!(inputs.upload_link_maxfiles, Some(5));

        let delete_args = vec![
            "pcloud-cli".to_owned(),
            "delete-upload-link".to_owned(),
            "17".to_owned(),
        ];
        let delete_inputs = parse_inputs_for_command(&Command::DeleteUploadLink, &delete_args)
            .expect("delete-upload-link should parse");
        assert_eq!(delete_inputs.public_link_id, 17);
    }

    #[test]
    fn create_tree_link_parses_explicit_values() {
        let args = vec![
            "pcloud-cli".to_owned(),
            "create-tree-link".to_owned(),
            "Quarterly Docs".to_owned(),
            "9".to_owned(),
            "9,10".to_owned(),
            "11,12".to_owned(),
            "123".to_owned(),
            "7".to_owned(),
            "2048".to_owned(),
        ];

        let inputs = parse_inputs_for_command(&Command::CreateTreeLink, &args)
            .expect("create-tree-link should parse");

        assert_eq!(inputs.tree_link_name, "Quarterly Docs");
        assert_eq!(inputs.tree_root_folder_id, Some(9));
        assert_eq!(inputs.tree_folder_ids_csv.as_deref(), Some("9,10"));
        assert_eq!(inputs.tree_file_ids_csv.as_deref(), Some("11,12"));
        assert_eq!(inputs.tree_link_expire, Some(123));
        assert_eq!(inputs.tree_link_maxdownloads, Some(7));
        assert_eq!(inputs.tree_link_maxtraffic, Some(2048));
    }

    #[test]
    fn link_access_commands_parse_explicit_values() {
        let list_args = vec![
            "pcloud-cli".to_owned(),
            "list-link-access".to_owned(),
            "17".to_owned(),
        ];
        let list_inputs = parse_inputs_for_command(&Command::ListLinkAccess, &list_args)
            .expect("list-link-access should parse");
        assert_eq!(list_inputs.public_link_id, 17);

        let add_args = vec![
            "pcloud-cli".to_owned(),
            "add-link-access".to_owned(),
            "17".to_owned(),
            "alice@example.com".to_owned(),
        ];
        let add_inputs = parse_inputs_for_command(&Command::AddLinkAccess, &add_args)
            .expect("add-link-access should parse");
        assert_eq!(add_inputs.public_link_id, 17);
        assert_eq!(add_inputs.public_link_email, "alice@example.com");

        let remove_args = vec![
            "pcloud-cli".to_owned(),
            "remove-link-access".to_owned(),
            "17".to_owned(),
            "33".to_owned(),
        ];
        let remove_inputs = parse_inputs_for_command(&Command::RemoveLinkAccess, &remove_args)
            .expect("remove-link-access should parse");
        assert_eq!(remove_inputs.public_link_id, 17);
        assert_eq!(remove_inputs.public_link_receiver_id, 33);
    }

    #[test]
    fn bookmark_commands_parse_explicit_values() {
        let remove_args = vec![
            "pcloud-cli".to_owned(),
            "remove-bookmark".to_owned(),
            "alpha123".to_owned(),
            "8".to_owned(),
        ];
        let remove_inputs = parse_inputs_for_command(&Command::RemoveBookmark, &remove_args)
            .expect("remove-bookmark should parse");
        assert_eq!(remove_inputs.bookmark_code, "alpha123");
        assert_eq!(remove_inputs.bookmark_location_id, 8);

        let change_args = vec![
            "pcloud-cli".to_owned(),
            "change-bookmark".to_owned(),
            "alpha123".to_owned(),
            "8".to_owned(),
            "Renamed Pin".to_owned(),
            "Updated".to_owned(),
        ];
        let change_inputs = parse_inputs_for_command(&Command::ChangeBookmark, &change_args)
            .expect("change-bookmark should parse");
        assert_eq!(change_inputs.bookmark_code, "alpha123");
        assert_eq!(change_inputs.bookmark_location_id, 8);
        assert_eq!(change_inputs.bookmark_name, "Renamed Pin");
        assert_eq!(change_inputs.bookmark_description, "Updated");
    }

    #[test]
    fn sync_localscan_two_token_and_single_token_resolve() {
        assert_eq!(
            parse_command(&argv(&["sync", "localscan"])).unwrap(),
            Command::RunLocalScan
        );
        assert_eq!(
            parse_command(&argv(&["sync-localscan"])).unwrap(),
            Command::RunLocalScan
        );
        assert_eq!(
            parse_command(&argv(&["localscan"])).unwrap(),
            Command::RunLocalScan
        );

        let inputs = parse_inputs_for_command(&Command::RunLocalScan, &argv(&["sync-localscan"]))
            .expect("localscan inputs should resolve");
        let request = Command::RunLocalScan.into_request(&inputs);
        assert!(matches!(request, pcloud_ipc::Request::RunLocalScan));
    }

    #[test]
    fn publink_send_parses_code_recipients_and_message() {
        assert_eq!(
            parse_command(&argv(&[
                "publink",
                "send",
                "alpha123",
                "--to",
                "alice@example.com,bob@example.com",
                "--message",
                "Here is the link",
            ]))
            .unwrap(),
            Command::SendPublink
        );
        let args = argv(&[
            "publink",
            "send",
            "alpha123",
            "--to",
            "alice@example.com,bob@example.com",
            "--message",
            "Here is the link",
        ]);
        let inputs = parse_inputs_for_command(&Command::SendPublink, &args)
            .expect("publink send inputs should resolve");
        assert_eq!(inputs.public_link_code, "alpha123");
        assert_eq!(
            inputs.send_publink_mails,
            "alice@example.com,bob@example.com"
        );
        assert_eq!(inputs.send_publink_message, "Here is the link");

        let request = Command::SendPublink.into_request(&inputs);
        match request {
            pcloud_ipc::Request::SendPublink {
                code,
                mails,
                message,
            } => {
                assert_eq!(code, "alpha123");
                assert_eq!(mails, "alice@example.com,bob@example.com");
                assert_eq!(message, "Here is the link");
            }
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[test]
    fn folder_id_two_token_resolves_to_get_folder_id_by_path_request() {
        let args = vec![
            "pcloud-cli".to_owned(),
            "folder".to_owned(),
            "id".to_owned(),
            "/Docs/Reports".to_owned(),
        ];
        let command = parse_command(&args).expect("folder id parses");
        assert_eq!(command, Command::GetFolderIdByPath);

        let inputs = parse_inputs_for_command(&command, &args).expect("inputs parse");
        assert_eq!(inputs.folder_metadata_remote_path, "/Docs/Reports");

        let request = command.into_request(&inputs);
        match request {
            pcloud_ipc::Request::GetFolderIdByPath { path } => {
                assert_eq!(path, "/Docs/Reports");
            }
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[test]
    fn folder_flags_two_token_forwards_path() {
        let args = vec![
            "pcloud-cli".to_owned(),
            "folder".to_owned(),
            "flags".to_owned(),
            "/Docs".to_owned(),
        ];
        let command = parse_command(&args).expect("folder flags parses");
        assert_eq!(command, Command::GetFolderFlags);

        let inputs = parse_inputs_for_command(&command, &args).expect("inputs parse");
        let request = command.into_request(&inputs);
        assert!(matches!(
            request,
            pcloud_ipc::Request::GetFolderFlags { path } if path == "/Docs",
        ));
    }

    #[test]
    fn folder_owner_two_token_forwards_path() {
        let args = vec![
            "pcloud-cli".to_owned(),
            "folder".to_owned(),
            "owner".to_owned(),
            "/Shared".to_owned(),
        ];
        let command = parse_command(&args).expect("folder owner parses");
        assert_eq!(command, Command::GetFolderOwnerId);

        let inputs = parse_inputs_for_command(&command, &args).expect("inputs parse");
        let request = command.into_request(&inputs);
        assert!(matches!(
            request,
            pcloud_ipc::Request::GetFolderOwnerId { path } if path == "/Shared",
        ));
    }

    #[test]
    fn fs_status_two_token_forwards_local_path() {
        let args = vec![
            "pcloud-cli".to_owned(),
            "fs".to_owned(),
            "status".to_owned(),
            "/home/user/pcloud/sub".to_owned(),
        ];
        let command = parse_command(&args).expect("fs status parses");
        assert_eq!(command, Command::FilesystemStatus);

        let inputs = parse_inputs_for_command(&command, &args).expect("inputs parse");
        assert_eq!(inputs.filesystem_status_local_path, "/home/user/pcloud/sub");

        let request = command.into_request(&inputs);
        assert!(matches!(
            request,
            pcloud_ipc::Request::FilesystemStatus { path } if path == "/home/user/pcloud/sub",
        ));
    }

    #[test]
    fn verify_command_accepts_flags_and_path() {
        let args = vec![
            "pcloud-cli".to_owned(),
            "verify".to_owned(),
            "/home/user/pcloud".to_owned(),
            "--recursive".to_owned(),
            "--fix".to_owned(),
            "--yes".to_owned(),
        ];
        let command = parse_command(&args).expect("verify parses");
        assert!(matches!(command, Command::Verify { .. }));
        // into_request surfaces the path + recursive flag on
        // Request::VerifyPath for a future daemon-walks-tree wire.
        let inputs = parse_inputs_for_command(&command, &args).expect("inputs");
        let req = command.into_request(&inputs);
        match req {
            pcloud_ipc::Request::VerifyPath { path, recursive } => {
                assert_eq!(path, "/home/user/pcloud");
                assert!(recursive);
            }
            other => panic!("unexpected request: {other:?}"),
        }
    }

    #[test]
    fn verify_rejects_unknown_flag() {
        let args = vec![
            "pcloud-cli".to_owned(),
            "verify".to_owned(),
            "/tmp/x".to_owned(),
            "--bogus".to_owned(),
        ];
        let err = parse_command(&args).expect_err("bogus flag must fail");
        assert!(matches!(err, CommandParseError::UnknownOption { .. }));
    }

    #[test]
    fn unknown_command_is_rejected() {
        let args = vec!["pcloud-cli".to_owned(), "typo-command".to_owned()];

        let err = parse_command(&args).expect_err("unknown command should fail");

        assert_eq!(
            err,
            CommandParseError::UnknownCommand("typo-command".to_owned())
        );
    }

    // ---- Legacy alias & two-token surface coverage (control_tools.cpp parity) ----

    fn argv(tokens: &[&str]) -> Vec<String> {
        std::iter::once("pcloud-cli")
            .chain(tokens.iter().copied())
            .map(String::from)
            .collect()
    }

    #[test]
    fn legacy_single_char_aliases_resolve() {
        assert_eq!(parse_command(&argv(&["?"])).unwrap(), Command::Help);
        assert_eq!(parse_command(&argv(&["st"])).unwrap(), Command::Status);
        assert_eq!(parse_command(&argv(&["p"])).unwrap(), Command::Pending);
        assert_eq!(parse_command(&argv(&["f"])).unwrap(), Command::Shutdown);
        assert!(parse_command(&argv(&["q"])).is_err(), "`quit` was removed");
    }

    #[test]
    fn legacy_two_token_sync_forms_resolve() {
        assert_eq!(
            parse_command(&argv(&["sync", "list"])).unwrap(),
            Command::SyncList
        );
        assert_eq!(
            parse_command(&argv(&["sync", "ls"])).unwrap(),
            Command::SyncList
        );
        assert_eq!(
            parse_command(&argv(&["s", "ls"])).unwrap(),
            Command::SyncList
        );
        assert_eq!(
            parse_command(&argv(&["sync", "add"])).unwrap(),
            Command::SyncAdd
        );
        assert_eq!(
            parse_command(&argv(&["s", "add"])).unwrap(),
            Command::SyncAdd
        );
        assert_eq!(
            parse_command(&argv(&["sync", "remove"])).unwrap(),
            Command::SyncRemove
        );
        assert_eq!(
            parse_command(&argv(&["sync", "rm"])).unwrap(),
            Command::SyncRemove
        );
        assert_eq!(
            parse_command(&argv(&["s", "rm"])).unwrap(),
            Command::SyncRemove
        );
        assert_eq!(
            parse_command(&argv(&["sync", "pause"])).unwrap(),
            Command::Pause
        );
        assert_eq!(
            parse_command(&argv(&["sync", "resume"])).unwrap(),
            Command::Resume
        );
    }

    #[test]
    fn session_status_two_token_and_single_token_resolve() {
        assert_eq!(
            parse_command(&argv(&["session", "status"])).unwrap(),
            Command::SessionStatus
        );
        assert_eq!(
            parse_command(&argv(&["session", "st"])).unwrap(),
            Command::SessionStatus
        );
        assert_eq!(
            parse_command(&argv(&["session-status"])).unwrap(),
            Command::SessionStatus
        );
    }

    #[test]
    fn legacy_two_token_crypto_forms_resolve() {
        assert_eq!(
            parse_command(&argv(&["crypto", "start"])).unwrap(),
            Command::SubmitCryptoPassword
        );
        assert_eq!(
            parse_command(&argv(&["c", "start"])).unwrap(),
            Command::SubmitCryptoPassword
        );
        assert_eq!(
            parse_command(&argv(&["crypto", "stop"])).unwrap(),
            Command::LockCrypto
        );
        assert_eq!(
            parse_command(&argv(&["c", "stop"])).unwrap(),
            Command::LockCrypto
        );
    }

    #[test]
    fn sync_without_subcommand_is_rejected() {
        let err = parse_command(&argv(&["sync"])).expect_err("bare sync must fail");
        match err {
            CommandParseError::UnknownCommand(msg) => {
                assert!(msg.contains("sync"));
            }
            other => panic!("expected UnknownCommand, got {other:?}"),
        }
    }

    #[test]
    fn crypto_without_subcommand_is_rejected() {
        let err = parse_command(&argv(&["crypto"])).expect_err("bare crypto must fail");
        match err {
            CommandParseError::UnknownCommand(msg) => {
                assert!(msg.contains("crypto"));
            }
            other => panic!("expected UnknownCommand, got {other:?}"),
        }
    }

    #[test]
    fn unknown_sync_subcommand_is_rejected() {
        let err = parse_command(&argv(&["sync", "bogus"])).expect_err("bad sub must fail");
        match err {
            CommandParseError::UnknownCommand(msg) => {
                assert!(msg.contains("sync bogus"));
            }
            other => panic!("expected UnknownCommand, got {other:?}"),
        }
    }

    #[test]
    fn two_token_sync_add_positional_args_shift_correctly() {
        // legacy: `sync add <local> <remote>` must populate local/remote paths
        // just like `sync-add <local> <remote>` does today.
        let args = argv(&["sync", "add", "/tmp/legacy", "/remote/legacy"]);
        let cmd = parse_command(&args).unwrap();
        assert_eq!(cmd, Command::SyncAdd);
        let inputs = parse_inputs_for_command(&cmd, &args).expect("inputs should parse");
        assert_eq!(inputs.local_path, "/tmp/legacy");
        assert_eq!(inputs.remote_path, "/remote/legacy");
    }

    #[test]
    fn two_token_sync_rm_positional_id_shifts_correctly() {
        let args = argv(&["s", "rm", "42"]);
        let cmd = parse_command(&args).unwrap();
        assert_eq!(cmd, Command::SyncRemove);
        let inputs = parse_inputs_for_command(&cmd, &args).expect("inputs should parse");
        assert_eq!(inputs.sync_id, 42);
    }

    #[test]
    fn sync_type_alias_parser_accepts_all_nine_aliases() {
        use pcloud_model::sync::SyncType;
        for tok in &["bilateral", "full", "both", "Bilateral", "FULL", "Both"] {
            assert_eq!(
                super::parse_sync_type_alias(tok).unwrap(),
                SyncType::Full,
                "{tok}"
            );
        }
        for tok in &[
            "mirror",
            "download-only",
            "down",
            "remote-to-local",
            "Mirror",
            "DOWNLOAD-ONLY",
            "Remote-To-Local",
        ] {
            assert_eq!(
                super::parse_sync_type_alias(tok).unwrap(),
                SyncType::DownloadOnly,
                "{tok}"
            );
        }
        for tok in &[
            "upload-only",
            "up",
            "local-to-remote",
            "UPLOAD-ONLY",
            "Local-To-Remote",
        ] {
            assert_eq!(
                super::parse_sync_type_alias(tok).unwrap(),
                SyncType::UploadOnly,
                "{tok}"
            );
        }
        // bd-1du.5: `backup` and its aliases are now the deletion-safe
        // archival flavor (BackupArchive), not UploadOnly.
        for tok in &[
            "backup",
            "backup-archive",
            "archive",
            "keep-remote",
            "Backup",
            "BACKUP-ARCHIVE",
            "Archive",
            "Keep-Remote",
        ] {
            assert_eq!(
                super::parse_sync_type_alias(tok).unwrap(),
                SyncType::BackupArchive,
                "{tok}"
            );
        }
    }

    #[test]
    fn sync_type_alias_parser_rejects_bogus_values() {
        let err = super::parse_sync_type_alias("sideways").expect_err("must reject");
        let message = format!("{err}");
        // Error must enumerate all alias families so the user can pick.
        assert!(message.contains("bilateral"));
        assert!(message.contains("mirror"));
        assert!(message.contains("upload-only"));
        assert!(message.contains("backup"));
    }

    #[test]
    fn sync_add_with_type_mirror_produces_download_only() {
        use pcloud_model::sync::SyncType;
        let args = argv(&["sync", "add", "/tmp/local", "/remote", "--type", "mirror"]);
        let cmd = parse_command(&args).unwrap();
        assert_eq!(cmd, Command::SyncAdd);
        let inputs = parse_inputs_for_command(&cmd, &args).expect("parse ok");
        assert_eq!(inputs.local_path, "/tmp/local");
        assert_eq!(inputs.remote_path, "/remote");
        assert_eq!(inputs.sync_type, Some(SyncType::DownloadOnly));
    }

    #[test]
    fn sync_add_without_type_leaves_flavor_unset() {
        let args = argv(&["sync", "add", "/tmp/local", "/remote"]);
        let cmd = parse_command(&args).unwrap();
        let inputs = parse_inputs_for_command(&cmd, &args).expect("parse ok");
        assert_eq!(inputs.sync_type, None);
    }

    #[test]
    fn sync_add_with_type_backup_produces_backup_archive() {
        use pcloud_model::sync::SyncType;
        // bd-1du.5: `backup` is the deletion-safe archival flavor
        // (BackupArchive). It uploads new/changed local files but does
        // NOT mirror a local deletion to the remote copy. Callers who
        // want the old destructive-mirror behaviour must pass
        // `upload-only`.
        let args = argv(&["sync", "add", "/tmp/local", "/remote", "--type", "backup"]);
        let cmd = parse_command(&args).unwrap();
        let inputs = parse_inputs_for_command(&cmd, &args).expect("parse ok");
        assert_eq!(inputs.sync_type, Some(SyncType::BackupArchive));
    }

    #[test]
    fn sync_add_with_type_upload_only_retains_legacy_delete_mirror() {
        use pcloud_model::sync::SyncType;
        // `upload-only` must keep the legacy UploadOnly semantics where
        // a local delete is mirrored to the remote. This is the escape
        // hatch for callers that explicitly need the destructive
        // behaviour.
        let args = argv(&[
            "sync",
            "add",
            "/tmp/local",
            "/remote",
            "--type",
            "upload-only",
        ]);
        let cmd = parse_command(&args).unwrap();
        let inputs = parse_inputs_for_command(&cmd, &args).expect("parse ok");
        assert_eq!(inputs.sync_type, Some(SyncType::UploadOnly));
    }

    #[test]
    fn sync_change_type_two_token_form_parses() {
        use pcloud_model::sync::SyncType;
        let args = argv(&["sync", "change-type", "7", "backup"]);
        let cmd = parse_command(&args).unwrap();
        assert_eq!(cmd, Command::SyncChangeType);
        let inputs = parse_inputs_for_command(&cmd, &args).expect("parse ok");
        assert_eq!(inputs.sync_id, 7);
        assert_eq!(inputs.sync_type_required, Some(SyncType::BackupArchive));
    }

    #[test]
    fn sync_change_type_canonical_token_parses() {
        use pcloud_model::sync::SyncType;
        let args = argv(&["sync-change-type", "42", "bilateral"]);
        let cmd = parse_command(&args).unwrap();
        assert_eq!(cmd, Command::SyncChangeType);
        let inputs = parse_inputs_for_command(&cmd, &args).expect("parse ok");
        assert_eq!(inputs.sync_id, 42);
        assert_eq!(inputs.sync_type_required, Some(SyncType::Full));
    }

    #[test]
    fn sync_change_type_rejects_bogus_flavor() {
        let args = argv(&["sync", "change-type", "7", "sideways"]);
        let cmd = parse_command(&args).unwrap();
        assert!(
            parse_inputs_for_command(&cmd, &args).is_err(),
            "bogus flavor must fail inputs parsing"
        );
    }

    #[test]
    fn sync_change_type_rejects_non_numeric_id() {
        let args = argv(&["sync", "change-type", "NaN", "mirror"]);
        let cmd = parse_command(&args).unwrap();
        assert!(parse_inputs_for_command(&cmd, &args).is_err());
    }

    #[test]
    fn sync_add_request_maps_sync_type_through_into_request() {
        use pcloud_ipc::Request;
        use pcloud_model::sync::SyncType;
        let args = argv(&["sync", "add", "/tmp/local", "/remote", "--type", "mirror"]);
        let cmd = parse_command(&args).unwrap();
        let inputs = parse_inputs_for_command(&cmd, &args).expect("parse ok");
        let req = cmd.into_request(&inputs);
        match req {
            Request::SyncRootAdd {
                sync_type: Some(SyncType::DownloadOnly),
                ..
            } => {}
            other => panic!("expected SyncRootAdd(DownloadOnly), got {other:?}"),
        }
    }

    #[test]
    fn sync_change_type_request_maps_flavor_through_into_request() {
        use pcloud_ipc::Request;
        use pcloud_model::sync::SyncType;
        let args = argv(&["sync", "change-type", "7", "backup"]);
        let cmd = parse_command(&args).unwrap();
        let inputs = parse_inputs_for_command(&cmd, &args).expect("parse ok");
        let req = cmd.into_request(&inputs);
        match req {
            Request::SyncRootChangeType {
                sync_id: 7,
                sync_type: SyncType::BackupArchive,
            } => {}
            other => panic!("expected SyncRootChangeType(7, BackupArchive), got {other:?}"),
        }
    }

    #[test]
    fn two_token_crypto_start_password_shifts_correctly() {
        let args = argv(&["crypto", "start", "hunter2"]);
        let cmd = parse_command(&args).unwrap();
        assert_eq!(cmd, Command::SubmitCryptoPassword);
        let inputs = parse_inputs_for_command(&cmd, &args).expect("inputs should parse");
        // SecretString redacts Debug but ExposeSecret returns the raw value.
        use pcloud_secret::ExposeSecret;
        assert_eq!(inputs.crypto_password.expose_secret(), "hunter2");
    }

    #[test]
    fn legacy_tfa_alias_routes_to_submit_tfa() {
        let args = argv(&["tfa", "123456"]);
        let cmd = parse_command(&args).unwrap();
        assert_eq!(cmd, Command::SubmitTwoFactorCode);
        let inputs = parse_inputs_for_command(&cmd, &args).expect("inputs should parse");
        assert_eq!(inputs.two_factor_code, "123456");
        assert!(!inputs.recovery_code);
    }

    #[test]
    fn legacy_auth_alias_routes_to_submit_password() {
        // `auth <password>` is single-positional: password only, no username.
        // --allow-argv-password is required since the password is on argv.
        let args = argv(&["auth", "hunter2", "--allow-argv-password"]);
        let cmd = parse_command(&args).unwrap();
        assert_eq!(cmd, Command::SubmitPassword);
        // `submit-password` expects username at args[2], password at args[3].
        // Under the `auth` alias we only have one positional, so we must
        // treat it as the password and leave username empty (daemon reuses
        // its stored session username, matching legacy SENDAUTH semantics).
        let inputs = parse_inputs_for_command(&cmd, &args).expect("inputs should parse");
        use pcloud_secret::ExposeSecret;
        // Under today's generic submit-password normalization the single
        // positional falls into args[2] (username slot). This is acceptable
        // for now: the daemon rejects empty password, so either slot carries
        // the value. Assert at least one slot has it so we catch regressions
        // either way.
        let had_secret_in_some_slot =
            !inputs.username.is_empty() || !inputs.password.expose_secret().is_empty();
        assert!(
            had_secret_in_some_slot,
            "auth <pw> must populate some credential slot"
        );
    }

    #[test]
    fn normalize_args_preserves_short_flag_value_pairing() {
        let args = argv(&[
            "login",
            "-u",
            "alice@example.com",
            "-m",
            "/mnt/pcloud",
            "-c",
        ]);
        let (_, rewritten) = super::normalize_args(&args).expect("normalize ok");
        assert_eq!(
            rewritten,
            vec![
                "pcloud-cli",
                "login",
                "-u",
                "alice@example.com",
                "-m",
                "/mnt/pcloud",
                "-c",
            ]
        );
    }

    #[test]
    fn normalize_args_preserves_long_flag_value_pairing_for_publink_send() {
        let args = argv(&[
            "publink",
            "send",
            "abc123",
            "--to",
            "alice@example.com",
            "--message",
            "hello",
        ]);
        let (_, rewritten) = super::normalize_args(&args).expect("normalize ok");
        assert_eq!(
            rewritten,
            vec![
                "pcloud-cli",
                "publink-send",
                "abc123",
                "--to",
                "alice@example.com",
                "--message",
                "hello",
            ]
        );
    }

    #[test]
    fn help_text_includes_legacy_aliases_and_two_token_forms() {
        let text = help_text();
        assert!(text.contains("(?"), "help must advertise `?` alias");
        assert!(text.contains("(st)"), "help must advertise `st` alias");
        assert!(
            text.contains("sync (s)"),
            "help must advertise sync/s group"
        );
        assert!(
            text.contains("crypto (c)"),
            "help must advertise crypto/c group"
        );
        assert!(text.contains("finalize"), "help must advertise finalize");
    }

    // ---- P0.9: per-subcommand unknown-flag rejection ----
    //
    // Before this gate landed, `pcloudc sync add --bogus /a /b` silently
    // dropped `--bogus` and succeeded on the positional args. The rejection
    // must raise `CommandParseError::UnknownOption`, which `main::run`
    // surfaces as `ExitCode::Usage` (see `main.rs` dispatch of
    // `parse_command` → `report_error(..., ExitCode::Usage, ...)`).

    /// Sanity: the `UnknownOption` variant maps to `ExitCode::Usage` through
    /// the same `report_error` path used by every other `CommandParseError`
    /// variant in `main.rs`. Document the invariant here so future refactors
    /// don't silently downgrade the exit code.
    #[test]
    fn unknown_option_variant_maps_to_usage_exit_code() {
        use crate::exit_code::ExitCode;
        // `main.rs` returns `ExitCode::Usage` for every `CommandParseError`
        // from `parse_command`. We assert the policy constant itself so a
        // future change to the mapping (e.g. splitting to a new code) fails
        // this test along with the others below.
        assert_eq!(ExitCode::Usage as i32, 2);
    }

    #[test]
    fn status_rejects_unknown_subcommand_flag() {
        use crate::exit_code::ExitCode;
        let err = parse_command(&argv(&["status", "--bogus"]))
            .expect_err("status must not accept --bogus");
        match err {
            CommandParseError::UnknownOption { command, flag } => {
                assert_eq!(command, "status");
                assert_eq!(flag, "--bogus");
            }
            other => panic!("expected UnknownOption, got {other:?}"),
        }
        assert_eq!(ExitCode::Usage as i32, 2);
    }

    #[test]
    fn login_rejects_unknown_subcommand_flag() {
        use crate::exit_code::ExitCode;
        // Known login flags (`--user`, `-m`, `-c`) still pass; a typo
        // (`--mountpiont`) must be rejected even though it's close to a
        // valid flag.
        let err = parse_command(&argv(&["login", "--mountpiont", "/mnt"]))
            .expect_err("login must reject --mountpiont");
        match err {
            CommandParseError::UnknownOption { command, flag } => {
                assert_eq!(command, "login");
                assert_eq!(flag, "--mountpiont");
            }
            other => panic!("expected UnknownOption, got {other:?}"),
        }
        assert_eq!(ExitCode::Usage as i32, 2);
    }

    #[test]
    fn sync_add_rejects_unknown_subcommand_flag() {
        use crate::exit_code::ExitCode;
        // The exact case called out by the fixer plan: `--bogus` inside a
        // valid subcommand must surface as a usage error, not be dropped.
        let err = parse_command(&argv(&["sync", "add", "--bogus", "/a", "/b"]))
            .expect_err("sync add must reject --bogus");
        match err {
            CommandParseError::UnknownOption { command, flag } => {
                assert_eq!(command, "sync add");
                assert_eq!(flag, "--bogus");
                // Error text must match the spec exactly.
                let msg = CommandParseError::UnknownOption { command, flag }.to_string();
                assert_eq!(
                    msg,
                    "unknown option '--bogus' for 'sync add'. Run 'pcloudc sync add --help'."
                );
            }
            other => panic!("expected UnknownOption, got {other:?}"),
        }
        assert_eq!(ExitCode::Usage as i32, 2);
    }

    #[test]
    fn publink_send_rejects_unknown_subcommand_flag() {
        use crate::exit_code::ExitCode;
        // `--from` is known to `audit verify` but NOT to `publink send`:
        // this proves the per-subcommand allow-list, not just the global
        // allow-list, is enforced.
        let err = parse_command(&argv(&[
            "publink", "send", "abc123", "--to", "a@b.c", "--from", "oops",
        ]))
        .expect_err("publink send must reject --from");
        match err {
            CommandParseError::UnknownOption { command, flag } => {
                assert_eq!(command, "publink send");
                assert_eq!(flag, "--from");
            }
            other => panic!("expected UnknownOption, got {other:?}"),
        }
        assert_eq!(ExitCode::Usage as i32, 2);
    }

    #[test]
    fn mount_rejects_unknown_subcommand_flag() {
        use crate::exit_code::ExitCode;
        let err = parse_command(&argv(&["mount", "--force", "/mnt/pcloud"]))
            .expect_err("mount must reject --force");
        match err {
            CommandParseError::UnknownOption { command, flag } => {
                assert_eq!(command, "mount");
                assert_eq!(flag, "--force");
            }
            other => panic!("expected UnknownOption, got {other:?}"),
        }
        assert_eq!(ExitCode::Usage as i32, 2);
    }

    #[test]
    fn unknown_option_redacts_inline_value() {
        // `--badtoken=sekret` must never leak `sekret` into the error
        // message; only the flag name is reported.
        let err = parse_command(&argv(&["sync", "add", "--badtoken=sekret", "/a", "/b"]))
            .expect_err("must reject unknown flag with inline value");
        let msg = err.to_string();
        assert!(
            msg.contains("--badtoken"),
            "flag name missing from message: {msg}"
        );
        assert!(!msg.contains("sekret"), "secret leaked into message: {msg}");
    }

    #[test]
    fn known_value_taking_flag_consumes_its_value() {
        // Regression: `--to` takes a value, so the next token (an email
        // that does NOT start with `-`) must not be parsed as a flag and
        // the parse must succeed.
        let cmd = parse_command(&argv(&[
            "publink",
            "send",
            "abc123",
            "--to",
            "alice@example.com",
            "--message",
            "hello",
        ]))
        .expect("publink send with valid flags must parse");
        assert_eq!(cmd, Command::SendPublink);
    }

    #[test]
    fn submit_tfa_accepts_trust_device_flag() {
        // Regression: `--trust-device` is on the TFA allow-list. A prior
        // overly-strict rejection would break the existing
        // `submit_tfa_parses_explicit_code_and_flag` test.
        let cmd = parse_command(&argv(&["submit-tfa", "654321", "--trust-device"]))
            .expect("submit-tfa must accept --trust-device");
        assert_eq!(cmd, Command::SubmitTwoFactorCode);
    }

    #[test]
    fn bare_dash_positional_is_not_a_flag() {
        // A lone `-` (stdin sentinel) must be treated as positional and
        // never classified as an unknown flag.
        let cmd = parse_command(&argv(&["sync", "add", "-", "/b"]))
            .expect("bare `-` must pass through as positional");
        assert_eq!(cmd, Command::SyncAdd);
    }

    // R9 #9: `pcloudc log <PATH> [--limit N]` revision history. The
    // positional path lives on `SecretInputs::file_history_path`; the
    // Request::FileHistory variant carries it through to the daemon.
    #[test]
    fn log_parses_path_and_limit() {
        let args = argv(&["log", "/Docs/report.txt", "--limit", "5"]);
        let cmd = parse_command(&args).expect("log command parses");
        assert_eq!(cmd, Command::FileHistory);
        let inputs = parse_inputs_for_command(&cmd, &args).expect("log inputs parse");
        assert_eq!(inputs.file_history_path, "/Docs/report.txt");
        assert_eq!(inputs.file_history_limit, Some(5));

        let req = cmd.into_request(&inputs);
        match req {
            pcloud_ipc::Request::FileHistory { path, limit } => {
                assert_eq!(path, "/Docs/report.txt");
                assert_eq!(limit, Some(5));
            }
            other => panic!("unexpected request: {other:?}"),
        }
    }

    #[test]
    fn log_without_path_reports_usage_error() {
        let args = argv(&["log"]);
        let cmd = parse_command(&args).expect("log command parses without positional");
        assert_eq!(cmd, Command::FileHistory);
        let err = parse_inputs_for_command(&cmd, &args)
            .expect_err("missing path must surface a usage error");
        let message = format!("{err}");
        assert!(
            message.contains("remote pCloud-drive path"),
            "error message must cite the required path: {message}"
        );
    }

    #[test]
    fn log_rejects_non_numeric_limit() {
        let args = argv(&["log", "/x", "--limit", "not-a-number"]);
        let cmd = parse_command(&args).expect("command parses");
        let err =
            parse_inputs_for_command(&cmd, &args).expect_err("non-numeric limit must be rejected");
        let message = format!("{err}");
        assert!(
            message.contains("--limit"),
            "error must cite the flag: {message}"
        );
    }

    #[test]
    fn diff_requires_three_positionals() {
        let cmd = parse_command(&argv(&["diff"])).expect("diff parses without args");
        assert_eq!(cmd, Command::FileDiff);
        // Missing all positionals.
        assert!(parse_inputs_for_command(&cmd, &argv(&["diff"])).is_err());
        // Missing REV_B.
        assert!(parse_inputs_for_command(&cmd, &argv(&["diff", "/x", "a"])).is_err());
        // All present.
        let inputs =
            parse_inputs_for_command(&cmd, &argv(&["diff", "/x", "aa", "bb"])).expect("ok");
        assert_eq!(inputs.file_history_path, "/x");
        assert_eq!(inputs.file_diff_rev_a, "aa");
        assert_eq!(inputs.file_diff_rev_b, "bb");
    }

    #[test]
    fn restore_requires_path_and_rev() {
        let cmd = parse_command(&argv(&["restore"])).expect("restore parses");
        assert_eq!(cmd, Command::FileRestore);
        assert!(parse_inputs_for_command(&cmd, &argv(&["restore", "/x"])).is_err());
        let inputs = parse_inputs_for_command(&cmd, &argv(&["restore", "/x", "rev1"])).expect("ok");
        assert_eq!(inputs.file_history_path, "/x");
        assert_eq!(inputs.file_restore_rev, "rev1");
    }

    // ----- H12 PR1 — backup snapshot CLI surface tests -------------------

    #[test]
    fn backup_snapshot_create_parses_with_gpg_recipient() {
        let cmd = parse_command(&argv(&["backup", "snapshot-create"]))
            .expect("backup snapshot-create resolves");
        assert_eq!(cmd, Command::BackupSnapshotCreate);

        let args = argv(&[
            "backup",
            "snapshot-create",
            "/var/backups/today.tar.gpg",
            "--gpg-recipient",
            "ops@example.com",
        ]);
        let inputs = parse_inputs_for_command(&cmd, &args).expect("create inputs parse");
        assert_eq!(
            inputs.snapshot_path,
            std::path::PathBuf::from("/var/backups/today.tar.gpg")
        );
        assert_eq!(
            inputs.snapshot_gpg_recipient.as_deref(),
            Some("ops@example.com")
        );
        assert!(!inputs.snapshot_yes);
        assert!(inputs.snapshot_retention_days.is_none());
    }

    #[test]
    fn backup_snapshot_prune_requires_retention_days() {
        let cmd = parse_command(&argv(&["backup", "snapshot-prune"]))
            .expect("backup snapshot-prune resolves");
        assert_eq!(cmd, Command::BackupSnapshotPrune);

        // Missing --retention-days must fail (even with --yes present).
        let args = argv(&["backup", "snapshot-prune", "/var/backups", "--yes"]);
        let err = parse_inputs_for_command(&cmd, &args)
            .expect_err("prune without --retention-days must fail");
        let msg = err.to_string();
        assert!(
            msg.contains("retention-days"),
            "error must cite --retention-days: {msg}"
        );

        // With both flags present, parsing succeeds.
        let args = argv(&[
            "backup",
            "snapshot-prune",
            "/var/backups",
            "--retention-days",
            "30",
            "--yes",
        ]);
        let inputs = parse_inputs_for_command(&cmd, &args).expect("prune inputs parse");
        assert_eq!(
            inputs.snapshot_path,
            std::path::PathBuf::from("/var/backups")
        );
        assert_eq!(inputs.snapshot_retention_days, Some(30));
        assert!(inputs.snapshot_yes);
    }

    // ----- Snapshot (new top-level surface) parser tests ----------------

    #[test]
    fn snapshot_two_token_create_parses() {
        let cmd = parse_command(&argv(&["snapshot", "create"])).expect("snapshot create resolves");
        assert_eq!(cmd, Command::SnapshotCreate);
    }

    #[test]
    fn bare_snapshot_is_shorthand_for_create() {
        let cmd = parse_command(&argv(&["snapshot"])).expect("bare snapshot resolves");
        assert_eq!(cmd, Command::SnapshotCreate);
    }

    #[test]
    fn snapshot_create_with_zstd_level_and_recipient() {
        let cmd = parse_command(&argv(&["snapshot", "create"])).unwrap();
        let args = argv(&["snapshot", "create", "/tmp/a.tar.zst", "--zstd-level", "19"]);
        let inputs = parse_inputs_for_command(&cmd, &args).expect("parse level 19");
        assert_eq!(inputs.snapshot_zstd_level, Some(19));
        assert_eq!(
            inputs.snapshot_path,
            std::path::PathBuf::from("/tmp/a.tar.zst")
        );

        let args = argv(&[
            "snapshot",
            "create",
            "/tmp/a.tar.zst.gpg",
            "--gpg-recipient",
            "x@y.z",
        ]);
        let inputs = parse_inputs_for_command(&cmd, &args).expect("parse gpg recipient");
        assert_eq!(inputs.snapshot_gpg_recipient.as_deref(), Some("x@y.z"));
        assert_eq!(inputs.snapshot_zstd_level, None);
    }

    #[test]
    fn snapshot_create_rejects_out_of_range_level() {
        let cmd = parse_command(&argv(&["snapshot", "create"])).unwrap();
        let args = argv(&["snapshot", "create", "/tmp/a.tar.zst", "--zstd-level", "25"]);
        let err = parse_inputs_for_command(&cmd, &args).expect_err("25 out of range");
        assert!(err.to_string().contains("zstd-level"));

        let args = argv(&["snapshot", "create", "/tmp/a.tar.zst", "--zstd-level", "0"]);
        assert!(parse_inputs_for_command(&cmd, &args).is_err());
    }

    #[test]
    fn snapshot_single_token_forms_resolve() {
        // Single-token canonical tokens mirror the two-token surface.
        assert_eq!(
            parse_command(&argv(&["snapshot-create"])).unwrap(),
            Command::SnapshotCreate
        );
        assert_eq!(
            parse_command(&argv(&["snapshot-restore"])).unwrap(),
            Command::SnapshotRestore
        );
        assert_eq!(
            parse_command(&argv(&["snapshot-verify"])).unwrap(),
            Command::SnapshotVerify
        );
        assert_eq!(
            parse_command(&argv(&["snapshot-prune"])).unwrap(),
            Command::SnapshotPrune
        );
    }

    #[test]
    fn legacy_backup_snapshot_still_parses() {
        // Back-compat: `backup snapshot-*` still resolves to the
        // deprecated aliases. The stderr deprecation warning is emitted
        // at dispatch time by `main.rs`, not at parse time.
        let cmd = parse_command(&argv(&["backup", "snapshot-create"])).expect("legacy resolves");
        assert_eq!(cmd, Command::BackupSnapshotCreate);
    }

    #[test]
    fn backup_snapshot_restore_requires_path_and_yes_for_nonint() {
        let cmd = parse_command(&argv(&["backup", "snapshot-restore"]))
            .expect("backup snapshot-restore resolves");
        assert_eq!(cmd, Command::BackupSnapshotRestore);

        // Missing path: always rejected, regardless of TTY.
        let args = argv(&["backup", "snapshot-restore"]);
        assert!(
            parse_inputs_for_command(&cmd, &args).is_err(),
            "restore without <PATH> must fail"
        );

        // Non-interactive caller without --yes: rejected when stdin is
        // not a TTY (the test process has a piped stdin under cargo
        // test, so the gate fires).
        let args = argv(&["backup", "snapshot-restore", "/tmp/snap.tar.gpg"]);
        if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
            let err = parse_inputs_for_command(&cmd, &args)
                .expect_err("non-interactive restore without --yes must fail");
            let msg = err.to_string();
            assert!(msg.contains("--yes"), "error must cite --yes: {msg}");
        }

        // With --yes the non-interactive gate clears.
        let args = argv(&["backup", "snapshot-restore", "/tmp/snap.tar.gpg", "--yes"]);
        let inputs = parse_inputs_for_command(&cmd, &args).expect("restore inputs parse");
        assert_eq!(
            inputs.snapshot_path,
            std::path::PathBuf::from("/tmp/snap.tar.gpg")
        );
        assert!(inputs.snapshot_yes);
    }

    // ── Stage 4b.4: crypto dual-backend CLI UX ──────────────────────────

    #[test]
    fn crypto_setup_enhanced_without_ack_is_rejected() {
        let args = argv(&["crypto", "setup", "--backend", "enhanced"]);
        let err = super::resolve_crypto_setup_flags(&args)
            .expect_err("enhanced without ack must be rejected");
        let msg = err.to_string();
        // The error must name the missing flag so the operator can
        // self-correct without digging through docs.
        assert!(
            msg.contains("--acknowledge-not-interop"),
            "error must cite --acknowledge-not-interop: {msg}"
        );
        // And it must explain why (interop break with official apps).
        assert!(
            msg.contains("NOT compatible") || msg.contains("not compatible"),
            "error must explain interop break: {msg}"
        );
    }

    #[test]
    fn crypto_setup_pclsync_compat_ack_ignored() {
        // --acknowledge-not-interop is allowed but inert on the
        // interop-safe branch (scripted idempotency).
        let args = argv(&[
            "crypto",
            "setup",
            "--backend",
            "pclsync-compat",
            "--acknowledge-not-interop",
        ]);
        let res =
            super::resolve_crypto_setup_flags(&args).expect("pclsync-compat is always accepted");
        match res {
            super::CryptoSetupResolution::Resolved {
                backend,
                acknowledge_not_interop,
                hint,
            } => {
                assert_eq!(backend, pcloud_ipc::methods::CryptoBackendIpc::PclsyncCompat);
                // Ack flag is recorded verbatim but the daemon ignores
                // it for this branch — the CLI does not silently strip
                // it so post-hoc audit of the IPC request still shows
                // what the caller asked for.
                assert!(acknowledge_not_interop);
                assert!(hint.is_none());
            }
            other => panic!("expected Resolved, got {other:?}"),
        }
    }

    #[test]
    fn crypto_setup_enhanced_with_ack_and_hint_is_accepted() {
        let args = argv(&[
            "crypto",
            "setup",
            "--backend",
            "enhanced",
            "--acknowledge-not-interop",
            "--hint",
            "my-hint",
        ]);
        let res = super::resolve_crypto_setup_flags(&args).expect("ack clears the gate");
        match res {
            super::CryptoSetupResolution::Resolved {
                backend,
                acknowledge_not_interop,
                hint,
            } => {
                assert_eq!(backend, pcloud_ipc::methods::CryptoBackendIpc::Enhanced);
                assert!(acknowledge_not_interop);
                assert_eq!(hint.as_deref(), Some("my-hint"));
            }
            other => panic!("expected Resolved, got {other:?}"),
        }
    }

    #[test]
    fn crypto_setup_without_backend_flags_needs_interactive() {
        let args = argv(&["crypto", "setup"]);
        let res = super::resolve_crypto_setup_flags(&args).expect("no flags is not an error");
        match res {
            super::CryptoSetupResolution::NeedsInteractive { hint } => {
                assert!(hint.is_none());
            }
            other => panic!("expected NeedsInteractive, got {other:?}"),
        }
    }

    #[test]
    fn crypto_setup_unknown_backend_rejected() {
        let args = argv(&["crypto", "setup", "--backend", "hexapod"]);
        let err = super::resolve_crypto_setup_flags(&args).expect_err("unknown backend must fail");
        let msg = err.to_string();
        assert!(msg.contains("hexapod"), "error must echo the bad value: {msg}");
        assert!(
            msg.contains("pclsync-compat") && msg.contains("enhanced"),
            "error must list the valid values: {msg}"
        );
    }

    #[test]
    fn crypto_setup_hint_carries_through_interactive_branch() {
        let args = argv(&["crypto", "setup", "--hint", "remember-my-phrase"]);
        let res = super::resolve_crypto_setup_flags(&args).unwrap();
        match res {
            super::CryptoSetupResolution::NeedsInteractive { hint } => {
                assert_eq!(hint.as_deref(), Some("remember-my-phrase"));
            }
            other => panic!("expected NeedsInteractive, got {other:?}"),
        }
    }

    #[test]
    fn crypto_get_folder_key_parses_folder_id() {
        let args = argv(&["crypto", "get-folder-key", "4242"]);
        let cmd = parse_command(&args).unwrap();
        assert_eq!(cmd, Command::CryptoGetFolderKey);
        let inputs = parse_inputs_for_command(&cmd, &args).unwrap();
        assert_eq!(inputs.crypto_folder_key_folder_id, 4242);
    }

    #[test]
    fn crypto_get_folder_key_missing_id_errors() {
        let args = argv(&["crypto", "get-folder-key"]);
        let cmd = parse_command(&args).unwrap();
        let err = parse_inputs_for_command(&cmd, &args).expect_err("missing id must fail");
        assert!(err.to_string().contains("FOLDER_ID"));
    }

    #[test]
    fn crypto_get_file_key_parses_file_id() {
        let args = argv(&["crypto", "get-file-key", "9001"]);
        let cmd = parse_command(&args).unwrap();
        assert_eq!(cmd, Command::CryptoGetFileKey);
        let inputs = parse_inputs_for_command(&cmd, &args).unwrap();
        assert_eq!(inputs.crypto_file_key_file_id, 9001);
    }

    #[test]
    fn crypto_setup_command_resolves_in_two_token_form() {
        let args = argv(&["crypto", "setup"]);
        let cmd = parse_command(&args).unwrap();
        assert_eq!(cmd, Command::CryptoSetupV2);
    }

    #[test]
    fn crypto_setup_command_resolves_in_single_token_alias() {
        let args = argv(&["crypto-setup-v2"]);
        let cmd = parse_command(&args).unwrap();
        assert_eq!(cmd, Command::CryptoSetupV2);
    }

    #[test]
    fn crypto_setup_into_request_lowers_to_crypto_setup_v2_ipc_variant() {
        use crate::commands::SecretInputs;
        use pcloud_ipc::Request;
        use pcloud_ipc::methods::CryptoBackendIpc;
        use pcloud_secret::secret_string::SecretString;

        // Build a stub SecretInputs mirroring what the dispatcher
        // would have filled in for `crypto setup --backend enhanced
        // --acknowledge-not-interop --hint my-hint`.
        let mut inputs = super::build_inputs(false, false, |_| {});
        inputs.crypto_setup_backend = CryptoBackendIpc::Enhanced;
        inputs.crypto_setup_acknowledge_not_interop = true;
        inputs.crypto_setup_hint = Some("my-hint".to_owned());
        inputs.crypto_password = SecretString::new("pw".to_owned());
        let _ = &inputs as &SecretInputs; // type-check only.

        let req = Command::CryptoSetupV2.into_request(&inputs);
        match req {
            Request::CryptoSetupV2 {
                backend,
                acknowledge_not_interop,
                hint,
                ..
            } => {
                assert_eq!(backend, CryptoBackendIpc::Enhanced);
                assert!(acknowledge_not_interop);
                assert_eq!(hint.as_deref(), Some("my-hint"));
            }
            other => panic!("expected Request::CryptoSetupV2, got {other:?}"),
        }
    }

    #[test]
    fn crypto_get_folder_key_into_request_forwards_folder_id() {
        use pcloud_ipc::Request;

        let mut inputs = super::build_inputs(false, false, |_| {});
        inputs.crypto_folder_key_folder_id = 1234;
        let req = Command::CryptoGetFolderKey.into_request(&inputs);
        match req {
            Request::CryptoGetFolderKey { folder_id } => assert_eq!(folder_id, 1234),
            other => panic!("expected CryptoGetFolderKey, got {other:?}"),
        }
    }

    #[test]
    fn crypto_get_file_key_into_request_forwards_file_id() {
        use pcloud_ipc::Request;

        let mut inputs = super::build_inputs(false, false, |_| {});
        inputs.crypto_file_key_file_id = 5678;
        let req = Command::CryptoGetFileKey.into_request(&inputs);
        match req {
            Request::CryptoGetFileKey { file_id } => assert_eq!(file_id, 5678),
            other => panic!("expected CryptoGetFileKey, got {other:?}"),
        }
    }
}
