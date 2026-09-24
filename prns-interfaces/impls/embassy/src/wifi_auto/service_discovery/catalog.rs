use ::core::net::Ipv6Addr;

use heapless::Vec;

use prns_core::interfaces::wifi_auto as contract;

use super::codec::{
    decode_name, encoded_name, is_udp_service_instance, name_matches, read_u16,
    txt_version_compatibility, visit_resource_records, DiscoveryInstance, DnsName,
    DnsResourceRecord, TxtVersionCompatibility, DNS_CACHE_FLUSH_CLASS_IN, DNS_CLASS_IN,
    DNS_TYPE_AAAA, DNS_TYPE_ANY, DNS_TYPE_PTR, DNS_TYPE_SRV, DNS_TYPE_TXT, SERVICE_LABELS,
};

#[derive(Debug, PartialEq, Eq)]
pub(super) enum CatalogUpdate {
    Applied,
    Malformed,
}

#[derive(Debug, PartialEq, Eq)]
enum RecordCompatibility {
    Awaiting,
    Compatible,
    Incompatible,
}

#[derive(Debug, PartialEq, Eq)]
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

#[derive(Debug, PartialEq, Eq)]
pub(super) struct ResolutionQuery {
    pub(super) name: DnsName,
    pub(super) record_type: u16,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum ServiceResolution {
    Query(ResolutionQuery),
    Resolved,
    Expired,
    Incompatible,
    Missing,
}

pub(super) struct ServiceCatalog<const TARGETS: usize> {
    services: Vec<BrowsedService, TARGETS>,
}

impl<const TARGETS: usize> ServiceCatalog<TARGETS> {
    pub(super) const fn new() -> Self {
        Self {
            services: Vec::new(),
        }
    }

