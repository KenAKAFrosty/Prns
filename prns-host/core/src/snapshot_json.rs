use alloc::string::String;

use serde::ser::{Error as _, Serialize, SerializeSeq, SerializeStruct, Serializer};

use crate::{
    BackendInfo, DestinationIdentitySnapshot, HostSnapshot, InterfaceSnapshot, PersistenceSnapshot,
    RouteSnapshot, RuntimeHealthSnapshot, SAFE_UINT_MAX,
};

/// Serialize a canonical Host snapshot for a JSON transport.
///
/// The projection follows the Host contract rather than Serde's default integer representation:
/// contract `u64` values are decimal strings, fixed byte identifiers are byte arrays, and
/// `safeUint` values remain JSON numbers. Optional fields are omitted when absent.
///
/// # Errors
///
/// Returns an error if a value declared as `safeUint` by the Host contract is outside the
/// JavaScript-safe range, or if JSON serialization otherwise fails.
pub fn serialize_host_snapshot_json(snapshot: &HostSnapshot) -> Result<String, serde_json::Error> {
    serde_json::to_string(&JsonHostSnapshot(snapshot))
}

struct JsonHostSnapshot<'a>(&'a HostSnapshot);

impl Serialize for JsonHostSnapshot<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let snapshot = self.0;
        let mut state = serializer.serialize_struct("HostSnapshot", 8)?;
        state.serialize_field("revision", &DecimalU64(snapshot.revision))?;
        state.serialize_field("backend", &JsonBackendInfo(&snapshot.backend))?;
        state.serialize_field("interfaces", &JsonInterfaces(&snapshot.interfaces))?;
        state.serialize_field("routes", &JsonRoutes(&snapshot.routes))?;
        state.serialize_field("activeLinkCount", &snapshot.active_link_count)?;
        state.serialize_field(
            "destinationIdentities",
            &JsonDestinationIdentities(&snapshot.destination_identities),
        )?;
        state.serialize_field("runtime", &JsonRuntimeHealth(&snapshot.runtime))?;
        state.serialize_field("persistence", &JsonPersistence(&snapshot.persistence))?;
        state.end()
    }
}

struct JsonBackendInfo<'a>(&'a BackendInfo);

impl Serialize for JsonBackendInfo<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let backend = self.0;
        let mut state = serializer.serialize_struct("BackendInfo", 3)?;
        state.serialize_field("backend", backend.backend().contract_name())?;
        state.serialize_field("capabilities", &JsonCapabilities(backend))?;
        state.serialize_field("interfaceKinds", &JsonInterfaceKinds(backend))?;
        state.end()
    }
}

struct JsonCapabilities<'a>(&'a BackendInfo);

impl Serialize for JsonCapabilities<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let capabilities = self.0.capabilities();
        let mut sequence = serializer.serialize_seq(Some(capabilities.len()))?;
        for capability in capabilities {
            sequence.serialize_element(capability.contract_name())?;
        }
        sequence.end()
    }
}

struct JsonInterfaceKinds<'a>(&'a BackendInfo);

impl Serialize for JsonInterfaceKinds<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let interface_kinds = self.0.interface_kinds();
        let mut sequence = serializer.serialize_seq(Some(interface_kinds.len()))?;
        for kind in interface_kinds {
            sequence.serialize_element(kind.contract_name())?;
        }
        sequence.end()
    }
}

struct JsonInterfaces<'a>(&'a [InterfaceSnapshot]);

impl Serialize for JsonInterfaces<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut sequence = serializer.serialize_seq(Some(self.0.len()))?;
        for interface in self.0 {
            sequence.serialize_element(&JsonInterface(interface))?;
        }
        sequence.end()
    }
}

struct JsonInterface<'a>(&'a InterfaceSnapshot);

impl Serialize for JsonInterface<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let interface = self.0;
        let optional_fields = usize::from(interface.name.is_some())
            + usize::from(interface.kind.is_some())
            + usize::from(interface.failure_detail.is_some())
            + usize::from(interface.rx_bps.is_some())
            + usize::from(interface.tx_bps.is_some());
        let mut state = serializer.serialize_struct("InterfaceSnapshot", 7 + optional_fields)?;
        state.serialize_field("interfaceId", interface.interface_id.as_bytes())?;
        if let Some(name) = &interface.name {
            state.serialize_field("name", name)?;
        }
        if let Some(kind) = interface.kind {
            state.serialize_field("kind", kind.contract_name())?;
        }
        state.serialize_field("health", interface.health.contract_name())?;
        if let Some(detail) = &interface.failure_detail {
            state.serialize_field("failureDetail", detail)?;
        }
        state.serialize_field("rxBytes", &DecimalU64(interface.rx_bytes))?;
        state.serialize_field("txBytes", &DecimalU64(interface.tx_bytes))?;
        if let Some(value) = interface.rx_bps {
            state.serialize_field("rxBps", &SafeUint::new("interfaces[].rxBps", value))?;
        }
        if let Some(value) = interface.tx_bps {
            state.serialize_field("txBps", &SafeUint::new("interfaces[].txBps", value))?;
        }
        state.serialize_field("routeCount", &interface.route_count)?;
        state.serialize_field("linkCount", &interface.link_count)?;
        state.serialize_field("transportedLinkCount", &interface.transported_link_count)?;
        state.end()
    }
}

