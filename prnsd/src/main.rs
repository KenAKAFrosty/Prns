//! The Personal Reticulum daemon: a configurable shared-instance node on the high-level [`Prns`]
//! runtime.
//!
//! It reads a stock RNS config the way a stock RNS user expects (`<dir>/config`, discovered along
//! RNS's own search order) and projects it onto a [`DaemonPlan`]. Then it elects its role on the
//! host's shared instance: with none running it becomes the instance — standing up the plan's
//! interfaces and serving the bus and control RPC for local apps (Sideband, NomadNet, MeshChat),
//! keyed on the node's own persistent identity; with one already running it defers, joining as a
//! client over that instance's bus and standing up none of its own, the honorable parity behavior a
//! stock RNS app follows. It announces itself as `lxmf.delivery` so it surfaces as a messageable
//! peer, and forwards others' traffic when the config enables the transport role.

// 100% safe Rust, compiler-enforced (rationale in personal-rns/src/lib.rs). The daemon is async
// glue around the engine; syscalls go through tokio/std, so no `unsafe`.
#![forbid(unsafe_code)]

mod cli;
mod construct;
mod identity;
mod splash;

use core::time::Duration;
use std::process;

use clap::Parser;

use personal_rns::config::{discover, plan, SharedInstance};
use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, EngineCommand, RatchetPolicy,
};
use personal_rns::identity::in_memory::InMemoryNodeIdentity;
use personal_rns::identity::IdentitySigner;
use personal_rns::routes;
use personal_rns::routing::ProofStrategy;
use personal_rns::runtime::{
    Diagnostic, Manual, PreConfiguredDestination, Prns, PrnsEvent, PrnsRecipe, TokioPrnsHandle,
};
use personal_rns::shared_instance::{
    join_shared_instance, InstancePorts, JoinError, OnExisting, Role, SharedInstanceIntent,
};
use personal_rns::storage::GrowableHeap;
use personal_rns::wire::{DestinationHash, TransportId};

/// The destination the daemon announces itself as: `lxmf.delivery`, the aspect LXMF apps
/// (Sideband/Columba) message — so the daemon surfaces as a real, messageable peer.
const ANNOUNCE_APP_NAME: &str = "lxmf";
const ANNOUNCE_ASPECTS: &[&str] = &["delivery"];
/// The `lxmf.delivery` announce app_data: `msgpack([display_name, stamp_cost])` = `fixarray(2)` ‖
/// `bin8("Personal rnsd")` ‖ `nil` — the shape LXMF emits, so apps surface the display name (the
/// `\x0d` length byte = 13 = the name's length).
const ANNOUNCE_APP_DATA: &[u8] = b"\x92\xc4\x0dPersonal rnsd\xc0";

/// How often the daemon re-announces itself (RNS default 6h; the first fires immediately).
const ANNOUNCE_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

/// The default config when a host has none yet: a single LAN auto-interface and a shared instance,
/// the same starting point RNS writes on first run.
const DEFAULT_CONFIG: &str = "[reticulum]\n\
    enable_transport = No\n\
    share_instance = Yes\n\
    [interfaces]\n\
      [[Default Interface]]\n\
        type = AutoInterface\n\
        interface_enabled = Yes\n";

async fn announce_loop(handle: TokioPrnsHandle, destination: DestinationHash) {
    let mut interval = tokio::time::interval(ANNOUNCE_INTERVAL);
    loop {
        interval.tick().await;
        handle.issue(EngineCommand::AnnounceNow(AnnounceNow {
            destination,
            target: AnnounceTarget::AllInterfaces,
            app_data: AnnounceAppData::Registered,
        }));
    }
}

fn log_event(event: PrnsEvent<'_>) {
    match event {
        PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard {
            destination,
            hops,
            source_interface,
        }) => println!(
            "RNSD_ANNOUNCE_HEARD destination={:02x?} hops={hops} kind={:?}",
            destination.as_bytes(),
            source_interface.kind(),
        ),
        PrnsEvent::Message(_) => println!("RNSD_RX_MESSAGE"),
        PrnsEvent::Diagnostic(_) => {}
    }
}

