use super::*;

pub enum WorkerDisposition {
    Release,
    CloseLink,
    Retire,
}
pub fn worker(triple: &mut Triple<'_, '_>, disposition: WorkerDisposition) {
    use personal_rns::routing::links::resources::ResourceStrategy;
    use prns_runtime_tokio::runtime::{
        ControlledCryptoEvent, ControlledWorkKind, CryptoWorkBoundary,
    };
    let Some((_, _, control)) = triple.controls.iter().rev().find(|(node, generation, _)| {
        *node == PRIMARY && *generation == triple.generations[PRIMARY]
    }) else {
        overlap(triple, 0x80);
        return;
    };
    let control = control.clone();
    let Handle::Tokio(primary) = triple.nodes[PRIMARY].handle.clone() else {
        unreachable!("controlled worker belongs to Tokio");
    };
    let link = triple.app_links[0];
    let command = triple.issue(
        TARGET,
        PrnsCommand::SetResourceStrategy(SetResourceStrategy {
            link_id: link,
            strategy: ResourceStrategy::Accept {
                max_uncompressed_bytes: traffic::COMMON_RESPONSE_BYTES as u64,
                accept_compressed: false,
            },
        }),
    );
    let observed = triple.nodes[TARGET].events.clone();
    triple.drive_until(move || observed.snapshot().iter().any(|event| matches!(event, Event::Settled { id, settlement: CommandOutcome::Strategy(Ok(())) } if *id == command)));
    let before = control.trace().expect("worker evidence").len();
    control
        .hold(
            ControlledWorkKind::BuildResource,
            CryptoWorkBoundary::Publication,
        )
        .expect("hold result publication");
    let transfer = Operation::start(triple, async move {
        primary
            .send_resource_with_compression(
                link,
                traffic::COMMON_RESPONSE_BYTES as u64,
                &traffic::PAYLOAD[..traffic::COMMON_RESPONSE_BYTES],
                SegmentCompression::Never,
            )
            .await
    });
    let observed = control.clone();
    triple.drive_until(move || {
        observed.trace().expect("worker trace")[before..]
            .iter()
            .any(|event| matches!(event, ControlledCryptoEvent::Executed { .. }))
            && observed.snapshot().expect("occupancy").computed > 0
    });
    assert!(matches!(*transfer.state(), State::Pending));
    healthy(triple);
    match disposition {
        WorkerDisposition::Release => {
            control
                .release(
                    ControlledWorkKind::BuildResource,
                    CryptoWorkBoundary::Publication,
                )
                .expect("publish actual result");
            transfer.finish(triple).expect("built resource delivered");
        }
        WorkerDisposition::CloseLink => {
            transfer.cancel(triple);
            triple.nodes[PRIMARY].handle.close(link);
            triple.tasks.settle();
            control
                .release(
                    ControlledWorkKind::BuildResource,
                    CryptoWorkBoundary::Publication,
                )
                .expect("publish stale reservation");
            triple.tasks.settle();
            triple.reconnect(PRIMARY);
            overlap(triple, 0x82);
        }
        WorkerDisposition::Retire => {
            transfer.cancel(triple);
            triple.restart(PRIMARY);
            assert!(matches!(
                control.trace().expect("retired evidence").last(),
                Some(ControlledCryptoEvent::Retired { .. })
            ));
            assert!(matches!(
                control.snapshot(),
                Err(prns_runtime_tokio::runtime::ControlledCryptoError::Retired)
            ));
            overlap(triple, 0x81);
        }
    }
}
