use super::*;
use fixture::{HEALTHY, PRIMARY};
use personal_rns::remote_control::{
    RemoteControlAppMessage, RemoteControlRequest, RemoteControlResponse,
};

#[test]
fn three_nodes_replay_independent_controllers_through_ten_execution_profiles() {
    for profile in profile::all() {
        let run = || {
            fixture::with_triple(profile.clone(), 42, |triple| {
                for controller in [PRIMARY, HEALTHY] {
                    let payload = RemoteControlAppMessage::from_slice(&[controller as u8, 0x42])
                        .expect("bounded");
                    assert_eq!(
                        triple.exchange(
                            controller,
                            RemoteControlRequest::AppMessage(payload.clone())
                        ),
                        Ok(RemoteControlResponse::AppMessage(payload))
                    );
                }
                assert_eq!(triple.messages.0.borrow().len(), 2);
                for (invocation, index) in triple.messages.0.borrow().iter().zip([PRIMARY, HEALTHY])
                {
                    assert_eq!(
                        invocation.identity,
                        crate::remote_control::secrets(index)
                            .identities()
                            .controller()
                            .identity_hash()
                    );
                }
            })
        };
        assert_eq!(run(), run(), "three-node replay {profile:?}");
    }
}

#[test]
fn resources_requests_and_channels_overlap_without_cross_controller_delivery() {
    use crate::node_events::{CommandOutcome, Event};
    use personal_rns::engine::{PrnsCommand, SendToChannel, SendToChannelBody};
    use personal_rns::routing::links::channel::MessageType;
    for profile in profile::all() {
        fixture::with_triple(profile.clone(), 0x5eed, |triple| {
            let primary = triple.nodes[PRIMARY].handle.clone();
            let healthy = triple.nodes[HEALTHY].handle.clone();
            let app_link = triple.app_links[0];
            let control_link = triple.links[1];
            let before = triple.nodes[fixture::TARGET].events.snapshot().len();
            let body = SendToChannelBody::from_slice(b"owned-channel").expect("bounded channel");
            let command = triple.issue(
                PRIMARY,
                PrnsCommand::SendToChannel(SendToChannel {
                    link_id: app_link,
                    message_type: MessageType(0x3344),
                    body,
                }),
            );
            let payload = RemoteControlAppMessage::from_slice(b"healthy").expect("bounded");
            let expected = payload.clone();
            let results = triple.complete(async move {
                tokio::join!(
                    fixture::app_request(
                        primary.clone(),
                        app_link,
                        traffic::RESOURCE_PATH,
                        (traffic::COMMON_RESPONSE_BYTES as u16)
                            .to_be_bytes()
                            .to_vec()
                    ),
                    fixture::app_request(
                        primary,
                        app_link,
                        traffic::ECHO_PATH,
                        b"ordinary".to_vec()
                    ),
                    fixture::exchange(
                        healthy,
                        control_link,
                        RemoteControlRequest::AppMessage(payload)
                    ),
                )
            });
            assert_eq!(
                results,
                (
                    Ok(traffic::PAYLOAD[..traffic::COMMON_RESPONSE_BYTES].to_vec()),
                    Ok(b"ordinary".to_vec()),
                    Ok(RemoteControlResponse::AppMessage(expected))
                ),
                "mixed results {profile:?}; events {:?}",
                triple.events()
            );
            assert_eq!(
                triple
                    .messages
                    .0
                    .borrow()
                    .last()
                    .expect("healthy handler")
                    .identity,
                crate::remote_control::secrets(HEALTHY)
                    .identities()
                    .controller()
                    .identity_hash()
            );
            let target_events = triple.nodes[fixture::TARGET].events.snapshot();
            assert!(target_events[before..].iter().any(|event| matches!(event, Event::Channel { link, kind: MessageType(0x3344), bytes } if *link == app_link && bytes == b"owned-channel")));
            assert_eq!(triple.nodes[PRIMARY].events.snapshot().iter().filter(|event| matches!(event, Event::Settled { id, settlement: CommandOutcome::Channel(Ok(_)) } if *id == command)).count(), 1);
            assert!(!triple.nodes[HEALTHY]
                .events
                .snapshot()
                .iter()
                .any(|event| matches!(
                    event,
                    Event::Channel {
                        kind: MessageType(0x3344),
                        ..
                    }
                )));
        });
    }
}

