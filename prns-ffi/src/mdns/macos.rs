#![allow(deprecated)]

use core::cell::RefCell;
use core::num::NonZeroU8;
use core::time::Duration;
use std::collections::BTreeSet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV6};
use std::string::String;
use std::sync::{mpsc as sync_mpsc, Arc, Mutex};
use std::thread;
use std::vec::Vec;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{define_class, msg_send, AllocAnyThread, DefinedClass, Message};
use objc2_core_foundation::{kCFRunLoopDefaultMode, CFRunLoop};
use objc2_foundation::{
    NSArray, NSData, NSDictionary, NSNetService, NSNetServiceBrowser, NSNetServiceBrowserDelegate,
    NSNetServiceDelegate, NSNumber, NSObject, NSObjectProtocol, NSString,
};
use tokio::sync::{oneshot, watch};

use prns_core::interfaces::wifi_auto::{
    AdvertisementInsertion, CandidateInsertion, DiscoveryEndpoint, DiscoveryServiceName,
    DiscoveryServiceNameError, DiscoverySnapshot, DiscoveryVersion, DiscoveryVersionError,
    ServiceAdvertisement, DNS_SD_BASE_SERVICE_TYPE, DNS_SD_LOCAL_DOMAIN, TCP_RENDEZVOUS_PORT,
    TXT_VERSION_KEY, TXT_VERSION_VALUE,
};

pub const DISCOVERY_CAPACITY: NonZeroU8 = NonZeroU8::MAX;
const PUBLISH_TIMEOUT: Duration = Duration::from_secs(10);
const RESOLVE_TIMEOUT: f64 = 6.0;

const AF_INET: u8 = 2;
const AF_INET6: u8 = 30;

#[derive(Debug)]
pub enum MdnsError {
    PublishFailed,
    PublishTimeout,
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PublicationOutcome {
    Published,
    Rejected,
}

struct AdvertiserDelegateIvars {
    ready: RefCell<Option<oneshot::Sender<PublicationOutcome>>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[ivars = AdvertiserDelegateIvars]
    struct AdvertiserDelegate;

    unsafe impl NSObjectProtocol for AdvertiserDelegate {}

    unsafe impl NSNetServiceDelegate for AdvertiserDelegate {
        #[unsafe(method(netServiceDidPublish:))]
        fn did_publish(&self, _sender: &NSNetService) {
            if let Some(ready) = self.ivars().ready.borrow_mut().take() {
                let _ = ready.send(PublicationOutcome::Published);
            }
        }

