use core::fmt::Write as _;

use embassy_executor::Spawner;
use embassy_futures::join::join5;
use embassy_futures::select::{select, Either};
use embassy_nrf::usb::vbus_detect::SoftwareVbusDetect;
use embassy_nrf::usb::Driver;
use embassy_nrf::{bind_interrupts, config, peripherals, usb};
use embassy_nrf::gpio::{Level, Output, OutputDrive};
use embassy_nrf::interrupt::{self, InterruptExt, Priority};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::{Channel, Receiver, Sender};
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Timer};
use embassy_usb::class::cdc_acm::{CdcAcmClass, State};
use embassy_usb::{Builder, Config as UsbConfig};
use static_cell::StaticCell;

use nrf_softdevice::ble::{gatt_server, peripheral};
use nrf_softdevice::{raw, SocEvent, Softdevice};

use personal_rns::interfaces::bluetooth_auto::core::{
    encode_advertisement, fragments_of, BleAddress, Control, Dialect, Fragment, L2capPlan,
    Reassembler, BLE_HW_MTU, CONTROL_MAX_LEN, FRAGMENT_HEADER_LEN,
};
use personal_rns::interfaces::bluetooth_auto::seam::{
    BleBackend, BleEvent, BleLink, BleSink, BleSource,
};

type Mtx = CriticalSectionRawMutex;
type FrameBytes = heapless09::Vec<u8, BLE_HW_MTU>;
type GattValue = heapless09::Vec<u8, 244>;
type LogLine = heapless09::String<96>;

const CTRL_DEPTH: usize = 4;
const DATA_DEPTH: usize = 4;
const GATT_FRAGMENT_PAYLOAD: usize = 180;
const GATT_REASSEMBLY_CAP: usize = 600;
const NOTIFY_PACING: Duration = Duration::from_millis(15);

static LOG: Channel<Mtx, LogLine, 32> = Channel::new();

macro_rules! diag {
    ($($arg:tt)*) => {{
        let mut line: LogLine = heapless09::String::new();
        let _ = core::write!(&mut line, $($arg)*);
        let _ = LOG.try_send(line);
    }};
}

bind_interrupts!(struct Irqs {
    USBD => usb::InterruptHandler<peripherals::USBD>;
});

#[derive(Debug, Clone, Copy)]
pub struct Closed;

#[embassy_executor::task]
async fn softdevice_task(sd: &'static Softdevice, vbus: &'static SoftwareVbusDetect) -> ! {
    sd.run_with_callback(|event| match event {
        SocEvent::PowerUsbDetected => vbus.detected(true),
        SocEvent::PowerUsbPowerReady => vbus.ready(),
        SocEvent::PowerUsbRemoved => vbus.detected(false),
        _ => {}
    })
    .await
}

#[nrf_softdevice::gatt_service(uuid = "37145b00-442d-4a94-917f-8f42c5da28e3")]
struct ReticulumService {
    #[characteristic(uuid = "37145b00-442d-4a94-917f-8f42c5da28e7", write, notify)]
    control: GattValue,
    #[characteristic(uuid = "37145b00-442d-4a94-917f-8f42c5da28e8", write, notify)]
    data: GattValue,
}

#[nrf_softdevice::gatt_server]
struct Server {
    rns: ReticulumService,
}

fn softdevice_config() -> nrf_softdevice::Config {
    nrf_softdevice::Config {
        clock: Some(raw::nrf_clock_lf_cfg_t {
            source: raw::NRF_CLOCK_LF_SRC_RC as u8,
            rc_ctiv: 16,
            rc_temp_ctiv: 2,
            accuracy: raw::NRF_CLOCK_LF_ACCURACY_500_PPM as u8,
        }),
        conn_gatt: Some(raw::ble_gatt_conn_cfg_t { att_mtu: 247 }),
        ..Default::default()
    }
}

struct BleBridge {
    connected: Channel<Mtx, (), 2>,
    control_in: Channel<Mtx, Control, CTRL_DEPTH>,
    control_out: Channel<Mtx, Control, CTRL_DEPTH>,
    data_in: Channel<Mtx, FrameBytes, DATA_DEPTH>,
    data_out: Channel<Mtx, FrameBytes, DATA_DEPTH>,
    link_dead: Signal<Mtx, ()>,
    advertise: Signal<Mtx, ()>,
}

