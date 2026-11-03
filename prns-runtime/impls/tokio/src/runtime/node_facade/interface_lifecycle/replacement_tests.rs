use std::collections::HashMap;
use std::future::{poll_fn, Future};
use std::pin::pin;
use std::sync::{Arc, Mutex};
use std::task::Poll;

use crate::interfaces::{
    FrameAccounting, InterfaceId, InterfaceKind, InterfaceVitals, Membership, ReportsStatus,
    StatusView,
};
use crate::manifold::driver::TokioInterfaceStatus;
use crate::manifold::interface_seam::Interface;

use super::tests::{registered_status, StatusInterface};
use super::{drive_interfaces, Fleet, RetiredMemberFrameAccounting, StatusRegistration};

struct PendingInterface(StatusInterface);

impl Interface for PendingInterface {
    const HW_MTU: usize = StatusInterface::HW_MTU;
    const KIND: InterfaceKind = StatusInterface::KIND;

    fn channel_tag(&self) -> &[u8] {
        self.0.channel_tag()
    }

    fn descriptor(&self) -> crate::interfaces::InterfaceDescriptor {
        self.0.descriptor()
    }

    async fn run<S: crate::manifold::interface_seam::InterfaceSeam>(self, _seam: S) {
        let _live = self;
        std::future::pending::<()>().await;
    }
}

impl ReportsStatus for PendingInterface {
    fn status_view(&self) -> Option<StatusView> {
        self.0.status_view()
    }
}

#[tokio::test]
async fn queued_teardown_preserves_replacement_status_and_retires_only_departed_traffic() {
    let supervisor = InterfaceId::new([0x76; 8]);
    let (fleet, tail) = Fleet::detached(supervisor);
    let supervisor_status = TokioInterfaceStatus::new_unaccounted(
        supervisor,
        crate::interfaces::ConnectionState::Connected,
    );
    fleet.interfaces.lock().unwrap().insert(
        supervisor,
        registered_status(
            Arc::new(move || std::vec![InterfaceVitals::of(&supervisor_status)]),
            Membership::Independent,
        ),
    );
    let original = PendingInterface(StatusInterface::new(b"replacement-member"));
    let member_id = original.0.id();
    let original_status = original.0.status.clone();
    original_status.add_rx(100);
    original_status.add_tx(20);
    original_status.count_frame_in();
    original_status.count_frame_delivered();
    let attached = fleet.add(original);
    let mut driver = pin!(drive_interfaces(
        std::vec![],
        tail._iface_build,
        fleet.commands.clone(),
        fleet.interfaces.clone(),
    ));
    assert_eq!(
        poll_fn(|context| Poll::Ready(driver.as_mut().poll(context))).await,
        Poll::Pending
    );

    attached.teardown();
    let replacement = PendingInterface(StatusInterface::new(b"replacement-member"));
    let replacement_status = replacement.0.status.clone();
    replacement_status.add_rx(700);
    replacement_status.add_tx(80);
    let replacement = fleet.add(replacement);
    let replacement_epoch = fleet.interfaces.lock().unwrap()[&member_id].attachment_epoch;
    original_status.add_rx(7);
    original_status.add_tx(3);
    assert_eq!(
        poll_fn(|context| Poll::Ready(driver.as_mut().poll(context))).await,
        Poll::Pending
    );
    {
        let map = fleet.interfaces.lock().unwrap();
        assert_eq!(
            map.get(&member_id)
                .map(|entry| (entry.attachment_epoch, (entry.view)())),
            Some((
                replacement_epoch,
                std::vec![InterfaceVitals::of(&replacement_status)]
            ))
        );
        let supervisor = &map[&supervisor];
        assert_eq!(
            (
                supervisor.retired_member_bytes.rx,
                supervisor.retired_member_bytes.tx
            ),
            (107, 23)
        );
        assert!(matches!(
            supervisor.retired_member_frame_accounting,
            RetiredMemberFrameAccounting::Complete(FrameAccounting {
                frames_in: 1,
                delivered: 1,
                malformed: 0,
                protocol_violations: 0,
                undecodable: 0
            })
        ));
    }
    replacement.teardown();
    assert_eq!(
        poll_fn(|context| Poll::Ready(driver.as_mut().poll(context))).await,
        Poll::Pending
    );
    let map = fleet.interfaces.lock().unwrap();
    assert!(!map.contains_key(&member_id));
    let supervisor = &map[&supervisor];
    assert_eq!(
        (
            supervisor.retired_member_bytes.rx,
            supervisor.retired_member_bytes.tx
        ),
        (807, 103)
    );
}

