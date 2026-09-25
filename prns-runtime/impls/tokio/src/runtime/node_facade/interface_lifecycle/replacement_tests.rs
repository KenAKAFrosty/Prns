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
