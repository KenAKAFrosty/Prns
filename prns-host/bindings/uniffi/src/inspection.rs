use crate::remote_control::{
    RemoteControlInterfaceKind, RemoteControlPeerDetails, RemoteControlRadioIndication,
};
use crate::transport::{HostSnapshot, InterfaceHealth, InterfaceKind};

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum HostPhysicalRole {
    Independent,
    Supervisor,
    Member,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum HostPhysicalOrigin {
    Configured,
    Discovered,
}

/// Physical rows retain their explicit control and parent associations. Their
/// counters are separate from the logical host snapshot's folded counters.
#[derive(uniffi::Record)]
pub struct HostPhysicalInterface {
    pub runtime_interface: Vec<u8>,
    pub logical_interface: Vec<u8>,
    pub attachment: Option<Vec<u8>>,
    pub parent: Option<Vec<u8>>,
    pub role: HostPhysicalRole,
    pub kind: Option<InterfaceKind>,
    pub runtime_kind: Option<RemoteControlInterfaceKind>,
    pub origin: HostPhysicalOrigin,
    pub attachment_epoch: u64,
    pub name: Option<String>,
    pub health: InterfaceHealth,
    pub failure_detail: Option<String>,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub rx_bps: Option<u64>,
    pub tx_bps: Option<u64>,
    pub endpoint: Option<String>,
    pub radio: RemoteControlRadioIndication,
    pub details: RemoteControlPeerDetails,
}

#[derive(uniffi::Record)]
pub struct HostInspection {
    pub host: HostSnapshot,
    pub engine_time_millis: u64,
    pub interfaces: Vec<HostPhysicalInterface>,
}
impl From<prns_host_runtime::HostInspectionSnapshot> for HostInspection {
    fn from(value: prns_host_runtime::HostInspectionSnapshot) -> Self {
        Self {
            host: value.host.into(),
            engine_time_millis: value.engine_time_millis,
            interfaces: value
                .physical_interfaces
                .into_iter()
                .map(Into::into)
                .collect(),
        }
    }
}
impl From<prns_host_runtime::HostConnectionSnapshot> for HostPhysicalInterface {
    fn from(value: prns_host_runtime::HostConnectionSnapshot) -> Self {
        use prns_core::interfaces::InterfaceOriginKind;
        use prns_host_runtime::HostConnectionRole;
        Self {
            runtime_interface: value.runtime_interface.into_bytes().to_vec(),
            logical_interface: value.logical_interface.into_bytes().to_vec(),
            attachment: value.attachment.map(|id| id.into_bytes().to_vec()),
            parent: value.parent.map(|id| id.into_bytes().to_vec()),
            role: match value.role {
                HostConnectionRole::Independent => HostPhysicalRole::Independent,
                HostConnectionRole::Supervisor => HostPhysicalRole::Supervisor,
                HostConnectionRole::Member => HostPhysicalRole::Member,
            },
            kind: value.kind.map(Into::into),
            runtime_kind: value.runtime_kind.map(Into::into),
            origin: match value.origin {
                InterfaceOriginKind::Configured => HostPhysicalOrigin::Configured,
                InterfaceOriginKind::Discovered => HostPhysicalOrigin::Discovered,
            },
            attachment_epoch: value.attachment_epoch,
            name: value.name,
            health: value.health.into(),
            failure_detail: value.failure_detail,
            rx_bytes: value.rx_bytes,
            tx_bytes: value.tx_bytes,
            rx_bps: value.rx_bps,
            tx_bps: value.tx_bps,
            endpoint: value.endpoint,
            radio: value.radio.into(),
            details: value.details.into(),
        }
    }
}
