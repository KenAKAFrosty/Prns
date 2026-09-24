use super::super::tests::{with_pair, Pair};
use super::*;
use personal_rns::engine::SendRequestFailure;
use personal_rns::runtime::SendError;
use prns_simulation::{ManualTaskScheduling, SimulationSeed};
use std::cell::Cell;
use std::num::NonZeroUsize;
use std::panic::{catch_unwind, AssertUnwindSafe};

fn permissions(profile: &Permission) -> RemoteControlRequestSet {
    match profile {
        Permission::Full => requests(),
        Permission::DescribeOnly => {
            RemoteControlRequestSet::only(RemoteControlRequestKind::Describe)
        }
        Permission::Absent => RemoteControlRequestSet::empty(),
    }
}
struct Ledger {
    permission: Permission,
    generations: [u64; 2],
    calls: usize,
}
fn check(condition: bool, stage: usize, action: &Action, detail: &str) -> Result<(), Failure> {
    if condition {
        Ok(())
    } else {
        Err(Failure {
            class: FailureClass::Expectation,
            stage,
            operation: Some(action.clone()),
            detail: detail.to_owned(),
        })
    }
}
fn timeout(reply: &Result<RemoteControlResponse, SendError<SendRequestFailure>>) -> bool {
    *reply == Err(SendError::Failed(SendRequestFailure::Timeout))
}
fn response_header(
    link: personal_rns::routing::links::LinkId,
) -> personal_rns::wire::WirePacketHeader {
    use personal_rns::wire::*;
    WirePacketHeader {
        ifac_flag: IfacFlag::Open,
        context_flag: ContextFlag::Unset,
        propagation: PropagationType::Broadcast,
        destination_type: DestinationType::Link,
        packet_type: PacketType::Data,
        hops: 0,
        transport_id: None,
        address: link.to_address(),
        context: WireContext::Response,
    }
}
fn apply(
    pair: &mut Pair<'_, '_>,
    ledger: &mut Ledger,
    case: &Case,
    stage: usize,
    action: &Action,
) -> Result<String, Failure> {
    let allowed = matches!(ledger.permission, Permission::Full);
    let reply = match action {
        Action::Permissions(profile) => {
            match profile {
                Permission::Absent => {
                    let handle = pair.nodes[TARGET].handle.clone();
                    let identity = *secrets(CONTROLLER).identities().controller();
                    pair.complete(async move {
                        handle.revoke(identity).await.expect("local revoke");
                    });
                }
                Permission::Full | Permission::DescribeOnly => pair.grant(permissions(profile)),
            }
            ledger.permission = profile.clone();
            return Ok(format!("permissions={profile:?}"));
        }
        Action::RestartController | Action::RestartTarget => {
            let index = if matches!(action, Action::RestartController) {
                CONTROLLER
            } else {
                TARGET
            };
            pair.restart(index);
            ledger.generations[index] += 1;
            check(
                pair.generations == ledger.generations,
                stage,
                action,
                "fresh boot generation",
            )?;
            return Ok(format!("restored={index}"));
        }
        Action::Reconnect => {
            pair.reconnect();
            return Ok("reconnected".to_owned());
        }
        Action::Pair(approval) => {
            pairing_action::run(pair, approval);
            if matches!(approval, Approval::Accept) {
                ledger.permission = Permission::Full;
            }
            return Ok(format!("pairing={approval:?}"));
        }
        Action::Describe => {
            let reply = pair.exchange(RemoteControlRequest::Describe);
            if !matches!(ledger.permission, Permission::Absent) {
                let Ok(RemoteControlResponse::Describe(description)) = &reply else {
                    return Err(Failure {
                        class: FailureClass::Expectation,
                        stage,
                        operation: Some(action.clone()),
                        detail: "missing admitted description".to_owned(),
                    });
                };
                let mut expected = permissions(&ledger.permission);
                for request in [
                    RemoteControlRequestKind::InventoryControllers,
                    RemoteControlRequestKind::AuthorizeController,
                    RemoteControlRequestKind::RevokeController,
                ] {
                    expected.insert(request);
                }
                if matches!(case.runtimes[TARGET], adapter::Runtime::Embassy) {
                    expected = expected
                        .iter()
                        .filter(|request| *request != RemoteControlRequestKind::WatchInterfaces)
                        .fold(RemoteControlRequestSet::empty(), |mut set, request| {
                            set.insert(request);
                            set
                        });
                }
                check(
                    *description.available_requests() == expected,
                    stage,
                    action,
                    &format!(
                        "advertised {:?}, expected {:?}",
                        description.available_requests(),
                        expected
                    ),
                )?;
            } else {
                check(
                    timeout(&reply),
                    stage,
                    action,
                    "unlisted description remains silent",
                )?;
            }
            return Ok(format!("{reply:?}"));
        }
        Action::Build => {
            let reply = pair.exchange(RemoteControlRequest::DescribeBuild);
            if allowed {
                check(
                    reply
                        == Ok(RemoteControlResponse::DescribeBuild(
                            RemoteControlBuildVersion::from_text("simulation-v1")
                                .expect("build label"),
                        )),
                    stage,
                    action,
                    "build reply",
                )?;
            } else {
                check(
                    timeout(&reply),
                    stage,
                    action,
                    "build admission remains silent",
                )?;
            }
            reply
        }
        Action::Echo { marker, len } => {
            let payload = RemoteControlAppMessage::from_slice(&vec![*marker; *len])
                .expect("validated payload bound");
            let reply = pair.exchange(RemoteControlRequest::AppMessage(payload.clone()));
            if allowed {
                ledger.calls += 1;
                check(
                    reply == Ok(RemoteControlResponse::AppMessage(payload)),
                    stage,
                    action,
                    "echo bytes and response ownership",
                )?;
            } else {
                check(
                    timeout(&reply),
                    stage,
                    action,
                    "app request before admission remains silent",
                )?;
            }
            reply
        }
        Action::AppReject => {
            let reply = pair.exchange(RemoteControlRequest::AppMessage(
                RemoteControlAppMessage::from_slice(&[0xff]).expect("bounded app rejection"),
            ));
            if allowed {
                ledger.calls += 1;
                check(
                    reply
                        == Ok(RemoteControlResponse::ProtocolError(
                            RemoteControlProtocolError::ApplyFailed {
                                request: RemoteControlRequestKind::AppMessage,
                            },
                        )),
                    stage,
                    action,
                    "authorized app rejection",
                )?;
            } else {
                check(
                    timeout(&reply),
                    stage,
                    action,
                    "app rejection is never exposed before admission",
                )?;
            }
            reply
        }
        Action::Malformed | Action::Oversized => {
            let mut bytes = vec![1, RemoteControlRequestKind::AppMessage as u8, 0xc4, 97];
            if matches!(action, Action::Oversized) {
                bytes.extend([0; 97]);
            } else {
                bytes.clear();
            }
            let handle = pair.nodes[CONTROLLER].handle.clone();
            let link = pair.link;
            let reply = pair.complete(async move {
                handle.raw(link, bytes).await.map(|bytes| {
                    RemoteControlResponse::parse(&bytes).expect("admitted parser response")
                })
            });
            if allowed {
                check(
                    reply
                        == Ok(RemoteControlResponse::ProtocolError(
                            RemoteControlProtocolError::MalformedRequest,
                        )),
                    stage,
                    action,
                    "payload parser bound",
                )?;
            } else {
                check(
                    timeout(&reply),
                    stage,
                    action,
                    "parser errors remain silent before admission",
                )?;
            }
            reply
        }
        Action::LoseResponse | Action::CancelCaller => {
            let wire = pair.nodes[TARGET].wire.clone();
            wire.lose_after(response_header(pair.link), 0, NonZeroUsize::MIN);
            let handle = pair.nodes[CONTROLLER].handle.clone();
            let link = pair.link;
            let mut bytes = [0; RemoteControlRequest::MAX_ENCODED_LEN];
            let len = RemoteControlRequest::DescribeBuild
                .write_into(&mut bytes)
                .expect("build request");
            let bytes = bytes[..len].to_vec();
            let reply = if matches!(action, Action::CancelCaller) && allowed {
                let gate = wire.clone();
                pair.complete(async move { tokio::select! { biased; result = handle.raw(link, bytes) => panic!("canceled response unexpectedly completed: {result:?}"), _ = gate.first_loss() => {} } });
                check(
                    wire.stop_loss() == 1,
                    stage,
                    action,
                    "canceled admitted reply was emitted",
                )?;
                return Ok("caller canceled after response emission".to_owned());
            } else {
                pair.complete(async move {
                    handle
                        .raw(link, bytes)
                        .await
                        .map(|bytes| RemoteControlResponse::parse(&bytes).expect("response"))
                })
            };
            check(
                wire.stop_loss() == usize::from(allowed),
                stage,
                action,
                "only admitted work emitted a response",
            )?;
            check(
                timeout(&reply),
                stage,
                action,
                "lost response expired locally",
            )?;
            reply
        }
        Action::Inventory => {
            let reply = inventory_action::fetch(pair);
            if allowed {
                check(
                    matches!(reply, Ok(RemoteControlResponse::InventoryInterfaces(_))),
                    stage,
                    action,
                    "inventory converged",
                )?;
            } else {
                check(timeout(&reply), stage, action, "inventory unauthorized")?;
            }
            reply
        }
        Action::WatchInventory => {
            let target_support = matches!(case.runtimes[TARGET], adapter::Runtime::Tokio);
            let id = personal_rns::runtime::StreamId::new(3).expect("watch ID");
            let reply = if let adapter::Handle::Tokio(handle) = &pair.nodes[CONTROLLER].handle {
                if target_support && allowed {
                    let handle = handle.clone();
                    let link = pair.link;
                    pair.complete(async move {
                        let (mut watch, _) = handle
                            .remote_control(link)
                            .watch_interfaces(id)
                            .await
                            .expect("typed controller watch");
                        assert_eq!(
                            watch.next_event().await.expect("initial resync"),
                            RemoteControlStreamEvent::ResyncRequired { sequence: 1 }
                        );
                    });
                    Ok(RemoteControlResponse::WatchInterfaces { stream_id: id })
                } else {
                    pair.exchange(RemoteControlRequest::WatchInterfaces { stream_id: id })
                }
            } else {
                pair.exchange(RemoteControlRequest::WatchInterfaces { stream_id: id })
            };
            if target_support && allowed {
                check(
                    reply == Ok(RemoteControlResponse::WatchInterfaces { stream_id: id }),
                    stage,
                    action,
                    "Tokio target watch admission",
                )?;
            } else {
                check(
                    timeout(&reply),
                    stage,
                    action,
                    "absent watch capability/admission is silent",
                )?;
            }
            pair.reconnect();
            if allowed {
                inventory_action::fetch(pair).expect("refetch after watch/reconnect");
            }
            reply
        }
    };
    check(
        pair.messages.0.borrow().len() == ledger.calls,
        stage,
        action,
        "independent app admission count",
    )?;
    check(
        pair.messages.0.borrow().iter().all(|call| {
            call.identity
                == secrets(CONTROLLER)
                    .identities()
                    .controller()
                    .identity_hash()
        }),
        stage,
        action,
        "verified controller identity isolation",
    )?;
    Ok(format!("{reply:?}"))
}

