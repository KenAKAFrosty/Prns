use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

use prns_core::interfaces::shared_instance::rns_rpc::{RpcDialect, RpcVerb};

#[derive(Clone, Default)]
pub struct RpcTelemetry {
    inner: Arc<RpcTelemetryInner>,
}

#[derive(Default)]
struct RpcTelemetryInner {
    active_clients: AtomicU64,
    total_connections: AtomicU64,
    request_frames: AtomicU64,
    completed_requests: AtomicU64,
    auth_failures: AtomicU64,
    protocol_failures: AtomicU64,
    read_failures: AtomicU64,
    write_failures: AtomicU64,
    invalid_frames: AtomicU64,
    msgpack_requests: AtomicU64,
    pickle_requests: AtomicU64,
    get_interface_stats: AtomicU64,
    get_path_table: AtomicU64,
    get_rate_table: AtomicU64,
    get_link_count: AtomicU64,
    get_next_hop: AtomicU64,
    get_next_hop_if_name: AtomicU64,
    get_first_hop_timeout: AtomicU64,
    get_bitrate_timing: AtomicU64,
    get_phy_stats: AtomicU64,
    management_reads: AtomicU64,
    management_writes: AtomicU64,
    unknown_requests: AtomicU64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RpcTelemetrySnapshot {
    pub active_clients: u64,
    pub total_connections: u64,
    pub request_frames: u64,
    pub completed_requests: u64,
    pub auth_failures: u64,
    pub protocol_failures: u64,
    pub read_failures: u64,
    pub write_failures: u64,
    pub invalid_frames: u64,
    pub msgpack_requests: u64,
    pub pickle_requests: u64,
    pub get_interface_stats: u64,
    pub get_path_table: u64,
    pub get_rate_table: u64,
    pub get_link_count: u64,
    pub get_next_hop: u64,
    pub get_next_hop_if_name: u64,
    pub get_first_hop_timeout: u64,
    pub get_bitrate_timing: u64,
    pub get_phy_stats: u64,
    pub management_reads: u64,
    pub management_writes: u64,
    pub unknown_requests: u64,
}

impl RpcTelemetry {
    #[must_use]
    pub fn snapshot(&self) -> RpcTelemetrySnapshot {
        let load = |counter: &AtomicU64| counter.load(Ordering::Relaxed);
        RpcTelemetrySnapshot {
            active_clients: load(&self.inner.active_clients),
            total_connections: load(&self.inner.total_connections),
            request_frames: load(&self.inner.request_frames),
            completed_requests: load(&self.inner.completed_requests),
            auth_failures: load(&self.inner.auth_failures),
            protocol_failures: load(&self.inner.protocol_failures),
            read_failures: load(&self.inner.read_failures),
            write_failures: load(&self.inner.write_failures),
            invalid_frames: load(&self.inner.invalid_frames),
            msgpack_requests: load(&self.inner.msgpack_requests),
            pickle_requests: load(&self.inner.pickle_requests),
            get_interface_stats: load(&self.inner.get_interface_stats),
            get_path_table: load(&self.inner.get_path_table),
            get_rate_table: load(&self.inner.get_rate_table),
            get_link_count: load(&self.inner.get_link_count),
            get_next_hop: load(&self.inner.get_next_hop),
            get_next_hop_if_name: load(&self.inner.get_next_hop_if_name),
            get_first_hop_timeout: load(&self.inner.get_first_hop_timeout),
            get_bitrate_timing: load(&self.inner.get_bitrate_timing),
            get_phy_stats: load(&self.inner.get_phy_stats),
            management_reads: load(&self.inner.management_reads),
            management_writes: load(&self.inner.management_writes),
            unknown_requests: load(&self.inner.unknown_requests),
        }
    }

