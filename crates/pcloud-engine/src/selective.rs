//! Selective sync include/exclude pattern policy (parity item P4.7).
//!
//! # `.pcloudsync` file format
//!
//! Mirrors rclone-style filter files, one pattern per line:
//!
//! | Line shape          | Meaning                                         |
//! |---------------------|-------------------------------------------------|
//! | _blank_             | ignored                                         |
//! | `# …`               | comment, ignored                                |
//! | `!<pattern>`        | exclude glob (the leading `!` is stripped)      |
//! | `<pattern>`         | include glob                                    |
//!
//! Each pattern is compiled with [`globset`] and therefore supports
//! `*`, `?`, `[..]` character classes, and `**` for nested directories.
//! Leading/trailing whitespace is trimmed before compilation; a pattern
//! that becomes empty after stripping the `!` prefix is a no-op.
//!
//! # Evaluation: exclude-wins precedence (P4.7)
//!
//! `SelectivePolicy::matches` applies the following decision tree:
//!
//! 1. If the path matches **any** exclude glob → **not synced**.
//! 2. Otherwise, if there are **no** include globs → **synced**
//!    (default permissive: "sync everything except the excludes").
//! 3. Otherwise, if the path matches **any** include glob → **synced**.
//! 4. Otherwise → **not synced**.
//!
//! Paths are normalized by stripping a leading `/` before matching, so
//! policy authors can write `docs/*` or `/docs/*` interchangeably.
//!
//! # Defaults
//!
//! A missing `.pcloudsync` file yields `SelectivePolicy::allow_all`
//! — every path passes — preserving backward compatibility with sync
//! roots that predate the selective-sync feature.
//!
//! # Errors
//!
//! Parse errors carry the 1-based line number and the offending raw
//! pattern so end-user tooling can render a precise diagnostic. I/O
//! errors (other than `NotFound`, which is treated as "allow all") are
//! surfaced verbatim via `SelectiveError::Io`.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::fmt;
use std::fs;
use std::io;
use std::path::Path;

use globset::{Glob, GlobSet, GlobSetBuilder};

/// Compiled include/exclude glob policy for a single sync root.
///
/// Built once per sync root at scan time (or on `.pcloudsync` change)
/// and then consulted by [`crate::local_scan::LocalScanner::normalize_entries_filtered`]
/// for every candidate path. Cheap to clone; internally the globsets
/// are `Arc`-compatible via the `globset` crate.
#[derive(Debug, Clone)]
pub struct SelectivePolicy {
    includes: GlobSet,
    excludes: GlobSet,
    has_includes: bool,
    has_excludes: bool,
}

/// Errors produced while loading or parsing a `.pcloudsync` file.
#[derive(Debug)]
pub enum SelectiveError {
    /// The policy file could not be read.
    Io(io::Error),
    /// A pattern failed to compile as a glob.
    ParseError {
        /// 1-based line number of the offending pattern.
        line: usize,
        /// The raw pattern text.
        pattern: String,
        /// Underlying globset error message.
        message: String,
    },
}

impl fmt::Display for SelectiveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "selective policy I/O error: {err}"),
            Self::ParseError {
                line,
                pattern,
                message,
            } => write!(
                f,
                "selective policy parse error at line {line} ({pattern:?}): {message}"
            ),
        }
    }
}

impl std::error::Error for SelectiveError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            Self::ParseError { .. } => None,
        }
    }
}

impl From<io::Error> for SelectiveError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

impl Default for SelectivePolicy {
    fn default() -> Self {
        Self::allow_all()
    }
}

impl SelectivePolicy {
    /// Policy that syncs every path (no includes, no excludes).
    ///
    /// # Example
    ///
    /// ```
    /// use pcloud_engine::selective::SelectivePolicy;
    /// let policy = SelectivePolicy::allow_all();
    /// assert!(policy.matches("anything/goes.txt"));
    /// assert!(policy.matches(""));
    /// ```
    #[must_use]
    pub fn allow_all() -> Self {
        Self {
            includes: GlobSet::empty(),
            excludes: GlobSet::empty(),
            has_includes: false,
            has_excludes: false,
        }
    }

    /// Parse a `.pcloudsync` policy from an in-memory string. Primarily
    /// exposed for tests and callers who have already loaded the file.
    ///
    /// # Example
    ///
    /// ```
    /// use pcloud_engine::selective::SelectivePolicy;
    /// let policy = SelectivePolicy::parse("\
    /// # comment
    /// docs/*
    /// !docs/secret.txt
    /// ").unwrap();
    /// assert!(policy.matches("docs/report.md"));
    /// assert!(!policy.matches("docs/secret.txt"));
    /// // Not in includes -> excluded.
    /// assert!(!policy.matches("other/file.txt"));
    /// ```
    pub fn parse(contents: &str) -> Result<Self, SelectiveError> {
        let mut includes = GlobSetBuilder::new();
        let mut excludes = GlobSetBuilder::new();
        let mut has_includes = false;
        let mut has_excludes = false;

        for (idx, raw_line) in contents.lines().enumerate() {
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (is_exclude, pattern_text) = if let Some(rest) = line.strip_prefix('!') {
                (true, rest.trim())
            } else {
                (false, line)
            };
            if pattern_text.is_empty() {
                continue;
            }
            let glob = Glob::new(pattern_text).map_err(|err| SelectiveError::ParseError {
                line: idx + 1,
                pattern: pattern_text.to_owned(),
                message: err.to_string(),
            })?;
            if is_exclude {
                excludes.add(glob);
                has_excludes = true;
            } else {
                includes.add(glob);
                has_includes = true;
            }
        }

        let includes = includes.build().map_err(|err| SelectiveError::ParseError {
            line: 0,
            pattern: String::new(),
            message: err.to_string(),
        })?;
        let excludes = excludes.build().map_err(|err| SelectiveError::ParseError {
            line: 0,
            pattern: String::new(),
            message: err.to_string(),
        })?;

        Ok(Self {
            includes,
            excludes,
            has_includes,
            has_excludes,
        })
    }

