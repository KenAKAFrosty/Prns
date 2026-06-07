//WIP NEEDS REVIEW
use core::convert::Infallible;
use std::sync::OnceLock;

use personal_rns::engine::self_announce::AnnounceConfig;
use personal_rns::engine::{IssuedCommand, RatchetPolicy, ReannounceSchedule};
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::storage::GrowableInterfaceSet;
use personal_rns::interfaces::substrate::StdInterfaceHandle;
use personal_rns::interfaces::StartedInterface;
use personal_rns::routing::storage::GrowableHeap;
use personal_rns::routing::ProofStrategy;
use personal_rns::runtime::host::impls::LinuxSync;
use personal_rns::runtime::{block_on, Prns, PrnsEvent, Recipe, StartingDestinationConfig};
use personal_rns::wire::MTU;

type NoInterfacesYet = GrowableInterfaceSet<StartedInterface<StdInterfaceHandle<MTU>, Infallible>>;

const SELF_ANNOUNCE_APP_NAME: &str = "lxmf";
const SELF_ANNOUNCE_ASPECTS: &[&str] = &["delivery"];
const SELF_ANNOUNCE_APP_DATA: &[u8] = b"personal-hopspot";
const ANNOUNCE_EVERY_MS: u64 = 8_000;

struct Engine;

static ENGINE: OnceLock<Engine> = OnceLock::new();

pub(crate) fn start() {
    let _ = ENGINE.get_or_init(spawn_engine);
}

fn spawn_engine() -> Engine {
    let _ = std::thread::Builder::new()
        .name("hopspot-engine".into())
        .spawn(run_engine);
    Engine
}

fn load_identity_secret_key() -> Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]> {
    let mut key = Zeroizing::new([0u8; IDENTITY_SECRET_KEY_LEN]);
    getrandom::getrandom(&mut *key).expect("OS CSPRNG must provide identity key material");
    key
}

fn run_engine() {
    println!("HOPSPOT_IOS_ENGINE starting: self-announce scheduled, no interfaces yet");
    let identity_secret_key = load_identity_secret_key();
    let host = LinuxSync::new();
    let interfaces = NoInterfacesYet::new();

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
        |event: PrnsEvent<'_>| {
            if let PrnsEvent::SnapshotUpdated(snapshot) = event {
                println!(
                    "HOPSPOT_IOS_ENGINE snapshot: interfaces={}",
                    snapshot.interfaces.len()
                );
            }
        },
        || -> Option<IssuedCommand> { None },
    ));
}
