use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV6};

use jni::objects::{JByteArray, JClass, JIntArray, JObjectArray, JString};
use jni::sys::{jboolean, jint, jlong, jstring};
use jni::JNIEnv;
use personal_rns::interfaces::wifi_auto as wifi_auto_contract;
use personal_rns::wifi_auto::DiscoveryParticipation;

use super::usb::jni_string;
use crate::engine::service_discovery_bridge;
use crate::service_discovery::DISCOVERY_CAPACITY;

const DISCOVERY_INACTIVE: jint = 0;
const DISCOVERY_SATELLITE: jint = 1;
const DISCOVERY_CENTRAL: jint = 2;

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeWifiServicePort(
    _env: JNIEnv,
    _class: JClass,
) -> jint {
    jint::from(wifi_auto_contract::TCP_RENDEZVOUS_PORT)
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeWifiServiceType(
    env: JNIEnv,
    _class: JClass,
) -> jstring {
    jni_string(env, wifi_auto_contract::DNS_SD_BASE_SERVICE_TYPE)
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeWifiTxtVersionKey(
    env: JNIEnv,
    _class: JClass,
) -> jstring {
    jni_string(env, wifi_auto_contract::TXT_VERSION_KEY)
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeWifiTxtVersionValue(
    env: JNIEnv,
    _class: JClass,
) -> jstring {
    jni_string(env, wifi_auto_contract::TXT_VERSION_VALUE)
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeWifiServiceCapacity(
    _env: JNIEnv,
    _class: JClass,
) -> jint {
    jint::from(DISCOVERY_CAPACITY.get())
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeWifiCandidateCapacity(
    _env: JNIEnv,
    _class: JClass,
) -> jint {
    jint::from(wifi_auto_contract::SERVICE_ADVERTISEMENT_CANDIDATE_CAPACITY)
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeWifiDiscoveryParticipation(
    _env: JNIEnv,
    _class: JClass,
) -> jint {
    match service_discovery_bridge().synchronize_participation() {
        DiscoveryParticipation::Inactive => DISCOVERY_INACTIVE,
        DiscoveryParticipation::Satellite => DISCOVERY_SATELLITE,
        DiscoveryParticipation::Central => DISCOVERY_CENTRAL,
    }
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeWifiWorkGeneration(
    _env: JNIEnv,
    _class: JClass,
) -> jlong {
    service_discovery_bridge().work_generation() as jlong
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeWifiWaitForWork(
    _env: JNIEnv,
    _class: JClass,
    observed_generation: jlong,
    timeout_millis: jlong,
) -> jlong {
    service_discovery_bridge()
        .wait_for_work(observed_generation as u64, timeout_millis.max(0) as u64) as jlong
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeWifiWakeDiscoveryPump(
    _env: JNIEnv,
    _class: JClass,
) {
    service_discovery_bridge().wake_waiters();
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SocketAddressError {
    InvalidScope,
    InvalidLength,
}

fn socket_address(
    address_octets: &[u8],
    scope_id: jint,
    port: u16,
) -> Result<SocketAddr, SocketAddressError> {
    if address_octets.len() == 16 {
        let mut octets = [0u8; 16];
        octets.copy_from_slice(address_octets);
        let scope_id = u32::try_from(scope_id).map_err(|_| SocketAddressError::InvalidScope)?;
        Ok(SocketAddr::V6(SocketAddrV6::new(
            Ipv6Addr::from(octets),
            port,
            0,
            scope_id,
        )))
    } else if address_octets.len() == 4 {
        if scope_id != 0 {
            return Err(SocketAddressError::InvalidScope);
        }
        Ok(SocketAddr::new(
            IpAddr::V4(Ipv4Addr::new(
                address_octets[0],
                address_octets[1],
                address_octets[2],
                address_octets[3],
            )),
            port,
        ))
    } else {
        Err(SocketAddressError::InvalidLength)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SocketAddressListError {
    ArrayUnavailable,
    InvalidPort,
    CandidateCapacity { actual: usize },
    ScopeCountMismatch { addresses: usize, scopes: usize },
    InvalidAddress(SocketAddressError),
}

fn socket_addresses(
    env: &mut JNIEnv,
    address_arrays: &JObjectArray,
    scope_ids: &JIntArray,
    port: jint,
) -> Result<Vec<SocketAddr>, SocketAddressListError> {
    let address_count = env
        .get_array_length(address_arrays)
        .ok()
        .and_then(|length| usize::try_from(length).ok())
        .ok_or(SocketAddressListError::ArrayUnavailable)?;
    let scope_count = env
        .get_array_length(scope_ids)
        .ok()
        .and_then(|length| usize::try_from(length).ok())
        .ok_or(SocketAddressListError::ArrayUnavailable)?;
    if address_count != scope_count {
        return Err(SocketAddressListError::ScopeCountMismatch {
            addresses: address_count,
            scopes: scope_count,
        });
    }
    let candidate_capacity =
        usize::from(wifi_auto_contract::SERVICE_ADVERTISEMENT_CANDIDATE_CAPACITY);
    if address_count > candidate_capacity {
        return Err(SocketAddressListError::CandidateCapacity {
            actual: address_count,
        });
    }
    let port = u16::try_from(port)
        .ok()
        .filter(|port| *port != 0)
        .ok_or(SocketAddressListError::InvalidPort)?;

    let mut candidate_scopes =
        [0; wifi_auto_contract::SERVICE_ADVERTISEMENT_CANDIDATE_CAPACITY as usize];
    env.get_int_array_region(scope_ids, 0, &mut candidate_scopes[..scope_count])
        .map_err(|_unavailable| SocketAddressListError::ArrayUnavailable)?;

    let mut candidates = Vec::with_capacity(address_count);
    for (candidate_index, scope_id) in candidate_scopes
        .iter()
        .copied()
        .take(address_count)
        .enumerate()
    {
        let address_object = env
            .get_object_array_element(address_arrays, candidate_index as jint)
            .map_err(|_unavailable| SocketAddressListError::ArrayUnavailable)?;
        let address_array = JByteArray::from(address_object);
        let address_octets = env
            .convert_byte_array(&address_array)
            .map_err(|_unavailable| SocketAddressListError::ArrayUnavailable)?;
        candidates.push(
            socket_address(&address_octets, scope_id, port)
                .map_err(SocketAddressListError::InvalidAddress)?,
        );
    }
    Ok(candidates)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JavaStringError {
    Missing,
    Invalid,
}

fn required_java_string(env: &mut JNIEnv, value: &JString) -> Result<String, JavaStringError> {
    if value.is_null() {
        return Err(JavaStringError::Missing);
    }
    env.get_string(value)
        .map_err(|_invalid| JavaStringError::Invalid)?
        .to_str()
        .map(str::to_owned)
        .map_err(|_invalid| JavaStringError::Invalid)
}

fn optional_java_string(
    env: &mut JNIEnv,
    value: &JString,
) -> Result<Option<String>, JavaStringError> {
    if value.is_null() {
        return Ok(None);
    }
    required_java_string(env, value).map(Some)
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeWifiResolved(
    mut env: JNIEnv,
    _class: JClass,
    service_instance: JString,
    address_arrays: JObjectArray,
    scope_ids: JIntArray,
    port: jint,
    version: JString,
) -> jboolean {
    let service_instance = match required_java_string(&mut env, &service_instance) {
        Ok(service_instance) => service_instance,
        Err(_invalid_service_instance) => return false.into(),
    };
    let socket_addresses = match socket_addresses(&mut env, &address_arrays, &scope_ids, port) {
        Ok(socket_addresses) => socket_addresses,
        Err(_invalid_socket_addresses) => return false.into(),
    };
    let version = match optional_java_string(&mut env, &version) {
        Ok(version) => version,
        Err(_invalid_version) => return false.into(),
    };
    service_discovery_bridge()
        .resolved(
            &service_instance,
            socket_addresses,
            version.as_deref().map(str::as_bytes),
        )
        .endpoint_is_visible()
        .into()
}

#[no_mangle]
pub extern "system" fn Java_org_personal_hopspot_NativeBridge_nativeWifiLost(
    mut env: JNIEnv,
    _class: JClass,
    service_instance: JString,
) {
    if let Ok(service_instance) = required_java_string(&mut env, &service_instance) {
        let _removal_outcome = service_discovery_bridge().lost(&service_instance);
    }
}
