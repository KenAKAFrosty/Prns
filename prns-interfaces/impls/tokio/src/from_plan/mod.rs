#[cfg(feature = "rnode")]
use std::collections::HashSet;
use std::fmt;
use std::io;
use std::sync::Arc;

pub use prns_config as config;
use prns_config::{
    ConfiguredInterfaceLifecycle, DaemonPlan, InterfaceAccessPlan,
    InterfaceKind as PlannedInterfaceKind, PlannedInterface, PlannedMedium,
};
use prns_core::identity::IdentityHash;
use prns_core::interfaces::ax25_kiss::Ax25AddressError;
use prns_core::interfaces::bluetooth_auto::BleIdentity;
use prns_core::interfaces::kiss::EmptyStationIdentification;
use prns_core::interfaces::{IfacContext, InterfaceId, InterfaceOriginKind};
use prns_runtime::interfaces::rnode::protocol::RadioConfigError;
use prns_runtime::runtime::{AttachIntent, Attachable, PrnsNodeHandle};

use crate::i2p::{DuplicateI2pPeer, I2pInterfaceNameError, I2pPeerAddressError, RnsI2pStorage};
use crate::reconnect::ReconnectPolicy;
#[cfg(feature = "rnode")]
use crate::rnode::multi::RNodeMultiMembersError;
#[cfg(feature = "wifi-auto")]
use crate::wifi_auto::AutoWifiSettingsError;

#[cfg(feature = "ax25")]
mod ax25_kiss;
#[cfg(feature = "backbone")]
mod backbone;
#[cfg(feature = "bluetooth-auto")]
mod bluetooth_auto;
#[cfg(feature = "i2p")]
mod i2p;
#[cfg(feature = "kiss")]
mod kiss;
#[cfg(feature = "pipe")]
mod pipe;
#[cfg(feature = "rnode")]
mod rnode;
#[cfg(feature = "serial")]
mod serial;
#[cfg(any(
    feature = "kiss",
    feature = "ax25",
    feature = "rnode",
    feature = "weave"
))]
mod station_identification;
#[cfg(feature = "tcp")]
mod tcp;
#[cfg(feature = "udp")]
mod udp;
#[cfg(all(
    feature = "usb",
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]
mod usb_auto;
#[cfg(feature = "weave")]
mod weave;
#[cfg(feature = "websocket")]
mod websocket;
#[cfg(feature = "wifi-auto")]
mod wifi_auto;

const RECONNECT_POLICY: ReconnectPolicy = ReconnectPolicy::STANDARD;
pub enum PlanOutcome<'a> {
    Up {
        interface: &'a PlannedInterface,
        id: InterfaceId,
    },
    Failed {
        interface: &'a PlannedInterface,
        error: PlanFailure,
    },
}

#[derive(Debug, Clone)]
pub enum PlanFailure {
    MissingIfacCredentials,
    #[cfg(feature = "wifi-auto")]
    AutoWifiSettings(AutoWifiSettingsError),
    Network(Arc<io::Error>),
    EmptyStationIdentification,
    Ax25Address(Ax25AddressError),
    RadioConfig(RadioConfigError),
    UngroupedRNodeMultiMember,
    I2pInterfaceName(I2pInterfaceNameError),
    I2pPeerAddress(I2pPeerAddressError),
    DuplicateI2pPeer(DuplicateI2pPeer),
    MissingI2pStorage,
    MissingBleIdentity,
    #[cfg(feature = "rnode")]
    RNodeMultiMembers(RNodeMultiMembersError),
    #[cfg(feature = "weave")]
    WeaveIdentity(getrandom::Error),
    InterfaceNotBuilt(PlannedInterfaceKind),
}

