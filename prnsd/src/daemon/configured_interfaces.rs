use personal_rns::config::{
    ConfiguredInterfaceLifecycle, DaemonPlan, DiscoveryPublicationProblem, InterfaceDiscoveryPlan,
    PlannedInterface, PlannedMedium,
};
use personal_rns::from_plan::{
    attach_plan_with_context, PlanAttachments, PlanOutcome, PlanRuntimeContext,
};
use personal_rns::interfaces::{InterfaceId, InterfaceOriginKind};
use personal_rns::runtime::PrnsNodeHandle;

use crate::interface_discovery::MonitoredInterfaces;

#[derive(Clone)]
pub(crate) struct AttachedConfiguredInterface {
    pub(crate) id: InterfaceId,
    pub(crate) plan: PlannedInterface,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct StartupInterfaceReport {
    pub(crate) online: u32,
    pub(crate) listening: u32,
    pub(crate) retrying: u32,
    pub(crate) failed: u32,
}

impl StartupInterfaceReport {
    pub(crate) fn merge(&mut self, other: Self) {
        self.online = self.online.saturating_add(other.online);
        self.listening = self.listening.saturating_add(other.listening);
        self.retrying = self.retrying.saturating_add(other.retrying);
        self.failed = self.failed.saturating_add(other.failed);
    }

    pub(crate) const fn degraded(self) -> bool {
        self.retrying != 0 || self.failed != 0
    }
}

#[derive(Default)]
pub(crate) struct ConstructedInterfaces {
    pub(crate) units: Vec<ActiveInterfaceUnit>,
    pub(crate) startup: StartupInterfaceReport,
}

pub(crate) struct ActiveInterfaceUnit {
    key: InterfaceUnitKey,
    plan: Vec<PlannedInterface>,
    pub(crate) attached: Vec<AttachedConfiguredInterface>,
    pub(crate) runtime: PlanAttachments,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum InterfaceUnitKey {
    Named(String),
    RnodeMulti { name: String, device: String },
}

#[derive(Clone)]
struct InterfaceUnitSpec {
    key: InterfaceUnitKey,
    plan: Vec<PlannedInterface>,
}

pub(crate) struct ConfiguredInterfaceManager {
    units: Vec<ActiveInterfaceUnit>,
    monitored: MonitoredInterfaces,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReconcileResult {
    Applied,
    Unchanged,
    RolledBack { rollback_failed: bool },
}

pub(crate) async fn construct(
    handle: &PrnsNodeHandle,
    plan: &DaemonPlan,
    context: &PlanRuntimeContext,
) -> ConstructedInterfaces {
    construct_specs(handle, plan, context, unit_specs(&plan.interfaces)).await
}

impl ConstructedInterfaces {
    pub(crate) fn attached(&self) -> Vec<AttachedConfiguredInterface> {
        self.units
            .iter()
            .flat_map(|unit| unit.attached.iter().cloned())
            .collect()
    }

    pub(crate) fn into_attachments(self) -> PlanAttachments {
        attachments_from_units(self.units)
    }
}

impl ConfiguredInterfaceManager {
    pub(crate) fn new(units: Vec<ActiveInterfaceUnit>, monitored: MonitoredInterfaces) -> Self {
        Self { units, monitored }
    }

    pub(crate) fn attached(&self) -> Vec<AttachedConfiguredInterface> {
        self.units
            .iter()
            .flat_map(|unit| unit.attached.iter().cloned())
            .collect()
    }

