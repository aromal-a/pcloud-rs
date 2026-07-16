#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![allow(clippy::pedantic)]
//! T2.5.b — wasmtime-backed `PluginBackend` for `pcloud-plugin-host`.
//!
//! # Why a separate crate
//!
//! `wasmtime` pulls cranelift + wasmparser + several MB of codegen
//! infrastructure. `pcloud-plugin-host` is supposed to stay
//! dep-light so that consumers who only want the capability model +
//! message bus (and a `NoopBackend` for tests) are not paying for
//! the full wasm runtime. This crate is the opt-in heavy backend.
//!
//! # Acceptance pivot
//!
//! T2.5 plan acceptance is *"a sample plugin runs sandboxed; an
//! attempted `fs::write` from inside the plugin is denied"*. The
//! wasm equivalent of `fs::write` is the WASI
//! `wasi_snapshot_preview1.fd_write` import. By **not** providing
//! WASI imports to the linker, any module that imports `fd_write`
//! fails to instantiate — exactly the deny path the plan calls
//! for. The unit test `wasmtime_module_with_fs_import_fails_to_load`
//! exercises this with a hand-written wasm module.
//!
//! # What this crate ships
//!
//! - [`WasmtimeBackend`] — `PluginBackend` impl that owns one
//!   `wasmtime::Engine`, one `Linker` with **no** WASI imports
//!   wired, and a `HashMap<PluginId, Module>` of pre-validated
//!   modules.
//! - The plugin-side host-call wiring (so a wasm module can call
//!   back into `HostBus`) is a separate follow-up step. `deliver`
//!   currently just confirms the module is in the registry.

// **PLATFORM:** all (wasmtime supports linux/macos/windows).
// **GATING:** none.

use std::collections::HashMap;

use pcloud_plugin_host::{HostError, HostResponse, PluginBackend, PluginId};
use wasmtime::{Engine, Linker, Module, Store};

/// Errors raised by the wasmtime backend before they are
/// translated into [`HostError`].
#[derive(Debug, thiserror::Error)]
pub enum WasmtimeBackendError {
    /// The wasmtime engine itself failed to construct (bad config).
    #[error("failed to construct wasmtime engine: {0}")]
    Engine(String),
    /// `Module::new` rejected the bytes (not a valid wasm module).
    #[error("invalid wasm module: {0}")]
    InvalidModule(String),
    /// The module instantiated against an empty linker — i.e. it
    /// imported a host function we do not provide. This is the
    /// deny path for sandboxed plugins.
    #[error("module imports a forbidden host function: {0}")]
    ForbiddenImport(String),
    /// `deliver` was called for a plugin id that was never `load`ed.
    #[error("plugin {0:?} not loaded")]
    UnknownPlugin(String),
}

impl From<WasmtimeBackendError> for HostError {
    fn from(err: WasmtimeBackendError) -> Self {
        HostError::LoadFailed(err.to_string())
    }
}

/// Wasmtime-backed plugin runtime. Kept deliberately minimal: the
/// engine has WASI **disabled**, so any plugin that imports a WASI
/// function (e.g. `fd_write`) fails at instantiate time — that is
/// the sandbox.
pub struct WasmtimeBackend {
    engine: Engine,
    modules: HashMap<PluginId, Module>,
}

impl std::fmt::Debug for WasmtimeBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WasmtimeBackend")
            .field("modules_loaded", &self.modules.len())
            .finish_non_exhaustive()
    }
}

impl WasmtimeBackend {
    /// Construct a new backend with a default `wasmtime::Engine`.
    ///
    /// # Errors
    ///
    /// Returns [`WasmtimeBackendError::Engine`] if wasmtime refuses
    /// the default config (extremely unlikely; almost always
    /// indicates a misbuilt wasmtime feature combination).
    pub fn new() -> Result<Self, WasmtimeBackendError> {
        let engine = Engine::default();
        Ok(Self {
            engine,
            modules: HashMap::new(),
        })
    }

    /// How many modules are currently loaded. Diagnostics-only.
    #[must_use]
    pub fn loaded_count(&self) -> usize {
        self.modules.len()
    }

    /// Validate a module by attempting to instantiate it against an
    /// **empty** linker. This is the heart of the sandbox: any
    /// import that the module declares (WASI, env-imports, etc.)
    /// will fail to resolve and we surface that as
    /// [`WasmtimeBackendError::ForbiddenImport`].
    ///
    /// We do not keep the resulting `Instance` — `deliver` does
    /// not (yet) call into the plugin. The instantiation here is
    /// purely a validation gate at load time.
    fn validate_no_imports(&self, module: &Module) -> Result<(), WasmtimeBackendError> {
        let linker: Linker<()> = Linker::new(&self.engine);
        let mut store = Store::new(&self.engine, ());
        match linker.instantiate(&mut store, module) {
            Ok(_instance) => Ok(()),
            Err(e) => Err(WasmtimeBackendError::ForbiddenImport(e.to_string())),
        }
    }
}

impl PluginBackend for WasmtimeBackend {
    fn load(&mut self, plugin_id: &PluginId, module_bytes: &[u8]) -> Result<(), HostError> {
        let module = Module::new(&self.engine, module_bytes)
            .map_err(|e| WasmtimeBackendError::InvalidModule(e.to_string()))?;
        // Sandbox gate: refuse modules that import host functions
        // we have not whitelisted (currently: nothing).
        self.validate_no_imports(&module)?;
        log::debug!(
            "pcloud-plugin-wasmtime: loaded module for plugin {:?}",
            plugin_id.as_str()
        );
        self.modules.insert(plugin_id.clone(), module);
        Ok(())
    }

