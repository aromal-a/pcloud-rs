//! Tiny helper used by the cross-process integration test.
//!
//! Usage: `shm_peek <anchor-path>`
//!
//! Attaches to the SysV shm segment keyed by `ftok(anchor, 'A')`, reads
//! the current payload if `flag == 1`, writes the bytes to stdout, and
//! clears the flag. Exits with status 0 on success, 2 if no payload is
//! ready, and 1 on any error.

// **PLATFORM:** all
// **GATING:** none (portable).

#[cfg(all(
    any(target_os = "linux", target_os = "freebsd"),
    feature = "legacy-shm"
))]
fn main() {
    use pcloud_compat::shm_producer::ShmSegment;
    use std::io::Write as _;

    let mut args = std::env::args().skip(1);
    let anchor = match args.next() {
        Some(p) => std::path::PathBuf::from(p),
        None => {
            eprintln!("usage: shm_peek <anchor-path>");
            std::process::exit(1);
        }
    };
    let seg = match ShmSegment::create(&anchor, 0o600) {
        Ok(s) => s,
        Err(err) => {
            eprintln!("attach error: {err}");
            std::process::exit(1);
        }
    };
    match seg.try_consume() {
        Some(bytes) => {
            let _ = std::io::stdout().lock().write_all(&bytes);
            std::process::exit(0);
        }
        None => std::process::exit(2),
    }
}

#[cfg(not(all(
    any(target_os = "linux", target_os = "freebsd"),
    feature = "legacy-shm"
)))]
fn main() {
    eprintln!("shm_peek requires Linux + --features legacy-shm");
    std::process::exit(1);
}
