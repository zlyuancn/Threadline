//! Manual probe: connect to a host:port and print whatever it sends first.
//!
//! Run with: `cargo test -p tl-core --test live_sniff -- --ignored --nocapture`
//! Set THREADLINE_SNIFF_TARGET="host:port" to change the target.

use std::time::Duration;
use tokio::io::AsyncReadExt;

/// Same target, but with std sockets instead of tokio: tells "tokio" apart from "this process".
#[test]
#[ignore]
fn print_first_bytes_std() {
    use std::io::Read;

    let target =
        std::env::var("THREADLINE_SNIFF_TARGET").unwrap_or_else(|_| "127.0.0.1:22".to_string());
    let mut stream = std::net::TcpStream::connect(&target).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_millis(800)))
        .expect("timeout");
    let mut buf = [0u8; 128];
    match stream.read(&mut buf) {
        Ok(n) if n > 0 => println!(
            "std read {n} bytes: {:?}",
            String::from_utf8_lossy(&buf[..n])
        ),
        other => println!("std no banner: {other:?}"),
    }
}

#[tokio::test]
#[ignore]
async fn print_first_bytes() {
    let target =
        std::env::var("THREADLINE_SNIFF_TARGET").unwrap_or_else(|_| "127.0.0.1:22".to_string());

    for attempt in 1..=2 {
        let stream = tokio::time::timeout(
            Duration::from_secs(5),
            tokio::net::TcpStream::connect(&target),
        )
        .await
        .expect("connect timeout")
        .expect("connect failed");

        println!("[{attempt}] peer = {:?}", stream.peer_addr().ok());

        let mut stream = stream;
        let mut buf = [0u8; 128];
        match tokio::time::timeout(Duration::from_millis(800), stream.read(&mut buf)).await {
            Ok(Ok(n)) if n > 0 => {
                println!(
                    "[{attempt}] read {n} bytes: {:?}",
                    String::from_utf8_lossy(&buf[..n])
                );
            }
            other => println!("[{attempt}] no banner: {other:?}"),
        }
    }
}
