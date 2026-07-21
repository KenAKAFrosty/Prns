use crate::interface_discovery::{
    AdvertisedInterfaceType, AdvertisementDetails, DiscoveredInterface, DiscoveredInterfaceId,
};

use super::file::encode_hex;

pub fn manual_configuration(interface: &DiscoveredInterface) -> Option<String> {
    let name = manual_interface_name(&interface.name, interface.id);
    let transport_identity =
        encode_hex(interface.advertisement.transport.transport_id().as_bytes());
    let mut lines = Vec::new();
    lines.push(format!("[[{name}]]"));
    match (
        interface.advertisement.interface_type,
        &interface.advertisement.details,
    ) {
        (AdvertisedInterfaceType::Backbone, AdvertisementDetails::Reachable { host, port }) => {
            lines.push(String::from("  type = BackboneClientInterface"));
            lines.push(format!("  target_host = {}", config_scalar(host)?));
            lines.push(format!("  target_port = {port}"));
        }
        (AdvertisedInterfaceType::TcpServer, AdvertisementDetails::Reachable { host, port }) => {
            lines.push(String::from("  type = TCPClientInterface"));
            lines.push(format!("  target_host = {}", config_scalar(host)?));
            lines.push(format!("  target_port = {port}"));
        }
        (AdvertisedInterfaceType::I2p, AdvertisementDetails::I2p { address }) => {
            lines.push(String::from("  type = I2PInterface"));
            lines.push(format!("  peers = {}", config_scalar(address)?));
        }
        (
            AdvertisedInterfaceType::RNode,
            AdvertisementDetails::RNode {
                frequency_hz,
                bandwidth_hz,
                spreading_factor,
                coding_rate,
            },
        ) => {
            lines.push(String::from("  type = RNodeInterface"));
            lines.push(String::from("  port = "));
            lines.push(format!("  frequency = {frequency_hz}"));
            lines.push(format!("  bandwidth = {bandwidth_hz}"));
            lines.push(format!("  spreadingfactor = {spreading_factor}"));
            lines.push(format!("  codingrate = {coding_rate}"));
            lines.push(String::from("  txpower = "));
        }
        (
            AdvertisedInterfaceType::Weave,
            AdvertisementDetails::Weave {
                frequency_hz,
                bandwidth_hz,
                channel,
                modulation,
            },
        ) => {
            lines.push(String::from("  type = WeaveInterface"));
            lines.push(String::from("  port = "));
            lines.push(format!("  frequency = {frequency_hz}"));
            lines.push(format!("  bandwidth = {bandwidth_hz}"));
            lines.push(format!("  channel = {channel}"));
            lines.push(format!("  modulation = {}", config_scalar(modulation)?));
        }
        (
            AdvertisedInterfaceType::Kiss,
            AdvertisementDetails::Kiss {
                frequency_hz,
                bandwidth_hz,
                modulation,
            },
        ) => {
            lines.push(String::from("  type = KISSInterface"));
            lines.push(String::from("  port = "));
            lines.push(format!("  frequency = {frequency_hz}"));
            lines.push(format!("  bandwidth = {bandwidth_hz}"));
            lines.push(format!("  modulation = {}", config_scalar(modulation)?));
        }
        (AdvertisedInterfaceType::TcpClient, AdvertisementDetails::None)
        | (AdvertisedInterfaceType::Backbone, _)
        | (AdvertisedInterfaceType::TcpServer, _)
        | (AdvertisedInterfaceType::TcpClient, _)
        | (AdvertisedInterfaceType::I2p, _)
        | (AdvertisedInterfaceType::RNode, _)
        | (AdvertisedInterfaceType::Weave, _)
        | (AdvertisedInterfaceType::Kiss, _) => return None,
    }
    lines.push(String::from("  enabled = Yes"));
    lines.push(format!("  transport_identity = {transport_identity}"));
    if let Some(ifac) = &interface.advertisement.published_ifac {
        if let Some(network_name) = &ifac.network_name {
            lines.push(format!("  network_name = {}", config_scalar(network_name)?));
        }
        if let Some(passphrase) = &ifac.passphrase {
            lines.push(format!("  passphrase = {}", config_scalar(passphrase)?));
        }
    }
    Some(lines.join("\n"))
}

fn manual_interface_name(name: &str, id: DiscoveredInterfaceId) -> String {
    let mut safe = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric()
                || matches!(character, ' ' | '-' | '_' | '.' | '(' | ')')
            {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    if safe.trim().is_empty() {
        safe = String::from("Discovered Interface");
    }
    let id = encode_hex(id.as_bytes());
    format!("{} ({})", safe.trim(), &id[..12])
}

fn config_scalar(value: &str) -> Option<String> {
    if !value
        .chars()
        .any(|character| matches!(character, '\r' | '\n' | '\''))
    {
        return Some(format!("'{value}'"));
    }
    if !value
        .chars()
        .any(|character| matches!(character, '\r' | '\n' | '"'))
    {
        return Some(format!("\"{value}\""));
    }
    if !value.contains("\"\"\"") {
        return Some(format!("\"\"\"{value}\"\"\""));
    }
    if !value.contains("'''") {
        return Some(format!("'''{value}'''"));
    }
    None
}
