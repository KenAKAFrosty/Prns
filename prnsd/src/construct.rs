//! Render the daemon's lines for a [`DaemonPlan`] standing up — construction itself lives in
//! `personal_rns::from_plan` ([`attach_plan`]); this module owns only the `RNSD_*` output the
//! smokes and operators read.

use personal_rns::config::{DaemonPlan, DeferReason, PlannedMedium, UnappliedSetting};
use personal_rns::from_plan::{attach_plan, PlanOutcome};
use personal_rns::interfaces::InterfaceOriginKind;
use personal_rns::runtime::TokioPrnsHandle;

pub async fn construct_interfaces(handle: &TokioPrnsHandle, plan: &DaemonPlan) {
    attach_plan(handle, plan, &mut render).await;
}

fn render(outcome: PlanOutcome<'_>) {
    match outcome {
        PlanOutcome::Up { interface, .. } => {
            tracing::info!(
                event = "interface_started",
                interface_origin = InterfaceOriginKind::Configured.as_str(),
                interface_name = ?interface.name,
                medium = medium_name(&interface.medium),
            );
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
        UnappliedSetting::Mode(_) => "mode",
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
