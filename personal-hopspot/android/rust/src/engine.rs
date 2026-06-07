use std::sync::{Arc, Mutex, OnceLock};

use personal_hopspot_ui::CardKind;
use personal_rns::engine::self_announce::AnnounceConfig;
use personal_rns::engine::{IssuedCommand, RatchetPolicy, ReannounceSchedule};
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::impls::usb_auto::usb_auto_interface;
use personal_rns::interfaces::storage::{GrowableInterfaceSet, InterfaceSet};
use personal_rns::interfaces::InterfaceId;
use personal_rns::routing::storage::GrowableHeap;
use personal_rns::routing::ProofStrategy;
use personal_rns::runtime::host::impls::LinuxSync;
use personal_rns::runtime::{
    block_on, Prns, PrnsEvent, Recipe, RuntimeSnapshot, StartingDestinationConfig,
};

pub(crate) const USB_INTERFACE_ID: InterfaceId = InterfaceId::new([0xD0; 16]);
const MAX_BUFFERED_PACKETS: usize = 64;
const SELF_ANNOUNCE_APP_NAME: &str = "lxmf";
const SELF_ANNOUNCE_ASPECTS: &[&str] = &["delivery"];
const SELF_ANNOUNCE_APP_DATA: &[u8] = b"personal-hopspot";
const ANNOUNCE_EVERY_MS: u64 = 60_000;

pub(crate) type SharedSnapshot = Arc<Mutex<Option<RuntimeSnapshot>>>;

static ENGINE: OnceLock<SharedSnapshot> = OnceLock::new();

pub(crate) fn shared_snapshot() -> SharedSnapshot {
    ENGINE.get_or_init(start_engine).clone()
}

pub(crate) fn classify(id: InterfaceId) -> Option<(CardKind, &'static str)> {
    if id == USB_INTERFACE_ID {
        Some((CardKind::Usb, "USB"))
    } else {
        None
    }
}

fn start_engine() -> SharedSnapshot {
    let snapshot: SharedSnapshot = Arc::new(Mutex::new(None));
    let slot = snapshot.clone();
    let _ = std::thread::Builder::new()
        .name("hopspot-engine".into())
        .spawn(move || run_engine(slot));
    snapshot
}

fn load_identity_secret_key() -> Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]> {
    let mut key = Zeroizing::new([0u8; IDENTITY_SECRET_KEY_LEN]);
    getrandom::getrandom(&mut *key).expect("OS CSPRNG must provide identity key material");
    key
}

fn run_engine(slot: SharedSnapshot) {
    let identity_secret_key = load_identity_secret_key();
    let host = LinuxSync::new();
    let mut interfaces = GrowableInterfaceSet::new();
    let _ = interfaces.push(host.attach(usb_auto_interface(USB_INTERFACE_ID), MAX_BUFFERED_PACKETS));

    block_on(Prns::run(
        Recipe {
            engine_storage: GrowableHeap,
            starting_destinations: [StartingDestinationConfig::Single {
                app_name: SELF_ANNOUNCE_APP_NAME,
                aspects: SELF_ANNOUNCE_ASPECTS,
                identity_secret_key,
                proof_strategy: ProofStrategy::ProveAll,
                ratchet_policy: RatchetPolicy::Ratcheted,
                announce: Some(AnnounceConfig {
                    app_data: SELF_ANNOUNCE_APP_DATA,
                    schedule: ReannounceSchedule::every(ANNOUNCE_EVERY_MS),
                }),
            }],
            interfaces,
            host,
        },
        move |event: PrnsEvent<'_>| {
            if let PrnsEvent::SnapshotUpdated(snapshot) = event {
                if let Ok(mut guard) = slot.lock() {
                    *guard = Some(snapshot.clone());
                }
            }
        },
        || -> Option<IssuedCommand> { None },
    ));
}
