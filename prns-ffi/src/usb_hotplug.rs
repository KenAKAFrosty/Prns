//! Event-driven serial-device hot-plug for the tokio USB-auto host on Windows.
//!
//! The host already re-enumerates the instant its `rescan` [`Notify`](tokio::sync::Notify) is
//! poked; on Linux the desktop pokes it from a udev monitor, but Windows had no event source and
//! fell back to a once-a-second poll. This registers a Windows PnP notification
//! ([`CM_Register_Notification`]) that pokes the host the moment a COM port arrives or leaves, so a
//! board shows up when it is plugged rather than on the next fallback scan.
//!
//! Like the Linux watcher (which matches the `tty` subsystem), this filters to the serial-port
//! device-interface class only — a non-serial USB device (a storage stick, an RTL-SDR bulk
//! interface) never triggers a spurious rescan. It lives in this crate because
//! [`CM_Register_Notification`] is raw `cfgmgr32` FFI, quarantined off the engine's
//! forbid-unsafe surface.

use core::ffi::c_void;

use windows::core::GUID;
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    CM_Register_Notification, CM_NOTIFY_ACTION, CM_NOTIFY_ACTION_DEVICEINTERFACEARRIVAL,
    CM_NOTIFY_ACTION_DEVICEINTERFACEREMOVAL, CM_NOTIFY_EVENT_DATA, CM_NOTIFY_FILTER,
    CM_NOTIFY_FILTER_TYPE_DEVICEINTERFACE, HCMNOTIFICATION,
};

/// `GUID_DEVINTERFACE_COMPORT` — the device-interface class for serial (COM) ports. The `windows`
/// crate does not project it as a constant, so it is spelled out here: it is the Win32 analogue of
/// udev's `tty` subsystem, scoping the watch to exactly the devices the USB-auto host opens.
const GUID_DEVINTERFACE_COMPORT: GUID = GUID::from_u128(0x86e0d1e0_8089_11d0_9ce4_08003e301f73);

/// The caller's "poke the host" closure, type-erased so the C callback can recover it through the
/// single `*const c_void` context pointer the registration round-trips.
type Sink = Box<dyn Fn() + Send + Sync + 'static>;

/// The `cfgmgr32` callback, invoked on a PnP service thread for every event on the filtered
/// interface class. `context` is the leaked `Box<Sink>` from [`watch_serial_hotplug`], which lives
/// for the whole process, so dereferencing it is sound on every call. Only interface arrival and
/// removal mean a port set may have changed; other actions (query-remove, custom events) are
/// ignored. The return value is informational here (it only vetoes a query-remove), so it is always
/// success.
unsafe extern "system" fn on_interface_change(
    _notify: HCMNOTIFICATION,
    context: *const c_void,
    action: CM_NOTIFY_ACTION,
    _event_data: *const CM_NOTIFY_EVENT_DATA,
    _event_data_size: u32,
) -> u32 {
    if action == CM_NOTIFY_ACTION_DEVICEINTERFACEARRIVAL
        || action == CM_NOTIFY_ACTION_DEVICEINTERFACEREMOVAL
    {
        log::debug!("serial-port interface change, poking rescan");
        let sink = &*(context as *const Sink);
        sink();
    }
    0
}

/// Register a process-lifetime watch that calls `sink` on every serial-port arrival or removal.
/// `sink` is poked from a PnP service thread, so it must be `Send + Sync`; pass a closure that pokes
/// the host's `rescan` signal.
pub fn watch_serial_hotplug<F: Fn() + Send + Sync + 'static>(sink: F) {
    // The registration is never torn down (it runs until the process exits), so the boxed closure
    // the callback dereferences must outlive every call: hand its ownership to the OS by leaking the
    // box and passing the raw pointer as the callback context.
    let sink: Sink = Box::new(sink);
    let context = Box::into_raw(Box::new(sink));

    let mut filter = CM_NOTIFY_FILTER::default();
    filter.cbSize = core::mem::size_of::<CM_NOTIFY_FILTER>() as u32;
    filter.FilterType = CM_NOTIFY_FILTER_TYPE_DEVICEINTERFACE;
    filter.u.DeviceInterface.ClassGuid = GUID_DEVINTERFACE_COMPORT;

    // The returned handle is a Copy pointer wrapper we do not keep — without a matching
    // CM_Unregister_Notification the watch simply persists for the process lifetime, which is what
    // we want.
    let mut handle = HCMNOTIFICATION(core::ptr::null_mut());
    unsafe {
        let _ = CM_Register_Notification(
            &filter,
            Some(context as *const c_void),
            Some(on_interface_change),
            &mut handle,
        );
    }
}
