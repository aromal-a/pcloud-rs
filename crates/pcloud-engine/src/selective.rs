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
    /// Raw include pattern strings, retained so a policy can be
    /// re-composed (e.g. layering on additional config-driven excludes
    /// from `SyncRootRecord.exclude_globs`).
    include_patterns: Vec<String>,
    /// Raw exclude pattern strings, retained for the same reason.
    exclude_patterns: Vec<String>,
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
            include_patterns: Vec::new(),
            exclude_patterns: Vec::new(),
        }
    }

    /// Build a policy whose only effect is to exclude paths matching any
    /// pattern in `patterns`. T1.1: lets `SyncRootRecord.exclude_globs`
    /// drive a config-source policy independently of the on-disk
    /// `.pcloudsync` file.
    ///
    /// Empty / whitespace-only patterns are silently skipped.
    ///
    /// # Errors
    ///
    /// Returns [`SelectiveError::ParseError`] if any pattern fails to
    /// compile as a glob; the offending pattern is reported with a
    /// 1-based index.
    pub fn from_exclude_patterns(patterns: &[String]) -> Result<Self, SelectiveError> {
        let mut excludes = GlobSetBuilder::new();
        let mut exclude_patterns = Vec::new();
        for (idx, pat) in patterns.iter().enumerate() {
            let trimmed = pat.trim();
            if trimmed.is_empty() {
                continue;
            }
            let glob = Glob::new(trimmed).map_err(|err| SelectiveError::ParseError {
                line: idx + 1,
                pattern: trimmed.to_owned(),
                message: err.to_string(),
            })?;
            excludes.add(glob);
            exclude_patterns.push(trimmed.to_owned());
        }
        let has_excludes = !exclude_patterns.is_empty();
        let excludes = excludes.build().map_err(|err| SelectiveError::ParseError {
            line: 0,
            pattern: String::new(),
            message: err.to_string(),
        })?;
        Ok(Self {
            includes: GlobSet::empty(),
            excludes,
            has_includes: false,
            has_excludes,
            include_patterns: Vec::new(),
            exclude_patterns,
        })
    }

    /// Compose `self` with additional exclude patterns. Includes from
    /// `self` are preserved; excludes are unioned. Used by the engine to
    /// layer `SyncRootRecord.exclude_globs` on top of the on-disk
    /// `.pcloudsync` policy without touching the file.
    ///
    /// # Errors
    ///
    /// Returns [`SelectiveError::ParseError`] if any added pattern fails
    /// to compile.
    pub fn with_additional_excludes(&self, patterns: &[String]) -> Result<Self, SelectiveError> {
        let mut combined_excludes: Vec<String> = self.exclude_patterns.clone();
        for pat in patterns {
            let trimmed = pat.trim();
            if !trimmed.is_empty() {
                combined_excludes.push(trimmed.to_owned());
            }
        }
        let mut include_builder = GlobSetBuilder::new();
        for pat in &self.include_patterns {
            let glob = Glob::new(pat).map_err(|err| SelectiveError::ParseError {
                line: 0,
                pattern: pat.clone(),
                message: err.to_string(),
            })?;
            include_builder.add(glob);
        }
        let mut exclude_builder = GlobSetBuilder::new();
        for pat in &combined_excludes {
            let glob = Glob::new(pat).map_err(|err| SelectiveError::ParseError {
                line: 0,
                pattern: pat.clone(),
                message: err.to_string(),
            })?;
            exclude_builder.add(glob);
        }
        let includes = include_builder
            .build()
            .map_err(|err| SelectiveError::ParseError {
                line: 0,
                pattern: String::new(),
                message: err.to_string(),
            })?;
        let excludes = exclude_builder
            .build()
            .map_err(|err| SelectiveError::ParseError {
                line: 0,
                pattern: String::new(),
                message: err.to_string(),
            })?;
        Ok(Self {
            includes,
            excludes,
            has_includes: !self.include_patterns.is_empty(),
            has_excludes: !combined_excludes.is_empty(),
            include_patterns: self.include_patterns.clone(),
            exclude_patterns: combined_excludes,
        })
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
        let mut include_patterns: Vec<String> = Vec::new();
        let mut exclude_patterns: Vec<String> = Vec::new();

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
                exclude_patterns.push(pattern_text.to_owned());
            } else {
                includes.add(glob);
                has_includes = true;
                include_patterns.push(pattern_text.to_owned());
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
            include_patterns,
            exclude_patterns,
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
    fn from_exclude_patterns_excludes_only() {
        let policy =
            SelectivePolicy::from_exclude_patterns(&["*.tmp".to_owned(), "build/**".to_owned()])
                .expect("parses");
        assert!(!policy.has_includes());
        assert!(policy.has_excludes());
        assert!(!policy.matches("foo.tmp"));
        assert!(!policy.matches("build/release/x"));
        // No includes => default permissive for non-excluded paths.
        assert!(policy.matches("src/main.rs"));
    }

    #[test]
    fn from_exclude_patterns_skips_blank_entries() {
        let policy = SelectivePolicy::from_exclude_patterns(&[
            String::new(),
            "  ".to_owned(),
            "*.bak".to_owned(),
        ])
        .expect("parses");
        assert!(policy.has_excludes());
        assert!(!policy.matches("a.bak"));
    }

    #[test]
    fn from_exclude_patterns_invalid_returns_err() {
        let err = SelectivePolicy::from_exclude_patterns(&["[unclosed".to_owned()])
            .expect_err("must fail");
        match err {
            SelectiveError::ParseError { line, pattern, .. } => {
                assert_eq!(line, 1);
                assert_eq!(pattern, "[unclosed");
            }
            other => panic!("expected ParseError, got {other:?}"),
        }
    }

    #[test]
    fn with_additional_excludes_layers_on_top() {
        // Base policy from a `.pcloudsync` file with includes + one exclude.
        let base = SelectivePolicy::parse("docs/**\n!docs/secret.md\n").expect("parses");
        // Layer in config-driven excludes from `SyncRootRecord.exclude_globs`.
        let composed = base
            .with_additional_excludes(&["**/*.tmp".to_owned()])
            .expect("composes");

        // Original include + exclude still in effect.
        assert!(composed.matches("docs/readme.md"));
        assert!(!composed.matches("docs/secret.md"));
        // New exclude blocks `.tmp` even within the includes.
        assert!(!composed.matches("docs/scratch.tmp"));
        // Outside includes is still excluded by the baseline policy.
        assert!(!composed.matches("other/file.txt"));
    }

    #[test]
    fn with_additional_excludes_preserves_allow_all_when_no_includes() {
        let base = SelectivePolicy::allow_all();
        let composed = base
            .with_additional_excludes(&["target/**".to_owned()])
            .expect("composes");
        assert!(!composed.has_includes());
        assert!(composed.has_excludes());
        assert!(composed.matches("src/main.rs"));
        assert!(!composed.matches("target/release/x"));
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
