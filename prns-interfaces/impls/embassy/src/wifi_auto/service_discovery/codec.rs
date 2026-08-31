use ::core::net::{Ipv4Addr, Ipv6Addr};
use ::core::ops::Deref;

use heapless::Vec;

use prns_core::interfaces::wifi_auto as contract;

pub(super) const MDNS_PORT: u16 = 5353;
pub(super) const MDNS_HOP_LIMIT: u8 = 255;
pub(super) const MDNS_IPV6_GROUP: Ipv6Addr = Ipv6Addr::new(0xff02, 0, 0, 0, 0, 0, 0, 0x00fb);
/// IPv4 mDNS group — preferred on APs that isolate IPv6 link-local multicast.
pub(super) const MDNS_IPV4_GROUP: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 251);
pub(super) const DNS_CLASS_IN: u16 = 1;
pub(super) const DNS_CACHE_FLUSH_CLASS_IN: u16 = 0x8001;
pub(super) const DNS_TYPE_A: u16 = 1;
pub(super) const DNS_TYPE_AAAA: u16 = 28;
pub(super) const DNS_TYPE_ANY: u16 = 255;
pub(super) const DNS_TYPE_PTR: u16 = 12;
pub(super) const DNS_TYPE_SRV: u16 = 33;
pub(super) const DNS_TYPE_TXT: u16 = 16;
const DNS_RESPONSE_FLAGS: u16 = 0x8400;
/// PTR + SRV + TXT + AAAA; optional A adds one more.
pub(super) const DNS_CORE_RECORD_COUNT: u16 = 4;
const DNS_NAME_CAPACITY: usize = 96;
const DNS_POINTER_HOP_LIMIT: u8 = 8;
const INSTANCE_LABEL_BYTES: usize = contract::EPHEMERAL_DISCOVERY_INSTANCE_PREFIX.len()
    + (contract::EPHEMERAL_DISCOVERY_INSTANCE_RANDOM_BYTES * 2);

pub(super) const SERVICE_LABELS: [&[u8]; 3] = [b"_reticulum", b"_udp", b"local"];
const LOCAL_LABEL: &[u8] = b"local";
const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DnsName {
    encoded: Vec<u8, DNS_NAME_CAPACITY>,
}

impl DnsName {
    const fn new() -> Self {
        Self {
            encoded: Vec::new(),
        }
    }
}

impl Deref for DnsName {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.encoded
    }
}

impl AsRef<[u8]> for DnsName {
    fn as_ref(&self) -> &[u8] {
        &self.encoded
    }
}

