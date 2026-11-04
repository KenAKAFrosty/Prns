use core::cell::RefCell;

use embassy_sync::blocking_mutex::{raw::CriticalSectionRawMutex, Mutex};
use embassy_sync::signal::Signal;
use personal_rns::runtime::RemoteControlControllerGrantControl;
use personal_rns::usb_auto::UsbAutoControllerEnrollment;

use super::{
    Admission, EnrollmentState, UsbControllerEnrollment, UsbControllerEnrollmentBusy,
    UsbControllerEnrollmentStatus,
};

static REQUEST: Signal<CriticalSectionRawMutex, UsbControllerEnrollment> = Signal::new();
static STATE: Mutex<CriticalSectionRawMutex, RefCell<EnrollmentState>> =
    Mutex::new(RefCell::new(EnrollmentState::new()));

pub(crate) fn serial_enrollment(
    identity: &personal_rns::remote_control::RemoteControlNodeIdentitySecrets,
) -> UsbAutoControllerEnrollment {
    // The serial protocol owns the fixed Administrator/all grant policy and verifies
    // the selected target's full key before it admits a local USB request.
    UsbAutoControllerEnrollment::Supported {
        request,
        status,
        target_public_key: identity
            .identities()
            .target()
            .public_keys()
            .public_key_bytes(),
    }
}

fn status() -> UsbControllerEnrollmentStatus {
    STATE.lock(|state| state.borrow().status())
}

fn request(enrollment: UsbControllerEnrollment) -> Result<(), UsbControllerEnrollmentBusy> {
    STATE.lock(|state| {
        if state.borrow_mut().admit(&enrollment)? == Admission::New {
            REQUEST.signal(enrollment);
        }
        Ok(())
    })
}

pub(crate) async fn run(handle: impl RemoteControlControllerGrantControl) -> ! {
    loop {
        let UsbControllerEnrollment { transaction, grant } = REQUEST.wait().await;
        // The manifold restores retained authorization before servicing the command.
        // This API settles only after its journal commit; Pending remains visible
        // while storage is in progress, including across a USB disconnect.
        let saved = handle
            .set_remote_control_controller_grant(grant)
            .await
            .is_ok();
        STATE.lock(|state| state.borrow_mut().settle(transaction, saved));
    }
}
