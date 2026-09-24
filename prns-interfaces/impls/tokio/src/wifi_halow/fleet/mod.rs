use std::{collections::HashMap, num::NonZeroU8, sync::Arc, time::Duration};

use prns_core::interfaces::wifi_halow::{self as contract, InstanceTag, PeerMac};
use prns_core::interfaces::{
    ConnectionState, ConnectionView, EffectiveInterfacePolicy, IngressCapability,
    InterfaceDescriptor, InterfaceId, InterfaceKind, InterfaceVitals, MtuPolicy, ReportsStatus,
    StatusView,
};
use prns_runtime::manifold::driver::TokioInterfaceStatus;
use prns_runtime::manifold::interface_seam::{
    Interface, InterfaceSeam, OutboundDisposition, OutboundDropReason,
};
use prns_runtime::runtime::{AttachedInterface, Fleet, InterfaceSupervisor};
use tokio::sync::{mpsc, watch};
use tokio::time::{Instant, MissedTickBehavior};

use super::{Destination, HaLowDatagrams};

const PEER_QUEUE_DEPTH: usize = 16;
const RECEIVE_BURST_LIMIT: usize = 32;
const SEND_TIMEOUT: Duration = Duration::from_secs(2);
const EXPIRY_INTERVAL: Duration = Duration::from_secs(5);
const HW_MTU: usize = contract::HARDWARE_MTU;

/// Resource limits apply before Reticulum authentication. A MAC is an address,
/// not a trusted identity; full tables reject new sources without evicting live peers.
pub struct HaLowLimits {
    pub peers: NonZeroU8,
    pub idle_seconds: std::num::NonZeroU32,
}

/// One already configured radio. The caller owns radio configuration and chooses
/// distinct broadcast/unicast pacing policies from the measured radio profile.
pub struct HaLow<D: HaLowDatagrams> {
    socket: Arc<D>,
    instance: InstanceTag,
    tag: Vec<u8>,
    peer_policy: EffectiveInterfacePolicy,
    broadcast_policy: EffectiveInterfacePolicy,
    limits: HaLowLimits,
    status: TokioInterfaceStatus,
}

impl<D: HaLowDatagrams> HaLow<D> {
    pub fn new(
        socket: D,
        instance: InstanceTag,
        peer_policy: EffectiveInterfacePolicy,
        broadcast_policy: EffectiveInterfacePolicy,
        limits: HaLowLimits,
    ) -> Self {
        let tag = instance.channel_tag().to_vec();
        let id = InterfaceId::from_channel_tag(InterfaceKind::WifiHaLow, &tag);
        Self {
            socket: Arc::new(socket),
            instance,
            tag,
            limits,
            peer_policy: bounded_policy(peer_policy),
            broadcast_policy: bounded_policy(broadcast_policy),
            status: TokioInterfaceStatus::new_unaccounted(id, ConnectionState::Connected),
        }
    }
}

fn bounded_policy(mut policy: EffectiveInterfacePolicy) -> EffectiveInterfacePolicy {
    // Reserve IFAC headroom even for an open fleet, so enabling access control
    // cannot silently exceed the Ethernet datagram cap.
    let mtu = policy
        .mtu
        .resolve(policy.bitrate)
        .unwrap_or(HW_MTU)
        .min(HW_MTU);
    policy.mtu = MtuPolicy::fixed(mtu);
    policy
}

struct Member {
    attached: AttachedInterface,
    inbound: mpsc::Sender<Vec<u8>>,
    activity: watch::Sender<Instant>,
    status: TokioInterfaceStatus,
}

impl<D: HaLowDatagrams> InterfaceSupervisor for HaLow<D> {
    const KIND: InterfaceKind = InterfaceKind::WifiHaLow;
    fn channel_tag(&self) -> &[u8] {
        &self.tag
    }
    fn policy(&self) -> EffectiveInterfacePolicy {
        self.peer_policy
    }

    async fn run(self, fleet: Fleet) {
        self.run_on(&fleet).await;
    }
}