#[test]
fn held_bulk_work_allows_an_independent_controller_to_finish_before_release() {
    use operation::{Operation, State};
    use personal_rns::routing::links::resources::ResourceStrategy;
    use personal_rns::runtime::SegmentCompression;
    use prns_runtime_tokio::runtime::{
        ControlledCryptoEvent, ControlledWorkKind, CryptoWorkBoundary,
    };
    use std::num::NonZeroUsize;
    for profile in profile::all() {
        if matches!(profile.execution, profile::Execution::Inline)
            || profile.runtimes
                != [
                    crate::remote_control::adapter::Runtime::Tokio,
                    crate::remote_control::adapter::Runtime::Tokio,
                ]
        {
            continue;
        }
        fixture::with_triple(profile.clone(), 1, |triple| {
            let control = triple
                .controls
                .iter()
                .find(|(index, _, _)| *index == PRIMARY)
                .expect("primary worker")
                .2
                .clone();
            let kind = ControlledWorkKind::BuildResource;
            control
                .hold(kind, CryptoWorkBoundary::Execution)
                .expect("hold build");
            let crate::remote_control::adapter::Handle::Tokio(target) =
                triple.nodes[fixture::TARGET].handle.clone()
            else {
                unreachable!()
            };
            let app_link = triple.app_links[0];
            triple.complete(async move {
                target
                    .set_link_resource_strategy(
                        app_link,
                        ResourceStrategy::Accept {
                            max_uncompressed_bytes: traffic::COMMON_RESPONSE_BYTES as u64,
                            accept_compressed: false,
                        },
                    )
                    .await
                    .expect("accept lab transfer");
            });
            let crate::remote_control::adapter::Handle::Tokio(primary) =
                triple.nodes[PRIMARY].handle.clone()
            else {
                unreachable!()
            };
            let resource = Operation::start(triple, async move {
                primary
                    .send_resource_with_compression(
                        app_link,
                        traffic::COMMON_RESPONSE_BYTES as u64,
                        &traffic::PAYLOAD[..traffic::COMMON_RESPONSE_BYTES],
                        SegmentCompression::Never,
                    )
                    .await
            });
            triple
                .tasks
                .poll_turns(NonZeroUsize::new(4096).expect("turn budget"));
            assert!(
                matches!(*resource.state(), State::Pending),
                "held resource {:?}",
                *resource.state()
            );
            assert!(
                control
                    .trace()
                    .expect("worker evidence")
                    .iter()
                    .any(|event| matches!(
                        event,
                        ControlledCryptoEvent::Queued {
                            kind: ControlledWorkKind::BuildResource,
                            ..
                        }
                    )),
                "actual resource job reached the held boundary"
            );
            assert!(
                matches!(
                    triple.exchange(HEALTHY, RemoteControlRequest::DescribeBuild),
                    Ok(RemoteControlResponse::DescribeBuild(_))
                ),
                "healthy controller remains live"
            );
            assert!(matches!(*resource.state(), State::Pending));
            control
                .release(kind, CryptoWorkBoundary::Execution)
                .expect("release build");
            resource.finish(triple).expect("resource completed");
            assert!(triple.nodes[fixture::TARGET].events.snapshot().iter().any(|event| matches!(event, crate::node_events::Event::Resource { link, bytes, .. } if *link == app_link && bytes == &traffic::PAYLOAD[..traffic::COMMON_RESPONSE_BYTES])));
        });
    }
}

#[test]
fn large_resource_open_is_held_at_real_worker_publication() {
    use operation::{Operation, State};
    use prns_runtime_tokio::runtime::{
        ControlledCryptoEvent, ControlledWorkKind, CryptoWorkBoundary,
    };
    for profile in profile::all().into_iter().filter(|profile| {
        profile.runtimes
            == [
                crate::remote_control::adapter::Runtime::Tokio,
                crate::remote_control::adapter::Runtime::Tokio,
            ]
            && profile.execution != profile::Execution::Inline
    }) {
        fixture::with_triple_layout(
            profile,
            42,
            fixture::ResourceLayout::WorkerSized,
            |triple| {
                let control = triple
                    .controls
                    .iter()
                    .find(|(node, _, _)| *node == fixture::PRIMARY)
                    .expect("primary worker")
                    .2
                    .clone();
                let before = control.trace().expect("worker trace").len();
                control
                    .hold(
                        ControlledWorkKind::OpenSpan,
                        CryptoWorkBoundary::Publication,
                    )
                    .expect("hold open result");
                let handle = triple.nodes[fixture::PRIMARY].handle.clone();
                let link = triple.app_links[0];
                let resource = Operation::start(triple, async move {
                    fixture::app_request(
                        handle,
                        link,
                        traffic::RESOURCE_PATH,
                        (traffic::LARGE_RESPONSE_BYTES as u16)
                            .to_be_bytes()
                            .to_vec(),
                    )
                    .await
                });
                let observed = control.clone();
                triple.drive_until(move || {
                    observed.trace().expect("actual worker trace")[before..]
                        .iter()
                        .any(|event| {
                            matches!(
                                event,
                                ControlledCryptoEvent::Queued {
                                    kind: ControlledWorkKind::OpenSpan,
                                    ..
                                }
                            )
                        })
                        && observed.snapshot().expect("actual occupancy").computed > 0
                });
                assert!(matches!(*resource.state(), State::Pending));
                let handle = triple.nodes[fixture::HEALTHY].handle.clone();
                let link = triple.links[1];
                let healthy = Operation::start(triple, async move {
                    fixture::exchange(handle, link, RemoteControlRequest::DescribeBuild).await
                });
                triple
                    .tasks
                    .poll_turns(std::num::NonZeroUsize::new(8192).expect("healthy turns"));
                assert!(matches!(
                    *healthy.state(),
                    State::Completed(Ok(RemoteControlResponse::DescribeBuild(_)))
                ));
                assert!(matches!(*resource.state(), State::Pending));
                control
                    .release(
                        ControlledWorkKind::OpenSpan,
                        CryptoWorkBoundary::Publication,
                    )
                    .expect("publish actual open result");
                assert_eq!(resource.finish(triple), Ok(traffic::PAYLOAD.to_vec()));
                healthy
                    .finish(triple)
                    .expect("independent controller delivered");
            },
        );
    }
}