impl fmt::Display for PlanFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingIfacCredentials => {
                formatter.write_str("IFAC requires a network name or passphrase")
            }
            #[cfg(feature = "wifi-auto")]
            Self::AutoWifiSettings(error) => error.fmt(formatter),
            Self::Network(error) => error.fmt(formatter),
            Self::EmptyStationIdentification => {
                formatter.write_str("station identification callsign cannot be empty")
            }
            Self::Ax25Address(error) => write!(formatter, "invalid AX.25 address: {error:?}"),
            Self::RadioConfig(error) => write!(formatter, "invalid RNode radio config: {error:?}"),
            Self::UngroupedRNodeMultiMember => {
                formatter.write_str("RNodeMulti member was not grouped with its parent device")
            }
            Self::I2pInterfaceName(error) => error.fmt(formatter),
            Self::I2pPeerAddress(error) => error.fmt(formatter),
            Self::DuplicateI2pPeer(error) => error.fmt(formatter),
            Self::MissingI2pStorage => formatter.write_str(
                "connectable I2P requires the daemon's RNS storage directory and transport identity",
            ),
            Self::MissingBleIdentity => {
                formatter.write_str("Bluetooth Auto requires a persisted BLE identity")
            }
            #[cfg(feature = "rnode")]
            Self::RNodeMultiMembers(error) => error.fmt(formatter),
            #[cfg(feature = "weave")]
            Self::WeaveIdentity(error) => {
                write!(formatter, "could not generate the Weave discovery identity: {error}")
            }
            Self::InterfaceNotBuilt(kind) => write!(
                formatter,
                "this build does not include the {} interface family",
                kind.cli_name()
            ),
        }
    }
}

impl std::error::Error for PlanFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            #[cfg(feature = "wifi-auto")]
            Self::AutoWifiSettings(error) => Some(error),
            Self::Network(error) => Some(error.as_ref()),
            Self::I2pInterfaceName(error) => Some(error),
            Self::I2pPeerAddress(error) => Some(error),
            Self::DuplicateI2pPeer(error) => Some(error),
            #[cfg(feature = "rnode")]
            Self::RNodeMultiMembers(error) => Some(error),
            #[cfg(feature = "weave")]
            Self::WeaveIdentity(_) => None,
            Self::MissingIfacCredentials
            | Self::EmptyStationIdentification
            | Self::Ax25Address(_)
            | Self::RadioConfig(_)
            | Self::UngroupedRNodeMultiMember
            | Self::MissingI2pStorage
            | Self::MissingBleIdentity
            | Self::InterfaceNotBuilt(_) => None,
        }
    }
}

#[cfg(feature = "wifi-auto")]
impl From<AutoWifiSettingsError> for PlanFailure {
    fn from(error: AutoWifiSettingsError) -> Self {
        Self::AutoWifiSettings(error)
    }
}

impl From<io::Error> for PlanFailure {
    fn from(error: io::Error) -> Self {
        Self::Network(Arc::new(error))
    }
}

impl From<EmptyStationIdentification> for PlanFailure {
    fn from(_: EmptyStationIdentification) -> Self {
        Self::EmptyStationIdentification
    }
}

impl From<Ax25AddressError> for PlanFailure {
    fn from(error: Ax25AddressError) -> Self {
        Self::Ax25Address(error)
    }
}

impl From<RadioConfigError> for PlanFailure {
    fn from(error: RadioConfigError) -> Self {
        Self::RadioConfig(error)
    }
}

impl From<I2pInterfaceNameError> for PlanFailure {
    fn from(error: I2pInterfaceNameError) -> Self {
        Self::I2pInterfaceName(error)
    }
}

impl From<I2pPeerAddressError> for PlanFailure {
    fn from(error: I2pPeerAddressError) -> Self {
        Self::I2pPeerAddress(error)
    }
}

impl From<DuplicateI2pPeer> for PlanFailure {
    fn from(error: DuplicateI2pPeer) -> Self {
        Self::DuplicateI2pPeer(error)
    }
}

#[cfg(feature = "rnode")]
impl From<RNodeMultiMembersError> for PlanFailure {
    fn from(error: RNodeMultiMembersError) -> Self {
        Self::RNodeMultiMembers(error)
    }
}

