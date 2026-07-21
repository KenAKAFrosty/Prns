use core::ffi::c_void;

use windows::core::GUID;
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    CM_Register_Notification, CM_NOTIFY_ACTION, CM_NOTIFY_ACTION_DEVICEINTERFACEARRIVAL,
    CM_NOTIFY_ACTION_DEVICEINTERFACEREMOVAL, CM_NOTIFY_EVENT_DATA, CM_NOTIFY_FILTER,
    CM_NOTIFY_FILTER_TYPE_DEVICEINTERFACE, HCMNOTIFICATION,
};

const GUID_DEVINTERFACE_COMPORT: GUID = GUID::from_u128(0x86e0d1e0_8089_11d0_9ce4_08003e301f73);

type Sink = Box<dyn Fn() + Send + Sync + 'static>;

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
        crate::diagnostic_log::debug!("serial-port interface change, poking rescan");
        // SAFETY: context points to the process-lifetime Box<Sink> leaked by watch_serial_hotplug.
        let sink = unsafe { &*(context as *const Sink) };
        sink();
    }
    0
}

/// Register a process-lifetime watch that calls `sink` on every serial-port arrival or removal.
/// `sink` is poked from a PnP service thread, so it must be `Send + Sync`; pass a closure that pokes
/// the host's `rescan` signal.
pub fn watch_serial_hotplug<F: Fn() + Send + Sync + 'static>(sink: F) {
    let sink: Sink = Box::new(sink);
    let context = Box::into_raw(Box::new(sink));

    let mut filter = CM_NOTIFY_FILTER {
        cbSize: core::mem::size_of::<CM_NOTIFY_FILTER>() as u32,
        FilterType: CM_NOTIFY_FILTER_TYPE_DEVICEINTERFACE,
        ..Default::default()
    };
    filter.u.DeviceInterface.ClassGuid = GUID_DEVINTERFACE_COMPORT;

    let mut handle = HCMNOTIFICATION(core::ptr::null_mut());
    // SAFETY: the filter, context, callback, and output handle are valid for this call; the context is intentionally leaked for the process-lifetime registration.
    unsafe {
        let _ = CM_Register_Notification(
            &filter,
            Some(context as *const c_void),
            Some(on_interface_change),
            &mut handle,
        );
    }
}