    pub(crate) async fn reconcile(
        &mut self,
        handle: &PrnsNodeHandle,
        plan: &DaemonPlan,
        context: &PlanRuntimeContext,
    ) -> ReconcileResult {
        let mut requested = unit_specs(&plan.interfaces)
            .into_iter()
            .filter(|spec| {
                spec.plan.first().is_some_and(|interface| {
                    interface.lifecycle == ConfiguredInterfaceLifecycle::Persistent
                })
            })
            .collect::<Vec<_>>();
        let unchanged = self.units.len() == requested.len()
            && self.units.iter().all(|unit| {
                requested
                    .iter()
                    .any(|spec| spec.key == unit.key && spec.plan == unit.plan)
            });
        if unchanged {
            return ReconcileResult::Unchanged;
        }
        let mut retained = Vec::new();
        let mut replaced = Vec::new();
        for unit in std::mem::take(&mut self.units) {
            if let Some(index) = requested
                .iter()
                .position(|spec| spec.key == unit.key && spec.plan == unit.plan)
            {
                requested.remove(index);
                retained.push(unit);
            } else {
                self.monitored
                    .remove(unit.attached.iter().map(|interface| interface.id));
                replaced.push(InterfaceUnitSpec {
                    key: unit.key.clone(),
                    plan: unit.plan.clone(),
                });
                unit.runtime.detach(handle).await;
            }
        }
        let mut replacements = construct_specs(handle, plan, context, requested).await;
        if replacements.startup.failed == 0 {
            self.monitored.add(
                replacements
                    .units
                    .iter()
                    .flat_map(|unit| unit.attached.iter().map(|interface| interface.id)),
            );
            retained.append(&mut replacements.units);
            self.units = retained;
            return ReconcileResult::Applied;
        }
        for unit in replacements.units {
            unit.runtime.detach(handle).await;
        }
        let mut restored = construct_specs(handle, plan, context, replaced).await;
        let rollback_failed = restored.startup.failed != 0;
        self.monitored.add(
            restored
                .units
                .iter()
                .flat_map(|unit| unit.attached.iter().map(|interface| interface.id)),
        );
        retained.append(&mut restored.units);
        self.units = retained;
        ReconcileResult::RolledBack { rollback_failed }
    }
}

pub(crate) fn attached_from_units(
    units: &[ActiveInterfaceUnit],
) -> Vec<AttachedConfiguredInterface> {
    units
        .iter()
        .flat_map(|unit| unit.attached.iter().cloned())
        .collect()
}

pub(super) fn partition_units(
    units: Vec<ActiveInterfaceUnit>,
) -> (Vec<ActiveInterfaceUnit>, Vec<ActiveInterfaceUnit>) {
    units.into_iter().partition(|unit| {
        unit.plan.first().is_some_and(|interface| {
            interface.lifecycle == ConfiguredInterfaceLifecycle::Persistent
        })
    })
}

pub(crate) fn attachments_from_units(units: Vec<ActiveInterfaceUnit>) -> PlanAttachments {
    let mut attachments = PlanAttachments::default();
    for unit in units {
        attachments.append(unit.runtime);
    }
    attachments
}

async fn construct_specs(
    handle: &PrnsNodeHandle,
    plan: &DaemonPlan,
    context: &PlanRuntimeContext,
    specs: Vec<InterfaceUnitSpec>,
) -> ConstructedInterfaces {
    let mut tasks = tokio::task::JoinSet::new();
    for spec in specs {
        let handle = handle.clone();
        let mut unit_plan = plan.clone();
        unit_plan.interfaces = spec.plan.clone();
        let context = context.clone();
        tasks.spawn(async move { construct_unit(&handle, &unit_plan, &context, spec).await });
    }
    let mut constructed = ConstructedInterfaces::default();
    while let Some(result) = tasks.join_next().await {
        match result {
            Ok((unit, startup)) => {
                constructed.startup.merge(startup);
                constructed.units.push(unit);
            }
            Err(error) => {
                constructed.startup.failed = constructed.startup.failed.saturating_add(1);
                tracing::error!(event = "interface_start_task_failed", error = %error);
            }
        }
    }
    constructed
}

async fn construct_unit(
    handle: &PrnsNodeHandle,
    plan: &DaemonPlan,
    context: &PlanRuntimeContext,
    spec: InterfaceUnitSpec,
) -> (ActiveInterfaceUnit, StartupInterfaceReport) {
    let mut startup = StartupInterfaceReport::default();
    let mut attached = Vec::new();
    let runtime = attach_plan_with_context(handle, plan, context, &mut |outcome| {
        startup.merge(classify(&outcome));
        if let PlanOutcome::Up { interface, id } = &outcome {
            attached.push(AttachedConfiguredInterface {
                id: *id,
                plan: (*interface).clone(),
            });
        }
        render(outcome);
    })
    .await;
    (
        ActiveInterfaceUnit {
            key: spec.key,
            plan: spec.plan,
            attached,
            runtime,
        },
        startup,
    )
}

fn unit_specs(interfaces: &[PlannedInterface]) -> Vec<InterfaceUnitSpec> {
    let mut specs: Vec<InterfaceUnitSpec> = Vec::new();
    for interface in interfaces {
        let key = match &interface.medium {
            PlannedMedium::RnodeMulti { member } => InterfaceUnitKey::RnodeMulti {
                name: member.parent().name().to_string(),
                device: member.parent().device().to_string(),
            },
            _ => InterfaceUnitKey::Named(interface.name.clone()),
        };
        if let Some(spec) = specs.iter_mut().find(|spec| spec.key == key) {
            spec.plan.push(interface.clone());
        } else {
            specs.push(InterfaceUnitSpec {
                key,
                plan: vec![interface.clone()],
            });
        }
    }
    specs
}

fn classify(outcome: &PlanOutcome<'_>) -> StartupInterfaceReport {
    let mut report = StartupInterfaceReport::default();
    match outcome {
        PlanOutcome::Up { interface, .. } => match &interface.medium {
            PlannedMedium::TcpServer { .. }
            | PlannedMedium::Backbone { .. }
            | PlannedMedium::PrnsWebSocketServer { .. } => {
                report.listening = 1;
            }
            PlannedMedium::AutoWifi(_)
            | PlannedMedium::Udp { .. }
            | PlannedMedium::PrnsUsbAuto
            | PlannedMedium::PrnsBluetoothAuto { .. } => report.online = 1,
            PlannedMedium::I2p {
                peers,
                reachability,
            } if peers.is_empty() && !reachability.is_connectable() => report.online = 1,
            PlannedMedium::TcpClient { .. }
            | PlannedMedium::Serial { .. }
            | PlannedMedium::Kiss { .. }
            | PlannedMedium::Ax25Kiss { .. }
            | PlannedMedium::Rnode { .. }
            | PlannedMedium::RnodeMulti { .. }
            | PlannedMedium::BackboneClient { .. }
            | PlannedMedium::Pipe { .. }
            | PlannedMedium::I2p { .. }
            | PlannedMedium::Weave { .. }
            | PlannedMedium::PrnsWebSocketClient { .. } => report.retrying = 1,
        },
        PlanOutcome::Failed { .. } => report.failed = 1,
    }
    report
}

fn render(outcome: PlanOutcome<'_>) {
    match outcome {
        PlanOutcome::Up { interface, id } => {
            tracing::info!(
                event = "interface_started",
                interface_origin = InterfaceOriginKind::Configured.as_str(),
                interface = ?id.as_bytes(),
                interface_name = ?interface.name,
                medium = medium_name(&interface.medium),
            );
            match &interface.discovery {
                InterfaceDiscoveryPlan::Disabled | InterfaceDiscoveryPlan::Announce(_) => {}
                InterfaceDiscoveryPlan::Unpublishable(
                    DiscoveryPublicationProblem::UnsupportedInterfaceType,
                ) => {
                    tracing::warn!(
                        event = "interface_discovery_publication_unavailable",
                        interface_origin = InterfaceOriginKind::Configured.as_str(),
                        interface = ?id.as_bytes(),
                        interface_name = %interface.name,
                        reason = "unsupported_interface_type",
                    );
                }
                InterfaceDiscoveryPlan::Unpublishable(
                    DiscoveryPublicationProblem::MissingRequiredSetting { key },
                ) => {
                    tracing::warn!(
                        event = "interface_discovery_publication_unavailable",
                        interface_origin = InterfaceOriginKind::Configured.as_str(),
                        interface = ?id.as_bytes(),
                        interface_name = %interface.name,
                        reason = "missing_required_setting",
                        setting = *key,
                    );
                }
                InterfaceDiscoveryPlan::Unpublishable(
                    DiscoveryPublicationProblem::IncompatibleSetting { key },
                ) => {
                    tracing::warn!(
                        event = "interface_discovery_publication_unavailable",
                        interface_origin = InterfaceOriginKind::Configured.as_str(),
                        interface = ?id.as_bytes(),
                        interface_name = %interface.name,
                        reason = "incompatible_setting",
                        setting = *key,
                    );
                }
            }
        }
        PlanOutcome::Failed { interface, error } => {
            tracing::warn!(
                event = "interface_start_failed",
                interface_origin = InterfaceOriginKind::Configured.as_str(),
                medium = medium_name(&interface.medium),
            );
            tracing::debug!(
                event = "interface_start_failed_detail",
                interface_origin = InterfaceOriginKind::Configured.as_str(),
                interface_name = ?interface.name,
                interface = ?interface.medium,
                error = %error,
            );
        }
    }
}

fn medium_name(medium: &PlannedMedium) -> &'static str {
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

#[cfg(test)]
mod tests {
    use super::{
        classify, construct, unit_specs, ConfiguredInterfaceManager, PlanOutcome, ReconcileResult,
        StartupInterfaceReport,
    };
    use personal_rns::config::parse_and_plan;
    use personal_rns::interfaces::InterfaceId;
    use personal_rns::runtime::{
        ManuallyAttached, NoPersistence, PreConfiguredDestination, PrnsNode, PrnsNodeRecipe,
    };
    use personal_rns::storage::GrowableHeap;