#[cfg(feature = "weave")]
impl From<getrandom::Error> for PlanFailure {
    fn from(error: getrandom::Error) -> Self {
        Self::WeaveIdentity(error)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlanRuntimeContext {
    i2p_storage: Option<RnsI2pStorage>,
    ble_identity: Option<BleIdentity>,
}

impl PlanRuntimeContext {
    pub fn with_rns_i2p_storage(
        storage_dir: impl Into<std::path::PathBuf>,
        transport_identity: IdentityHash,
    ) -> Self {
        Self {
            i2p_storage: Some(RnsI2pStorage::new(storage_dir, transport_identity)),
            ble_identity: None,
        }
    }

    pub fn with_ble_identity(mut self, identity: BleIdentity) -> Self {
        self.ble_identity = Some(identity);
        self
    }
}

#[derive(Default)]
pub struct PlanAttachments {
    groups: Vec<PlanAttachmentGroup>,
}

struct PlanAttachmentGroup {
    lifecycle: ConfiguredInterfaceLifecycle,
    interfaces: Vec<InterfaceId>,
    supervisor_task: Option<tokio::task::JoinHandle<()>>,
}

impl PlanAttachments {
    pub fn append(&mut self, mut other: Self) {
        self.groups.append(&mut other.groups);
    }

    pub fn for_lifecycle(mut self, lifecycle: ConfiguredInterfaceLifecycle) -> Self {
        self.groups
            .retain(|attachment| attachment.lifecycle == lifecycle);
        self
    }

    pub fn interfaces(&self) -> impl Iterator<Item = InterfaceId> + '_ {
        self.groups
            .iter()
            .flat_map(|attachment| attachment.interfaces.iter().copied())
    }

    pub fn is_empty(&self) -> bool {
        self.groups.is_empty()
    }

    pub async fn detach(self, handle: &PrnsNodeHandle) {
        let mut supervisor_tasks = Vec::new();
        for attachment in self.groups {
            if let Some(task) = attachment.supervisor_task {
                task.abort();
                supervisor_tasks.push(task);
            }
            for interface in attachment.interfaces {
                handle.remove_interface(interface);
            }
        }
        for task in supervisor_tasks {
            let _ = task.await;
        }
    }

    fn push_interface(&mut self, lifecycle: ConfiguredInterfaceLifecycle, interface: InterfaceId) {
        self.groups.push(PlanAttachmentGroup {
            lifecycle,
            interfaces: vec![interface],
            supervisor_task: None,
        });
    }

    #[cfg(feature = "rnode")]
    fn push_supervisor(
        &mut self,
        lifecycle: ConfiguredInterfaceLifecycle,
        interfaces: Vec<InterfaceId>,
        supervisor_task: tokio::task::JoinHandle<()>,
    ) {
        self.groups.push(PlanAttachmentGroup {
            lifecycle,
            interfaces,
            supervisor_task: Some(supervisor_task),
        });
    }
}

pub struct FromPlan(pub DaemonPlan);

impl AttachIntent for FromPlan {
    fn attach(self, handle: &PrnsNodeHandle) {
        let handle = handle.clone();
        let plan = self.0;
        tokio::spawn(async move {
            attach_plan(&handle, &plan, &mut |outcome| match outcome {
                PlanOutcome::Up { interface, .. } => {
                    #[cfg(feature = "tracing")]
                    {
                        tracing::info!(
                            target: "prns.interface",
                            event = "interface_configured",
                            interface_origin = InterfaceOriginKind::Configured.as_str(),
                            medium = planned_medium_name(&interface.medium),
                        );
                        tracing::debug!(
                            target: "prns.interface",
                            event = "interface_configured_detail",
                            interface_origin = InterfaceOriginKind::Configured.as_str(),
                            interface_name = ?interface.name,
                            medium = ?interface.medium,
                        );
                    }
                    #[cfg(not(feature = "tracing"))]
                    crate::diagnostic_log::info!(
                        "interface up [{}]: {:?} ({:?})",
                        InterfaceOriginKind::Configured.as_str(),
                        interface.name,
                        interface.medium
                    );
                }
                PlanOutcome::Failed { interface, error } => {
                    #[cfg(feature = "tracing")]
                    {
                        tracing::warn!(
                            target: "prns.interface",
                            event = "interface_configuration_failed",
                            interface_origin = InterfaceOriginKind::Configured.as_str(),
                            medium = planned_medium_name(&interface.medium),
                        );
                        tracing::debug!(
                            target: "prns.interface",
                            event = "interface_configuration_failed_detail",
                            interface_origin = InterfaceOriginKind::Configured.as_str(),
                            interface_name = ?interface.name,
                            medium = ?interface.medium,
                            error = %error,
                        );
                    }
                    #[cfg(not(feature = "tracing"))]
                    crate::diagnostic_log::warn!(
                        "interface failed [{}]: {:?} ({error})",
                        InterfaceOriginKind::Configured.as_str(),
                        interface.name
                    );
                }
            })
            .await;
        });
    }
}

pub async fn attach_plan(
    handle: &PrnsNodeHandle,
    plan: &DaemonPlan,
    report: &mut impl FnMut(PlanOutcome<'_>),
) -> PlanAttachments {
    attach_plan_with_context(handle, plan, &PlanRuntimeContext::default(), report).await
}

pub async fn attach_plan_with_context(
    handle: &PrnsNodeHandle,
    plan: &DaemonPlan,
    context: &PlanRuntimeContext,
    report: &mut impl FnMut(PlanOutcome<'_>),
) -> PlanAttachments {
    let mut attachments = PlanAttachments::default();
    #[cfg(feature = "rnode")]
    let mut rnode_multi_parents = HashSet::new();
    for interface in &plan.interfaces {
        if let PlannedMedium::RnodeMulti { member } = &interface.medium {
            #[cfg(feature = "rnode")]
            {
                let parent = member.parent();
                let key = (parent.name(), parent.device());
                if rnode_multi_parents.insert(key) {
                    rnode::stand_up_multi(
                        handle,
                        plan.interfaces.iter().filter_map(|candidate| {
                            let PlannedMedium::RnodeMulti { member } = &candidate.medium else {
                                return None;
                            };
                            (member.parent() == parent).then_some((candidate, member))
                        }),
                        &mut attachments,
                        report,
                    );
                }
            }
            #[cfg(not(feature = "rnode"))]
            {
                let _ = member;
                stand_up(handle, interface, context, &mut attachments, report).await;
            }
        } else {
            stand_up(handle, interface, context, &mut attachments, report).await;
        }
    }
    attachments
}

async fn stand_up<'a>(
    handle: &PrnsNodeHandle,
    interface: &'a PlannedInterface,
    context: &PlanRuntimeContext,
    attachments: &mut PlanAttachments,
    report: &mut impl FnMut(PlanOutcome<'a>),
) {
    let access = match runtime_access(interface) {
        Ok(access) => access,
        Err(error) => {
            report(PlanOutcome::Failed { interface, error });
            return;
        }
    };
    let construction = InterfaceConstruction {
        handle,
        interface,
        access,
    };
    let result = match &interface.medium {
        PlannedMedium::AutoWifi(planned) => {
            #[cfg(feature = "wifi-auto")]
            {
                wifi_auto::stand_up(construction, planned)
            }
            #[cfg(not(feature = "wifi-auto"))]
            {
                let _ = planned;
                Err(PlanFailure::InterfaceNotBuilt(PlannedInterfaceKind::Auto))
            }
        }
        PlannedMedium::PrnsUsbAuto => {
            #[cfg(all(
                feature = "usb",
                any(target_os = "linux", target_os = "macos", target_os = "windows")
            ))]
            {
                usb_auto::stand_up(construction)
            }
            #[cfg(not(all(
                feature = "usb",
                any(target_os = "linux", target_os = "macos", target_os = "windows")
            )))]
            {
                Err(PlanFailure::InterfaceNotBuilt(
                    PlannedInterfaceKind::PrnsUsbAuto,
                ))
            }
        }
        PlannedMedium::PrnsBluetoothAuto { .. } => {
            #[cfg(feature = "bluetooth-auto")]
            {
                bluetooth_auto::stand_up(construction, context)
            }
            #[cfg(not(feature = "bluetooth-auto"))]
            {
                Err(PlanFailure::InterfaceNotBuilt(
                    PlannedInterfaceKind::PrnsBluetoothAuto,
                ))
            }
        }
        PlannedMedium::PrnsWebSocketClient { target, framing } => {
            #[cfg(feature = "websocket")]
            {
                websocket::stand_up_client(construction, target, *framing)
            }
            #[cfg(not(feature = "websocket"))]
            {
                let _ = (target, framing);
                Err(PlanFailure::InterfaceNotBuilt(
                    PlannedInterfaceKind::PrnsWebSocketClient,
                ))
            }
        }
        PlannedMedium::PrnsWebSocketServer { listener, framing } => {
            #[cfg(feature = "websocket")]
            {
                websocket::stand_up_server(construction, listener, *framing).await
            }
            #[cfg(not(feature = "websocket"))]
            {
                let _ = (listener, framing);
                Err(PlanFailure::InterfaceNotBuilt(
                    PlannedInterfaceKind::PrnsWebSocketServer,
                ))
            }
        }
        PlannedMedium::TcpClient {
            connection,
            framing,
        } => tcp::stand_up_client(construction, connection, *framing),
        PlannedMedium::TcpServer { listener, framing } => {
            tcp::stand_up_server(construction, listener, *framing).await
        }
        PlannedMedium::Udp { flow } => udp::stand_up(construction, flow).await,
        PlannedMedium::Serial { device, line } => {
            #[cfg(feature = "serial")]
            {
                serial::stand_up(construction, device, *line)
            }
            #[cfg(not(feature = "serial"))]
            {
                let _ = (device, line);
                Err(PlanFailure::InterfaceNotBuilt(PlannedInterfaceKind::Serial))
            }
        }
        PlannedMedium::Kiss {
            device,
            line,
            preamble_ms,
            txtail_ms,
            persistence,
            slottime_ms,
            flow_control,
            station_id,
        } => {
            #[cfg(feature = "kiss")]
            {
                kiss::stand_up(
                    construction,
                    kiss::Configuration {
                        device,
                        line: *line,
                        preamble_ms: *preamble_ms,
                        txtail_ms: *txtail_ms,
                        persistence: *persistence,
                        slottime_ms: *slottime_ms,
                        flow_control: *flow_control,
                        station_id,
                    },
                )
            }
            #[cfg(not(feature = "kiss"))]
            {
                let _ = (
                    device,
                    line,
                    preamble_ms,
                    txtail_ms,
                    persistence,
                    slottime_ms,
                    flow_control,
                    station_id,
                );
                Err(PlanFailure::InterfaceNotBuilt(PlannedInterfaceKind::Kiss))
            }
        }
        PlannedMedium::Ax25Kiss {
            device,
            line,
            preamble_ms,
            txtail_ms,
            persistence,
            slottime_ms,
            flow_control,
            callsign,
            ssid,
        } => {
            #[cfg(feature = "ax25")]
            {
                ax25_kiss::stand_up(
                    construction,
                    ax25_kiss::Configuration {
                        device,
                        line: *line,
                        preamble_ms: *preamble_ms,
                        txtail_ms: *txtail_ms,
                        persistence: *persistence,
                        slottime_ms: *slottime_ms,
                        flow_control: *flow_control,
                        callsign,
                        ssid: *ssid,
                    },
                )
            }
            #[cfg(not(feature = "ax25"))]
            {
                let _ = (
                    device,
                    line,
                    preamble_ms,
                    txtail_ms,
                    persistence,
                    slottime_ms,
                    flow_control,
                    callsign,
                    ssid,
                );
                Err(PlanFailure::InterfaceNotBuilt(
                    PlannedInterfaceKind::Ax25Kiss,
                ))
            }
        }
        PlannedMedium::Rnode {
            transport,
            frequency_hz,
            bandwidth_hz,
            tx_power_dbm,
            spreading_factor,
            coding_rate,
            flow_control,
            station_id,
            airtime_limit_short,
            airtime_limit_long,
        } => {
            #[cfg(feature = "rnode")]
            {
                rnode::stand_up(
                    construction,
                    rnode::Configuration {
                        transport,
                        frequency_hz: *frequency_hz,
                        bandwidth_hz: *bandwidth_hz,
                        tx_power_dbm: *tx_power_dbm,
                        spreading_factor: *spreading_factor,
                        coding_rate: *coding_rate,
                        flow_control: *flow_control,
                        station_id,
                        airtime_limit_short: *airtime_limit_short,
                        airtime_limit_long: *airtime_limit_long,
                    },
                )
            }
            #[cfg(not(feature = "rnode"))]
            {
                let _ = (
                    transport,
                    frequency_hz,
                    bandwidth_hz,
                    tx_power_dbm,
                    spreading_factor,
                    coding_rate,
                    flow_control,
                    station_id,
                    airtime_limit_short,
                    airtime_limit_long,
                );
                Err(PlanFailure::InterfaceNotBuilt(PlannedInterfaceKind::Rnode))
            }
        }
        PlannedMedium::RnodeMulti { .. } => {
            #[cfg(feature = "rnode")]
            {
                Err(PlanFailure::UngroupedRNodeMultiMember)
            }
            #[cfg(not(feature = "rnode"))]
            {
                Err(PlanFailure::InterfaceNotBuilt(
                    PlannedInterfaceKind::RnodeMulti,
                ))
            }
        }
        PlannedMedium::Backbone { listener } => {
            backbone::stand_up_server(construction, listener).await
        }
        PlannedMedium::BackboneClient { connection } => {
            backbone::stand_up_client(construction, connection)
        }
        PlannedMedium::Pipe {
            command,
            respawn_delay,
        } => pipe::stand_up(construction, command, *respawn_delay),
        PlannedMedium::I2p {
            peers,
            reachability,
        } => i2p::stand_up(construction, peers, *reachability, context),
        PlannedMedium::Weave { device } => {
            #[cfg(feature = "weave")]
            {
                weave::stand_up(construction, device)
            }
            #[cfg(not(feature = "weave"))]
            {
                let _ = device;
                Err(PlanFailure::InterfaceNotBuilt(PlannedInterfaceKind::Weave))
            }
        }
    };
    match result {
        Ok(id) => {
            attachments.push_interface(interface.lifecycle, id);
            report_up(handle, interface, id, report);
        }
        Err(error) => report(PlanOutcome::Failed { interface, error }),
    }
}
struct IfacAccess {
    context: IfacContext,
    network_name: Option<String>,
}

