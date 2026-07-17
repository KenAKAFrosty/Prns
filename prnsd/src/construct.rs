use personal_rns::config::{
    DaemonPlan, DeferReason, DiscoveryPublicationProblem, InterfaceDiscoveryPlan, PlannedInterface,
    PlannedMedium, UnappliedSetting,
};
use personal_rns::from_plan::{attach_plan, PlanOutcome};
use personal_rns::interfaces::{InterfaceId, InterfaceOriginKind};
use personal_rns::runtime::TokioPrnsHandle;

pub(crate) struct AttachedConfiguredInterface {
    pub id: InterfaceId,
    pub plan: PlannedInterface,
}

pub(crate) async fn construct_interfaces(
    handle: &TokioPrnsHandle,
    plan: &DaemonPlan,
) -> Vec<AttachedConfiguredInterface> {
    let mut constructed = Vec::new();
    attach_plan(handle, plan, &mut |outcome| {
        if let PlanOutcome::Up { interface, id } = &outcome {
            constructed.push(AttachedConfiguredInterface {
                id: *id,
                plan: (*interface).clone(),
            });
        }
        render(outcome);
    })
    .await;
    constructed
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
        PlanOutcome::Unapplied(interface) => {
            for setting in &interface.unapplied {
                tracing::warn!(
                    event = "interface_setting_unapplied",
                    interface_origin = InterfaceOriginKind::Configured.as_str(),
                    setting = unapplied_name(setting),
                );
                tracing::debug!(
                    event = "interface_setting_unapplied_detail",
                    interface_origin = InterfaceOriginKind::Configured.as_str(),
                    interface_name = ?interface.name,
                    setting = ?setting,
                );
            }
        }
        PlanOutcome::Deferred(deferred) => {
            tracing::info!(
                event = "interface_deferred",
                interface_origin = InterfaceOriginKind::Configured.as_str(),
                interface_name = ?deferred.name,
                interface_type = ?deferred.type_name,
                reason = defer_name(&deferred.why),
            );
            tracing::debug!(
                event = "interface_deferred_detail",
                interface_origin = InterfaceOriginKind::Configured.as_str(),
                reason = ?deferred.why,
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
        PlannedMedium::Backbone { .. } => "backbone",
        PlannedMedium::BackboneClient { .. } => "backbone_client",
        PlannedMedium::Pipe { .. } => "pipe",
    }
}

fn unapplied_name(setting: &UnappliedSetting) -> &'static str {
    match setting {
        UnappliedSetting::AnnounceBandwidthCap => "announce_bandwidth_cap",
        UnappliedSetting::AnnounceRateLimit => "announce_rate_limit",
        UnappliedSetting::MediumOption(_) => "medium_option",
    }
}

fn defer_name(reason: &DeferReason) -> &'static str {
    match reason {
        DeferReason::Disabled => "disabled",
        DeferReason::UnsupportedKind => "unsupported_kind",
        DeferReason::MissingRequiredField { .. } => "missing_required_field",
        DeferReason::InvalidSetting { .. } => "invalid_setting",
    }
}
