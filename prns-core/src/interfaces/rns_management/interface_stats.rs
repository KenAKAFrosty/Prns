use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::time::Duration;

use crate::identity::IdentityHash;
use crate::interfaces::IfacSize;
use crate::interfaces::{
    ConnectionState, InterfaceId, InterfaceMode, InterfaceSnapshot, TransferRates,
};
use crate::wire::DestinationHash;

use super::message_pack::MessagePackEncoder;
use super::wire_names::{interface, transport};
use super::{interface_name, RnsManagementEncodeError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RnsInterfaceAccessCode {
    signature: [u8; 64],
    size: IfacSize,
    network_name: Option<String>,
}

impl RnsInterfaceAccessCode {
    pub fn new(signature: [u8; 64], size: IfacSize, network_name: Option<String>) -> Self {
        Self {
            signature,
            size,
            network_name,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RnsInterfaceStatsEntry {
    name: Option<String>,
    snapshot: InterfaceSnapshot,
    access_code: Option<RnsInterfaceAccessCode>,
    rssi: Option<i8>,
    group_id: Option<String>,
    fleet_peers: Vec<RnsInterfaceStatsEntry>,
}

impl RnsInterfaceStatsEntry {
    pub fn new(
        name: Option<String>,
        snapshot: InterfaceSnapshot,
        access_code: Option<RnsInterfaceAccessCode>,
    ) -> Self {
        Self {
            name,
            snapshot,
            access_code,
            rssi: None,
            group_id: None,
            fleet_peers: Vec::new(),
        }
    }

    pub fn with_rssi(mut self, rssi: Option<i8>) -> Self {
        self.rssi = rssi;
        self
    }

    pub fn with_group_id(mut self, group_id: Option<String>) -> Self {
        self.group_id = group_id;
        self
    }

    pub fn with_fleet_peers(mut self, fleet_peers: Vec<RnsInterfaceStatsEntry>) -> Self {
        self.fleet_peers = fleet_peers;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RnsTransportStatus {
    transport_identity: IdentityHash,
    network_identity: Option<IdentityHash>,
    uptime: Duration,
    probe_responder: Option<DestinationHash>,
    software_version: Option<String>,
}

impl RnsTransportStatus {
    pub const fn new(
        transport_identity: IdentityHash,
        network_identity: Option<IdentityHash>,
        uptime: Duration,
    ) -> Self {
        Self {
            transport_identity,
            network_identity,
            uptime,
            probe_responder: None,
            software_version: None,
        }
    }

    #[must_use]
    pub const fn with_probe_responder(mut self, probe_responder: Option<DestinationHash>) -> Self {
        self.probe_responder = probe_responder;
        self
    }

    pub fn with_software_version(mut self, software_version: impl Into<String>) -> Self {
        self.software_version = Some(software_version.into());
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RnsInterfaceStats {
    entries: Vec<RnsInterfaceStatsEntry>,
    transport: Option<RnsTransportStatus>,
}

impl RnsInterfaceStats {
    pub fn new(entries: Vec<RnsInterfaceStatsEntry>) -> Self {
        Self {
            entries,
            transport: None,
        }
    }

    pub fn with_transport(mut self, transport: RnsTransportStatus) -> Self {
        self.transport = Some(transport);
        self
    }

    pub fn encode_message_pack(&self) -> Result<Vec<u8>, RnsManagementEncodeError> {
        let mut encoder = MessagePackEncoder::new();
        self.encode_into(&mut encoder)?;
        Ok(encoder.finish())
    }

    pub fn encode_remote_response(
        &self,
        link_count: Option<u32>,
    ) -> Result<Vec<u8>, RnsManagementEncodeError> {
        let mut encoder = MessagePackEncoder::new();
        encoder.array(if link_count.is_some() { 2 } else { 1 })?;
        self.encode_into(&mut encoder)?;
        if let Some(link_count) = link_count {
            encoder.unsigned(u64::from(link_count));
        }
        Ok(encoder.finish())
    }

    pub(crate) fn encode_into(
        &self,
        encoder: &mut MessagePackEncoder,
    ) -> Result<(), RnsManagementEncodeError> {
        encoder.map(if self.transport.is_some() {
            let mut fields = 10usize;
            if self
                .transport
                .as_ref()
                .is_some_and(|status| status.software_version.is_some())
            {
                fields = fields.saturating_add(1);
            }
            fields
        } else {
            6
        })?;
        encoder.field(interface::INTERFACES)?;
        encoder.array(self.entries.len())?;

        let mut total_receive_bytes = 0u64;
        let mut total_transmit_bytes = 0u64;
        let mut total_receive_speed = 0u64;
        let mut total_transmit_speed = 0u64;
        for entry in &self.entries {
            encode_interface_entry(encoder, entry)?;
            let rates = entry.snapshot.transfer_rates.unwrap_or(TransferRates {
                rx_bps: 0,
                tx_bps: 0,
            });
            total_receive_bytes = total_receive_bytes.saturating_add(entry.snapshot.rx_bytes);
            total_transmit_bytes = total_transmit_bytes.saturating_add(entry.snapshot.tx_bytes);
            total_receive_speed = total_receive_speed.saturating_add(u64::from(rates.rx_bps));
            total_transmit_speed = total_transmit_speed.saturating_add(u64::from(rates.tx_bps));
        }

        encoder.unsigned_field(interface::RECEIVE_BYTES, total_receive_bytes)?;
        encoder.unsigned_field(interface::TRANSMIT_BYTES, total_transmit_bytes)?;
        encoder.unsigned_field(interface::RECEIVE_SPEED, total_receive_speed)?;
        encoder.unsigned_field(interface::TRANSMIT_SPEED, total_transmit_speed)?;
        encoder.field(interface::RESIDENT_SET_SIZE)?;
        encoder.nil();

        if let Some(status) = &self.transport {
            encoder.field(transport::IDENTITY)?;
            encoder.binary(status.transport_identity.as_bytes())?;
            encoder.field(transport::NETWORK_IDENTITY)?;
            match status.network_identity {
                Some(identity) => encoder.binary(identity.as_bytes())?,
                None => encoder.nil(),
            }
            encoder.field(transport::UPTIME)?;
            encoder.float(status.uptime.as_secs_f64());
            encoder.field(transport::PROBE_RESPONDER)?;
            match status.probe_responder {
                Some(destination) => encoder.binary(destination.as_bytes())?,
                None => encoder.nil(),
            }
            if let Some(software_version) = &status.software_version {
                encoder.string_field(transport::SOFTWARE_VERSION, software_version)?;
            }
        }
        Ok(())
    }
}

fn encode_interface_entry(
    encoder: &mut MessagePackEncoder,
    entry: &RnsInterfaceStatsEntry,
) -> Result<(), RnsManagementEncodeError> {
    let name = entry
        .name
        .clone()
        .unwrap_or_else(|| interface_name(entry.snapshot.id));
    let rates = entry.snapshot.transfer_rates.unwrap_or(TransferRates {
        rx_bps: 0,
        tx_bps: 0,
    });
    let mut fields = 14usize;
    if entry.rssi.is_some() {
        fields = fields.saturating_add(1);
    }
    if entry.group_id.is_some() {
        fields = fields.saturating_add(1);
    }
    if !entry.fleet_peers.is_empty() {
        fields = fields.saturating_add(1);
    }
    encoder.map(fields)?;
    encoder.string_field(interface::NAME, &name)?;
    encoder.string_field(interface::SHORT_NAME, &name)?;
    encoder.string_field(interface::TYPE, &interface_type(entry.snapshot.id))?;
    encoder.field(interface::STATUS)?;
    encoder.boolean(is_online(entry.snapshot.connection));
    encoder.field(interface::MODE)?;
    encoder.signed(interface_mode(entry.snapshot.mode));
    encoder.field(interface::GRAVITY)?;
    encoder.signed(entry.snapshot.gravity.get());
    encoder.field(interface::CLIENTS)?;
    encoder.nil();
    encoder.unsigned_field(interface::RECEIVE_BYTES, entry.snapshot.rx_bytes)?;
    encoder.unsigned_field(interface::TRANSMIT_BYTES, entry.snapshot.tx_bytes)?;
    encoder.unsigned_field(interface::RECEIVE_SPEED, u64::from(rates.rx_bps))?;
    encoder.unsigned_field(interface::TRANSMIT_SPEED, u64::from(rates.tx_bps))?;
    encoder.field(interface::IFAC_SIGNATURE)?;
    match &entry.access_code {
        Some(access_code) => encoder.binary(&access_code.signature)?,
        None => encoder.nil(),
    }
    encoder.field(interface::IFAC_SIZE)?;
    match &entry.access_code {
        Some(access_code) => {
            encoder.unsigned(u64::try_from(access_code.size.bytes()).unwrap_or(u64::MAX))
        }
        None => encoder.nil(),
    }
    encoder.field(interface::IFAC_NETWORK_NAME)?;
    match entry
        .access_code
        .as_ref()
        .and_then(|access_code| access_code.network_name.as_deref())
    {
        Some(network_name) => encoder.string(network_name)?,
        None => encoder.nil(),
    }
    if let Some(rssi) = entry.rssi {
        encoder.field(interface::RSSI)?;
        encoder.signed(i64::from(rssi));
    }
    if let Some(group_id) = &entry.group_id {
        encoder.string_field(interface::GROUP_ID, group_id)?;
    }
    if !entry.fleet_peers.is_empty() {
        encoder.field(interface::FLEET_PEERS)?;
        encoder.array(entry.fleet_peers.len())?;
        for peer in &entry.fleet_peers {
            encode_fleet_peer_entry(encoder, peer)?;
        }
    }
    Ok(())
}

fn encode_fleet_peer_entry(
    encoder: &mut MessagePackEncoder,
    entry: &RnsInterfaceStatsEntry,
) -> Result<(), RnsManagementEncodeError> {
    let name = entry
        .name
        .clone()
        .unwrap_or_else(|| interface_name(entry.snapshot.id));
    let rates = entry.snapshot.transfer_rates.unwrap_or(TransferRates {
        rx_bps: 0,
        tx_bps: 0,
    });
    let fields = if entry.rssi.is_some() { 7usize } else { 6usize };
    encoder.map(fields)?;
    encoder.string_field(interface::NAME, &name)?;
    encoder.field(interface::STATUS)?;
    encoder.boolean(is_online(entry.snapshot.connection));
    encoder.unsigned_field(interface::RECEIVE_BYTES, entry.snapshot.rx_bytes)?;
    encoder.unsigned_field(interface::TRANSMIT_BYTES, entry.snapshot.tx_bytes)?;
    encoder.unsigned_field(interface::RECEIVE_SPEED, u64::from(rates.rx_bps))?;
    encoder.unsigned_field(interface::TRANSMIT_SPEED, u64::from(rates.tx_bps))?;
    if let Some(rssi) = entry.rssi {
        encoder.field(interface::RSSI)?;
        encoder.signed(i64::from(rssi));
    }
    Ok(())
}

fn interface_mode(mode: InterfaceMode) -> i64 {
    match mode {
        InterfaceMode::Full => 0x01,
        InterfaceMode::PointToPoint => 0x02,
        InterfaceMode::AccessPoint => 0x03,
        InterfaceMode::Roaming => 0x04,
        InterfaceMode::Boundary => 0x05,
        InterfaceMode::Gateway => 0x06,
        InterfaceMode::Internal => 0x07,
    }
}

fn interface_type(id: InterfaceId) -> String {
    match id.kind() {
        Some(kind) => format!("{kind:?}"),
        None => String::from("Interface"),
    }
}

fn is_online(connection: ConnectionState) -> bool {
    matches!(
        connection,
        ConnectionState::Connected | ConnectionState::Degraded
    )
}
