use std::collections::BTreeMap;

use prns_core::interfaces::ax25_kiss;
use prns_core::interfaces::backbone;
use prns_core::interfaces::bluetooth_auto as bluetooth_contract;
use prns_core::interfaces::i2p as i2p_core;
use prns_core::interfaces::kiss;
use prns_core::interfaces::pipe;
use prns_core::interfaces::serial;
use prns_core::interfaces::usb_auto;
use prns_core::interfaces::weave as weave_core;
use prns_core::interfaces::wifi_auto as wifi_auto_contract;
use prns_core::interfaces::{
    tcp, udp, websocket, AnnounceBandwidthCap, AnnounceRateLimit, BitrateBps,
    ConfiguredInterfacePolicy, EffectiveInterfacePolicy, EgressCapability, FrequencyMilliHertz,
    IngressCapability, InterfaceCommonPolicy, InterfaceDefaults, InterfaceForwardingPolicy,
    InterfaceGravity, InterfaceMode, MtuBytes, MtuPolicy, RecursivePathRequestPolicy,
};
use prns_core::routing::links::MAX_LINK_MTU;

use super::discovery::InterfaceDiscoveryPlan;
use super::medium::{rnode_defaults, PlannedMedium, UdpFlowPlan};
use super::PlanErrorKind;
use crate::plan::error::{GlobalPlanError, SettingRepresentationError};
use crate::plan::reference_globals::{global_bool, global_f64, global_i64};
use crate::reference::keys::{
    common as common_key, global as global_key, interface as interface_key,
};
use crate::reference::{
    announce_rate_target_is_explicit_off, ReferenceAnnounceRateTarget, ReferenceConfig,
    ReferenceConfigParams, ReferenceInterface, ReferenceMode, ReferenceValue,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::plan) enum MemberEgressPolicy {
    Inherit,
    Disabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::plan) struct InheritedInterfacePolicy {
    pub common: InterfaceCommonPolicy,
    pub announce_rate: Option<AnnounceRateLimit>,
    pub gravity: InterfaceGravity,
}

impl MemberEgressPolicy {
    pub(in crate::plan) const fn from_outgoing(outgoing: Option<bool>) -> Self {
        if matches!(outgoing, Some(false)) {
            Self::Disabled
        } else {
            Self::Inherit
        }
    }
}

pub(in crate::plan) fn effective_policy(
    interface: &ReferenceInterface,
    medium: &PlannedMedium,
    discovery: &InterfaceDiscoveryPlan,
    inherited: InheritedInterfacePolicy,
    transport_enabled: bool,
    member_egress: MemberEgressPolicy,
) -> Result<EffectiveInterfacePolicy, PlanErrorKind> {
    let bitrate = interface
        .bitrate
        .map(|bitrate| {
            BitrateBps::new(bitrate).ok_or(PlanErrorKind::InvalidSetting {
                key: interface_key::BITRATE,
            })
        })
        .transpose()?;
    let mtu = configured_mtu(interface)?;
    let defaults = interface_defaults(medium)?;
    let ingress = if matches!(
        medium,
        PlannedMedium::Udp {
            flow: UdpFlowPlan::SendOnly { .. }
        }
    ) {
        IngressCapability::Disabled
    } else {
        defaults.capabilities.ingress
    };
    let egress = if interface.outgoing == Some(false)
        || member_egress == MemberEgressPolicy::Disabled
        || matches!(
            medium,
            PlannedMedium::Udp {
                flow: UdpFlowPlan::ReceiveOnly { .. }
            }
        ) {
        EgressCapability::Disabled
    } else {
        defaults.capabilities.egress
    };
    let capabilities = (ingress != defaults.capabilities.ingress
        || egress != defaults.capabilities.egress)
        .then_some(prns_core::interfaces::InterfaceCapabilities { ingress, egress });
    let announce_bandwidth_cap = interface
        .announce_cap
        .map(announce_bandwidth_cap)
        .transpose()?;
    let announce_rate_limit =
        planned_announce_rate_limit(interface, inherited.announce_rate, transport_enabled)?;
    let common = interface_common_policy(interface, inherited.common)?;
    Ok(defaults.configured(ConfiguredInterfacePolicy {
        capabilities,
        mode: Some(planned_mode(interface, medium, discovery)),
        gravity: Some(InterfaceGravity::new(
            interface.gravity.unwrap_or(inherited.gravity.get()),
        )),
        bitrate,
        mtu,
        announce_rate_limit,
        announce_bandwidth_cap,
        common: Some(common),
        ..ConfiguredInterfacePolicy::default()
    }))
}

