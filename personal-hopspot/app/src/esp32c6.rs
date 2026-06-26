//! The Seeed XIAO ESP32-C6 Hopspot board: single-core, no-PSRAM, headless. The engine is constructed
//! and run on the one RISC-V core (no second-core hand-off like the S3), with its columns inline in
//! internal SRAM. USB-Serial-JTAG carries diagnostics only — there is no USB network interface. First
//! light brings the node up with zero interfaces and a heartbeat log; BLE is the first real interface.

use esp_backtrace as _;
use esp_bootloader_esp_idf::esp_app_desc;
use esp_hal::clock::CpuClock;
use esp_hal::efuse::{base_mac_address, MacAddress};
use esp_hal::interrupt::software::SoftwareInterruptControl;
use esp_hal::rtc_cntl::Rtc;
use esp_hal::timer::timg::TimerGroup;
use esp_println::println;

use embassy_executor::Spawner;
use embassy_futures::join::join;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::{Duration, Timer};
use heapless::Vec as HVec;
use portable_atomic::{AtomicU64, Ordering};
use static_cell::StaticCell;

use personal_rns::engine::{InstantMillis, IssuedCommand, RatchetPolicy};
use personal_rns::identity::in_memory::InMemoryNodeIdentity;
use personal_rns::identity::{IdentitySigner, Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::substrate::EmbassyTimebase;
use personal_rns::interfaces::InterfaceId;
use personal_rns::reactor::impls::embassy_reactor::{
    EmbassyGrantConsumer, EmbassyGrantProducer, EmbassyHost, InterfaceLifecycle, PooledEgress,
};
use personal_rns::reactor::interface_seam::EMBEDDED_MAX_WIRE_FRAME_LEN;
use personal_rns::runtime::{
    CompletionPool, EmbassyInterfaceStore, EmbassyPrnsHandle, PreConfiguredDestination, Prns,
    PrnsEvent, PrnsRecipe, ReactorPlumbing,
};
use personal_rns::wire::TransportId;

use crate::engine_storage::{C6Storage, EngineStorageType};

esp_app_desc!();

const ANNOUNCE_APP_DATA: &[u8] = b"\x92\xc4\x13Personal Hopspot C6\xc0";

const IFACES: usize = 1;
const MAX_IFACES: usize = 4;
const NOTIFY_CAP: usize = 16;
const COMMANDS_CAP: usize = 8;
const LIFECYCLE_CAP: usize = 16;
const COMPLETIONS_CAP: usize = 4;
const STORE_CAP: usize = 16;
const HEAP_BYTES: usize = 32 * 1024;
const HEARTBEAT_INTERVAL: Duration = Duration::from_millis(1000);

type Mtx = CriticalSectionRawMutex;
type ReactorInbound = HVec<
    (
        InterfaceId,
        EmbassyGrantConsumer<'static, Mtx, EMBEDDED_MAX_WIRE_FRAME_LEN>,
    ),
    IFACES,
>;
type ReactorEgressLanes = HVec<
    (
        InterfaceId,
        EmbassyGrantProducer<'static, Mtx, EMBEDDED_MAX_WIRE_FRAME_LEN>,
    ),
    IFACES,
>;
type Node = Prns<
    (),
    (),
    for<'a> fn(PrnsEvent<'a>, &()),
    EngineStorageType,
    EmbassyHost<fn(&mut [u8])>,
    Mtx,
    EMBEDDED_MAX_WIRE_FRAME_LEN,
    IFACES,
    MAX_IFACES,
    NOTIFY_CAP,
    COMMANDS_CAP,
    LIFECYCLE_CAP,
    COMPLETIONS_CAP,
>;

static NOTIFY: Channel<Mtx, InterfaceId, NOTIFY_CAP> = Channel::new();
static COMMANDS: Channel<Mtx, IssuedCommand, COMMANDS_CAP> = Channel::new();
static LIFECYCLE: Channel<Mtx, InterfaceLifecycle, LIFECYCLE_CAP> = Channel::new();
static COMPLETION: CompletionPool<Mtx, COMPLETIONS_CAP> = CompletionPool::new();
static INTERFACE_COUNTS: EmbassyInterfaceStore<Mtx, STORE_CAP> = EmbassyInterfaceStore::new();
static ENTROPY_STATE: AtomicU64 = AtomicU64::new(0x9e37_79b9_7f4a_7c15);

