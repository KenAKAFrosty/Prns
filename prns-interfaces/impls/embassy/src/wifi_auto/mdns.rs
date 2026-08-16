use embassy_futures::select::{select3, Either3};
use embassy_net::udp::UdpSocket;
use embassy_net::{IpAddress, IpEndpoint, Ipv4Address, Ipv6Address, Stack};
use embassy_time::{Duration, Ticker, Timer};

use prns_core::interfaces::wifi_auto as contract;
use prns_core::interfaces::MacAddress;

const MDNS_PORT: u16 = 5353;
const MDNS_TTL_SECS: u32 = 120;
const ANNOUNCE_INTERVAL: Duration = Duration::from_secs(45);
const BIND_RETRY: Duration = Duration::from_millis(500);
const CLASS_IN: u16 = 1;
const CLASS_IN_FLUSH: u16 = 0x8001;
const TYPE_A: u16 = 1;
const TYPE_PTR: u16 = 12;
const TYPE_TXT: u16 = 16;
const TYPE_AAAA: u16 = 28;
const TYPE_SRV: u16 = 33;
const TYPE_ANY: u16 = 255;

const MDNS_GROUP: Ipv4Address = Ipv4Address::new(224, 0, 0, 251);
const MDNS_GROUP_V6: Ipv6Address = Ipv6Address::new(0xff02, 0, 0, 0, 0, 0, 0, 0xfb);
const SERVICE_LABELS: [&[u8]; 3] = [b"_reticulum", b"_udp", b"local"];
const SERVICES_LABELS: [&[u8]; 4] = [b"_services", b"_dns-sd", b"_udp", b"local"];
const TXT_VALUE: &[u8] = b"v=1";

/// Hostname-safe DNS-SD instance, matching the host `prns-` + 8 hex digits shape.
#[must_use]
pub fn instance_label(mac: [u8; 6]) -> heapless::String<16> {
    let mut label = heapless::String::new();
    let _ = core::fmt::write(
        &mut label,
        format_args!(
            "prns-{:02x}{:02x}{:02x}{:02x}",
            mac[2], mac[3], mac[4], mac[5]
        ),
    );
    label
}

pub async fn advertise_ipv4(stack: Stack<'_>, mut socket: UdpSocket<'_>, mac: [u8; 6]) -> ! {
    let instance = instance_label(mac);
    let ipv6 = contract::link_local_from_mac(MacAddress::new(mac));
    socket.set_hop_limit(Some(255));
    loop {
        if socket.bind(MDNS_PORT).is_ok() {
            break;
        }
        Timer::after(BIND_RETRY).await;
    }
    let mut rx = [0u8; 512];
    let mut tx = [0u8; 512];
    loop {
        stack.wait_link_up().await;
        let ipv4 = stack.config_v4().map(|config| config.address.address());
        let joined_v4 = stack
            .join_multicast_group(IpAddress::Ipv4(MDNS_GROUP))
            .is_ok();
        let joined_v6 = stack
            .join_multicast_group(IpAddress::Ipv6(MDNS_GROUP_V6))
            .is_ok();
        if !joined_v4 && !joined_v6 {
            crate::diagnostic_log::warn!("wifi-auto: mDNS join 224.0.0.251 and ff02::fb failed");
            Timer::after(Duration::from_secs(2)).await;
            continue;
        }
        crate::diagnostic_log::info!(
            "wifi-auto: advertising {} on {} as {} (v4={:?})",
            contract::MDNS_SERVICE_TYPE,
            ipv6,
            instance.as_str(),
            ipv4
        );
        announce(&socket, &mut tx, instance.as_bytes(), ipv4, ipv6).await;
        let mut announce_at = Ticker::every(ANNOUNCE_INTERVAL);
        let mut ipv4 = ipv4;
        loop {
            match select3(
                socket.recv_from(&mut rx),
                announce_at.next(),
                stack.wait_link_down(),
            )
            .await
            {
                Either3::First(Ok((len, meta))) => {
                    let unicast = meta.endpoint.port != MDNS_PORT;
                    if let Some(reply_len) = build_query_response(
                        &rx[..len],
                        instance.as_bytes(),
                        ipv4,
                        ipv6,
                        unicast,
                        &mut tx,
                    ) {
                        let dest = if unicast {
                            meta.endpoint
                        } else {
                            multicast_endpoint_for(meta.endpoint.addr)
                        };
                        if let Err(error) = socket.send_to(&tx[..reply_len], dest).await {
                            crate::diagnostic_log::debug!(
                                "wifi-auto: mDNS reply failed: {error:?}"
                            );
                        }
                    }
                }
                Either3::First(Err(error)) => {
                    crate::diagnostic_log::debug!("wifi-auto: mDNS recv failed: {error:?}");
                }
                Either3::Second(()) => {
                    announce(&socket, &mut tx, instance.as_bytes(), ipv4, ipv6).await;
                }
                Either3::Third(()) => {
                    crate::diagnostic_log::info!(
                        "wifi-auto: mDNS link down; keeping multicast groups"
                    );
                    stack.wait_link_up().await;
                    crate::diagnostic_log::info!("wifi-auto: mDNS link up again");
                }
            }
            let next_v4 = stack.config_v4().map(|next| next.address.address());
            if next_v4 != ipv4 {
                ipv4 = next_v4;
                crate::diagnostic_log::info!(
                    "wifi-auto: mDNS A record now {:?} as {}",
                    ipv4,
                    instance.as_str()
                );
                announce(&socket, &mut tx, instance.as_bytes(), ipv4, ipv6).await;
            }
        }
    }
}

