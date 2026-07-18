//! Stand up a [`DaemonPlan`]'s interfaces on a running node — the library side of
//! "read the interfaces from a stock RNS config". Construction lives here; each outcome
//! is reported through the caller's callback ([`PlanOutcome`]), so a daemon renders its
//! own lines.

use core::time::Duration;
use std::collections::HashSet;

pub use prns_config as config;
use prns_config::{
    AddressFamilyPreference as PlannedAddressFamilyPreference, AutoInterfacePlan,
    ConfiguredInterfaceLifecycle, DaemonPlan, I2pPeersPlan,
    I2pReachabilityPlan as PlannedI2pReachability, InterfaceAccessPlan, PlannedInterface,
    PlannedMedium, RNodeMultiMemberPlan, ReadyCommandFlowControl as PlannedReadyCommandFlowControl,
    ReconnectLimit as PlannedReconnectLimit, SerialDataBits, SerialLinePlan, SerialParity,
    SerialStopBits, StationIdentificationPlan, TcpDialPlan, TcpTunnelMode as PlannedTcpTunnelMode,
    UdpFlowPlan,
};
use prns_core::identity::IdentityHash;
use prns_core::interfaces::ifac::IfacContext;
use prns_core::interfaces::{InterfaceId, InterfaceOriginKind};
use prns_runtime::interfaces::kiss::core::TncConfig;
use prns_runtime::interfaces::rnode::core::{RadioConfig, RadioConfigInput};
use prns_runtime::runtime::{AttachIntent, Attachable, PrnsNodeHandle};

use crate::ax25::{Ax25KissInterface, Ax25KissSettings};
use crate::backbone::client::BackboneClientInterface;
use crate::backbone::server::BackboneServer;
use crate::host_network::{
    resolve_tcp_listener, resolve_udp_endpoint, tcp_target, udp_ephemeral_bind,
};
use crate::i2p::{
    I2pInterface, I2pInterfaceName, I2pPeerAddress, I2pPeers, I2pReachability, I2pRetryPolicy,
    I2pRuntimeConfig, RnsI2pStorage, TokioSamBridge,
};
use crate::kiss::{KissInterface, KissSettings, DEFAULT_TNC_CONFIGURE_DELAY};
use crate::pipe::{PipeInterface, PipeRespawnDelay};
use crate::reconnect::ReconnectDelay;
use crate::rnode::{RNodeInterface, RNodeSettings};
use crate::rnode_host::{RNodeHostOpener, RNODE_BAUD};
use crate::rnode_multi::{
    RNodeMultiAccess, RNodeMultiInterface, RNodeMultiMemberSettings, RNodeMultiMembers,
    RNodeMultiSettings, DEFAULT_RNODE_MULTI_CONFIGURE_DELAY,
};
use crate::serial::SerialInterface;
use crate::serial_host::{
    open_host_serial, open_host_serial_with_settings, HostSerialDataBits, HostSerialLineSettings,
    HostSerialParity, HostSerialStopBits,
};
use crate::tcp::client::TcpClientInterface;
use crate::tcp::server::TcpServer;
use crate::tcp::tokio_socket::{
    AddressFamilyPreference, ReconnectLimit, TcpConnectionSettings, TcpTunnelMode,
};
use crate::udp::UdpInterface;
use crate::wifi::{AutoWifi, AutoWifiDevicePolicy, AutoWifiSettings};
use prns_core::interfaces::kiss::transmission_control::{
    ReadyCommandFlowControl, ReadyTimeout, StationIdInterval, StationIdWireFormat,
    StationIdentification,
};

const TCP_RECONNECT_DELAY: Duration = Duration::from_secs(5);
const SERIAL_RECONNECT_DELAY: ReconnectDelay = ReconnectDelay::new(Duration::from_millis(500));
const KISS_FLOW_CONTROL_TIMEOUT: ReadyTimeout = ReadyTimeout::new(Duration::from_secs(5));
/// RNS `RNodeInterface.RECONNECT_WAIT`: an RNode's bring-up handshake is expensive (settle, detect,
/// configure, validate), so a dropped or absent device is retried on a slower cadence than a bare
/// serial port.
const RNODE_RECONNECT_DELAY: ReconnectDelay = ReconnectDelay::new(Duration::from_secs(5));

