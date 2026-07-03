//! Render the daemon's lines for a [`DaemonPlan`] standing up — construction itself lives in
//! `personal_rns::from_plan` ([`attach_plan`]); this module owns only the `RNSD_*` output the
//! smokes and operators read.

use personal_rns::config::{
    DaemonPlan, DeferReason, PlannedInterface, PlannedMedium, UnappliedSetting,
};
use personal_rns::from_plan::{attach_plan, PlanOutcome};
use personal_rns::runtime::TokioPrnsHandle;

pub async fn construct_interfaces(handle: &TokioPrnsHandle, plan: &DaemonPlan) {
    attach_plan(handle, plan, &mut render).await;
}

fn render(outcome: PlanOutcome<'_>) {
    match outcome {
        PlanOutcome::Up(interface) => {
            println!(
                "RNSD_INTERFACE_UP name={:?} {}",
                interface.name,
                medium_detail(interface)
            );
        }
        PlanOutcome::Failed { interface, error } => {
            eprintln!(
                "RNSD_INTERFACE_FAILED name={:?} {} error={error}",
                interface.name,
                medium_detail(interface)
            );
        }
        PlanOutcome::Unapplied(interface) => {
            for setting in &interface.unapplied {
                let detail = match setting {
                    UnappliedSetting::Mode(mode) => format!("mode={mode:?}"),
                    UnappliedSetting::AnnounceBandwidthCap => String::from("announce_cap"),
                    UnappliedSetting::AnnounceRateLimit => String::from("announce_rate_limit"),
                    UnappliedSetting::IfacAuthentication => String::from("ifac"),
                    UnappliedSetting::MediumOption(key) => format!("option={key}"),
                };
                println!("RNSD_SETTING_UNAPPLIED name={:?} {detail}", interface.name);
            }
        }
        PlanOutcome::Deferred(deferred) => {
            let reason = match deferred.why {
                DeferReason::Disabled => String::from("disabled"),
                DeferReason::UnsupportedKind => String::from("unsupported-kind"),
                DeferReason::MissingRequiredField { key } => format!("missing-field:{key}"),
            };
            println!(
                "RNSD_INTERFACE_DEFERRED name={:?} type={:?} reason={reason}",
                deferred.name, deferred.type_name
            );
        }
    }
}

fn medium_detail(interface: &PlannedInterface) -> String {
    match &interface.medium {
        PlannedMedium::AutoWifi { .. } => String::from("medium=auto-wifi"),
        PlannedMedium::TcpClient { host, port } => {
            format!("medium=tcp-client target={host}:{port}")
        }
        PlannedMedium::TcpServer { bind } => format!("medium=tcp-server bind={bind}"),
        PlannedMedium::Udp { listen, forward } => {
            format!("medium=udp listen={listen} forward={forward}")
        }
        PlannedMedium::Serial { device, baud } => {
            format!("medium=serial device={device} baud={baud}")
        }
        PlannedMedium::Kiss { device, baud, .. } => {
            format!("medium=kiss device={device} baud={baud}")
        }
        PlannedMedium::Ax25Kiss {
            device,
            callsign,
            ssid,
            ..
        } => format!("medium=ax25-kiss device={device} callsign={callsign} ssid={ssid}"),
        PlannedMedium::Rnode {
            device,
            frequency_hz,
            bandwidth_hz,
            txpower_dbm,
            spreading_factor,
            coding_rate,
            ..
        } => format!(
            "medium=rnode device={device} freq={frequency_hz} bw={bandwidth_hz} sf={spreading_factor} cr={coding_rate} txpower={txpower_dbm}"
        ),
        PlannedMedium::Backbone { bind } => format!("medium=backbone bind={bind}"),
        PlannedMedium::BackboneClient { host, port } => {
            format!("medium=backbone-client target={host}:{port}")
        }
        PlannedMedium::Pipe { command, .. } => format!("medium=pipe command={command:?}"),
    }
}
