//! Print one JSON status snapshot for a Prns-owned local shared instance.
//!
//! The shape mirrors the Android service status bundle where the fields overlap, and extends it with
//! shared-instance RPC telemetry so daemon wrappers and smoke tests can observe local-client usage.
#![allow(clippy::expect_used)]

use std::time::{Duration, Instant};

use personal_rns::interfaces::local::impls::rpc_compat::{RpcTelemetry, SharedInstanceRpcCompat};
use personal_rns::interfaces::local::impls::tokio::LocalServer;
use personal_rns::runtime::{Prns, PrnsRecipe, RuntimeHealth};
use personal_rns::storage::GrowableHeap;
use personal_rns::{interfaces, routes};
use serde_json::json;

fn env_port(name: &str, fallback: u16) -> u16 {
    std::env::var(name)
        .ok()
        .and_then(|raw| raw.parse().ok())
        .unwrap_or(fallback)
}

fn env_millis(name: &str, fallback: u64) -> Duration {
    Duration::from_millis(
        std::env::var(name)
            .ok()
            .and_then(|raw| raw.parse().ok())
            .unwrap_or(fallback),
    )
}

fn decode_key(hex: &str) -> Option<[u8; 32]> {
    let trimmed = hex.trim();
    if trimmed.len() != 64 {
        return None;
    }
    let mut key = [0u8; 32];
    for (i, slot) in key.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&trimmed[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(key)
}

#[tokio::main]
async fn main() {
    let local_port = env_port("PRNS_LOCAL_PORT", 37428);
    let rpc_port = env_port("PRNS_RPC_PORT", local_port + 1);
    let snapshot_delay = env_millis("PRNS_STATUS_DELAY_MS", 50);
    let rpc_key = std::env::var("PRNS_RPC_KEY")
        .ok()
        .and_then(|hex| decode_key(&hex))
        .unwrap_or([0x5a; 32]);

    let node = Prns::new(PrnsRecipe {
        transport: None,
        pre_configured_destinations: [] as [personal_rns::runtime::PreConfiguredDestination; 0],
        app_state: (),
        storage: GrowableHeap,
        routes: routes![],
        interfaces: interfaces![],
        on_event: |_event, _state: &()| {},
    });

    let started = Instant::now();
    let handle = node.handle();
    handle.supervise(LocalServer::with_port(local_port));

    let rpc_telemetry = RpcTelemetry::default();
    let status_telemetry = rpc_telemetry.clone();
    let fleet = handle.clone();
    tokio::spawn(
        SharedInstanceRpcCompat::tcp(rpc_key, rpc_port, handle.clone())
            .with_interfaces(move || fleet.interface_vitals())
            .with_telemetry(rpc_telemetry)
            .run(),
    );
    tokio::select! {
        () = node.run() => {}
        () = tokio::time::sleep(snapshot_delay) => {
            let health = RuntimeHealth::from_snapshots(started.elapsed(), &handle.interfaces());
            let rpc = status_telemetry.snapshot();
            let rpc_json = json!({
                "active_clients": rpc.active_clients,
                "total_connections": rpc.total_connections,
                "request_frames": rpc.request_frames,
                "completed_requests": rpc.completed_requests,
                "auth_failures": rpc.auth_failures,
                "protocol_failures": rpc.protocol_failures,
                "read_failures": rpc.read_failures,
                "write_failures": rpc.write_failures,
                "invalid_frames": rpc.invalid_frames,
                "msgpack_requests": rpc.msgpack_requests,
                "pickle_requests": rpc.pickle_requests,
                "get_interface_stats": rpc.get_interface_stats,
                "get_path_table": rpc.get_path_table,
                "get_rate_table": rpc.get_rate_table,
                "get_link_count": rpc.get_link_count,
                "get_next_hop": rpc.get_next_hop,
                "get_next_hop_if_name": rpc.get_next_hop_if_name,
                "get_first_hop_timeout": rpc.get_first_hop_timeout,
                "get_phy_stats": rpc.get_phy_stats,
                "management_reads": rpc.management_reads,
                "management_writes": rpc.management_writes,
                "unknown_requests": rpc.unknown_requests,
            });
            println!(
                "{}",
                json!({
                    "schema": "personal-rns.runtime-health.v1",
                    "state": "running",
                    "running": true,
                    "instance_role": "server",
                    "local_port": local_port,
                    "rpc_port": rpc_port,
                    "runtime_uptime_ms": health.uptime_millis,
                    "interface_count": health.interface_count,
                    "online_interface_count": health.online_interface_count,
                    "local_client_count": health.local_client_count,
                    "route_count": health.route_count,
                    "link_count": health.link_count,
                    "transported_link_count": health.transported_link_count,
                    "rx_bytes": health.rx_bytes,
                    "tx_bytes": health.tx_bytes,
                    "rx_bps": health.rx_bps,
                    "tx_bps": health.tx_bps,
                    "rpc": rpc_json
                })
            );
        }
    }
}
