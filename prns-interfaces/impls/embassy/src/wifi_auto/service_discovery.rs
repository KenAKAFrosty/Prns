use ::core::net::Ipv6Addr;

use embassy_futures::select::{select, select5, Either, Either5};
use embassy_net::udp::UdpSocket;
use embassy_net::{IpAddress, Stack};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::watch::Receiver;
use embassy_time::{with_timeout, Duration, Instant, Ticker, Timer};
use heapless::Vec;

use prns_core::interfaces::wifi_auto as contract;

use super::AutoWifiStatus;

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
const MDNS_PORT: u16 = 5353;
const MDNS_HOP_LIMIT: u8 = 255;
const MDNS_IPV6_GROUP: Ipv6Addr = Ipv6Addr::new(0xff02, 0, 0, 0, 0, 0, 0, 0x00fb);
const DNS_CLASS_IN: u16 = 1;
const DNS_CACHE_FLUSH_CLASS_IN: u16 = 0x8001;
const DNS_TYPE_AAAA: u16 = 28;
const DNS_TYPE_ANY: u16 = 255;
const DNS_TYPE_PTR: u16 = 12;
const DNS_TYPE_SRV: u16 = 33;
const DNS_TYPE_TXT: u16 = 16;
const DNS_RESPONSE_FLAGS: u16 = 0x8400;
const DNS_RECORD_COUNT: u16 = 4;
const DNS_NAME_CAPACITY: usize = 96;
const DNS_POINTER_HOP_LIMIT: u8 = 8;
const PUBLICATION_TTL_SECONDS: u32 = 120;
const ANNOUNCEMENT_INTERVAL: Duration = Duration::from_secs(60);
const BROWSE_INTERVAL: Duration = Duration::from_secs(30);
const FAILURE_RETRY_INTERVAL: Duration = Duration::from_secs(5);
const SEND_TIMEOUT: Duration = Duration::from_millis(300);
const INSTANCE_LABEL_BYTES: usize = contract::EPHEMERAL_DISCOVERY_INSTANCE_PREFIX.len()
    + (contract::EPHEMERAL_DISCOVERY_INSTANCE_RANDOM_BYTES * 2);

const SERVICE_LABELS: [&[u8]; 3] = [b"_reticulum", b"_udp", b"local"];
const LOCAL_LABEL: &[u8] = b"local";
const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";

type DnsName = Vec<u8, DNS_NAME_CAPACITY>;

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

pub struct UdpServiceDiscovery<'a, const TARGETS: usize> {
    socket: UdpSocket<'a>,
    stack: Stack<'a>,
    address: Ipv6Addr,
    participation: DiscoveryParticipationReceiver,
    status: AutoWifiStatus<TARGETS>,
    storage: &'a mut UdpServiceDiscoveryStorage<TARGETS>,
    fill_random: fn(&mut [u8]),
}

impl<'a, const TARGETS: usize> UdpServiceDiscovery<'a, TARGETS> {
    pub fn new(
        socket: UdpSocket<'a>,
        stack: Stack<'a>,
        address: Ipv6Addr,
        status: AutoWifiStatus<TARGETS>,
        storage: &'a mut UdpServiceDiscoveryStorage<TARGETS>,
        fill_random: fn(&mut [u8]),
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
            fill_random,
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

            let instance = DiscoveryInstance::fresh(self.fill_random);
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
                    let _ = select(retry, participation_changed).await;
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
        self.status
            .publish_discovery_targets(super::EmbeddedDiscoveryTargets::new());
    }