impl<D: HaLowDatagrams> HaLow<D> {
    pub(super) async fn run_on(self, fleet: &Fleet) {
        let broadcast = Channel::<D, Broadcast>::new(
            self.socket.clone(),
            self.tag.clone(),
            self.broadcast_policy,
            Destination::Broadcast,
        );
        let broadcast = fleet.add(BroadcastChannel(broadcast));
        let mut peers = HashMap::<PeerMac, Member>::new();
        let mut buffer = [0; contract::DATAGRAM_MTU];
        let mut expiry = tokio::time::interval(EXPIRY_INTERVAL);
        expiry.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let idle = Duration::from_secs(u64::from(self.limits.idle_seconds.get()));
        let mut received_since_yield = 0;
        loop {
            if received_since_yield == RECEIVE_BURST_LIMIT {
                received_since_yield = 0;
                tokio::task::yield_now().await;
            }
            tokio::select! {
                received = self.socket.receive(&mut buffer) => {
                    received_since_yield += 1;
                    let received = match received {
                        Ok(received) => received,
                        Err(_) => break,
                    };
                    let Some(datagram) = buffer.get(..received.length) else { continue; };
                    let Ok(frame) = contract::decode(datagram) else { continue; };
                    if !peers.contains_key(&received.source) {
                        if peers.len() >= usize::from(self.limits.peers.get()) { continue; }
                        let tag = self.instance.peer_channel_tag(received.source).to_vec();
                        let channel = Channel::<D, Peer>::new(
                            self.socket.clone(), tag, self.peer_policy,
                            Destination::Peer(received.source),
                        );
                        let (inbound, receiver) = mpsc::channel(PEER_QUEUE_DEPTH);
                        let activity = channel.activity.clone();
                        let status = channel.status.clone();
                        let attached = fleet.add(PeerChannel { channel, inbound: receiver });
                        peers.insert(received.source, Member { attached, inbound, activity, status });
                    }
                    if let Some(member) = peers.get(&received.source) {
                        member.activity.send_replace(Instant::now());
                        // A full peer lane drops only this datagram; it never stalls
                        // reception from other neighbors on the shared socket.
                        if let Ok(slot) = member.inbound.try_reserve() {
                            slot.send(frame.to_vec());
                        }
                    }
                }
                _ = expiry.tick() => {
                    let expired: Vec<_> = peers.iter().filter_map(|(mac, member)| {
                        (member.activity.borrow().elapsed() >= idle || member.inbound.is_closed())
                            .then_some(*mac)
                    }).collect();
                    for mac in expired {
                        if let Some(member) = peers.remove(&mac) {
                            member.status.set_connection(ConnectionState::Disconnected);
                            member.attached.teardown();
                        }
                    }
                }
            }
        }
        self.status.set_connection(ConnectionState::Disconnected);
        for (_, member) in peers {
            member.attached.teardown();
        }
        broadcast.teardown();
    }
}

trait ChannelKind {
    const KIND: InterfaceKind;
}
struct Peer;
impl ChannelKind for Peer {
    const KIND: InterfaceKind = InterfaceKind::WifiHaLowPeer;
}
struct Broadcast;
impl ChannelKind for Broadcast {
    const KIND: InterfaceKind = InterfaceKind::WifiHaLowBroadcast;
}

struct Channel<D: HaLowDatagrams, K: ChannelKind> {
    socket: Arc<D>,
    tag: Vec<u8>,
    policy: EffectiveInterfacePolicy,
    destination: Destination,
    status: TokioInterfaceStatus,
    activity: watch::Sender<Instant>,
    kind: std::marker::PhantomData<K>,
}

impl<D: HaLowDatagrams, K: ChannelKind> Channel<D, K> {
    fn new(
        socket: Arc<D>,
        tag: Vec<u8>,
        policy: EffectiveInterfacePolicy,
        destination: Destination,
    ) -> Self {
        let id = InterfaceId::from_channel_tag(K::KIND, &tag);
        let (activity, _) = watch::channel(Instant::now());
        Self {
            socket,
            tag,
            policy,
            destination,
            activity,
            status: TokioInterfaceStatus::new_unaccounted(id, ConnectionState::Connected),
            kind: std::marker::PhantomData,
        }
    }