struct JsonRoutes<'a>(&'a [RouteSnapshot]);

impl Serialize for JsonRoutes<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut sequence = serializer.serialize_seq(Some(self.0.len()))?;
        for route in self.0 {
            sequence.serialize_element(&JsonRoute(route))?;
        }
        sequence.end()
    }
}

struct JsonRoute<'a>(&'a RouteSnapshot);

impl Serialize for JsonRoute<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let route = self.0;
        let mut state = serializer.serialize_struct(
            "RouteSnapshot",
            6 + usize::from(route.via_identity.is_some()),
        )?;
        state.serialize_field("destination", route.destination.as_bytes())?;
        state.serialize_field("hops", &route.hops)?;
        if let Some(identity) = route.via_identity {
            state.serialize_field("viaIdentity", identity.as_bytes())?;
        }
        state.serialize_field("interfaceId", route.interface_id.as_bytes())?;
        state.serialize_field(
            "learnedAtMillis",
            &SafeUint::new("routes[].learnedAtMillis", route.learned_at_millis),
        )?;
        state.serialize_field(
            "lastRouteActivityAtMillis",
            &SafeUint::new(
                "routes[].lastRouteActivityAtMillis",
                route.last_route_activity_at_millis,
            ),
        )?;
        state.serialize_field(
            "expiresAtMillis",
            &SafeUint::new("routes[].expiresAtMillis", route.expires_at_millis),
        )?;
        state.end()
    }
}

struct JsonDestinationIdentities<'a>(&'a [DestinationIdentitySnapshot]);

impl Serialize for JsonDestinationIdentities<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut sequence = serializer.serialize_seq(Some(self.0.len()))?;
        for association in self.0 {
            sequence.serialize_element(&JsonDestinationIdentity(association))?;
        }
        sequence.end()
    }
}

struct JsonDestinationIdentity<'a>(&'a DestinationIdentitySnapshot);

impl Serialize for JsonDestinationIdentity<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let association = self.0;
        let mut state = serializer.serialize_struct("DestinationIdentitySnapshot", 2)?;
        state.serialize_field("destination", association.destination.as_bytes())?;
        state.serialize_field("identity", association.identity.as_bytes())?;
        state.end()
    }
}

struct JsonRuntimeHealth<'a>(&'a RuntimeHealthSnapshot);

impl Serialize for JsonRuntimeHealth<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let runtime = self.0;
        let mut state = serializer.serialize_struct("RuntimeHealthSnapshot", 11)?;
        state.serialize_field("running", &runtime.running)?;
        state.serialize_field(
            "uptimeMillis",
            &SafeUint::new("runtime.uptimeMillis", runtime.uptime_millis),
        )?;
        state.serialize_field("interfaceCount", &runtime.interface_count)?;
        state.serialize_field("onlineInterfaceCount", &runtime.online_interface_count)?;
        state.serialize_field("routeCount", &runtime.route_count)?;
        state.serialize_field("linkCount", &runtime.link_count)?;
        state.serialize_field("transportedLinkCount", &runtime.transported_link_count)?;
        state.serialize_field("rxBytes", &DecimalU64(runtime.rx_bytes))?;
        state.serialize_field("txBytes", &DecimalU64(runtime.tx_bytes))?;
        state.serialize_field("rxBps", &SafeUint::new("runtime.rxBps", runtime.rx_bps))?;
        state.serialize_field("txBps", &SafeUint::new("runtime.txBps", runtime.tx_bps))?;
        state.end()
    }
}

struct JsonPersistence<'a>(&'a PersistenceSnapshot);

impl Serialize for JsonPersistence<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let persistence = self.0;
        let optional_fields = usize::from(persistence.last_flush_cause.is_some())
            + usize::from(persistence.last_failure_detail.is_some());
        let mut state = serializer.serialize_struct("PersistenceSnapshot", 2 + optional_fields)?;
        state.serialize_field("persistent", &persistence.persistent)?;
        state.serialize_field("restored", &persistence.restored)?;
        if let Some(cause) = persistence.last_flush_cause {
            state.serialize_field("lastFlushCause", cause.contract_name())?;
        }
        if let Some(detail) = &persistence.last_failure_detail {
            state.serialize_field("lastFailureDetail", detail)?;
        }
        state.end()
    }
}

struct DecimalU64(u64);

impl Serialize for DecimalU64 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_str(&self.0)
    }
}

struct SafeUint {
    field: &'static str,
    value: u64,
}

impl SafeUint {
    const fn new(field: &'static str, value: u64) -> Self {
        Self { field, value }
    }
}

impl Serialize for SafeUint {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if self.value > SAFE_UINT_MAX {
            return Err(S::Error::custom(format_args!(
                "{} value {} exceeds Host contract safeUint maximum {}",
                self.field, self.value, SAFE_UINT_MAX
            )));
        }
        serializer.serialize_u64(self.value)
    }
}