pub fn run(case: &Case) -> Run {
    if let Err(error) = case.validate() {
        return Run::Failed {
            failure: Failure {
                class: FailureClass::InvalidInput,
                stage: 0,
                operation: None,
                detail: format!("{error:?}"),
            },
            report: None,
        };
    }
    let stage = Cell::new(0);
    let observations = Rc::new(RefCell::new(Vec::new()));
    let result = catch_unwind(AssertUnwindSafe(|| {
        with_pair(
            case.runtimes.clone(),
            ManualTaskScheduling::Seeded {
                seed: SimulationSeed::new(case.seed),
            },
            |pair| {
                catch_unwind(AssertUnwindSafe(|| {
                    let mut ledger = Ledger {
                        permission: Permission::Full,
                        generations: [0; 2],
                        calls: 0,
                    };
                    for (index, action) in case.actions.iter().enumerate() {
                        stage.set(index);
                        let outcome = apply(pair, &mut ledger, case, index, action)?;
                        observations.borrow_mut().push(Observation {
                            stage: index,
                            action: action.clone(),
                            outcome,
                            permission: ledger.permission.clone(),
                            generations: ledger.generations,
                            app_calls: ledger.calls,
                        });
                    }
                    Ok(())
                }))
                .unwrap_or_else(|panic| Err(runtime_failure(case, stage.get(), panic)))
            },
        )
    }));
    match result {
        Ok((outcome, wire, persistence)) => {
            let report = Report {
                observations: std::mem::take(&mut *observations.borrow_mut()),
                wire,
                persistence,
            };
            match outcome {
                Ok(()) => Run::Passed(report),
                Err(failure) => Run::Failed {
                    failure,
                    report: Some(report),
                },
            }
        }
        Err(panic) => Run::Failed {
            failure: runtime_failure(case, stage.get(), panic),
            report: None,
        },
    }
}
fn runtime_failure(case: &Case, stage: usize, panic: Box<dyn std::any::Any + Send>) -> Failure {
    let detail = panic
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| {
            panic
                .downcast_ref::<&str>()
                .map(|message| (*message).to_owned())
        })
        .unwrap_or_else(|| "non-text runtime panic".to_owned());
    Failure {
        class: FailureClass::Runtime,
        stage,
        operation: case.actions.get(stage).cloned(),
        detail,
    }
}
