//! Data-residency region resolver, cache, enforcement, and audit helpers.
//!
//! This module is the runtime counterpart of the declarative
//! [`pcloud_config::data_residency::DataResidencyPolicy`] section. It
//! exposes:
//!
//! - [`Region`] — the coarse region tag (EU / US / unknown) that backend
//!   call sites compare against the allow-list.
//! - [`resolve_region`] — maps an API-server hint (e.g. `"eapi.pcloud.com"`,
//!   `"api.pcloud.com"`) to a [`Region`].
//! - [`RegionCache`] — a small per-folder-id TTL cache (1h) so repeated
//!   enforcement checks don't re-issue `listfolder` calls.
//! - [`ResidencyDecision`] — the outcome of an enforcement check, used by
//!   the daemon to build the wire [`pcloud_ipc::ResponseStatus::PolicyViolation`]
//!   response and emit [`ResidencyAuditEvent`] entries.
//!
//! The enforcement helpers are deliberately free functions taking an
//! explicit [`DataResidencyPolicy`] reference rather than hooking into the
//! backend structs, so:
//!
//! 1. each of the three call sites (`sync_backend`, `transfer_backend`,
//!    `account_backend`) stays a thin shim,
//! 2. unit tests can exercise the logic without constructing a live
//!    backend,
//! 3. the daemon controls the audit-sink plumbing.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use pcloud_config::data_residency::DataResidencyPolicy;

/// Default TTL for cached [`Region`] lookups (1 hour). Re-exported so
/// tests and operators who subclass the cache can refer to the same
/// constant.
pub const REGION_CACHE_TTL: Duration = Duration::from_secs(3600);

/// Coarse pCloud data-center region tag.
///
/// We deliberately keep the variant set small because pCloud publicly
/// differentiates on EU vs US locations and the allow-list is a
/// compliance boundary, not a telemetry dimension. Unknown hints collapse
/// into [`Region::Unknown`] which, under `strict` mode, is always
/// refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Region {
    /// pCloud EU data center (host pattern `eapi.*`).
    Eu,
    /// pCloud US data center (host pattern `api.*` with no regional
    /// prefix).
    Us,
    /// Unable to classify. Strict-mode enforcement treats this as a
    /// refusal so operators cannot accidentally allow-list an unknown
    /// hint.
    Unknown,
}

impl Region {
    /// Short stable tag emitted in audit events and
    /// [`pcloud_ipc::Response::message`] payloads.
    #[must_use]
    pub fn as_tag(&self) -> &'static str {
        match self {
            Region::Eu => "EU",
            Region::Us => "US",
            Region::Unknown => "UNKNOWN",
        }
    }
}

/// Subset of the pCloud folder metadata response that the resolver cares
/// about. We accept a struct rather than a raw JSON `Value` so the
/// resolver stays unit-testable in isolation of the network layer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FolderMetadataHint {
    /// Folder id (used as the cache key). `0` disables caching.
    pub folder_id: u64,
    /// API-server hint returned by `listfolder` / `getfilelink` /
    /// `getapiserver`. Typically one of `"api.pcloud.com"`,
    /// `"eapi.pcloud.com"`, or an empty string on legacy responses.
    pub api_server: String,
}

/// Classify an API-server host hint into a [`Region`].
///
/// The classification is conservative — any non-empty hint that does not
/// match the known EU or US patterns collapses to [`Region::Unknown`] so
/// strict-mode enforcement never silently allow-lists an unrecognised
/// host.
#[must_use]
pub fn resolve_region(meta: &FolderMetadataHint) -> Region {
    resolve_region_from_host(&meta.api_server)
}

/// Lower-level helper: classify a raw host string. Accepts an empty
/// string (returns [`Region::Unknown`]) and is case-insensitive.
#[must_use]
pub fn resolve_region_from_host(host: &str) -> Region {
    let h = host.trim().to_ascii_lowercase();
    if h.is_empty() {
        return Region::Unknown;
    }
    // EU data center: eapi.pcloud.com / eapi-*.pcloud.com
    if h.starts_with("eapi.") || h.starts_with("eapi-") || h == "eapi.pcloud.com" {
        return Region::Eu;
    }
    // US data center: api.pcloud.com / api-*.pcloud.com / binapi.pcloud.com
    if h.starts_with("api.")
        || h.starts_with("api-")
        || h.starts_with("binapi.")
        || h.starts_with("bineapi.")
    {
        // bineapi.* is the EU binary endpoint.
        if h.starts_with("bineapi.") {
            return Region::Eu;
        }
        return Region::Us;
    }
    Region::Unknown
}

/// Per-folder-id TTL cache for region lookups.
///
/// Uses a plain `Mutex<HashMap>` rather than a lock-free map because:
///
/// - enforcement checks happen at most once per backend dispatch,
/// - the entry count is bounded by the active sync-root + upload-target
///   set (typically < 1k),
/// - contention is negligible under realistic daemon load.
#[derive(Debug, Default)]
pub struct RegionCache {
    inner: Mutex<HashMap<u64, (Region, Instant)>>,
    ttl: Duration,
}

