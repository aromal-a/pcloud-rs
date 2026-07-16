# Vendored spin 0.9.8

This directory is the source of `spin` 0.9.8 from upstream commit
`502c9dca17c99762184095c9d64c0aedd1db97ff`. The only local source change is
a crate-level allowance for newer compiler diagnostics that did not exist
when 0.9.8 was published; runtime code is unchanged.

The crates.io release was yanked after upstream moved development to a new
repository. `flume 0.11` and the current policy engine still require the 0.9
API. Keeping the audited source in-tree avoids a yanked release in the locked
dependency graph and preserves offline, reproducible builds.

Remove this directory and the root `[patch.crates-io]` entry once all
dependants support `spin` 0.12 or later.
