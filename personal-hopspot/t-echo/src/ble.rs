use embassy_executor::Spawner;
use embassy_nrf::config;
use embassy_nrf::gpio::{Level, Output, OutputDrive};
use embassy_nrf::interrupt::Priority;
use embassy_time::{Duration, Timer};

use nrf_softdevice::ble::{gatt_server, peripheral};
use nrf_softdevice::{raw, Softdevice};

use personal_rns::interfaces::bluetooth_auto::core::encode_advertisement;

#[embassy_executor::task]
async fn softdevice_task(sd: &'static Softdevice) -> ! {
    sd.run().await
}

#[nrf_softdevice::gatt_service(uuid = "37145b00-442d-4a94-917f-8f42c5da28e3")]
struct ReticulumService {
    #[characteristic(uuid = "37145b00-442d-4a94-917f-8f42c5da28e7", write, notify)]
    control: heapless09::Vec<u8, 244>,
    #[characteristic(uuid = "37145b00-442d-4a94-917f-8f42c5da28e8", write, notify)]
    data: heapless09::Vec<u8, 244>,
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

async fn blink(led: &mut Output<'static>, count: usize, on_ms: u64, off_ms: u64) {
    for _ in 0..count {
        led.set_low();
        Timer::after(Duration::from_millis(on_ms)).await;
        led.set_high();
        Timer::after(Duration::from_millis(off_ms)).await;
    }
}

pub async fn run(spawner: Spawner) -> ! {
    let mut nrf_config = config::Config::default();
    nrf_config.gpiote_interrupt_priority = Priority::P2;
    nrf_config.time_interrupt_priority = Priority::P2;
    let p = embassy_nrf::init(nrf_config);

    let mut led = Output::new(p.P1_01, Level::High, OutputDrive::Standard);

    let sd = Softdevice::enable(&softdevice_config());
    blink(&mut led, 2, 250, 250).await;

    let _server = Server::new(sd).unwrap();
    spawner.spawn(softdevice_task(sd).expect("softdevice task fits"));

    loop {
        blink(&mut led, 1, 80, 0).await;

        let mut adv_buf = [0u8; 31];
        let mut adv_len = encode_advertisement(&mut adv_buf).unwrap_or(0);
        let name = b"Prns";
        adv_buf[adv_len] = (1 + name.len()) as u8;
        adv_buf[adv_len + 1] = 0x09;
        adv_buf[adv_len + 2..adv_len + 2 + name.len()].copy_from_slice(name);
        adv_len += 2 + name.len();

        let scan_data = [0x05u8, 0x09, b'P', b'r', b'n', b's'];
        let adv = peripheral::NonconnectableAdvertisement::ScannableUndirected {
            adv_data: &adv_buf[..adv_len],
            scan_data: &scan_data,
        };
        let mut cfg = peripheral::Config::default();
        cfg.timeout = Some(200);
        let _ = peripheral::advertise(sd, adv, &cfg).await;
    }
}
