# Inventory methodology

## Included files

The generator inventories:

```bash
git ls-files --cached --others --exclude-standard
```

That includes every tracked file and every untracked file not excluded by
`.gitignore`. It intentionally excludes `.git`, Cargo build targets, rendered
mdBook output, fuzz build output, local runtime state, and other ignored
artifacts.

Vendored files are included in the exhaustive file inventory but clearly
classified as upstream code. They are not treated as pcloud-rs architectural
owners.

## File descriptions

Descriptions are derived in this order:

1. Rust module documentation (`//!` or `///`);
2. Markdown title;
3. script or configuration leading comment;
4. known filename role (`Cargo.toml`, `main.rs`, `lib.rs`, tests, benches,
   workflows, packaging assets);
5. extension- and directory-based fallback.

This is deliberately conservative. A generated one-line description is a
navigation hint, not a replacement for reading the file.

## Rust declaration index

For every workspace crate, the generator scans Rust files for named functions
and methods, structs, enums, traits, type aliases, constants, statics, and
modules. Public, restricted-public, and private declarations are indexed. It
records the nearest documentation line plus source file and line number.

The scanner is lexical rather than a compiler frontend. Macro-generated items
and unusual multiline declarations can be absent; rustdoc remains
authoritative for exact public API.

## Package metadata

Cargo package, target, feature, and direct dependency data comes from:

```bash
cargo metadata --format-version 1 --no-deps
```

This ensures workspace membership and renamed packages such as
`pcloud-embedded-sdk` are represented by Cargo's current view rather than
directory-name assumptions.

## Regeneration ownership

Generated output lives under `src/generated/` and is replaced atomically by
the generator. Hand-authored architecture chapters live directly under
`src/`. The rendered site lives under `book/` and is ignored project output.
