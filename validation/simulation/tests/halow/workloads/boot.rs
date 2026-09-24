use super::{recovery, *};
use personal_rns::remote_control::{RemoteControlControllerAuthority, RemoteControlTargetAccess};
use personal_rns::runtime::RequestPathError;
use personal_rns::RemoteControlTargetAccessControl;

const WIRED_BOOT_WINDOW_MS: u64 = 10_000;
const RADIO_BIND_BUDGET_MS: u64 = 8_000;
const WIRED_BOOTS: u8 = 2;

fn isolate_unicast(lab: &mut Lab<'_>) {
    for (from, to) in [(TARGET, HEALTHY), (HEALTHY, TARGET)] {
        lab.medium.set_path(
            lab.nodes[from].radio,
            lab.nodes[to].radio,
            PathState::BroadcastOnly,
        );
    }
}

pub(super) fn missing_unicast_paths(lab: &mut Lab<'_>) {
    recovery::gateway(lab);
    isolate_unicast(lab);
    let handle = lab.nodes[PRIMARY].handle.clone();
    lab.complete(async move {
        handle
            .set_remote_control_target_access(
                RemoteControlTargetAccess::new(
                    target(TARGET),
                    RemoteControlControllerAuthority::Operator,
                    permissions(),
                )
                .expect("explicit target pin"),
            )
            .await
            .expect("local access");
    });
    recovery::announce_page(lab, TARGET);
    recovery::announce_page(lab, HEALTHY);
    assert!(
        lab.announces
            .borrow()
            .iter()
            .any(|seen| seen.node == HEALTHY
                && seen.destination == page(TARGET).destination_hash().expect("page hash")),
        "broadcast reached the gateway"
    );
    let handle = lab.nodes[PRIMARY].handle.clone();
    let task = lab.insert(async move {
        Event::Path(
            handle
                .request_path(target(TARGET).endpoint().destination_hash())
                .await,
        )
    });
    let Event::Path(result) = lab.wait(task, 20_000) else {
        panic!("path actor");
    };
    assert_eq!(
        result,
        Err(RequestPathError::Failed(
            personal_rns::engine::RequestPathFailure::Timeout
        ))
    );

    recovery::gateway_at(lab, TARGET);
    isolate_unicast(lab);
    recovery::discover(lab);
    let link = lab.link(PRIMARY);
    lab.app(PRIMARY, link, b"wired-during-missing-kernel-paths");
    recovery::announce_page(lab, TARGET);
    recovery::fetch_page(lab, TARGET);

    recovery::gateway(lab);
    recovery::announce_page(lab, TARGET);
    recovery::announce_page(lab, HEALTHY);
    recovery::discover(lab);
    let link =
        recovery::connection(lab).expect("unicast path setup restores authenticated control");
    lab.app(PRIMARY, link, b"paths-ready-without-app-restart");
    recovery::fetch_page(lab, TARGET);
}

pub(super) fn delayed_radio_boot(lab: &mut Lab<'_>, device: &DeviceControl) {
    for boot in 0..WIRED_BOOTS {
        recovery::gateway_at(lab, TARGET);
        assert_eq!(device.radio(), None);
        assert!(lab.peer_ids(TARGET).is_empty());
        let link = lab.link(PRIMARY);
        lab.app(PRIMARY, link, &[boot]);
        recovery::announce_page(lab, TARGET);
        recovery::fetch_page(lab, TARGET);
        assert!(lab.advance(WIRED_BOOT_WINDOW_MS).is_empty());
        assert_eq!(device.radio(), None);
        lab.app(PRIMARY, link, b"wired-beyond-radio-startup");
        if boot + 1 < WIRED_BOOTS {
            for index in 0..NODE_COUNT {
                lab.flush(index);
            }
            lab.restart(TARGET);
        }
    }
    assert!(
        device.attempts() <= 16,
        "missing-radio retries remain bounded"
    );
    device.neighbor(lab.nodes[HEALTHY].radio);
    device.set_presence(Presence::Present);
    let mut elapsed = 0;
    while device.radio().is_none() {
        assert!(
            elapsed < RADIO_BIND_BUDGET_MS,
            "late radio binds without restarting the app"
        );
        assert!(lab.advance(1).is_empty());
        elapsed += 1;
    }
    lab.nodes[TARGET].radio = device.radio().expect("bound radio");
    recovery::gateway(lab);
    recovery::announce_page(lab, TARGET);
    recovery::announce_page(lab, HEALTHY);
    recovery::fetch_page(lab, TARGET);
    recovery::discover(lab);
    let link = recovery::connection(lab).expect("over-air control with the retained grant");
    lab.app(PRIMARY, link, b"late-radio-ready");
    assert_eq!(lab.peer_ids(TARGET), [scope(TARGET).peer_id(mac(HEALTHY))]);
    let attempts = device.attempts();
    assert!(lab.advance(WIRED_BOOT_WINDOW_MS).is_empty());
    assert_eq!(
        device.attempts(),
        attempts,
        "a healthy binding has no retry polling"
    );
    lab.measurements.push(Measurement::ColdRadioBoot {
        wired_boots: WIRED_BOOTS,
        rebind_ticks: elapsed,
        open_attempts: attempts,
    });
}