#[test]
fn retirement_only_removes_its_own_reported_registration() {
    let interface = StatusInterface::new(b"independent-replacement");
    let id = interface.id();
    let view = interface.status_view().unwrap();
    let interfaces = Arc::new(Mutex::new(HashMap::new()));
    let mut registered = registered_status(view.clone(), Membership::Independent);
    registered.attachment_epoch = 1;
    interfaces.lock().unwrap().insert(id, registered);
    for registration in [
        StatusRegistration::Unreported,
        StatusRegistration::new(0, Some(&view)),
    ] {
        registration.retire(&interfaces, id, None);
        let map = interfaces.lock().unwrap();
        assert_eq!(
            map.get(&id)
                .map(|entry| (entry.attachment_epoch, (entry.view)())),
            Some((1, std::vec![InterfaceVitals::of(&interface.status)]))
        );
    }
    StatusRegistration::new(1, Some(&view)).retire(&interfaces, id, None);
    assert!(interfaces.lock().unwrap().is_empty());
}

struct Dropped(std::sync::Arc<std::sync::atomic::AtomicBool>);
impl Drop for Dropped {
    fn drop(&mut self) {
        self.0.store(true, std::sync::atomic::Ordering::Release);
    }
}

#[tokio::test]
async fn acknowledged_supervisor_retirement_drops_queued_members_before_the_manifold_barrier() {
    use super::{DriverMsg, InterfaceDetachError};
    use crate::manifold::driver::HostCommand;
    use std::sync::atomic::{AtomicBool, Ordering};
    let supervisor = InterfaceId::new([0x76; 8]);
    let child = InterfaceId::new([0x77; 8]);
    let (messages, receiver) = tokio::sync::mpsc::unbounded_channel();
    let (commands, mut command_receiver) = tokio::sync::mpsc::unbounded_channel();
    let interfaces = Arc::new(Mutex::new(HashMap::new()));
    let parent_dropped = Arc::new(AtomicBool::new(false));
    let child_dropped = Arc::new(AtomicBool::new(false));
    let parent_guard = Dropped(parent_dropped.clone());
    messages
        .send(DriverMsg::Add {
            id: supervisor,
            supervisor: None,
            registration: StatusRegistration::Unreported,
            build: Box::new(move || {
                Box::pin(async move {
                    let _guard = parent_guard;
                    std::future::pending::<()>().await
                })
            }),
        })
        .unwrap_or_else(|_| panic!("parent admitted"));
    let (reply, retired) = tokio::sync::oneshot::channel::<Result<(), InterfaceDetachError>>();
    messages
        .send(DriverMsg::StopAndWait {
            id: supervisor,
            reply,
        })
        .unwrap_or_else(|_| panic!("stop admitted"));
    // A final member queued after the stop must never begin run(), and its
    // resources still have to be released before retirement is confirmed.
    let child_guard = Dropped(child_dropped.clone());
    messages
        .send(DriverMsg::Add {
            id: child,
            supervisor: Some(supervisor),
            registration: StatusRegistration::Unreported,
            build: Box::new(move || {
                Box::pin(async move {
                    let _guard = child_guard;
                    std::future::pending::<()>().await
                })
            }),
        })
        .unwrap_or_else(|_| panic!("member admitted"));
    tokio::join!(
        drive_interfaces(std::vec![], receiver, commands, interfaces.clone()),
        async {
            let mut removed = std::collections::HashSet::new();
            while let Some(command) = command_receiver.recv().await {
                match command {
                    HostCommand::RemoveInterface { id, .. } => {
                        removed.insert(id);
                    }
                    HostCommand::InterfaceLifecycleBarrier { reply } => {
                        assert!(parent_dropped.load(Ordering::Acquire));
                        assert!(child_dropped.load(Ordering::Acquire));
                        assert!(removed.contains(&child));
                        reply.send(()).expect("waiter remains");
                        break;
                    }
                    _ => panic!("unexpected command"),
                }
            }
            assert_eq!(retired.await, Ok(Ok(())));
            drop(messages);
        }
    );
}