struct InterfaceAccess {
    ifac: Option<IfacAccess>,
}

fn runtime_access(interface: &PlannedInterface) -> Result<InterfaceAccess, PlanFailure> {
    match &interface.access {
        InterfaceAccessPlan::Open => Ok(InterfaceAccess { ifac: None }),
        InterfaceAccessPlan::Ifac {
            network_name,
            passphrase,
            size,
        } => IfacContext::derive(network_name.as_deref(), passphrase.as_deref(), *size)
            .map(|context| InterfaceAccess {
                ifac: Some(IfacAccess {
                    context,
                    network_name: network_name.clone(),
                }),
            })
            .ok_or(PlanFailure::MissingIfacCredentials),
    }
}

type AttachmentResult = Result<InterfaceId, PlanFailure>;

struct InterfaceConstruction<'a> {
    handle: &'a PrnsNodeHandle,
    interface: &'a PlannedInterface,
    access: InterfaceAccess,
}

impl InterfaceConstruction<'_> {
    fn attach<A: Attachable>(self, attachable: A) -> A::Attached {
        match self.access.ifac {
            None => self.handle.attach(attachable),
            Some(IfacAccess {
                context,
                network_name,
            }) => self
                .handle
                .attach_with_ifac_name(attachable, context, network_name),
        }
    }
}