impl RegionCache {
    /// Construct an empty cache with the default TTL
    /// ([`REGION_CACHE_TTL`], 1 hour).
    #[must_use]
    pub fn new() -> Self {
        Self::with_ttl(REGION_CACHE_TTL)
    }

    /// Construct an empty cache with a custom TTL. Intended for tests
    /// that need to exercise expiry semantics without sleeping.
    #[must_use]
    pub fn with_ttl(ttl: Duration) -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
            ttl,
        }
    }

    /// Look up a cached region. Returns `None` if the entry is missing
    /// or expired. Expired entries are evicted lazily.
    pub fn get(&self, folder_id: u64) -> Option<Region> {
        if folder_id == 0 {
            return None;
        }
        let now = Instant::now();
        let mut guard = self.inner.lock().ok()?;
        if let Some(&(region, inserted)) = guard.get(&folder_id) {
            if now.duration_since(inserted) < self.ttl {
                return Some(region);
            }
            guard.remove(&folder_id);
        }
        None
    }

    /// Insert or refresh a cache entry. A `folder_id` of `0` is a no-op
    /// (the resolver uses `0` as the "don't cache" sentinel).
    pub fn insert(&self, folder_id: u64, region: Region) {
        if folder_id == 0 {
            return;
        }
        if let Ok(mut guard) = self.inner.lock() {
            guard.insert(folder_id, (region, Instant::now()));
        }
    }

    /// Resolve + memoize a region. If the cache hits, returns the cached
    /// value; otherwise invokes `f` to compute and stores the result.
    pub fn resolve_or_insert_with<F>(&self, folder_id: u64, f: F) -> Region
    where
        F: FnOnce() -> Region,
    {
        if let Some(region) = self.get(folder_id) {
            return region;
        }
        let region = f();
        self.insert(folder_id, region);
        region
    }

    /// Remove all entries. Intended for session-reset and test cleanup.
    pub fn clear(&self) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.clear();
        }
    }
}

/// Result of a residency-policy check. Call sites turn `Refuse` into a
/// `ResponseStatus::PolicyViolation { kind: "data_residency" }` on the
/// wire and emit the returned [`ResidencyAuditEvent`] either way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResidencyDecision {
    /// Policy permits the operation. Non-strict-mode violations also
    /// land here, flagged via [`ResidencyAuditEvent::warned`].
    Allow,
    /// Policy refuses the operation. Strict-mode violations only.
    Refuse,
}

/// Stable audit-record produced by every enforcement check, regardless
/// of outcome. The daemon sink persists these alongside the existing
/// audit chain under the `ResidencyViolation` event kind described in
/// CLAUDE.md.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResidencyAuditEvent {
    /// Which call site triggered the check (`"sync_root_add"`,
    /// `"upload_create"`, `"set_api_server"`).
    pub action: &'static str,
    /// Resolved region the operation targeted.
    pub region: Region,
    /// Allow-list configured at the time of the check.
    pub allowed: Vec<String>,
    /// `true` when the operation was refused (strict mode). Non-strict
    /// violations set `refused = false` and `warned = true`.
    pub refused: bool,
    /// `true` when the policy would have refused the operation but
    /// strict-mode was disabled (warn-only). Allows an operator to count
    /// near-misses without blocking traffic.
    pub warned: bool,
}

/// Stable wire discriminator for
/// [`pcloud_ipc::ResponseStatus::PolicyViolation::kind`] produced by
/// this module.
pub const POLICY_KIND_DATA_RESIDENCY: &str = "data_residency";

/// Enforce the allow-list for a single call site.
///
/// Returns the decision plus an audit event the caller must persist
/// (via the daemon audit sink). `action` identifies the call site and is
/// copied verbatim into [`ResidencyAuditEvent::action`].
#[must_use]
pub fn enforce(
    policy: &DataResidencyPolicy,
    region: Region,
    action: &'static str,
) -> (ResidencyDecision, ResidencyAuditEvent) {
    // Empty allow-list: permit everything (backward-compatible).
    if policy.is_unrestricted() {
        return (
            ResidencyDecision::Allow,
            ResidencyAuditEvent {
                action,
                region,
                allowed: policy.allowed_regions.clone(),
                refused: false,
                warned: false,
            },
        );
    }

    let permitted = policy.permits(region.as_tag());
    if permitted {
        return (
            ResidencyDecision::Allow,
            ResidencyAuditEvent {
                action,
                region,
                allowed: policy.allowed_regions.clone(),
                refused: false,
                warned: false,
            },
        );
    }

    // Violation: strict mode refuses, non-strict warns.
    if policy.strict {
        (
            ResidencyDecision::Refuse,
            ResidencyAuditEvent {
                action,
                region,
                allowed: policy.allowed_regions.clone(),
                refused: true,
                warned: false,
            },
        )
    } else {
        (
            ResidencyDecision::Allow,
            ResidencyAuditEvent {
                action,
                region,
                allowed: policy.allowed_regions.clone(),
                refused: false,
                warned: true,
            },
        )
    }
}

