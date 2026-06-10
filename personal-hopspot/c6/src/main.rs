#![no_std]
#![no_main]

extern crate alloc;

use esp_backtrace as _;
use esp_bootloader_esp_idf::esp_app_desc;
use esp_hal::clock::CpuClock;
use esp_hal::efuse::base_mac_address;
use esp_hal::interrupt::software::SoftwareInterruptControl;
use esp_hal::peripherals::USB_DEVICE;
use esp_hal::rng::Rng;
use esp_hal::rtc_cntl::Rtc;
use esp_hal::timer::timg::TimerGroup;
use esp_hal::usb_serial_jtag::{UsbSerialJtag, UsbSerialJtagRx, UsbSerialJtagTx};
use esp_hal::Async;
use esp_println::println;

use embassy_executor::Spawner;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::{Duration, Ticker};

use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, CommandId, EngineCommand, EngineState,
    InstantMillis, IssuedCommand, Journaled, RatchetPolicy, SelfAnnounceAppData,
};
use personal_rns::identity::in_memory::InMemoryNodeIdentity;
use personal_rns::identity::{IdentitySigner, Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::substrate::EmbassyTimebase;
use personal_rns::interfaces::{ConnectionState, InterfaceId};
use personal_rns::reactor::impls::embassy_reactor::{
    run as run_reactor, EmbassyEgress, EmbassyHost, EmbassyInterfaceSeam, EmbassyInterfaceStatus,
};
use personal_rns::reactor::interface_seam::{InboundFrame, Interface, OutboundFrame};
use personal_rns::reactor::interfaces::usb_auto::core::device_descriptor;
use personal_rns::reactor::interfaces::usb_auto::impls::embassy::UsbAutoDevice;
use personal_rns::routing::announce::{derive_destination_hash, expand_name};
use personal_rns::routing::storage::FixedInline;
use personal_rns::routing::ProofStrategy;
use personal_rns::wire::DestinationHash;

esp_app_desc!();

const USB_INTERFACE_ID: InterfaceId = InterfaceId::new(*b"prsnl-hopspot-c6");

const SELF_ANNOUNCE_APP_DATA: &[u8] = b"\x92\xc4\x13Personal Hopspot C6\xc0";

const ANNOUNCE_INTERVAL: Duration = Duration::from_secs(8);

const INBOUND_CAP: usize = 8;
const OUTBOUND_CAP: usize = 8;
const COMMANDS_CAP: usize = 4;

type EngineStorageType = FixedInline<24, 32, 1024, 4, 128, 4, 4, 4, 4, 32, 8, 8, 8, 8, 8>;

static USB_STATUS: EmbassyInterfaceStatus =
    EmbassyInterfaceStatus::new(USB_INTERFACE_ID, ConnectionState::Initializing);

static FUNNEL: Channel<CriticalSectionRawMutex, InboundFrame, INBOUND_CAP> = Channel::new();
static OUTBOUND: Channel<CriticalSectionRawMutex, OutboundFrame, OUTBOUND_CAP> = Channel::new();
static COMMANDS: Channel<CriticalSectionRawMutex, IssuedCommand, COMMANDS_CAP> = Channel::new();

#[esp_rtos::main]
async fn main(spawner: Spawner) {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let p = esp_hal::init(config);

    esp_alloc::heap_allocator!(size: 64 * 1024);
    let timg0 = TimerGroup::new(p.TIMG0);
    let sw_int = SoftwareInterruptControl::new(p.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_int.software_interrupt0);

    let rtc = Rtc::new(p.LPWR);
    let timebase = EmbassyTimebase::start_at(InstantMillis(rtc.current_time_us() / 1000));

    println!("HOPSPOT_C6 boot — USB-auto on the reactor");

    let (usb_rx, usb_tx) = UsbSerialJtag::new(p.USB_DEVICE).into_async().split();
    spawner.spawn(usb_device_task(usb_rx, usb_tx).expect("device task fits the pool"));

    let self_destination = {
        let secret_key = fixture_identity_secret_key();
        let identity = InMemoryNodeIdentity::from_secret_key_bytes(&secret_key);
        let name = expand_name("lxmf", &["delivery"]).expect("the self-announce name is valid");
        derive_destination_hash(&identity.identity_hash(), &name)
    };
    let app_data =
        SelfAnnounceAppData::from_slice(SELF_ANNOUNCE_APP_DATA).expect("the lxmf app_data fits");
    spawner.spawn(announce_task(self_destination, app_data).expect("announce task fits"));

    let secret_key = fixture_identity_secret_key();
    spawner.spawn(engine_task(secret_key, timebase).expect("engine task fits"));
}

fn fixture_identity_secret_key() -> Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]> {
    let mut secret_key = Zeroizing::new([0u8; IDENTITY_SECRET_KEY_LEN]);
    secret_key[..32].fill(0x22);
    secret_key[32..].fill(0x11);
    let mac = base_mac_address();
    let mac_bytes = mac.as_bytes();
    for (i, byte) in mac_bytes.iter().enumerate() {
        secret_key[i] ^= byte;
        secret_key[32 + i] ^= byte;
    }
    secret_key
}

#[embassy_executor::task]
async fn engine_task(
    secret_key: Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>,
    timebase: EmbassyTimebase,
) {
    let mut engine = EngineState::<EngineStorageType>::new(secret_key);
    let node = engine.held_identity_hashes()[0];
    engine
        .set_transport_identity(&node)
        .expect("the held identity takes the transport role");
    let _ = engine
        .register_single_destination(
            &node,
            "lxmf",
            &["delivery"],
            ProofStrategy::ProveAll,
            RatchetPolicy::Ratcheted,
        )
        .expect("registers the lxmf.delivery destination");

    let host = EmbassyHost::new_with_timebase(timebase, |bytes: &mut [u8]| {
        Rng::new().read(bytes);
    });

    let interfaces = [device_descriptor(USB_INTERFACE_ID)];
    let lanes = [(USB_INTERFACE_ID, OUTBOUND.sender())];
    let egress = EmbassyEgress::new(&lanes);

    run_reactor(
        engine,
        &interfaces,
        host,
        FUNNEL.receiver(),
        COMMANDS.receiver(),
        egress,
        |_journaled: Journaled<'_>| {},
    )
    .await
}

#[embassy_executor::task]
async fn usb_device_task(rx: UsbSerialJtagRx<'static, Async>, tx: UsbSerialJtagTx<'static, Async>) {
    let mut last_sof = 0u16;
    let host_present = move || {
        let frame = USB_DEVICE::regs().fram_num().read().sof_frame_index().bits();
        let advanced = frame != last_sof;
        last_sof = frame;
        advanced
    };
    let device = UsbAutoDevice::new(USB_INTERFACE_ID, rx, tx, &USB_STATUS, host_present);
    let seam = EmbassyInterfaceSeam::new(USB_INTERFACE_ID, FUNNEL.sender(), OUTBOUND.receiver());
    device.run(seam).await
}

#[embassy_executor::task]
async fn announce_task(destination: DestinationHash, app_data: SelfAnnounceAppData) {
    let mut ticker = Ticker::every(ANNOUNCE_INTERVAL);
    let mut next_id = 0u64;
    loop {
        next_id += 1;
        let _ = COMMANDS.try_send(IssuedCommand {
            id: CommandId(next_id),
            command: EngineCommand::AnnounceNow(AnnounceNow {
                destination,
                target: AnnounceTarget::AllInterfaces,
                app_data: AnnounceAppData::Data(app_data.clone()),
            }),
        });
        ticker.next().await;
    }
}
