use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use jni::objects::{JByteBuffer, JClass, JString};
use jni::sys::{jint, jstring};
use jni::JNIEnv;
use personal_rns::crypto::sha256;
use personal_rns::interfaces::wifi_auto as wifi_auto_contract;

use crate::engine::{identity_snapshot, mdns_bridge};

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeMdnsServicePort(
    _env: JNIEnv,
    _class: JClass,
) -> jint {
    jint::from(wifi_auto_contract::MDNS_SERVICE_PORT)
}

/// DNS-SD instance label matching host/ESP `prns-` + 8 hex digits, stable per node identity.
#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeMdnsInstanceName<'local>(
    env: JNIEnv<'local>,
    _class: JClass<'local>,
) -> jstring {
    env.new_string(mdns_instance_name())
        .map_or(core::ptr::null_mut(), JString::into_raw)
}

fn mdns_instance_name() -> String {
    let digest = match identity_snapshot() {
        Some(identity) => {
            let bytes = identity.node_identity_hash.as_bytes();
            [bytes[0], bytes[1], bytes[2], bytes[3]]
        }
        None => {
            let hash = sha256(wifi_auto_contract::GROUP_ID);
            [hash[0], hash[1], hash[2], hash[3]]
        }
    };
    format!(
        "prns-{:02x}{:02x}{:02x}{:02x}",
        digest[0], digest[1], digest[2], digest[3]
    )
}

/// Build a peer's AutoInterface find endpoint from the raw address bytes NsdManager resolved
/// (4 = IPv4, 16 = IPv6) and the SRV port. Prefer link-local IPv6 so UDP reverse probes can run;
/// an NsdManager-resolved IPv6 carries no interface scope and is probed across every AutoWifi NIC.
fn socket_addr(env: &JNIEnv, buffer: &JByteBuffer, port: jint) -> Option<SocketAddr> {
    let address = env.get_direct_buffer_address(buffer).ok()?;
    let capacity = env.get_direct_buffer_capacity(buffer).ok()?;
    let port = u16::try_from(port).ok()?;
    if address.is_null() || port == 0 {
        return None;
    }
    let ip = if capacity >= 16 {
        // SAFETY: `address` points at the JVM-owned direct buffer, pinned for this call; we read
        // exactly the 16 bytes whose presence the reported capacity just confirmed.
        let bytes = unsafe { core::slice::from_raw_parts(address, 16) };
        let mut octets = [0u8; 16];
        octets.copy_from_slice(bytes);
        IpAddr::V6(Ipv6Addr::from(octets))
    } else if capacity >= 4 {
        // SAFETY: as above, reading exactly the 4 IPv4 bytes the reported capacity confirmed present.
        let bytes = unsafe { core::slice::from_raw_parts(address, 4) };
        IpAddr::V4(Ipv4Addr::new(bytes[0], bytes[1], bytes[2], bytes[3]))
    } else {
        return None;
    };
    Some(SocketAddr::new(ip, port))
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeWifiSighting(
    env: JNIEnv,
    _class: JClass,
    address: JByteBuffer,
    port: jint,
) {
    if let Some(addr) = socket_addr(&env, &address, port) {
        mdns_bridge().sighting(addr);
    }
}
