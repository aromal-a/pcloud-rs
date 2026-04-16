//! Extension-weighted sync-folder suggestion scorer.
//!
//! This module is the Rust port of the C `psuggest_scan_folder`
//! implementation in `pclsync/psuggest.c` (with the extension tables
//! from `pclsync/pscanexts.h` and the tuning constants from
//! `pclsync/psettings.h`). It walks a local directory tree, classifies
//! each regular file by extension into one of five buckets
//! (other, pictures, videos, music, documents), aggregates per-folder
//! counts, and returns the folders that are dominated by non-"other"
//! content.
//!
//! Differences from the C implementation:
//!
//! * The traversal is explicitly bounded by a depth cap
//!   (`MAX_SCAN_DEPTH`) and an entry cap (`MAX_SCAN_ENTRIES`) so that
//!   the scorer cannot be coerced into exhausting memory on pathological
//!   trees. The C code has no such limits.
//! * Directory entries are iterated in a stable, case-sensitive sorted
//!   order so suggestion output is deterministic regardless of the
//!   underlying filesystem.
//! * Hidden entries (leading `.`) are skipped, matching the C
//!   `ignore_patters` array (`{ ".*" }`).
//!
//! The extension id function and lookup tables are reproduced byte-for
//! byte from the C source so that classification matches pixel-for-pixel
//! across the two implementations.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::io;
use std::path::{Path, PathBuf};

/// Number of file-type buckets: other, pictures, videos, music, documents.
pub const SCAN_TYPES_CNT: usize = 5;

/// Human-readable labels for each bucket (index 0 = "other files").
/// Mirrors `psync_scan_typenames` in `pclsync/pscanexts.h`.
pub const SCAN_TYPE_NAMES: [&str; SCAN_TYPES_CNT] = [
    "other files",
    "pictures",
    "videos",
    "music files",
    "documents",
];

/// Minimum number of non-"other" files for a folder to be suggested.
/// Mirrors `PSYNC_SCANNER_MIN_FILES` in `pclsync/psettings.h`.
pub const SCANNER_MIN_FILES: u32 = 25;

/// Minimum non-"other"-to-total ratio, expressed as an integer percent.
/// Mirrors `PSYNC_SCANNER_PERCENT` in `pclsync/psettings.h`.
pub const SCANNER_PERCENT: u32 = 80;

/// Minimum per-type count for the type to be listed in the description.
/// Mirrors `PSYNC_SCANNER_MIN_DISPLAY` in `pclsync/psettings.h`.
pub const SCANNER_MIN_DISPLAY: u32 = 10;

/// Maximum number of suggestions returned.
/// Mirrors `PSYNC_SCANNER_MAX_SUGGESTIONS` in `pclsync/psettings.h`.
pub const SCANNER_MAX_SUGGESTIONS: usize = 6;

/// Bounded-traversal depth cap. Not present in C; added here to keep
/// the scanner safe on hostile or accidentally deep trees.
pub const MAX_SCAN_DEPTH: usize = 16;

/// Bounded-traversal entry cap (files + directories visited). Not
/// present in C; prevents runaway scans on huge trees.
pub const MAX_SCAN_ENTRIES: usize = 200_000;

/// Result row from [`scan_folder`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SuggestedFolder {
    /// Canonical local path to the candidate folder.
    pub local_path: String,
    /// Leaf name (last path component) of the candidate folder.
    pub name: String,
    /// Short human-readable description, e.g. "42 pictures, 11 music files".
    pub description: String,
    /// Sum of non-"other" files contained transitively.
    pub file_count: u32,
    /// Per-bucket counts (index 0 = other).
    pub type_counts: [u32; SCAN_TYPES_CNT],
}

/// Character map used to fold ASCII into the 37-symbol alphabet that
/// the C extension-id hash operates on. Values outside `[0-9A-Za-z]`
/// yield 0, which means "not a known extension character" and aborts
/// hashing for that extension.
///
/// Reproduced verbatim from `psync_character_map` in
/// `pclsync/pscanexts.h`.
const CHARACTER_MAP: [u8; 256] = {
    let mut m = [0u8; 256];
    // '0'..'9' -> 1..10
    let mut c = b'0';
    let mut v = 1u8;
    while c <= b'9' {
        m[c as usize] = v;
        c += 1;
        v += 1;
    }
    // 'A'..'Z' -> 11..36
    let mut c = b'A';
    let mut v = 11u8;
    while c <= b'Z' {
        m[c as usize] = v;
        c += 1;
        v += 1;
    }
    // 'a'..'z' -> 11..36
    let mut c = b'a';
    let mut v = 11u8;
    while c <= b'z' {
        m[c as usize] = v;
        c += 1;
        v += 1;
    }
    m
};

