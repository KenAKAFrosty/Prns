use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV6};
use std::ptr;

use jni::objects::{JByteArray, JIntArray, JObject, JObjectArray, JString};
use jni::sys::{jboolean, jint, jlong, jstring};
use jni::JNIEnv;
use prns_core::interfaces::wifi_auto as wifi_auto_contract;
use prns_core::interfaces::wifi_auto::DiscoveryTransport;
use prns_interfaces_tokio::wifi_auto::DiscoveryParticipation;

use prns_host_runtime::platform::android::lan;
use prns_interfaces_tokio::wifi_auto::PlatformServiceDiscoveryBridge;
use prns_interfaces_tokio::wifi_auto::{
    ServiceResolutionOutcome, DISCOVERY_CAPACITY, RESOLVED_CANDIDATE_INPUT_CAPACITY,
};

fn jni_string(env: JNIEnv, value: &str) -> jstring {
    env.new_string(value)
        .map_or(ptr::null_mut(), |value| value.into_raw())
}

fn lan_session(env: &mut JNIEnv, object: &JObject) -> Option<std::sync::Arc<lan::LanSession>> {
    let generation = env
        .get_field(object, "generation", "J")
        .ok()
        .and_then(|value| value.j().ok())
        .unwrap_or(0) as u64;
    lan::session(generation).filter(|session| session.active())
}

fn service_discovery_bridge(env: &mut JNIEnv, object: &JObject) -> PlatformServiceDiscoveryBridge {
    lan_session(env, object)
        .map(|session| session.bridge.clone())
        .unwrap_or_default()
}

#[no_mangle]
pub extern "system" fn Java_rs_reticulum_prns_expo_PrnsLanSessionNative_nativeWifiDiscoverySession(
    mut env: JNIEnv,
    object: JObject,
    session_id: jlong,
    active: jboolean,
) {
    if let (Some(session), Ok(session_id)) =
        (lan_session(&mut env, &object), u64::try_from(session_id))
    {
        session.observe_discovery_session(session_id, active != 0);
    }
}

#[no_mangle]
pub extern "system" fn Java_rs_reticulum_prns_expo_PrnsLanSessionNative_nativeWifiDiscoveryStarted(
    mut env: JNIEnv,
    object: JObject,
    session_id: jlong,
) {
    if let (Some(session), Ok(session_id)) =
        (lan_session(&mut env, &object), u64::try_from(session_id))
    {
        session.observe_tcp_browse_started(session_id);
    }
}

const DISCOVERY_INACTIVE: jint = 0;
const DISCOVERY_SATELLITE: jint = 1;
const DISCOVERY_CENTRAL: jint = 2;
const RESOLVED_SERVICE_VISIBLE: jint = 0;
const RESOLVED_SERVICE_REJECTED: jint = 1;
const RESOLVED_SERVICE_AT_CAPACITY: jint = 2;
const RESOLVED_SERVICE_UNAVAILABLE: jint = 3;

#[no_mangle]
pub extern "system" fn Java_rs_reticulum_prns_expo_PrnsLanSessionNative_nativeWifiTcpServicePort(
    _env: JNIEnv,
    _class: JObject,
) -> jint {
    jint::from(wifi_auto_contract::TCP_RENDEZVOUS_PORT)
}

#[no_mangle]
pub extern "system" fn Java_rs_reticulum_prns_expo_PrnsLanSessionNative_nativeWifiUdpServicePort(
    _env: JNIEnv,
    _class: JObject,
) -> jint {
    jint::from(wifi_auto_contract::UNICAST_DISCOVERY_PORT)
}

#[no_mangle]
pub extern "system" fn Java_rs_reticulum_prns_expo_PrnsLanSessionNative_nativeWifiTcpServiceType(
    env: JNIEnv,
    _class: JObject,
) -> jstring {
    jni_string(env, wifi_auto_contract::TCP_DNS_SD_BASE_SERVICE_TYPE)
}

#[no_mangle]
pub extern "system" fn Java_rs_reticulum_prns_expo_PrnsLanSessionNative_nativeWifiUdpServiceType(
    env: JNIEnv,
    _class: JObject,
) -> jstring {
    jni_string(env, wifi_auto_contract::UDP_DNS_SD_BASE_SERVICE_TYPE)
}