        #[unsafe(method(netService:didNotPublish:))]
        fn did_not_publish(
            &self,
            _sender: &NSNetService,
            error: &NSDictionary<NSString, NSNumber>,
        ) {
            crate::diagnostic_log::error!("mdns: advertise failed: {error:?}");
            if let Some(ready) = self.ivars().ready.borrow_mut().take() {
                let _ = ready.send(PublicationOutcome::Rejected);
            }
        }
    }
);

impl AdvertiserDelegate {
    fn new(ready: oneshot::Sender<PublicationOutcome>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(AdvertiserDelegateIvars {
            ready: RefCell::new(Some(ready)),
        });
        // SAFETY: `this` is a freshly allocated AdvertiserDelegate with fully initialized ivars;
        // forwarding to NSObject's designated initializer preserves its allocation identity.
        unsafe { msg_send![super(this), init] }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum SnapshotResolution {
    Changed,
    Unchanged,
    RejectedAtCapacity,
    StateUnavailable,
}

#[derive(Debug, PartialEq, Eq)]
enum SnapshotRemoval {
    Changed,
    Unchanged,
    StateUnavailable,
}

struct SnapshotState {
    visible: Mutex<DiscoverySnapshot>,
    snapshot_sender: watch::Sender<DiscoverySnapshot>,
}

impl SnapshotState {
    fn channel(
        advertisement_capacity: NonZeroU8,
    ) -> (Arc<Self>, watch::Receiver<DiscoverySnapshot>) {
        let initial_snapshot = DiscoverySnapshot::new(advertisement_capacity);
        let (snapshot_sender, snapshot_receiver) = watch::channel(initial_snapshot.clone());
        (
            Arc::new(Self {
                visible: Mutex::new(initial_snapshot),
                snapshot_sender,
            }),
            snapshot_receiver,
        )
    }

    fn resolve(&self, service_advertisement: ServiceAdvertisement) -> SnapshotResolution {
        let Ok(mut visible_snapshot) = self.visible.lock() else {
            return SnapshotResolution::StateUnavailable;
        };
        if visible_snapshot.get(service_advertisement.service()) == Some(&service_advertisement) {
            return SnapshotResolution::Unchanged;
        }
        match visible_snapshot.insert(service_advertisement) {
            AdvertisementInsertion::Inserted | AdvertisementInsertion::Replaced => {
                self.snapshot_sender.send_replace(visible_snapshot.clone());
                SnapshotResolution::Changed
            }
            AdvertisementInsertion::AtCapacity => SnapshotResolution::RejectedAtCapacity,
        }
    }

    fn remove(&self, service_name: &DiscoveryServiceName) -> SnapshotRemoval {
        let Ok(mut visible_snapshot) = self.visible.lock() else {
            return SnapshotRemoval::StateUnavailable;
        };
        if visible_snapshot.remove(service_name) {
            self.snapshot_sender.send_replace(visible_snapshot.clone());
            SnapshotRemoval::Changed
        } else {
            SnapshotRemoval::Unchanged
        }
    }
}

struct ResolveDelegateIvars {
    snapshot_state: Arc<SnapshotState>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[ivars = ResolveDelegateIvars]
    struct ResolveDelegate;

    unsafe impl NSObjectProtocol for ResolveDelegate {}

    unsafe impl NSNetServiceDelegate for ResolveDelegate {
        #[unsafe(method(netServiceDidResolveAddress:))]
        fn did_resolve(&self, sender: &NSNetService) {
            let Some(addresses) = sender.addresses() else {
                return;
            };
            match apply_resolved_service(&self.ivars().snapshot_state, sender, &addresses) {
                ServiceResolutionOutcome::SnapshotChanged
                | ServiceResolutionOutcome::SnapshotUnchanged => {}
                ServiceResolutionOutcome::RejectedAtCapacity => {
                    crate::diagnostic_log::debug!(
                        "mdns: rejecting newly resolved service at Apple discovery capacity"
                    );
                }
                ServiceResolutionOutcome::StateUnavailable => {
                    crate::diagnostic_log::debug!("mdns: Apple discovery state is unavailable");
                }
                ServiceResolutionOutcome::RejectedRecord {
                    rejection,
                    visible_advertisement,
                } => {
                    crate::diagnostic_log::debug!(
                        "mdns: rejected resolved Apple service: {rejection:?}; \
                         previous advertisement: {visible_advertisement:?}"
                    );
                }
            }
        }

        #[unsafe(method(netService:didNotResolve:))]
        fn did_not_resolve(&self, sender: &NSNetService, error: &NSDictionary<NSString, NSNumber>) {
            crate::diagnostic_log::debug!("mdns: resolve failed: {error:?}");
            if let Ok(service_name) = discovery_service_name(sender) {
                match self.ivars().snapshot_state.remove(&service_name) {
                    SnapshotRemoval::Changed | SnapshotRemoval::Unchanged => {}
                    SnapshotRemoval::StateUnavailable => {
                        crate::diagnostic_log::debug!("mdns: Apple discovery state is unavailable");
                    }
                }
            }
        }
    }
);

impl ResolveDelegate {
    fn new(snapshot_state: Arc<SnapshotState>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(ResolveDelegateIvars { snapshot_state });
        // SAFETY: `this` is a freshly allocated ResolveDelegate with fully initialized ivars;
        // forwarding to NSObject's designated initializer preserves its allocation identity.
        unsafe { msg_send![super(this), init] }
    }
}

struct BrowserDelegateIvars {
    resolver: Retained<ResolveDelegate>,
    snapshot_state: Arc<SnapshotState>,
    resolving: RefCell<Vec<Retained<NSNetService>>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[ivars = BrowserDelegateIvars]
    struct BrowserDelegate;

    unsafe impl NSObjectProtocol for BrowserDelegate {}

    unsafe impl NSNetServiceBrowserDelegate for BrowserDelegate {
        #[unsafe(method(netServiceBrowser:didFindService:moreComing:))]
        fn did_find(
            &self,
            _browser: &NSNetServiceBrowser,
            service: &NSNetService,
            _more_coming: bool,
        ) {
            let service_name = match discovery_service_name(service) {
                Ok(service_name) => service_name,
                Err(service_rejection) => {
                    crate::diagnostic_log::debug!(
                        "mdns: rejected discovered Apple service: {service_rejection:?}"
                    );
                    return;
                }
            };
            let mut resolving_services = self.ivars().resolving.borrow_mut();
            let known_service_index = resolving_services.iter().position(|known_service| {
                discovery_service_name(known_service).as_ref() == Ok(&service_name)
            });
            match known_service_index {
                Some(known_service_index) => {
                    resolving_services[known_service_index] = service.retain();
                }
                None if resolving_services.len() >= usize::from(DISCOVERY_CAPACITY.get()) => {
                    crate::diagnostic_log::debug!(
                        "mdns: rejecting newly discovered Apple service at capacity"
                    );
                    return;
                }
                None => resolving_services.push(service.retain()),
            }
            drop(resolving_services);

            let resolver: &ResolveDelegate = &self.ivars().resolver;
            let resolver_protocol = ProtocolObject::from_ref(resolver);
            // SAFETY: both the discovered service and retained resolver delegate remain live while
            // Foundation installs the correctly typed protocol object.
            unsafe { service.setDelegate(Some(resolver_protocol)) };
            service.resolveWithTimeout(RESOLVE_TIMEOUT);
        }

        #[unsafe(method(netServiceBrowser:didRemoveService:moreComing:))]
        fn did_remove(
            &self,
            _browser: &NSNetServiceBrowser,
            service: &NSNetService,
            _more_coming: bool,
        ) {
            let Ok(service_name) = discovery_service_name(service) else {
                return;
            };
            match self.ivars().snapshot_state.remove(&service_name) {
                SnapshotRemoval::Changed | SnapshotRemoval::Unchanged => {}
                SnapshotRemoval::StateUnavailable => {
                    crate::diagnostic_log::debug!("mdns: Apple discovery state is unavailable");
                }
            }
            self.ivars().resolving.borrow_mut().retain(|candidate| {
                discovery_service_name(candidate).as_ref() != Ok(&service_name)
            });
        }
    }
);

impl BrowserDelegate {
    fn new(
        resolver: Retained<ResolveDelegate>,
        snapshot_state: Arc<SnapshotState>,
    ) -> Retained<Self> {
        let this = Self::alloc().set_ivars(BrowserDelegateIvars {
            resolver,
            snapshot_state,
            resolving: RefCell::new(Vec::new()),
        });
        // SAFETY: `this` is a freshly allocated BrowserDelegate with fully initialized ivars;
        // forwarding to NSObject's designated initializer preserves its allocation identity.
        unsafe { msg_send![super(this), init] }
    }
}

pub struct AppleServiceDiscoveryBackend {
    snapshot_receiver: watch::Receiver<DiscoverySnapshot>,
    _native_thread: NativeMdnsThread,
}

struct NativeMdnsThread {
    shutdown: Option<sync_mpsc::Sender<()>>,
    join: Option<thread::JoinHandle<()>>,
}

impl Drop for NativeMdnsThread {
    fn drop(&mut self) {
        self.shutdown.take();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl AppleServiceDiscoveryBackend {
    pub async fn new() -> Result<Self, MdnsError> {
        let (ready_sender, ready_receiver) = oneshot::channel::<PublicationOutcome>();
        let (snapshot_state, snapshot_receiver) = SnapshotState::channel(DISCOVERY_CAPACITY);
        let (shutdown_tx, shutdown_rx) = sync_mpsc::channel::<()>();
        let port = core::ffi::c_int::from(TCP_RENDEZVOUS_PORT);
        let service_type = format!("{DNS_SD_BASE_SERVICE_TYPE}.");
        let txt = [(
            String::from(TXT_VERSION_KEY),
            TXT_VERSION_VALUE.as_bytes().to_vec(),
        )];

        let join = thread::Builder::new()
            .name("hopspot-mdns".into())
            .spawn(move || {
                let advertiser = AdvertiserDelegate::new(ready_sender);
                let advertiser_proto = ProtocolObject::from_ref(&*advertiser);
                let service = NSNetService::initWithDomain_type_name_port(
                    NSNetService::alloc(),
                    &NSString::from_str(DNS_SD_LOCAL_DOMAIN),
                    &NSString::from_str(&service_type),
                    &NSString::from_str(""),
                    port,
                );
                // SAFETY: the service and retained advertiser delegate live for the entire run-loop
                // thread, and the protocol object has NSNetServiceDelegate's runtime type.
                unsafe { service.setDelegate(Some(advertiser_proto)) };
                service.setIncludesPeerToPeer(true);
                if let Some(data) = build_txt(&txt) {
                    service.setTXTRecordData(Some(&data));
                }
                service.publish();

                let resolver = ResolveDelegate::new(Arc::clone(&snapshot_state));
                let browser_delegate = BrowserDelegate::new(resolver, snapshot_state);
                let browser_proto = ProtocolObject::from_ref(&*browser_delegate);
                let browser = NSNetServiceBrowser::new();
                // SAFETY: the browser and retained browser delegate live for the entire run-loop
                // thread, and the protocol object has NSNetServiceBrowserDelegate's runtime type.
                unsafe { browser.setDelegate(Some(browser_proto)) };
                browser.setIncludesPeerToPeer(true);
                browser.searchForServicesOfType_inDomain(
                    &NSString::from_str(&service_type),
                    &NSString::from_str(DNS_SD_LOCAL_DOMAIN),
                );

                while matches!(shutdown_rx.try_recv(), Err(sync_mpsc::TryRecvError::Empty)) {
                    // SAFETY: the process-global default run-loop mode has static lifetime.
                    let mode = unsafe { kCFRunLoopDefaultMode };
                    let _ = CFRunLoop::run_in_mode(mode, 0.1, false);
                }
                service.stop();
                browser.stop();
                drop((service, advertiser, browser, browser_delegate));
            })
            .map_err(|_| MdnsError::Closed)?;
        let native_thread = NativeMdnsThread {
            shutdown: Some(shutdown_tx),
            join: Some(join),
        };

        match tokio::time::timeout(PUBLISH_TIMEOUT, ready_receiver).await {
            Ok(Ok(PublicationOutcome::Published)) => {
                crate::diagnostic_log::debug!(
                    "mdns: advertising + browsing {DNS_SD_BASE_SERVICE_TYPE} on port {port}"
                );
                Ok(Self {
                    snapshot_receiver,
                    _native_thread: native_thread,
                })
            }
            Ok(Ok(PublicationOutcome::Rejected)) => Err(MdnsError::PublishFailed),
            Ok(Err(_)) => Err(MdnsError::Closed),
            Err(_) => Err(MdnsError::PublishTimeout),
        }
    }

    /// Waits for the next complete, bounded Apple service-discovery snapshot.
    pub async fn next_snapshot(&mut self) -> Result<DiscoverySnapshot, MdnsError> {
        self.snapshot_receiver
            .changed()
            .await
            .map_err(|_native_thread_closed| MdnsError::Closed)?;
        Ok(self.snapshot_receiver.borrow_and_update().clone())
    }
}

fn parse_sockaddr(data: &[u8]) -> Option<SocketAddr> {
    if data.len() < 2 {
        return None;
    }
    match data[1] {
        AF_INET if data.len() >= 8 => {
            let port = u16::from_be_bytes([data[2], data[3]]);
            if port == 0 {
                return None;
            }
            let ip = Ipv4Addr::new(data[4], data[5], data[6], data[7]);
            Some(SocketAddr::new(IpAddr::V4(ip), port))
        }
        AF_INET6 if data.len() >= 24 => {
            let port = u16::from_be_bytes([data[2], data[3]]);
            if port == 0 {
                return None;
            }
            let mut octets = [0u8; 16];
            octets.copy_from_slice(&data[8..24]);
            let ip = Ipv6Addr::from(octets);
            let scope = if data.len() >= 28 {
                u32::from_ne_bytes([data[24], data[25], data[26], data[27]])
            } else {
                0
            };
            Some(SocketAddr::V6(SocketAddrV6::new(ip, port, 0, scope)))
        }
        _ => None,
    }
}

#[derive(Debug, PartialEq, Eq)]
enum ServiceRecordRejection {
    WrongServiceType,
    WrongDomain,
    InvalidServiceName(DiscoveryServiceNameError),
    WrongPort,
    InvalidVersion(DiscoveryVersionError),
    NoEligibleEndpoints,
}

#[derive(Debug, PartialEq, Eq)]
enum RejectedRecordCleanup {
    Removed,
    NotPresent,
    IdentityUnavailable,
    StateUnavailable,
}

#[derive(Debug, PartialEq, Eq)]
enum ServiceResolutionOutcome {
    SnapshotChanged,
    SnapshotUnchanged,
    RejectedAtCapacity,
    StateUnavailable,
    RejectedRecord {
        rejection: ServiceRecordRejection,
        visible_advertisement: RejectedRecordCleanup,
    },
}

fn apply_resolved_service(
    snapshot_state: &SnapshotState,
    service: &NSNetService,
    addresses: &NSArray<NSData>,
) -> ServiceResolutionOutcome {
    let service_name = match discovery_service_name(service) {
        Ok(service_name) => service_name,
        Err(rejection) => {
            return ServiceResolutionOutcome::RejectedRecord {
                rejection,
                visible_advertisement: RejectedRecordCleanup::IdentityUnavailable,
            };
        }
    };
    let service_advertisement =
        match resolved_service_advertisement(service_name.clone(), service, addresses) {
            Ok(service_advertisement) => service_advertisement,
            Err(rejection) => {
                let visible_advertisement = match snapshot_state.remove(&service_name) {
                    SnapshotRemoval::Changed => RejectedRecordCleanup::Removed,
                    SnapshotRemoval::Unchanged => RejectedRecordCleanup::NotPresent,
                    SnapshotRemoval::StateUnavailable => RejectedRecordCleanup::StateUnavailable,
                };
                return ServiceResolutionOutcome::RejectedRecord {
                    rejection,
                    visible_advertisement,
                };
            }
        };
    match snapshot_state.resolve(service_advertisement) {
        SnapshotResolution::Changed => ServiceResolutionOutcome::SnapshotChanged,
        SnapshotResolution::Unchanged => ServiceResolutionOutcome::SnapshotUnchanged,
        SnapshotResolution::RejectedAtCapacity => ServiceResolutionOutcome::RejectedAtCapacity,
        SnapshotResolution::StateUnavailable => ServiceResolutionOutcome::StateUnavailable,
    }
}

fn resolved_service_advertisement(
    service_name: DiscoveryServiceName,
    service: &NSNetService,
    addresses: &NSArray<NSData>,
) -> Result<ServiceAdvertisement, ServiceRecordRejection> {
    if u16::try_from(service.port()) != Ok(TCP_RENDEZVOUS_PORT) {
        return Err(ServiceRecordRejection::WrongPort);
    }
    discovery_version(service).map_err(ServiceRecordRejection::InvalidVersion)?;

    let mut discovery_endpoints = BTreeSet::new();
    for address in addresses.iter() {
        let Some(socket_address) = parse_sockaddr(&address.to_vec()) else {
            continue;
        };
        if let Ok(discovery_endpoint) = DiscoveryEndpoint::new(socket_address) {
            discovery_endpoints.insert(discovery_endpoint);
        }
    }

    let mut service_advertisement = ServiceAdvertisement::new(service_name);
    for discovery_endpoint in discovery_endpoints {
        if service_advertisement.insert(discovery_endpoint) == CandidateInsertion::AtCapacity {
            break;
        }
    }
    if service_advertisement.is_empty() {
        Err(ServiceRecordRejection::NoEligibleEndpoints)
    } else {
        Ok(service_advertisement)
    }
}

fn discovery_service_name(
    service: &NSNetService,
) -> Result<DiscoveryServiceName, ServiceRecordRejection> {
    let expected_service_type = format!("{DNS_SD_BASE_SERVICE_TYPE}.");
    if !service
        .r#type()
        .to_string()
        .eq_ignore_ascii_case(&expected_service_type)
    {
        return Err(ServiceRecordRejection::WrongServiceType);
    }
    if !service
        .domain()
        .to_string()
        .eq_ignore_ascii_case(DNS_SD_LOCAL_DOMAIN)
    {
        return Err(ServiceRecordRejection::WrongDomain);
    }
    DiscoveryServiceName::from_instance(&service.name().to_string())
        .map_err(ServiceRecordRejection::InvalidServiceName)
}

fn discovery_version(service: &NSNetService) -> Result<DiscoveryVersion, DiscoveryVersionError> {
    let Some(txt_record_data) = service.TXTRecordData() else {
        return DiscoveryVersion::parse(None);
    };
    let txt_record = txt_record_data.to_vec();
    match txt_version_metadata(&txt_record)? {
        TxtVersionMetadata::Missing => DiscoveryVersion::parse(None),
        TxtVersionMetadata::Value(version) => DiscoveryVersion::parse(Some(version)),
    }
}

#[derive(Debug, PartialEq, Eq)]
enum TxtVersionMetadata<'record> {
    Missing,
    Value(&'record [u8]),
}

fn txt_version_metadata(
    txt_record: &[u8],
) -> Result<TxtVersionMetadata<'_>, DiscoveryVersionError> {
    let mut remaining_record = txt_record;
    let mut version_metadata = TxtVersionMetadata::Missing;
    while let Some((&entry_length, remaining_after_length)) = remaining_record.split_first() {
        let entry_length = usize::from(entry_length);
        if remaining_after_length.len() < entry_length {
            return Err(DiscoveryVersionError::Malformed);
        }
        if entry_length == 0 {
            remaining_record = remaining_after_length;
            continue;
        }
        let (entry, remaining_after_entry) = remaining_after_length.split_at(entry_length);
        remaining_record = remaining_after_entry;
        let key_end = match entry.iter().position(|byte| *byte == b'=') {
            Some(separator_index) => separator_index,
            None => entry.len(),
        };
        let key = &entry[..key_end];
        if key.is_empty() {
            return Err(DiscoveryVersionError::Malformed);
        }
        if !key.eq_ignore_ascii_case(TXT_VERSION_KEY.as_bytes()) {
            continue;
        }
        version_metadata = match version_metadata {
            TxtVersionMetadata::Missing => TxtVersionMetadata::Value(if key_end == entry.len() {
                &[]
            } else {
                &entry[key_end + 1..]
            }),
            TxtVersionMetadata::Value(_) => return Err(DiscoveryVersionError::Malformed),
        };
    }
    Ok(version_metadata)
}

fn build_txt(pairs: &[(String, Vec<u8>)]) -> Option<Retained<NSData>> {
    if pairs.is_empty() {
        return None;
    }
    let keys: Vec<Retained<NSString>> = pairs
        .iter()
        .map(|(key, _)| NSString::from_str(key))
        .collect();
    let values: Vec<Retained<NSData>> = pairs
        .iter()
        .map(|(_, value)| NSData::with_bytes(value))
        .collect();
    let key_refs: Vec<&NSString> = keys.iter().map(|key| &**key).collect();
    let value_refs: Vec<&NSData> = values.iter().map(|value| &**value).collect();
    let dict = NSDictionary::from_slices(&key_refs, &value_refs);
    Some(NSNetService::dataFromTXTRecordDictionary(&dict))
}

#[cfg(test)]
mod native_thread_tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    #[derive(Debug, PartialEq, Eq)]
    enum TxtAssignment {
        Accepted,
        Rejected,
    }

    const SOCKADDR_V4_LENGTH: u8 = 16;

    fn apple_service(instance_name: &str, port: u16) -> Retained<NSNetService> {
        NSNetService::initWithDomain_type_name_port(
            NSNetService::alloc(),
            &NSString::from_str(DNS_SD_LOCAL_DOMAIN),
            &NSString::from_str(&format!("{DNS_SD_BASE_SERVICE_TYPE}.")),
            &NSString::from_str(instance_name),
            core::ffi::c_int::from(port),
        )
    }

    fn assign_txt_version(service: &NSNetService, version: &[u8]) -> TxtAssignment {
        let version_key = NSString::from_str(TXT_VERSION_KEY);
        let version_value = NSData::with_bytes(version);
        let txt_dictionary = NSDictionary::from_slices(&[&*version_key], &[&*version_value]);
        let txt_record = NSNetService::dataFromTXTRecordDictionary(&txt_dictionary);
        match service.setTXTRecordData(Some(&txt_record)) {
            true => TxtAssignment::Accepted,
            false => TxtAssignment::Rejected,
        }
    }

    fn ipv4_sockaddr_data(ip_address: Ipv4Addr, port: u16) -> Retained<NSData> {
        let mut bytes = vec![0u8; usize::from(SOCKADDR_V4_LENGTH)];
        bytes[0] = SOCKADDR_V4_LENGTH;
        bytes[1] = AF_INET;
        bytes[2..4].copy_from_slice(&port.to_be_bytes());
        bytes[4..8].copy_from_slice(&ip_address.octets());
        NSData::with_bytes(&bytes)
    }

    fn service_advertisement(
        service_name: &str,
        socket_address: &str,
    ) -> Result<ServiceAdvertisement, Box<dyn std::error::Error>> {
        let discovery_service_name = DiscoveryServiceName::new(service_name)?;
        let discovery_endpoint = DiscoveryEndpoint::new(socket_address.parse()?)?;
        let mut service_advertisement = ServiceAdvertisement::new(discovery_service_name);
        let _ = service_advertisement.insert(discovery_endpoint);
        Ok(service_advertisement)
    }

    #[test]
    fn dropping_owner_stops_and_joins_native_thread() {
        let exited = Arc::new(AtomicBool::new(false));
        let exited_on_thread = exited.clone();
        let (shutdown, shutdown_rx) = sync_mpsc::channel::<()>();
        let join = thread::spawn(move || {
            while matches!(shutdown_rx.try_recv(), Err(sync_mpsc::TryRecvError::Empty)) {
                thread::yield_now();
            }
            exited_on_thread.store(true, Ordering::Release);
        });
        let owner = NativeMdnsThread {
            shutdown: Some(shutdown),
            join: Some(join),
        };

        drop(owner);
        assert!(exited.load(Ordering::Acquire));
    }

    #[test]
    fn snapshot_state_is_bounded_and_known_service_updates_win(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let (snapshot_state, snapshot_receiver) = SnapshotState::channel(NonZeroU8::MIN);
        let initial = service_advertisement("first", "192.168.1.2:42699")?;
        assert_eq!(
            snapshot_state.resolve(initial.clone()),
            SnapshotResolution::Changed
        );
        assert_eq!(
            snapshot_state.resolve(initial),
            SnapshotResolution::Unchanged
        );
        assert_eq!(
            snapshot_state.resolve(service_advertisement("overflow", "192.168.1.3:42699")?),
            SnapshotResolution::RejectedAtCapacity
        );

        let replacement = service_advertisement("first", "192.168.1.4:42699")?;
        let first_service_name = replacement.service().clone();
        assert_eq!(
            snapshot_state.resolve(replacement),
            SnapshotResolution::Changed
        );
        assert_eq!(snapshot_receiver.borrow().len(), 1);
        assert_eq!(
            snapshot_state.remove(&first_service_name),
            SnapshotRemoval::Changed
        );
        assert_eq!(
            snapshot_state.remove(&first_service_name),
            SnapshotRemoval::Unchanged
        );
        assert!(snapshot_receiver.borrow().is_empty());
        Ok(())
    }

    #[test]
    fn txt_scanner_distinguishes_missing_present_and_malformed_versions() {
        assert_eq!(
            [
                txt_version_metadata(&[]),
                txt_version_metadata(&[3, b'v', b'=', b'1']),
                txt_version_metadata(&[3, b'x', b'=', b'a', 3, b'V', b'=', b'1']),
                txt_version_metadata(&[1, b'v']),
                txt_version_metadata(&[4, b'v', b'=', b'1']),
                txt_version_metadata(&[3, b'v', b'=', b'1', 3, b'V', b'=', b'1']),
                txt_version_metadata(&[0]),
            ],
            [
                Ok(TxtVersionMetadata::Missing),
                Ok(TxtVersionMetadata::Value(b"1")),
                Ok(TxtVersionMetadata::Value(b"1")),
                Ok(TxtVersionMetadata::Value(b"")),
                Err(DiscoveryVersionError::Malformed),
                Err(DiscoveryVersionError::Malformed),
                Ok(TxtVersionMetadata::Missing),
            ]
        );
    }

    #[test]
    fn resolved_service_versions_update_and_remove_one_bounded_snapshot(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let (snapshot_state, snapshot_receiver) = SnapshotState::channel(NonZeroU8::MIN);
        let service = apple_service("peer", TCP_RENDEZVOUS_PORT);
        let addresses = NSArray::from_retained_slice(&[
            ipv4_sockaddr_data("192.168.1.8".parse()?, TCP_RENDEZVOUS_PORT),
            ipv4_sockaddr_data("10.0.0.8".parse()?, TCP_RENDEZVOUS_PORT),
            ipv4_sockaddr_data("8.8.8.8".parse()?, TCP_RENDEZVOUS_PORT),
        ]);

        assert_eq!(
            apply_resolved_service(&snapshot_state, &service, &addresses),
            ServiceResolutionOutcome::SnapshotChanged
        );
        let mut expected_advertisement =
            ServiceAdvertisement::new(DiscoveryServiceName::from_instance("peer")?);
        assert_eq!(
            expected_advertisement.insert(DiscoveryEndpoint::new("10.0.0.8:42699".parse()?,)?),
            CandidateInsertion::Inserted
        );
        assert_eq!(
            expected_advertisement.insert(DiscoveryEndpoint::new("192.168.1.8:42699".parse()?,)?),
            CandidateInsertion::Inserted
        );
        let mut expected_snapshot = DiscoverySnapshot::new(NonZeroU8::MIN);
        assert_eq!(
            expected_snapshot.insert(expected_advertisement),
            AdvertisementInsertion::Inserted
        );
        assert_eq!(snapshot_receiver.borrow().clone(), expected_snapshot);

        assert_eq!(
            assign_txt_version(&service, TXT_VERSION_VALUE.as_bytes()),
            TxtAssignment::Accepted
        );
        assert_eq!(
            apply_resolved_service(&snapshot_state, &service, &addresses),
            ServiceResolutionOutcome::SnapshotUnchanged
        );

        assert_eq!(assign_txt_version(&service, b"2"), TxtAssignment::Accepted);
        assert_eq!(
            apply_resolved_service(&snapshot_state, &service, &addresses),
            ServiceResolutionOutcome::RejectedRecord {
                rejection: ServiceRecordRejection::InvalidVersion(
                    DiscoveryVersionError::Unsupported(2),
                ),
                visible_advertisement: RejectedRecordCleanup::Removed,
            }
        );
        assert!(snapshot_receiver.borrow().is_empty());
        assert_eq!(
            apply_resolved_service(&snapshot_state, &service, &addresses),
            ServiceResolutionOutcome::RejectedRecord {
                rejection: ServiceRecordRejection::InvalidVersion(
                    DiscoveryVersionError::Unsupported(2),
                ),
                visible_advertisement: RejectedRecordCleanup::NotPresent,
            }
        );
        Ok(())
    }

    #[test]
    fn sockaddr_decoding_preserves_ipv6_link_scope_for_shared_validation() {
        let mut bytes = vec![0u8; 28];
        bytes[1] = AF_INET6;
        bytes[2..4].copy_from_slice(&TCP_RENDEZVOUS_PORT.to_be_bytes());
        bytes[8..24].copy_from_slice(&Ipv6Addr::LOCALHOST.octets());
        bytes[8] = 0xfe;
        bytes[9] = 0x80;
        bytes[24..28].copy_from_slice(&7u32.to_ne_bytes());

        match parse_sockaddr(&bytes) {
            Some(SocketAddr::V6(socket_address)) => {
                assert!(socket_address.ip().is_unicast_link_local());
                assert_eq!(socket_address.scope_id(), 7);
            }
            decoded => assert!(
                matches!(decoded, Some(SocketAddr::V6(_))),
                "expected a scoped IPv6 address"
            ),
        }
    }
}