enum AnnounceRateSource {
    Interface { target_seconds: u64 },
    TransportDefault(AnnounceRateLimit),
}

fn planned_announce_rate_limit(
    interface: &ReferenceInterface,
    global: Option<AnnounceRateLimit>,
    transport_enabled: bool,
) -> Result<Option<AnnounceRateLimit>, PlanErrorKind> {
    let source = match (interface.announce_rate_target, transport_enabled) {
        (Some(ReferenceAnnounceRateTarget::Off), _) => return Ok(None),
        (Some(ReferenceAnnounceRateTarget::Seconds(target_seconds)), _) => {
            AnnounceRateSource::Interface {
                target_seconds: target_seconds.get(),
            }
        }
        (None, true) => match global {
            Some(defaults) => AnnounceRateSource::TransportDefault(defaults),
            None => return Ok(None),
        },
        (None, false) => return Ok(None),
    };
    let (target_ms, default_grace, default_penalty_ms) = match source {
        AnnounceRateSource::Interface { target_seconds } => (
            checked_milliseconds(target_seconds, interface_key::ANNOUNCE_RATE_TARGET)?,
            0,
            0,
        ),
        AnnounceRateSource::TransportDefault(defaults) => {
            (defaults.target_ms, defaults.grace, defaults.penalty_ms)
        }
    };
    let grace = interface
        .announce_rate_grace
        .map(u16::try_from)
        .transpose()
        .map_err(|_| PlanErrorKind::InvalidSetting {
            key: interface_key::ANNOUNCE_RATE_GRACE,
        })?
        .unwrap_or(default_grace);
    let penalty_ms = interface
        .announce_rate_penalty
        .map(|seconds| checked_milliseconds(seconds, interface_key::ANNOUNCE_RATE_PENALTY))
        .transpose()?
        .unwrap_or(default_penalty_ms);
    Ok(Some(AnnounceRateLimit {
        target_ms,
        grace,
        penalty_ms,
    }))
}

fn checked_milliseconds(seconds: u64, key: &'static str) -> Result<u64, PlanErrorKind> {
    seconds
        .checked_mul(1_000)
        .ok_or(PlanErrorKind::InvalidSetting { key })
}

fn interface_defaults(medium: &PlannedMedium) -> Result<InterfaceDefaults, PlanErrorKind> {
    match medium {
        PlannedMedium::AutoWifi(_) => Ok(wifi_auto_contract::DEFAULTS),
        PlannedMedium::TcpClient { .. } | PlannedMedium::TcpServer { .. } => Ok(tcp::DEFAULTS),
        PlannedMedium::Backbone { .. } => Ok(backbone::DEFAULTS),
        PlannedMedium::BackboneClient { .. } => Ok(backbone::CLIENT_DEFAULTS),
        PlannedMedium::Udp { .. } => Ok(udp::DEFAULTS),
        PlannedMedium::I2p { .. } => Ok(i2p_core::DEFAULTS),
        PlannedMedium::Weave { .. } => Ok(weave_core::DEFAULTS),
        PlannedMedium::PrnsUsbAuto => Ok(usb_auto::HOST_DEFAULTS),
        PlannedMedium::PrnsBluetoothAuto { .. } => Ok(bluetooth_contract::defaults_for_bitrate(
            bluetooth_contract::BLE_BITRATE_GUESS_BPS,
        )),
        PlannedMedium::PrnsWebSocketClient { .. } | PlannedMedium::PrnsWebSocketServer { .. } => {
            Ok(websocket::DEFAULTS)
        }
        PlannedMedium::Serial { line, .. } => {
            let bitrate =
                BitrateBps::new(u64::from(line.baud())).ok_or(PlanErrorKind::InvalidSetting {
                    key: interface_key::SPEED,
                })?;
            Ok(serial::defaults_for_bitrate(bitrate))
        }
        PlannedMedium::Kiss { .. } => Ok(kiss::DEFAULTS),
        PlannedMedium::Ax25Kiss { .. } => Ok(ax25_kiss::DEFAULTS),
        PlannedMedium::Pipe { .. } => Ok(pipe::DEFAULTS),
        PlannedMedium::Rnode {
            bandwidth_hz,
            spreading_factor,
            coding_rate,
            ..
        } => rnode_defaults(*spreading_factor, *coding_rate, *bandwidth_hz),
        PlannedMedium::RnodeMulti { member } => {
            let radio = member.radio();
            rnode_defaults(
                radio.spreading_factor(),
                radio.coding_rate(),
                radio.bandwidth_hz(),
            )
        }
    }
}

