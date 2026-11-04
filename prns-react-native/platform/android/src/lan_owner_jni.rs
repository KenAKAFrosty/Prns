//! Generation-fenced OS inventory and provider retirement for automatic LAN.
use jni::objects::{JByteArray, JClass, JIntArray, JObjectArray, JString};
use jni::sys::{jboolean, jint, jlong};
use jni::JNIEnv;
use prns_host_runtime::platform::android::lan;
use prns_interfaces_tokio::wifi_auto::{HostLanAddress, HostLanInterface, HostLanReplaceOutcome};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::Duration;

#[no_mangle]
pub extern "system" fn Java_rs_reticulum_prns_expo_PrnsLanNative_nativeGeneration(
    _env: JNIEnv,
    _class: JClass,
) -> jlong {
    lan::current_generation() as jlong
}
#[no_mangle]
pub extern "system" fn Java_rs_reticulum_prns_expo_PrnsLanNative_nativeRevision(
    _env: JNIEnv,
    _class: JClass,
) -> jlong {
    lan::revision() as jlong
}
#[no_mangle]
pub extern "system" fn Java_rs_reticulum_prns_expo_PrnsLanNative_nativeWaitForChange(
    _env: JNIEnv,
    _class: JClass,
    observed: jlong,
    timeout: jlong,
) -> jlong {
    lan::wait_for_change(
        observed as u64,
        Duration::from_millis(timeout.clamp(1, 1000) as u64),
    ) as jlong
}
#[no_mangle]
pub extern "system" fn Java_rs_reticulum_prns_expo_PrnsLanNative_nativeWake(
    _env: JNIEnv,
    _class: JClass,
) {
    lan::wake();
}
#[no_mangle]
pub extern "system" fn Java_rs_reticulum_prns_expo_PrnsLanNative_nativeRetiring(
    _env: JNIEnv,
    _class: JClass,
    generation: jlong,
) -> jboolean {
    lan::session(generation as u64)
        .is_none_or(|session| session.is_retiring())
        .into()
}
#[no_mangle]
pub extern "system" fn Java_rs_reticulum_prns_expo_PrnsLanNative_nativeStopped(
    _env: JNIEnv,
    _class: JClass,
    generation: jlong,
) {
    lan::acknowledge_stopped(generation as u64);
}
#[no_mangle]
pub extern "system" fn Java_rs_reticulum_prns_expo_PrnsLanNative_nativeNetworkPermission(
    _env: JNIEnv,
    _class: JClass,
    generation: jlong,
    allowed: jboolean,
) {
    if let Some(session) = lan::session(generation as u64) {
        session.set_network_permission(allowed != 0);
    }
}
#[no_mangle]
pub extern "system" fn Java_rs_reticulum_prns_expo_PrnsLanNative_nativeDetail(
    mut env: JNIEnv,
    _class: JClass,
    generation: jlong,
    detail: JString,
) {
    let Some(session) = lan::session(generation as u64).filter(|session| session.active()) else {
        return;
    };
    let detail = if detail.is_null() {
        None
    } else {
        env.get_string(&detail)
            .ok()
            .map(|value| value.to_string_lossy().chars().take(256).collect())
    };
    session.set_detail(detail);
}

