use prns_core::interfaces::channel_rendezvous::{ChannelCommitment, WifiChannel};
use prns_core::interfaces::MacAddress;

const BONJOUR_PTR_QUERY: &[u8] = &[
    0x05, 0x5f, 0x70, 0x72, 0x6e, 0x73, 0xc0, 0x0c, 0x00, 0x0c, 0x01,
];
const BONJOUR_PTR_RESPONSE: &[u8] = &[0x04, 0x50, 0x72, 0x6e, 0x73, 0xc0, 0x27];
const SD_PTR_QUERY_TLV: &[u8] = &[
    0x0d, 0x00, 0x01, 0x01, 0x05, 0x5f, 0x70, 0x72, 0x6e, 0x73, 0xc0, 0x0c, 0x00, 0x0c, 0x01,
];
const SERVICE_MARKER_HEX: &str = "5f70726e73";
const QUERY_TYPE_MARKER_HEX: &str = "c00c000c01";
const BROADCAST_ADDRESS: &str = "00:00:00:00:00:00";

pub fn hex(bytes: &[u8]) -> String {
    let mut rendered = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        rendered.push(char::from_digit((byte >> 4).into(), 16).unwrap_or('0'));
        rendered.push(char::from_digit((byte & 0x0f).into(), 16).unwrap_or('0'));
    }
    rendered
}

pub fn advertise_service_command() -> String {
    format!(
        "P2P_SERVICE_ADD bonjour {} {}",
        hex(BONJOUR_PTR_QUERY),
        hex(BONJOUR_PTR_RESPONSE)
    )
}

pub fn discover_service_command() -> String {
    format!(
        "P2P_SERV_DISC_REQ {BROADCAST_ADDRESS} {}",
        hex(SD_PTR_QUERY_TLV)
    )
}

pub fn positional(payload: &str) -> Option<&str> {
    payload.split_whitespace().next()
}

pub fn field<'a>(payload: &'a str, key: &str) -> Option<&'a str> {
    payload.split_whitespace().find_map(|token| {
        token
            .strip_prefix(key)?
            .strip_prefix('=')
            .map(|value| value.trim_matches(['"', '\'']))
    })
}

pub fn parse_mac(rendered: &str) -> Option<MacAddress> {
    let mut octets = [0u8; 6];
    let mut parts = rendered.split(':');
    for octet in &mut octets {
        *octet = u8::from_str_radix(parts.next()?, 16).ok()?;
    }
    if parts.next().is_some() {
        return None;
    }
    Some(MacAddress::new(octets))
}

pub struct GroupStarted {
    pub interface: String,
    pub is_owner: bool,
    pub ssid: String,
}

pub fn parse_group_started(payload: &str) -> Option<GroupStarted> {
    let mut tokens = payload.split_whitespace();
    let interface = tokens.next()?.to_owned();
    let is_owner = tokens.next()? == "GO";
    let ssid = field(payload, "ssid")?.to_owned();
    Some(GroupStarted {
        interface,
        is_owner,
        ssid,
    })
}

pub fn parse_peer_address(payload: &str) -> Option<MacAddress> {
    field(payload, "p2p_dev_addr")
        .or_else(|| positional(payload))
        .and_then(parse_mac)
}

pub fn service_response_is_prns(payload: &str) -> bool {
    payload
        .split_whitespace()
        .last()
        .is_some_and(|tlvs| tlvs.contains(SERVICE_MARKER_HEX))
}

pub fn parse_status_ssid(status: &str) -> Option<String> {
    status
        .lines()
        .find_map(|line| line.strip_prefix("ssid="))
        .map(str::to_owned)
}

pub fn parse_status_commitment(status: &str) -> ChannelCommitment {
    let associated = status
        .lines()
        .find_map(|line| line.strip_prefix("wpa_state="))
        .is_some_and(|state| state == "COMPLETED");
    let channel = status
        .lines()
        .find_map(|line| line.strip_prefix("freq="))
        .and_then(|mhz| mhz.parse::<u16>().ok())
        .and_then(WifiChannel::new);
    match (associated, channel) {
        (true, Some(channel)) => ChannelCommitment::Anchored(channel),
        _ => ChannelCommitment::Free,
    }
}

pub fn advertise_offer_command(ssid: &str) -> String {
    let mut rdata = Vec::with_capacity(ssid.len() + 3);
    rdata.push(ssid.len() as u8);
    rdata.extend_from_slice(ssid.as_bytes());
    rdata.push(0xc0);
    rdata.push(0x27);
    format!(
        "P2P_SERVICE_ADD bonjour {} {}",
        hex(BONJOUR_PTR_QUERY),
        hex(&rdata)
    )
}

