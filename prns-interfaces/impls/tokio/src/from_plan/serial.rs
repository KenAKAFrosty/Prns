use prns_config::{SerialDataBits, SerialLinePlan, SerialParity, SerialStopBits};

use crate::serial::{
    open_host_serial_with_settings, HostSerialDataBits, HostSerialLineSettings, HostSerialParity,
    HostSerialStopBits, SerialInterface,
};

use super::{AttachmentResult, InterfaceConstruction, RECONNECT_POLICY};

pub(super) fn stand_up(
    construction: InterfaceConstruction<'_>,
    device: &str,
    line: SerialLinePlan,
) -> AttachmentResult {
    let line = host_line(line);
    let open_path = device.to_string();
    let serial = SerialInterface::with_policy(
        move || {
            let open_path = open_path.clone();
            async move { open_host_serial_with_settings(&open_path, line) }
        },
        RECONNECT_POLICY,
        construction.interface.policy,
        device.as_bytes(),
    );
    let attached = construction.attach(serial);
    Ok(attached.id())
}

pub(super) fn host_line(line: SerialLinePlan) -> HostSerialLineSettings {
    HostSerialLineSettings::new(
        line.baud(),
        match line.data_bits() {
            SerialDataBits::Five => HostSerialDataBits::Five,
            SerialDataBits::Six => HostSerialDataBits::Six,
            SerialDataBits::Seven => HostSerialDataBits::Seven,
            SerialDataBits::Eight => HostSerialDataBits::Eight,
        },
        match line.parity() {
            SerialParity::None => HostSerialParity::None,
            SerialParity::Even => HostSerialParity::Even,
            SerialParity::Odd => HostSerialParity::Odd,
        },
        match line.stop_bits() {
            SerialStopBits::One => HostSerialStopBits::One,
            SerialStopBits::Two => HostSerialStopBits::Two,
        },
    )
}

#[cfg(test)]
mod tests {
    use prns_config::PlannedMedium;

    use crate::serial::{HostSerialDataBits, HostSerialParity, HostSerialStopBits};

    use super::host_line;

    #[test]
    fn planned_line_reaches_the_host_transport_without_defaulting() {
        let plan = prns_config::parse_and_plan(
            "[interfaces]\n[[Serial]]\ntype = SerialInterface\nenabled = Yes\nport = test\nspeed = 57600\ndatabits = 7\nparity = odd\nstopbits = 2\n",
        )
        .expect("valid serial configuration")
        .value;
        let PlannedMedium::Serial { line, .. } = &plan.interfaces[0].medium else {
            panic!("serial medium expected")
        };
        let host = host_line(*line);
        assert_eq!(host.baud(), 57_600);
        assert_eq!(host.data_bits(), HostSerialDataBits::Seven);
        assert_eq!(host.parity(), HostSerialParity::Odd);
        assert_eq!(host.stop_bits(), HostSerialStopBits::Two);
    }
}
