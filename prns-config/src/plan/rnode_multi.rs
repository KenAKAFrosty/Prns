use prns_core::interfaces::rnode::multi::{RadioConfig, RadioConfigError, RadioConfigInput, VPort};
use prns_core::interfaces::{AnnounceRateLimit, InterfaceCommonPolicy};

use crate::reference::keys::interface as interface_key;
use crate::reference::{RNodeSubinterface, ReferenceInterface, ReferenceParams};

use super::{
    airtime_limit, effective_policy, plan_access, plan_interface_discovery,
    ready_command_flow_control, station_identification, MemberEgressPolicy, PlanErrorKind,
    PlannedInterface, PlannedMedium, ReadyCommandFlowControl, StationIdentificationPlan,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RNodeMultiDevicePlan {
    name: String,
    device: String,
    station_id: Option<StationIdentificationPlan>,
}

impl RNodeMultiDevicePlan {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn device(&self) -> &str {
        &self.device
    }

    pub fn station_id(&self) -> Option<&StationIdentificationPlan> {
        self.station_id.as_ref()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RNodeMultiMemberPlan {
    parent: RNodeMultiDevicePlan,
    vport: VPort,
    radio: RadioConfig,
    flow_control: ReadyCommandFlowControl,
}

impl RNodeMultiMemberPlan {
    pub fn parent(&self) -> &RNodeMultiDevicePlan {
        &self.parent
    }

    pub const fn vport(&self) -> VPort {
        self.vport
    }

    pub const fn radio(&self) -> RadioConfig {
        self.radio
    }

    pub const fn flow_control(&self) -> ReadyCommandFlowControl {
        self.flow_control
    }
}

pub(super) struct PlanFailure {
    pub(super) subinterface_name: Option<String>,
    pub(super) kind: PlanErrorKind,
}

impl PlanFailure {
    fn parent(kind: PlanErrorKind) -> Self {
        Self {
            subinterface_name: None,
            kind,
        }
    }

    fn member(name: &str, kind: PlanErrorKind) -> Self {
        Self {
            subinterface_name: Some(name.to_string()),
            kind,
        }
    }
}

pub(super) fn plan(
    interface: &ReferenceInterface,
    global_common: InterfaceCommonPolicy,
    global_announce_rate: AnnounceRateLimit,
    transport_enabled: bool,
) -> Result<Vec<PlannedInterface>, PlanFailure> {
    let ReferenceParams::RnodeMulti {
        port,
        id_callsign,
        id_interval,
        subinterfaces,
    } = &interface.params
    else {
        return Err(PlanFailure::parent(PlanErrorKind::UnsupportedKind));
    };
    let device = port.clone().ok_or_else(|| {
        PlanFailure::parent(PlanErrorKind::MissingRequiredField {
            key: interface_key::PORT,
        })
    })?;
    let station_id = station_identification(id_callsign.as_deref(), *id_interval, Some(32))
        .map_err(PlanFailure::parent)?;
    let parent = RNodeMultiDevicePlan {
        name: interface.name.clone(),
        device,
        station_id,
    };
    subinterfaces
        .iter()
        .map(|subinterface| {
            plan_member(
                interface,
                subinterface,
                parent.clone(),
                global_common,
                global_announce_rate,
                transport_enabled,
            )
            .map_err(|kind| PlanFailure::member(&subinterface.name, kind))
        })
        .collect()
}

fn plan_member(
    interface: &ReferenceInterface,
    subinterface: &RNodeSubinterface,
    parent: RNodeMultiDevicePlan,
    global_common: InterfaceCommonPolicy,
    global_announce_rate: AnnounceRateLimit,
    transport_enabled: bool,
) -> Result<PlannedInterface, PlanErrorKind> {
    let member = RNodeMultiMemberPlan {
        parent,
        vport: VPort::new(required(subinterface.vport, interface_key::VPORT)?).ok_or(
            PlanErrorKind::InvalidSetting {
                key: interface_key::VPORT,
            },
        )?,
        radio: radio_config(subinterface)?,
        flow_control: ready_command_flow_control(subinterface.flow_control),
    };
    let medium = PlannedMedium::RnodeMulti { member };
    let discovery = plan_interface_discovery(interface, &medium);
    let policy = effective_policy(
        interface,
        &medium,
        &discovery,
        global_common,
        global_announce_rate,
        transport_enabled,
        MemberEgressPolicy::from_outgoing(subinterface.outgoing),
    )?;
    Ok(PlannedInterface {
        name: format!("{}[{}]", interface.name, subinterface.name),
        policy,
        access: plan_access(interface, &medium)?,
        medium,
        discovery,
    })
}

fn radio_config(subinterface: &RNodeSubinterface) -> Result<RadioConfig, PlanErrorKind> {
    let short = airtime_limit(
        subinterface.airtime_limit_short,
        interface_key::AIRTIME_LIMIT_SHORT,
    )?;
    let long = airtime_limit(
        subinterface.airtime_limit_long,
        interface_key::AIRTIME_LIMIT_LONG,
    )?;
    RadioConfig::new(RadioConfigInput {
        frequency_hz: required(subinterface.radio.frequency, interface_key::FREQUENCY)?,
        bandwidth_hz: required(subinterface.radio.bandwidth, interface_key::BANDWIDTH)?,
        tx_power_dbm: required(subinterface.radio.txpower, interface_key::TXPOWER)?,
        spreading_factor: required(
            subinterface.radio.spreadingfactor,
            interface_key::SPREADINGFACTOR,
        )?,
        coding_rate: required(subinterface.radio.codingrate, interface_key::CODINGRATE)?,
        airtime_limit_short_centi_percent: short.map(|limit| limit.get()),
        airtime_limit_long_centi_percent: long.map(|limit| limit.get()),
    })
    .map_err(radio_config_error)
}

fn required<T>(value: Option<T>, key: &'static str) -> Result<T, PlanErrorKind> {
    value.ok_or(PlanErrorKind::MissingRequiredField { key })
}

fn radio_config_error(error: RadioConfigError) -> PlanErrorKind {
    let key = match error {
        RadioConfigError::Frequency(_) => interface_key::FREQUENCY,
        RadioConfigError::Bandwidth(_) => interface_key::BANDWIDTH,
        RadioConfigError::TxPower(_) => interface_key::TXPOWER,
        RadioConfigError::SpreadingFactor(_) => interface_key::SPREADINGFACTOR,
        RadioConfigError::CodingRate(_) => interface_key::CODINGRATE,
        RadioConfigError::ShortAirtimeLimit(_) => interface_key::AIRTIME_LIMIT_SHORT,
        RadioConfigError::LongAirtimeLimit(_) => interface_key::AIRTIME_LIMIT_LONG,
    };
    PlanErrorKind::InvalidSetting { key }
}

#[cfg(test)]
mod tests {
    use prns_core::interfaces::ifac::IfacSize;
    use prns_core::interfaces::{
        AnnounceBandwidthCap, AnnounceRateLimit, EgressCapability, InterfaceMode,
        TransportCapability,
    };

    use crate::plan::{
        DaemonPlan, DiscoveryAdvertisementPlan, InterfaceAccessPlan, InterfaceDiscoveryPlan,
        PlannedInterface, PlannedMedium, ReadyCommandFlowControl, StationIdentificationPlan,
    };

    use super::RNodeMultiMemberPlan;

    fn plan_of(config: &str) -> DaemonPlan {
        crate::parse_and_plan(config).expect("config plans").value
    }

    fn named<'a>(plan: &'a DaemonPlan, name: &str) -> &'a PlannedInterface {
        plan.interfaces
            .iter()
            .find(|interface| interface.name == name)
            .unwrap_or_else(|| panic!("interface '{name}' was planned"))
    }

    fn member(interface: &PlannedInterface) -> &RNodeMultiMemberPlan {
        let PlannedMedium::RnodeMulti { member } = &interface.medium else {
            panic!("RNodeMulti member expected")
        };
        member
    }

    #[test]
    fn members_inherit_one_typed_parent_policy_and_access_plan() {
        let plan = plan_of(
            "[interfaces]\n[[Dual]]\ntype = RNodeMultiInterface\nenabled = Yes\nport = /dev/ttyACM0\n\
             interface_mode = internal\nannounce_cap = 3.5\nannounce_rate_target = 120\n\
             network_name = field\npassphrase = secret\nifac_size = 64\nrecursive_prs = Yes\n\
             announces_from_internal = No\ningress_control = No\negress_control = Yes\n\
             id_callsign = N0CALL\nid_interval = 600\n\
             [[[Low]]]\ninterface_enabled = Yes\nvport = 0\nfrequency = 868000000\n\
             bandwidth = 125000\ntxpower = -4\nspreadingfactor = 8\ncodingrate = 5\n\
             flow_control = Yes\noutgoing = No\nairtime_limit_short = 1.5\n\
             [[[High]]]\ninterface_enabled = Yes\nvport = 1\nfrequency = 2400000000\n\
             bandwidth = 812500\ntxpower = 10\nspreadingfactor = 7\ncodingrate = 6\n\
             outgoing = Yes\n",
        );
        assert_eq!(plan.interfaces.len(), 2);
        let low = named(&plan, "Dual[Low]");
        let high = named(&plan, "Dual[High]");
        let low_member = member(low);
        let high_member = member(high);

        assert_eq!(low_member.parent(), high_member.parent());
        assert_eq!(low_member.parent().name(), "Dual");
        assert_eq!(low_member.parent().device(), "/dev/ttyACM0");
        assert_eq!(
            low_member
                .parent()
                .station_id()
                .map(StationIdentificationPlan::callsign),
            Some("N0CALL")
        );
        assert_eq!(low_member.vport().get(), 0);
        assert_eq!(high_member.vport().get(), 1);
        assert_eq!(low_member.flow_control(), ReadyCommandFlowControl::Enabled);
        assert_eq!(
            high_member.flow_control(),
            ReadyCommandFlowControl::Disabled
        );
        assert_eq!(
            low_member.radio().airtime_limit_short_centi_percent(),
            Some(150)
        );

        assert_eq!(low.access, high.access);
        assert!(matches!(
            low.access,
            InterfaceAccessPlan::Ifac {
                size: IfacSize::NARROW,
                ..
            }
        ));
        assert_eq!(low.policy.mode, InterfaceMode::Internal);
        assert_eq!(high.policy.mode, InterfaceMode::Internal);
        assert_eq!(low.policy.common, high.policy.common);
        assert!(low.policy.common.forwarding.recursive_path_requests);
        assert!(!low.policy.common.forwarding.announces_from_internal);
        assert!(!low.policy.common.ingress_control.enabled);
        assert!(low.policy.common.path_request_egress.enabled);
        assert_eq!(low.policy.bitrate.get(), 3_125);
        assert_eq!(high.policy.bitrate.get(), 29_622);
        assert_eq!(low.policy.mtu.resolve(low.policy.bitrate), Some(508));
        assert_eq!(high.policy.mtu.resolve(high.policy.bitrate), Some(508));
        assert_eq!(low.policy.capabilities.egress, EgressCapability::Disabled);
        assert_eq!(
            high.policy.capabilities.egress,
            EgressCapability::Enabled(TransportCapability::SameInterfaceRepeat)
        );
        assert_eq!(
            low.policy.announce_bandwidth_cap,
            AnnounceBandwidthCap::Limited { cap_per_mille: 35 }
        );
        assert_eq!(
            low.policy.announce_rate_limit,
            Some(AnnounceRateLimit {
                target_ms: 120_000,
                grace: 0,
                penalty_ms: 0,
            })
        );
        assert_eq!(
            low.policy.announce_rate_limit,
            high.policy.announce_rate_limit
        );
    }

    #[test]
    fn parent_egress_bitrate_and_discovery_apply_to_every_member() {
        let plan = plan_of(
            "[interfaces]\n[[Dual]]\ntype = RNodeMultiInterface\nenabled = Yes\nport = /dev/ttyACM0\n\
             outgoing = No\nbitrate = 500000\ndiscoverable = Yes\n\
             [[[Low]]]\ninterface_enabled = Yes\nvport = 0\nfrequency = 868000000\n\
             bandwidth = 125000\ntxpower = 7\nspreadingfactor = 8\ncodingrate = 5\n\
             outgoing = Yes\n\
             [[[High]]]\ninterface_enabled = Yes\nvport = 1\nfrequency = 2400000000\n\
             bandwidth = 812500\ntxpower = 10\nspreadingfactor = 7\ncodingrate = 6\n",
        );
        let low = named(&plan, "Dual[Low]");
        let high = named(&plan, "Dual[High]");
        for member in [low, high] {
            assert_eq!(member.policy.bitrate.get(), 500_000);
            assert_eq!(member.policy.mtu.resolve(member.policy.bitrate), Some(508));
            assert_eq!(
                member.policy.capabilities.egress,
                EgressCapability::Disabled
            );
            assert_eq!(member.policy.mode, InterfaceMode::AccessPoint);
        }
        let InterfaceDiscoveryPlan::Announce(low_discovery) = &low.discovery else {
            panic!("low radio discovery plan expected")
        };
        let InterfaceDiscoveryPlan::Announce(high_discovery) = &high.discovery else {
            panic!("high radio discovery plan expected")
        };
        assert_eq!(
            low_discovery.advertisement,
            DiscoveryAdvertisementPlan::RNode {
                frequency_hz: 868_000_000,
                bandwidth_hz: 125_000,
                spreading_factor: 8,
                coding_rate: 5,
            }
        );
        assert_eq!(
            high_discovery.advertisement,
            DiscoveryAdvertisementPlan::RNode {
                frequency_hz: 2_400_000_000,
                bandwidth_hz: 812_500,
                spreading_factor: 7,
                coding_rate: 6,
            }
        );
    }
}