fn multicast_endpoint_for(addr: IpAddress) -> IpEndpoint {
    match addr {
        IpAddress::Ipv6(_) => IpEndpoint::new(IpAddress::Ipv6(MDNS_GROUP_V6), MDNS_PORT),
        IpAddress::Ipv4(_) => IpEndpoint::new(IpAddress::Ipv4(MDNS_GROUP), MDNS_PORT),
    }
}

async fn announce(
    socket: &UdpSocket<'_>,
    tx: &mut [u8],
    instance: &[u8],
    ipv4: Option<Ipv4Address>,
    ipv6: core::net::Ipv6Addr,
) {
    let Some(len) = build_announcement(instance, ipv4, ipv6, tx) else {
        return;
    };
    if ipv4.is_some() {
        if let Err(error) = socket
            .send_to(&tx[..len], IpEndpoint::new(IpAddress::Ipv4(MDNS_GROUP), MDNS_PORT))
            .await
        {
            crate::diagnostic_log::debug!("wifi-auto: IPv4 mDNS announce failed: {error:?}");
        }
    }
    if let Err(error) = socket
        .send_to(
            &tx[..len],
            IpEndpoint::new(IpAddress::Ipv6(MDNS_GROUP_V6), MDNS_PORT),
        )
        .await
    {
        crate::diagnostic_log::debug!("wifi-auto: IPv6 mDNS announce failed: {error:?}");
    }
}

#[derive(Clone, Copy)]
struct AnswerSet {
    services_ptr: bool,
    service_ptr: bool,
    srv: bool,
    txt: bool,
    host_a: bool,
    host_aaaa: bool,
}

impl AnswerSet {
    const fn empty() -> Self {
        Self {
            services_ptr: false,
            service_ptr: false,
            srv: false,
            txt: false,
            host_a: false,
            host_aaaa: false,
        }
    }

    fn any(self) -> bool {
        self.services_ptr
            || self.service_ptr
            || self.srv
            || self.txt
            || self.host_a
            || self.host_aaaa
    }
}

fn build_announcement(
    instance: &[u8],
    ipv4: Option<Ipv4Address>,
    ipv6: core::net::Ipv6Addr,
    out: &mut [u8],
) -> Option<usize> {
    write_records(
        0,
        AnswerSet {
            services_ptr: true,
            service_ptr: true,
            srv: true,
            txt: true,
            host_a: ipv4.is_some(),
            host_aaaa: true,
        },
        instance,
        ipv4,
        ipv6,
        out,
    )
}

fn build_query_response(
    query: &[u8],
    instance: &[u8],
    ipv4: Option<Ipv4Address>,
    ipv6: core::net::Ipv6Addr,
    _unicast: bool,
    out: &mut [u8],
) -> Option<usize> {
    let mut answers = parse_questions(query, instance)?;
    if ipv4.is_none() {
        answers.host_a = false;
    }
    if !answers.any() {
        return None;
    }
    let id = u16::from_be_bytes(query.get(..2)?.try_into().ok()?);
    write_records(id, answers, instance, ipv4, ipv6, out)
}