fn configured_mtu(interface: &ReferenceInterface) -> Result<Option<MtuPolicy>, PlanErrorKind> {
    let fixed_mtu = match &interface.params {
        ReferenceConfigParams::TcpClient { fixed_mtu, .. }
        | ReferenceConfigParams::TcpServer { fixed_mtu, .. } => *fixed_mtu,
        _ => None,
    };
    fixed_mtu
        .map(|fixed_mtu| {
            if fixed_mtu > MAX_LINK_MTU {
                return Err(PlanErrorKind::InvalidSetting {
                    key: interface_key::FIXED_MTU,
                });
            }
            MtuBytes::new(fixed_mtu)
                .map(MtuPolicy::Fixed)
                .ok_or(PlanErrorKind::InvalidSetting {
                    key: interface_key::FIXED_MTU,
                })
        })
        .transpose()
}

fn planned_mode(
    interface: &ReferenceInterface,
    medium: &PlannedMedium,
    discovery: &InterfaceDiscoveryPlan,
) -> InterfaceMode {
    let configured = interface.mode.map(map_mode).unwrap_or(match medium {
        PlannedMedium::PrnsUsbAuto
        | PlannedMedium::PrnsWebSocketClient { .. }
        | PlannedMedium::PrnsWebSocketServer { .. } => InterfaceMode::PointToPoint,
        _ => InterfaceMode::Full,
    });
    if matches!(discovery, InterfaceDiscoveryPlan::Disabled)
        || matches!(
            configured,
            InterfaceMode::Gateway | InterfaceMode::AccessPoint | InterfaceMode::Internal
        )
    {
        return configured;
    }
    if matches!(
        interface.params,
        ReferenceConfigParams::Rnode { .. } | ReferenceConfigParams::RnodeMulti { .. }
    ) {
        InterfaceMode::AccessPoint
    } else {
        InterfaceMode::Gateway
    }
}

fn announce_bandwidth_cap(percent: f64) -> Result<AnnounceBandwidthCap, PlanErrorKind> {
    if !percent.is_finite() || !(0.0..=100.0).contains(&percent) {
        return Err(PlanErrorKind::InvalidSetting {
            key: interface_key::ANNOUNCE_CAP,
        });
    }
    let per_mille = (percent * 10.0).round();
    Ok(AnnounceBandwidthCap::Limited {
        cap_per_mille: per_mille as u16,
    })
}

fn interface_common_policy(
    interface: &ReferenceInterface,
    global: InterfaceCommonPolicy,
) -> Result<InterfaceCommonPolicy, PlanErrorKind> {
    let mut common = global;
    common.forwarding = InterfaceForwardingPolicy {
        recursive_path_requests: interface.recursive_prs.map_or(
            common.forwarding.recursive_path_requests,
            RecursivePathRequestPolicy::from_configured,
        ),
        announces_from_internal: interface
            .announces_from_internal
            .unwrap_or(common.forwarding.announces_from_internal),
        announces_to_internal: interface
            .announces_to_internal
            .unwrap_or(common.forwarding.announces_to_internal),
    };
    common.ingress_control.enabled = interface
        .ingress_control
        .unwrap_or(common.ingress_control.enabled);
    common.path_request_egress.enabled = interface
        .egress_control
        .unwrap_or(common.path_request_egress.enabled);
    if let Some(value) = interface.ic_max_held_announces {
        common.ingress_control.max_held_announces =
            usize::try_from(value).map_err(|_| PlanErrorKind::InvalidSetting {
                key: common_key::IC_MAX_HELD_ANNOUNCES,
            })?;
    }
    apply_common_numbers(
        CommonNumberOverrides::from_interface(interface),
        &mut common,
    )
    .map_err(PlanErrorKind::from)?;
    Ok(common)
}

