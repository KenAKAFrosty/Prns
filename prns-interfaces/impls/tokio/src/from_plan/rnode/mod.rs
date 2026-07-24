mod multi;

use prns_config::{
    AirtimeLimitCentiPercent, RNodeTransportPlan,
    ReadyCommandFlowControl as PlannedReadyCommandFlowControl, StationIdentificationPlan,
};
use prns_core::interfaces::kiss::{ReadyCommandFlowControl, StationIdWireFormat};
use prns_runtime::interfaces::rnode::protocol::{RadioConfig, RadioConfigInput};

use crate::rnode::host::RNodeHostOpener;
use crate::rnode::{RNodeInterface, RNodeSettings};

use super::{station_identification, AttachmentResult, InterfaceConstruction, RECONNECT_POLICY};

pub(super) use multi::stand_up as stand_up_multi;

pub(super) struct Configuration<'a> {
    pub(super) transport: &'a RNodeTransportPlan,
    pub(super) frequency_hz: u64,
    pub(super) bandwidth_hz: u32,
    pub(super) tx_power_dbm: i16,
    pub(super) spreading_factor: u8,
    pub(super) coding_rate: u8,
    pub(super) flow_control: PlannedReadyCommandFlowControl,
    pub(super) station_id: &'a Option<StationIdentificationPlan>,
    pub(super) airtime_limit_short: Option<AirtimeLimitCentiPercent>,
    pub(super) airtime_limit_long: Option<AirtimeLimitCentiPercent>,
}

pub(super) fn stand_up(
    construction: InterfaceConstruction<'_>,
    configuration: Configuration<'_>,
) -> AttachmentResult {
    let Configuration {
        transport,
        frequency_hz,
        bandwidth_hz,
        tx_power_dbm,
        spreading_factor,
        coding_rate,
        flow_control,
        station_id,
        airtime_limit_short,
        airtime_limit_long,
    } = configuration;
    let radio = RadioConfig::new(RadioConfigInput {
        frequency_hz,
        bandwidth_hz,
        tx_power_dbm,
        spreading_factor,
        coding_rate,
        airtime_limit_short_centi_percent: airtime_limit_short.map(|limit| limit.get()),
        airtime_limit_long_centi_percent: airtime_limit_long.map(|limit| limit.get()),
    })?;
    let station_identification =
        station_identification::runtime(station_id, StationIdWireFormat::Exact)?;
    let opener = RNodeHostOpener::new(transport.clone());
    let channel_tag = transport.channel_tag();
    let detect_timeout = opener.detect_timeout();
    let keepalive = opener.keepalive();
    let rnode = RNodeInterface::with_runtime_settings(
        move || {
            let opener = opener.clone();
            async move { opener.open().await }
        },
        RECONNECT_POLICY,
        RNodeSettings {
            reset_delay: crate::rnode::DEFAULT_RNODE_RESET_DELAY,
            detect_timeout,
            keepalive,
            radio,
            flow_control: runtime_flow_control(flow_control),
            station_identification,
            policy: construction.interface.policy,
            channel_tag: &channel_tag,
        },
    );
    let attached = construction.attach(rnode);
    Ok(attached.id())
}

pub(super) fn runtime_flow_control(
    planned: PlannedReadyCommandFlowControl,
) -> ReadyCommandFlowControl {
    match planned {
        PlannedReadyCommandFlowControl::Disabled => ReadyCommandFlowControl::Disabled,
        PlannedReadyCommandFlowControl::Enabled => ReadyCommandFlowControl::WaitForReady,
    }
}