#[tokio::main]
async fn main() {
    let cli = cli::Cli::parse();
    splash::print(concat!(
        "Personal Reticulum daemon · v",
        env!("CARGO_PKG_VERSION")
    ));

    let discovered_config = discover(cli.config.as_deref());
    let config_text = match &discovered_config.reference {
        Some(path) => match std::fs::read_to_string(path) {
            Ok(text) => {
                println!("RNSD_CONFIG path={}", path.display());
                text
            }
            Err(error) => {
                eprintln!("RNSD_CONFIG_ERROR path={} error={error}", path.display());
                process::exit(1);
            }
        },
        None => {
            println!(
                "RNSD_CONFIG_DEFAULT dir={} (no config file; using a default AutoInterface shared instance)",
                discovered_config.dir.display()
            );
            DEFAULT_CONFIG.to_string()
        }
    };

    let reference = match personal_rns::config::reference::parse(&config_text) {
        Ok(reference) => reference,
        Err(error) => {
            eprintln!("RNSD_CONFIG_PARSE_ERROR {error}");
            process::exit(1);
        }
    };
    let plan = plan(&reference);

    let storage_dir = discovered_config.dir.join("storage");
    let secret = identity::load_or_seed_transport_identity(&storage_dir);
    let transport_secret = plan.transport.then(|| secret.clone());

    let announce_destination = PreConfiguredDestination::Single {
        resource_strategy: personal_rns::routing::links::resources::ResourceStrategy::AcceptNone,
        app_name: ANNOUNCE_APP_NAME,
        aspects: ANNOUNCE_ASPECTS,
        identity: secret,
        announce_app_data: ANNOUNCE_APP_DATA,
        proof: ProofStrategy::ProveAll,
        ratchet: RatchetPolicy::Ratcheted,
    };
    let destination = announce_destination
        .destination_hash()
        .expect("the lxmf.delivery name is valid");

    let prns = Prns::new(PrnsRecipe {
        transport_identity: transport_secret,
        pre_configured_destinations: [announce_destination],
        app_state: (),
        storage: GrowableHeap,
        routes: routes![],
        interfaces: Manual,
        on_event: |event, _state: &()| log_event(event),
    });
    let prns_handle = prns.handle();

    // Elect this node's role on the host's shared instance before standing up any interfaces: a
    // client defers to the running instance and rides its bus, standing up none of its own.
    match plan.shared_instance {
        SharedInstance::Enabled {
            instance_port,
            control_port,
        } => {
            let mut ports = InstancePorts::default();
            if let Some(bus) = instance_port {
                ports.bus = bus;
            }
            if let Some(control) = control_port {
                ports.control = control;
            }
            match join_shared_instance(
                &prns_handle,
                SharedInstanceIntent {
                    identity_dir: discovered_config.dir.clone(),
                    ports,
                    on_existing: OnExisting::JoinAsClient,
                },
            )
            .await
            {
                Ok(Role::BecameInstance) => {
                    println!(
                        "RNSD_BECAME_INSTANCE bus=127.0.0.1:{} rpc=127.0.0.1:{} (Sideband, NomadNet, MeshChat can connect)",
                        ports.bus, ports.control
                    );
                    construct::construct_interfaces(&prns_handle, &plan).await;
                }
                Ok(Role::JoinedAsClient { of }) => {
                    println!(
                        "RNSD_JOINED_AS_CLIENT of={of} (a shared instance is already running; deferring to it and riding its bus — it owns the interfaces, so this node stands up none of its own)"
                    );
                }
                Err(JoinError::InstanceAlreadyRunning { at }) => {
                    eprintln!("RNSD_INSTANCE_REFUSED at={at}");
                    process::exit(1);
                }
            }
        }
        SharedInstance::Disabled => {
            println!("RNSD_SHARED_INSTANCE disabled (standalone node)");
            construct::construct_interfaces(&prns_handle, &plan).await;
        }
    }

    tokio::spawn(announce_loop(prns_handle.clone(), destination));

    println!(
        "RNSD_READY transport={} deferred={}",
        plan.transport,
        plan.deferred.len(),
    );
    prns.run().await;
}