#[no_mangle]
pub extern "system" fn Java_rs_reticulum_prns_expo_PrnsLanNative_nativeInventory(
    mut env: JNIEnv,
    _class: JClass,
    generation: jlong,
    names: JObjectArray,
    indices: JIntArray,
    address_bytes: JObjectArray,
    prefixes: JIntArray,
    address_indices: JIntArray,
) -> jint {
    let Some(session) = lan::session(generation as u64).filter(|session| session.active()) else {
        return 0;
    };
    let result = (|| -> Option<HostLanReplaceOutcome> {
        let name_count = usize::try_from(env.get_array_length(&names).ok()?).ok()?;
        let address_count = usize::try_from(env.get_array_length(&address_bytes).ok()?).ok()?;
        if name_count > 8
            || address_count > 64
            || env.get_array_length(&indices).ok()? as usize != name_count
            || env.get_array_length(&prefixes).ok()? as usize != address_count
            || env.get_array_length(&address_indices).ok()? as usize != address_count
        {
            return None;
        }
        let mut native_indices = vec![0; name_count];
        let mut native_prefixes = vec![0; address_count];
        let mut native_address_indices = vec![0; address_count];
        env.get_int_array_region(&indices, 0, &mut native_indices)
            .ok()?;
        env.get_int_array_region(&prefixes, 0, &mut native_prefixes)
            .ok()?;
        env.get_int_array_region(&address_indices, 0, &mut native_address_indices)
            .ok()?;
        let mut interfaces = Vec::new();
        for (slot, &index) in native_indices.iter().enumerate() {
            let name = JString::from(env.get_object_array_element(&names, slot as i32).ok()?);
            let name: String = env.get_string(&name).ok()?.to_string_lossy().into_owned();
            if name.is_empty() || name.len() > 64 {
                return None;
            }
            let index = u32::try_from(index).ok().filter(|index| *index != 0)?;
            let mut addresses = Vec::new();
            for (offset, &address_index) in native_address_indices.iter().enumerate() {
                if address_index != index as i32 {
                    continue;
                }
                let array = JByteArray::from(
                    env.get_object_array_element(&address_bytes, offset as i32)
                        .ok()?,
                );
                let octets = env.convert_byte_array(array).ok()?;
                let address = match octets.as_slice() {
                    [a, b, c, d] => IpAddr::V4(Ipv4Addr::new(*a, *b, *c, *d)),
                    octets if octets.len() == 16 => {
                        IpAddr::V6(Ipv6Addr::from(<[u8; 16]>::try_from(octets).ok()?))
                    }
                    _ => return None,
                };
                if let Ok(address) =
                    HostLanAddress::new(address, u8::try_from(native_prefixes[offset]).ok()?)
                {
                    addresses.push(address);
                }
            }
            if !addresses.is_empty() {
                interfaces.push(HostLanInterface::new(name, index, addresses).ok()?);
            }
        }
        Some(session.inventory.replace(interfaces))
    })();
    inventory_publication(result, || session.network_changed())
}

fn inventory_publication(
    outcome: Option<HostLanReplaceOutcome>,
    network_changed: impl FnOnce(),
) -> jint {
    match outcome {
        Some(HostLanReplaceOutcome::Replaced) => {
            network_changed();
            2
        }
        Some(HostLanReplaceOutcome::Unchanged) => 1,
        Some(HostLanReplaceOutcome::Unavailable) | None => 0,
    }
}

pub fn ensure_linked() {
    std::hint::black_box([
        Java_rs_reticulum_prns_expo_PrnsLanNative_nativeGeneration as *const (),
        Java_rs_reticulum_prns_expo_PrnsLanNative_nativeRevision as *const (),
        Java_rs_reticulum_prns_expo_PrnsLanNative_nativeWaitForChange as *const (),
        Java_rs_reticulum_prns_expo_PrnsLanNative_nativeWake as *const (),
        Java_rs_reticulum_prns_expo_PrnsLanNative_nativeRetiring as *const (),
        Java_rs_reticulum_prns_expo_PrnsLanNative_nativeStopped as *const (),
        Java_rs_reticulum_prns_expo_PrnsLanNative_nativeNetworkPermission as *const (),
        Java_rs_reticulum_prns_expo_PrnsLanNative_nativeDetail as *const (),
        Java_rs_reticulum_prns_expo_PrnsLanNative_nativeInventory as *const (),
    ]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use prns_interfaces_tokio::wifi_auto::HostLanInventory;
    use std::cell::Cell;

    #[test]
    fn unchanged_connectivity_does_not_wake_or_retire_the_current_discovery() {
        let inventory = HostLanInventory::authoritative();
        let wakes = Cell::new(0);
        let publish = |interfaces| {
            inventory_publication(Some(inventory.replace(interfaces)), || {
                wakes.set(wakes.get() + 1)
            })
        };
        assert_eq!(publish(Vec::new()), 1);
        assert_eq!(wakes.get(), 0);
        let interface = HostLanInterface::new(
            "wlan0",
            7,
            vec![HostLanAddress::new("192.168.1.7".parse().unwrap(), 24).unwrap()],
        )
        .unwrap();
        assert_eq!(publish(vec![interface.clone()]), 2);
        assert_eq!(wakes.get(), 1);
        assert_eq!(publish(vec![interface]), 1);
        assert_eq!(wakes.get(), 1);
        assert_eq!(publish(Vec::new()), 2);
        assert_eq!(wakes.get(), 2);
        assert_eq!(publish(Vec::new()), 1);
        assert_eq!(wakes.get(), 2);
        assert_eq!(inventory_publication(None, || wakes.set(99)), 0);
        assert_eq!(
            inventory_publication(Some(HostLanReplaceOutcome::Unavailable), || wakes.set(99)),
            0
        );
        assert_eq!(wakes.get(), 2);
    }
}
