use personal_rns::config::{
    DaemonPlan, SharedInstance, SharedInstanceTransport as ConfigSharedInstanceTransport,
};
use personal_rns::from_plan::PlanRuntimeContext;
use personal_rns::identity::IdentityHash;
use personal_rns::routing::announce::derive_single_destination_hash;
use personal_rns::runtime::PrnsNodeHandle;
use personal_rns::shared_instance::{
    join_shared_instance, ExistingSharedInstancePolicy, RnsBlackholeFiles,
    SharedInstanceCredentials, SharedInstanceIntent, SharedInstanceJoinError, SharedInstancePorts,
    SharedInstanceRole, SharedInstanceTransport as RuntimeSharedInstanceTransport,
};

use super::configured_interfaces::{
    self, ActiveInterfaceUnit, ConstructedInterfaces, StartupInterfaceReport,
};

pub(super) struct InterfaceOwnership {
    startup: StartupInterfaceReport,
    routing_tables: Option<RoutingTableOwnership>,
}

pub(super) struct RoutingTableOwnership {
    pub(super) configured_units: Vec<ActiveInterfaceUnit>,
}

impl InterfaceOwnership {
    pub(super) fn startup(&self) -> StartupInterfaceReport {
        self.startup
    }

    pub(super) fn routing_tables(&self) -> Option<&RoutingTableOwnership> {
        self.routing_tables.as_ref()
    }

    pub(super) fn into_routing_tables(self) -> Option<RoutingTableOwnership> {
        self.routing_tables
    }

    fn routing_table_owner(constructed: ConstructedInterfaces) -> Self {
        Self {
            startup: constructed.startup,
            routing_tables: Some(RoutingTableOwnership {
                configured_units: constructed.units,
            }),
        }
    }

    fn shared_client() -> Self {
        Self {
            startup: StartupInterfaceReport {
                online: 1,
                ..StartupInterfaceReport::default()
            },
            routing_tables: None,
        }
    }
}

pub(super) async fn establish(
    handle: &PrnsNodeHandle,
    plan: &DaemonPlan,
    context: &PlanRuntimeContext,
    credentials: &SharedInstanceCredentials,
    transport_identity: IdentityHash,
    network_identity: Option<IdentityHash>,
    blackhole_files: &RnsBlackholeFiles,
) -> Result<InterfaceOwnership, SharedInstanceJoinError> {
    match &plan.shared_instance {
        SharedInstance::Enabled {
            name,
            transport,
            instance_port,
            control_port,
            forced_bitrate,
            ..
        } => {
            let ports = SharedInstancePorts {
                bus: *instance_port,
                control: *control_port,
            };
            let runtime_transport = match transport {
                ConfigSharedInstanceTransport::Tcp => RuntimeSharedInstanceTransport::Tcp,
                ConfigSharedInstanceTransport::Unix => {
                    #[cfg(any(target_os = "linux", target_os = "android"))]
                    {
                        RuntimeSharedInstanceTransport::AbstractUnix {
                            socket_path: name.clone(),
                        }
                    }
                    #[cfg(not(any(target_os = "linux", target_os = "android")))]
                    {
                        tracing::warn!(
                            event = "shared_instance_unix_fallback",
                            configured_name = %name,
                            fallback = "tcp",
                        );
                        RuntimeSharedInstanceTransport::Tcp
                    }
                }
            };
            let policy = personal_rns::interfaces::shared_instance::configured_policy(
                personal_rns::interfaces::ConfiguredInterfacePolicy {
                    bitrate: *forced_bitrate,
                    ..Default::default()
                },
            );
            match join_shared_instance(
                handle,
                SharedInstanceIntent {
                    credentials: credentials.clone(),
                    transport_identity,
                    network_identity,
                    probe_responder: plan.probe_responder.is_enabled().then(|| {
                        derive_single_destination_hash(
                            &transport_identity,
                            "rnstransport",
                            &["probe"],
                        )
                        .expect("rnstransport.probe is a valid destination name")
                    }),
                    blackhole_source: transport_identity,
                    blackhole_files: blackhole_files.clone(),
                    ports,
                    transport: runtime_transport,
                    policy,
                    on_existing: ExistingSharedInstancePolicy::JoinAsClient,
                },
            )
            .await?
            {
                SharedInstanceRole::BecameInstance => {
                    tracing::info!(
                        event = "shared_instance_started",
                        bus_port = ports.bus,
                        control_port = ports.control,
                        instance_name = %name,
                    );
                    let constructed = configured_interfaces::construct(handle, plan, context).await;
                    let mut ownership = InterfaceOwnership::routing_table_owner(constructed);
                    ownership.startup.listening = ownership.startup.listening.saturating_add(1);
                    Ok(ownership)
                }
                SharedInstanceRole::JoinedAsClient { of } => {
                    tracing::info!(event = "shared_instance_joined");
                    tracing::debug!(event = "shared_instance_joined_detail", instance = %of);
                    Ok(InterfaceOwnership::shared_client())
                }
            }
        }
        SharedInstance::Disabled => {
            tracing::info!(event = "standalone_node_started");
            let constructed = configured_interfaces::construct(handle, plan, context).await;
            Ok(InterfaceOwnership::routing_table_owner(constructed))
        }
    }
}

pub(super) fn report_join_error(error: &SharedInstanceJoinError) {
    match error {
        SharedInstanceJoinError::InstanceAlreadyRunning { at } => {
            tracing::error!(event = "shared_instance_refused", endpoint = %at);
        }
        SharedInstanceJoinError::EndpointUnavailable { endpoint, kind } => {
            tracing::error!(
                event = "shared_instance_endpoint_unavailable",
                endpoint = endpoint.as_str(),
                error_kind = ?kind,
            );
        }
    }
}