impl BleBridge {
    const fn new() -> Self {
        Self {
            connected: Channel::new(),
            control_in: Channel::new(),
            control_out: Channel::new(),
            data_in: Channel::new(),
            data_out: Channel::new(),
            link_dead: Signal::new(),
            advertise: Signal::new(),
        }
    }
}

static BRIDGE: BleBridge = BleBridge::new();

struct NrfBleBackend {
    connected: Receiver<'static, Mtx, (), 2>,
    bridge: &'static BleBridge,
}

impl NrfBleBackend {
    fn new(bridge: &'static BleBridge) -> Self {
        Self {
            connected: bridge.connected.receiver(),
            bridge,
        }
    }
}

impl BleBackend for NrfBleBackend {
    const MAX_PEERS: usize = 1;
    type Error = Closed;
    type Link = NrfBleLink;

    async fn set_advertising(&mut self, enabled: bool) -> Result<(), Closed> {
        if enabled {
            self.bridge.advertise.signal(());
        } else {
            self.bridge.advertise.reset();
        }
        Ok(())
    }

    async fn next_event(&mut self) -> BleEvent<NrfBleLink> {
        self.connected.receive().await;
        BleEvent::Inbound(NrfBleLink {
            control_in: self.bridge.control_in.receiver(),
            control_out: self.bridge.control_out.sender(),
            data_in: self.bridge.data_in.receiver(),
            data_out: self.bridge.data_out.sender(),
            link_dead: &self.bridge.link_dead,
        })
    }

    async fn dial(&mut self, _address: BleAddress) {}
}

struct NrfBleLink {
    control_in: Receiver<'static, Mtx, Control, CTRL_DEPTH>,
    control_out: Sender<'static, Mtx, Control, CTRL_DEPTH>,
    data_in: Receiver<'static, Mtx, FrameBytes, DATA_DEPTH>,
    data_out: Sender<'static, Mtx, FrameBytes, DATA_DEPTH>,
    link_dead: &'static Signal<Mtx, ()>,
}

impl BleLink for NrfBleLink {
    type Error = Closed;
    type Source = NrfBleSource;
    type Sink = NrfBleSink;

    fn dialect(&self) -> Dialect {
        Dialect::Native
    }

    fn address(&self) -> BleAddress {
        BleAddress::new([0u8; 6])
    }

    async fn control_send(&mut self, msg: &Control) -> Result<(), Closed> {
        match select(self.control_out.send(*msg), self.link_dead.wait()).await {
            Either::First(()) => Ok(()),
            Either::Second(()) => Err(Closed),
        }
    }

    async fn control_recv(&mut self) -> Result<Control, Closed> {
        match select(self.control_in.receive(), self.link_dead.wait()).await {
            Either::First(msg) => Ok(msg),
            Either::Second(()) => Err(Closed),
        }
    }

    async fn upgrade(&mut self, _plan: &L2capPlan) -> Result<(), Closed> {
        Ok(())
    }

    fn into_data(self) -> (NrfBleSource, NrfBleSink) {
        (
            NrfBleSource {
                data_in: self.data_in,
                link_dead: self.link_dead,
            },
            NrfBleSink {
                data_out: self.data_out,
                link_dead: self.link_dead,
            },
        )
    }
}

struct NrfBleSource {
    data_in: Receiver<'static, Mtx, FrameBytes, DATA_DEPTH>,
    link_dead: &'static Signal<Mtx, ()>,
}

impl BleSource for NrfBleSource {
    type Error = Closed;

    async fn recv_frame(&mut self, out: &mut [u8]) -> Result<usize, Closed> {
        match select(self.data_in.receive(), self.link_dead.wait()).await {
            Either::First(frame) => {
                let len = frame.len().min(out.len());
                out[..len].copy_from_slice(&frame[..len]);
                Ok(len)
            }
            Either::Second(()) => Err(Closed),
        }
    }
}

