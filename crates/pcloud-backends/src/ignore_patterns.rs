//! Filename glob matcher for the `ignorepatterns` setting.
//!
//! Mirrors the C implementation:
//!
//! * `psync_match_pattern` — `pclsync/plibs.c:136`
//! * `psync_is_lname_to_ignore` — `pclsync/psynclib.c:815`
//! * `psync_is_name_to_ignore` — `pclsync/psynclib.c:861`
//!
//! The C matcher accepts a shell-glob style pattern using `*` (zero or more
//! arbitrary characters) and `?` (exactly one character). Patterns are
//! compared against an ASCII-lowercased copy of the candidate filename.
//! Multiple patterns are stored in a single string separated by `;` and
//! each entry is whitespace-trimmed before matching. The C matcher does
//! **not** support character classes; this Rust port adds optional
//! `[abc]` / `[!abc]` (and `[a-z]`) character-class support as a strict
//! superset extension. Patterns that contain no `[` behave bit-exact like
//! the C matcher.
//!
//! Settings integration: the patterns string lives under the
//! `"ignorepatterns"` key in the daemon's settings KV store
//! (`pcloud-store::handle_settings_kv`). Callers that already have a
//! `StoreHandle` should fetch the string with
//! `handle.settings_kv().get_string("ignorepatterns")` and pass it to
//! [`is_name_ignored`] / [`is_local_path_ignored`].
//!
//! This module is intentionally pure (no I/O) so it composes cleanly with
//! the existing path-prefix helpers in [`crate::mount_discovery`].

// **PLATFORM:** all
// **GATING:** none (portable).

use std::path::Path;

/// Default value of the `ignorepatterns` setting, mirroring the C client's
/// `PSYNC_IGNORE_PATTERNS_DEFAULT` (`pclsync/psettings.h:241`). Sourced
/// here so call sites can fall back when the KV store has no entry.
pub const DEFAULT_IGNORE_PATTERNS_STR: &str = concat!(
    ".DS_Store;",
    ".DS_Store?;",
    ".AppleDouble;",
    "._*;",
    ".Spotlight-V100;",
    ".DocumentRevisions-V100;",
    ".TemporaryItems;",
    ".Trashes;",
    ".fseventsd;",
    "desktop.ini;",
    "Thumbs.db;",
    "$RECYCLE.BIN;",
    "*~;",
    "*.part;",
    "*.crdownload"
);

/// The C matcher key — `name` and `pattern` are byte slices because the C
/// implementation operates on `unsigned char` (Latin-1) and uses no
/// multi-byte awareness. The candidate is expected to already be
/// lowercased by the caller (the C entry point lowercases the name once
/// before iterating patterns).
///
/// Returns `true` if the entire `name` is consumed by `pattern`.
fn match_pattern(name: &[u8], pattern: &[u8]) -> bool {
    // Iterative star-handling with backtracking, equivalent to the
    // recursive C implementation but stack-safe for long inputs. The
    // semantics — `*` matches zero or more chars, `?` matches exactly one,
    // and `[...]` is the (extension) character class — are enforced
    // consistently with the C matcher for the `*`/`?`-only subset.
    let mut ni: usize = 0;
    let mut pi: usize = 0;
    let mut star_pi: Option<usize> = None;
    let mut star_ni: usize = 0;

    while ni < name.len() {
        if pi < pattern.len() {
            let pc = pattern[pi];
            match pc {
                b'*' => {
                    star_pi = Some(pi);
                    star_ni = ni;
                    pi += 1;
                    continue;
                }
                b'?' => {
                    pi += 1;
                    ni += 1;
                    continue;
                }
                b'[' => {
                    if let Some((matched, next_pi)) = try_char_class(pattern, pi, name[ni]) {
                        if matched {
                            pi = next_pi;
                            ni += 1;
                            continue;
                        }
                        // class did not match — fall through to backtrack
                    } else {
                        // Malformed class — treat `[` as a literal, like
                        // the C matcher would (no special handling).
                        if name[ni] == b'[' {
                            pi += 1;
                            ni += 1;
                            continue;
                        }
                    }
                }
                _ => {
                    if pc == name[ni] {
                        pi += 1;
                        ni += 1;
                        continue;
                    }
                }
            }
        }
        // Mismatch: backtrack to the most recent `*` if any.
        if let Some(spi) = star_pi {
            pi = spi + 1;
            star_ni += 1;
            ni = star_ni;
        } else {
            return false;
        }
    }
    // Consume any trailing `*`s.
    while pi < pattern.len() && pattern[pi] == b'*' {
        pi += 1;
    }
    pi == pattern.len()
}

