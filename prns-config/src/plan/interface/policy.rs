use std::collections::BTreeMap;

use prns_core::interfaces::ax25_kiss::core as ax25_core;
use prns_core::interfaces::kiss::core as kiss_core;
use prns_core::interfaces::pipe::core as pipe_core;
use prns_core::interfaces::rnode::policy as rnode_policy;
use prns_core::interfaces::serial::core as serial_core;
use prns_core::interfaces::tcp::core as tcp_core;
use prns_core::interfaces::udp::core as udp_core;
use prns_core::interfaces::wifi_auto::core as wifi_core;
use prns_core::interfaces::{
    AnnounceBandwidthCap, AnnounceRateLimit, BitrateBps, ConfiguredInterfacePolicy,
    EffectiveInterfacePolicy, EgressCapability, FrequencyMilliHertz, IngressCapability,
    InterfaceCommonPolicy, InterfaceDefaults, InterfaceForwardingPolicy, InterfaceMode, MtuBytes,
    MtuPolicy,
};
use prns_core::routing::links::MAX_LINK_MTU;

use super::discovery::InterfaceDiscoveryPlan;
use super::medium::{PlannedMedium, UdpFlowPlan};
use super::DeferReason;
use crate::plan::reference_globals::{global_bool, global_f64, global_i64};
use crate::reference::keys::{
    common as common_key, global as global_key, interface as interface_key,
};
use crate::reference::{
    ReferenceConfig, ReferenceInterface, ReferenceMode, ReferenceParams, ReferenceValue,
};

