//! FUSE mount policy attached to a [`crate::ConfigProfile`].
//!
//! The defaults enforce owner-only access: only the user who started the
//! daemon can traverse the mounted tree. Enabling `allow_other` is a
//! deliberate opt-in that also requires `owner_only_by_default = false`
//! (validated in [`crate::ConfigProfile::validate`]).

// **PLATFORM:** all
// **GATING:** none (portable).

use serde::{Deserialize, Serialize};

/// Policy controlling who may access the FUSE mount point.
///
/// Persists in the envelope's `profile.mount` object. Both fields are
/// required by the schema; there are no env-var overrides today.
///
/// Valid combinations:
///
/// | `allow_other` | `owner_only_by_default` | Meaning                           |
/// |---------------|-------------------------|-----------------------------------|
/// | `false`       | `true`                  | Default. Only the owner sees it.  |
/// | `false`       | `false`                 | Explicitly relaxed owner-only; still no `allow_other`. |
/// | `true`        | `false`                 | Multi-user mount (opt-in).        |
/// | `true`        | `true`                  | Rejected by [`crate::ConfigProfile::validate`]. |
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MountPolicy {
    /// Pass the FUSE `allow_other` mount option, exposing the mount to
    /// every local user (uid != owner). Default: `false`. Valid values:
    /// `true`, `false`. **Security:** enabling this *must* be combined
    /// with `owner_only_by_default = false`; otherwise
    /// [`crate::ConfigProfile::validate`] returns
    /// [`crate::ConfigError::InvalidMountPolicy`]. On multi-user systems
    /// this also requires `user_allow_other` in `/etc/fuse.conf`. Example:
    /// `allow_other = false`.
    pub allow_other: bool,
    /// Enforce owner-only access on the FUSE mount (kernel
    /// `default_permissions` plus owner-uid enforcement). Default:
    /// `true`. Valid values: `true`, `false`. **Security:** the secure
    /// default — prevents `ls` / `read` from other local users even on
    /// world-readable mount points. Incompatible with
    /// `allow_other = true`. Example: `owner_only_by_default = true`.
    pub owner_only_by_default: bool,
    /// Maximum page-cache memory budget in MiB. Controls how much
    /// file-content the FUSE adapter keeps in memory for read
    /// acceleration. Default: `256`. A zero value is clamped to `1` by
    /// the page-cache constructor. Overridden at runtime by
    /// `PCLOUD_CACHE_SIZE_GB` (which takes precedence when set).
    /// Example: `cache_size_mb = 256`.
    #[serde(default = "default_cache_size_mb")]
    pub cache_size_mb: u32,
    /// Maximum number of page-cache entries (each entry is one 64 KiB
    /// page). Default: `4096`. The page cache evicts LRU entries once
    /// either this count or `cache_size_mb` is exceeded.
    /// Example: `page_cache_entries = 4096`.
    #[serde(default = "default_page_cache_entries")]
    pub page_cache_entries: u32,
    /// Metadata-cache TTL in seconds. Controls how long
    /// `getattr`/`lookup`/`readdir` results are cached before the FUSE
    /// adapter re-queries the remote. Default: `60`. A zero value
    /// disables caching (every request round-trips to the API).
    /// Example: `metadata_ttl_secs = 60`.
    #[serde(default = "default_metadata_ttl_secs")]
    pub metadata_ttl_secs: u32,
}

fn default_cache_size_mb() -> u32 {
    256
}

fn default_page_cache_entries() -> u32 {
    4096
}

fn default_metadata_ttl_secs() -> u32 {
    60
}

impl MountPolicy {
    /// Default cache size in MiB.
    pub const DEFAULT_CACHE_SIZE_MB: u32 = 256;
    /// Default page-cache entry count.
    pub const DEFAULT_PAGE_CACHE_ENTRIES: u32 = 4096;
    /// Default metadata-cache TTL in seconds.
    pub const DEFAULT_METADATA_TTL_SECS: u32 = 60;
}