/// Sorted extension-id table. Mirrors `psync_scan_extensions` in
/// `pclsync/pscanexts.h`. Must stay aligned index-for-index with
/// [`SCAN_TYPES`].
const SCAN_EXTENSIONS: [u32; 166] = [
    438, 540, 550, 651, 1029, 1047, 1059, 1139, 1244, 6131, 15536, 15778, 15938, 16036, 16125,
    16145, 16148, 16166, 16262, 16292, 16328, 16349, 17305, 17480, 18220, 18343, 18345, 18789,
    18836, 18866, 18884, 19885, 19938, 20104, 20121, 21587, 22737, 22750, 23992, 24369, 26517,
    26582, 28285, 28357, 28359, 29184, 29739, 29741, 29848, 29984, 30000, 31207, 31225, 31666,
    31683, 32017, 32204, 32296, 32392, 32444, 32452, 32453, 32454, 32464, 32466, 32481, 32626,
    32776, 33427, 34755, 34756, 34762, 34766, 34769, 34772, 34773, 34865, 34871, 34886, 35274,
    35277, 35353, 35361, 35364, 35365, 36031, 36061, 36109, 36128, 36246, 36437, 36499, 36505,
    36549, 36579, 36585, 36586, 36681, 38762, 38768, 38973, 39458, 40207, 40222, 40252, 40352,
    40418, 40603, 40697, 40767, 40844, 40902, 40976, 40992, 41789, 42154, 42175, 42358, 45616,
    45618, 46039, 46060, 46062, 46395, 47013, 47372, 47389, 47390, 47531, 47781, 583799, 583802,
    684197, 737737, 743871, 743882, 744500, 744511, 840986, 1049226, 1170265, 1191567, 1201185,
    1201253, 1352336, 1352347, 1353002, 1353668, 1353679, 1353705, 1353716, 1499596, 1499607,
    1513410, 1541413, 1546209, 1688854, 1692551, 1752750, 1753405, 1753416, 1753427, 1753453,
    1753464, 44416554, 44443856,
];

/// Type bucket for each extension in [`SCAN_EXTENSIONS`]. Mirrors
/// `psync_scan_types` in `pclsync/pscanexts.h`. Encoding:
/// 0=other, 1=pictures, 2=videos, 3=music, 4=documents.
const SCAN_TYPES: [u8; 166] = [
    3, 2, 2, 2, 2, 3, 3, 2, 2, 2, 4, 3, 3, 3, 1, 4, 2, 2, 2, 3, 3, 2, 1, 4, 3, 1, 1, 1, 1, 1, 3, 2,
    1, 4, 4, 1, 2, 2, 1, 3, 1, 1, 1, 1, 1, 3, 4, 4, 4, 4, 4, 2, 2, 3, 3, 4, 3, 2, 2, 2, 3, 3, 2, 2,
    2, 2, 2, 2, 1, 4, 4, 1, 4, 4, 4, 4, 3, 3, 2, 3, 1, 4, 4, 4, 4, 1, 1, 1, 4, 1, 3, 1, 1, 4, 1, 4,
    4, 1, 3, 1, 1, 4, 3, 3, 4, 4, 3, 3, 3, 4, 4, 1, 4, 4, 1, 2, 2, 4, 3, 3, 3, 2, 2, 2, 1, 4, 4, 4,
    1, 1, 3, 3, 4, 1, 4, 4, 4, 4, 3, 1, 2, 3, 2, 3, 4, 4, 4, 4, 4, 4, 4, 4, 4, 1, 4, 1, 1, 2, 4, 4,
    4, 4, 4, 4, 2, 3,
];

/// Compute the 37-radix extension id for `ext`. Mirrors `get_ext_id`
/// in `pclsync/psuggest.c`. Returns 0 if the string contains any
/// character outside the accepted alphabet.
fn get_ext_id(ext: &str) -> u32 {
    let mut n: u32 = 0;
    for byte in ext.bytes() {
        let c = CHARACTER_MAP[byte as usize];
        if c == 0 {
            return 0;
        }
        // Use wrapping arithmetic to match the C uint32_t overflow semantics.
        n = n.wrapping_mul(37).wrapping_add(c as u32);
    }
    n
}

/// Map an extension id back to its bucket using binary search.
/// Mirrors `get_extid_type` in `pclsync/psuggest.c`.
fn get_extid_type(extid: u32) -> u32 {
    if extid == 0 {
        return 0;
    }
    match SCAN_EXTENSIONS.binary_search(&extid) {
        Ok(idx) => SCAN_TYPES[idx] as u32,
        Err(_) => 0,
    }
}