struct NrfBleSink {
    data_out: Sender<'static, Mtx, FrameBytes, DATA_DEPTH>,
    link_dead: &'static Signal<Mtx, ()>,
}

impl BleSink for NrfBleSink {
    type Error = Closed;

    async fn send_frame(&mut self, frame: &[u8]) -> Result<(), Closed> {
        let mut bytes = FrameBytes::new();
        bytes.extend_from_slice(frame).map_err(|_| Closed)?;
        match select(self.data_out.send(bytes), self.link_dead.wait()).await {
            Either::First(()) => Ok(()),
            Either::Second(()) => Err(Closed),
        }
    }
}

async fn driver(sd: &'static Softdevice, server: &Server, bridge: &'static BleBridge) -> ! {
    let control_out_rx = bridge.control_out.receiver();
    let data_out_rx = bridge.data_out.receiver();
    let control_in_tx = bridge.control_in.sender();
    let data_in_tx = bridge.data_in.sender();

    loop {
        bridge.advertise.wait().await;

        let mut adv_buf = [0u8; 31];
        let mut adv_len = encode_advertisement(&mut adv_buf).unwrap_or(0);
        let name = b"Prns";
        adv_buf[adv_len] = (1 + name.len()) as u8;
        adv_buf[adv_len + 1] = 0x09;
        adv_buf[adv_len + 2..adv_len + 2 + name.len()].copy_from_slice(name);
        adv_len += 2 + name.len();
        let scan_data = [0x05u8, 0x09, b'P', b'r', b'n', b's'];
        let adv = peripheral::ConnectableAdvertisement::ScannableUndirected {
            adv_data: &adv_buf[..adv_len],
            scan_data: &scan_data,
        };
        diag!("adv: advertising");
        let conn =
            match peripheral::advertise_connectable(sd, adv, &peripheral::Config::default()).await {
                Ok(conn) => conn,
                Err(_) => {
                    diag!("adv: error, retry");
                    Timer::after(Duration::from_millis(500)).await;
                    continue;
                }
            };

        diag!("link: CONNECTED");
        bridge.link_dead.reset();
        bridge.connected.send(()).await;

        let mut reassembler: Reassembler<GATT_REASSEMBLY_CAP> = Reassembler::new();

        let inbound = gatt_server::run(&conn, server, |event| match event {
            ServerEvent::Rns(rns) => match rns {
                ReticulumServiceEvent::ControlWrite(value) => {
                    diag!("gatt: control write {}b", value.len());
                    if let Some(ctrl) = Control::decode(&value) {
                        let _ = control_in_tx.try_send(ctrl);
                    } else {
                        diag!("gatt: control decode FAILED");
                    }
                }
                ReticulumServiceEvent::ControlCccdWrite { notifications } => {
                    diag!("gatt: control cccd notify={}", notifications);
                }
                ReticulumServiceEvent::DataWrite(value) => {
                    diag!("gatt: data write {}b", value.len());
                    if let Some(fragment) = Fragment::decode(&value) {
                        if let Some(frame) = reassembler.absorb(&fragment) {
                            let mut bytes = FrameBytes::new();
                            if bytes.extend_from_slice(frame).is_ok() {
                                let _ = data_in_tx.try_send(bytes);
                            }
                        }
                    }
                }
                ReticulumServiceEvent::DataCccdWrite { notifications } => {
                    diag!("gatt: data cccd notify={}", notifications);
                }
            },
        });

        let outbound = async {
            loop {
                match select(control_out_rx.receive(), data_out_rx.receive()).await {
                    Either::First(ctrl) => {
                        let mut buf = [0u8; CONTROL_MAX_LEN];
                        if let Some(n) = ctrl.encode(&mut buf) {
                            if let Ok(value) = GattValue::from_slice(&buf[..n]) {
                                match server.rns.control_notify(&conn, &value) {
                                    Ok(()) => diag!("gatt: control notify {}b", n),
                                    Err(_) => diag!("gatt: control notify ERR"),
                                }
                            }
                        }
                    }
                    Either::Second(frame) => {
                        for fragment in fragments_of(&frame, GATT_FRAGMENT_PAYLOAD) {
                            let mut buf = [0u8; FRAGMENT_HEADER_LEN + GATT_FRAGMENT_PAYLOAD];
                            if let Some(n) = fragment.encode(&mut buf) {
                                if let Ok(value) = GattValue::from_slice(&buf[..n]) {
                                    let _ = server.rns.data_notify(&conn, &value);
                                }
                            }
                            Timer::after(NOTIFY_PACING).await;
                        }
                    }
                }
            }
        };

        let _ = select(inbound, outbound).await;
        diag!("link: DISCONNECTED");
        bridge.link_dead.signal(());
    }
}