/// What became of one planned item, handed to the caller's reporter as it happens.
pub enum PlanOutcome<'a> {
    Up {
        interface: &'a PlannedInterface,
        id: InterfaceId,
    },
    Failed {
        interface: &'a PlannedInterface,
        visible_error_message: String,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlanRuntimeContext {
    i2p_storage: Option<RnsI2pStorage>,
}

impl PlanRuntimeContext {
    pub fn with_rns_i2p_storage(
        storage_dir: impl Into<std::path::PathBuf>,
        transport_identity: IdentityHash,
    ) -> Self {
        Self {
            i2p_storage: Some(RnsI2pStorage::new(storage_dir, transport_identity)),
        }
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

/// The recipe intent for a config-driven node. Construction awaits socket binds, so it rides its
/// own task off `PrnsNode::new`.
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
                PlanOutcome::Failed {
                    interface,
                    visible_error_message,
                } => {
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
                            error = %visible_error_message,
                        );
                    }
                    #[cfg(not(feature = "tracing"))]
                    crate::diagnostic_log::warn!(
                        "interface failed [{}]: {:?} ({visible_error_message})",
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
    let mut rnode_multi_parents = HashSet::new();
    for interface in &plan.interfaces {
        if let PlannedMedium::RnodeMulti { member } = &interface.medium {
            let parent = member.parent();
            let key = (parent.name(), parent.device());
            if rnode_multi_parents.insert(key) {
                stand_up_rnode_multi(
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
        } else {
            stand_up(handle, interface, context, &mut attachments, report).await;
        }
    }
    attachments
}

async fn stand_up(
    handle: &PrnsNodeHandle,
    interface: &PlannedInterface,
    context: &PlanRuntimeContext,
    attachments: &mut PlanAttachments,
    report: &mut impl FnMut(PlanOutcome<'_>),
) {
    let access = match runtime_access(interface) {
        Ok(access) => access,
        Err(visible_error_message) => {
            report(PlanOutcome::Failed {
                interface,
                visible_error_message,
            });
            return;
        }
    };
    match &interface.medium {
        PlannedMedium::AutoWifi(planned) => {
            let settings = match auto_wifi_settings(&interface.name, planned) {
                Ok(settings) => settings,
                Err(error) => {
                    report(PlanOutcome::Failed {
                        interface,
                        visible_error_message: error.to_string(),
                    });
                    return;
                }
            };
            let wifi = AutoWifi::with_policy_and_settings(interface.policy, settings);
            let attached = attach_with_access(handle, access, wifi);
            report_attached(handle, interface, attached.id(), attachments, report);
        }
        PlannedMedium::TcpClient {
            connection,
            framing,
        } => {
            let attached = attach_with_access(
                handle,
                access,
                TcpClientInterface::with_policy_and_connection_settings(
                    tcp_target(connection),
                    interface.policy,
                    *framing,
                    tcp_connection_settings(connection),
                ),
            );
            report_attached(handle, interface, attached.id(), attachments, report);
        }
        PlannedMedium::TcpServer { listener, framing } => {
            let resolved = resolve_tcp_listener(listener).await;
            let opened = match resolved {
                Ok(bind) => {
                    TcpServer::bind_with_policy_and_tunnel_and_framing(
                        bind,
                        interface.policy,
                        tcp_tunnel_mode(listener.tunnel),
                        *framing,
                    )
                    .await
                }
                Err(error) => Err(error),
            };
            match opened {
                Ok(server) => {
                    let attached = attach_with_access(handle, access, server);
                    report_attached(handle, interface, attached.id(), attachments, report);
                }
                Err(error) => report(PlanOutcome::Failed {
                    interface,
                    visible_error_message: error.to_string(),
                }),
            }
        }
        PlannedMedium::Udp { flow } => {
            let opened = match flow {
                UdpFlowPlan::ReceiveOnly { listen } => match resolve_udp_endpoint(listen).await {
                    Ok(listen) => {
                        UdpInterface::bind_receive_with_policy(listen, interface.policy).await
                    }
                    Err(error) => Err(error),
                },
                UdpFlowPlan::SendOnly { forward } => match resolve_udp_endpoint(forward).await {
                    Ok(forward) => {
                        UdpInterface::bind_send_with_policy(
                            udp_ephemeral_bind(),
                            forward,
                            interface.policy,
                        )
                        .await
                    }
                    Err(error) => Err(error),
                },
                UdpFlowPlan::Bidirectional { listen, forward } => {
                    match (
                        resolve_udp_endpoint(listen).await,
                        resolve_udp_endpoint(forward).await,
                    ) {
                        (Ok(listen), Ok(forward)) => {
                            UdpInterface::bind_with_policy(listen, forward, interface.policy).await
                        }
                        (Err(error), _) | (_, Err(error)) => Err(error),
                    }
                }
            };
            match opened {
                Ok(udp) => {
                    let attached = attach_with_access(handle, access, udp);
                    report_attached(handle, interface, attached.id(), attachments, report);
                }
                Err(error) => report(PlanOutcome::Failed {
                    interface,
                    visible_error_message: error.to_string(),
                }),
            }
        }
        PlannedMedium::Serial { device, line } => {
            let line = host_serial_line(*line);
            let open_path = device.clone();
            let serial = SerialInterface::with_policy(
                move || {
                    let open_path = open_path.clone();
                    async move { open_host_serial_with_settings(&open_path, line) }
                },
                SERIAL_RECONNECT_DELAY,
                interface.policy,
                device.as_bytes(),
            );
            let attached = attach_with_access(handle, access, serial);
            report_attached(handle, interface, attached.id(), attachments, report);
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
            let line = host_serial_line(*line);
            let open_path = device.clone();
            let tnc = TncConfig {
                preamble_ms: *preamble_ms,
                txtail_ms: *txtail_ms,
                persistence: *persistence,
                slottime_ms: *slottime_ms,
            };
            let station_identification =
                match runtime_station_identification(station_id, StationIdWireFormat::KissPadded) {
                    Ok(station_identification) => station_identification,
                    Err(message) => {
                        report(PlanOutcome::Failed {
                            interface,
                            visible_error_message: message,
                        });
                        return;
                    }
                };
            let kiss = KissInterface::with_runtime_settings(
                move || {
                    let open_path = open_path.clone();
                    async move { open_host_serial_with_settings(&open_path, line) }
                },
                SERIAL_RECONNECT_DELAY,
                KissSettings {
                    configure_delay: DEFAULT_TNC_CONFIGURE_DELAY,
                    tnc,
                    flow_control: runtime_kiss_flow_control(*flow_control),
                    station_identification,
                    policy: interface.policy,
                    channel_tag: device.as_bytes(),
                },
            );
            let attached = attach_with_access(handle, access, kiss);
            report_attached(handle, interface, attached.id(), attachments, report);
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
            let line = host_serial_line(*line);
            let open_path = device.clone();
            let tnc = TncConfig {
                preamble_ms: *preamble_ms,
                txtail_ms: *txtail_ms,
                persistence: *persistence,
                slottime_ms: *slottime_ms,
            };
            let opened = Ax25KissInterface::with_policy(
                move || {
                    let open_path = open_path.clone();
                    async move { open_host_serial_with_settings(&open_path, line) }
                },
                SERIAL_RECONNECT_DELAY,
                Ax25KissSettings {
                    configure_delay: DEFAULT_TNC_CONFIGURE_DELAY,
                    tnc,
                    flow_control: runtime_kiss_flow_control(*flow_control),
                    callsign,
                    ssid: *ssid,
                    policy: interface.policy,
                    channel_tag: device.as_bytes(),
                },
            );
            match opened {
                Ok(ax25) => {
                    let attached = attach_with_access(handle, access, ax25);
                    report_attached(handle, interface, attached.id(), attachments, report);
                }
                Err(error) => report(PlanOutcome::Failed {
                    interface,
                    visible_error_message: format!("{error:?}"),
                }),
            }
        }
        PlannedMedium::Rnode {
            transport,
            frequency_hz,
            bandwidth_hz,
            txpower_dbm,
            spreading_factor,
            coding_rate,
            flow_control,
            station_id,
            airtime_limit_short,
            airtime_limit_long,
        } => {
            match RadioConfig::new(RadioConfigInput {
                frequency_hz: *frequency_hz,
                bandwidth_hz: *bandwidth_hz,
                txpower_dbm: *txpower_dbm,
                spreading_factor: *spreading_factor,
                coding_rate: *coding_rate,
                airtime_limit_short_centi_percent: airtime_limit_short.map(|limit| limit.get()),
                airtime_limit_long_centi_percent: airtime_limit_long.map(|limit| limit.get()),
            }) {
                Ok(radio) => {
                    let station_identification = match runtime_station_identification(
                        station_id,
                        StationIdWireFormat::Exact,
                    ) {
                        Ok(station_identification) => station_identification,
                        Err(message) => {
                            report(PlanOutcome::Failed {
                                interface,
                                visible_error_message: message,
                            });
                            return;
                        }
                    };
                    let opener = RNodeHostOpener::new(transport.clone());
                    let channel_tag = transport.channel_tag();
                    let detect_timeout = opener.detect_timeout();
                    let keepalive = opener.keepalive();
                    let rnode = RNodeInterface::with_runtime_settings(
                        move || {
                            let opener = opener.clone();
                            async move { opener.open().await }
                        },
                        RNODE_RECONNECT_DELAY,
                        RNodeSettings {
                            reset_delay: crate::rnode::DEFAULT_RNODE_RESET_DELAY,
                            detect_timeout,
                            keepalive,
                            radio,
                            flow_control: runtime_rnode_flow_control(*flow_control),
                            station_identification,
                            policy: interface.policy,
                            channel_tag: &channel_tag,
                        },
                    );
                    let attached = attach_with_access(handle, access, rnode);
                    report_attached(handle, interface, attached.id(), attachments, report);
                }
                Err(error) => report(PlanOutcome::Failed {
                    interface,
                    visible_error_message: format!("{error:?}"),
                }),
            }
        }
        PlannedMedium::RnodeMulti { .. } => report(PlanOutcome::Failed {
            interface,
            visible_error_message: "RNodeMulti member was not grouped with its parent device"
                .to_string(),
        }),
        PlannedMedium::Backbone { listener } => {
            let opened = match resolve_tcp_listener(listener).await {
                Ok(bind) => BackboneServer::bind_with_policy(bind, interface.policy).await,
                Err(error) => Err(error),
            };
            match opened {
                Ok(server) => {
                    let attached = attach_with_access(handle, access, server);
                    report_attached(handle, interface, attached.id(), attachments, report);
                }
                Err(error) => report(PlanOutcome::Failed {
                    interface,
                    visible_error_message: error.to_string(),
                }),
            }
        }
        PlannedMedium::BackboneClient { connection } => {
            let attached = attach_with_access(
                handle,
                access,
                BackboneClientInterface::with_policy_and_connection_settings(
                    tcp_target(connection),
                    interface.policy,
                    tcp_connection_settings(connection),
                ),
            );
            report_attached(handle, interface, attached.id(), attachments, report);
        }
        PlannedMedium::Pipe {
            command,
            respawn_delay,
        } => {
            let respawn_delay = PipeRespawnDelay::new(respawn_delay.get());
            let argv = command.argv().to_vec();
            let pipe = PipeInterface::with_policy(
                move || {
                    let argv = argv.clone();
                    async move { crate::pipe_host::spawn(&argv).await }
                },
                respawn_delay,
                interface.policy,
                command.source().as_bytes(),
            );
            let attached = attach_with_access(handle, access, pipe);
            report_attached(handle, interface, attached.id(), attachments, report);
        }
        PlannedMedium::I2p {
            peers,
            reachability,
        } => {
            let config = match i2p_runtime_config(interface, peers, *reachability, context) {
                Ok(config) => config,
                Err(visible_error_message) => {
                    report(PlanOutcome::Failed {
                        interface,
                        visible_error_message,
                    });
                    return;
                }
            };
            let i2p = I2pInterface::new(TokioSamBridge::default(), config);
            let attached = attach_with_access(handle, access, i2p);
            report_attached(handle, interface, attached.id(), attachments, report);
        }
    }
}

fn auto_wifi_settings(
    interface_name: &str,
    planned: &AutoInterfacePlan,
) -> Result<AutoWifiSettings, crate::wifi::AutoWifiSettingsError> {
    let group_id = planned.group_id().as_bytes();
    let mut instance_tag = (group_id.len() as u64).to_be_bytes().to_vec();
    instance_tag.extend_from_slice(group_id);
    instance_tag.extend_from_slice(interface_name.as_bytes());
    AutoWifiSettings::new(
        group_id.to_vec(),
        planned.discovery_scope(),
        planned.multicast_address_type(),
        planned.discovery_port().get(),
        planned.data_port().get(),
        AutoWifiDevicePolicy::new(
            planned.devices().allowed().to_vec(),
            planned.devices().ignored().to_vec(),
        ),
    )
    .map(|settings| settings.with_instance_tag(instance_tag))
}

fn stand_up_rnode_multi<'a>(
    handle: &PrnsNodeHandle,
    interfaces: impl Iterator<Item = (&'a PlannedInterface, &'a RNodeMultiMemberPlan)>,
    attachments: &mut PlanAttachments,
    report: &mut impl FnMut(PlanOutcome<'a>),
) {
    let interfaces = interfaces.collect::<Vec<_>>();
    let Some((first, first_member)) = interfaces.first().copied() else {
        return;
    };
    let access = match runtime_access(first) {
        Ok(None) => RNodeMultiAccess::Open,
        Ok(Some((context, network_name))) => RNodeMultiAccess::Ifac {
            context: Box::new(context),
            network_name,
        },
        Err(visible_error_message) => {
            for (interface, _) in interfaces {
                report(PlanOutcome::Failed {
                    interface,
                    visible_error_message: visible_error_message.clone(),
                });
            }
            return;
        }
    };
    let station_plan = first_member.parent().station_id().cloned();
    let station_identification =
        match runtime_station_identification(&station_plan, StationIdWireFormat::Exact) {
            Ok(station_identification) => station_identification,
            Err(visible_error_message) => {
                for (interface, _) in interfaces {
                    report(PlanOutcome::Failed {
                        interface,
                        visible_error_message: visible_error_message.clone(),
                    });
                }
                return;
            }
        };
    let settings = interfaces
        .iter()
        .map(|(interface, member)| runtime_rnode_multi_member(interface, member, access.clone()))
        .collect::<Vec<_>>();
    let members = match RNodeMultiMembers::new(settings) {
        Ok(members) => members,
        Err(error) => {
            let visible_error_message = error.to_string();
            for (interface, _) in interfaces {
                report(PlanOutcome::Failed {
                    interface,
                    visible_error_message: visible_error_message.clone(),
                });
            }
            return;
        }
    };
    let parent = first_member.parent();
    let device = parent.device().to_string();
    let open_path = device.clone();
    let rnode_multi = RNodeMultiInterface::new(
        parent.name(),
        &device,
        move || {
            let open_path = open_path.clone();
            async move { open_host_serial(&open_path, RNODE_BAUD) }
        },
        RNodeMultiSettings {
            reconnect_delay: RNODE_RECONNECT_DELAY,
            reset_delay: crate::rnode::DEFAULT_RNODE_RESET_DELAY,
            configure_delay: DEFAULT_RNODE_MULTI_CONFIGURE_DELAY,
            station_identification,
            members,
        },
    );
    let ids = rnode_multi.member_ids().collect::<Vec<_>>();
    let registered = rnode_multi.register(handle);
    let task = tokio::spawn(registered.run());
    attachments.push_supervisor(first.lifecycle, ids.clone(), task);
    for ((interface, _), id) in interfaces.into_iter().zip(ids) {
        report_up(handle, interface, id, report);
    }
}

fn runtime_rnode_multi_member(
    interface: &PlannedInterface,
    member: &RNodeMultiMemberPlan,
    access: RNodeMultiAccess,
) -> RNodeMultiMemberSettings {
    RNodeMultiMemberSettings::new(
        interface.name.clone(),
        member.vport(),
        member.radio(),
        runtime_rnode_flow_control(member.flow_control()),
        interface.policy,
        access,
        member.parent().device().as_bytes(),
    )
}

fn runtime_access(
    interface: &PlannedInterface,
) -> Result<Option<(IfacContext, Option<String>)>, String> {
    match &interface.access {
        InterfaceAccessPlan::Open => Ok(None),
        InterfaceAccessPlan::Ifac {
            network_name,
            passphrase,
            size,
        } => IfacContext::derive(network_name.as_deref(), passphrase.as_deref(), *size)
            .map(|context| Some((context, network_name.clone())))
            .ok_or_else(|| "IFAC requires a network name or passphrase".to_string()),
    }
}

fn i2p_runtime_config(
    interface: &PlannedInterface,
    planned_peers: &I2pPeersPlan,
    planned_reachability: PlannedI2pReachability,
    context: &PlanRuntimeContext,
) -> Result<I2pRuntimeConfig, String> {
    let name = I2pInterfaceName::new(interface.name.clone()).map_err(|error| error.to_string())?;
    let peers = planned_peers
        .iter()
        .map(|peer| I2pPeerAddress::new(peer.as_str()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    let peers = I2pPeers::new(peers).map_err(|error| error.to_string())?;
    let reachability = match planned_reachability {
        PlannedI2pReachability::OutboundOnly => I2pReachability::OutboundOnly,
        PlannedI2pReachability::Connectable => {
            let storage = context.i2p_storage.as_ref().ok_or_else(|| {
                "connectable I2P requires the daemon's RNS storage directory and transport identity"
                    .to_string()
            })?;
            I2pReachability::Connectable {
                key_path: storage.destination_key_path(&name),
            }
        }
    };
    Ok(I2pRuntimeConfig {
        name,
        peers,
        reachability,
        policy: interface.policy,
        retry: I2pRetryPolicy::STOCK,
    })
}

fn host_serial_line(line: SerialLinePlan) -> HostSerialLineSettings {
    HostSerialLineSettings::new(
        line.baud(),
        match line.data_bits() {
            SerialDataBits::Five => HostSerialDataBits::Five,
            SerialDataBits::Six => HostSerialDataBits::Six,
            SerialDataBits::Seven => HostSerialDataBits::Seven,
            SerialDataBits::Eight => HostSerialDataBits::Eight,
        },
        match line.parity() {
            SerialParity::None => HostSerialParity::None,
            SerialParity::Even => HostSerialParity::Even,
            SerialParity::Odd => HostSerialParity::Odd,
        },
        match line.stop_bits() {
            SerialStopBits::One => HostSerialStopBits::One,
            SerialStopBits::Two => HostSerialStopBits::Two,
        },
    )
}

fn runtime_kiss_flow_control(planned: PlannedReadyCommandFlowControl) -> ReadyCommandFlowControl {
    match planned {
        PlannedReadyCommandFlowControl::Disabled => ReadyCommandFlowControl::Disabled,
        PlannedReadyCommandFlowControl::Enabled => {
            ReadyCommandFlowControl::WaitForReadyOrTimeout(KISS_FLOW_CONTROL_TIMEOUT)
        }
    }
}

fn runtime_rnode_flow_control(planned: PlannedReadyCommandFlowControl) -> ReadyCommandFlowControl {
    match planned {
        PlannedReadyCommandFlowControl::Disabled => ReadyCommandFlowControl::Disabled,
        PlannedReadyCommandFlowControl::Enabled => ReadyCommandFlowControl::WaitForReady,
    }
}

fn runtime_station_identification(
    planned: &Option<StationIdentificationPlan>,
    wire_format: StationIdWireFormat,
) -> Result<Option<StationIdentification>, String> {
    planned
        .as_ref()
        .map(|planned| {
            StationIdentification::new(
                planned.callsign().as_bytes(),
                StationIdInterval::new(Duration::from_secs(planned.interval_seconds())),
                wire_format,
            )
            .map_err(|_| "station identification callsign cannot be empty".to_string())
        })
        .transpose()
}

fn tcp_connection_settings(plan: &TcpDialPlan) -> TcpConnectionSettings {
    TcpConnectionSettings {
        connect_timeout: Duration::from_secs(plan.connect_timeout.get()),
        reconnect_wait: TCP_RECONNECT_DELAY,
        reconnect_limit: match plan.reconnect_limit {
            PlannedReconnectLimit::Unlimited => ReconnectLimit::Unlimited,
            PlannedReconnectLimit::Attempts(attempts) => ReconnectLimit::Attempts(attempts),
        },
        address_family: match plan.address_family {
            PlannedAddressFamilyPreference::System => AddressFamilyPreference::System,
            PlannedAddressFamilyPreference::Ipv4 => AddressFamilyPreference::Ipv4,
            PlannedAddressFamilyPreference::Ipv6 => AddressFamilyPreference::Ipv6,
        },
        tunnel: tcp_tunnel_mode(plan.tunnel),
    }
}

const fn tcp_tunnel_mode(mode: PlannedTcpTunnelMode) -> TcpTunnelMode {
    match mode {
        PlannedTcpTunnelMode::Direct => TcpTunnelMode::Direct,
        PlannedTcpTunnelMode::I2p => TcpTunnelMode::I2p,
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

fn report_attached<'a>(
    handle: &PrnsNodeHandle,
    interface: &'a PlannedInterface,
    id: InterfaceId,
    attachments: &mut PlanAttachments,
    report: &mut impl FnMut(PlanOutcome<'a>),
) {
    attachments.push_interface(interface.lifecycle, id);
    report_up(handle, interface, id, report);
}

fn attach_with_access<A: Attachable>(
    handle: &PrnsNodeHandle,
    access: Option<(IfacContext, Option<String>)>,
    attachable: A,
) -> A::Attached {
    match access {
        None => handle.attach(attachable),
        Some((context, network_name)) => {
            handle.attach_with_ifac_name(attachable, context, network_name)
        }
    }
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planned_serial_line_reaches_the_host_transport_without_defaulting() {
        let plan = prns_config::parse_and_plan(
            "[interfaces]\n[[Serial]]\ntype = SerialInterface\nenabled = Yes\nport = test\nspeed = 57600\ndatabits = 7\nparity = odd\nstopbits = 2\n",
        )
        .expect("valid serial configuration")
        .value;
        let PlannedMedium::Serial { line, .. } = &plan.interfaces[0].medium else {
            panic!("serial medium expected")
        };
        let host = host_serial_line(*line);
        assert_eq!(host.baud(), 57_600);
        assert_eq!(host.data_bits(), HostSerialDataBits::Seven);
        assert_eq!(host.parity(), HostSerialParity::Odd);
        assert_eq!(host.stop_bits(), HostSerialStopBits::Two);
    }

    #[test]
    fn planned_auto_interface_settings_cross_the_runtime_boundary_without_defaulting() {
        let plan = prns_config::parse_and_plan(
            "[interfaces]\n[[Mesh]]\ntype = AutoInterface\nenabled = Yes\ngroup_id = field-mesh\n\
             discovery_scope = organisation\nmulticast_address_type = permanent\ndiscovery_port = 31000\n\
             data_port = 32000\ndevices = en0, wlan0\nignored_devices = wlan0\n",
        )
        .expect("valid AutoInterface configuration")
        .value;
        let PlannedMedium::AutoWifi(planned) = &plan.interfaces[0].medium else {
            panic!("AutoInterface medium expected")
        };

        let settings = auto_wifi_settings(&plan.interfaces[0].name, planned)
            .expect("typed plan maps to runtime settings");

        assert_eq!(settings.group_id(), b"field-mesh");
        assert_eq!(
            settings.discovery_scope(),
            prns_core::interfaces::wifi_auto::core::DiscoveryScope::Organisation
        );
        assert_eq!(
            settings.multicast_address_type(),
            prns_core::interfaces::wifi_auto::core::MulticastAddressType::Permanent
        );
        assert_eq!(settings.discovery_port(), 31_000);
        assert_eq!(settings.data_port(), 32_000);
        assert_eq!(settings.devices().allowed(), &["en0", "wlan0"]);
        assert_eq!(settings.devices().ignored(), &["wlan0"]);
    }

    #[tokio::test]
    async fn planned_rnode_multi_members_register_once_under_one_device_supervisor() {
        use prns_core::interfaces::ConnectionState;
        use prns_runtime::runtime::{Manual, PreConfiguredDestination, PrnsNode, PrnsNodeRecipe};
        use prns_runtime::storage::GrowableHeap;

        let plan = prns_config::parse_and_plan(
            "[interfaces]\n[[Dual]]\ntype = RNodeMultiInterface\nenabled = Yes\nbootstrap_only = Yes\nport = test\n\
             [[[Low]]]\ninterface_enabled = Yes\nvport = 0\nfrequency = 868000000\n\
             bandwidth = 125000\ntxpower = 7\nspreadingfactor = 8\ncodingrate = 5\n\
             [[[High]]]\ninterface_enabled = Yes\nvport = 1\nfrequency = 2400000000\n\
             bandwidth = 812500\ntxpower = 10\nspreadingfactor = 7\ncodingrate = 6\n",
        )
        .expect("valid RNodeMulti configuration")
        .value;
        let node = PrnsNode::new(PrnsNodeRecipe {
            transport_identity: None,
            pre_configured_destinations: std::iter::empty::<PreConfiguredDestination<'static>>(),
            app_state: (),
            storage: GrowableHeap,
            routes: prns_runtime::routes![],
            interfaces: Manual,
            on_event: |_event, _state: &()| {},
        });
        let mut outcomes = Vec::new();
        let attachments = attach_plan(&node.handle(), &plan, &mut |outcome| match outcome {
            PlanOutcome::Up { interface, id } => {
                outcomes.push((interface.name.clone(), Some(id)));
            }
            PlanOutcome::Failed { interface, .. } => {
                outcomes.push((interface.name.clone(), None));
            }
        })
        .await;

        assert_eq!(outcomes.len(), 2);
        assert_eq!(outcomes[0].0, "Dual[Low]");
        assert_eq!(outcomes[1].0, "Dual[High]");
        assert!(outcomes.iter().all(|(_, id)| id.is_some()));
        assert_ne!(outcomes[0].1, outcomes[1].1);
        let registered = node.handle().interfaces();
        assert_eq!(registered.len(), 2);
        assert!(registered
            .iter()
            .all(|member| member.connection == ConnectionState::Initializing));
        assert_eq!(attachments.groups.len(), 1);
        assert_eq!(attachments.groups[0].interfaces.len(), 2);
        assert_eq!(
            attachments.groups[0].lifecycle,
            ConfiguredInterfaceLifecycle::BootstrapOnly
        );
        assert!(attachments.groups[0].supervisor_task.is_some());
        attachments.detach(&node.handle()).await;
    }

    #[test]
    fn planned_i2p_peers_cross_the_runtime_boundary_without_reinterpretation() {
        let plan = prns_config::parse_and_plan(
            "[interfaces]\n[[Private I2P]]\ntype = I2PInterface\nenabled = Yes\npeers = example.i2p, QUJDRA==\n",
        )
        .expect("valid I2P configuration")
        .value;
        let interface = &plan.interfaces[0];
        let PlannedMedium::I2p {
            peers,
            reachability,
        } = &interface.medium
        else {
            panic!("I2P medium expected")
        };

        let config = i2p_runtime_config(
            interface,
            peers,
            *reachability,
            &PlanRuntimeContext::default(),
        )
        .expect("the typed plan converts to runtime types");

        assert_eq!(
            config
                .peers
                .iter()
                .map(I2pPeerAddress::as_str)
                .collect::<Vec<_>>(),
            vec!["example.i2p", "QUJDRA=="]
        );
        assert_eq!(config.reachability, I2pReachability::OutboundOnly);
        assert_eq!(config.policy, interface.policy);
        assert_eq!(config.retry, I2pRetryPolicy::STOCK);
    }

    #[test]
    fn connectable_i2p_requires_host_runtime_context() {
        let plan = prns_config::parse_and_plan(
            "[interfaces]\n[[Private I2P]]\ntype = I2PInterface\nenabled = Yes\nconnectable = Yes\n",
        )
        .expect("valid I2P configuration")
        .value;
        let interface = &plan.interfaces[0];
        let PlannedMedium::I2p {
            peers,
            reachability,
        } = &interface.medium
        else {
            panic!("I2P medium expected")
        };

        let error = i2p_runtime_config(
            interface,
            peers,
            *reachability,
            &PlanRuntimeContext::default(),
        )
        .expect_err("connectable I2P needs persistent host context");

        assert!(error.contains("RNS storage directory and transport identity"));
    }

    #[test]
    fn connectable_i2p_uses_the_host_supplied_stock_key_scope() {
        let plan = prns_config::parse_and_plan(
            "[interfaces]\n[[Private I2P]]\ntype = I2PInterface\nenabled = Yes\nconnectable = Yes\n",
        )
        .expect("valid I2P configuration")
        .value;
        let interface = &plan.interfaces[0];
        let PlannedMedium::I2p {
            peers,
            reachability,
        } = &interface.medium
        else {
            panic!("I2P medium expected")
        };
        let context = PlanRuntimeContext::with_rns_i2p_storage(
            "/var/lib/reticulum/storage",
            IdentityHash::new([
                0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
                0xee, 0xff,
            ]),
        );

        let config = i2p_runtime_config(interface, peers, *reachability, &context)
            .expect("the daemon context completes connectable I2P");
        let I2pReachability::Connectable { key_path } = config.reachability else {
            panic!("connectable runtime expected")
        };

        assert_eq!(
            key_path.as_path(),
            std::path::Path::new(
                "/var/lib/reticulum/storage/i2p/4c621c0110154bbe086a0395dbeb07878a1613258d5e0346c96ddef1a5aeae2d.i2p"
            )
        );
    }

    #[test]
    fn config_peer_validation_matches_runtime_peer_types() {
        for peer in [
            "example.i2p",
            "52chars.b32.i2p",
            "QUJDRA==",
            "EXAMPLE.I2P",
            "abc",
            "A=AA",
            "not a peer",
            "",
        ] {
            let config = format!(
                "[interfaces]\n[[Private I2P]]\ntype = I2PInterface\nenabled = Yes\npeers = {peer}\n"
            );
            assert_eq!(
                prns_config::parse_and_plan(&config).is_ok(),
                I2pPeerAddress::new(peer).is_ok(),
                "config and runtime must agree for {peer:?}"
            );
        }
    }
}