    use crate::interface_discovery::MonitoredInterfaces;

    macro_rules! test_node {
        () => {
            PrnsNode::new(PrnsNodeRecipe {
                transport_identity: None,
                remote_control: crate::test_support::remote_control_service(),
                pre_configured_destinations: std::iter::empty::<PreConfiguredDestination<'static>>(
                ),
                app_state: (),
                storage: GrowableHeap,
                request_endpoints: personal_rns::request_endpoints![],
                interfaces: ManuallyAttached,
                persistence: NoPersistence,
                on_event: |_event, _state: &()| {},
            })
        };
    }

    fn listener(name: &str, port: u16) -> personal_rns::config::DaemonPlan {
        parse_and_plan(&format!(
            "[interfaces]\n[[{name}]]\ntype = TCPServerInterface\ninterface_enabled = Yes\nlisten_ip = 127.0.0.1\nlisten_port = {port}\n"
        ))
        .unwrap_or_else(|error| panic!("{error}"))
        .value
    }

    fn free_port() -> u16 {
        std::net::TcpListener::bind(("127.0.0.1", 0))
            .unwrap_or_else(|error| panic!("{error}"))
            .local_addr()
            .unwrap_or_else(|error| panic!("{error}"))
            .port()
    }

    #[test]
    fn startup_counts_merge_and_expose_degraded_readiness() {
        let mut report = StartupInterfaceReport {
            online: 2,
            listening: 1,
            retrying: 0,
            failed: 0,
        };
        assert!(!report.degraded());
        report.merge(StartupInterfaceReport {
            retrying: 1,
            failed: 1,
            ..StartupInterfaceReport::default()
        });
        assert_eq!(report.online, 2);
        assert_eq!(report.listening, 1);
        assert_eq!(report.retrying, 1);
        assert_eq!(report.failed, 1);
        assert!(report.degraded());
    }