    async fn send_browse_queries(&self) {
        let mut query_packet = [0u8; UDP_SERVICE_DISCOVERY_PACKET_BYTES];
        let Ok(service_name) = encoded_name(&SERVICE_LABELS) else {
            return;
        };
        if let Ok(query_length) = build_query_packet(&mut query_packet, &service_name, DNS_TYPE_PTR)
        {
            let _ = self.send(&query_packet[..query_length]).await;
        }
        let queries = self
            .storage
            .catalog
            .resolution_queries(Instant::now().as_millis());
        for query in queries {
            let Ok(query_length) =
                build_query_packet(&mut query_packet, &query.name, query.record_type)
            else {
                continue;
            };
            let _ = self.send(&query_packet[..query_length]).await;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PublicationActivation {
    Active,
    Retry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PublicationSend {
    Sent,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PublicationPurpose {
    InitialAnnouncement,
    QueryResponse,
    Refresh,
    Withdrawal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PacketBuildError {
    BufferTooSmall,
    LabelTooLong,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QueryRelevance {
    Relevant,
    Unrelated,
    Response,
    Malformed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DiscoveryInstance {
    label: [u8; INSTANCE_LABEL_BYTES],
}

impl DiscoveryInstance {
    fn fresh(fill_random: fn(&mut [u8])) -> Self {
        let mut random = [0u8; contract::EPHEMERAL_DISCOVERY_INSTANCE_RANDOM_BYTES];
        fill_random(&mut random);
        Self::from_random_bytes(random)
    }

    fn from_random_bytes(
        random: [u8; contract::EPHEMERAL_DISCOVERY_INSTANCE_RANDOM_BYTES],
    ) -> Self {
        let mut label = [0u8; INSTANCE_LABEL_BYTES];
        let prefix = contract::EPHEMERAL_DISCOVERY_INSTANCE_PREFIX.as_bytes();
        label[..prefix.len()].copy_from_slice(prefix);
        for (index, byte) in random.into_iter().enumerate() {
            let output = prefix.len() + (index * 2);
            label[output] = HEX_DIGITS[usize::from(byte >> 4)];
            label[output + 1] = HEX_DIGITS[usize::from(byte & 0x0f)];
        }
        Self { label }
    }

    fn service_labels(&self) -> [&[u8]; 4] {
        [
            &self.label,
            SERVICE_LABELS[0],
            SERVICE_LABELS[1],
            SERVICE_LABELS[2],
        ]
    }

    fn host_labels(&self) -> [&[u8]; 2] {
        [&self.label, LOCAL_LABEL]
    }
}

fn build_publication_packet(
    output: &mut [u8],
    instance: &DiscoveryInstance,
    address: Ipv6Addr,
    ttl_seconds: u32,
) -> Result<usize, PacketBuildError> {
    let mut writer = PacketWriter::new(output);
    writer.write_u16(0)?;
    writer.write_u16(DNS_RESPONSE_FLAGS)?;
    writer.write_u16(0)?;
    writer.write_u16(DNS_RECORD_COUNT)?;
    writer.write_u16(0)?;
    writer.write_u16(0)?;

    let service_labels = SERVICE_LABELS;
    let instance_labels = instance.service_labels();
    let host_labels = instance.host_labels();

    writer.write_record(
        &service_labels,
        DNS_TYPE_PTR,
        DNS_CLASS_IN,
        ttl_seconds,
        |writer| writer.write_name(&instance_labels),
    )?;
    writer.write_record(
        &instance_labels,
        DNS_TYPE_SRV,
        DNS_CACHE_FLUSH_CLASS_IN,
        ttl_seconds,
        |writer| {
            writer.write_u16(0)?;
            writer.write_u16(0)?;
            writer.write_u16(contract::UNICAST_DISCOVERY_PORT)?;
            writer.write_name(&host_labels)
        },
    )?;
    writer.write_record(
        &instance_labels,
        DNS_TYPE_TXT,
        DNS_CACHE_FLUSH_CLASS_IN,
        ttl_seconds,
        |writer| {
            let txt_length =
                contract::TXT_VERSION_KEY.len() + 1 + contract::TXT_VERSION_VALUE.len();
            writer
                .write_u8(u8::try_from(txt_length).map_err(|_| PacketBuildError::LabelTooLong)?)?;
            writer.write_bytes(contract::TXT_VERSION_KEY.as_bytes())?;
            writer.write_u8(b'=')?;
            writer.write_bytes(contract::TXT_VERSION_VALUE.as_bytes())
        },
    )?;
    writer.write_record(
        &host_labels,
        DNS_TYPE_AAAA,
        DNS_CACHE_FLUSH_CLASS_IN,
        ttl_seconds,
        |writer| writer.write_bytes(&address.octets()),
    )?;
    Ok(writer.len())
}

fn query_relevance(packet: &[u8], instance: &DiscoveryInstance) -> QueryRelevance {
    if packet.len() < 12 {
        return QueryRelevance::Malformed;
    }
    let Some(flags) = read_u16(packet, 2) else {
        return QueryRelevance::Malformed;
    };
    if flags & 0x8000 != 0 {
        return QueryRelevance::Response;
    }
    let Some(question_count) = read_u16(packet, 4) else {
        return QueryRelevance::Malformed;
    };
    let mut cursor = 12usize;
    for _ in 0..question_count {
        let Ok((name, next_cursor)) = decode_name(packet, cursor) else {
            return QueryRelevance::Malformed;
        };
        cursor = next_cursor;
        let (Some(question_type), Some(question_class)) =
            (read_u16(packet, cursor), read_u16(packet, cursor + 2))
        else {
            return QueryRelevance::Malformed;
        };
        cursor += 4;
        if question_class & 0x7fff != DNS_CLASS_IN {
            continue;
        }

        let service_query = name_matches(&name, &SERVICE_LABELS)
            && matches!(question_type, DNS_TYPE_PTR | DNS_TYPE_ANY);
        let instance_query = name_matches(&name, &instance.service_labels())
            && matches!(question_type, DNS_TYPE_SRV | DNS_TYPE_TXT | DNS_TYPE_ANY);
        let host_query = name_matches(&name, &instance.host_labels())
            && matches!(question_type, DNS_TYPE_AAAA | DNS_TYPE_ANY);
        if service_query || instance_query || host_query {
            return QueryRelevance::Relevant;
        }
    }
    QueryRelevance::Unrelated
}

fn build_query_packet(
    output: &mut [u8],
    name: &DnsName,
    record_type: u16,
) -> Result<usize, PacketBuildError> {
    let mut writer = PacketWriter::new(output);
    writer.write_u16(0)?;
    writer.write_u16(0)?;
    writer.write_u16(1)?;
    writer.write_u16(0)?;
    writer.write_u16(0)?;
    writer.write_u16(0)?;
    writer.write_encoded_name(name)?;
    writer.write_u16(record_type)?;
    writer.write_u16(DNS_CLASS_IN)?;
    Ok(writer.len())
}

fn encoded_name<const LABELS: usize>(
    labels: &[&[u8]; LABELS],
) -> Result<DnsName, PacketBuildError> {
    let mut encoded = DnsName::new();
    for label in labels {
        let length = u8::try_from(label.len()).map_err(|_| PacketBuildError::LabelTooLong)?;
        if length > 63 {
            return Err(PacketBuildError::LabelTooLong);
        }
        encoded
            .push(length)
            .map_err(|_| PacketBuildError::BufferTooSmall)?;
        encoded
            .extend_from_slice(label)
            .map_err(|_| PacketBuildError::BufferTooSmall)?;
    }
    encoded
        .push(0)
        .map_err(|_| PacketBuildError::BufferTooSmall)?;
    Ok(encoded)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CatalogUpdate {
    Applied,
    Malformed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecordCompatibility {
    Awaiting,
    Compatible,
    Incompatible,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BrowsedService {
    instance: DnsName,
    host: Option<DnsName>,
    address: Option<Ipv6Addr>,
    port: RecordCompatibility,
    version: RecordCompatibility,
    expires_at_ms: u64,
}

impl BrowsedService {
    fn new(instance: DnsName, expires_at_ms: u64) -> Self {
        Self {
            instance,
            host: None,
            address: None,
            port: RecordCompatibility::Awaiting,
            version: RecordCompatibility::Compatible,
            expires_at_ms,
        }
    }

    fn target(&self, now_ms: u64) -> Option<Ipv6Addr> {
        if self.expires_at_ms <= now_ms
            || self.port != RecordCompatibility::Compatible
            || self.version != RecordCompatibility::Compatible
        {
            return None;
        }
        self.address
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolutionQuery {
    name: DnsName,
    record_type: u16,
}

struct ServiceCatalog<const TARGETS: usize> {
    services: Vec<BrowsedService, TARGETS>,
}

impl<const TARGETS: usize> ServiceCatalog<TARGETS> {
    const fn new() -> Self {
        Self {
            services: Vec::new(),
        }
    }

    fn apply_response(
        &mut self,
        packet: &[u8],
        local_instance: &DiscoveryInstance,
        now_ms: u64,
    ) -> CatalogUpdate {
        if visit_resource_records(packet, |_| {}).is_err() {
            return CatalogUpdate::Malformed;
        }
        let own_service = match encoded_name(&local_instance.service_labels()) {
            Ok(own_service) => own_service,
            Err(_) => return CatalogUpdate::Malformed,
        };
        let pointer_pass = visit_resource_records(packet, |record| {
            self.apply_pointer_record(packet, record, &own_service, now_ms);
        });
        if pointer_pass.is_err() {
            return CatalogUpdate::Malformed;
        }
        let service_pass = visit_resource_records(packet, |record| {
            self.apply_service_record(packet, record, now_ms);
        });
        if service_pass.is_err() {
            return CatalogUpdate::Malformed;
        }
        if self.prepare_address_updates(packet).is_err() {
            return CatalogUpdate::Malformed;
        }
        let address_pass = visit_resource_records(packet, |record| {
            self.apply_address_record(record, now_ms);
        });
        match address_pass {
            Ok(()) => CatalogUpdate::Applied,
            Err(()) => CatalogUpdate::Malformed,
        }
    }

    fn apply_pointer_record(
        &mut self,
        packet: &[u8],
        record: DnsResourceRecord<'_>,
        own_service: &DnsName,
        now_ms: u64,
    ) {
        if record.record_class & 0x7fff != DNS_CLASS_IN
            || record.record_type != DNS_TYPE_PTR
            || !name_matches(&record.name, &SERVICE_LABELS)
        {
            return;
        }
        let Ok((instance, next_cursor)) = decode_name(packet, record.data_offset) else {
            return;
        };
        if next_cursor != record.data_end
            || instance == *own_service
            || !is_udp_service_instance(&instance)
        {
            return;
        }
        if record.ttl_seconds == 0 {
            self.remove(&instance);
            return;
        }
        let expires_at_ms = record_expiry(now_ms, record.ttl_seconds);
        if let Some(service) = self.find_mut(&instance) {
            service.expires_at_ms = expires_at_ms;
        } else if self.services.len() < TARGETS {
            let _ = self
                .services
                .push(BrowsedService::new(instance, expires_at_ms));
        }
    }

    fn apply_service_record(&mut self, packet: &[u8], record: DnsResourceRecord<'_>, now_ms: u64) {
        if record.record_class & 0x7fff != DNS_CLASS_IN {
            return;
        }
        match record.record_type {
            DNS_TYPE_SRV => {
                if record.ttl_seconds == 0 {
                    self.remove(&record.name);
                    return;
                }
                let Some(port_offset) = record.data_offset.checked_add(4) else {
                    return;
                };
                let Some(host_offset) = record.data_offset.checked_add(6) else {
                    return;
                };
                let Some(port) = read_u16(packet, port_offset) else {
                    return;
                };
                let Ok((host, next_cursor)) = decode_name(packet, host_offset) else {
                    return;
                };
                if next_cursor != record.data_end {
                    return;
                }
                let Some(service) = self.find_mut(&record.name) else {
                    return;
                };
                if service.host.as_ref() != Some(&host) {
                    service.address = None;
                }
                service.host = Some(host);
                service.port = if port == contract::UNICAST_DISCOVERY_PORT {
                    RecordCompatibility::Compatible
                } else {
                    RecordCompatibility::Incompatible
                };
                service.expires_at_ms = record_expiry(now_ms, record.ttl_seconds);
            }
            DNS_TYPE_TXT => {
                if record.ttl_seconds == 0 {
                    self.remove(&record.name);
                    return;
                }
                let Some(service) = self.find_mut(&record.name) else {
                    return;
                };
                service.version = txt_version_compatibility(record.data);
                service.expires_at_ms = record_expiry(now_ms, record.ttl_seconds);
            }
            _ => {}
        }
    }

    fn apply_address_record(&mut self, record: DnsResourceRecord<'_>, now_ms: u64) {
        if record.record_class & 0x7fff != DNS_CLASS_IN
            || record.record_type != DNS_TYPE_AAAA
            || record.data.len() != 16
        {
            return;
        }
        let mut octets = [0u8; 16];
        octets.copy_from_slice(record.data);
        let address = Ipv6Addr::from(octets);
        for service in &mut self.services {
            if service.host.as_ref() != Some(&record.name) {
                continue;
            }
            if record.ttl_seconds == 0 {
                if service.address == Some(address) {
                    service.address = None;
                }
                continue;
            }
            if address.is_unicast_link_local()
                && service.address.is_none_or(|current| address < current)
            {
                service.address = Some(address);
                service.expires_at_ms = record_expiry(now_ms, record.ttl_seconds);
            }
        }
    }

    fn prepare_address_updates(&mut self, packet: &[u8]) -> Result<(), ()> {
        for service in &mut self.services {
            let Some(host) = service.host.as_ref() else {
                continue;
            };
            let mut address_replaced = false;
            visit_resource_records(packet, |record| {
                if record.name == *host
                    && record.record_type == DNS_TYPE_AAAA
                    && record.record_class & DNS_CACHE_FLUSH_CLASS_IN == DNS_CACHE_FLUSH_CLASS_IN
                    && record.ttl_seconds != 0
                {
                    address_replaced = true;
                }
            })?;
            if address_replaced {
                service.address = None;
            }
        }
        Ok(())
    }

    fn targets(
        &self,
        now_ms: u64,
        local_address: Ipv6Addr,
    ) -> super::EmbeddedDiscoveryTargets<TARGETS> {
        let mut targets = super::EmbeddedDiscoveryTargets::new();
        for address in self
            .services
            .iter()
            .filter_map(|service| service.target(now_ms))
            .filter(|address| *address != local_address)
        {
            targets.insert(address);
        }
        targets
    }

    fn resolution_queries(&self, now_ms: u64) -> Vec<ResolutionQuery, TARGETS> {
        let mut queries = Vec::new();
        for service in &self.services {
            if service.expires_at_ms <= now_ms
                || service.version == RecordCompatibility::Incompatible
                || service.port == RecordCompatibility::Incompatible
            {
                continue;
            }
            let query = if service.port == RecordCompatibility::Awaiting || service.host.is_none() {
                ResolutionQuery {
                    name: service.instance.clone(),
                    record_type: DNS_TYPE_ANY,
                }
            } else if service.address.is_none() {
                let Some(host) = service.host.clone() else {
                    continue;
                };
                ResolutionQuery {
                    name: host,
                    record_type: DNS_TYPE_AAAA,
                }
            } else {
                continue;
            };
            let _ = queries.push(query);
        }
        queries
    }

    fn prune(&mut self, now_ms: u64) {
        let mut index = 0;
        while index < self.services.len() {
            if self.services[index].expires_at_ms <= now_ms {
                self.services.swap_remove(index);
            } else {
                index += 1;
            }
        }
    }

    fn clear(&mut self) {
        self.services.clear();
    }

    fn find_mut(&mut self, instance: &DnsName) -> Option<&mut BrowsedService> {
        self.services
            .iter_mut()
            .find(|service| service.instance == *instance)
    }

    fn remove(&mut self, instance: &DnsName) {
        if let Some(index) = self
            .services
            .iter()
            .position(|service| service.instance == *instance)
        {
            self.services.swap_remove(index);
        }
    }
}

struct DnsResourceRecord<'a> {
    name: DnsName,
    record_type: u16,
    record_class: u16,
    ttl_seconds: u32,
    data_offset: usize,
    data_end: usize,
    data: &'a [u8],
}

fn visit_resource_records(
    packet: &[u8],
    mut visitor: impl FnMut(DnsResourceRecord<'_>),
) -> Result<(), ()> {
    if packet.len() < 12 {
        return Err(());
    }
    let question_count = usize::from(read_u16(packet, 4).ok_or(())?);
    let answer_count = usize::from(read_u16(packet, 6).ok_or(())?);
    let authority_count = usize::from(read_u16(packet, 8).ok_or(())?);
    let additional_count = usize::from(read_u16(packet, 10).ok_or(())?);
    let record_count = answer_count
        .checked_add(authority_count)
        .and_then(|count| count.checked_add(additional_count))
        .ok_or(())?;
    let mut cursor = 12usize;
    for _ in 0..question_count {
        let (_, next_cursor) = decode_name(packet, cursor)?;
        cursor = next_cursor.checked_add(4).ok_or(())?;
        if cursor > packet.len() {
            return Err(());
        }
    }
    for _ in 0..record_count {
        let (name, next_cursor) = decode_name(packet, cursor)?;
        cursor = next_cursor;
        let record_type = read_u16(packet, cursor).ok_or(())?;
        let record_class = read_u16(packet, cursor.checked_add(2).ok_or(())?).ok_or(())?;
        let ttl_seconds = read_u32(packet, cursor.checked_add(4).ok_or(())?).ok_or(())?;
        let data_length =
            usize::from(read_u16(packet, cursor.checked_add(8).ok_or(())?).ok_or(())?);
        let data_offset = cursor.checked_add(10).ok_or(())?;
        let data_end = data_offset.checked_add(data_length).ok_or(())?;
        let data = packet.get(data_offset..data_end).ok_or(())?;
        visitor(DnsResourceRecord {
            name,
            record_type,
            record_class,
            ttl_seconds,
            data_offset,
            data_end,
            data,
        });
        cursor = data_end;
    }
    Ok(())
}

fn txt_version_compatibility(data: &[u8]) -> RecordCompatibility {
    let mut cursor = 0usize;
    let mut version = None;
    while cursor < data.len() {
        let Some(length) = data.get(cursor).copied().map(usize::from) else {
            return RecordCompatibility::Incompatible;
        };
        cursor += 1;
        let Some(value) = data.get(cursor..cursor.saturating_add(length)) else {
            return RecordCompatibility::Incompatible;
        };
        cursor += length;
        let Some(separator) = value.iter().position(|byte| *byte == b'=') else {
            continue;
        };
        if value[..separator].eq_ignore_ascii_case(contract::TXT_VERSION_KEY.as_bytes()) {
            if version.is_some() {
                return RecordCompatibility::Incompatible;
            }
            version = Some(&value[separator + 1..]);
        }
    }
    match version {
        None => RecordCompatibility::Compatible,
        Some(value) if value == contract::TXT_VERSION_VALUE.as_bytes() => {
            RecordCompatibility::Compatible
        }
        Some(_) => RecordCompatibility::Incompatible,
    }
}

fn is_udp_service_instance(name: &DnsName) -> bool {
    let Some(instance_length) = name.first().copied().map(usize::from) else {
        return false;
    };
    if instance_length == 0 {
        return false;
    }
    let service_offset = match 1usize.checked_add(instance_length) {
        Some(service_offset) => service_offset,
        None => return false,
    };
    let Ok(service_name) = encoded_name(&SERVICE_LABELS) else {
        return false;
    };
    name.get(service_offset..) == Some(service_name.as_slice())
}

fn record_expiry(now_ms: u64, ttl_seconds: u32) -> u64 {
    now_ms.saturating_add(u64::from(ttl_seconds).saturating_mul(1_000))
}

fn decode_name(packet: &[u8], start: usize) -> Result<(Vec<u8, DNS_NAME_CAPACITY>, usize), ()> {
    let mut decoded = Vec::new();
    let mut cursor = start;
    let mut next_cursor = None;
    let mut pointer_hops = 0u8;
    loop {
        let Some(length) = packet.get(cursor).copied() else {
            return Err(());
        };
        if length == 0 {
            let end = next_cursor.unwrap_or(cursor + 1);
            decoded.push(0).map_err(|_| ())?;
            return Ok((decoded, end));
        }
        if length & 0xc0 == 0xc0 {
            let Some(second) = packet.get(cursor + 1).copied() else {
                return Err(());
            };
            if next_cursor.is_none() {
                next_cursor = Some(cursor + 2);
            }
            pointer_hops = pointer_hops.checked_add(1).ok_or(())?;
            if pointer_hops > DNS_POINTER_HOP_LIMIT {
                return Err(());
            }
            cursor = (usize::from(length & 0x3f) << 8) | usize::from(second);
            continue;
        }
        if length > 63 || length & 0xc0 != 0 {
            return Err(());
        }
        let label_start = cursor + 1;
        let label_end = label_start.checked_add(usize::from(length)).ok_or(())?;
        let Some(label) = packet.get(label_start..label_end) else {
            return Err(());
        };
        decoded.push(length).map_err(|_| ())?;
        decoded.extend_from_slice(label).map_err(|_| ())?;
        cursor = label_end;
    }
}

fn name_matches<const LABELS: usize>(encoded: &[u8], labels: &[&[u8]; LABELS]) -> bool {
    let mut expected = Vec::<u8, DNS_NAME_CAPACITY>::new();
    for label in labels {
        let Ok(length) = u8::try_from(label.len()) else {
            return false;
        };
        if expected.push(length).is_err() || expected.extend_from_slice(label).is_err() {
            return false;
        }
    }
    if expected.push(0).is_err() {
        return false;
    }
    encoded.eq_ignore_ascii_case(&expected)
}

fn read_u16(packet: &[u8], offset: usize) -> Option<u16> {
    let bytes = packet.get(offset..offset.checked_add(2)?)?;
    Some(u16::from_be_bytes([bytes[0], bytes[1]]))
}

fn read_u32(packet: &[u8], offset: usize) -> Option<u32> {
    let bytes = packet.get(offset..offset.checked_add(4)?)?;
    Some(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

struct PacketWriter<'a> {
    output: &'a mut [u8],
    cursor: usize,
}

impl<'a> PacketWriter<'a> {
    fn new(output: &'a mut [u8]) -> Self {
        Self { output, cursor: 0 }
    }

    fn len(&self) -> usize {
        self.cursor
    }

    fn write_u8(&mut self, value: u8) -> Result<(), PacketBuildError> {
        let Some(slot) = self.output.get_mut(self.cursor) else {
            return Err(PacketBuildError::BufferTooSmall);
        };
        *slot = value;
        self.cursor += 1;
        Ok(())
    }

    fn write_u16(&mut self, value: u16) -> Result<(), PacketBuildError> {
        self.write_bytes(&value.to_be_bytes())
    }

    fn write_u32(&mut self, value: u32) -> Result<(), PacketBuildError> {
        self.write_bytes(&value.to_be_bytes())
    }

    fn write_bytes(&mut self, bytes: &[u8]) -> Result<(), PacketBuildError> {
        let end = self
            .cursor
            .checked_add(bytes.len())
            .ok_or(PacketBuildError::BufferTooSmall)?;
        let Some(output) = self.output.get_mut(self.cursor..end) else {
            return Err(PacketBuildError::BufferTooSmall);
        };
        output.copy_from_slice(bytes);
        self.cursor = end;
        Ok(())
    }

    fn write_name<const LABELS: usize>(
        &mut self,
        labels: &[&[u8]; LABELS],
    ) -> Result<(), PacketBuildError> {
        for label in labels {
            let length = u8::try_from(label.len()).map_err(|_| PacketBuildError::LabelTooLong)?;
            if length > 63 {
                return Err(PacketBuildError::LabelTooLong);
            }
            self.write_u8(length)?;
            self.write_bytes(label)?;
        }
        self.write_u8(0)
    }

    fn write_encoded_name(&mut self, name: &DnsName) -> Result<(), PacketBuildError> {
        if name.last() != Some(&0) {
            return Err(PacketBuildError::LabelTooLong);
        }
        self.write_bytes(name)
    }

    fn write_record<const LABELS: usize>(
        &mut self,
        name: &[&[u8]; LABELS],
        record_type: u16,
        class: u16,
        ttl_seconds: u32,
        write_data: impl FnOnce(&mut Self) -> Result<(), PacketBuildError>,
    ) -> Result<(), PacketBuildError> {
        self.write_name(name)?;
        self.write_u16(record_type)?;
        self.write_u16(class)?;
        self.write_u32(ttl_seconds)?;
        let data_length_offset = self.cursor;
        self.write_u16(0)?;
        let data_start = self.cursor;
        write_data(self)?;
        let data_length = u16::try_from(self.cursor - data_start)
            .map_err(|_| PacketBuildError::BufferTooSmall)?;
        let length_bytes = data_length.to_be_bytes();
        self.output[data_length_offset..data_length_offset + 2].copy_from_slice(&length_bytes);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INSTANCE_RANDOM: [u8; contract::EPHEMERAL_DISCOVERY_INSTANCE_RANDOM_BYTES] =
        [0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef];
    const LINK_LOCAL: Ipv6Addr = Ipv6Addr::new(0xfe80, 0, 0, 0, 0x0212, 0x34ff, 0xfe56, 0x789a);

    #[test]
    fn publication_is_bounded_udp_dns_sd() {
        let instance = DiscoveryInstance::from_random_bytes(INSTANCE_RANDOM);
        let mut packet = [0u8; UDP_SERVICE_DISCOVERY_PACKET_BYTES];
        let length =
            build_publication_packet(&mut packet, &instance, LINK_LOCAL, PUBLICATION_TTL_SECONDS)
                .expect("the fixed publication capacity fits the complete record set");

        assert!(length <= UDP_SERVICE_DISCOVERY_PACKET_BYTES);
        assert_eq!(read_u16(&packet[..length], 6), Some(DNS_RECORD_COUNT));
        assert!(packet[..length]
            .windows(2)
            .any(|window| window == contract::UNICAST_DISCOVERY_PORT.to_be_bytes()));
        assert!(packet[..length].windows(3).any(|window| window == b"v=1"));
        assert!(packet[..length]
            .windows(LINK_LOCAL.octets().len())
            .any(|window| window == LINK_LOCAL.octets()));
        assert!(!packet[..length]
            .windows(2)
            .any(|window| window == contract::TCP_RENDEZVOUS_PORT.to_be_bytes()));
    }

    #[test]
    fn goodbye_uses_zero_ttl_for_every_record() {
        let instance = DiscoveryInstance::from_random_bytes(INSTANCE_RANDOM);
        let mut packet = [0u8; UDP_SERVICE_DISCOVERY_PACKET_BYTES];
        let length = build_publication_packet(&mut packet, &instance, LINK_LOCAL, 0)
            .expect("the fixed publication capacity fits the goodbye record set");
        let mut cursor = 12;
        for _ in 0..DNS_RECORD_COUNT {
            let (_, next) = decode_name(&packet[..length], cursor).expect("record name is valid");
            cursor = next + 4;
            assert_eq!(packet.get(cursor..cursor + 4), Some(&[0, 0, 0, 0][..]));
            cursor += 4;
            let data_length = usize::from(read_u16(&packet[..length], cursor).expect("RDLENGTH"));
            cursor += 2 + data_length;
        }
        assert_eq!(cursor, length);
    }

    #[test]
    fn only_relevant_queries_receive_a_publication() {
        let instance = DiscoveryInstance::from_random_bytes(INSTANCE_RANDOM);
        let service_query = query_packet(&SERVICE_LABELS, DNS_TYPE_PTR);
        assert_eq!(
            query_relevance(&service_query, &instance),
            QueryRelevance::Relevant
        );

        let unrelated = query_packet(&[b"_other", b"_udp", b"local"], DNS_TYPE_PTR);
        assert_eq!(
            query_relevance(&unrelated, &instance),
            QueryRelevance::Unrelated
        );

        let mut response = [0u8; UDP_SERVICE_DISCOVERY_PACKET_BYTES];
        let response_len = build_publication_packet(
            &mut response,
            &instance,
            LINK_LOCAL,
            PUBLICATION_TTL_SECONDS,
        )
        .expect("the fixed publication capacity fits the response");
        assert_eq!(
            query_relevance(&response[..response_len], &instance),
            QueryRelevance::Response
        );

        assert_eq!(
            query_relevance(&service_query[..service_query.len() - 1], &instance),
            QueryRelevance::Malformed
        );
    }

    #[test]
    fn compressed_query_names_are_bounded_and_supported() {
        let instance = DiscoveryInstance::from_random_bytes(INSTANCE_RANDOM);
        let mut packet = Vec::<u8, 96>::new();
        packet
            .extend_from_slice(&[0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0])
            .expect("header fits");
        packet
            .extend_from_slice(&[0xc0, 0x12])
            .expect("pointer fits");
        packet
            .extend_from_slice(&DNS_TYPE_PTR.to_be_bytes())
            .expect("type fits");
        packet
            .extend_from_slice(&DNS_CLASS_IN.to_be_bytes())
            .expect("class fits");
        push_name(&mut packet, &SERVICE_LABELS);

        assert_eq!(
            query_relevance(&packet, &instance),
            QueryRelevance::Relevant
        );
    }

    #[test]
    fn instance_name_is_ephemeral_material_only() {
        let first = DiscoveryInstance::from_random_bytes(INSTANCE_RANDOM);
        let second = DiscoveryInstance::from_random_bytes([0xff; 8]);
        assert_eq!(&first.label, b"prns-0123456789abcdef");
        assert_ne!(first, second);
    }

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
    fn browser_resolves_records_independently_of_packet_order() {
        let local_instance = DiscoveryInstance::from_random_bytes(INSTANCE_RANDOM);
        let peer_instance = DiscoveryInstance::from_random_bytes([0x22; 8]);
        let peer_address = Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 0x22);
        let publication = publication_packet(&peer_instance, peer_address, 120);
        let reordered = reorder_publication_records(&publication, [3, 2, 1, 0]);
        let mut catalog = ServiceCatalog::<2>::new();

        assert_eq!(
            catalog.apply_response(&reordered, &local_instance, 1_000),
            CatalogUpdate::Applied
        );
        assert_eq!(
            catalog
                .targets(1_000, LINK_LOCAL)
                .iter()
                .collect::<Vec<Ipv6Addr, 2>>(),
            Vec::<Ipv6Addr, 2>::from_slice(&[peer_address]).expect("one target fits")
        );

        let own_publication = publication_packet(&local_instance, LINK_LOCAL, 120);
        assert_eq!(
            catalog.apply_response(&own_publication, &local_instance, 1_000),
            CatalogUpdate::Applied
        );
        assert_eq!(catalog.services.len(), 1);
    }

    #[test]
    fn browser_capacity_keeps_known_updates_and_removals_free_slots() {
        let local_instance = DiscoveryInstance::from_random_bytes(INSTANCE_RANDOM);
        let first_instance = DiscoveryInstance::from_random_bytes([0x11; 8]);
        let second_instance = DiscoveryInstance::from_random_bytes([0x22; 8]);
        let third_instance = DiscoveryInstance::from_random_bytes([0x33; 8]);
        let first_address = Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 0x31);
        let updated_first_address = Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 0x41);
        let second_address = Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 0x22);
        let third_address = Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 0x13);
        let mut catalog = ServiceCatalog::<2>::new();

        for (instance, address) in [
            (&first_instance, first_address),
            (&second_instance, second_address),
            (&third_instance, third_address),
        ] {
            assert_eq!(
                catalog.apply_response(
                    &publication_packet(instance, address, 120),
                    &local_instance,
                    1_000,
                ),
                CatalogUpdate::Applied
            );
        }
        assert_eq!(catalog.services.len(), 2);
        assert_eq!(
            catalog
                .targets(1_000, LINK_LOCAL)
                .iter()
                .collect::<Vec<Ipv6Addr, 2>>(),
            Vec::<Ipv6Addr, 2>::from_slice(&[second_address, first_address])
                .expect("two targets fit")
        );

        catalog.apply_response(
            &publication_packet(&first_instance, updated_first_address, 120),
            &local_instance,
            2_000,
        );
        assert_eq!(
            catalog
                .targets(2_000, LINK_LOCAL)
                .iter()
                .collect::<Vec<Ipv6Addr, 2>>(),
            Vec::<Ipv6Addr, 2>::from_slice(&[second_address, updated_first_address])
                .expect("two targets fit")
        );

        catalog.apply_response(
            &publication_packet(&second_instance, second_address, 0),
            &local_instance,
            3_000,
        );
        catalog.apply_response(
            &publication_packet(&third_instance, third_address, 120),
            &local_instance,
            3_000,
        );
        assert_eq!(
            catalog
                .targets(3_000, LINK_LOCAL)
                .iter()
                .collect::<Vec<Ipv6Addr, 2>>(),
            Vec::<Ipv6Addr, 2>::from_slice(&[third_address, updated_first_address])
                .expect("two targets fit")
        );
    }

    #[test]
    fn browser_rejects_incompatible_records_and_expires_stale_targets() {
        let local_instance = DiscoveryInstance::from_random_bytes(INSTANCE_RANDOM);
        let peer_instance = DiscoveryInstance::from_random_bytes([0x44; 8]);
        let peer_address = Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 0x44);
        let mut unsupported_version = publication_packet(&peer_instance, peer_address, 120);
        let version = unsupported_version
            .windows(3)
            .position(|window| window == b"v=1")
            .expect("publication contains the version");
        unsupported_version[version + 2] = b'2';
        let mut catalog = ServiceCatalog::<2>::new();

        catalog.apply_response(&unsupported_version, &local_instance, 1_000);
        assert!(catalog.targets(1_000, LINK_LOCAL).iter().next().is_none());

        let expiring = publication_packet(&peer_instance, peer_address, 1);
        catalog.apply_response(&expiring, &local_instance, 2_000);
        assert_eq!(
            catalog.targets(2_999, LINK_LOCAL).iter().next(),
            Some(peer_address)
        );
        catalog.prune(3_000);
        assert!(catalog.targets(3_000, LINK_LOCAL).iter().next().is_none());

        assert_eq!(
            txt_version_compatibility(&[]),
            RecordCompatibility::Compatible
        );
        assert_eq!(
            txt_version_compatibility(&[3, b'v', b'=', b'1']),
            RecordCompatibility::Compatible
        );
        assert_eq!(
            txt_version_compatibility(&[3, b'v', b'=', b'9']),
            RecordCompatibility::Incompatible
        );
        assert_eq!(
            txt_version_compatibility(&[4, b'v', b'=', b'1']),
            RecordCompatibility::Incompatible
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

    fn publication_packet(
        instance: &DiscoveryInstance,
        address: Ipv6Addr,
        ttl_seconds: u32,
    ) -> Vec<u8, UDP_SERVICE_DISCOVERY_PACKET_BYTES> {
        let mut packet = [0u8; UDP_SERVICE_DISCOVERY_PACKET_BYTES];
        let length = build_publication_packet(&mut packet, instance, address, ttl_seconds)
            .expect("publication fits");
        Vec::from_slice(&packet[..length]).expect("publication capacity matches output")
    }

    fn reorder_publication_records(
        packet: &[u8],
        order: [usize; DNS_RECORD_COUNT as usize],
    ) -> Vec<u8, UDP_SERVICE_DISCOVERY_PACKET_BYTES> {
        let mut ranges = Vec::<(usize, usize), { DNS_RECORD_COUNT as usize }>::new();
        let mut cursor = 12usize;
        for _ in 0..DNS_RECORD_COUNT {
            let start = cursor;
            let (_, next_cursor) = decode_name(packet, cursor).expect("record name is valid");
            let data_length = usize::from(
                read_u16(packet, next_cursor + 8).expect("record data length is present"),
            );
            cursor = next_cursor + 10 + data_length;
            ranges.push((start, cursor)).expect("record range fits");
        }
        let mut reordered = Vec::new();
        reordered
            .extend_from_slice(&packet[..12])
            .expect("header fits");
        for index in order {
            let (start, end) = ranges[index];
            reordered
                .extend_from_slice(&packet[start..end])
                .expect("records fit");
        }
        reordered
    }

    fn query_packet<const LABELS: usize>(labels: &[&[u8]; LABELS], query_type: u16) -> Vec<u8, 96> {
        let mut packet = Vec::new();
        packet
            .extend_from_slice(&[0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0])
            .expect("header fits");
        push_name(&mut packet, labels);
        packet
            .extend_from_slice(&query_type.to_be_bytes())
            .expect("type fits");
        packet
            .extend_from_slice(&DNS_CLASS_IN.to_be_bytes())
            .expect("class fits");
        packet
    }

    fn push_name<const CAPACITY: usize, const LABELS: usize>(
        packet: &mut Vec<u8, CAPACITY>,
        labels: &[&[u8]; LABELS],
    ) {
        for label in labels {
            packet
                .push(u8::try_from(label.len()).expect("test label length fits"))
                .expect("query fits");
            packet.extend_from_slice(label).expect("query fits");
        }
        packet.push(0).expect("query fits");
    }
}
