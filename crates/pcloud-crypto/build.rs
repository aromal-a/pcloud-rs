#![allow(clippy::pedantic)]
//! Build script: emits the `[[u8; 8]; N]` Rust const `PASSWORD_DICT` used by
//! the password scorer.
//!
//! Source of truth order:
//!
//! 1. If the legacy C header `pclsync/ppassworddict.h` is present at the
//!    expected location (three levels up from this crate), parse it and emit
//!    the dictionary byte-for-byte from that file. This preserves lock-step
//!    with any upstream dictionary change when the C tree is checked out
//!    alongside the Rust workspace.
//! 2. Otherwise, fall back to the vendored copy at
//!    `crates/pcloud-crypto/vendored/password_dict.rs`, which was generated
//!    once from the header and committed into the repository so the Rust
//!    workspace builds standalone (the upstream C sources have been removed
//!    from this fork — see `CLAUDE.md`).
//!
//! If neither source is available, the build script aborts: an empty
//! dictionary would silently weaken the password scorer and must never be
//! emitted accidentally.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    let manifest_dir = PathBuf::from(
        env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR must be set by Cargo"),
    );

    // Candidate 1: the upstream C header (when the C tree is co-checked-out).
    // crates/pcloud-crypto -> ../../../pclsync/ppassworddict.h
    let header = manifest_dir
        .join("..")
        .join("..")
        .join("..")
        .join("pclsync")
        .join("ppassworddict.h");

    // Candidate 2: the in-tree vendored fallback.
    let vendored = manifest_dir.join("vendored").join("password_dict.rs");

    // Tell cargo to re-run when either input changes.
    println!("cargo:rerun-if-changed={}", header.display());
    println!("cargo:rerun-if-changed={}", vendored.display());

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR must be set by Cargo"));
    let out_path = out_dir.join("password_dict.rs");

    if header.is_file() {
        emit_from_c_header(&header, &out_path);
        return;
    }

    if vendored.is_file() {
        fs::copy(&vendored, &out_path).unwrap_or_else(|e| {
            panic!(
                "failed to copy vendored dictionary {} -> {}: {e}",
                vendored.display(),
                out_path.display()
            )
        });
        return;
    }

    panic!(
        "pcloud-crypto build script could not locate a password dictionary source.\n\
         Tried:\n  \
         1. legacy C header: {}\n  \
         2. vendored fallback: {}\n\
         Restore one of them, or commit a fresh vendored dictionary to {}.",
        header.display(),
        vendored.display(),
        vendored.display()
    );
}

/// Parse the legacy C header byte-for-byte and write `password_dict.rs` into
/// `out_path`. Aborts on any malformed line or empty result — an empty
/// dictionary would silently degrade the password scorer.
fn emit_from_c_header(header: &Path, out_path: &Path) {
    let src = fs::read_to_string(header)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", header.display()));

    // Each entry line looks like:
    //   {0x31, 0x39, 0x34, 0x32, 0x00, 0x00, 0x00, 0x00},
    // We collect all such 8-tuples in source order.
    let mut entries: Vec<[u8; 8]> = Vec::with_capacity(8600);
    for line in src.lines() {
        let l = line.trim();
        if !l.starts_with('{') || !l.contains("0x") {
            continue;
        }
        let inner = l
            .trim_start_matches('{')
            .trim_end_matches(|c: char| c == ',' || c.is_whitespace())
            .trim_end_matches('}');
        let mut bytes = [0u8; 8];
        let mut i = 0usize;
        for tok in inner.split(',') {
            let tok = tok.trim();
            if tok.is_empty() {
                continue;
            }
            let hex = tok
                .strip_prefix("0x")
                .or_else(|| tok.strip_prefix("0X"))
                .unwrap_or(tok);
            let v = u8::from_str_radix(hex, 16)
                .unwrap_or_else(|e| panic!("bad byte {tok:?} in dict line {l:?}: {e}"));
            bytes[i] = v;
            i += 1;
            if i == 8 {
                break;
            }
        }
        if i == 8 {
            entries.push(bytes);
        }
    }

    if entries.is_empty() {
        panic!(
            "password dictionary parse produced 0 entries from {}",
            header.display()
        );
    }

    let mut out = String::with_capacity(entries.len() * 64);
    out.push_str("// AUTO-GENERATED from pclsync/ppassworddict.h. Do not edit.\n");
    out.push_str("#[allow(clippy::large_stack_arrays, clippy::large_const_arrays)]\n");
    out.push_str(&format!(
        "pub(crate) static PASSWORD_DICT: [[u8; 8]; {}] = [\n",
        entries.len()
    ));
    for e in &entries {
        out.push_str("    [");
        for (i, b) in e.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            out.push_str(&format!("0x{b:02x}"));
        }
        out.push_str("],\n");
    }
    out.push_str("];\n");

    fs::write(out_path, out)
        .unwrap_or_else(|e| panic!("failed to write {}: {e}", out_path.display()));
}