#[derive(Debug, Clone, Copy, Default)]
struct CommonNumberOverrides {
    new_time: Option<f64>,
    burst_hold: Option<f64>,
    burst_penalty: Option<f64>,
    held_release_interval: Option<f64>,
    burst_freq_new: Option<f64>,
    burst_freq: Option<f64>,
    pr_burst_freq_new: Option<f64>,
    pr_burst_freq: Option<f64>,
    pr_egress_freq: Option<f64>,
}

impl CommonNumberOverrides {
    fn from_interface(interface: &ReferenceInterface) -> Self {
        Self {
            new_time: interface.ic_new_time,
            burst_hold: interface.ic_burst_hold,
            burst_penalty: interface.ic_burst_penalty,
            held_release_interval: interface.ic_held_release_interval,
            burst_freq_new: interface.ic_burst_freq_new,
            burst_freq: interface.ic_burst_freq,
            pr_burst_freq_new: interface.ic_pr_burst_freq_new,
            pr_burst_freq: interface.ic_pr_burst_freq,
            pr_egress_freq: interface.ec_pr_freq,
        }
    }

    fn from_globals(globals: &BTreeMap<String, ReferenceValue>) -> Self {
        Self {
            new_time: global_f64(globals, common_key::IC_NEW_TIME),
            burst_hold: global_f64(globals, common_key::IC_BURST_HOLD),
            burst_penalty: global_f64(globals, common_key::IC_BURST_PENALTY),
            held_release_interval: global_f64(globals, common_key::IC_HELD_RELEASE_INTERVAL),
            burst_freq_new: global_f64(globals, common_key::IC_BURST_FREQ_NEW),
            burst_freq: global_f64(globals, common_key::IC_BURST_FREQ),
            pr_burst_freq_new: global_f64(globals, common_key::IC_PR_BURST_FREQ_NEW),
            pr_burst_freq: global_f64(globals, common_key::IC_PR_BURST_FREQ),
            pr_egress_freq: global_f64(globals, common_key::EC_PR_FREQ),
        }
    }
}

fn apply_common_numbers(
    configured: CommonNumberOverrides,
    common: &mut InterfaceCommonPolicy,
) -> Result<(), SettingRepresentationError> {
    if let Some(value) = configured.new_time {
        common.ingress_control.new_interface_millis =
            seconds_to_millis(value, common_key::IC_NEW_TIME)?;
    }
    if let Some(value) = configured.burst_hold {
        common.ingress_control.burst_hold_millis =
            seconds_to_millis(value, common_key::IC_BURST_HOLD)?;
    }
    if let Some(value) = configured.burst_penalty {
        common.ingress_control.burst_penalty_millis =
            seconds_to_millis(value, common_key::IC_BURST_PENALTY)?;
    }
    if let Some(value) = configured.held_release_interval {
        common.ingress_control.held_release_interval_millis =
            seconds_to_millis(value, common_key::IC_HELD_RELEASE_INTERVAL)?;
    }
    if let Some(value) = configured.burst_freq_new {
        common.ingress_control.announce_burst_frequency_new =
            hertz_to_milli_hertz(value, common_key::IC_BURST_FREQ_NEW)?;
    }
    if let Some(value) = configured.burst_freq {
        common.ingress_control.announce_burst_frequency =
            hertz_to_milli_hertz(value, common_key::IC_BURST_FREQ)?;
    }
    if let Some(value) = configured.pr_burst_freq_new {
        common.ingress_control.path_request_burst_frequency_new =
            hertz_to_milli_hertz(value, common_key::IC_PR_BURST_FREQ_NEW)?;
    }
    if let Some(value) = configured.pr_burst_freq {
        common.ingress_control.path_request_burst_frequency =
            hertz_to_milli_hertz(value, common_key::IC_PR_BURST_FREQ)?;
    }
    if let Some(value) = configured.pr_egress_freq {
        common.path_request_egress.frequency = hertz_to_milli_hertz(value, common_key::EC_PR_FREQ)?;
    }
    Ok(())
}