fn parse_questions(query: &[u8], instance: &[u8]) -> Option<AnswerSet> {
    if query.len() < 12 || query[2] & 0x80 != 0 {
        return None;
    }
    let qdcount = u16::from_be_bytes([query[4], query[5]]);
    let mut offset = 12usize;
    let mut answers = AnswerSet::empty();
    for _ in 0..qdcount {
        let (qname, next) = parse_name(query, offset)?;
        offset = next;
        let qtype = u16::from_be_bytes(query.get(offset..offset + 2)?.try_into().ok()?);
        let qclass = u16::from_be_bytes(query.get(offset + 2..offset + 4)?.try_into().ok()?);
        offset += 4;
        let class = qclass & 0x7fff;
        if class != CLASS_IN && class != TYPE_ANY {
            continue;
        }
        match classify_qname(&qname, instance) {
            QnameKind::Services if matches!(qtype, TYPE_PTR | TYPE_ANY) => {
                answers.services_ptr = true;
            }
            QnameKind::Service if matches!(qtype, TYPE_PTR | TYPE_ANY) => {
                answers.service_ptr = true;
            }
            QnameKind::Instance if matches!(qtype, TYPE_SRV | TYPE_ANY) => answers.srv = true,
            QnameKind::Instance if qtype == TYPE_TXT => answers.txt = true,
            QnameKind::Host if matches!(qtype, TYPE_A | TYPE_ANY) => answers.host_a = true,
            QnameKind::Host if qtype == TYPE_AAAA => answers.host_aaaa = true,
            _ => {}
        }
        if qtype == TYPE_ANY {
            match classify_qname(&qname, instance) {
                QnameKind::Instance => answers.txt = true,
                QnameKind::Host => {
                    answers.host_a = true;
                    answers.host_aaaa = true;
                }
                _ => {}
            }
        }
    }
    if answers.service_ptr {
        answers.srv = true;
        answers.txt = true;
        answers.host_a = true;
        answers.host_aaaa = true;
    }
    Some(answers)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum QnameKind {
    Services,
    Service,
    Instance,
    Host,
    Other,
}

fn classify_qname(qname: &Qname, instance: &[u8]) -> QnameKind {
    if qname.eq_labels(&SERVICES_LABELS) {
        QnameKind::Services
    } else if qname.eq_labels(&SERVICE_LABELS) {
        QnameKind::Service
    } else if qname.len() == 4
        && qname.label_eq(0, instance)
        && qname.label_eq(1, b"_reticulum")
        && qname.label_eq(2, b"_udp")
        && qname.label_eq(3, b"local")
    {
        QnameKind::Instance
    } else if qname.len() == 2 && qname.label_eq(0, instance) && qname.label_eq(1, b"local") {
        QnameKind::Host
    } else {
        QnameKind::Other
    }
}

struct Qname {
    labels: heapless::Vec<heapless::Vec<u8, 32>, 6>,
}

impl Qname {
    fn len(&self) -> usize {
        self.labels.len()
    }

    fn label_eq(&self, index: usize, expected: &[u8]) -> bool {
        self.labels
            .get(index)
            .is_some_and(|label| eq_ignore_ascii_case(label, expected))
    }

    fn eq_labels(&self, expected: &[&[u8]]) -> bool {
        self.labels.len() == expected.len()
            && self
                .labels
                .iter()
                .zip(expected.iter())
                .all(|(label, want)| eq_ignore_ascii_case(label, want))
    }
}

fn eq_ignore_ascii_case(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right.iter())
            .all(|(a, b)| a.eq_ignore_ascii_case(b))
}

fn parse_name(message: &[u8], mut offset: usize) -> Option<(Qname, usize)> {
    let mut labels = heapless::Vec::new();
    let mut end = None;
    let mut jumps = 0u8;
    loop {
        let len = *message.get(offset)? as usize;
        if len == 0 {
            if end.is_none() {
                end = Some(offset + 1);
            }
            break;
        }
        if len & 0xc0 == 0xc0 {
            let pointer = ((len & 0x3f) << 8) | (*message.get(offset + 1)? as usize);
            if end.is_none() {
                end = Some(offset + 2);
            }
            offset = pointer;
            jumps = jumps.saturating_add(1);
            if jumps > 8 {
                return None;
            }
            continue;
        }
        if len & 0xc0 != 0 || len > 32 {
            return None;
        }
        offset += 1;
        let label = message.get(offset..offset + len)?;
        let mut stored = heapless::Vec::new();
        stored.extend_from_slice(label).ok()?;
        labels.push(stored).ok()?;
        offset += len;
        if labels.len() > 5 {
            return None;
        }
    }
    Some((Qname { labels }, end?))
}

