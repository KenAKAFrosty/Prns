mod catalog;
mod codec;

use ::core::net::Ipv6Addr;

use super::AutoWifiStatus;
use catalog::{CatalogUpdate, ResolutionQuery, ServiceCatalog, ServiceResolution};
use codec::{
    build_publication_packet, build_query_packet, encoded_name, query_relevance, DiscoveryInstance,
    QueryRelevance, DNS_TYPE_PTR, MDNS_HOP_LIMIT, MDNS_IPV6_GROUP, MDNS_PORT, SERVICE_LABELS,
};
use embassy_futures::select::{select, select5, Either, Either5};
use embassy_net::udp::UdpSocket;
use embassy_net::{IpAddress, Stack};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::watch::Receiver;
use embassy_time::{with_timeout, Duration, Instant, Ticker, Timer};
use prns_core::entropy::EntropySource;
use prns_core::interfaces::wifi_auto as contract;
use prns_runtime::runtime::EntropyHandle;

pub const EMBEDDED_SERVICE_DISCOVERY_CAPACITY: u8 = 1;
pub const UDP_SERVICE_DISCOVERY_SOCKET_COUNT: u8 = 1;
pub const UDP_SERVICE_DISCOVERY_PACKET_BYTES: usize = 384;
pub const UDP_SERVICE_DISCOVERY_RECEIVE_PACKET_BYTES: usize = 1_536;
pub const UDP_SERVICE_DISCOVERY_RX_QUEUED_PACKETS: usize = 3;
pub const UDP_SERVICE_DISCOVERY_RX_SOCKET_BYTES: usize =
    UDP_SERVICE_DISCOVERY_RECEIVE_PACKET_BYTES * UDP_SERVICE_DISCOVERY_RX_QUEUED_PACKETS;
pub const UDP_SERVICE_DISCOVERY_RX_SOCKET_METADATA: usize =
    UDP_SERVICE_DISCOVERY_RX_QUEUED_PACKETS + 1;
pub const UDP_SERVICE_DISCOVERY_TX_QUEUED_PACKETS: usize = 2;
pub const UDP_SERVICE_DISCOVERY_TX_SOCKET_BYTES: usize =
    UDP_SERVICE_DISCOVERY_PACKET_BYTES * UDP_SERVICE_DISCOVERY_TX_QUEUED_PACKETS;
pub const UDP_SERVICE_DISCOVERY_TX_SOCKET_METADATA: usize =
    UDP_SERVICE_DISCOVERY_TX_QUEUED_PACKETS + 1;

const DISCOVERY_WATCHERS: usize = EMBEDDED_SERVICE_DISCOVERY_CAPACITY as usize;
const PUBLICATION_TTL_SECONDS: u32 = 120;
const ANNOUNCEMENT_INTERVAL: Duration = Duration::from_secs(60);
const BROWSE_INTERVAL: Duration = Duration::from_secs(30);
const FAILURE_RETRY_INTERVAL: Duration = Duration::from_secs(5);
const SEND_TIMEOUT: Duration = Duration::from_millis(300);
const _: () =
    assert!(UDP_SERVICE_DISCOVERY_RX_SOCKET_METADATA > UDP_SERVICE_DISCOVERY_RX_QUEUED_PACKETS);
const _: () = assert!(
    UDP_SERVICE_DISCOVERY_RX_SOCKET_BYTES
        == UDP_SERVICE_DISCOVERY_RECEIVE_PACKET_BYTES * UDP_SERVICE_DISCOVERY_RX_QUEUED_PACKETS
);
const _: () =
    assert!(UDP_SERVICE_DISCOVERY_TX_SOCKET_METADATA > UDP_SERVICE_DISCOVERY_TX_QUEUED_PACKETS);
const _: () = assert!(
    UDP_SERVICE_DISCOVERY_TX_SOCKET_BYTES
        == UDP_SERVICE_DISCOVERY_PACKET_BYTES * UDP_SERVICE_DISCOVERY_TX_QUEUED_PACKETS
);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EmbeddedDiscoveryParticipation {
    Inactive,
    Central,
}

pub(crate) type DiscoveryParticipationReceiver =
    Receiver<'static, CriticalSectionRawMutex, EmbeddedDiscoveryParticipation, DISCOVERY_WATCHERS>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UdpServiceDiscoveryConstructionError {
    DiscoveryCapacityExhausted,
    AddressNotLinkLocal,
}