/// Classify a filename into one of the [`SCAN_TYPES_CNT`] buckets.
/// Mirrors `get_file_type` in `pclsync/psuggest.c`.
fn get_file_type(name: &str) -> u32 {
    match name.rfind('.') {
        Some(idx) => get_extid_type(get_ext_id(&name[idx + 1..])),
        None => 0,
    }
}

struct ScanNode {
    path: PathBuf,
    counts: [u32; SCAN_TYPES_CNT],
    children: Vec<ScanNode>,
}

struct ScanBudget {
    entries_seen: usize,
    cap: usize,
}

impl ScanBudget {
    fn new(cap: usize) -> Self {
        Self {
            entries_seen: 0,
            cap,
        }
    }

    fn tick(&mut self) -> bool {
        if self.entries_seen >= self.cap {
            return false;
        }
        self.entries_seen += 1;
        true
    }
}

fn scan_recursive(path: &Path, depth: usize, budget: &mut ScanBudget) -> ScanNode {
    let mut node = ScanNode {
        path: path.to_path_buf(),
        counts: [0; SCAN_TYPES_CNT],
        children: Vec::new(),
    };

    let Ok(iter) = std::fs::read_dir(path) else {
        return node;
    };

    // Collect and sort entries deterministically.
    let mut entries: Vec<(std::ffi::OsString, std::fs::FileType)> = Vec::new();
    for entry in iter.flatten() {
        if !budget.tick() {
            break;
        }
        let Ok(ty) = entry.file_type() else { continue };
        entries.push((entry.file_name(), ty));
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));

    for (file_name, ty) in entries {
        let name_lossy = file_name.to_string_lossy();
        // Mirrors C ignore_patters = { ".*" }.
        if name_lossy.starts_with('.') {
            continue;
        }
        let child_path = path.join(&file_name);
        if ty.is_dir() {
            if depth + 1 >= MAX_SCAN_DEPTH {
                continue;
            }
            let child = scan_recursive(&child_path, depth + 1, budget);
            for i in 0..SCAN_TYPES_CNT {
                node.counts[i] = node.counts[i].saturating_add(child.counts[i]);
            }
            node.children.push(child);
        } else if ty.is_file() {
            let bucket = get_file_type(&name_lossy) as usize;
            let bucket = if bucket < SCAN_TYPES_CNT { bucket } else { 0 };
            node.counts[bucket] = node.counts[bucket].saturating_add(1);
        }
    }
    node
}

fn non_other_sum(counts: &[u32; SCAN_TYPES_CNT]) -> u32 {
    counts
        .iter()
        .skip(1)
        .copied()
        .fold(0u32, |acc, v| acc.saturating_add(v))
}

/// Mirrors `suggest_folders` in `pclsync/psuggest.c`: emit this folder
/// if it passes the ratio/threshold test, otherwise recurse into its
/// children.
fn collect_suggestions(node: &ScanNode, out: &mut Vec<SuggestedFolder>) {
    let sum = non_other_sum(&node.counts);
    let total = node.counts[0].saturating_add(sum);
    let threshold_num = total.saturating_mul(SCANNER_PERCENT) / 100;
    if sum >= SCANNER_MIN_FILES && sum >= threshold_num {
        out.push(materialize(node, sum));
        return;
    }
    for child in &node.children {
        collect_suggestions(child, out);
    }
}

fn materialize(node: &ScanNode, sum: u32) -> SuggestedFolder {
    let path_string = node.path.display().to_string();
    let name = node
        .path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path_string.clone());
    SuggestedFolder {
        local_path: path_string,
        name,
        description: build_description(&node.counts),
        file_count: sum,
        type_counts: node.counts,
    }
}

fn build_description(counts: &[u32; SCAN_TYPES_CNT]) -> String {
    // Sort non-other buckets by descending count, tie-break by bucket id
    // (matches the `qsort` with `sort_comp_tuple_rev` in C: stable tie
    // on count; we also tie by original index for determinism).
    let mut ranked: Vec<(u32, usize)> = (1..SCAN_TYPES_CNT).map(|i| (counts[i], i)).collect();
    ranked.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));

    let mut parts: Vec<String> = Vec::new();
    for (count, idx) in ranked {
        if count >= SCANNER_MIN_DISPLAY {
            parts.push(format!("{} {}", count, SCAN_TYPE_NAMES[idx]));
        }
    }
    parts.join(", ")
}