fn report_up<'a>(
    handle: &PrnsNodeHandle,
    interface: &'a PlannedInterface,
    id: InterfaceId,
    report: &mut impl FnMut(PlanOutcome<'a>),
) {
    let _ = handle.set_interface_name(id, interface.name.clone());
    report(PlanOutcome::Up { interface, id });
}

#[cfg(feature = "tracing")]
fn planned_medium_name(medium: &PlannedMedium) -> &'static str {
    match medium {
        PlannedMedium::AutoWifi(_) => "auto_wifi",
        PlannedMedium::TcpClient { .. } => "tcp_client",
        PlannedMedium::TcpServer { .. } => "tcp_server",
        PlannedMedium::Udp { .. } => "udp",
        PlannedMedium::Serial { .. } => "serial",
        PlannedMedium::Kiss { .. } => "kiss",
        PlannedMedium::Ax25Kiss { .. } => "ax25_kiss",
        PlannedMedium::Rnode { .. } => "rnode",
        PlannedMedium::RnodeMulti { .. } => "rnode_multi",
        PlannedMedium::Backbone { .. } => "backbone",
        PlannedMedium::BackboneClient { .. } => "backbone_client",
        PlannedMedium::Pipe { .. } => "pipe",
        PlannedMedium::I2p { .. } => "i2p",
        PlannedMedium::Weave { .. } => "weave",
        PlannedMedium::PrnsUsbAuto => "prns_usb_auto",
        PlannedMedium::PrnsBluetoothAuto { .. } => "prns_bluetooth_auto",
        PlannedMedium::PrnsWebSocketClient { .. } => "prns_websocket_client",
        PlannedMedium::PrnsWebSocketServer { .. } => "prns_websocket_server",
    }
}