    /// Load a `.pcloudsync` file from disk. Returns a permissive
    /// [`SelectivePolicy::allow_all`] policy if the file does not exist.
    pub fn from_pcloudsync_file(path: &Path) -> Result<Self, SelectiveError> {
        match fs::read_to_string(path) {
            Ok(contents) => Self::parse(&contents),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(Self::allow_all()),
            Err(err) => Err(SelectiveError::Io(err)),
        }
    }

    /// Convenience loader that looks for `<sync_root>/.pcloudsync`.
    pub fn for_sync_root(sync_root: &Path) -> Result<Self, SelectiveError> {
        Self::from_pcloudsync_file(&sync_root.join(".pcloudsync"))
    }

    /// Returns `true` if the given forward-slash relative path should be
    /// synced under this policy. Exclusion always wins over inclusion.
    #[must_use]
    pub fn matches(&self, relative_path: &str) -> bool {
        let normalized = relative_path.trim_start_matches('/');
        if self.has_excludes && self.excludes.is_match(normalized) {
            return false;
        }
        if !self.has_includes {
            return true;
        }
        self.includes.is_match(normalized)
    }

    /// Whether any include glob was configured.
    #[must_use]
    pub fn has_includes(&self) -> bool {
        self.has_includes
    }

    /// Whether any exclude glob was configured.
    #[must_use]
    pub fn has_excludes(&self) -> bool {
        self.has_excludes
    }
}

#[cfg(test)]
mod tests {
    use super::{SelectiveError, SelectivePolicy};

    #[test]
    fn parse_simple_includes_excludes() {
        let policy = SelectivePolicy::parse(
            "# comment line\n\
             docs/*.md\n\
             !docs/secret.md\n\
             src/**/*.rs\n",
        )
        .expect("parses");

        assert!(policy.has_includes());
        assert!(policy.has_excludes());
        assert!(policy.matches("docs/readme.md"));
        assert!(policy.matches("src/lib.rs"));
        assert!(policy.matches("src/nested/mod.rs"));
        assert!(!policy.matches("docs/secret.md"));
        assert!(!policy.matches("bin/cli"));
    }

    #[test]
    fn exclude_wins_over_include() {
        let policy = SelectivePolicy::parse(
            "data/**\n\
             !data/secret.bin\n",
        )
        .expect("parses");

        assert!(policy.matches("data/public.bin"));
        assert!(!policy.matches("data/secret.bin"));
    }

    #[test]
    fn nested_path_glob_matching() {
        let policy = SelectivePolicy::parse(
            "**/*\n\
             !**/node_modules/**\n\
             !**/target/**\n",
        )
        .expect("parses");

        assert!(policy.matches("src/main.rs"));
        assert!(policy.matches("a/b/c/file.txt"));
        assert!(!policy.matches("frontend/node_modules/react/index.js"));
        assert!(!policy.matches("crates/foo/target/debug/bar"));
    }

    #[test]
    fn default_policy_syncs_everything_when_no_file() {
        let tmp =
            std::env::temp_dir().join(format!("pcloud-selective-missing-{}", std::process::id()));
        // Make sure the file does not exist.
        let missing = tmp.join(".pcloudsync");
        let policy = SelectivePolicy::from_pcloudsync_file(&missing).expect("missing is ok");
        assert!(!policy.has_includes());
        assert!(!policy.has_excludes());
        assert!(policy.matches("anything/goes.txt"));
        assert!(policy.matches("deep/nested/path/file.bin"));
    }

    #[test]
    fn invalid_pattern_returns_parseerror() {
        // Unclosed character class is a hard globset parse error.
        let err = SelectivePolicy::parse("docs/[unclosed\n").expect_err("must fail");
        match err {
            SelectiveError::ParseError {
                line,
                pattern,
                message,
            } => {
                assert_eq!(line, 1);
                assert_eq!(pattern, "docs/[unclosed");
                assert!(!message.is_empty());
            }
            other => panic!("expected ParseError, got {other:?}"),
        }
    }

    #[test]
    fn blank_lines_and_comments_ignored() {
        let policy = SelectivePolicy::parse(
            "\n   \n# a comment\n   # still a comment if trimmed? no, keep literal\n!\n!   \n",
        )
        .expect("parses");
        // Only empty excludes after strip — treated as no-op.
        assert!(!policy.has_includes());
        assert!(!policy.has_excludes());
        assert!(policy.matches("anything"));
    }
}