/// Public entry point equivalent to `psuggest_scan_folder`. Scans
/// `root`, returns up to [`SCANNER_MAX_SUGGESTIONS`] folders sorted by
/// descending non-"other" file count.
///
/// # Example
///
/// ```no_run
/// use std::path::Path;
/// // Walks the local filesystem; requires a real path to run.
/// let suggestions = pcloud_backends::sync_suggest::scan_folder(
///     Path::new("/home/alice"),
/// ).unwrap();
/// for s in suggestions {
///     println!("{:?}", s);
/// }
/// ```
pub fn scan_folder(root: &Path) -> io::Result<Vec<SuggestedFolder>> {
    scan_folder_with_limit(root, SCANNER_MAX_SUGGESTIONS)
}

/// Variant of [`scan_folder`] that caps the number of suggestions at
/// `max`. `max == 0` is coerced to 1 to match the shape of the existing
/// daemon entry point.
pub fn scan_folder_with_limit(root: &Path, max: usize) -> io::Result<Vec<SuggestedFolder>> {
    let canonical = std::fs::canonicalize(root)?;
    let mut budget = ScanBudget::new(MAX_SCAN_ENTRIES);
    let tree = scan_recursive(&canonical, 0, &mut budget);

    let mut suggestions: Vec<SuggestedFolder> = Vec::new();
    collect_suggestions(&tree, &mut suggestions);

    // Stable sort: primary = descending file_count, tie break = path
    // so output is deterministic across runs and filesystems.
    suggestions.sort_by(|a, b| {
        b.file_count
            .cmp(&a.file_count)
            .then_with(|| a.local_path.cmp(&b.local_path))
    });
    let cap = max.max(1);
    suggestions.truncate(cap);
    Ok(suggestions)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    struct TempDir(PathBuf);
    impl TempDir {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "pcloud-suggest-{}-{}-{}",
                tag,
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn touch(p: &Path) {
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(p, b"x").unwrap();
    }

    #[test]
    fn ext_id_matches_c_hash() {
        // Spot check a handful of known (ext, id, type) tuples from
        // pclsync/pscanexts.h.
        assert_eq!(get_ext_id("txt"), 42358);
        assert_eq!(get_ext_id("TXT"), 42358); // case folded
        assert_eq!(get_ext_id("jpg"), 28359);
        assert_eq!(get_ext_id("pdf"), 36128);
        assert_eq!(get_ext_id("docx"), 743882);
        assert_eq!(get_ext_id("mp3"), 32453);
        assert_eq!(get_ext_id("mpeg"), 1201185);

        // Non-alphanumeric -> 0.
        assert_eq!(get_ext_id("a-b"), 0);
        assert_eq!(get_ext_id(""), 0);
    }

    #[test]
    fn ext_type_lookup() {
        // txt -> documents (4)
        assert_eq!(get_extid_type(42358), 4);
        // jpg -> pictures (1)
        assert_eq!(get_extid_type(28359), 1);
        // mp3 -> music (3)
        assert_eq!(get_extid_type(32453), 3);
        // mov -> videos (2)
        assert_eq!(get_extid_type(32444), 2);
        // unknown id -> other (0)
        assert_eq!(get_extid_type(999_999_999), 0);
    }

    #[test]
    fn file_type_classification() {
        assert_eq!(get_file_type("report.pdf"), 4);
        assert_eq!(get_file_type("holiday.JPG"), 1);
        assert_eq!(get_file_type("track.mp3"), 3);
        assert_eq!(get_file_type("clip.mp4"), 2);
        assert_eq!(get_file_type("readme"), 0);
        assert_eq!(get_file_type("archive.xyz"), 0);
    }

    #[test]
    fn suggests_document_heavy_folder() {
        let tmp = TempDir::new("docs");
        let docs = tmp.path().join("MyDocs");
        for i in 0..30 {
            touch(&docs.join(format!("note{i}.txt")));
        }
        // A few noise files, still under the 20% "other" threshold.
        for i in 0..3 {
            touch(&docs.join(format!("n{i}.dat")));
        }
        // Push the root below the ratio by planting many "other" files
        // at the top level so the scorer must descend into MyDocs.
        for i in 0..500 {
            touch(&tmp.path().join(format!("noise{i}.dat")));
        }
        let res = scan_folder(tmp.path()).unwrap();
        assert_eq!(res.len(), 1);
        assert_eq!(res[0].name, "MyDocs");
        assert_eq!(res[0].file_count, 30);
        assert!(res[0].description.contains("documents"));
    }

    #[test]
    fn rejects_other_dominated_folder() {
        let tmp = TempDir::new("other");
        let junk = tmp.path().join("Junk");
        for i in 0..40 {
            touch(&junk.join(format!("f{i}.dat")));
        }
        for i in 0..5 {
            touch(&junk.join(format!("p{i}.jpg")));
        }
        // Only 5 pictures < MIN_FILES (25), so nothing is suggested.
        let res = scan_folder(tmp.path()).unwrap();
        assert!(res.is_empty(), "unexpected suggestion: {:?}", res);
    }

    #[test]
    fn ranks_folders_by_non_other_count() {
        let tmp = TempDir::new("rank");
        let pics = tmp.path().join("Pictures");
        let tunes = tmp.path().join("Music");
        for i in 0..60 {
            touch(&pics.join(format!("i{i}.jpg")));
        }
        for i in 0..30 {
            touch(&tunes.join(format!("t{i}.mp3")));
        }
        // Force the root below the ratio so the scorer descends and
        // reports the two leaf folders separately.
        for i in 0..500 {
            touch(&tmp.path().join(format!("x{i}.dat")));
        }
        let res = scan_folder(tmp.path()).unwrap();
        assert_eq!(res.len(), 2);
        assert_eq!(res[0].name, "Pictures");
        assert_eq!(res[0].file_count, 60);
        assert_eq!(res[1].name, "Music");
        assert_eq!(res[1].file_count, 30);
    }

    #[test]
    fn empty_directory_yields_nothing() {
        let tmp = TempDir::new("empty");
        let res = scan_folder(tmp.path()).unwrap();
        assert!(res.is_empty());
    }

    #[test]
    fn hidden_entries_are_ignored() {
        let tmp = TempDir::new("hidden");
        let hidden = tmp.path().join(".secret");
        for i in 0..40 {
            touch(&hidden.join(format!("x{i}.txt")));
        }
        let res = scan_folder(tmp.path()).unwrap();
        assert!(res.is_empty());
    }

    #[test]
    fn deeply_nested_tree_is_bounded() {
        let tmp = TempDir::new("deep");
        // Build a chain deeper than MAX_SCAN_DEPTH.
        let mut cur = tmp.path().to_path_buf();
        for i in 0..(MAX_SCAN_DEPTH + 5) {
            cur = cur.join(format!("d{i}"));
            fs::create_dir_all(&cur).unwrap();
            touch(&cur.join("a.txt"));
        }
        // Should not panic or blow up, and should return at most the
        // suggestion cap.
        let res = scan_folder(tmp.path()).unwrap();
        assert!(res.len() <= SCANNER_MAX_SUGGESTIONS);
    }

    #[test]
    fn unicode_paths_are_handled() {
        let tmp = TempDir::new("utf8");
        let folder = tmp.path().join("Документы");
        for i in 0..30 {
            touch(&folder.join(format!("заметка{i}.txt")));
        }
        // Force root below ratio so the unicode child is reported.
        for i in 0..500 {
            touch(&tmp.path().join(format!("z{i}.dat")));
        }
        let res = scan_folder(tmp.path()).unwrap();
        assert_eq!(res.len(), 1);
        assert_eq!(res[0].name, "Документы");
        assert!(res[0].file_count >= SCANNER_MIN_FILES);
    }

    #[test]
    fn deterministic_ordering_on_equal_counts() {
        let tmp = TempDir::new("det");
        let a = tmp.path().join("A");
        let b = tmp.path().join("B");
        for i in 0..30 {
            touch(&a.join(format!("a{i}.pdf")));
            touch(&b.join(format!("b{i}.pdf")));
        }
        // Force recursion into A/B.
        for i in 0..500 {
            touch(&tmp.path().join(format!("w{i}.dat")));
        }
        let res1 = scan_folder(tmp.path()).unwrap();
        let res2 = scan_folder(tmp.path()).unwrap();
        assert_eq!(res1, res2);
        assert_eq!(res1[0].name, "A");
        assert_eq!(res1[1].name, "B");
    }

    #[test]
    fn suggestion_cap_is_respected() {
        let tmp = TempDir::new("cap");
        // Create SCANNER_MAX_SUGGESTIONS + 2 qualifying folders.
        for k in 0..(SCANNER_MAX_SUGGESTIONS + 2) {
            let f = tmp.path().join(format!("F{k}"));
            for i in 0..30 {
                touch(&f.join(format!("x{i}.txt")));
            }
        }
        // Force recursion into children.
        for i in 0..2000 {
            touch(&tmp.path().join(format!("n{i}.dat")));
        }
        let res = scan_folder(tmp.path()).unwrap();
        assert_eq!(res.len(), SCANNER_MAX_SUGGESTIONS);
    }
}
