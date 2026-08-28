use prns_config::{PlannedInterface, RNodeMultiMemberPlan};
use prns_core::interfaces::kiss::StationIdWireFormat;

use crate::rnode::host::RNODE_BAUD;
use crate::rnode::multi::{
    RNodeMultiAccess, RNodeMultiInterface, RNodeMultiMemberSettings, RNodeMultiMembers,
    RNodeMultiSettings, DEFAULT_RNODE_MULTI_CONFIGURE_DELAY,
};
use crate::serial::open_host_serial;

use super::super::{
    report_up, runtime_access, station_identification, InterfaceAccess, PlanAttachments,
    PlanFailure, PlanOutcome, RECONNECT_POLICY,
};
use super::runtime_flow_control;

pub(in crate::from_plan) fn stand_up<'a>(
    handle: &prns_runtime::runtime::PrnsNodeHandle,
    interfaces: impl Iterator<Item = (&'a PlannedInterface, &'a RNodeMultiMemberPlan)>,
    attachments: &mut PlanAttachments,
    report: &mut impl FnMut(PlanOutcome<'a>),
) {
    let interfaces = interfaces.collect::<Vec<_>>();
    let Some((first, first_member)) = interfaces.first().copied() else {
        return;
    };
    let access = match runtime_access(first) {
        Ok(InterfaceAccess { ifac: None }) => RNodeMultiAccess::Open,
        Ok(InterfaceAccess { ifac: Some(access) }) => RNodeMultiAccess::Ifac {
            context: Box::new(access.context),
            network_name: access.network_name,
        },
        Err(error) => {
            for (interface, _) in interfaces {
                report(PlanOutcome::Failed {
                    interface,
                    error: error.clone(),
                });
            }
            return;
        }
    };
    let station_plan = first_member.parent().station_id().cloned();
    let station_identification =
        match station_identification::runtime(&station_plan, StationIdWireFormat::Exact) {
            Ok(station_identification) => station_identification,
            Err(error) => {
                for (interface, _) in interfaces {
                    report(PlanOutcome::Failed {
                        interface,
                        error: error.clone(),
                    });
                }
                return;
            }
        };
    let settings = interfaces
        .iter()
        .map(|(interface, member)| member_settings(interface, member, access.clone()))
        .collect::<Vec<_>>();
    let members = match RNodeMultiMembers::new(settings) {
        Ok(members) => members,
        Err(error) => {
            let error = PlanFailure::from(error);
            for (interface, _) in interfaces {
                report(PlanOutcome::Failed {
                    interface,
                    error: error.clone(),
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
            reconnect_policy: RECONNECT_POLICY,
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

fn member_settings(
    interface: &PlannedInterface,
    member: &RNodeMultiMemberPlan,
    access: RNodeMultiAccess,
) -> RNodeMultiMemberSettings {
    RNodeMultiMemberSettings::new(
        interface.name.clone(),
        member.vport(),
        member.radio(),
        runtime_flow_control(member.flow_control()),
        interface.policy,
        access,
        member.parent().device().as_bytes(),
    )
}

#[cfg(test)]
mod tests {
    use prns_core::interfaces::ConnectionState;
    use prns_runtime::runtime::{
        ManuallyAttached, NoPersistence, PreConfiguredDestination, PrnsNode, PrnsNodeRecipe,
    };
    use prns_runtime::storage::GrowableHeap;

    use crate::from_plan::{attach_plan, PlanOutcome};

    #[tokio::test]
    async fn planned_members_register_once_under_one_device_supervisor() {
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
            request_endpoints: prns_runtime::request_endpoints![],
            remote_control: prns_runtime::remote_control::RemoteControlService::Unavailable,
            interfaces: ManuallyAttached,
            persistence: NoPersistence,
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
            prns_config::ConfiguredInterfaceLifecycle::BootstrapOnly
        );
        assert!(attachments.groups[0].supervisor_task.is_some());
        attachments.detach(&node.handle()).await;
    }
}
