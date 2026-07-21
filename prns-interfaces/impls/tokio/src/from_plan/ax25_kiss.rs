use prns_config::{ReadyCommandFlowControl, SerialLinePlan};
use prns_runtime::interfaces::kiss::TncConfig;

use crate::ax25_kiss::{Ax25KissInterface, Ax25KissSettings};
use crate::kiss::DEFAULT_TNC_CONFIGURE_DELAY;
use crate::serial::open_host_serial_with_settings;

use super::{kiss, serial, AttachmentResult, InterfaceConstruction, RECONNECT_POLICY};

pub(super) struct Configuration<'a> {
    pub(super) device: &'a str,
    pub(super) line: SerialLinePlan,
    pub(super) preamble_ms: u32,
    pub(super) txtail_ms: u32,
    pub(super) persistence: u8,
    pub(super) slottime_ms: u32,
    pub(super) flow_control: ReadyCommandFlowControl,
    pub(super) callsign: &'a str,
    pub(super) ssid: u8,
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
        callsign,
        ssid,
    } = configuration;
    let line = serial::host_line(line);
    let open_path = device.to_string();
    let opened = Ax25KissInterface::with_policy(
        move || {
            let open_path = open_path.clone();
            async move { open_host_serial_with_settings(&open_path, line) }
        },
        RECONNECT_POLICY,
        Ax25KissSettings {
            configure_delay: DEFAULT_TNC_CONFIGURE_DELAY,
            tnc: TncConfig {
                preamble_ms,
                txtail_ms,
                persistence,
                slottime_ms,
            },
            flow_control: kiss::runtime_flow_control(flow_control),
            callsign,
            ssid,
            policy: construction.interface.policy,
            channel_tag: device.as_bytes(),
        },
    );
    let ax25 = opened?;
    let attached = construction.attach(ax25);
    Ok(attached.id())
}
