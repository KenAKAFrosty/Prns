mod catalog;
mod codec;

use ::core::net::Ipv6Addr;

use super::AutoWifiStatus;
use catalog::{CatalogUpdate, ResolutionQuery, ServiceCatalog, ServiceResolution};
use codec::{
    build_publication_packet, build_query_packet, encoded_name, query_relevance, DiscoveryInstance,
    QueryRelevance, DNS_TYPE_PTR, MDNS_HOP_LIMIT, MDNS_IPV6_GROUP, MDNS_PORT, SERVICE_LABELS,
};
use embassy_futures::select::{select, select3, select5, Either, Either3, Either5};
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
const MDNS_GROUP: IpAddress = IpAddress::Ipv6(MDNS_IPV6_GROUP);
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

            // Classic LL peering suppresses DNS-SD until the SoftAP/STA link cycles or wifi-auto
            // is re-enabled.
            while self.status.is_dns_sd_suppressed() {
                match select3(
                    self.status.wait_until_dns_sd_allowed(),
                    self.participation.changed(),
                    self.stack.wait_link_down(),
                )
                .await
                {
                    Either3::First(()) => break,
                    Either3::Second(_) => {
                        if !matches!(
                            self.participation.try_get(),
                            Some(EmbeddedDiscoveryParticipation::Central)
                        ) {
                            break;
                        }
                    }
                    Either3::Third(()) => {
                        self.status.clear_dns_sd_suppression();
                        break;
                    }
                }
            }
            if !matches!(
                self.participation.try_get(),
                Some(EmbeddedDiscoveryParticipation::Central)
            ) || self.status.is_dns_sd_suppressed()
            {
                continue;
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
        if let Err(error) = self.stack.join_multicast_group(MDNS_GROUP) {
            crate::diagnostic_log::warn!(
                "wifi-auto: embedded UDP DNS-SD multicast join failed (FF02::FB): {error:?}"
            );
            return PublicationActivation::Retry;
        }
        crate::diagnostic_log::info!(
            "wifi-auto: embedded UDP DNS-SD active (FF02::FB) aaaa={}",
            self.address
        );
        PublicationActivation::Active
    }

    async fn serve(&mut self, instance: &DiscoveryInstance) {
        let mut packet = [0u8; UDP_SERVICE_DISCOVERY_PACKET_BYTES];
        let Some(packet_len) =
            self.encode_publication(&mut packet, instance, PUBLICATION_TTL_SECONDS)
        else {
            return;
        };
        self.publish(
            &packet[..packet_len],
            PublicationPurpose::InitialAnnouncement,
            instance,
        )
        .await;
        self.send_browse_queries().await;

        let mut announcement = Ticker::every(ANNOUNCEMENT_INTERVAL);
        let mut browse = Ticker::every(BROWSE_INTERVAL);
        let mut unrelated_rx: u32 = 0;
        loop {
            match select(
                select5(
                    self.socket.recv_from(&mut self.storage.receive_packet),
                    announcement.next(),
                    browse.next(),
                    self.participation.changed(),
                    self.stack.wait_link_down(),
                ),
                self.status.wait_until_dns_sd_suppressed(),
            )
            .await
            {
                Either::Second(()) => return,
                Either::First(Either5::First(Ok((length, meta)))) => {
                    match query_relevance(&self.storage.receive_packet[..length], instance) {
                        QueryRelevance::Relevant => {
                            crate::diagnostic_log::debug!(
                                "wifi-auto: DNS-SD relevant query from={meta:?} len={length}"
                            );
                            if let Some(packet_len) = self.encode_publication(
                                &mut packet,
                                instance,
                                PUBLICATION_TTL_SECONDS,
                            ) {
                                self.publish(
                                    &packet[..packet_len],
                                    PublicationPurpose::QueryResponse,
                                    instance,
                                )
                                .await;
                            }
                        }
                        QueryRelevance::Response => {
                            crate::diagnostic_log::debug!(
                                "wifi-auto: DNS-SD response from={meta:?} len={length}"
                            );
                            self.apply_response(length, instance);
                        }
                        QueryRelevance::Malformed => {
                            crate::diagnostic_log::debug!(
                                "wifi-auto: DNS-SD malformed from={meta:?} len={length}"
                            );
                        }
                        QueryRelevance::Unrelated => {
                            unrelated_rx = unrelated_rx.saturating_add(1);
                            if unrelated_rx == 1 || unrelated_rx % 32 == 0 {
                                crate::diagnostic_log::debug!(
                                    "wifi-auto: DNS-SD unrelated rx count={unrelated_rx} last_from={meta:?} len={length}"
                                );
                            }
                        }
                    }
                }
                Either::First(Either5::First(Err(error))) => {
                    crate::diagnostic_log::debug!(
                        "wifi-auto: embedded UDP DNS-SD packet dropped: {error:?}"
                    );
                }
                Either::First(Either5::Second(())) => {
                    if let Some(packet_len) =
                        self.encode_publication(&mut packet, instance, PUBLICATION_TTL_SECONDS)
                    {
                        self.publish(&packet[..packet_len], PublicationPurpose::Refresh, instance)
                            .await;
                    }
                }
                Either::First(Either5::Third(())) => {
                    self.prune_targets();
                    self.send_browse_queries().await;
                }
                Either::First(Either5::Fourth(_)) => return,
                Either::First(Either5::Fifth(())) => {
                    self.status.clear_dns_sd_suppression();
                    return;
                }
            }
        }
    }

    async fn deactivate(&mut self, instance: &DiscoveryInstance) {
        let mut goodbye = [0u8; UDP_SERVICE_DISCOVERY_PACKET_BYTES];
        if let Some(goodbye_len) = self.encode_publication(&mut goodbye, instance, 0) {
            self.publish(
                &goodbye[..goodbye_len],
                PublicationPurpose::Withdrawal,
                instance,
            )
            .await;
        }
        self.clear_targets();
        self.socket.close();
        self.leave_multicast_group();
    }

    fn encode_publication(
        &self,
        packet: &mut [u8; UDP_SERVICE_DISCOVERY_PACKET_BYTES],
        instance: &DiscoveryInstance,
        ttl_seconds: u32,
    ) -> Option<usize> {
        match build_publication_packet(packet, instance, self.address, ttl_seconds) {
            Ok(packet_len) => Some(packet_len),
            Err(error) => {
                crate::diagnostic_log::error!(
                    "wifi-auto: embedded UDP DNS-SD packet does not fit: {error:?}"
                );
                None
            }
        }
    }

    fn apply_response(&mut self, packet_length: usize, instance: &DiscoveryInstance) {
        let now_ms = Instant::now().as_millis();
        let packet = &self.storage.receive_packet[..packet_length];
        match self
            .storage
            .catalog
            .apply_response(packet, instance, now_ms)
        {
            CatalogUpdate::Applied => {}
            CatalogUpdate::Malformed => {
                crate::diagnostic_log::debug!(
                    "wifi-auto: DNS-SD response parse malformed len={packet_length}"
                );
                return;
            }
        }
        let current_targets = self.storage.catalog.targets(now_ms, self.address);
        let (resolved, pending, incompatible, expired) =
            self.storage.catalog.resolution_counts(now_ms);
        crate::diagnostic_log::debug!(
            "wifi-auto: DNS-SD catalog services={} targets={} resolved={resolved} pending={pending} incompatible={incompatible} expired={expired}",
            self.storage.catalog.len(),
            current_targets.len()
        );
        // Always publish so the AutoWifi loop can refresh_known_peer on repeated ads, not only
        // when the target set changes.
        self.status.publish_discovery_targets(current_targets);
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
        crate::diagnostic_log::debug!(
            "wifi-auto: DNS-SD browse send start catalog={} queries={query_count}",
            self.storage.catalog.len()
        );
        let mut sent = 0u8;
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
                match self
                    .socket
                    .send_to(&query_packet[..query_length], (MDNS_GROUP, MDNS_PORT))
                    .await
                {
                    Ok(()) => sent = sent.saturating_add(1),
                    Err(error) => {
                        crate::diagnostic_log::debug!(
                            "wifi-auto: DNS-SD browse send failed type={} err={error:?}",
                            query.record_type
                        );
                    }
                }
            }
        });
        match sends.await {
            Ok(()) => {
                crate::diagnostic_log::debug!("wifi-auto: DNS-SD browse send done sent={sent}");
            }
            Err(_timeout) => {
                crate::diagnostic_log::debug!(
                    "wifi-auto: DNS-SD browse send budget exhausted sent={sent}"
                );
            }
        }
    }

    async fn publish(
        &self,
        packet: &[u8],
        purpose: PublicationPurpose,
        instance: &DiscoveryInstance,
    ) {
        match self.send(packet).await {
            PublicationSend::Sent => match purpose {
                PublicationPurpose::Refresh => {}
                PublicationPurpose::InitialAnnouncement
                | PublicationPurpose::QueryResponse
                | PublicationPurpose::Withdrawal => {
                    crate::diagnostic_log::debug!(
                        "wifi-auto: DNS-SD publish {purpose:?} instance={} aaaa={} bytes={}",
                        instance.label(),
                        self.address,
                        packet.len()
                    );
                }
            },
            PublicationSend::Failed => {
                crate::diagnostic_log::debug!(
                    "wifi-auto: DNS-SD publish {purpose:?} failed instance={} aaaa={} bytes={}",
                    instance.label(),
                    self.address,
                    packet.len()
                );
            }
        }
    }

    async fn send(&self, packet: &[u8]) -> PublicationSend {
        match with_timeout(
            SEND_TIMEOUT,
            self.socket.send_to(packet, (MDNS_GROUP, MDNS_PORT)),
        )
        .await
        {
            Ok(Ok(())) => PublicationSend::Sent,
            Ok(Err(_)) | Err(_) => PublicationSend::Failed,
        }
    }

    fn leave_multicast_group(&self) {
        if let Err(error) = self.stack.leave_multicast_group(MDNS_GROUP) {
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
