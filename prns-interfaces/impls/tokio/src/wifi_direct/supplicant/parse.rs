use prns_core::interfaces::channel_rendezvous::{ChannelCommitment, WifiChannel};
use prns_core::interfaces::wifi_direct::SUPPLICANT_SERVICE_INSTANCE;
use prns_core::interfaces::MacAddress;

use crate::wifi_direct::service_discovery;

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
    let response = service_discovery::ptr_response(SUPPLICANT_SERVICE_INSTANCE).unwrap_or_default();
    format!(
        "P2P_SERVICE_ADD bonjour {} {}",
        hex(service_discovery::BONJOUR_PTR_QUERY),
        hex(&response)
    )
}

pub fn discover_service_command() -> String {
    format!(
        "P2P_SERV_DISC_REQ {BROADCAST_ADDRESS} {}",
        hex(service_discovery::SD_PTR_QUERY_TLV)
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
    service_instance(payload).is_some()
}

#[cfg(test)]
fn parse_status_ssid(status: &str) -> Option<String> {
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
    let rdata = service_discovery::ptr_response(ssid).unwrap_or_default();
    format!(
        "P2P_SERVICE_ADD bonjour {} {}",
        hex(service_discovery::BONJOUR_PTR_QUERY),
        hex(&rdata)
    )
}

pub fn service_instance(payload: &str) -> Option<String> {
    let tlvs = payload.split_whitespace().last()?;
    let decoded = decode_hex(tlvs)?;
    service_discovery::recognized_instance(&decoded).map(str::to_owned)
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
            "P2P_SERVICE_ADD bonjour 055f70726e73c00c000c01 \
             0f50726e732d737570706c6963616e74c027"
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
            "42:00:00:00:00:00 1 \
             055f70726e73c00c000c010f50726e732d737570706c6963616e74c027"
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
}