    fn deliver(&mut self, plugin_id: &PluginId, _response: &HostResponse) -> Result<(), HostError> {
        // Placeholder: confirm the module is registered. Full
        // host-call wiring (so the plugin can `recv()` the
        // response) is a separate follow-up step.
        if !self.modules.contains_key(plugin_id) {
            return Err(WasmtimeBackendError::UnknownPlugin(plugin_id.as_str().to_owned()).into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcloud_plugin_host::HostResponse;

    fn pid(s: &str) -> PluginId {
        PluginId::new(s).unwrap()
    }

    /// Minimal valid wasm module: 4-byte magic + 4-byte version, no
    /// sections. wasmtime accepts this and there are no imports to
    /// resolve, so `validate_no_imports` succeeds.
    const EMPTY_WASM: &[u8] = &[0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];

    /// Hand-written wasm module that imports `wasi_snapshot_preview1.fd_write`.
    /// Layout:
    ///   magic(4) + version(4)
    ///   type section (id=1):
    ///     1 type: func (i32, i32, i32, i32) -> i32
    ///   import section (id=2):
    ///     1 import: "wasi_snapshot_preview1" . "fd_write" : func type 0
    ///
    /// The default `Linker<()>` has nothing wired for that import,
    /// so `linker.instantiate` returns a "function not found"-class
    /// error and we surface it as `ForbiddenImport`.
    fn fd_write_import_wasm() -> Vec<u8> {
        let mut m = Vec::new();
        // magic + version
        m.extend_from_slice(&[0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00]);

        // Type section (id=1).
        // 1 type entry: func, 4 i32 params, 1 i32 result.
        // body: 0x01 (count), 0x60 (func), 0x04 (param count),
        //       0x7f 0x7f 0x7f 0x7f (i32 x4), 0x01 (result count),
        //       0x7f (i32)
        let type_body: [u8; 9] = [0x01, 0x60, 0x04, 0x7f, 0x7f, 0x7f, 0x7f, 0x01, 0x7f];
        m.push(0x01); // section id
        m.push(type_body.len() as u8); // section size (LEB128 single-byte for <128)
        m.extend_from_slice(&type_body);

        // Import section (id=2).
        // 1 import: module "wasi_snapshot_preview1", field "fd_write",
        //          desc = func, type index 0.
        let module_name = b"wasi_snapshot_preview1";
        let field_name = b"fd_write";
        let mut import_body: Vec<u8> = Vec::new();
        import_body.push(0x01); // count = 1
        import_body.push(module_name.len() as u8);
        import_body.extend_from_slice(module_name);
        import_body.push(field_name.len() as u8);
        import_body.extend_from_slice(field_name);
        import_body.push(0x00); // import desc kind = func
        import_body.push(0x00); // type index 0
        m.push(0x02); // section id
        m.push(import_body.len() as u8);
        m.extend_from_slice(&import_body);

        m
    }

    #[test]
    fn wasmtime_backend_constructs() {
        let be = WasmtimeBackend::new().expect("engine builds");
        assert_eq!(be.loaded_count(), 0);
    }

    /// Acceptance: a no-import sample module loads cleanly, confirming
    /// the sandbox is not pathologically over-strict.
    #[test]
    fn wasmtime_module_with_no_imports_loads_ok() {
        let mut be = WasmtimeBackend::new().unwrap();
        be.load(&pid("empty"), EMPTY_WASM)
            .expect("empty wasm with no imports must load");
        assert_eq!(be.loaded_count(), 1);
    }

    /// Deny-path acceptance pivot. A module that imports
    /// `wasi_snapshot_preview1.fd_write` (the wasm equivalent of
    /// `fs::write`) MUST fail to load against the empty linker.
    #[test]
    fn wasmtime_module_with_fs_import_fails_to_load() {
        let mut be = WasmtimeBackend::new().unwrap();
        let bytes = fd_write_import_wasm();
        // `Module::new` on its own may succeed (the module is
        // structurally well-formed). The denial must come from the
        // instantiate-against-empty-linker step inside our `load`.
        let err = be
            .load(&pid("fs-attacker"), &bytes)
            .expect_err("module that imports fd_write must be denied");
        match err {
            HostError::LoadFailed(msg) => {
                // Message is wasmtime-version-dependent; just assert
                // the offending import name is mentioned.
                assert!(
                    msg.contains("fd_write")
                        || msg.contains("wasi_snapshot_preview1")
                        || msg.contains("unknown import")
                        || msg.contains("forbidden"),
                    "unexpected error message: {msg}"
                );
            }
            other => panic!("expected LoadFailed, got {other:?}"),
        }
        // Module must NOT be registered after a denied load.
        assert_eq!(be.loaded_count(), 0);
    }

    #[test]
    fn deliver_unknown_plugin_errors() {
        let mut be = WasmtimeBackend::new().unwrap();
        let resp = HostResponse::AuditAck;
        let err = be
            .deliver(&pid("ghost"), &resp)
            .expect_err("ghost not loaded");
        match err {
            HostError::LoadFailed(msg) => {
                assert!(msg.contains("ghost"), "unexpected: {msg}");
            }
            other => panic!("expected LoadFailed, got {other:?}"),
        }
    }

    #[test]
    fn deliver_known_plugin_ok() {
        let mut be = WasmtimeBackend::new().unwrap();
        be.load(&pid("p"), EMPTY_WASM).unwrap();
        be.deliver(&pid("p"), &HostResponse::AuditAck).unwrap();
    }

    #[test]
    fn invalid_bytes_rejected() {
        let mut be = WasmtimeBackend::new().unwrap();
        // Random non-wasm bytes.
        let err = be
            .load(&pid("junk"), b"this is not wasm at all")
            .expect_err("bad bytes must be rejected");
        match err {
            HostError::LoadFailed(_) => {}
            other => panic!("expected LoadFailed, got {other:?}"),
        }
    }
}