pub fn parse_offer_ssid(tlvs: &str) -> Option<String> {
    let rdata = tlvs.split_once(QUERY_TYPE_MARKER_HEX)?.1;
    let length = usize::from_str_radix(rdata.get(0..2)?, 16).ok()?;
    let label = rdata.get(2..2 + length * 2)?;
    String::from_utf8(decode_hex(label)?).ok()
}

fn decode_hex(hex: &str) -> Option<Vec<u8>> {
    if !hex.len().is_multiple_of(2) {
        return None;
    }
    (0..hex.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&hex[index..index + 2], 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_render_as_lowercase_hex() {
        assert_eq!(hex(&[0x05, 0x5f, 0x00, 0xff]), "055f00ff");
    }

    #[test]
    fn the_advertise_command_carries_the_prns_bonjour_records() {
        assert_eq!(
            advertise_service_command(),
            "P2P_SERVICE_ADD bonjour 055f70726e73c00c000c01 0450726e73c027"
        );
    }

    #[test]
    fn a_group_started_line_yields_interface_role_and_ssid() {
        let started = parse_group_started(
            "p2p-wlan0-0 GO ssid=\"DIRECT-45\" freq=2412 go_dev_addr=42:00:00:00:00:00",
        )
        .expect("a GO line parses");
        assert_eq!(started.interface, "p2p-wlan0-0");
        assert!(started.is_owner);
        assert_eq!(started.ssid, "DIRECT-45");

        let client = parse_group_started("p2p-wlan0-0 client ssid=\"DIRECT-45\" freq=2412")
            .expect("a client line parses");
        assert!(!client.is_owner);
    }

    #[test]
    fn a_device_found_line_prefers_the_p2p_device_address() {
        let address = parse_peer_address(
            "aa:bb:cc:dd:ee:ff p2p_dev_addr=42:00:00:00:00:00 name='Prns' dev_capab=0x25",
        )
        .expect("an address parses");
        assert_eq!(address, MacAddress::new([0x42, 0, 0, 0, 0, 0]));
    }

    #[test]
    fn a_service_response_is_recognized_by_its_prns_marker() {
        assert!(service_response_is_prns(
            "42:00:00:00:00:00 1 0b005f70726e73c00c000c01"
        ));
        assert!(!service_response_is_prns("42:00:00:00:00:00 1 0b00abcdef"));
    }

    #[test]
    fn the_ssid_is_read_out_of_a_status_block() {
        let status = "bssid=06:00:00:00:00:00\nfreq=2412\nssid=DIRECT-45\nmode=P2P GO\n";
        assert_eq!(parse_status_ssid(status).as_deref(), Some("DIRECT-45"));
    }

    #[test]
    fn an_associated_station_anchors_to_its_channel() {
        let two_point_four = "bssid=aa:bb:cc:dd:ee:ff\nfreq=2412\nssid=Home\nwpa_state=COMPLETED\n";
        assert_eq!(
            parse_status_commitment(two_point_four),
            ChannelCommitment::Anchored(WifiChannel::new(2412).unwrap())
        );
        let dfs = "freq=5300\nssid=Home\nwpa_state=COMPLETED\n";
        assert_eq!(
            parse_status_commitment(dfs),
            ChannelCommitment::Anchored(WifiChannel::new(5300).unwrap())
        );
    }

    #[test]
    fn a_group_owner_or_unassociated_station_is_free() {
        let group_owner = "bssid=06:00:00:00:00:00\nfreq=2412\nssid=DIRECT-45\nmode=P2P GO\n";
        assert_eq!(
            parse_status_commitment(group_owner),
            ChannelCommitment::Free
        );
        let scanning = "wpa_state=SCANNING\nfreq=2412\n";
        assert_eq!(parse_status_commitment(scanning), ChannelCommitment::Free);
        let disconnected = "wpa_state=DISCONNECTED\n";
        assert_eq!(
            parse_status_commitment(disconnected),
            ChannelCommitment::Free
        );
        let out_of_band = "wpa_state=COMPLETED\nfreq=2600\n";
        assert_eq!(
            parse_status_commitment(out_of_band),
            ChannelCommitment::Free
        );
    }

    #[test]
    fn the_offer_command_encodes_the_ssid_as_the_instance_label() {
        assert_eq!(
            advertise_offer_command("DIRECT-Prns-bench1"),
            "P2P_SERVICE_ADD bonjour 055f70726e73c00c000c01 \
             124449524543542d50726e732d62656e636831c027"
        );
    }

    #[test]
    fn the_offer_ssid_is_read_back_out_of_a_service_response() {
        let hosting = "055f70726e73c00c000c01124449524543542d50726e732d62656e636831c027";
        assert_eq!(
            parse_offer_ssid(hosting).as_deref(),
            Some("DIRECT-Prns-bench1")
        );
        let forming = "055f70726e73c00c000c010450726e73c027";
        assert_eq!(parse_offer_ssid(forming).as_deref(), Some("Prns"));
    }
}