    pub(super) fn connection_opened(&self) -> RpcConnectionGuard {
        self.inner.total_connections.fetch_add(1, Ordering::Relaxed);
        self.inner.active_clients.fetch_add(1, Ordering::Relaxed);
        RpcConnectionGuard {
            telemetry: self.clone(),
        }
    }

    pub(super) fn record_request_frame(&self) {
        self.inner.request_frames.fetch_add(1, Ordering::Relaxed);
    }

    pub(super) fn record_request(&self, dialect: RpcDialect, verb: RpcVerb) {
        match dialect {
            RpcDialect::Pickle => self.inner.pickle_requests.fetch_add(1, Ordering::Relaxed),
            RpcDialect::Msgpack => self.inner.msgpack_requests.fetch_add(1, Ordering::Relaxed),
        };
        match verb {
            RpcVerb::GetInterfaceStats => self
                .inner
                .get_interface_stats
                .fetch_add(1, Ordering::Relaxed),
            RpcVerb::GetPathTable => self.inner.get_path_table.fetch_add(1, Ordering::Relaxed),
            RpcVerb::GetRateTable => self.inner.get_rate_table.fetch_add(1, Ordering::Relaxed),
            RpcVerb::GetLinkCount => self.inner.get_link_count.fetch_add(1, Ordering::Relaxed),
            RpcVerb::GetNextHop => self.inner.get_next_hop.fetch_add(1, Ordering::Relaxed),
            RpcVerb::GetNextHopInterfaceName => self
                .inner
                .get_next_hop_if_name
                .fetch_add(1, Ordering::Relaxed),
            RpcVerb::GetFirstHopTimeout => self
                .inner
                .get_first_hop_timeout
                .fetch_add(1, Ordering::Relaxed),
            RpcVerb::GetLowestInterfaceBitrate | RpcVerb::GetMediumPathTimeout => self
                .inner
                .get_bitrate_timing
                .fetch_add(1, Ordering::Relaxed),
            RpcVerb::GetPacketRssi | RpcVerb::GetPacketSnr | RpcVerb::GetPacketQuality => {
                self.inner.get_phy_stats.fetch_add(1, Ordering::Relaxed)
            }
            RpcVerb::GetBlackholedIdentities | RpcVerb::CheckIdentityBlackholed => {
                self.inner.management_reads.fetch_add(1, Ordering::Relaxed)
            }
            RpcVerb::DropPath
            | RpcVerb::DropAllVia
            | RpcVerb::DropAnnounceQueues
            | RpcVerb::BlackholeIdentity
            | RpcVerb::UnblackholeIdentity
            | RpcVerb::UpdateDestinationData
            | RpcVerb::RetainIdentity => {
                self.inner.management_writes.fetch_add(1, Ordering::Relaxed)
            }
            RpcVerb::Unknown => self.inner.unknown_requests.fetch_add(1, Ordering::Relaxed),
        };
    }

    pub(super) fn record_completed(&self) {
        self.inner
            .completed_requests
            .fetch_add(1, Ordering::Relaxed);
    }

    pub(super) fn record_auth_failure(&self) {
        self.inner.auth_failures.fetch_add(1, Ordering::Relaxed);
    }

    pub(super) fn record_protocol_failure(&self) {
        self.inner.protocol_failures.fetch_add(1, Ordering::Relaxed);
    }

    pub(super) fn record_read_failure(&self, kind: std::io::ErrorKind) {
        self.inner.read_failures.fetch_add(1, Ordering::Relaxed);
        if kind == std::io::ErrorKind::InvalidData {
            self.inner.invalid_frames.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(super) fn record_write_failure(&self) {
        self.inner.write_failures.fetch_add(1, Ordering::Relaxed);
    }
}

pub(super) struct RpcConnectionGuard {
    telemetry: RpcTelemetry,
}

impl Drop for RpcConnectionGuard {
    fn drop(&mut self) {
        self.telemetry
            .inner
            .active_clients
            .fetch_sub(1, Ordering::Relaxed);
    }
}
