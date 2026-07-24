use std::time::{Duration, Instant};

use benchmarks::microscope::{ResourceCycle, ResourceTransferProfile, RESOURCE_PAYLOAD_LEN};

fn main() {
    let mut args = std::env::args().skip(1);
    let transfers = args
        .next()
        .and_then(|arg| arg.parse::<usize>().ok())
        .unwrap_or(64);
    let payload_len = args
        .next()
        .and_then(|arg| arg.parse::<usize>().ok())
        .unwrap_or(RESOURCE_PAYLOAD_LEN);
    let warmup = args
        .next()
        .and_then(|arg| arg.parse::<usize>().ok())
        .unwrap_or(4);

    let mut cycle = ResourceCycle::new(payload_len);
    for _ in 0..warmup {
        let _ = cycle.transfer_profile();
    }

    let mut total = ResourceTransferProfile::new(payload_len);
    let wall = Instant::now();
    for _ in 0..transfers {
        total.add_assign(&cycle.transfer_profile());
    }
    let wall = wall.elapsed();
    print_report(transfers, wall, &total);
}

fn print_report(transfers: usize, wall: Duration, total: &ResourceTransferProfile) {
    let payload_bytes = transfers as f64 * total.payload_len as f64;
    let wall_goodput = payload_bytes / wall.as_secs_f64();
    let staged_goodput = payload_bytes / total.stage_total().as_secs_f64();
    println!(
        "resource-profile transfers={transfers} payload_len={}",
        total.payload_len
    );
    println!(
        "engine_wall={:.3} ms goodput={:.1} MB/s staged_goodput={:.1} MB/s",
        ms(wall),
        wall_goodput / 1_000_000.0,
        staged_goodput / 1_000_000.0,
    );
    println!(
        "frames per transfer: advertisements={:.1} requests={:.1} parts={:.1} hmu={:.1} proofs={:.1} wire={:.1} KiB",
        total.advertisements as f64 / transfers as f64,
        total.requests as f64 / transfers as f64,
        total.parts as f64 / transfers as f64,
        total.hashmap_updates as f64 / transfers as f64,
        total.proofs as f64 / transfers as f64,
        total.wire_bytes as f64 / transfers as f64 / 1024.0,
    );
    println!();
    println!("stage                         total ms   per transfer µs   share");
    stage(
        "sender build+advertise",
        total.sender_offer,
        total.stage_total(),
        transfers,
    );
    stage(
        "receiver accept+first pull",
        total.receiver_accept,
        total.stage_total(),
        transfers,
    );
    stage(
        "sender serve requests",
        total.sender_serve,
        total.stage_total(),
        transfers,
    );
    stage(
        "receiver parts+assemble",
        total.receiver_receive,
        total.stage_total(),
        transfers,
    );
    stage(
        "initiator verify proof",
        total.initiator_settle,
        total.stage_total(),
        transfers,
    );
}

fn stage(label: &str, duration: Duration, total: Duration, transfers: usize) {
    println!(
        "{label:<30} {:>8.3} {:>17.2} {:>7.1}%",
        ms(duration),
        duration.as_secs_f64() * 1_000_000.0 / transfers as f64,
        duration.as_secs_f64() * 100.0 / total.as_secs_f64(),
    );
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}