#[no_mangle]
pub extern "system" fn Java_rs_reticulum_prns_expo_PrnsLanSessionNative_nativeWifiTxtVersionKey(
    env: JNIEnv,
    _class: JObject,
) -> jstring {
    jni_string(env, wifi_auto_contract::TXT_VERSION_KEY)
}

#[no_mangle]
pub extern "system" fn Java_rs_reticulum_prns_expo_PrnsLanSessionNative_nativeWifiTxtVersionValue(
    env: JNIEnv,
    _class: JObject,
) -> jstring {
    jni_string(env, wifi_auto_contract::TXT_VERSION_VALUE)
}

#[no_mangle]
pub extern "system" fn Java_rs_reticulum_prns_expo_PrnsLanSessionNative_nativeWifiServiceCapacity(
    _env: JNIEnv,
    _class: JObject,
) -> jint {
    jint::from(DISCOVERY_CAPACITY.get())
}

#[no_mangle]
pub extern "system" fn Java_rs_reticulum_prns_expo_PrnsLanSessionNative_nativeWifiCandidateCapacity(
    _env: JNIEnv,
    _class: JObject,
) -> jint {
    jint::from(wifi_auto_contract::SERVICE_ADVERTISEMENT_CANDIDATE_CAPACITY)
}

#[no_mangle]
pub extern "system" fn Java_rs_reticulum_prns_expo_PrnsLanSessionNative_nativeWifiResolvedCandidateInputCapacity(
    _env: JNIEnv,
    _class: JObject,
) -> jint {
    jint::from(RESOLVED_CANDIDATE_INPUT_CAPACITY.get())
}

#[no_mangle]
pub extern "system" fn Java_rs_reticulum_prns_expo_PrnsLanSessionNative_nativeWifiDiscoveryParticipation(
    mut env: JNIEnv,
    _class: JObject,
) -> jint {
    match service_discovery_bridge(&mut env, &_class).synchronize_participation() {
        DiscoveryParticipation::Inactive => DISCOVERY_INACTIVE,
        DiscoveryParticipation::Satellite => DISCOVERY_SATELLITE,
        DiscoveryParticipation::Central => DISCOVERY_CENTRAL,
    }
}

#[no_mangle]
pub extern "system" fn Java_rs_reticulum_prns_expo_PrnsLanSessionNative_nativeWifiWorkGeneration(
    mut env: JNIEnv,
    _class: JObject,
) -> jlong {
    service_discovery_bridge(&mut env, &_class).work_generation() as jlong
}

#[no_mangle]
pub extern "system" fn Java_rs_reticulum_prns_expo_PrnsLanSessionNative_nativeWifiWaitForWork(
    mut env: JNIEnv,
    _class: JObject,
    observed_generation: jlong,
    timeout_millis: jlong,
) -> jlong {
    service_discovery_bridge(&mut env, &_class)
        .wait_for_work(observed_generation as u64, timeout_millis.max(0) as u64) as jlong
}

#[no_mangle]
pub extern "system" fn Java_rs_reticulum_prns_expo_PrnsLanSessionNative_nativeWifiWakeDiscoveryPump(
    mut env: JNIEnv,
    _class: JObject,
) {
    service_discovery_bridge(&mut env, &_class).wake_waiters();
}

fn publication_name(
    mut env: JNIEnv,
    _class: JObject,
    discovery_transport: DiscoveryTransport,
) -> jstring {
    match service_discovery_bridge(&mut env, &_class).publication_name(discovery_transport) {
        Ok(publication_name) => jni_string(env, publication_name.as_str()),
        Err(_publication_name_error) => ptr::null_mut(),
    }
}

#[no_mangle]
pub extern "system" fn Java_rs_reticulum_prns_expo_PrnsLanSessionNative_nativeWifiTcpPublicationName(
    env: JNIEnv,
    _class: JObject,
) -> jstring {
    publication_name(env, _class, DiscoveryTransport::Tcp)
}

#[no_mangle]
pub extern "system" fn Java_rs_reticulum_prns_expo_PrnsLanSessionNative_nativeWifiUdpPublicationName(
    env: JNIEnv,
    _class: JObject,
) -> jstring {
    publication_name(env, _class, DiscoveryTransport::Udp)
}

#[no_mangle]
pub extern "system" fn Java_rs_reticulum_prns_expo_PrnsLanSessionNative_nativeWifiEndPublicationSession(
    mut env: JNIEnv,
    _class: JObject,
) {
    service_discovery_bridge(&mut env, &_class).end_publication_session();
}

