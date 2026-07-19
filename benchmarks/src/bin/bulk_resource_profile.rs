use std::time::{Duration, Instant};

use benchmarks::microscope::{ResourceCycle, ResourceTransferProfile};
use personal_rns::routing::links::resources::MAX_EFFICIENT_SIZE;

fn main() {
    let mut args = std::env::args().skip(1);
    let transfers = args
        .next()
        .and_then(|arg| arg.parse::<usize>().ok())
        .unwrap_or(8);
    let total_len = args
        .next()
        .and_then(|arg| arg.parse::<usize>().ok())
        .unwrap_or(64 * 1024 * 1024);
    let warmup = args
        .next()
        .and_then(|arg| arg.parse::<usize>().ok())
        .unwrap_or(1);

    let mut cycle = ResourceCycle::new(MAX_EFFICIENT_SIZE);
    for _ in 0..warmup {
        let _ = cycle.transfer_profile_multi(total_len);
    }

    let mut total = ResourceTransferProfile::new(total_len);
    let wall = Instant::now();
    for _ in 0..transfers {
        total.add_assign(&cycle.transfer_profile_multi(total_len));
    }
    let wall = wall.elapsed();
    print_report(transfers, total_len, wall, &total);
}

fn print_report(
    transfers: usize,
    total_len: usize,
    wall: Duration,
    total: &ResourceTransferProfile,
) {
    let payload_bytes = transfers as f64 * total_len as f64;
    let wall_goodput = payload_bytes / wall.as_secs_f64();
    let staged_goodput = payload_bytes / total.stage_total().as_secs_f64();
    let segments = total.advertisements as f64;
    let segments_per_transfer = segments / transfers as f64;
    println!(
        "bulk-resource-profile transfers={transfers} total_len={total_len} ({:.1} MiB) segments/transfer={segments_per_transfer:.0}",
        total_len as f64 / (1024.0 * 1024.0),
    );
    println!(
        "engine_wall={:.3} ms goodput={:.1} MB/s staged_goodput={:.1} MB/s per_segment_goodput={:.1} MB/s",
        ms(wall),
        wall_goodput / 1_000_000.0,
        staged_goodput / 1_000_000.0,
        payload_bytes / segments / (total.stage_total().as_secs_f64() / segments) / 1_000_000.0,
    );
    println!(
        "per segment: advertise={:.1} us serve={:.1} us receive+assemble={:.1} us proof={:.1} us",
        per_segment_us(total.sender_offer, segments),
        per_segment_us(total.sender_serve, segments),
        per_segment_us(total.receiver_receive, segments),
        per_segment_us(total.initiator_settle, segments),
    );

    stage(
        "sender build+advertise",
        total.sender_offer,
        total.stage_total(),
    );
    stage(
        "receiver accept+first pull",
        total.receiver_accept,
        total.stage_total(),
    );
    stage(
        "sender serve requests",
        total.sender_serve,
        total.stage_total(),
    );
    stage(
        "receiver parts+assemble",
        total.receiver_receive,
        total.stage_total(),
    );
    stage(
        "initiator verify proof",
        total.initiator_settle,
        total.stage_total(),
    );
}

fn per_segment_us(duration: Duration, segments: f64) -> f64 {
    duration.as_secs_f64() * 1_000_000.0 / segments
}

fn stage(label: &str, duration: Duration, total: Duration) {
    let share = duration.as_secs_f64() / total.as_secs_f64() * 100.0;
    println!("  {label:<28} {:>10.3} ms   {share:>5.1}%", ms(duration));
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}