fn write_records(
    id: u16,
    answers: AnswerSet,
    instance: &[u8],
    ipv4: Option<Ipv4Address>,
    ipv6: core::net::Ipv6Addr,
    out: &mut [u8],
) -> Option<usize> {
    if out.len() < 12 {
        return None;
    }
    out[..12].fill(0);
    out[..2].copy_from_slice(&id.to_be_bytes());
    out[2] = 0x84; // QR + AA
    let mut pos = 12usize;
    let mut ancount = 0u16;
    if answers.services_ptr {
        write_ptr(
            out,
            &mut pos,
            &SERVICES_LABELS,
            &SERVICE_LABELS,
            CLASS_IN,
        )?;
        ancount += 1;
    }
    if answers.service_ptr {
        let instance_service = [instance, b"_reticulum", b"_udp", b"local"];
        write_ptr(out, &mut pos, &SERVICE_LABELS, &instance_service, CLASS_IN)?;
        ancount += 1;
    }
    if answers.srv {
        write_srv(out, &mut pos, instance)?;
        ancount += 1;
    }
    if answers.txt {
        write_txt(out, &mut pos, instance)?;
        ancount += 1;
    }
    if answers.host_a {
        write_a(out, &mut pos, instance, ipv4?)?;
        ancount += 1;
    }
    if answers.host_aaaa {
        write_aaaa(out, &mut pos, instance, ipv6)?;
        ancount += 1;
    }
    out[6..8].copy_from_slice(&ancount.to_be_bytes());
    Some(pos)
}

fn write_name(out: &mut [u8], pos: &mut usize, labels: &[&[u8]]) -> Option<()> {
    for label in labels {
        if label.len() > 63 || *pos + 1 + label.len() + 1 > out.len() {
            return None;
        }
        out[*pos] = label.len() as u8;
        *pos += 1;
        out[*pos..*pos + label.len()].copy_from_slice(label);
        *pos += label.len();
    }
    out.get_mut(*pos).map(|byte| *byte = 0)?;
    *pos += 1;
    Some(())
}

fn write_rr_header(
    out: &mut [u8],
    pos: &mut usize,
    name: &[&[u8]],
    rrtype: u16,
    class: u16,
    rdata_len: u16,
) -> Option<()> {
    write_name(out, pos, name)?;
    write_u16(out, pos, rrtype)?;
    write_u16(out, pos, class)?;
    write_u32(out, pos, MDNS_TTL_SECS)?;
    write_u16(out, pos, rdata_len)
}

fn write_ptr(
    out: &mut [u8],
    pos: &mut usize,
    owner: &[&[u8]],
    target: &[&[u8]],
    class: u16,
) -> Option<()> {
    let rdata_len = name_len(target)?;
    write_rr_header(out, pos, owner, TYPE_PTR, class, rdata_len)?;
    write_name(out, pos, target)
}

fn write_srv(out: &mut [u8], pos: &mut usize, instance: &[u8]) -> Option<()> {
    let owner = [instance, b"_reticulum", b"_udp", b"local"];
    let target = [instance, b"local"];
    let rdata_len = 6u16.checked_add(name_len(&target)?)?;
    write_rr_header(out, pos, &owner, TYPE_SRV, CLASS_IN_FLUSH, rdata_len)?;
    write_u16(out, pos, 0)?;
    write_u16(out, pos, 0)?;
    write_u16(out, pos, contract::MDNS_SERVICE_PORT)?;
    write_name(out, pos, &target)
}

fn write_txt(out: &mut [u8], pos: &mut usize, instance: &[u8]) -> Option<()> {
    let owner = [instance, b"_reticulum", b"_udp", b"local"];
    let rdata_len = 1u16.checked_add(TXT_VALUE.len() as u16)?;
    write_rr_header(out, pos, &owner, TYPE_TXT, CLASS_IN_FLUSH, rdata_len)?;
    if *pos + usize::from(rdata_len) > out.len() {
        return None;
    }
    out[*pos] = TXT_VALUE.len() as u8;
    *pos += 1;
    out[*pos..*pos + TXT_VALUE.len()].copy_from_slice(TXT_VALUE);
    *pos += TXT_VALUE.len();
    Some(())
}

fn write_a(out: &mut [u8], pos: &mut usize, instance: &[u8], ipv4: Ipv4Address) -> Option<()> {
    let owner = [instance, b"local"];
    write_rr_header(out, pos, &owner, TYPE_A, CLASS_IN_FLUSH, 4)?;
    let octets = ipv4.octets();
    out.get_mut(*pos..*pos + 4)?.copy_from_slice(&octets);
    *pos += 4;
    Some(())
}