pub struct UdpServiceDiscoveryStorage<const TARGETS: usize> {
    receive_packet: [u8; UDP_SERVICE_DISCOVERY_RECEIVE_PACKET_BYTES],
    catalog: ServiceCatalog<TARGETS>,
}

impl<const TARGETS: usize> UdpServiceDiscoveryStorage<TARGETS> {
    pub const fn new() -> Self {
        Self {
            receive_packet: [0; UDP_SERVICE_DISCOVERY_RECEIVE_PACKET_BYTES],
            catalog: ServiceCatalog::new(),
        }
    }
}

impl<const TARGETS: usize> Default for UdpServiceDiscoveryStorage<TARGETS> {
    fn default() -> Self {
        Self::new()
    }
}

pub struct UdpServiceDiscovery<'a, S: EntropySource + 'static, const TARGETS: usize> {
    socket: UdpSocket<'a>,
    stack: Stack<'a>,
    address: Ipv6Addr,
    participation: DiscoveryParticipationReceiver,
    status: AutoWifiStatus<TARGETS>,
    storage: &'a mut UdpServiceDiscoveryStorage<TARGETS>,
    entropy: EntropyHandle<CriticalSectionRawMutex, S>,
    query_cursor: super::RoundRobinCursor,
}

impl<'a, S: EntropySource + Send + 'static, const TARGETS: usize>
    UdpServiceDiscovery<'a, S, TARGETS>
{
    pub fn new(
        socket: UdpSocket<'a>,
        stack: Stack<'a>,
        address: Ipv6Addr,
        status: AutoWifiStatus<TARGETS>,
        storage: &'a mut UdpServiceDiscoveryStorage<TARGETS>,
        entropy: EntropyHandle<CriticalSectionRawMutex, S>,
    ) -> Result<Self, UdpServiceDiscoveryConstructionError> {
        validate_publication_address(address)?;
        let participation = status.discovery_participation_receiver()?;
        Ok(Self {
            socket,
            stack,
            address,
            participation,
            status,
            storage,
            entropy,
            query_cursor: super::RoundRobinCursor::new(),
        })
    }

    pub async fn run(mut self) -> ! {
        loop {
            self.participation
                .get_and(|participation| *participation == EmbeddedDiscoveryParticipation::Central)
                .await;
            match select(self.stack.wait_config_up(), self.participation.changed()).await {
                Either::First(()) => {}
                Either::Second(_) => continue,
            }
            match select(self.stack.wait_link_up(), self.participation.changed()).await {
                Either::First(()) => {}
                Either::Second(_) => continue,
            }

            let mut random = [0; contract::EPHEMERAL_DISCOVERY_INSTANCE_RANDOM_BYTES];
            self.entropy.fill_random(&mut random);
            let instance = DiscoveryInstance::fresh(random);
            match self.activate().await {
                PublicationActivation::Active => {
                    self.serve(&instance).await;
                    self.deactivate(&instance).await;
                }
                PublicationActivation::Retry => {
                    self.clear_targets();
                    self.socket.close();
                    self.leave_multicast_group();
                    let retry = Timer::after(FAILURE_RETRY_INTERVAL);
                    let participation_changed = self.participation.changed();
                    select(retry, participation_changed).await;
                }
            }
        }
    }

    async fn activate(&mut self) -> PublicationActivation {
        self.socket.set_hop_limit(Some(MDNS_HOP_LIMIT));
        if let Err(error) = self.socket.bind(MDNS_PORT) {
            crate::diagnostic_log::warn!("wifi-auto: embedded UDP DNS-SD bind failed: {error:?}");
            return PublicationActivation::Retry;
        }
        if let Err(error) = self
            .stack
            .join_multicast_group(IpAddress::Ipv6(MDNS_IPV6_GROUP))
        {
            crate::diagnostic_log::warn!(
                "wifi-auto: embedded UDP DNS-SD multicast join failed: {error:?}"
            );
            return PublicationActivation::Retry;
        }
        crate::diagnostic_log::debug!("wifi-auto: embedded UDP DNS-SD active");
        PublicationActivation::Active
    }

    async fn serve(&mut self, instance: &DiscoveryInstance) {
        let mut packet = [0u8; UDP_SERVICE_DISCOVERY_PACKET_BYTES];
        let Ok(packet_len) =
            build_publication_packet(&mut packet, instance, self.address, PUBLICATION_TTL_SECONDS)
        else {
            crate::diagnostic_log::error!("wifi-auto: embedded UDP DNS-SD packet does not fit");
            return;
        };
        self.publish(
            &packet[..packet_len],
            PublicationPurpose::InitialAnnouncement,
        )
        .await;
        self.send_browse_queries().await;

        let mut announcement = Ticker::every(ANNOUNCEMENT_INTERVAL);
        let mut browse = Ticker::every(BROWSE_INTERVAL);
        loop {
            match select5(
                self.socket.recv_from(&mut self.storage.receive_packet),
                announcement.next(),
                browse.next(),
                self.participation.changed(),
                self.stack.wait_link_down(),
            )
            .await
            {
                Either5::First(Ok((length, _))) => {
                    match query_relevance(&self.storage.receive_packet[..length], instance) {
                        QueryRelevance::Relevant => {
                            self.publish(&packet[..packet_len], PublicationPurpose::QueryResponse)
                                .await;
                        }
                        QueryRelevance::Response => {
                            self.apply_response(length, instance);
                        }
                        QueryRelevance::Unrelated | QueryRelevance::Malformed => {}
                    }
                }
                Either5::First(Err(error)) => {
                    crate::diagnostic_log::debug!(
                        "wifi-auto: embedded UDP DNS-SD packet dropped: {error:?}"
                    );
                }
                Either5::Second(()) => {
                    self.publish(&packet[..packet_len], PublicationPurpose::Refresh)
                        .await;
                }
                Either5::Third(()) => {
                    self.prune_targets();
                    self.send_browse_queries().await;
                }
                Either5::Fourth(_) | Either5::Fifth(()) => return,
            }
        }
    }

    async fn deactivate(&mut self, instance: &DiscoveryInstance) {
        let mut goodbye = [0u8; UDP_SERVICE_DISCOVERY_PACKET_BYTES];
        match build_publication_packet(&mut goodbye, instance, self.address, 0) {
            Ok(goodbye_len) => {
                self.publish(&goodbye[..goodbye_len], PublicationPurpose::Withdrawal)
                    .await;
            }
            Err(error) => {
                crate::diagnostic_log::error!(
                    "wifi-auto: embedded UDP DNS-SD withdrawal does not fit: {error:?}"
                );
            }
        }
        self.clear_targets();
        self.socket.close();
        self.leave_multicast_group();
    }

    fn apply_response(&mut self, packet_length: usize, instance: &DiscoveryInstance) {
        let now_ms = Instant::now().as_millis();
        let packet = &self.storage.receive_packet[..packet_length];
        let previous_targets = self.storage.catalog.targets(now_ms, self.address);
        match self
            .storage
            .catalog
            .apply_response(packet, instance, now_ms)
        {
            CatalogUpdate::Applied => {}
            CatalogUpdate::Malformed => {
                crate::diagnostic_log::debug!(
                    "wifi-auto: embedded UDP DNS-SD response was malformed"
                );
                return;
            }
        }
        let current_targets = self.storage.catalog.targets(now_ms, self.address);
        if current_targets != previous_targets {
            crate::diagnostic_log::debug!(
                "wifi-auto: embedded UDP DNS-SD targets={}",
                current_targets.len()
            );
            self.status.publish_discovery_targets(current_targets);
        }
    }

    fn prune_targets(&mut self) {
        let now_ms = Instant::now().as_millis();
        let previous_targets = self.storage.catalog.targets(now_ms, self.address);
        self.storage.catalog.prune(now_ms);
        let current_targets = self.storage.catalog.targets(now_ms, self.address);
        if current_targets != previous_targets {
            self.status.publish_discovery_targets(current_targets);
        }
    }

    fn clear_targets(&mut self) {
        self.storage.catalog.clear();
        self.query_cursor.reset();
        self.status
            .publish_discovery_targets(super::EmbeddedDiscoveryTargets::new());
    }

    async fn send_browse_queries(&mut self) {
        let Ok(service_name) = encoded_name(&SERVICE_LABELS) else {
            return;
        };
        let query_count = self.storage.catalog.len().saturating_add(1);
        let now_ms = Instant::now().as_millis();
        let sends = with_timeout(SEND_TIMEOUT, async {
            let mut query_packet = [0u8; UDP_SERVICE_DISCOVERY_PACKET_BYTES];
            for _ in 0..query_count {
                let super::RoundRobinPosition::Item(index) = self.query_cursor.advance(query_count)
                else {
                    return;
                };
                let query = if index == 0 {
                    ResolutionQuery {
                        name: service_name.clone(),
                        record_type: DNS_TYPE_PTR,
                    }
                } else {
                    match self.storage.catalog.resolution_at(index - 1, now_ms) {
                        ServiceResolution::Query(query) => query,
                        ServiceResolution::Resolved
                        | ServiceResolution::Expired
                        | ServiceResolution::Incompatible
                        | ServiceResolution::Missing => continue,
                    }
                };
                let Ok(query_length) =
                    build_query_packet(&mut query_packet, &query.name, query.record_type)
                else {
                    continue;
                };
                let _send_result = self
                    .socket
                    .send_to(
                        &query_packet[..query_length],
                        (IpAddress::Ipv6(MDNS_IPV6_GROUP), MDNS_PORT),
                    )
                    .await;
            }
        });
        match sends.await {
            Ok(()) => {}
            Err(_timeout) => {
                crate::diagnostic_log::debug!(
                    "wifi-auto: embedded UDP DNS-SD query budget exhausted"
                );
            }
        }
    }

    async fn publish(&self, packet: &[u8], purpose: PublicationPurpose) {
        if self.send(packet).await == PublicationSend::Failed {
            crate::diagnostic_log::debug!("wifi-auto: embedded UDP DNS-SD {purpose:?} failed");
        }
    }

    async fn send(&self, packet: &[u8]) -> PublicationSend {
        match with_timeout(
            SEND_TIMEOUT,
            self.socket
                .send_to(packet, (IpAddress::Ipv6(MDNS_IPV6_GROUP), MDNS_PORT)),
        )
        .await
        {
            Ok(Ok(())) => PublicationSend::Sent,
            Ok(Err(_)) | Err(_) => PublicationSend::Failed,
        }
    }

    fn leave_multicast_group(&self) {
        if let Err(error) = self.stack.leave_multicast_group(MDNS_IPV6_GROUP) {
            crate::diagnostic_log::debug!(
                "wifi-auto: embedded UDP DNS-SD multicast leave failed: {error:?}"
            );
        }
    }
}

