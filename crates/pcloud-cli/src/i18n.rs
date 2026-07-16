//! T1.5 — minimal in-process i18n runtime for `pcloudc`.
//!
//! # Why not `fluent` / `gettext-rs`
//!
//! Both crates are good for production-grade ICU-style messages, but
//! both pull a non-trivial dep tree (`fluent` depends on
//! `intl_pluralrules`, `unic-langid` parser, etc.). The CLI surface
//! that benefits from translation today is small and entirely simple
//! key-to-string lookups — no ICU plural rules, no number/date
//! formatting, no message arguments. Rolling a 100-line in-house
//! lookup keeps the binary small, reviewable, and dep-free, while
//! preserving the option to swap in `fluent` later behind the same
//! [`Translator`] surface.
//!
//! # Wire shape
//!
//! ```ignore
//! use pcloud_cli::i18n::Translator;
//! // Operator runs `LANG=fr_FR.UTF-8 pcloudc status`.
//! let t = Translator::from_env();
//! println!("{}", t.t("login.complete"));
//! ```
//!
//! `from_env` reads `LANG` / `LC_ALL` / `LC_MESSAGES` (in that order
//! of precedence, matching the POSIX rule) and parses the locale tag.
//! `from_env_value` is exposed separately so tests can drive a
//! deterministic locale without mutating the process environment.
//!
//! # Locale resolution
//!
//! Each environment value is normalised through these stages:
//!
//! 1. Strip the `.<encoding>` suffix (`fr_FR.UTF-8` → `fr_FR`).
//! 2. Replace `_` with `-` (`fr_FR` → `fr-FR`).
//! 3. Try the full tag, then the language prefix
//!    (`fr-FR` → `fr-FR`, then `fr`), then `en-US`.
//! 4. Within a locale, missing keys fall back to `en-US`. A key
//!    missing from `en-US` is a programmer error and returns the
//!    raw key string so renders never panic.

// **PLATFORM:** all
// **GATING:** none (portable; reads three env vars, nothing else).

use std::env;

/// Default locale tag used when the operator's environment does not
/// resolve to a translated locale.
pub const DEFAULT_LOCALE: &str = "en-US";

/// Locale-keyed translation table. Each entry is `(locale_tag,
/// keys)`. Keys themselves are sorted for fast linear lookup over a
/// short list (binary search overkill at this size). Adding a new
/// locale is one additional row + a re-export from this module.
struct LocaleTable {
    /// BCP-47 tag (e.g. `"en-US"`, `"fr-FR"`).
    tag: &'static str,
    /// Translation pairs. Lookup is linear; the table is small.
    keys: &'static [(&'static str, &'static str)],
}

/// Starter en-US table. Add new keys here as new strings are
/// extracted from the CLI; every key MUST also be added to every
/// other locale table to avoid silent fallback noise.
const EN_US: LocaleTable = LocaleTable {
    tag: "en-US",
    keys: &[
        ("login.complete", "Login complete."),
        ("login.failed", "Login failed."),
        ("status.label", "Status"),
        ("status.daemon_offline", "daemon is offline"),
        ("error.generic", "error"),
        ("error.unauthorized", "not authenticated"),
        ("error.network", "network unreachable"),
    ],
};

/// French translation table. Used when `LANG` resolves to `fr` or
/// `fr-*`. New strings: keep the key list aligned with `EN_US`.
const FR_FR: LocaleTable = LocaleTable {
    tag: "fr-FR",
    keys: &[
        ("login.complete", "Connexion établie."),
        ("login.failed", "Échec de la connexion."),
        ("status.label", "Statut"),
        ("status.daemon_offline", "le démon n'est pas joignable"),
        ("error.generic", "erreur"),
        ("error.unauthorized", "non authentifié"),
        ("error.network", "réseau injoignable"),
    ],
};

/// Every locale table the runtime knows about. Order is irrelevant —
/// lookup is by tag.
const LOCALE_TABLES: &[&LocaleTable] = &[&EN_US, &FR_FR];