pub(super) fn effective_policy(
    interface: &ReferenceInterface,
    medium: &PlannedMedium,
    discovery: &InterfaceDiscoveryPlan,
    global_common: InterfaceCommonPolicy,
    global_announce_rate: AnnounceRateLimit,
    transport_enabled: bool,
) -> Result<EffectiveInterfacePolicy, DeferReason> {
    let bitrate = interface
        .bitrate
        .map(|bitrate| {
            BitrateBps::new(bitrate).ok_or(DeferReason::InvalidSetting {
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
        planned_announce_rate_limit(interface, global_announce_rate, transport_enabled)?;
    let common = interface_common_policy(interface, global_common)?;
    Ok(defaults.configured(ConfiguredInterfacePolicy {
        capabilities,
        mode: Some(planned_mode(interface, discovery)),
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
    global: AnnounceRateLimit,
    transport_enabled: bool,
) -> Result<Option<AnnounceRateLimit>, DeferReason> {
    let source = match (interface.announce_rate_target, transport_enabled) {
        (Some(target_seconds), _) => AnnounceRateSource::Interface { target_seconds },
        (None, true) => AnnounceRateSource::TransportDefault(global),
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
        .map_err(|_| DeferReason::InvalidSetting {
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

fn checked_milliseconds(seconds: u64, key: &'static str) -> Result<u64, DeferReason> {
    seconds
        .checked_mul(1_000)
        .ok_or(DeferReason::InvalidSetting { key })
}

fn interface_defaults(medium: &PlannedMedium) -> Result<InterfaceDefaults, DeferReason> {
    match medium {
        PlannedMedium::AutoWifi { .. } => Ok(wifi_core::DEFAULTS),
        PlannedMedium::TcpClient { .. }
        | PlannedMedium::TcpServer { .. }
        | PlannedMedium::Backbone { .. }
        | PlannedMedium::BackboneClient { .. } => Ok(tcp_core::DEFAULTS),
        PlannedMedium::Udp { .. } => Ok(udp_core::DEFAULTS),
        PlannedMedium::Serial { baud, .. } => {
            let bitrate = BitrateBps::new(u64::from(*baud)).ok_or(DeferReason::InvalidSetting {
                key: interface_key::SPEED,
            })?;
            Ok(serial_core::defaults_for_bitrate(bitrate))
        }
        PlannedMedium::Kiss { .. } => Ok(kiss_core::DEFAULTS),
        PlannedMedium::Ax25Kiss { .. } => Ok(ax25_core::DEFAULTS),
        PlannedMedium::Pipe { .. } => Ok(pipe_core::DEFAULTS),
        PlannedMedium::Rnode {
            bandwidth_hz,
            spreading_factor,
            coding_rate,
            ..
        } => {
            let raw =
                rnode_policy::nominal_bitrate_bps(*spreading_factor, *coding_rate, *bandwidth_hz);
            let bitrate = BitrateBps::new(u64::from(raw)).ok_or(DeferReason::InvalidSetting {
                key: "radio bitrate",
            })?;
            Ok(rnode_policy::defaults_for_bitrate(bitrate))
        }
    }
}

fn configured_mtu(interface: &ReferenceInterface) -> Result<Option<MtuPolicy>, DeferReason> {
    let fixed_mtu = match &interface.params {
        ReferenceParams::TcpClient { fixed_mtu, .. }
        | ReferenceParams::TcpServer { fixed_mtu, .. } => *fixed_mtu,
        _ => None,
    };
    fixed_mtu
        .map(|fixed_mtu| {
            if fixed_mtu > MAX_LINK_MTU {
                return Err(DeferReason::InvalidSetting {
                    key: interface_key::FIXED_MTU,
                });
            }
            MtuBytes::new(fixed_mtu)
                .map(MtuPolicy::Fixed)
                .ok_or(DeferReason::InvalidSetting {
                    key: interface_key::FIXED_MTU,
                })
        })
        .transpose()
}

fn planned_mode(
    interface: &ReferenceInterface,
    discovery: &InterfaceDiscoveryPlan,
) -> InterfaceMode {
    let configured = interface.mode.map(map_mode).unwrap_or(InterfaceMode::Full);
    if matches!(discovery, InterfaceDiscoveryPlan::Disabled)
        || matches!(
            configured,
            InterfaceMode::Gateway | InterfaceMode::AccessPoint
        )
    {
        return configured;
    }
    if matches!(
        interface.params,
        ReferenceParams::Rnode { .. } | ReferenceParams::RnodeMulti { .. }
    ) {
        InterfaceMode::AccessPoint
    } else {
        InterfaceMode::Gateway
    }
}

fn announce_bandwidth_cap(percent: f64) -> Result<AnnounceBandwidthCap, DeferReason> {
    if !percent.is_finite() || !(0.0..=100.0).contains(&percent) {
        return Err(DeferReason::InvalidSetting {
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
) -> Result<InterfaceCommonPolicy, DeferReason> {
    let mut common = global;
    common.forwarding = InterfaceForwardingPolicy {
        recursive_path_requests: interface
            .recursive_prs
            .unwrap_or(common.forwarding.recursive_path_requests),
        announces_from_internal: interface
            .announces_from_internal
            .unwrap_or(common.forwarding.announces_from_internal),
    };
    common.ingress_control.enabled = interface
        .ingress_control
        .unwrap_or(common.ingress_control.enabled);
    common.path_request_egress.enabled = interface
        .egress_control
        .unwrap_or(common.path_request_egress.enabled);
    if let Some(value) = interface.ic_max_held_announces {
        common.ingress_control.max_held_announces =
            usize::try_from(value).map_err(|_| DeferReason::InvalidSetting {
                key: common_key::IC_MAX_HELD_ANNOUNCES,
            })?;
    }
    apply_common_numbers(
        CommonNumberOverrides::from_interface(interface),
        &mut common,
    )?;
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
) -> Result<(), DeferReason> {
    if let Some(value) = configured.new_time {
        common.ingress_control.new_interface_ms =
            seconds_to_millis(value, common_key::IC_NEW_TIME)?;
    }
    if let Some(value) = configured.burst_hold {
        common.ingress_control.burst_hold_ms = seconds_to_millis(value, common_key::IC_BURST_HOLD)?;
    }
    if let Some(value) = configured.burst_penalty {
        common.ingress_control.burst_penalty_ms =
            seconds_to_millis(value, common_key::IC_BURST_PENALTY)?;
    }
    if let Some(value) = configured.held_release_interval {
        common.ingress_control.held_release_interval_ms =
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

fn seconds_to_millis(value: f64, key: &'static str) -> Result<u64, DeferReason> {
    let millis = (value * 1_000.0).round();
    if !value.is_finite() || value < 0.0 || millis >= u64::MAX as f64 {
        return Err(DeferReason::InvalidSetting { key });
    }
    Ok(millis as u64)
}

fn hertz_to_milli_hertz(value: f64, key: &'static str) -> Result<FrequencyMilliHertz, DeferReason> {
    let milli_hertz = (value * 1_000.0).round();
    if !value.is_finite() || value < 0.0 || milli_hertz >= u64::MAX as f64 {
        return Err(DeferReason::InvalidSetting { key });
    }
    Ok(FrequencyMilliHertz::new(milli_hertz as u64))
}

pub(in crate::plan) fn global_common_policy(config: &ReferenceConfig) -> InterfaceCommonPolicy {
    let mut common = InterfaceCommonPolicy::RNS_DEFAULT;
    common.path_request_egress.enabled =
        global_bool(&config.globals, common_key::EGRESS_CONTROL, false);
    if let Some(value) = global_i64(&config.globals, common_key::IC_MAX_HELD_ANNOUNCES) {
        common.ingress_control.max_held_announces = usize::try_from(value)
            .expect("validated ic_max_held_announces must fit the current platform");
    }
    apply_common_numbers(
        CommonNumberOverrides::from_globals(&config.globals),
        &mut common,
    )
    .expect("validated common interface controls must have representable values");
    common
}

pub(in crate::plan) fn global_announce_rate(config: &ReferenceConfig) -> AnnounceRateLimit {
    let seconds = |key, default| {
        global_i64(&config.globals, key)
            .and_then(|value| u64::try_from(value).ok())
            .unwrap_or(default)
    };
    let target_seconds = seconds(global_key::DEFAULT_AR_TARGET, 3_600);
    let penalty_seconds = seconds(global_key::DEFAULT_AR_PENALTY, 0);
    AnnounceRateLimit {
        target_ms: target_seconds
            .checked_mul(1_000)
            .expect("validated default_ar_target must fit milliseconds"),
        grace: seconds(global_key::DEFAULT_AR_GRACE, 5)
            .try_into()
            .expect("validated default_ar_grace must fit u16"),
        penalty_ms: penalty_seconds
            .checked_mul(1_000)
            .expect("validated default_ar_penalty must fit milliseconds"),
    }
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