/// Parse a `[...]` character class starting at `pattern[pi]` (which is
/// `b'['`). Returns `Some((matched, index_after_class))` on success or
/// `None` if the class is malformed (no closing `]`). Supports `[!...]`
/// negation and `[a-z]` ranges.
fn try_char_class(pattern: &[u8], pi: usize, ch: u8) -> Option<(bool, usize)> {
    debug_assert_eq!(pattern[pi], b'[');
    let mut i = pi + 1;
    let negate = i < pattern.len() && (pattern[i] == b'!' || pattern[i] == b'^');
    if negate {
        i += 1;
    }
    let class_start = i;
    let mut found = false;
    while i < pattern.len() && pattern[i] != b']' {
        let lo = pattern[i];
        if i + 2 < pattern.len() && pattern[i + 1] == b'-' && pattern[i + 2] != b']' {
            let hi = pattern[i + 2];
            let (a, b) = if lo <= hi { (lo, hi) } else { (hi, lo) };
            if ch >= a && ch <= b {
                found = true;
            }
            i += 3;
        } else {
            if lo == ch {
                found = true;
            }
            i += 1;
        }
    }
    if i >= pattern.len() {
        return None; // unterminated — caller treats as literal
    }
    // Empty class `[]` or `[!]` is malformed in POSIX; treat as literal.
    if i == class_start {
        return None;
    }
    Some((found ^ negate, i + 1))
}

/// Iterate semicolon-separated patterns, trimming ASCII whitespace from
/// each one (matches the C `isspace` strip in `psync_is_lname_to_ignore`).
fn iter_patterns(patterns: &str) -> impl Iterator<Item = &str> {
    patterns.split(';').map(str::trim).filter(|p| !p.is_empty())
}

/// Public matcher: returns `true` if `name` matches any pattern in the
/// semicolon-separated `patterns` string.
///
/// `name` is lowercased ASCII-only before matching, mirroring the C
/// implementation which uses `tolower` on raw bytes.
///
/// # Example
///
/// ```
/// use pcloud_backends::ignore_patterns::is_name_ignored;
/// // Semicolon-separated patterns; case-insensitive.
/// assert!(is_name_ignored(".DS_Store", "*.tmp;.ds_store"));
/// assert!(is_name_ignored("build.log", "*.log"));
/// assert!(!is_name_ignored("main.rs", "*.log;*.tmp"));
/// ```
#[must_use]
pub fn is_name_ignored(name: &str, patterns: &str) -> bool {
    let lowered: Vec<u8> = name.bytes().map(|b| b.to_ascii_lowercase()).collect();
    for pat in iter_patterns(patterns) {
        // Patterns themselves are stored lowercased by the C settings layer
        // (see `lower_patterns` in `pclsync/psettings.c`). To preserve the
        // intent regardless of how the value entered the KV store, also
        // lowercase the pattern bytes here.
        let pat_bytes: Vec<u8> = pat.bytes().map(|b| b.to_ascii_lowercase()).collect();
        if match_pattern(&lowered, &pat_bytes) {
            return true;
        }
    }
    false
}

