use ::core::net::Ipv6Addr;

use embassy_futures::select::{select, select3, Either, Either3};
use embassy_net::udp::UdpSocket;
use embassy_net::{IpAddress, Stack};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::watch::Receiver;
use embassy_time::{with_timeout, Duration, Ticker, Timer};
use heapless::Vec;

use prns_core::interfaces::wifi_auto as contract;

use super::AutoWifiStatus;

pub const EMBEDDED_DISCOVERY_PUBLISHER_CAPACITY: u8 = 1;
pub const UDP_SERVICE_DISCOVERY_SOCKET_COUNT: u8 = 1;
pub const UDP_SERVICE_DISCOVERY_PACKET_BYTES: usize = 384;
pub const UDP_SERVICE_DISCOVERY_SOCKET_BYTES: usize = 512;
pub const UDP_SERVICE_DISCOVERY_SOCKET_METADATA: usize = 2;

const DISCOVERY_WATCHERS: usize = EMBEDDED_DISCOVERY_PUBLISHER_CAPACITY as usize;
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
const DNS_NAME_CAPACITY: usize = 64;
const DNS_POINTER_HOP_LIMIT: u8 = 8;
const PUBLICATION_TTL_SECONDS: u32 = 120;
const ANNOUNCEMENT_INTERVAL: Duration = Duration::from_secs(60);
const FAILURE_RETRY_INTERVAL: Duration = Duration::from_secs(5);
const SEND_TIMEOUT: Duration = Duration::from_millis(300);
const INSTANCE_LABEL_BYTES: usize = contract::EPHEMERAL_DISCOVERY_INSTANCE_PREFIX.len()
    + (contract::EPHEMERAL_DISCOVERY_INSTANCE_RANDOM_BYTES * 2);

const SERVICE_LABELS: [&[u8]; 3] = [b"_reticulum", b"_udp", b"local"];
const LOCAL_LABEL: &[u8] = b"local";
const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EmbeddedDiscoveryParticipation {
    Inactive,
    Central,
}

pub(crate) type DiscoveryParticipationReceiver =
    Receiver<'static, CriticalSectionRawMutex, EmbeddedDiscoveryParticipation, DISCOVERY_WATCHERS>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UdpServiceDiscoveryConstructionError {
    PublisherCapacityExhausted,
    AddressNotLinkLocal,
}

pub struct UdpServiceDiscoveryPublisher<'a> {
    socket: UdpSocket<'a>,
    stack: Stack<'a>,
    address: Ipv6Addr,
    participation: DiscoveryParticipationReceiver,
    fill_random: fn(&mut [u8]),
}

impl<'a> UdpServiceDiscoveryPublisher<'a> {
    pub fn new<const MEMBERS: usize>(
        socket: UdpSocket<'a>,
        stack: Stack<'a>,
        address: Ipv6Addr,
        status: AutoWifiStatus<MEMBERS>,
        fill_random: fn(&mut [u8]),
    ) -> Result<Self, UdpServiceDiscoveryConstructionError> {
        validate_publication_address(address)?;
        let participation = status.discovery_participation_receiver()?;
        Ok(Self {
            socket,
            stack,
            address,
            participation,
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

            let instance = DiscoveryInstance::fresh(self.fill_random);
            match self.activate().await {
                PublicationActivation::Active => {
                    self.serve(&instance).await;
                    self.deactivate(&instance).await;
                }
                PublicationActivation::Retry => {
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

        let mut receive_buffer = [0u8; UDP_SERVICE_DISCOVERY_SOCKET_BYTES];
        let mut announcement = Ticker::every(ANNOUNCEMENT_INTERVAL);
        loop {
            match select3(
                self.socket.recv_from(&mut receive_buffer),
                announcement.next(),
                self.participation.changed(),
            )
            .await
            {
                Either3::First(Ok((length, _))) => {
                    if query_relevance(&receive_buffer[..length], instance)
                        == QueryRelevance::Relevant
                    {
                        self.publish(&packet[..packet_len], PublicationPurpose::QueryResponse)
                            .await;
                    }
                }
                Either3::First(Err(error)) => {
                    crate::diagnostic_log::debug!(
                        "wifi-auto: embedded UDP DNS-SD query dropped: {error:?}"
                    );
                }
                Either3::Second(()) => {
                    self.publish(&packet[..packet_len], PublicationPurpose::Refresh)
                        .await;
                }
                Either3::Third(_) => return,
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
        self.socket.close();
        self.leave_multicast_group();
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
