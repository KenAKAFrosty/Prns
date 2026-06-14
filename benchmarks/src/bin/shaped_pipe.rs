//! A constrained wire between two scenario nodes: `shaped_pipe <manifest.json> pipe
//! <target-addr>`. Binds an ephemeral port (its READY line carries the address, like any
//! responder), connects each inbound session to the target, and forwards bytes under the
//! manifest's `wire_shape`: a serializing rate limit — the transmitter occupies the channel
//! for `bytes×8/rate_bps`, which is what LoRa airtime is — plus a fixed one-way latency
//! (preamble + propagation). Backpressure is the read loop itself: while a chunk serializes,
//! nothing more is read, so TCP flow control pushes back to the sender exactly like a radio's
//! small buffer would. Every forwarded byte is counted, and when a session closes the totals
//! go out on a `WIRE a_to_b_bytes=… b_to_a_bytes=…` line — payload divided by wire bytes is
//! the protocol's overhead ratio, measured where it can't lie. Forwarding happens in
//! 64-byte slices with individual release times: a real wire delivers a clump's leading
//! frame after only its own airtime, so chunk-atomic delivery would invent a convoy
//! penalty the physics doesn't charge — slices grow with the rate (about four milliseconds of
//! airtime, 64 B to 64 KiB) since sub-millisecond sleeps collapse into scheduler granularity
//! on high-rate pipes and silently meter the benchmark below the configured wire rate. The
//! release schedule is absolute — each slice's
//! due time advances by exact airtime, the clock only re-clamps to now when the source
//! went idle, and backpressure is the bounded queue — so timer rounding delays a write
//! without ever stealing channel throughput.

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
        let Ok(outbound) = TcpStream::connect(target.as_str()).await else {
            eprintln!("pipe target closed; exiting");
            drop(inbound);
            break;
        };
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

    let mut buffer = [0u8; 65_536];
    let mut channel_free_at = tokio::time::Instant::now();
    'session: loop {
        let read = match reader.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        counter.fetch_add(read as u64, Ordering::Relaxed);
        channel_free_at = channel_free_at.max(tokio::time::Instant::now());
        let slice_len = (rate_bps as usize / 2_000).clamp(64, 65_536);
        for slice in buffer[..read].chunks(slice_len) {
            let airtime = Duration::from_secs_f64(slice.len() as f64 * 8.0 / rate_bps as f64);
            channel_free_at += airtime;
            if delayed_tx
                .send((channel_free_at + latency, slice.to_vec()))
                .await
                .is_err()
            {
                break 'session;
            }
        }
    }
    drop(delayed_tx);
    let _ = deliver.await;
}