    #[test]
    fn idle_i2p_is_ready_while_active_i2p_starts_retrying() {
        let idle = parse_and_plan("[interfaces]\n[[Idle]]\ntype = I2PInterface\nenabled = Yes\n")
            .expect("idle I2P configuration is valid")
            .value;
        let active = parse_and_plan(
            "[interfaces]\n[[Active]]\ntype = I2PInterface\nenabled = Yes\npeers = example.i2p\n",
        )
        .expect("active I2P configuration is valid")
        .value;
        let id = InterfaceId::new([0; 8]);

        assert_eq!(
            classify(&PlanOutcome::Up {
                interface: &idle.interfaces[0],
                id,
            }),
            StartupInterfaceReport {
                online: 1,
                ..StartupInterfaceReport::default()
            }
        );
        assert_eq!(
            classify(&PlanOutcome::Up {
                interface: &active.interfaces[0],
                id,
            }),
            StartupInterfaceReport {
                retrying: 1,
                ..StartupInterfaceReport::default()
            }
        );
    }

    #[test]
    fn every_rnode_multi_radio_is_counted_before_degraded_readiness() {
        let plan = parse_and_plan(
            "[interfaces]\n[[Dual]]\ntype = RNodeMultiInterface\nenabled = Yes\nport = test\n\
             [[[Low]]]\ninterface_enabled = Yes\nvport = 0\nfrequency = 868000000\n\
             bandwidth = 125000\ntxpower = 7\nspreadingfactor = 8\ncodingrate = 5\n\
             [[[High]]]\ninterface_enabled = Yes\nvport = 1\nfrequency = 2400000000\n\
             bandwidth = 812500\ntxpower = 10\nspreadingfactor = 7\ncodingrate = 6\n",
        )
        .expect("valid RNodeMulti configuration")
        .value;
        let mut report = StartupInterfaceReport::default();
        for interface in &plan.interfaces {
            report.merge(classify(&PlanOutcome::Up {
                interface,
                id: InterfaceId::new([0; 8]),
            }));
        }

        assert_eq!(
            report,
            StartupInterfaceReport {
                retrying: 2,
                ..StartupInterfaceReport::default()
            }
        );
        assert!(report.degraded());
        assert_eq!(unit_specs(&plan.interfaces).len(), 1);
    }

