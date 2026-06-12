//! A constrained wire between two scenario nodes: `shaped_pipe <manifest.json> pipe
//! <target-addr>`. Binds an ephemeral port (its READY line carries the address, like any
//! responder), connects each inbound session to the target, and forwards bytes under the
//! manifest's `wire_shape`: a serializing rate limit — the transmitter occupies the channel
//! for `bytes×8/rate_bps`, which is what LoRa airtime is — plus a fixed one-way latency
//! (preamble + propagation). Backpressure is the read loop itself: while a chunk serializes,
//! nothing more is read, so TCP flow control pushes back to the sender exactly like a radio's
//! small buffer would. Every forwarded byte is counted, and when a session closes the totals
//! go out on a `WIRE a_to_b_bytes=… b_to_a_bytes=…` line — payload divided by wire bytes is
//! the protocol's overhead ratio, measured where it can't lie.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

#[derive(serde::Deserialize)]
struct Manifest {
    profile: Profile,
}

#[derive(serde::Deserialize)]
struct Profile {
    wire_shape: WireShape,
}

#[derive(serde::Deserialize)]
struct WireShape {
    rate_bps: u64,
    latency_ms: u64,
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (manifest_path, target) = (&args[1], &args[3]);
    let manifest: Manifest =
        serde_json::from_str(&std::fs::read_to_string(manifest_path).expect("reads the manifest"))
            .expect("manifest carries a wire_shape");
    let shape = manifest.profile.wire_shape;

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("binds");
    let bound = listener.local_addr().expect("bound");
    println!("READY role=pipe addr=127.0.0.1:{}", bound.port());

    let a_to_b = Arc::new(AtomicU64::new(0));
    let b_to_a = Arc::new(AtomicU64::new(0));
    loop {
        let (inbound, _) = listener.accept().await.expect("accepts");
        let outbound = TcpStream::connect(target.as_str())
            .await
            .expect("reaches the target");
        inbound.set_nodelay(true).ok();
        outbound.set_nodelay(true).ok();
        let (in_read, in_write) = inbound.into_split();
        let (out_read, out_write) = outbound.into_split();
        let forward = tokio::spawn(pump(
            in_read,
            out_write,
            shape.rate_bps,
            shape.latency_ms,
            a_to_b.clone(),
        ));
        let backward = tokio::spawn(pump(
            out_read,
            in_write,
            shape.rate_bps,
            shape.latency_ms,
            b_to_a.clone(),
        ));
        let _ = forward.await;
        let _ = backward.await;
        println!(
            "WIRE a_to_b_bytes={} b_to_a_bytes={}",
            a_to_b.load(Ordering::Relaxed),
            b_to_a.load(Ordering::Relaxed),
        );
    }
}

async fn pump(
    mut reader: tokio::net::tcp::OwnedReadHalf,
    mut writer: tokio::net::tcp::OwnedWriteHalf,
    rate_bps: u64,
    latency_ms: u64,
    counter: Arc<AtomicU64>,
) {
    let latency = Duration::from_millis(latency_ms);
    let (delayed_tx, mut delayed_rx) = mpsc::channel::<(tokio::time::Instant, Vec<u8>)>(4);
    let deliver = tokio::spawn(async move {
        while let Some((due, chunk)) = delayed_rx.recv().await {
            tokio::time::sleep_until(due).await;
            if writer.write_all(&chunk).await.is_err() {
                break;
            }
        }
        let _ = writer.shutdown().await;
    });

    let mut buffer = [0u8; 2048];
    let mut channel_free_at = tokio::time::Instant::now();
    loop {
        let read = match reader.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        counter.fetch_add(read as u64, Ordering::Relaxed);
        let airtime = Duration::from_secs_f64(read as f64 * 8.0 / rate_bps as f64);
        let now = tokio::time::Instant::now();
        channel_free_at = channel_free_at.max(now) + airtime;
        tokio::time::sleep_until(channel_free_at).await;
        if delayed_tx
            .send((channel_free_at + latency, buffer[..read].to_vec()))
            .await
            .is_err()
        {
            break;
        }
    }
    drop(delayed_tx);
    let _ = deliver.await;
}