#[derive(Debug)]
pub(super) enum PacketBuildError {
    BufferTooSmall,
    LabelTooLong,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum QueryRelevance {
    Relevant,
    Unrelated,
    Response,
    Malformed,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum TxtVersionCompatibility {
    Compatible,
    Incompatible,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct DiscoveryInstance {
    label: [u8; INSTANCE_LABEL_BYTES],
}

impl DiscoveryInstance {
    pub(super) fn fresh(fill_random: fn(&mut [u8])) -> Self {
        let mut random = [0u8; contract::EPHEMERAL_DISCOVERY_INSTANCE_RANDOM_BYTES];
        fill_random(&mut random);
        Self::from_random_bytes(random)
    }

    pub(super) fn from_random_bytes(
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

    pub(super) fn service_labels(&self) -> [&[u8]; 4] {
        [
            &self.label,
            SERVICE_LABELS[0],
            SERVICE_LABELS[1],
            SERVICE_LABELS[2],
        ]
    }

    pub(super) fn host_labels(&self) -> [&[u8]; 2] {
        [&self.label, LOCAL_LABEL]
    }
}

pub(super) fn build_publication_packet(
    output: &mut [u8],
    instance: &DiscoveryInstance,
    ipv6: Ipv6Addr,
    ipv4: Option<Ipv4Addr>,
    ttl_seconds: u32,
) -> Result<usize, PacketBuildError> {
    let answer_count = DNS_CORE_RECORD_COUNT + u16::from(ipv4.is_some());
    let mut writer = PacketWriter::new(output);
    writer.write_u16(0)?;
    writer.write_u16(DNS_RESPONSE_FLAGS)?;
    writer.write_u16(0)?;
    writer.write_u16(answer_count)?;
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
        |writer| writer.write_bytes(&ipv6.octets()),
    )?;
    if let Some(ipv4) = ipv4 {
        writer.write_record(
            &host_labels,
            DNS_TYPE_A,
            DNS_CACHE_FLUSH_CLASS_IN,
            ttl_seconds,
            |writer| writer.write_bytes(&ipv4.octets()),
        )?;
    }
    Ok(writer.len())
}

pub(super) fn query_relevance(packet: &[u8], instance: &DiscoveryInstance) -> QueryRelevance {
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
            && matches!(question_type, DNS_TYPE_A | DNS_TYPE_AAAA | DNS_TYPE_ANY);
        if service_query || instance_query || host_query {
            return QueryRelevance::Relevant;
        }
    }
    QueryRelevance::Unrelated
}

pub(super) fn build_query_packet(
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

pub(super) fn encoded_name<const LABELS: usize>(
    labels: &[&[u8]; LABELS],
) -> Result<DnsName, PacketBuildError> {
    let mut encoded = DnsName::new();
    for label in labels {
        let length = u8::try_from(label.len()).map_err(|_| PacketBuildError::LabelTooLong)?;
        if length > 63 {
            return Err(PacketBuildError::LabelTooLong);
        }
        encoded
            .encoded
            .push(length)
            .map_err(|_| PacketBuildError::BufferTooSmall)?;
        encoded
            .encoded
            .extend_from_slice(label)
            .map_err(|_| PacketBuildError::BufferTooSmall)?;
    }
    encoded
        .encoded
        .push(0)
        .map_err(|_| PacketBuildError::BufferTooSmall)?;
    Ok(encoded)
}

pub(super) struct DnsResourceRecord<'a> {
    pub(super) name: DnsName,
    pub(super) record_type: u16,
    pub(super) record_class: u16,
    pub(super) ttl_seconds: u32,
    pub(super) data_offset: usize,
    pub(super) data_end: usize,
    pub(super) data: &'a [u8],
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum DnsPacketError {
    Truncated,
    LengthOverflow,
    InvalidLabel,
    PointerHopLimitExceeded,
    NameCapacityExceeded,
}

pub(super) fn visit_resource_records(
    packet: &[u8],
    mut visitor: impl FnMut(DnsResourceRecord<'_>),
) -> Result<(), DnsPacketError> {
    if packet.len() < 12 {
        return Err(DnsPacketError::Truncated);
    }
    let question_count = usize::from(read_u16(packet, 4).ok_or(DnsPacketError::Truncated)?);
    let answer_count = usize::from(read_u16(packet, 6).ok_or(DnsPacketError::Truncated)?);
    let authority_count = usize::from(read_u16(packet, 8).ok_or(DnsPacketError::Truncated)?);
    let additional_count = usize::from(read_u16(packet, 10).ok_or(DnsPacketError::Truncated)?);
    let record_count = answer_count
        .checked_add(authority_count)
        .and_then(|count| count.checked_add(additional_count))
        .ok_or(DnsPacketError::LengthOverflow)?;
    let mut cursor = 12usize;
    for _ in 0..question_count {
        let (_, next_cursor) = decode_name(packet, cursor)?;
        cursor = next_cursor
            .checked_add(4)
            .ok_or(DnsPacketError::LengthOverflow)?;
        if cursor > packet.len() {
            return Err(DnsPacketError::Truncated);
        }
    }
    for _ in 0..record_count {
        let (name, next_cursor) = decode_name(packet, cursor)?;
        cursor = next_cursor;
        let record_type = read_u16(packet, cursor).ok_or(DnsPacketError::Truncated)?;
        let record_class = read_u16(
            packet,
            cursor
                .checked_add(2)
                .ok_or(DnsPacketError::LengthOverflow)?,
        )
        .ok_or(DnsPacketError::Truncated)?;
        let ttl_seconds = read_u32(
            packet,
            cursor
                .checked_add(4)
                .ok_or(DnsPacketError::LengthOverflow)?,
        )
        .ok_or(DnsPacketError::Truncated)?;
        let data_length = usize::from(
            read_u16(
                packet,
                cursor
                    .checked_add(8)
                    .ok_or(DnsPacketError::LengthOverflow)?,
            )
            .ok_or(DnsPacketError::Truncated)?,
        );
        let data_offset = cursor
            .checked_add(10)
            .ok_or(DnsPacketError::LengthOverflow)?;
        let data_end = data_offset
            .checked_add(data_length)
            .ok_or(DnsPacketError::LengthOverflow)?;
        let data = packet
            .get(data_offset..data_end)
            .ok_or(DnsPacketError::Truncated)?;
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

pub(super) fn txt_version_compatibility(data: &[u8]) -> TxtVersionCompatibility {
    let mut cursor = 0usize;
    let mut version = None;
    while cursor < data.len() {
        let Some(length) = data.get(cursor).copied().map(usize::from) else {
            return TxtVersionCompatibility::Incompatible;
        };
        cursor += 1;
        let Some(value) = data.get(cursor..cursor.saturating_add(length)) else {
            return TxtVersionCompatibility::Incompatible;
        };
        cursor += length;
        let Some(separator) = value.iter().position(|byte| *byte == b'=') else {
            continue;
        };
        if value[..separator].eq_ignore_ascii_case(contract::TXT_VERSION_KEY.as_bytes()) {
            if version.is_some() {
                return TxtVersionCompatibility::Incompatible;
            }
            version = Some(&value[separator + 1..]);
        }
    }
    match version {
        None => TxtVersionCompatibility::Compatible,
        Some(value) if value == contract::TXT_VERSION_VALUE.as_bytes() => {
            TxtVersionCompatibility::Compatible
        }
        Some(_) => TxtVersionCompatibility::Incompatible,
    }
}

pub(super) fn is_udp_service_instance(name: &DnsName) -> bool {
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
    name.get(service_offset..) == Some(service_name.as_ref())
}

pub(super) fn decode_name(packet: &[u8], start: usize) -> Result<(DnsName, usize), DnsPacketError> {
    let mut decoded = DnsName::new();
    let mut cursor = start;
    let mut next_cursor = None;
    let mut pointer_hops = 0u8;
    loop {
        let Some(length) = packet.get(cursor).copied() else {
            return Err(DnsPacketError::Truncated);
        };
        if length == 0 {
            let end = next_cursor.unwrap_or(cursor + 1);
            decoded
                .encoded
                .push(0)
                .map_err(|_| DnsPacketError::NameCapacityExceeded)?;
            return Ok((decoded, end));
        }
        if length & 0xc0 == 0xc0 {
            let Some(second) = packet.get(cursor + 1).copied() else {
                return Err(DnsPacketError::Truncated);
            };
            if next_cursor.is_none() {
                next_cursor = Some(cursor + 2);
            }
            pointer_hops = pointer_hops
                .checked_add(1)
                .ok_or(DnsPacketError::PointerHopLimitExceeded)?;
            if pointer_hops > DNS_POINTER_HOP_LIMIT {
                return Err(DnsPacketError::PointerHopLimitExceeded);
            }
            cursor = (usize::from(length & 0x3f) << 8) | usize::from(second);
            continue;
        }
        if length > 63 || length & 0xc0 != 0 {
            return Err(DnsPacketError::InvalidLabel);
        }
        let label_start = cursor + 1;
        let label_end = label_start
            .checked_add(usize::from(length))
            .ok_or(DnsPacketError::LengthOverflow)?;
        let Some(label) = packet.get(label_start..label_end) else {
            return Err(DnsPacketError::Truncated);
        };
        decoded
            .encoded
            .push(length)
            .map_err(|_| DnsPacketError::NameCapacityExceeded)?;
        decoded
            .encoded
            .extend_from_slice(label)
            .map_err(|_| DnsPacketError::NameCapacityExceeded)?;
        cursor = label_end;
    }
}

pub(super) fn name_matches<const LABELS: usize>(
    encoded: &DnsName,
    labels: &[&[u8]; LABELS],
) -> bool {
    let Ok(expected) = encoded_name(labels) else {
        return false;
    };
    encoded.eq_ignore_ascii_case(&expected)
}

pub(super) fn read_u16(packet: &[u8], offset: usize) -> Option<u16> {
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
        let mut packet = [0u8; super::super::UDP_SERVICE_DISCOVERY_PACKET_BYTES];
        let length = build_publication_packet(
            &mut packet,
            &instance,
            LINK_LOCAL,
            None,
            super::super::PUBLICATION_TTL_SECONDS,
        )
        .expect("the fixed publication capacity fits the complete record set");

        assert!(length <= super::super::UDP_SERVICE_DISCOVERY_PACKET_BYTES);
        assert_eq!(read_u16(&packet[..length], 6), Some(DNS_CORE_RECORD_COUNT));
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
    fn publication_includes_optional_ipv4_a_record() {
        let instance = DiscoveryInstance::from_random_bytes(INSTANCE_RANDOM);
        let ipv4 = Ipv4Addr::new(192, 168, 1, 127);
        let mut packet = [0u8; super::super::UDP_SERVICE_DISCOVERY_PACKET_BYTES];
        let length = build_publication_packet(
            &mut packet,
            &instance,
            LINK_LOCAL,
            Some(ipv4),
            super::super::PUBLICATION_TTL_SECONDS,
        )
        .expect("A + AAAA publication fits");
        assert_eq!(
            read_u16(&packet[..length], 6),
            Some(DNS_CORE_RECORD_COUNT + 1)
        );
        assert!(packet[..length]
            .windows(ipv4.octets().len())
            .any(|window| window == ipv4.octets()));
        assert!(packet[..length]
            .windows(LINK_LOCAL.octets().len())
            .any(|window| window == LINK_LOCAL.octets()));
    }

    #[test]
    fn goodbye_uses_zero_ttl_for_every_record() {
        let instance = DiscoveryInstance::from_random_bytes(INSTANCE_RANDOM);
        let mut packet = [0u8; super::super::UDP_SERVICE_DISCOVERY_PACKET_BYTES];
        let length = build_publication_packet(&mut packet, &instance, LINK_LOCAL, None, 0)
            .expect("the fixed publication capacity fits the goodbye record set");
        let mut cursor = 12;
        for _ in 0..DNS_CORE_RECORD_COUNT {
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

        let mut response = [0u8; super::super::UDP_SERVICE_DISCOVERY_PACKET_BYTES];
        let response_len = build_publication_packet(
            &mut response,
            &instance,
            LINK_LOCAL,
            None,
            super::super::PUBLICATION_TTL_SECONDS,
        )
        .expect("publication fits");
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
    fn dns_name_enforces_label_and_total_bounds() {
        let oversized_label = [b'a'; 64];
        assert!(matches!(
            encoded_name(&[oversized_label.as_slice()]),
            Err(PacketBuildError::LabelTooLong)
        ));

        let maximum_label = [b'a'; 63];
        assert!(matches!(
            encoded_name(&[maximum_label.as_slice(), maximum_label.as_slice()]),
            Err(PacketBuildError::BufferTooSmall)
        ));
    }

    #[test]
    fn txt_version_is_explicitly_classified() {
        assert_eq!(
            txt_version_compatibility(&[]),
            TxtVersionCompatibility::Compatible
        );
        assert_eq!(
            txt_version_compatibility(&[3, b'v', b'=', b'1']),
            TxtVersionCompatibility::Compatible
        );
        assert_eq!(
            txt_version_compatibility(&[3, b'v', b'=', b'9']),
            TxtVersionCompatibility::Incompatible
        );
        assert_eq!(
            txt_version_compatibility(&[4, b'v', b'=', b'1']),
            TxtVersionCompatibility::Incompatible
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