fn seconds_to_millis(value: f64, key: &'static str) -> Result<u64, SettingRepresentationError> {
    let millis = (value * 1_000.0).round();
    if !value.is_finite() || value < 0.0 || millis >= u64::MAX as f64 {
        return Err(SettingRepresentationError { key });
    }
    Ok(millis as u64)
}

fn hertz_to_milli_hertz(
    value: f64,
    key: &'static str,
) -> Result<FrequencyMilliHertz, SettingRepresentationError> {
    let milli_hertz = (value * 1_000.0).round();
    if !value.is_finite() || value < 0.0 || milli_hertz >= u64::MAX as f64 {
        return Err(SettingRepresentationError { key });
    }
    Ok(FrequencyMilliHertz::new(milli_hertz as u64))
}

pub(in crate::plan) fn global_common_policy(
    config: &ReferenceConfig,
) -> Result<InterfaceCommonPolicy, GlobalPlanError> {
    let mut common = InterfaceCommonPolicy::RNS_DEFAULT;
    common.path_request_egress.enabled =
        global_bool(&config.globals, common_key::EGRESS_CONTROL, false);
    if config
        .globals
        .contains_key(common_key::IC_MAX_HELD_ANNOUNCES)
    {
        common.ingress_control.max_held_announces =
            global_i64(&config.globals, common_key::IC_MAX_HELD_ANNOUNCES)
                .and_then(|value| usize::try_from(value).ok())
                .ok_or(GlobalPlanError {
                    key: common_key::IC_MAX_HELD_ANNOUNCES,
                })?;
    }
    apply_common_numbers(
        CommonNumberOverrides::from_globals(&config.globals),
        &mut common,
    )
    .map_err(GlobalPlanError::from)?;
    Ok(common)
}

pub(in crate::plan) fn global_announce_rate(
    config: &ReferenceConfig,
) -> Result<Option<AnnounceRateLimit>, GlobalPlanError> {
    let configured_off = config
        .globals
        .get(global_key::DEFAULT_AR_TARGET)
        .and_then(ReferenceValue::as_scalar)
        .is_some_and(announce_rate_target_is_explicit_off);
    if configured_off {
        return Ok(None);
    }
    let target_seconds =
        global_nonnegative_integer(&config.globals, global_key::DEFAULT_AR_TARGET, 3_600)?;
    if target_seconds == 0 {
        return Ok(None);
    }
    let grace = global_nonnegative_integer(&config.globals, global_key::DEFAULT_AR_GRACE, 5)?;
    let penalty_seconds =
        global_nonnegative_integer(&config.globals, global_key::DEFAULT_AR_PENALTY, 0)?;
    Ok(Some(AnnounceRateLimit {
        target_ms: target_seconds.checked_mul(1_000).ok_or(GlobalPlanError {
            key: global_key::DEFAULT_AR_TARGET,
        })?,
        grace: grace.try_into().map_err(|_| GlobalPlanError {
            key: global_key::DEFAULT_AR_GRACE,
        })?,
        penalty_ms: penalty_seconds.checked_mul(1_000).ok_or(GlobalPlanError {
            key: global_key::DEFAULT_AR_PENALTY,
        })?,
    }))
}

fn global_nonnegative_integer(
    globals: &BTreeMap<String, ReferenceValue>,
    key: &'static str,
    default: u64,
) -> Result<u64, GlobalPlanError> {
    if !globals.contains_key(key) {
        return Ok(default);
    }
    global_i64(globals, key)
        .and_then(|value| u64::try_from(value).ok())
        .ok_or(GlobalPlanError { key })
}

fn map_mode(mode: ReferenceMode) -> InterfaceMode {
    match mode {
        ReferenceMode::Full => InterfaceMode::Full,
        ReferenceMode::AccessPoint => InterfaceMode::AccessPoint,
        ReferenceMode::PointToPoint => InterfaceMode::PointToPoint,
        ReferenceMode::Roaming => InterfaceMode::Roaming,
        ReferenceMode::Boundary => InterfaceMode::Boundary,
        ReferenceMode::Gateway => InterfaceMode::Gateway,
        ReferenceMode::Internal => InterfaceMode::Internal,
    }
}
