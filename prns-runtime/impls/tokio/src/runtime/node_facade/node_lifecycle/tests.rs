use std::sync::{Arc, Mutex};

use crate::engine::{InstantMillis, Journaled};
use crate::interfaces::InterfaceId;
use crate::routing::announce::AnnounceObservation;
use crate::wire::DestinationHash;

use super::{notify_accepted_announce, AcceptedAnnounceObserver};

#[test]
fn accepted_announce_observers_receive_the_complete_observation() {
    let captured = Arc::new(Mutex::new(None));
    let sink = captured.clone();
    let mut observer: Option<AcceptedAnnounceObserver> =
        Some(Box::new(move |observation: AnnounceObservation<'_>| {
            *sink.lock().unwrap() = Some((
                observation.destination,
                observation.announced_identity,
                observation.hops,
                observation.source_interface,
                observation.arrived_at,
                observation.app_data.to_vec(),
                observation.is_path_response,
            ));
        }));
    let app_data = [0x42, 0x43, 0x44];
    let observation = AnnounceObservation {
        destination: DestinationHash::new([0x11; 16]),
        announced_identity: crate::identity::IdentityHash::new([0x22; 16]),
        hops: crate::units::HopCount(3),
        source_interface: InterfaceId::new([0x33; 8]),
        arrived_at: InstantMillis(4_000),
        app_data: &app_data,
        is_path_response: false,
    };

    notify_accepted_announce(
        &mut observer,
        &Journaled::AnnounceHeard {
            observation,
            rate_accounting: crate::routing::announce::AnnounceRateAccounting::NotApplied,
        },
    );

    assert_eq!(
        *captured.lock().unwrap(),
        Some((
            observation.destination,
            observation.announced_identity,
            observation.hops,
            observation.source_interface,
            observation.arrived_at,
            app_data.to_vec(),
            observation.is_path_response,
        ))
    );
}