pub async fn run(spawner: Spawner) -> ! {
    let mut nrf_config = config::Config::default();
    nrf_config.gpiote_interrupt_priority = Priority::P2;
    nrf_config.time_interrupt_priority = Priority::P2;
    let p = embassy_nrf::init(nrf_config);

    let mut led = Output::new(p.P1_01, Level::High, OutputDrive::Standard);

    static SOFTWARE_VBUS: StaticCell<SoftwareVbusDetect> = StaticCell::new();
    let vbus = SOFTWARE_VBUS.init(SoftwareVbusDetect::new(true, true));

    interrupt::USBD.set_priority(Priority::P2);
    let usb_driver = Driver::new(p.USBD, Irqs, &*vbus);
    let mut usb_config = UsbConfig::new(0x1209, 0x0001);
    usb_config.manufacturer = Some("Stay Personal");
    usb_config.product = Some("Personal Hopspot (T-Echo BLE)");
    usb_config.serial_number = Some("PERSONAL-RNS-TECHO-BLE");
    usb_config.max_packet_size_0 = 64;
    static CONFIG_DESC: StaticCell<[u8; 256]> = StaticCell::new();
    static BOS_DESC: StaticCell<[u8; 256]> = StaticCell::new();
    static MSOS_DESC: StaticCell<[u8; 256]> = StaticCell::new();
    static CONTROL_BUF: StaticCell<[u8; 64]> = StaticCell::new();
    static USB_STATE: StaticCell<State> = StaticCell::new();
    let mut builder = Builder::new(
        usb_driver,
        usb_config,
        CONFIG_DESC.init([0; 256]),
        BOS_DESC.init([0; 256]),
        MSOS_DESC.init([0; 256]),
        CONTROL_BUF.init([0; 64]),
    );
    let mut class = CdcAcmClass::new(&mut builder, USB_STATE.init(State::new()), 64);
    let mut usb = builder.build();

    let sd = Softdevice::enable(&softdevice_config());
    let server = Server::new(sd).unwrap();
    spawner.spawn(softdevice_task(sd, vbus).expect("softdevice task fits"));

    BRIDGE.advertise.signal(());
    let mut backend = NrfBleBackend::new(&BRIDGE);

    let usb_fut = usb.run();

    let log_writer = async {
        loop {
            let line = LOG.receive().await;
            for chunk in line.as_bytes().chunks(60) {
                let _ = class.write_packet(chunk).await;
            }
            let _ = class.write_packet(b"\r\n").await;
        }
    };

    let heartbeat = async {
        let mut n = 0u32;
        loop {
            Timer::after(Duration::from_secs(1)).await;
            n = n.wrapping_add(1);
            diag!("alive {}", n);
        }
    };

    let exercise = async {
        loop {
            let BleEvent::Inbound(mut link) = backend.next_event().await else {
                continue;
            };
            diag!("seam: INBOUND link");
            led.set_low();
            loop {
                match link.control_recv().await {
                    Ok(ctrl) => {
                        diag!("seam: control recv -> echo");
                        if link.control_send(&ctrl).await.is_err() {
                            diag!("seam: control send failed (dead)");
                            break;
                        }
                    }
                    Err(_) => {
                        diag!("seam: link dead");
                        break;
                    }
                }
            }
            led.set_high();
        }
    };

    join5(
        usb_fut,
        log_writer,
        heartbeat,
        driver(sd, &server, &BRIDGE),
        exercise,
    )
    .await;
    loop {}
}
