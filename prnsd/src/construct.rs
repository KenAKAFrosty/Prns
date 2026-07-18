use personal_rns::config::{
    DaemonPlan, DiscoveryPublicationProblem, InterfaceDiscoveryPlan, PlannedInterface,
    PlannedMedium,
};
use personal_rns::from_plan::{attach_plan_with_context, PlanOutcome, PlanRuntimeContext};
use personal_rns::interfaces::{InterfaceId, InterfaceOriginKind};
use personal_rns::runtime::PrnsNodeHandle;

pub(crate) struct AttachedConfiguredInterface {
    pub id: InterfaceId,
    pub plan: PlannedInterface,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StartupInterfaceReport {
    pub online: u32,
    pub listening: u32,
    pub retrying: u32,
    pub failed: u32,
}

impl StartupInterfaceReport {
    pub fn merge(&mut self, other: Self) {
        self.online = self.online.saturating_add(other.online);
        self.listening = self.listening.saturating_add(other.listening);
        self.retrying = self.retrying.saturating_add(other.retrying);
        self.failed = self.failed.saturating_add(other.failed);
    }

    pub const fn degraded(self) -> bool {
        self.retrying != 0 || self.failed != 0
    }
}

#[derive(Default)]
pub struct ConstructedInterfaces {
    pub attached: Vec<AttachedConfiguredInterface>,
    pub startup: StartupInterfaceReport,
}

pub async fn construct_interfaces(
    handle: &PrnsNodeHandle,
    plan: &DaemonPlan,
    context: &PlanRuntimeContext,
) -> ConstructedInterfaces {
    let mut constructed = ConstructedInterfaces::default();
    attach_plan_with_context(handle, plan, context, &mut |outcome| {
        constructed.startup.merge(classify(&outcome));
        if let PlanOutcome::Up { interface, id } = &outcome {
            constructed.attached.push(AttachedConfiguredInterface {
                id: *id,
                plan: (*interface).clone(),
            });
        }
        render(outcome);
    })
    .await;
    constructed
}

fn classify(outcome: &PlanOutcome<'_>) -> StartupInterfaceReport {
    let mut report = StartupInterfaceReport::default();
    match outcome {
        PlanOutcome::Up { interface, .. } => match &interface.medium {
            PlannedMedium::TcpServer { .. } | PlannedMedium::Backbone { .. } => {
                report.listening = 1;
            }
            PlannedMedium::AutoWifi { .. } | PlannedMedium::Udp { .. } => report.online = 1,
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
            | PlannedMedium::I2p { .. } => report.retrying = 1,
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
        PlanOutcome::Failed {
            interface,
            visible_error_message,
        } => {
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
                error = %visible_error_message,
            );
        }
    }
}

fn medium_name(medium: &PlannedMedium) -> &'static str {
    match medium {
        PlannedMedium::AutoWifi { .. } => "auto_wifi",
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
    use super::{classify, PlanOutcome, StartupInterfaceReport};
    use personal_rns::config::parse_and_plan;
    use personal_rns::interfaces::InterfaceId;

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
    }
}
