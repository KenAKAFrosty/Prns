//! Native USB CDC enumeration kept behind the reviewed FFI boundary.

use std::io;

/// Return only USB-backed serial port paths/names, sorted and deduplicated.
pub fn available_ports() -> io::Result<Vec<String>> {
    let mut ports = platform::available_ports()?;
    ports.sort();
    ports.dedup();
    Ok(ports)
}

#[cfg(target_os = "macos")]
mod platform {
    use std::io;

    use objc2_core_foundation::{CFDictionary, CFRetained, CFString};
    use objc2_io_kit::{
        io_iterator_t, io_object_t, kIOMainPortDefault, kIORegistryIterateParents,
        kIORegistryIterateRecursively, kIOReturnSuccess, kIOSerialBSDServiceValue, kIOServicePlane,
        IOIteratorNext, IOObjectRelease, IORegistryEntryCreateCFProperty,
        IORegistryEntrySearchCFProperty, IOServiceGetMatchingServices, IOServiceMatching,
        IO_OBJECT_NULL,
    };

    struct IoObject(io_object_t);

    impl Drop for IoObject {
        fn drop(&mut self) {
            if self.0 != IO_OBJECT_NULL {
                let _ = IOObjectRelease(self.0);
            }
        }
    }

    pub(super) fn available_ports() -> io::Result<Vec<String>> {
        // SAFETY: The generated binding accepts a valid, NUL-terminated static class name and
        // returns an owned CoreFoundation dictionary when successful.
        let matching = unsafe { IOServiceMatching(kIOSerialBSDServiceValue.as_ptr()) }
            .ok_or_else(|| io::Error::other("IOServiceMatching returned null"))?;
        // SAFETY: CFMutableDictionary is a proper subtype of CFDictionary and ownership remains +1.
        let matching: CFRetained<CFDictionary> = unsafe { CFRetained::cast_unchecked(matching) };
        let mut iterator: io_iterator_t = IO_OBJECT_NULL;
        // SAFETY: The matching dictionary is owned and intentionally consumed; `iterator` is a
        // valid out-pointer released by IoObject below.
        let status = unsafe {
            IOServiceGetMatchingServices(kIOMainPortDefault, Some(matching), &mut iterator)
        };
        if status != kIOReturnSuccess {
            return Err(io::Error::other(format!(
                "IOServiceGetMatchingServices failed: {status:#x}"
            )));
        }
        let _iterator_guard = IoObject(iterator);
        let vendor_key = CFString::from_static_str("idVendor");
        let callout_key = CFString::from_static_str("IOCalloutDevice");
        let plane = kIOServicePlane.as_ptr().cast_mut().cast();
        let options = kIORegistryIterateParents | kIORegistryIterateRecursively;
        let mut ports = Vec::new();
        loop {
            let service = IOIteratorNext(iterator);
            if service == IO_OBJECT_NULL {
                break;
            }
            let _service_guard = IoObject(service);
            // SAFETY: `service` is valid for the guard's lifetime; the plane/key pointers and
            // traversal flags are valid IOKit inputs. A found property is returned retained.
            let vendor = unsafe {
                IORegistryEntrySearchCFProperty(service, plane, Some(&vendor_key), None, options)
            };
            if vendor.is_none() {
                continue;
            }
            // SAFETY: `service` and the CFString key remain valid. IOKit returns a retained CFType.
            let Some(callout) =
                (unsafe { IORegistryEntryCreateCFProperty(service, Some(&callout_key), None, 0) })
            else {
                continue;
            };
            if let Some(path) = callout.downcast_ref::<CFString>() {
                ports.push(path.to_string());
            }
        }
        Ok(ports)
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use std::io;

    pub(super) fn available_ports() -> io::Result<Vec<String>> {
        super::windows_setupapi::available_ports()
    }
}

#[cfg(target_os = "windows")]
mod windows_setupapi {
    use std::io;
    use std::mem::size_of;

    use windows::core::{HRESULT, PCWSTR};
    use windows::Win32::Devices::DeviceAndDriverInstallation::{
        SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInfo, SetupDiGetClassDevsW,
        SetupDiGetDeviceInstanceIdW, SetupDiGetDeviceRegistryPropertyW, DIGCF_ALLCLASSES,
        DIGCF_PRESENT, HDEVINFO, SPDRP_FRIENDLYNAME, SP_DEVINFO_DATA,
    };
    use windows::Win32::Foundation::{ERROR_NO_MORE_ITEMS, HWND};

    struct DeviceInfoSet(HDEVINFO);

    impl Drop for DeviceInfoSet {
        fn drop(&mut self) {
            // SAFETY: The handle was returned by SetupDiGetClassDevsW and is released once here.
            let _ = unsafe { SetupDiDestroyDeviceInfoList(self.0) };
        }
    }

    pub(super) fn available_ports() -> io::Result<Vec<String>> {
        // SAFETY: A null class/enumerator with ALLCLASSES asks SetupAPI for all present local devices.
        let set = unsafe {
            SetupDiGetClassDevsW(
                None,
                PCWSTR::null(),
                HWND::default(),
                DIGCF_PRESENT | DIGCF_ALLCLASSES,
            )
        }
        .map_err(io::Error::other)?;
        let set = DeviceInfoSet(set);
        let no_more_items = HRESULT::from_win32(ERROR_NO_MORE_ITEMS.0);
        let mut ports = Vec::new();
        for index in 0.. {
            let mut info = SP_DEVINFO_DATA {
                cbSize: size_of::<SP_DEVINFO_DATA>() as u32,
                ..Default::default()
            };
            // SAFETY: `set` is live and `info` is a correctly sized writable output structure.
            if let Err(error) = unsafe { SetupDiEnumDeviceInfo(set.0, index, &mut info) } {
                if error.code() == no_more_items {
                    break;
                }
                return Err(io::Error::other(error));
            }
            let mut instance = [0u16; 512];
            // SAFETY: The device-info structure belongs to `set`; the UTF-16 output buffer is valid.
            if unsafe { SetupDiGetDeviceInstanceIdW(set.0, &info, Some(&mut instance), None) }
                .is_err()
            {
                continue;
            }
            let instance = wide_string(&instance);
            if !instance.to_ascii_uppercase().starts_with("USB\\") {
                continue;
            }
            let mut friendly = [0u8; 1024];
            // SAFETY: The device-info structure belongs to `set`; SetupAPI writes at most the
            // provided byte-buffer length for this REG_SZ property.
            if unsafe {
                SetupDiGetDeviceRegistryPropertyW(
                    set.0,
                    &info,
                    SPDRP_FRIENDLYNAME,
                    None,
                    Some(&mut friendly),
                    None,
                )
            }
            .is_err()
            {
                continue;
            }
            let wide: Vec<u16> = friendly
                .chunks_exact(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .collect();
            if let Some(port) = com_name(&wide_string(&wide)) {
                ports.push(port);
            }
        }
        Ok(ports)
    }

    fn wide_string(value: &[u16]) -> String {
        let end = value
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(value.len());
        String::from_utf16_lossy(&value[..end])
    }

    fn com_name(friendly_name: &str) -> Option<String> {
        let start = friendly_name.rfind("(COM")? + 1;
        let end = friendly_name[start..].find(')')? + start;
        let port = &friendly_name[start..end];
        (port.len() > 3
            && port[3..]
                .chars()
                .all(|character| character.is_ascii_digit()))
        .then(|| port.to_string())
    }
}
