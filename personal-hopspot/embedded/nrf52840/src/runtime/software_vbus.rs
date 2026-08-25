use embassy_nrf::pac;
use embassy_nrf::usb::vbus_detect::SoftwareVbusDetect;
use static_cell::StaticCell;

enum InitialUsbPower {
    Absent,
    Detected,
    Ready,
}

impl InitialUsbPower {
    fn snapshot() -> Self {
        let status = pac::POWER.usbregstatus().read();
        match (status.vbusdetect(), status.outputrdy()) {
            (false, _) => Self::Absent,
            (true, false) => Self::Detected,
            (true, true) => Self::Ready,
        }
    }

    const fn embassy_state(self) -> (bool, bool) {
        match self {
            Self::Absent => (false, false),
            Self::Detected => (true, false),
            Self::Ready => (true, true),
        }
    }
}

pub(crate) fn initialize(
    cell: &'static StaticCell<SoftwareVbusDetect>,
) -> &'static SoftwareVbusDetect {
    let (detected, ready) = InitialUsbPower::snapshot().embassy_state();
    &*cell.init(SoftwareVbusDetect::new(detected, ready))
}