    #[tokio::test]
    async fn reconciliation_keeps_unchanged_units_and_replaces_only_changed_units() {
        let node = test_node!();
        let handle = node.handle();
        let first_port = free_port();
        let second_port = free_port();
        let third_port = free_port();
        let initial = listener("First", first_port);
        let exercise = async {
            let constructed = construct(
                &handle,
                &initial,
                &personal_rns::PlanRuntimeContext::default(),
            )
            .await;
            assert_eq!(constructed.startup.failed, 0);
            let monitored = MonitoredInterfaces::new(
                constructed.attached().iter().map(|interface| interface.id),
            );
            let mut manager = ConfiguredInterfaceManager::new(constructed.units, monitored.clone());
            let first_id = manager.attached()[0].id;

            let mut added = initial.clone();
            added
                .interfaces
                .extend(listener("Second", second_port).interfaces);
            assert_eq!(
                manager
                    .reconcile(
                        &handle,
                        &added,
                        &personal_rns::PlanRuntimeContext::default(),
                    )
                    .await,
                ReconcileResult::Applied
            );
            let attached = manager.attached();
            assert_eq!(attached.len(), 2);
            assert!(attached.iter().any(|interface| interface.id == first_id));

            let replacement = listener("Second", third_port);
            let second_id = attached
                .iter()
                .find(|interface| interface.plan.name == "Second")
                .map(|interface| interface.id)
                .unwrap_or_else(|| panic!("second interface was not attached"));
            assert_eq!(
                manager
                    .reconcile(
                        &handle,
                        &replacement,
                        &personal_rns::PlanRuntimeContext::default(),
                    )
                    .await,
                ReconcileResult::Applied
            );
            let attached = manager.attached();
            assert_eq!(attached.len(), 1);
            assert_ne!(attached[0].id, second_id);
            assert_eq!(monitored.subscribe().borrow().len(), 1);
        };
        tokio::select! {
            result = node.run() => panic!("test node stopped unexpectedly: {result:?}"),
            () = exercise => {}
        }
    }

    #[tokio::test]
    async fn immediate_replacement_failure_restores_the_previous_unit() {
        let node = test_node!();
        let handle = node.handle();
        let initial = listener("Listener", free_port());
        let exercise = async {
            let constructed = construct(
                &handle,
                &initial,
                &personal_rns::PlanRuntimeContext::default(),
            )
            .await;
            let monitored = MonitoredInterfaces::new(
                constructed.attached().iter().map(|interface| interface.id),
            );
            let mut manager = ConfiguredInterfaceManager::new(constructed.units, monitored);
            let invalid_runtime = parse_and_plan(
                "[interfaces]\n[[Listener]]\ntype = TCPServerInterface\ninterface_enabled = Yes\nlisten_ip = 256.256.256.256\nlisten_port = 4242\n",
            )
            .unwrap_or_else(|error| panic!("{error}"))
            .value;

            assert_eq!(
                manager
                    .reconcile(
                        &handle,
                        &invalid_runtime,
                        &personal_rns::PlanRuntimeContext::default(),
                    )
                    .await,
                ReconcileResult::RolledBack {
                    rollback_failed: false
                }
            );
            assert_eq!(manager.attached().len(), 1);
        };
        tokio::select! {
            result = node.run() => panic!("test node stopped unexpectedly: {result:?}"),
            () = exercise => {}
        }
    }

    #[test]
    fn weave_supervisor_registration_is_retrying_until_a_device_connects() {
        let plan = parse_and_plan(
            "[interfaces]\n[[Mesh]]\ntype = WeaveInterface\nenabled = Yes\nport = /dev/ttyACM0\n",
        )
        .expect("valid Weave configuration")
        .value;

        assert_eq!(
            classify(&PlanOutcome::Up {
                interface: &plan.interfaces[0],
                id: InterfaceId::new([0; 8]),
            }),
            StartupInterfaceReport {
                retrying: 1,
                ..StartupInterfaceReport::default()
            }
        );
    }

    #[test]
    fn prns_owned_interfaces_have_truthful_startup_states() {
        let plan = parse_and_plan(
            "[interfaces]\n[[USB]]\ntype = PrnsUsbAuto\nenabled = Yes\n\
             [[BLE]]\ntype = PrnsBluetoothAuto\nenabled = Yes\n\
             [[WebSocket Client]]\ntype = PrnsWebSocketClient\nenabled = Yes\ntarget = ws://peer.example/prns\nframing = raw\n\
             [[WebSocket Server]]\ntype = PrnsWebSocketServer\nenabled = Yes\nport = 4242\nframing = raw\n",
        )
        .expect("valid Prns-owned interface configuration")
        .value;
        let mut report = StartupInterfaceReport::default();
        for interface in &plan.interfaces {
            report.merge(classify(&PlanOutcome::Up {
                interface,
                id: InterfaceId::new([0; 8]),
            }));
        }

        assert_eq!(
            report,
            StartupInterfaceReport {
                online: 2,
                listening: 1,
                retrying: 1,
                failed: 0,
            }
        );
    }
}
