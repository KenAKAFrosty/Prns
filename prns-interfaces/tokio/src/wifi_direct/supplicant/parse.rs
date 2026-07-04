use prns_core::interfaces::MacAddress;

const BONJOUR_PTR_QUERY: &[u8] = &[
    0x05, 0x5f, 0x70, 0x72, 0x6e, 0x73, 0xc0, 0x0c, 0x00, 0x0c, 0x01,
];
const BONJOUR_PTR_RESPONSE: &[u8] = &[0x04, 0x50, 0x72, 0x6e, 0x73, 0xc0, 0x27];
const SD_PTR_QUERY_TLV: &[u8] = &[
    0x0d, 0x00, 0x01, 0x01, 0x05, 0x5f, 0x70, 0x72, 0x6e, 0x73, 0xc0, 0x0c, 0x00, 0x0c, 0x01,
];
const SERVICE_MARKER_HEX: &str = "5f70726e73";
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
}
