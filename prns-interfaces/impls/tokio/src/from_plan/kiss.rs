use core::time::Duration;

use prns_config::{
    ReadyCommandFlowControl as PlannedReadyCommandFlowControl, SerialLinePlan,
    StationIdentificationPlan,
};
use prns_core::interfaces::kiss::{ReadyCommandFlowControl, ReadyTimeout, StationIdWireFormat};
use prns_runtime::interfaces::kiss::TncConfig;

use crate::kiss::{KissInterface, KissSettings, DEFAULT_TNC_CONFIGURE_DELAY};
use crate::serial::open_host_serial_with_settings;

use super::{
    serial, station_identification, AttachmentResult, InterfaceConstruction, RECONNECT_POLICY,
};

const FLOW_CONTROL_TIMEOUT: ReadyTimeout = ReadyTimeout::new(Duration::from_secs(5));

pub(super) struct Configuration<'a> {
    pub(super) device: &'a str,
    pub(super) line: SerialLinePlan,
    pub(super) preamble_ms: u32,
    pub(super) txtail_ms: u32,
    pub(super) persistence: u8,
    pub(super) slottime_ms: u32,
    pub(super) flow_control: PlannedReadyCommandFlowControl,
    pub(super) station_id: &'a Option<StationIdentificationPlan>,
}

pub(super) fn stand_up(
    construction: InterfaceConstruction<'_>,
    configuration: Configuration<'_>,
) -> AttachmentResult {
    let Configuration {
        device,
        line,
        preamble_ms,
        txtail_ms,
        persistence,
        slottime_ms,
        flow_control,
        station_id,
    } = configuration;
    let line = serial::host_line(line);
    let open_path = device.to_string();
    let tnc = TncConfig {
        preamble_ms,
        txtail_ms,
        persistence,
        slottime_ms,
    };
    let station_identification =
        station_identification::runtime(station_id, StationIdWireFormat::KissPadded)?;
    let kiss = KissInterface::with_runtime_settings(
        move || {
            let open_path = open_path.clone();
            async move { open_host_serial_with_settings(&open_path, line) }
        },
        RECONNECT_POLICY,
        KissSettings {
            configure_delay: DEFAULT_TNC_CONFIGURE_DELAY,
            tnc,
            flow_control: runtime_flow_control(flow_control),
            station_identification,
            policy: construction.interface.policy,
            channel_tag: device.as_bytes(),
        },
    );
    let attached = construction.attach(kiss);
    Ok(attached.id())
}

pub(super) fn runtime_flow_control(
    planned: PlannedReadyCommandFlowControl,
) -> ReadyCommandFlowControl {
    match planned {
        PlannedReadyCommandFlowControl::Disabled => ReadyCommandFlowControl::Disabled,
        PlannedReadyCommandFlowControl::Enabled => {
            ReadyCommandFlowControl::WaitForReadyOrTimeout(FLOW_CONTROL_TIMEOUT)
        }
    }
}