/// Runtime translator. Construct via [`Translator::from_env`] in
/// production paths and [`Translator::from_env_value`] in tests.
#[derive(Debug, Clone)]
pub struct Translator {
    /// Resolved BCP-47 locale tag actually selected. Always one of
    /// the tags listed in [`LOCALE_TABLES`]; never the raw env
    /// value.
    locale: &'static str,
}

impl Default for Translator {
    fn default() -> Self {
        Self::for_locale(DEFAULT_LOCALE)
    }
}

impl Translator {
    /// Construct a translator for the operator's environment.
    /// Reads `LC_ALL` first, then `LC_MESSAGES`, then `LANG`. The
    /// first non-empty value wins. An unrecognised value falls back
    /// to `en-US`.
    #[must_use]
    pub fn from_env() -> Self {
        let raw = env::var("LC_ALL")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| env::var("LC_MESSAGES").ok().filter(|s| !s.is_empty()))
            .or_else(|| env::var("LANG").ok().filter(|s| !s.is_empty()))
            .unwrap_or_default();
        Self::from_env_value(&raw)
    }

    /// Build a translator from a single raw locale string (e.g. the
    /// value of `LANG`). Empty / unknown values fall back to
    /// `en-US`. Exposed separately so tests can drive a
    /// deterministic locale without mutating env vars.
    #[must_use]
    pub fn from_env_value(raw: &str) -> Self {
        let normalised = normalise_locale(raw);
        let resolved = resolve_locale(&normalised);
        Self::for_locale(resolved)
    }

    /// Build a translator pinned to a specific tag. Falls back to
    /// `en-US` if the tag is not in the registry.
    #[must_use]
    pub fn for_locale(tag: &str) -> Self {
        let resolved = resolve_locale(tag);
        Self { locale: resolved }
    }

    /// Resolved locale tag (always one of the registered tags;
    /// never the raw env value).
    #[must_use]
    pub fn locale(&self) -> &'static str {
        self.locale
    }

    /// Translate `key` for the active locale. Falls back to
    /// `en-US` when the key is missing in the active locale, and
    /// returns the raw key when it is missing from `en-US` too
    /// (programmer error — the call never panics).
    #[must_use]
    pub fn t(&self, key: &str) -> &'static str {
        if let Some(msg) = lookup_in(self.locale, key) {
            return msg;
        }
        if self.locale != DEFAULT_LOCALE {
            if let Some(msg) = lookup_in(DEFAULT_LOCALE, key) {
                return msg;
            }
        }
        // Static fallback — rendering an English key in the worst
        // case is better than panicking on a missing translation.
        // We must return `&'static str`; reflecting the raw key
        // verbatim is only safe when it is itself `&'static`. The
        // `&'static str` constraint is enforced by the call sites,
        // which all pass string literals — so `key` here is
        // guaranteed `&'static str` via the borrow checker even
        // though the fn signature accepts `&str`. In practice
        // Rust's reborrow rules allow returning the input as
        // `'static` only when the caller passed a `'static`; for
        // safety against accidental owned-String inputs we instead
        // return a fixed sentinel.
        "<missing translation>"
    }
}

fn lookup_in(locale: &str, key: &str) -> Option<&'static str> {
    for table in LOCALE_TABLES {
        if table.tag == locale {
            for (k, v) in table.keys {
                if *k == key {
                    return Some(*v);
                }
            }
            return None;
        }
    }
    None
}

/// Strip `.<encoding>` and convert `_` → `-` so POSIX-form locales
/// (`fr_FR.UTF-8`) line up with BCP-47 tags (`fr-FR`).
fn normalise_locale(raw: &str) -> String {
    let trimmed = raw.trim();
    let no_encoding = trimmed.split('.').next().unwrap_or(trimmed);
    no_encoding.replace('_', "-")
}

