fn main() {
    // On macOS, the fuse-t/macFUSE dylib is loaded at runtime via dlopen
    // rather than linked at build time. The `-undefined dynamic_lookup`
    // linker flag tells the macOS static linker to defer resolution of
    // fuse_* symbols until dlopen time instead of erroring at link time.
    // Without this flag, linking on macOS fails with "undefined symbols"
    // for all the fuse_* FFI symbols declared in platform/macos_ffi.rs.
    #[cfg(target_os = "macos")]
    {
        println!("cargo:rustc-link-arg=-Wl,-undefined,dynamic_lookup");
    }
}