fn seeded_entropy(bytes: &mut [u8]) {
    let mut state = ENTROPY_STATE.load(Ordering::Relaxed);
    for byte in bytes {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        *byte = (state >> 24) as u8;
    }
    ENTROPY_STATE.store(state, Ordering::Relaxed);
}

fn ignore_events(_event: PrnsEvent<'_>, _state: &()) {}

fn c6_secret_key(mac: &MacAddress) -> Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]> {
    let mut secret_key = Zeroizing::new([0u8; IDENTITY_SECRET_KEY_LEN]);
    secret_key[..32].fill(0x22);
    secret_key[32..].fill(0x11);
    for (i, byte) in mac.as_bytes().iter().enumerate() {
        secret_key[i] ^= byte;
        secret_key[32 + i] ^= byte;
    }
    secret_key
}

pub async fn run(_spawner: Spawner) {
    esp_println::logger::init_logger_from_env();
    esp_alloc::heap_allocator!(size: HEAP_BYTES);

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let p = esp_hal::init(config);

    let timg0 = TimerGroup::new(p.TIMG0);
    let sw_int = SoftwareInterruptControl::new(p.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_int.software_interrupt0);

    let mut rtc = Rtc::new(p.LPWR);
    rtc.rwdt.disable();
    rtc.swd.disable();
    let timebase = EmbassyTimebase::start_at(InstantMillis(rtc.current_time_us() / 1000));

    println!("HOPSPOT_XIAO_C6 boot — single-core RISC-V reactor, headless (USB = diagnostics)");

    let mac = base_mac_address();
    let secret_key = c6_secret_key(&mac);

    let (self_destination, transport_id) = {
        let signer = InMemoryNodeIdentity::from_secret_key_bytes(&secret_key);
        let name = personal_rns::routing::announce::expand_name("lxmf", &["delivery"])
            .expect("valid name");
        let destination =
            personal_rns::routing::announce::derive_destination_hash(&signer.identity_hash(), &name);
        let transport = TransportId::new(*signer.identity_hash().as_bytes());
        (destination, transport)
    };
    let seed = self_destination.as_bytes();
    ENTROPY_STATE.store(
        u64::from_le_bytes([
            seed[0], seed[1], seed[2], seed[3], seed[4], seed[5], seed[6], seed[7],
        ]) | 1,
        Ordering::Relaxed,
    );
    println!(
        "mac={:02x?} lxmf.delivery={:02x?} transport={:02x?}",
        mac.as_bytes(),
        self_destination.as_bytes(),
        transport_id.as_bytes(),
    );

    let inbound: ReactorInbound = HVec::new();
    let egress_lanes: ReactorEgressLanes = HVec::new();

    let handle = EmbassyPrnsHandle::new(COMMANDS.sender(), &COMPLETION);
    let plumbing = ReactorPlumbing::new(
        inbound,
        PooledEgress::new(egress_lanes),
        NOTIFY.receiver(),
        COMMANDS.receiver(),
        LIFECYCLE.receiver(),
        handle,
    );
    let host = EmbassyHost::new_with_timebase(timebase, seeded_entropy as fn(&mut [u8]));

    static NODE: StaticCell<Node> = StaticCell::new();
    let node: &'static mut Node = NODE.init(Prns::new(
        PrnsRecipe {
            transport: Some(transport_id),
            pre_configured_destinations: [PreConfiguredDestination::Single {
                resource_strategy: personal_rns::routing::links::resources::ResourceStrategy::AcceptNone,
                app_name: "lxmf",
                aspects: &["delivery"],
                identity: secret_key,
                announce_app_data: ANNOUNCE_APP_DATA,
                proof: personal_rns::routing::ProofStrategy::ProveAll,
                ratchet: RatchetPolicy::Ratcheted,
            }],
            app_state: (),
            storage: C6Storage,
            routes: personal_rns::routes![],
            interfaces: personal_rns::interfaces![],
            on_event: ignore_events as for<'a> fn(PrnsEvent<'a>, &()),
        },
        plumbing,
        host,
        HVec::new(),
    ));
    node.set_interface_store(&INTERFACE_COUNTS);

    println!("[mem] post-construction (engine columns inline in SRAM, no PSRAM)");
    println!("{}", esp_alloc::HEAP.stats());

    let heartbeat = async {
        let mut tick: u32 = 0;
        loop {
            Timer::after(HEARTBEAT_INTERVAL).await;
            tick += 1;
            println!("c6 hb tick={tick} free_heap={}", esp_alloc::HEAP.free());
        }
    };

    join(node.run_reactor(), heartbeat).await;
}
