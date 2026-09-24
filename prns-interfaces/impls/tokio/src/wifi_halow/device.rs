use super::{HaLow, HaLowDatagrams, HaLowLimits};
use crate::reconnect::ReconnectPolicy;
use prns_core::interfaces::{
    wifi_halow::InstanceTag, ConnectionState, EffectiveInterfacePolicy, InterfaceId, InterfaceKind,
    InterfaceVitals, ReportsStatus, StatusView,
};
use prns_runtime::{
    manifold::driver::TokioInterfaceStatus,
    runtime::{Fleet, InterfaceSupervisor},
};
use std::{io, sync::Arc};

#[allow(async_fn_in_trait)]
/// Opens an owned, cancel-safe binding. Its receive operation must terminate when
/// that binding retires, even if the device has already returned with the same address.
pub trait HaLowRadioSource: Send + 'static {
    type Datagrams: HaLowDatagrams;
    async fn open(&mut self) -> io::Result<Self::Datagrams>;
}

/// Keeps one configured device attached across packet-socket generations.
/// Device-specific invalidation belongs to the source's datagram implementation.
pub struct HaLowDevice<S> {
    source: S,
    instance: InstanceTag,
    tag: Vec<u8>,
    peer_policy: EffectiveInterfacePolicy,
    broadcast_policy: EffectiveInterfacePolicy,
    limits: HaLowLimits,
    reconnect: ReconnectPolicy,
    status: TokioInterfaceStatus,
}
impl<S: HaLowRadioSource> HaLowDevice<S> {
    pub fn new(
        source: S,
        instance: InstanceTag,
        peer_policy: EffectiveInterfacePolicy,
        broadcast_policy: EffectiveInterfacePolicy,
        limits: HaLowLimits,
        reconnect: ReconnectPolicy,
    ) -> Self {
        let tag = instance.channel_tag().to_vec();
        let id = InterfaceId::from_channel_tag(InterfaceKind::WifiHaLow, &tag);
        Self {
            source,
            instance,
            tag,
            peer_policy,
            broadcast_policy,
            limits,
            reconnect,
            status: TokioInterfaceStatus::new_unaccounted(id, ConnectionState::Disconnected),
        }
    }
}
impl<S: HaLowRadioSource> InterfaceSupervisor for HaLowDevice<S> {
    const KIND: InterfaceKind = InterfaceKind::WifiHaLow;
    fn channel_tag(&self) -> &[u8] {
        &self.tag
    }
    fn policy(&self) -> EffectiveInterfacePolicy {
        self.peer_policy
    }
    async fn run(mut self, fleet: Fleet) {
        let mut retry = self.reconnect.schedule();
        loop {
            self.status.set_connection(ConnectionState::Reconnecting);
            match self.source.open().await {
                Ok(socket) => {
                    self.status.set_connection(ConnectionState::Connected);
                    let started = tokio::time::Instant::now();
                    HaLow::new(
                        socket,
                        self.instance.clone(),
                        self.peer_policy,
                        self.broadcast_policy,
                        HaLowLimits {
                            peers: self.limits.peers,
                            idle_seconds: self.limits.idle_seconds,
                        },
                    )
                    .run_on(&fleet)
                    .await;
                    retry.record_connection_lifetime(started.elapsed());
                }
                Err(error) => crate::diagnostic_log::warn!("HaLoW device unavailable: {error}"),
            }
            self.status.set_connection(ConnectionState::Disconnected);
            tokio::time::sleep(retry.next_delay(|bytes| fleet.fill_random(bytes))).await;
        }
    }
}
impl<S: HaLowRadioSource> ReportsStatus for HaLowDevice<S> {
    fn status_view(&self) -> Option<StatusView> {
        let status = self.status.clone();
        Some(Arc::new(move || vec![InterfaceVitals::of(&status)]))
    }
}
