//! Data-residency policy attached to a [`crate::ConfigProfile`].
//!
//! Controls a region allow-list enforced at three call sites: sync-root
//! creation, `upload_create`, and `set_api_server`. The policy is
//! intentionally backward-compatible — an empty allow-list permits every
//! region — and it ships with `strict = false` so upgrades from older
//! config files warn rather than refuse.
//!
//! Enforcement lives in `pcloud-backends::residency`; this module is
//! purely the declarative config surface.

// **PLATFORM:** all
// **GATING:** none (portable).

use serde::{Deserialize, Serialize};

/// Per-profile data-residency policy.
///
/// # Backward compatibility
///
/// This section is optional on disk (v1/v2 envelopes that predate it still
/// load cleanly via `#[serde(default)]`). Default-constructed instances
/// permit every region and run in warn-only mode.
///
/// # Fields
///
/// - `allowed_regions`: case-insensitive list of region tags (e.g.
///   `"EU"`, `"US"`). An empty list means "allow all regions" so adding
///   this section to an existing deployment is a no-op until an operator
///   populates the list.
/// - `strict`: when `true`, a region outside the allow-list causes a hard
///   refusal (`ResponseStatus::PolicyViolation { kind: "data_residency" }`).
///   When `false`, the daemon emits a warning audit event but allows the
///   operation to proceed — useful for staged rollouts.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataResidencyPolicy {
    /// Allow-list of region tags. Empty means "permit any region"
    /// (backward-compatible default).
    #[serde(default)]
    pub allowed_regions: Vec<String>,
    /// When `true`, violations are refused; when `false`, violations only
    /// emit a warning audit event.
    #[serde(default)]
    pub strict: bool,
}

impl DataResidencyPolicy {
    /// Returns `true` when the policy imposes no restrictions.
    ///
    /// An empty `allowed_regions` list is treated as "allow all" regardless
    /// of the `strict` flag so operators can enable strict mode ahead of
    /// publishing the final region list without breaking existing flows.
    #[must_use]
    pub fn is_unrestricted(&self) -> bool {
        self.allowed_regions.is_empty()
    }

    /// Returns `true` when `region` matches one of the allow-list entries
    /// (case-insensitive). An empty allow-list always returns `true`.
    #[must_use]
    pub fn permits(&self, region: &str) -> bool {
        if self.allowed_regions.is_empty() {
            return true;
        }
        self.allowed_regions
            .iter()
            .any(|r| r.eq_ignore_ascii_case(region))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_unrestricted_and_non_strict() {
        let p = DataResidencyPolicy::default();
        assert!(p.is_unrestricted());
        assert!(!p.strict);
        assert!(p.permits("EU"));
        assert!(p.permits("US"));
        assert!(p.permits("XX"));
    }

    #[test]
    fn empty_list_permits_all_regardless_of_strict() {
        let p = DataResidencyPolicy {
            allowed_regions: Vec::new(),
            strict: true,
        };
        assert!(p.permits("EU"));
    }

    #[test]
    fn allow_list_is_case_insensitive() {
        let p = DataResidencyPolicy {
            allowed_regions: vec!["eu".to_string()],
            strict: true,
        };
        assert!(p.permits("EU"));
        assert!(p.permits("eu"));
        assert!(!p.permits("US"));
    }

    #[test]
    fn serialization_roundtrip() {
        let p = DataResidencyPolicy {
            allowed_regions: vec!["EU".to_string(), "US".to_string()],
            strict: true,
        };
        let s = serde_json::to_string(&p).unwrap();
        let back: DataResidencyPolicy = serde_json::from_str(&s).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn deserializes_with_missing_fields() {
        let p: DataResidencyPolicy = serde_json::from_str("{}").unwrap();
        assert!(p.allowed_regions.is_empty());
        assert!(!p.strict);
    }
}
