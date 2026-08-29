#[cfg(target_os = "linux")]
mod linux_only {
    use core::time::Duration;

    use personal_rns::engine::{
        AnnounceAppData, AnnounceNow, AnnounceTarget, PrnsCommand, RatchetPolicy,
    };
    use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
    use personal_rns::request_endpoints;
    use personal_rns::routing::links::resources::ResourceStrategy;
    use personal_rns::routing::{LinkRequestPolicy, ProofStrategy};
    use personal_rns::runtime::{
        Diagnostic, ManuallyAttached, NoPersistence, PreConfiguredDestination, PrnsEvent, PrnsNode,
        PrnsNodeRecipe, ServeMyRequestEndpoints,
    };
    use personal_rns::storage::GrowableHeap;
    use prns_core::interfaces::wifi_direct::GoIntent;
    use prns_core::interfaces::wifi_direct::{
        Availability, WifiDirectBackend, WifiDirectEvent, WifiDirectGroup,
    };
    use prns_core::interfaces::{InterfaceStatus, MacAddress};
    use prns_interfaces_tokio::wifi_direct::supplicant::SupplicantBackend;
    use prns_interfaces_tokio::wifi_direct::wpa::WpaP2pBackend;
    use prns_interfaces_tokio::wifi_direct::WifiDirectAuto;

    const CROSSING_DEADLINE: Duration = Duration::from_secs(120);
    const ANNOUNCER_SECRET: u8 = 0xA7;
    const LISTENER_SECRET: u8 = 0xB8;

