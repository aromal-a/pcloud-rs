//! Path canonicalisation for FUSE adapter inputs.
//!
//! pCloud paths are POSIX-style, always rooted at `/`, and must not contain
//! embedded NUL bytes. Kernel-supplied `name` arguments for `lookup` are
//! raw bytes without a leading separator, may contain `.` or `..`, and may
//! be empty — all of which must be rejected or resolved before hitting the
//! remote listing endpoint.

// **PLATFORM:** all
// **GATING:** none (portable).

use thiserror::Error;

/// Errors produced by the path canonicaliser.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum PathError {
    /// The path contained an embedded NUL byte, which is invalid on every
    /// POSIX system.
    #[error("path contains embedded NUL byte")]
    EmbeddedNul,
    /// A path component was structurally invalid (e.g. contains a `/` when
    /// the API expects a base name only).
    #[error("path contains invalid component: {0:?}")]
    InvalidComponent(String),
    /// The caller supplied an empty base name.
    #[error("path name is empty")]
    EmptyName,
    /// The path resolves above the filesystem root. Attempts to access
    /// outside the mount namespace are rejected.
    #[error("path escapes filesystem root")]
    EscapesRoot,
}

/// Normalise a parent path plus a child name into a canonical pCloud path.
///
/// Canonical form:
/// - always starts with `/`
/// - never ends with `/` (except for the root itself)
/// - never contains empty, `.`, or `..` segments after normalisation
/// - never contains embedded NUL bytes
/// - always valid UTF-8 (Rust `&str` enforces this)
pub fn join_child(parent: &str, name: &str) -> Result<String, PathError> {
    validate_no_nul(parent)?;
    validate_no_nul(name)?;

    if name.is_empty() {
        return Err(PathError::EmptyName);
    }
    if name == "." {
        return canonicalise(parent);
    }
    if name == ".." {
        return parent_of(parent);
    }
    if name.contains('/') {
        return Err(PathError::InvalidComponent(name.to_owned()));
    }

    let base = canonicalise(parent)?;
    let joined = if base == "/" {
        format!("/{name}")
    } else {
        format!("{base}/{name}")
    };
    Ok(joined)
}

/// Canonicalise a standalone path (collapses `//`, `.`, `..`).
pub fn canonicalise(path: &str) -> Result<String, PathError> {
    validate_no_nul(path)?;
    let mut stack: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => continue,
            ".." => {
                if stack.pop().is_none() {
                    return Err(PathError::EscapesRoot);
                }
            }
            other => stack.push(other),
        }
    }
    if stack.is_empty() {
        Ok("/".to_owned())
    } else {
        let mut out = String::with_capacity(path.len());
        for s in stack {
            out.push('/');
            out.push_str(s);
        }
        Ok(out)
    }
}

fn parent_of(path: &str) -> Result<String, PathError> {
    let canon = canonicalise(path)?;
    if canon == "/" {
        return Err(PathError::EscapesRoot);
    }
    let trimmed = match canon.rfind('/') {
        Some(0) => "/".to_owned(),
        Some(idx) => canon[..idx].to_owned(),
        None => return Err(PathError::EscapesRoot),
    };
    Ok(trimmed)
}

fn validate_no_nul(s: &str) -> Result<(), PathError> {
    if s.as_bytes().contains(&0u8) {
        return Err(PathError::EmbeddedNul);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_child_from_root() {
        assert_eq!(join_child("/", "docs").unwrap(), "/docs");
    }

    #[test]
    fn join_child_from_nested() {
        assert_eq!(join_child("/a/b", "c").unwrap(), "/a/b/c");
    }

    #[test]
    fn rejects_slash_in_name() {
        assert_eq!(
            join_child("/", "a/b"),
            Err(PathError::InvalidComponent("a/b".to_owned()))
        );
    }

    #[test]
    fn rejects_empty_name() {
        assert_eq!(join_child("/", ""), Err(PathError::EmptyName));
    }

    #[test]
    fn rejects_embedded_nul_in_name() {
        assert_eq!(join_child("/", "bad\0name"), Err(PathError::EmbeddedNul));
    }

    #[test]
    fn rejects_embedded_nul_in_parent() {
        assert_eq!(join_child("/a\0b", "x"), Err(PathError::EmbeddedNul));
    }

    #[test]
    fn dot_resolves_to_parent() {
        assert_eq!(join_child("/a/b", ".").unwrap(), "/a/b");
    }

    #[test]
    fn dotdot_pops_one_segment() {
        assert_eq!(join_child("/a/b", "..").unwrap(), "/a");
    }

    #[test]
    fn dotdot_at_root_escapes() {
        assert_eq!(join_child("/", ".."), Err(PathError::EscapesRoot));
    }

    #[test]
    fn canonicalise_collapses_duplicate_slashes() {
        assert_eq!(canonicalise("//a///b/./c").unwrap(), "/a/b/c");
    }

    #[test]
    fn canonicalise_root_variants() {
        assert_eq!(canonicalise("").unwrap(), "/");
        assert_eq!(canonicalise("/").unwrap(), "/");
        assert_eq!(canonicalise("/./.").unwrap(), "/");
    }

    #[test]
    fn unicode_names_are_preserved() {
        // pCloud paths are UTF-8; ensure multibyte characters pass through.
        assert_eq!(join_child("/", "éclair").unwrap(), "/éclair");
        assert_eq!(join_child("/", "日本語").unwrap(), "/日本語");
    }
}