/// Call-site identifier for the sync-root enforcement hook. Re-exported
/// so callers use the same literal the audit filters key off.
pub const ACTION_SYNC_ROOT_ADD: &str = "sync_root_add";
/// Call-site identifier for the upload-create enforcement hook.
pub const ACTION_UPLOAD_CREATE: &str = "upload_create";
/// Call-site identifier for the `set_api_server` enforcement hook.
pub const ACTION_SET_API_SERVER: &str = "set_api_server";

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(regions: &[&str], strict: bool) -> DataResidencyPolicy {
        DataResidencyPolicy {
            allowed_regions: regions.iter().map(|s| (*s).to_string()).collect(),
            strict,
        }
    }

    #[test]
    fn resolver_maps_known_hosts() {
        assert_eq!(resolve_region_from_host("eapi.pcloud.com"), Region::Eu);
        assert_eq!(resolve_region_from_host("api.pcloud.com"), Region::Us);
        assert_eq!(resolve_region_from_host("bineapi.pcloud.com"), Region::Eu);
        assert_eq!(resolve_region_from_host("binapi.pcloud.com"), Region::Us);
        assert_eq!(resolve_region_from_host(""), Region::Unknown);
        assert_eq!(resolve_region_from_host("unknown.example"), Region::Unknown);
    }

    #[test]
    fn resolver_is_case_insensitive() {
        assert_eq!(resolve_region_from_host("EAPI.pcloud.com"), Region::Eu);
        assert_eq!(resolve_region_from_host("API.PCLOUD.COM"), Region::Us);
    }

    #[test]
    fn empty_allow_list_permits_all_regions() {
        let p = policy(&[], true);
        for region in [Region::Eu, Region::Us, Region::Unknown] {
            let (decision, evt) = enforce(&p, region, "unit_test");
            assert_eq!(decision, ResidencyDecision::Allow);
            assert!(!evt.refused);
            assert!(!evt.warned);
        }
    }

    #[test]
    fn strict_mode_rejects_upload_to_disallowed_region() {
        let p = policy(&["EU"], true);
        let (decision, evt) = enforce(&p, Region::Us, ACTION_UPLOAD_CREATE);
        assert_eq!(decision, ResidencyDecision::Refuse);
        assert!(evt.refused);
        assert!(!evt.warned);
        assert_eq!(evt.region, Region::Us);
        assert_eq!(evt.action, ACTION_UPLOAD_CREATE);
    }

    #[test]
    fn non_strict_mode_emits_warning_but_allows() {
        let p = policy(&["EU"], false);
        let (decision, evt) = enforce(&p, Region::Us, ACTION_UPLOAD_CREATE);
        assert_eq!(decision, ResidencyDecision::Allow);
        assert!(!evt.refused);
        assert!(evt.warned);
    }

    #[test]
    fn set_api_server_refused_outside_allow_list() {
        let p = policy(&["EU"], true);
        let region = resolve_region_from_host("api.pcloud.com");
        assert_eq!(region, Region::Us);
        let (decision, evt) = enforce(&p, region, ACTION_SET_API_SERVER);
        assert_eq!(decision, ResidencyDecision::Refuse);
        assert_eq!(evt.action, ACTION_SET_API_SERVER);
    }

    #[test]
    fn sync_add_refused_when_remote_root_outside_allow_list() {
        let p = policy(&["US"], true);
        let meta = FolderMetadataHint {
            folder_id: 42,
            api_server: "eapi.pcloud.com".to_string(),
        };
        let region = resolve_region(&meta);
        assert_eq!(region, Region::Eu);
        let (decision, _evt) = enforce(&p, region, ACTION_SYNC_ROOT_ADD);
        assert_eq!(decision, ResidencyDecision::Refuse);
    }

    #[test]
    fn strict_mode_refuses_unknown_region() {
        let p = policy(&["EU", "US"], true);
        let (decision, _) = enforce(&p, Region::Unknown, "x");
        assert_eq!(decision, ResidencyDecision::Refuse);
    }

    #[test]
    fn cache_returns_cached_entry_within_ttl() {
        let cache = RegionCache::with_ttl(Duration::from_secs(60));
        cache.insert(7, Region::Eu);
        assert_eq!(cache.get(7), Some(Region::Eu));
    }

    #[test]
    fn cache_expires_after_ttl() {
        let cache = RegionCache::with_ttl(Duration::from_millis(1));
        cache.insert(7, Region::Eu);
        std::thread::sleep(Duration::from_millis(5));
        assert_eq!(cache.get(7), None);
    }

    #[test]
    fn cache_zero_folder_id_is_noop() {
        let cache = RegionCache::new();
        cache.insert(0, Region::Eu);
        assert_eq!(cache.get(0), None);
    }

    #[test]
    fn cache_resolve_or_insert_memoizes() {
        let cache = RegionCache::new();
        let mut call_count = 0;
        for _ in 0..3 {
            let _ = cache.resolve_or_insert_with(1, || {
                call_count += 1;
                Region::Us
            });
        }
        assert_eq!(call_count, 1);
        assert_eq!(cache.get(1), Some(Region::Us));
    }
}