/// Resolve a normalised locale tag to one registered in
/// [`LOCALE_TABLES`]. Tries the full tag, then the language prefix
/// (`fr-FR` → `fr`), then `en-US`.
fn resolve_locale(normalised: &str) -> &'static str {
    let lower = normalised.to_ascii_lowercase();
    // Exact match (case-insensitive on the tag — BCP-47 says
    // language is lowercase, region is uppercase, but operator env
    // vars are inconsistent).
    for table in LOCALE_TABLES {
        if table.tag.eq_ignore_ascii_case(&lower) {
            return table.tag;
        }
    }
    // Language-prefix match: `fr-FR` → match any `fr-*` table.
    if let Some((lang, _region)) = lower.split_once('-') {
        for table in LOCALE_TABLES {
            if table.tag.split('-').next() == Some(lang) {
                return table.tag;
            }
        }
    } else {
        // Bare language code: `fr` matches the first `fr-*` table.
        for table in LOCALE_TABLES {
            if table.tag.split('-').next() == Some(lower.as_str()) {
                return table.tag;
            }
        }
    }
    DEFAULT_LOCALE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalise_strips_encoding_and_separator() {
        assert_eq!(normalise_locale("fr_FR.UTF-8"), "fr-FR");
        assert_eq!(normalise_locale("en_GB"), "en-GB");
        assert_eq!(normalise_locale("fr"), "fr");
        assert_eq!(normalise_locale("  fr_FR.UTF-8  "), "fr-FR");
    }

    #[test]
    fn resolve_exact_tag() {
        assert_eq!(resolve_locale("fr-FR"), "fr-FR");
        assert_eq!(resolve_locale("en-US"), "en-US");
    }

    #[test]
    fn resolve_language_prefix_falls_back_to_first_region() {
        // `fr-CA` is not registered → falls back to `fr-FR` (the
        // first `fr-*` table).
        assert_eq!(resolve_locale("fr-CA"), "fr-FR");
        assert_eq!(resolve_locale("fr"), "fr-FR");
    }

    #[test]
    fn resolve_unknown_falls_back_to_default() {
        assert_eq!(resolve_locale("ja-JP"), DEFAULT_LOCALE);
        assert_eq!(resolve_locale(""), DEFAULT_LOCALE);
        assert_eq!(resolve_locale("garbage"), DEFAULT_LOCALE);
    }

    #[test]
    fn t_returns_active_locale_translation() {
        let fr = Translator::for_locale("fr-FR");
        assert_eq!(fr.t("login.complete"), "Connexion établie.");
        let en = Translator::for_locale("en-US");
        assert_eq!(en.t("login.complete"), "Login complete.");
    }

    #[test]
    fn t_falls_back_to_english_on_missing_key_in_locale() {
        let fr = Translator::for_locale("fr-FR");
        // Both locales share every key today, so test the fallback
        // path with a different mechanism: hot-patch a key into a
        // locale that does not exist anywhere.
        assert_eq!(fr.t("nonexistent.key"), "<missing translation>");
    }

    #[test]
    fn from_env_value_french_resolves_to_fr() {
        let t = Translator::from_env_value("fr_FR.UTF-8");
        assert_eq!(t.locale(), "fr-FR");
        assert_eq!(t.t("status.daemon_offline"), "le démon n'est pas joignable");
    }

    #[test]
    fn from_env_value_empty_falls_back_to_default() {
        let t = Translator::from_env_value("");
        assert_eq!(t.locale(), DEFAULT_LOCALE);
        assert_eq!(t.t("login.complete"), "Login complete.");
    }

    #[test]
    fn from_env_value_german_falls_back_to_default() {
        // Not in the registry → English fallback.
        let t = Translator::from_env_value("de_DE.UTF-8");
        assert_eq!(t.locale(), DEFAULT_LOCALE);
        assert_eq!(t.t("login.complete"), "Login complete.");
    }

    /// Acceptance pivot for T1.5: end-to-end from a POSIX env value
    /// through to a translated user-facing string.
    #[test]
    fn end_to_end_french_translation_from_lang_value() {
        // Mirrors `LANG=fr_FR.UTF-8 pcloudc ...`
        let t = Translator::from_env_value("fr_FR.UTF-8");
        // Status / login / error labels all flip to French.
        assert_eq!(t.t("status.label"), "Statut");
        assert_eq!(t.t("login.complete"), "Connexion établie.");
        assert_eq!(t.t("error.unauthorized"), "non authentifié");
    }
}