fn validate_publication_address(
    address: Ipv6Addr,
) -> Result<(), UdpServiceDiscoveryConstructionError> {
    if address.is_unicast_link_local() {
        Ok(())
    } else {
        Err(UdpServiceDiscoveryConstructionError::AddressNotLinkLocal)
    }
}

#[derive(Debug, PartialEq, Eq)]
enum PublicationActivation {
    Active,
    Retry,
}

#[derive(Debug, PartialEq, Eq)]
enum PublicationSend {
    Sent,
    Failed,
}

#[derive(Debug)]
enum PublicationPurpose {
    InitialAnnouncement,
    QueryResponse,
    Refresh,
    Withdrawal,
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINK_LOCAL: Ipv6Addr = Ipv6Addr::new(0xfe80, 0, 0, 0, 0x0212, 0x34ff, 0xfe56, 0x789a);

    #[test]
    fn publication_address_must_be_ipv6_link_local() {
        assert_eq!(validate_publication_address(LINK_LOCAL), Ok(()));
        assert_eq!(
            validate_publication_address(Ipv6Addr::LOCALHOST),
            Err(UdpServiceDiscoveryConstructionError::AddressNotLinkLocal)
        );
        assert_eq!(
            validate_publication_address(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1)),
            Err(UdpServiceDiscoveryConstructionError::AddressNotLinkLocal)
        );
    }

    #[test]
    fn embedded_discovery_memory_is_explicitly_bounded() {
        assert_eq!(
            UDP_SERVICE_DISCOVERY_RX_SOCKET_BYTES,
            UDP_SERVICE_DISCOVERY_RECEIVE_PACKET_BYTES * 3
        );
        assert_eq!(UDP_SERVICE_DISCOVERY_RX_SOCKET_METADATA, 4);
        assert_eq!(
            UDP_SERVICE_DISCOVERY_TX_SOCKET_BYTES,
            UDP_SERVICE_DISCOVERY_PACKET_BYTES * 2
        );
        assert_eq!(UDP_SERVICE_DISCOVERY_TX_SOCKET_METADATA, 3);
        assert!(::core::mem::size_of::<UdpServiceDiscoveryStorage<24>>() <= 8 * 1_024);
    }
}