    fn descriptor(&self) -> InterfaceDescriptor {
        self.policy
            .descriptor(InterfaceId::from_channel_tag(K::KIND, &self.tag))
    }

    async fn send(&self, frame: &[u8]) -> OutboundDisposition {
        let mut buffer = [0; contract::DATAGRAM_MTU];
        let Ok(datagram) = contract::encode(frame, &mut buffer) else {
            return OutboundDisposition::Dropped(OutboundDropReason::Rejected);
        };
        match tokio::time::timeout(SEND_TIMEOUT, self.socket.send(self.destination, datagram)).await
        {
            Ok(Ok(())) => {
                self.activity.send_replace(Instant::now());
                self.status.add_tx(frame.len() as u64);
                OutboundDisposition::Sent
            }
            Ok(Err(_)) => OutboundDisposition::Dropped(OutboundDropReason::TransportFailure),
            Err(_) => OutboundDisposition::Dropped(OutboundDropReason::TimedOut),
        }
    }
}

struct PeerChannel<D: HaLowDatagrams> {
    channel: Channel<D, Peer>,
    inbound: mpsc::Receiver<Vec<u8>>,
}
struct BroadcastChannel<D: HaLowDatagrams>(Channel<D, Broadcast>);

impl<D: HaLowDatagrams> Interface for PeerChannel<D> {
    const KIND: InterfaceKind = InterfaceKind::WifiHaLowPeer;
    const HW_MTU: usize = HW_MTU;
    fn channel_tag(&self) -> &[u8] {
        &self.channel.tag
    }
    fn descriptor(&self) -> InterfaceDescriptor {
        self.channel.descriptor()
    }
    async fn run<S: InterfaceSeam>(mut self, mut seam: S) {
        loop {
            tokio::select! {
                inbound = self.inbound.recv() => {
                    let Some(frame) = inbound else { break; };
                    self.channel.status.add_rx(frame.len() as u64);
                    seam.next_inbound(&frame).await;
                }
                outbound = seam.next_outbound() => {
                    let disposition = self.channel.send(outbound).await;
                    seam.complete_outbound(disposition);
                }
            }
        }
    }
}

impl<D: HaLowDatagrams> Interface for BroadcastChannel<D> {
    const KIND: InterfaceKind = InterfaceKind::WifiHaLowBroadcast;
    const HW_MTU: usize = HW_MTU;
    fn channel_tag(&self) -> &[u8] {
        &self.0.tag
    }
    fn descriptor(&self) -> InterfaceDescriptor {
        let mut descriptor = self.0.descriptor();
        descriptor.capabilities.ingress = IngressCapability::Disabled;
        descriptor
    }
    async fn run<S: InterfaceSeam>(self, mut seam: S) {
        loop {
            let disposition = self.0.send(seam.next_outbound().await).await;
            seam.complete_outbound(disposition);
        }
    }
}

fn status_view(status: TokioInterfaceStatus) -> Option<StatusView> {
    Some(Arc::new(move || vec![InterfaceVitals::of(&status)]))
}
impl<D: HaLowDatagrams> ReportsStatus for HaLow<D> {
    fn status_view(&self) -> Option<StatusView> {
        status_view(self.status.clone())
    }
}
impl<D: HaLowDatagrams> ReportsStatus for PeerChannel<D> {
    fn status_view(&self) -> Option<StatusView> {
        status_view(self.channel.status.clone())
    }
    fn connection_view(&self) -> Option<ConnectionView> {
        Some(ConnectionView::of(self.channel.status.clone()))
    }
}
impl<D: HaLowDatagrams> ReportsStatus for BroadcastChannel<D> {
    fn status_view(&self) -> Option<StatusView> {
        status_view(self.0.status.clone())
    }
    fn connection_view(&self) -> Option<ConnectionView> {
        Some(ConnectionView::of(self.0.status.clone()))
    }
}

#[cfg(test)]
mod tests;