    pub(super) fn apply_response(
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
            Err(_error) => CatalogUpdate::Malformed,
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
            let _insertion = self
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
                service.version = match txt_version_compatibility(record.data) {
                    TxtVersionCompatibility::Compatible => RecordCompatibility::Compatible,
                    TxtVersionCompatibility::Incompatible => RecordCompatibility::Incompatible,
                };
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

    fn prepare_address_updates(
        &mut self,
        packet: &[u8],
    ) -> Result<(), super::codec::DnsPacketError> {
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

    pub(super) fn targets(
        &self,
        now_ms: u64,
        local_address: Ipv6Addr,
    ) -> super::super::EmbeddedDiscoveryTargets<TARGETS> {
        let mut targets = super::super::EmbeddedDiscoveryTargets::new();
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

    pub(super) fn len(&self) -> usize {
        self.services.len()
    }

    pub(super) fn resolution_counts(&self, now_ms: u64) -> (u8, u8, u8, u8) {
        let mut resolved = 0u8;
        let mut pending = 0u8;
        let mut incompatible = 0u8;
        let mut expired = 0u8;
        for index in 0..self.services.len() {
            match self.resolution_at(index, now_ms) {
                ServiceResolution::Resolved => resolved = resolved.saturating_add(1),
                ServiceResolution::Query(_) => pending = pending.saturating_add(1),
                ServiceResolution::Incompatible => incompatible = incompatible.saturating_add(1),
                ServiceResolution::Expired => expired = expired.saturating_add(1),
                ServiceResolution::Missing => {}
            }
        }
        (resolved, pending, incompatible, expired)
    }

    pub(super) fn resolution_at(&self, index: usize, now_ms: u64) -> ServiceResolution {
        let Some(service) = self.services.get(index) else {
            return ServiceResolution::Missing;
        };
        if service.expires_at_ms <= now_ms {
            return ServiceResolution::Expired;
        }
        if service.version == RecordCompatibility::Incompatible {
            return ServiceResolution::Incompatible;
        }
        match (&service.port, &service.host, &service.address) {
            (RecordCompatibility::Incompatible, _, _) => ServiceResolution::Incompatible,
            (RecordCompatibility::Awaiting, _, _) | (_, None, _) => {
                ServiceResolution::Query(ResolutionQuery {
                    name: service.instance.clone(),
                    record_type: DNS_TYPE_ANY,
                })
            }
            (RecordCompatibility::Compatible, Some(host), None) => {
                ServiceResolution::Query(ResolutionQuery {
                    name: host.clone(),
                    record_type: DNS_TYPE_AAAA,
                })
            }
            (RecordCompatibility::Compatible, Some(_host), Some(_address)) => {
                ServiceResolution::Resolved
            }
        }
    }

    pub(super) fn prune(&mut self, now_ms: u64) {
        let mut index = 0;
        while index < self.services.len() {
            if self.services[index].expires_at_ms <= now_ms {
                self.services.swap_remove(index);
            } else {
                index += 1;
            }
        }
    }

    pub(super) fn clear(&mut self) {
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

fn record_expiry(now_ms: u64, ttl_seconds: u32) -> u64 {
    now_ms.saturating_add(u64::from(ttl_seconds).saturating_mul(1_000))
}

#[cfg(test)]
mod tests {
    use super::super::codec::{
        build_publication_packet, decode_name, encoded_name, read_u16, DNS_CORE_RECORD_COUNT,
    };
    use super::*;

    const INSTANCE_RANDOM: [u8; contract::EPHEMERAL_DISCOVERY_INSTANCE_RANDOM_BYTES] =
        [0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef];
    const LINK_LOCAL: Ipv6Addr = Ipv6Addr::new(0xfe80, 0, 0, 0, 0x0212, 0x34ff, 0xfe56, 0x789a);

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
    fn service_resolution_states_name_each_query_outcome() {
        let peer_instance = DiscoveryInstance::from_random_bytes([0x22; 8]);
        let instance_name =
            encoded_name(&peer_instance.service_labels()).expect("instance name fits");
        let host_name = encoded_name(&peer_instance.host_labels()).expect("host name fits");
        let mut catalog = ServiceCatalog::<2>::new();
        catalog
            .services
            .push(BrowsedService::new(instance_name.clone(), 10_000))
            .expect("one service fits");

        assert_eq!(
            catalog.resolution_at(0, 1_000),
            ServiceResolution::Query(ResolutionQuery {
                name: instance_name,
                record_type: DNS_TYPE_ANY,
            })
        );

        catalog.services[0].port = RecordCompatibility::Compatible;
        catalog.services[0].host = Some(host_name.clone());
        assert_eq!(
            catalog.resolution_at(0, 1_000),
            ServiceResolution::Query(ResolutionQuery {
                name: host_name,
                record_type: DNS_TYPE_AAAA,
            })
        );
        catalog.services[0].address = Some(LINK_LOCAL);
        assert_eq!(catalog.resolution_at(0, 1_000), ServiceResolution::Resolved);
        catalog.services[0].version = RecordCompatibility::Incompatible;
        assert_eq!(
            catalog.resolution_at(0, 1_000),
            ServiceResolution::Incompatible
        );
        assert_eq!(catalog.resolution_at(1, 1_000), ServiceResolution::Missing);
        assert_eq!(catalog.resolution_at(0, 10_000), ServiceResolution::Expired);
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
    }

    fn publication_packet(
        instance: &DiscoveryInstance,
        address: Ipv6Addr,
        ttl_seconds: u32,
    ) -> Vec<u8, { super::super::UDP_SERVICE_DISCOVERY_PACKET_BYTES }> {
        let mut packet = [0u8; super::super::UDP_SERVICE_DISCOVERY_PACKET_BYTES];
        let length = build_publication_packet(&mut packet, instance, address, None, ttl_seconds)
            .expect("publication fits");
        Vec::from_slice(&packet[..length]).expect("publication capacity matches output")
    }

    fn reorder_publication_records(
        packet: &[u8],
        order: [usize; DNS_CORE_RECORD_COUNT as usize],
    ) -> Vec<u8, { super::super::UDP_SERVICE_DISCOVERY_PACKET_BYTES }> {
        let mut ranges = Vec::<(usize, usize), { DNS_CORE_RECORD_COUNT as usize }>::new();
        let mut cursor = 12usize;
        for _ in 0..DNS_CORE_RECORD_COUNT {
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
}