/// Convenience helper: extract the final path component (the filename) and
/// run [`is_name_ignored`] on it. Paths whose terminal component cannot be
/// extracted (e.g. `/`) return `false`.
///
/// # Example
///
/// ```
/// use std::path::Path;
/// use pcloud_backends::ignore_patterns::is_local_path_ignored;
/// assert!(is_local_path_ignored(Path::new("/home/me/.DS_Store"), ".ds_store"));
/// assert!(!is_local_path_ignored(Path::new("/home/me/src/main.rs"), "*.log"));
/// ```
#[must_use]
pub fn is_local_path_ignored(path: &Path, patterns: &str) -> bool {
    let Some(name) = path.file_name() else {
        return false;
    };
    let s = name.to_string_lossy();
    is_name_ignored(&s, patterns)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_pattern_string_never_matches() {
        assert!(!is_name_ignored("anything", ""));
        assert!(!is_name_ignored("anything", "   ;  ;\t"));
    }

    #[test]
    fn empty_name_matches_only_star_patterns() {
        // Mirrors C: psync_match_pattern("", "*", 1) returns 1.
        assert!(is_name_ignored("", "*"));
        assert!(!is_name_ignored("", "abc"));
        assert!(!is_name_ignored("", "?"));
    }

    #[test]
    fn literal_single_pattern() {
        assert!(is_name_ignored("Thumbs.db", "thumbs.db"));
        assert!(is_name_ignored("THUMBS.DB", "Thumbs.db"));
        assert!(!is_name_ignored("thumbs.dbx", "thumbs.db"));
    }

    #[test]
    fn star_matches_zero_or_more() {
        assert!(is_name_ignored("foo.part", "*.part"));
        assert!(is_name_ignored(".part", "*.part"));
        assert!(is_name_ignored("a.b.part", "*.part"));
        assert!(!is_name_ignored("foo.parts", "*.part"));
        assert!(is_name_ignored("anyfile~", "*~"));
    }

    #[test]
    fn question_mark_matches_exactly_one() {
        assert!(is_name_ignored(".DS_Store0", ".DS_Store?"));
        assert!(!is_name_ignored(".DS_Store", ".DS_Store?"));
        assert!(!is_name_ignored(".DS_Store12", ".DS_Store?"));
    }

    #[test]
    fn semicolon_split_multi_pattern() {
        let pats = "*.tmp; *.bak ;~$*";
        assert!(is_name_ignored("notes.tmp", pats));
        assert!(is_name_ignored("notes.bak", pats));
        assert!(is_name_ignored("~$report.docx", pats));
        assert!(!is_name_ignored("notes.txt", pats));
    }

    #[test]
    fn character_class_extension() {
        assert!(is_name_ignored("file.a", "*.[abc]"));
        assert!(is_name_ignored("file.b", "*.[abc]"));
        assert!(!is_name_ignored("file.d", "*.[abc]"));
        // range
        assert!(is_name_ignored("file7", "file[0-9]"));
        assert!(!is_name_ignored("filex", "file[0-9]"));
        // negation
        assert!(is_name_ignored("filex", "file[!0-9]"));
        assert!(!is_name_ignored("file3", "file[!0-9]"));
    }

    #[test]
    fn malformed_class_treated_literally() {
        // No closing bracket -> `[` matches literal `[`
        assert!(is_name_ignored("[abc", "[abc"));
        // Empty class -> literal
        assert!(!is_name_ignored("a", "[]"));
    }

    #[test]
    fn defaults_match_well_known_junk() {
        let p = DEFAULT_IGNORE_PATTERNS_STR;
        assert!(is_name_ignored(".DS_Store", p));
        assert!(is_name_ignored("desktop.ini", p));
        assert!(is_name_ignored("Thumbs.db", p));
        assert!(is_name_ignored("backup~", p));
        assert!(is_name_ignored("download.crdownload", p));
        assert!(is_name_ignored("._hidden", p));
        assert!(!is_name_ignored("normal.txt", p));
    }

    #[test]
    fn local_path_uses_basename() {
        let p = "*.tmp";
        assert!(is_local_path_ignored(Path::new("/var/log/foo.tmp"), p));
        assert!(!is_local_path_ignored(Path::new("/var/log/foo.txt"), p));
        assert!(!is_local_path_ignored(Path::new("/"), p));
    }

    /// C-equivalence vectors derived by tracing the
    /// `psync_match_pattern` algorithm in `pclsync/plibs.c:136-167`.
    /// Each tuple is `(name, pattern, expected)`; expectations were
    /// computed by hand-executing the C state machine.
    #[test]
    fn c_equivalence_vectors() {
        // (name, pattern, expected)
        let cases: &[(&str, &str, bool)] = &[
            ("abc", "abc", true),
            ("abc", "ab", false),
            ("ab", "abc", false),
            ("abc", "a?c", true),
            ("ac", "a?c", false),
            ("abc", "a*c", true),
            ("ac", "a*c", true),
            ("abxyzc", "a*c", true),
            ("abxyz", "a*c", false),
            ("abc", "*", true),
            ("", "*", true),
            ("abc", "***", true),
            ("abc", "a**c", true),
            ("abc", "*c", true),
            ("abc", "a*", true),
            ("abcdef", "a*c*f", true),
            ("abcdeg", "a*c*f", false),
            ("foo.tar.gz", "*.gz", true),
            ("foo.tar.gz", "*.tar.gz", true),
            ("foo.tar.gz", "foo.*.gz", true),
            ("foo.tgz", "*.tar.gz", false),
        ];
        for (name, pat, want) in cases {
            let got = is_name_ignored(name, pat);
            assert_eq!(got, *want, "name={name:?} pattern={pat:?}");
        }
    }
}
