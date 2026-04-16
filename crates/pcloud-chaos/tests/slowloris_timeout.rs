#![allow(clippy::pedantic)]
//! Scenario 5: slowloris partial response → per-request timeout fires.
//!
//! We bind a loopback TCP listener that accepts a client, then trickles
//! exactly one byte per second and never closes. A "daemon-like" reader
//! tries to read a response with a 1-second per-request timeout wrapping
//! the whole read loop. We assert:
//!
//!   * the timeout fires before 3 s (budget),
//!   * the daemon-side buffer never grows beyond a small cap (no OOM),
//!   * the error is a typed `ElapsedError`, not a panic.
//!
//! Gated behind `#[ignore]` + `PCLOUD_CHAOS=1`.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};

const READ_CAP: usize = 4096;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "chaos: slow-by-design, requires PCLOUD_CHAOS=1"]
async fn chaos_slowloris_timeout() {
    if !pcloud_chaos::chaos_enabled() {
        let _ = pcloud_chaos::skip(
            "chaos_slowloris_timeout",
            "PCLOUD_CHAOS != 1 (set to 1 to run)",
        );
        return;
    }

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    let stop = Arc::new(AtomicBool::new(false));
    let stop_srv = stop.clone();

    let server = tokio::spawn(async move {
        let Ok((mut sock, _)) = listener.accept().await else {
            return;
        };
        let mut i: u8 = 0;
        while !stop_srv.load(Ordering::SeqCst) {
            let buf = [i; 1];
            if sock.write_all(&buf).await.is_err() {
                return;
            }
            let _ = sock.flush().await;
            i = i.wrapping_add(1);
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });

    let started = Instant::now();
    let res: Result<Result<(), std::io::Error>, tokio::time::error::Elapsed> =
        tokio::time::timeout(Duration::from_secs(1), async {
            let mut sock = TcpStream::connect(addr).await?;
            let mut total = vec![0u8; 0];
            let mut buf = [0u8; 64];
            loop {
                let n = sock.read(&mut buf).await?;
                if n == 0 {
                    break;
                }
                // Enforce a hard read cap so a real OOM would manifest as
                // an error, not unbounded growth.
                assert!(
                    total.len() + n <= READ_CAP,
                    "read buffer exceeded cap: {}+{} > {READ_CAP}",
                    total.len(),
                    n
                );
                total.extend_from_slice(&buf[..n]);
            }
            Ok(())
        })
        .await;

    let elapsed = started.elapsed();
    stop.store(true, Ordering::SeqCst);
    server.abort();
    let _ = server.await;

    // Predicted: outer timeout fired; `res` is Err(Elapsed).
    assert!(
        res.is_err(),
        "expected timeout Elapsed, got completed result: {res:?}"
    );
    assert!(
        elapsed < Duration::from_secs(3),
        "timeout took too long to fire: {elapsed:?}"
    );
}