#[tokio::test]
async fn acknowledged_retirement_drains_members_stopped_by_natural_parent_completion() {
    use super::{complete_interface, retire_driver_interface, DriverInbox, RunningInterface};
    use futures_util::stream::FuturesUnordered;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicBool, Ordering};

    let parent = InterfaceId::new([0x78; 8]);
    let child = InterfaceId::new([0x79; 8]);
    let dropped = Arc::new(AtomicBool::new(false));
    let owned = Dropped(dropped.clone());
    let (parent_stop, _parent_stopped) = tokio::sync::oneshot::channel();
    let (child_stop, child_stopped) = tokio::sync::oneshot::channel();
    let mut futures: FuturesUnordered<Pin<Box<dyn Future<Output = Option<InterfaceId>>>>> =
        FuturesUnordered::new();
    futures.push(Box::pin(async move {
        let _owned = owned;
        let _ = child_stopped.await;
        Some(child)
    }));
    let mut stops = HashMap::from([
        (
            parent,
            RunningInterface {
                stop: Some(parent_stop),
                registration: StatusRegistration::Unreported,
            },
        ),
        (
            child,
            RunningInterface {
                stop: Some(child_stop),
                registration: StatusRegistration::Unreported,
            },
        ),
    ]);
    let mut parents = HashMap::from([(child, parent)]);
    let interfaces = Arc::new(Mutex::new(HashMap::new()));
    let (commands, _command_receiver) = tokio::sync::mpsc::unbounded_channel();
    let (_messages, receiver) = tokio::sync::mpsc::unbounded_channel();
    let mut inbox = DriverInbox::new(receiver);

    // The parent's run future has naturally ended. Its children have received
    // stop, but their futures have not been polled again to release resources.
    complete_interface(&mut stops, &mut parents, &interfaces, &commands, parent);
    assert!(!dropped.load(Ordering::Acquire));

    // An explicit detach can arrive before the driver's next completion poll.
    // It must still find and drain those stopped children before its barrier.
    retire_driver_interface(
        &mut futures,
        &mut stops,
        &mut parents,
        &interfaces,
        &commands,
        &mut inbox,
        parent,
    )
    .await;
    assert!(dropped.load(Ordering::Acquire));
    assert!(futures.is_empty());
    assert!(stops.is_empty());
    assert!(parents.is_empty());
}

async fn assert_queued_retirement_keeps_replacement_lane(acknowledged: bool) {
    use super::super::PrnsNodeHandle;
    use crate::manifold::driver::HostCommand;

    let supervisor = InterfaceId::new([0x7a; 8]);
    let (fleet, mut tail) = Fleet::detached(supervisor);
    let original = PendingInterface(StatusInterface::new(b"queued-manifold-replacement"));
    let member_id = original.0.id();
    let original = fleet.add(original);
    let mut driver = pin!(drive_interfaces(
        std::vec![],
        tail._iface_build,
        fleet.commands.clone(),
        fleet.interfaces.clone(),
    ));
    assert_eq!(
        poll_fn(|context| Poll::Ready(driver.as_mut().poll(context))).await,
        Poll::Pending
    );

    let mut handle = PrnsNodeHandle::over(fleet.commands.clone());
    handle.iface_build = fleet.iface_build.clone();
    let mut retirement = pin!(handle.remove_interface_and_wait(member_id));
    if acknowledged {
        assert_eq!(
            poll_fn(|context| Poll::Ready(retirement.as_mut().poll(context))).await,
            Poll::Pending
        );
    } else {
        original.teardown();
    }
    // Like a reconnecting supervisor, publish the same wire identity's new
    // manifold lane before the driver has consumed its old run's stop message.
    let _replacement = fleet.add(PendingInterface(StatusInterface::new(
        b"queued-manifold-replacement",
    )));
    assert_eq!(
        poll_fn(|context| Poll::Ready(driver.as_mut().poll(context))).await,
        Poll::Pending
    );
    let mut mutations = std::vec::Vec::new();
    while let Ok(command) = tail._commands.try_recv() {
        match command {
            HostCommand::AddInterface(add) if add.descriptor.id == member_id => {
                mutations.push("attach");
            }
            HostCommand::RemoveInterface { id, .. } if id == member_id => {
                mutations.push("remove");
            }
            HostCommand::InterfaceLifecycleBarrier { reply } => {
                reply.send(()).expect("driver waits for the manifold");
            }
            _ => panic!("unexpected manifold command"),
        }
    }
    assert_eq!(mutations, ["attach", "remove", "attach"]);
    assert_eq!(
        poll_fn(|context| Poll::Ready(driver.as_mut().poll(context))).await,
        Poll::Pending
    );
    assert!(tail._commands.try_recv().is_err());
    if acknowledged {
        assert_eq!(
            poll_fn(|context| Poll::Ready(retirement.as_mut().poll(context))).await,
            Poll::Ready(Ok(()))
        );
    }
}

#[tokio::test]
async fn queued_normal_retirement_keeps_the_replacement_manifold_lane() {
    assert_queued_retirement_keeps_replacement_lane(false).await;
}

#[tokio::test]
async fn queued_acknowledged_retirement_keeps_the_replacement_manifold_lane() {
    assert_queued_retirement_keeps_replacement_lane(true).await;
}