    fn secret(byte: u8) -> Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]> {
        Zeroizing::new([byte; IDENTITY_SECRET_KEY_LEN])
    }

    fn single(
        identity: Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>,
    ) -> PreConfiguredDestination<'static> {
        PreConfiguredDestination::Single {
            resource_strategy: ResourceStrategy::AcceptNone,
            app_name: "wifi-direct-smoke",
            aspects: &["announce"],
            identity,
            announce_app_data: b"",
            proof: ProofStrategy::ProveAll,
            link_requests: LinkRequestPolicy::AcceptAll,
            ratchet: RatchetPolicy::NoRatchets,
            maximum_request_bytes: Default::default(),
            request_endpoints: ServeMyRequestEndpoints::No,
        }
    }

    pub async fn run() {
        env_logger::init();
        let mut args = std::env::args().skip(1);
        let (Some(ifname), Some(role)) = (args.next(), args.next()) else {
            eprintln!("usage: wifi_direct_linux <ifname> <announce|expect>");
            std::process::exit(2);
        };

        match std::env::var("HOPSPOT_WIFI_DIRECT_CTRL") {
            Ok(ctrl_dir) => {
                let backend = match SupplicantBackend::attach(&ctrl_dir, &ifname).await {
                    Ok(backend) => backend,
                    Err(err) => {
                        eprintln!(
                            "WIFI_DIRECT_SMOKE[{role}] attach {ifname} at {ctrl_dir} failed: {err:?}"
                        );
                        std::process::exit(1);
                    }
                };
                println!("WIFI_DIRECT_SMOKE[{role}] attached {ifname} at {ctrl_dir}");
                if role == "host" {
                    host_selftest(backend).await;
                } else {
                    drive(&role, backend).await;
                }
            }
            Err(_) => {
                let backend = match WpaP2pBackend::open(&ifname).await {
                    Ok(backend) => backend,
                    Err(err) => {
                        eprintln!("WIFI_DIRECT_SMOKE[{role}] open {ifname} failed: {err:?}");
                        std::process::exit(1);
                    }
                };
                println!("WIFI_DIRECT_SMOKE[{role}] opened {ifname}");
                drive(&role, backend).await;
            }
        }
    }

    async fn drive<B: WifiDirectBackend + Send + 'static>(role: &str, backend: B) {
        match role {
            "announce" => announce_forever(backend).await,
            "expect" => expect_crossing(backend).await,
            _ => {
                eprintln!("usage: wifi_direct_linux <ifname> <announce|expect>");
                std::process::exit(2);
            }
        }
    }

    async fn host_selftest(mut backend: SupplicantBackend) {
        let placeholder = MacAddress::new([0x02, 0, 0, 0, 0, 0]);
        backend
            .form_group(placeholder, GoIntent::PREFER_OWNER)
            .await;
        println!("WIFI_DIRECT_SCC[host] form_group issued; awaiting outcome");
        loop {
            match backend.next_event().await {
                WifiDirectEvent::GroupFormed { group } => {
                    println!("WIFI_DIRECT_SCC[host] group formed role={:?}", group.role());
                }
                WifiDirectEvent::AvailabilityChanged(Availability::Unavailable(reason)) => {
                    println!("WIFI_DIRECT_SCC[host] unavailable: {reason}");
                }
                WifiDirectEvent::GroupLost { .. } => {
                    println!("WIFI_DIRECT_SCC[host] group lost");
                }
                _ => {}
            }
        }
    }

    async fn announce_forever<B: WifiDirectBackend + Send + 'static>(backend: B) {
        let single_a = single(secret(ANNOUNCER_SECRET));
        let Ok(dest) = single_a.destination_hash() else {
            eprintln!("WIFI_DIRECT_SMOKE[announce] destination derivation failed");
            std::process::exit(1);
        };
        let node = PrnsNode::new(PrnsNodeRecipe {
            remote_control: personal_rns::remote_control::RemoteControlService::Unavailable,
            transport_identity: None,
            pre_configured_destinations: [single_a],
            app_state: (),
            storage: GrowableHeap,
            request_endpoints: request_endpoints![],
            on_event: |_event, _state| {},
            interfaces: ManuallyAttached,
            persistence: NoPersistence,
        });
        let commands = node.handle();
        let auto = WifiDirectAuto::new(backend, GoIntent::PREFER_OWNER);
        let status = auto.status();
        let _sup = commands.supervise(auto);

        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_millis(500));
            loop {
                ticker.tick().await;
                if commands
                    .issue(PrnsCommand::AnnounceNow(AnnounceNow {
                        destination: dest,
                        target: AnnounceTarget::AllInterfaces,
                        app_data: AnnounceAppData::Registered,
                    }))
                    .is_none()
                {
                    break;
                }
            }
        });
        tokio::spawn(status_printer("announce", status));
        let result = node.run().await;
        eprintln!("WIFI_DIRECT_SMOKE[announce] run loop returned: {result:?}");
        std::process::exit(1);
    }

    async fn expect_crossing<B: WifiDirectBackend + Send + 'static>(backend: B) {
        let Ok(expected) = single(secret(ANNOUNCER_SECRET)).destination_hash() else {
            eprintln!("WIFI_DIRECT_SMOKE[expect] destination derivation failed");
            std::process::exit(1);
        };
        let (heard_tx, mut heard_rx) = tokio::sync::mpsc::unbounded_channel();
        let node = PrnsNode::new(PrnsNodeRecipe {
            remote_control: personal_rns::remote_control::RemoteControlService::Unavailable,
            transport_identity: None,
            pre_configured_destinations: [single(secret(LISTENER_SECRET))],
            app_state: (),
            storage: GrowableHeap,
            request_endpoints: request_endpoints![],
            on_event: move |event, _state| {
                if let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { destination, .. }) = event
                {
                    let _ = heard_tx.send(destination);
                }
            },
            interfaces: ManuallyAttached,
            persistence: NoPersistence,
        });
        let commands = node.handle();
        let auto = WifiDirectAuto::new(backend, GoIntent::PREFER_CLIENT);
        let status = auto.status();
        let _sup = commands.supervise(auto);
        tokio::spawn(status_printer("expect", status));

        let heard = tokio::select! {
            biased;
            heard = tokio::time::timeout(CROSSING_DEADLINE, heard_rx.recv()) => heard,
            result = node.run() => {
                eprintln!("WIFI_DIRECT_SMOKE[expect] run loop returned: {result:?}");
                std::process::exit(1);
            }
        };
        match heard {
            Ok(Some(destination)) if destination == expected => {
                println!("WIFI_DIRECT_SMOKE[expect] announce crossed the group: {destination:x?}");
            }
            Ok(Some(destination)) => {
                eprintln!("WIFI_DIRECT_SMOKE[expect] unexpected destination: {destination:x?}");
                std::process::exit(1);
            }
            Ok(None) => {
                eprintln!("WIFI_DIRECT_SMOKE[expect] the announce channel closed");
                std::process::exit(1);
            }
            Err(_) => {
                eprintln!("WIFI_DIRECT_SMOKE[expect] no announce within {CROSSING_DEADLINE:?}");
                std::process::exit(1);
            }
        }
    }

    async fn status_printer(
        role: &'static str,
        status: prns_interfaces_tokio::wifi_direct::WifiDirectStatus,
    ) {
        let mut ticker = tokio::time::interval(Duration::from_secs(2));
        loop {
            ticker.tick().await;
            println!("WIFI_DIRECT_SMOKE[{role}] status {:?}", status.connection());
        }
    }
}

#[cfg(target_os = "linux")]
#[tokio::main(flavor = "multi_thread")]
async fn main() {
    linux_only::run().await;
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("wifi_direct_linux drives wpa_supplicant; it runs on Linux only");
}