#[no_mangle]
pub extern "system" fn Java_rs_reticulum_prns_expo_PrnsLanSessionNative_nativeWifiRegistered(
    mut env: JNIEnv,
    _class: JObject,
    service_type: JString,
    service_instance: JString,
) {
    let Ok(service_type) = required_java_string(&mut env, &service_type) else {
        return;
    };
    let Ok(discovery_transport) = discovery_transport(&service_type) else {
        return;
    };
    let Ok(service_instance) = required_java_string(&mut env, &service_instance) else {
        return;
    };
    let _registration_outcome = service_discovery_bridge(&mut env, &_class)
        .registered(discovery_transport, &service_instance);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ServiceTypeError {
    Unsupported,
}

fn discovery_transport(service_type: &str) -> Result<DiscoveryTransport, ServiceTypeError> {
    if service_type == wifi_auto_contract::TCP_DNS_SD_BASE_SERVICE_TYPE {
        Ok(DiscoveryTransport::Tcp)
    } else if service_type == wifi_auto_contract::UDP_DNS_SD_BASE_SERVICE_TYPE {
        Ok(DiscoveryTransport::Udp)
    } else {
        Err(ServiceTypeError::Unsupported)
    }
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
    let candidate_capacity = usize::from(RESOLVED_CANDIDATE_INPUT_CAPACITY.get());
    if address_count > candidate_capacity {
        return Err(SocketAddressListError::CandidateCapacity {
            actual: address_count,
        });
    }
    let port = u16::try_from(port)
        .ok()
        .filter(|port| *port != 0)
        .ok_or(SocketAddressListError::InvalidPort)?;

    let mut candidate_scopes = [0; RESOLVED_CANDIDATE_INPUT_CAPACITY.get() as usize];
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
pub extern "system" fn Java_rs_reticulum_prns_expo_PrnsLanSessionNative_nativeWifiResolved(
    mut env: JNIEnv,
    _class: JObject,
    service_type: JString,
    service_instance: JString,
    address_arrays: JObjectArray,
    scope_ids: JIntArray,
    port: jint,
    version: JString,
) -> jint {
    let discovery_transport =
        match required_java_string(&mut env, &service_type).and_then(|service_type| {
            discovery_transport(&service_type).map_err(|_unsupported| JavaStringError::Invalid)
        }) {
            Ok(discovery_transport) => discovery_transport,
            Err(_invalid_service_type) => return RESOLVED_SERVICE_REJECTED,
        };
    let service_instance = match required_java_string(&mut env, &service_instance) {
        Ok(service_instance) => service_instance,
        Err(_invalid_service_instance) => return RESOLVED_SERVICE_REJECTED,
    };
    let socket_addresses = match socket_addresses(&mut env, &address_arrays, &scope_ids, port) {
        Ok(socket_addresses) => socket_addresses,
        Err(_invalid_socket_addresses) => return RESOLVED_SERVICE_REJECTED,
    };
    let version = match optional_java_string(&mut env, &version) {
        Ok(version) => version,
        Err(_invalid_version) => return RESOLVED_SERVICE_REJECTED,
    };
    match service_discovery_bridge(&mut env, &_class).resolved(
        discovery_transport,
        &service_instance,
        socket_addresses,
        version.as_deref().map(str::as_bytes),
    ) {
        ServiceResolutionOutcome::SnapshotChanged | ServiceResolutionOutcome::SnapshotUnchanged => {
            RESOLVED_SERVICE_VISIBLE
        }
        ServiceResolutionOutcome::RejectedRecord(_)
        | ServiceResolutionOutcome::RejectedParticipation(_) => RESOLVED_SERVICE_REJECTED,
        ServiceResolutionOutcome::RejectedAdvertisementCapacity => RESOLVED_SERVICE_AT_CAPACITY,
        ServiceResolutionOutcome::CapacityMismatch | ServiceResolutionOutcome::StateUnavailable => {
            RESOLVED_SERVICE_UNAVAILABLE
        }
    }
}

#[no_mangle]
pub extern "system" fn Java_rs_reticulum_prns_expo_PrnsLanSessionNative_nativeWifiLost(
    mut env: JNIEnv,
    _class: JObject,
    service_type: JString,
    service_instance: JString,
) {
    let Ok(service_type) = required_java_string(&mut env, &service_type) else {
        return;
    };
    let Ok(discovery_transport) = discovery_transport(&service_type) else {
        return;
    };
    let Ok(service_instance) = required_java_string(&mut env, &service_instance) else {
        return;
    };
    let _removal_outcome =
        service_discovery_bridge(&mut env, &_class).lost(discovery_transport, &service_instance);
}

/// Keep all selected-image LAN JNI exports under LTO.
pub fn ensure_linked() {
    std::hint::black_box([
        Java_rs_reticulum_prns_expo_PrnsLanSessionNative_nativeWifiDiscoverySession as *const (),
        Java_rs_reticulum_prns_expo_PrnsLanSessionNative_nativeWifiDiscoveryStarted as *const (),
        Java_rs_reticulum_prns_expo_PrnsLanSessionNative_nativeWifiTcpServicePort as *const (),
        Java_rs_reticulum_prns_expo_PrnsLanSessionNative_nativeWifiUdpServicePort as *const (),
        Java_rs_reticulum_prns_expo_PrnsLanSessionNative_nativeWifiTcpServiceType as *const (),
        Java_rs_reticulum_prns_expo_PrnsLanSessionNative_nativeWifiUdpServiceType as *const (),
        Java_rs_reticulum_prns_expo_PrnsLanSessionNative_nativeWifiTxtVersionKey as *const (),
        Java_rs_reticulum_prns_expo_PrnsLanSessionNative_nativeWifiTxtVersionValue as *const (),
        Java_rs_reticulum_prns_expo_PrnsLanSessionNative_nativeWifiServiceCapacity as *const (),
        Java_rs_reticulum_prns_expo_PrnsLanSessionNative_nativeWifiCandidateCapacity as *const (),
        Java_rs_reticulum_prns_expo_PrnsLanSessionNative_nativeWifiResolvedCandidateInputCapacity
            as *const (),
        Java_rs_reticulum_prns_expo_PrnsLanSessionNative_nativeWifiDiscoveryParticipation
            as *const (),
        Java_rs_reticulum_prns_expo_PrnsLanSessionNative_nativeWifiWorkGeneration as *const (),
        Java_rs_reticulum_prns_expo_PrnsLanSessionNative_nativeWifiWaitForWork as *const (),
        Java_rs_reticulum_prns_expo_PrnsLanSessionNative_nativeWifiWakeDiscoveryPump as *const (),
        Java_rs_reticulum_prns_expo_PrnsLanSessionNative_nativeWifiTcpPublicationName as *const (),
        Java_rs_reticulum_prns_expo_PrnsLanSessionNative_nativeWifiUdpPublicationName as *const (),
        Java_rs_reticulum_prns_expo_PrnsLanSessionNative_nativeWifiEndPublicationSession
            as *const (),
        Java_rs_reticulum_prns_expo_PrnsLanSessionNative_nativeWifiRegistered as *const (),
        Java_rs_reticulum_prns_expo_PrnsLanSessionNative_nativeWifiResolved as *const (),
        Java_rs_reticulum_prns_expo_PrnsLanSessionNative_nativeWifiLost as *const (),
    ]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn android_service_types_derive_from_the_shared_transport_contract() {
        assert_eq!(
            discovery_transport(wifi_auto_contract::TCP_DNS_SD_BASE_SERVICE_TYPE),
            Ok(DiscoveryTransport::Tcp)
        );
        assert_eq!(
            discovery_transport(wifi_auto_contract::UDP_DNS_SD_BASE_SERVICE_TYPE),
            Ok(DiscoveryTransport::Udp)
        );
        assert_eq!(
            discovery_transport("_reticulum._quic"),
            Err(ServiceTypeError::Unsupported)
        );
    }

    #[test]
    fn android_socket_addresses_preserve_ipv6_scope() {
        assert_eq!(
            socket_address(&Ipv6Addr::LOCALHOST.octets(), 7, 29_717),
            Ok(SocketAddr::V6(SocketAddrV6::new(
                Ipv6Addr::LOCALHOST,
                29_717,
                0,
                7,
            )))
        );
        assert_eq!(
            socket_address(&Ipv4Addr::LOCALHOST.octets(), 7, 42_699),
            Err(SocketAddressError::InvalidScope)
        );
    }
}