fn write_aaaa(
    out: &mut [u8],
    pos: &mut usize,
    instance: &[u8],
    ipv6: core::net::Ipv6Addr,
) -> Option<()> {
    let owner = [instance, b"local"];
    write_rr_header(out, pos, &owner, TYPE_AAAA, CLASS_IN_FLUSH, 16)?;
    out.get_mut(*pos..*pos + 16)?
        .copy_from_slice(&ipv6.octets());
    *pos += 16;
    Some(())
}

fn name_len(labels: &[&[u8]]) -> Option<u16> {
    let mut len = 1u16;
    for label in labels {
        len = len.checked_add(1)?.checked_add(label.len() as u16)?;
    }
    Some(len)
}

fn write_u16(out: &mut [u8], pos: &mut usize, value: u16) -> Option<()> {
    out.get_mut(*pos..*pos + 2)?
        .copy_from_slice(&value.to_be_bytes());
    *pos += 2;
    Some(())
}

fn write_u32(out: &mut [u8], pos: &mut usize, value: u32) -> Option<()> {
    out.get_mut(*pos..*pos + 4)?
        .copy_from_slice(&value.to_be_bytes());
    *pos += 4;
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use prns_core::interfaces::MacAddress;

    fn query(labels: &[&[u8]], qtype: u16) -> heapless::Vec<u8, 128> {
        let mut packet = heapless::Vec::new();
        packet.extend_from_slice(&[0, 7, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0]).ok();
        for label in labels {
            packet.push(label.len() as u8).ok();
            packet.extend_from_slice(label).ok();
        }
        packet.push(0).ok();
        packet.extend_from_slice(&qtype.to_be_bytes()).ok();
        packet.extend_from_slice(&CLASS_IN.to_be_bytes()).ok();
        packet
    }

    #[test]
    fn instance_labels_are_hostname_safe() {
        let label = instance_label([0x02, 0x00, 0xab, 0xcd, 0xef, 0x11]);
        assert_eq!(label.as_str(), "prns-abcdef11");
    }

    #[test]
    fn a_reticulum_ptr_query_advertises_udp_42671_and_the_link_local_aaaa() {
        let mac = [0; 6];
        let instance = instance_label(mac);
        let packet = query(&SERVICE_LABELS, TYPE_PTR);
        let ipv4 = Ipv4Address::new(192, 168, 1, 40);
        let ipv6 = contract::link_local_from_mac(MacAddress::new(mac));
        let mut out = [0u8; 512];
        let len = build_query_response(
            &packet,
            instance.as_bytes(),
            Some(ipv4),
            ipv6,
            false,
            &mut out,
        )
        .expect("PTR for the service type is answered");
        let body = &out[..len];
        assert_eq!(body[2] & 0x84, 0x84);
        assert!(body.windows(2).any(|pair| pair == 42671u16.to_be_bytes()));
        assert!(body.windows(16).any(|octets| octets == ipv6.octets()));
        assert!(body.windows(instance.len()).any(|w| w == instance.as_bytes()));
        assert!(body.windows(b"_udp".len()).any(|w| w == b"_udp"));
    }

    #[test]
    fn unrelated_queries_are_ignored() {
        let packet = query(&[b"example", b"com"], TYPE_A);
        let mut out = [0u8; 512];
        assert!(build_query_response(
            &packet,
            b"prns-test",
            Some(Ipv4Address::new(10, 0, 0, 2)),
            core::net::Ipv6Addr::UNSPECIFIED,
            false,
            &mut out
        )
        .is_none());
    }

    #[test]
    fn unsolicited_announcements_include_the_service_ptr() {
        let mut out = [0u8; 512];
        let len = build_announcement(
            b"prns-test",
            Some(Ipv4Address::new(10, 1, 2, 3)),
            core::net::Ipv6Addr::UNSPECIFIED,
            &mut out,
        )
        .expect("announcement fits");
        assert!(out[..len]
            .windows(b"_reticulum".len())
            .any(|w| w == b"_reticulum"));
        assert!(u16::from_be_bytes([out[6], out[7]]) >= 3);
    }

    #[test]
    fn announcements_without_dhcp_still_carry_aaaa() {
        let mac = [0x02, 0x00, 0xab, 0xcd, 0xef, 0x11];
        let ipv6 = contract::link_local_from_mac(MacAddress::new(mac));
        let mut out = [0u8; 512];
        let len = build_announcement(b"prns-abcdef11", None, ipv6, &mut out)
            .expect("AAAA-only announcement fits");
        assert!(out[..len].windows(16).any(|octets| octets == ipv6.octets()));
        assert!(!out[..len].windows(4).any(|octets| octets == [10, 1, 2, 3]));
    }
}
