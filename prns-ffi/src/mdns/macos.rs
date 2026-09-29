#![allow(deprecated)]

use core::cell::RefCell;
use core::num::NonZeroU8;
use core::time::Duration;
use std::collections::BTreeSet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV6};
use std::string::String;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc as sync_mpsc, Arc, Mutex};
use std::thread;
use std::vec::Vec;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{define_class, msg_send, AllocAnyThread, DefinedClass, Message};
use objc2_core_foundation::{kCFRunLoopDefaultMode, CFRunLoop};
use objc2_foundation::{
    NSArray, NSBundle, NSData, NSDictionary, NSNetService, NSNetServiceBrowser,
    NSNetServiceBrowserDelegate, NSNetServiceDelegate, NSNumber, NSObject, NSObjectProtocol,
    NSString,
};
use tokio::sync::{oneshot, watch};

#[cfg(test)]
use prns_core::interfaces::wifi_auto::EPHEMERAL_DISCOVERY_INSTANCE_RANDOM_BYTES;
use prns_core::interfaces::wifi_auto::{
    AdvertisementInsertion, AdvertisementRemoval, CandidateInsertion, CandidateInsertionError,
    DiscoveryEndpoint, DiscoveryServiceName, DiscoveryServiceNameError, DiscoverySnapshot,
    DiscoveryTransport, DiscoveryVersion, DiscoveryVersionError, EphemeralDiscoveryInstanceName,
    ServiceAdvertisement, DEFAULT_DISCOVERY_SERVICE_CAPACITY, DNS_SD_LOCAL_DOMAIN,
    TCP_DNS_SD_BASE_SERVICE_TYPE, TCP_RENDEZVOUS_PORT, TXT_VERSION_KEY, TXT_VERSION_VALUE,
    UDP_DNS_SD_BASE_SERVICE_TYPE, UNICAST_DISCOVERY_PORT,
};

pub const DISCOVERY_CAPACITY: NonZeroU8 = DEFAULT_DISCOVERY_SERVICE_CAPACITY;
const PUBLISH_TIMEOUT: Duration = Duration::from_secs(10);
const RESOLVE_TIMEOUT: f64 = 6.0;

const AF_INET: u8 = 2;
const AF_INET6: u8 = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MdnsError {
    PublishFailed,
    ServiceUnavailable,
    PublishTimeout,
    InvalidPublicationName,
    Closed,
}

impl core::fmt::Display for MdnsError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::PublishFailed => formatter.write_str("Apple DNS-SD publication failed"),
            Self::ServiceUnavailable => formatter.write_str("Apple DNS-SD service is unavailable"),
            Self::PublishTimeout => formatter.write_str("Apple DNS-SD publication timed out"),
            Self::InvalidPublicationName => {
                formatter.write_str("Apple DNS-SD publication name is invalid")
            }
            Self::Closed => formatter.write_str("Apple DNS-SD backend closed"),
        }
    }
}

impl std::error::Error for MdnsError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PublicationOutcome {
    Published,
    Rejected(MdnsError),
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
        match visible_snapshot.remove(service_name) {
            AdvertisementRemoval::Removed => {
                self.snapshot_sender.send_replace(visible_snapshot.clone());
                SnapshotRemoval::Changed
            }
            AdvertisementRemoval::NotPresent => SnapshotRemoval::Unchanged,
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
    local_service_names: Arc<BTreeSet<DiscoveryServiceName>>,
    backend_failed: Arc<AtomicBool>,
    resolving: RefCell<Vec<Retained<NSNetService>>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[ivars = BrowserDelegateIvars]
    struct BrowserDelegate;

    unsafe impl NSObjectProtocol for BrowserDelegate {}

    unsafe impl NSNetServiceBrowserDelegate for BrowserDelegate {
        #[unsafe(method(netServiceBrowserDidStopSearch:))]
        fn did_stop_search(&self, _browser: &NSNetServiceBrowser) {
            self.ivars().backend_failed.store(true, Ordering::Release);
        }

        #[unsafe(method(netServiceBrowser:didNotSearch:))]
        fn did_not_search(
            &self,
            _browser: &NSNetServiceBrowser,
            error: &NSDictionary<NSString, NSNumber>,
        ) {
            crate::diagnostic_log::error!("mdns: browse failed: {error:?}");
            self.ivars().backend_failed.store(true, Ordering::Release);
        }

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
            if self.ivars().local_service_names.contains(&service_name) {
                return;
            }
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
        local_service_names: Arc<BTreeSet<DiscoveryServiceName>>,
        backend_failed: Arc<AtomicBool>,
    ) -> Retained<Self> {
        let this = Self::alloc().set_ivars(BrowserDelegateIvars {
            resolver,
            snapshot_state,
            local_service_names,
            backend_failed,
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
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

struct ApplePublication {
    transport: DiscoveryTransport,
    instance_name: EphemeralDiscoveryInstanceName,
    service_name: DiscoveryServiceName,
}

impl ApplePublication {
    fn new(
        transport: DiscoveryTransport,
        instance_name: EphemeralDiscoveryInstanceName,
    ) -> Result<Self, MdnsError> {
        let service_name = DiscoveryServiceName::from_instance(instance_name.as_str(), transport)
            .map_err(|_invalid_generated_name| MdnsError::InvalidPublicationName)?;
        Ok(Self {
            transport,
            instance_name,
            service_name,
        })
    }
}

/// Logs whether `NSBundle.mainBundle` sees the Local Network / Bonjour Info.plist keys.
///
/// Sequoia returns `NSNetServicesMissingRequiredConfigurationError` (-72008) when these
/// are missing from the *process* main bundle, even if `Contents/Info.plist` is correct on disk.
fn log_main_bundle_privacy_keys() {
    let bundle = NSBundle::mainBundle();
    let path = bundle.bundlePath().to_string();
    let lan = bundle
        .objectForInfoDictionaryKey(&NSString::from_str("NSLocalNetworkUsageDescription"))
        .map(|value| format!("{value:?}"))
        .unwrap_or_else(|| "MISSING".into());
    let bonjour = bundle
        .objectForInfoDictionaryKey(&NSString::from_str("NSBonjourServices"))
        .map(|value| format!("{value:?}"))
        .unwrap_or_else(|| "MISSING".into());
    crate::diagnostic_log::debug!(
        "mdns: mainBundle={path} NSLocalNetworkUsageDescription={lan} NSBonjourServices={bonjour}"
    );
}

/// Best-effort Local Network privacy alert trigger from TN3179 (UDP connect to link-local).
fn trigger_local_network_privacy_alert() {
    let mut ifaddrs_head: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: getifaddrs writes a linked list we free with freeifaddrs.
    if unsafe { libc::getifaddrs(&mut ifaddrs_head) } != 0 || ifaddrs_head.is_null() {
        return;
    }
    let mut cursor = ifaddrs_head;
    while !cursor.is_null() {
        // SAFETY: cursor is a live ifaddrs node from getifaddrs until freeifaddrs.
        let ifa = unsafe { &*cursor };
        cursor = ifa.ifa_next;
        let addr = ifa.ifa_addr;
        if addr.is_null() {
            continue;
        }
        // SAFETY: addr is the interface address pointer from getifaddrs.
        let family = unsafe { (*addr).sa_family as i32 };
        if family != libc::AF_INET6 {
            continue;
        }
        // SAFETY: family checked as AF_INET6; sockaddr_in6 is the matching layout.
        let mut sin6 = unsafe { *(addr as *const libc::sockaddr_in6) };
        let octets = sin6.sin6_addr.s6_addr;
        // fe80::/10 link-local only.
        if octets[0] != 0xfe || octets[1] & 0xc0 != 0x80 {
            continue;
        }
        sin6.sin6_port = 9u16.to_be();
        for byte in &mut sin6.sin6_addr.s6_addr[8..] {
            *byte = rand_u8();
        }
        // SAFETY: AF_INET6 SOCK_DGRAM creates an unbound datagram socket; no prior state.
        let fd = unsafe { libc::socket(libc::AF_INET6, libc::SOCK_DGRAM, 0) };
        if fd < 0 {
            continue;
        }
        // SAFETY: `fd` is an open AF_INET6 datagram socket; `sin6` is a stack sockaddr_in6
        // whose length matches socklen_t below (TN3179 best-effort privacy trigger).
        let _ = unsafe {
            libc::connect(
                fd,
                &sin6 as *const libc::sockaddr_in6 as *const libc::sockaddr,
                libc::socklen_t::try_from(std::mem::size_of::<libc::sockaddr_in6>()).unwrap_or(0),
            )
        };
        // SAFETY: `fd` was opened above and is not used after this close.
        unsafe { libc::close(fd) };
    }
    // SAFETY: pairs with successful getifaddrs above.
    unsafe { libc::freeifaddrs(ifaddrs_head) };
}

fn rand_u8() -> u8 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use std::time::{SystemTime, UNIX_EPOCH};
    let mut hasher = DefaultHasher::new();
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0)
        .hash(&mut hasher);
    std::thread::current().id().hash(&mut hasher);
    (hasher.finish() & 0xff) as u8
}

fn apple_service_type(discovery_transport: DiscoveryTransport) -> String {
    format!("{}.", discovery_transport.dns_sd_base_service_type())
}

mod dns_sd_ll {
    use super::{DiscoveryTransport, MdnsError, TXT_VERSION_KEY, TXT_VERSION_VALUE};
    use std::ffi::{c_void, CString};
    use std::net::Ipv6Addr;
    use std::ptr;
    use std::vec::Vec;

    type DnsServiceErrorType = i32;
    type DnsServiceFlags = u32;
    type DnsServiceRef = *mut c_void;
    type DnsRecordRef = *mut c_void;

    const DNS_SERVICE_NO_ERROR: DnsServiceErrorType = 0;
    const DNS_SERVICE_ERR_SERVICE_NOT_RUNNING: DnsServiceErrorType = -65563;
    const DNS_SERVICE_FLAGS_SHARED: DnsServiceFlags = 0x10;
    const DNS_SERVICE_FLAGS_UNIQUE: DnsServiceFlags = 0x20;
    const DNS_SERVICE_FLAGS_SHARE_CONNECTION: DnsServiceFlags = 0x4000;
    const DNS_SERVICE_TYPE_AAAA: u16 = 28;
    const DNS_SERVICE_CLASS_IN: u16 = 1;
    const DNS_SERVICE_INTERFACE_INDEX_ANY: u32 = 0;

    #[link(name = "System")]
    unsafe extern "C" {
        fn DNSServiceCreateConnection(sd_ref: *mut DnsServiceRef) -> DnsServiceErrorType;
        fn DNSServiceRegister(
            sd_ref: *mut DnsServiceRef,
            flags: DnsServiceFlags,
            interface_index: u32,
            name: *const libc::c_char,
            regtype: *const libc::c_char,
            domain: *const libc::c_char,
            host: *const libc::c_char,
            port: u16,
            txt_len: u16,
            txt_record: *const c_void,
            call_back: Option<
                unsafe extern "C" fn(
                    DnsServiceRef,
                    DnsServiceFlags,
                    u32,
                    DnsServiceErrorType,
                    *const libc::c_char,
                    *const libc::c_char,
                    *const libc::c_char,
                    u16,
                    *mut c_void,
                ),
            >,
            context: *mut c_void,
        ) -> DnsServiceErrorType;
        fn DNSServiceRegisterRecord(
            sd_ref: DnsServiceRef,
            record_ref: *mut DnsRecordRef,
            flags: DnsServiceFlags,
            interface_index: u32,
            fullname: *const libc::c_char,
            rrtype: u16,
            rrclass: u16,
            rdlen: u16,
            rdata: *const c_void,
            ttl: u32,
            call_back: Option<
                unsafe extern "C" fn(
                    DnsServiceRef,
                    DnsRecordRef,
                    DnsServiceFlags,
                    DnsServiceErrorType,
                    *mut c_void,
                ),
            >,
            context: *mut c_void,
        ) -> DnsServiceErrorType;
        fn DNSServiceProcessResult(sd_ref: DnsServiceRef) -> DnsServiceErrorType;
        fn DNSServiceRefDeallocate(sd_ref: DnsServiceRef);
        fn DNSServiceRefSockFD(sd_ref: DnsServiceRef) -> libc::c_int;
    }

    /// `DNSServiceRegisterRecord` rejects a NULL callback (`kDNSServiceErr_BadParam`).
    unsafe extern "C" fn register_record_reply(
        _sd_ref: DnsServiceRef,
        _record_ref: DnsRecordRef,
        _flags: DnsServiceFlags,
        error_code: DnsServiceErrorType,
        _context: *mut c_void,
    ) {
        if error_code != DNS_SERVICE_NO_ERROR {
            crate::diagnostic_log::debug!(
                "mdns: DNSServiceRegisterRecord async error {error_code}"
            );
        }
    }

    const fn publication_error(error_code: DnsServiceErrorType) -> MdnsError {
        if error_code == DNS_SERVICE_ERR_SERVICE_NOT_RUNNING {
            MdnsError::ServiceUnavailable
        } else {
            MdnsError::PublishFailed
        }
    }

    pub(super) const fn record_registration_flags(record_index: usize) -> DnsServiceFlags {
        if record_index == 0 {
            DNS_SERVICE_FLAGS_UNIQUE
        } else {
            DNS_SERVICE_FLAGS_SHARED
        }
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub(super) struct LinkLocalHostAddress {
        address: Ipv6Addr,
        interface_index: u32,
    }

    pub(super) struct LinkLocalPublisher {
        connection: DnsServiceRef,
        _aaaa_records: Vec<DnsRecordRef>,
        _service_refs: Vec<DnsServiceRef>,
        _advertised_addresses: Vec<Ipv6Addr>,
    }

    // SAFETY: `LinkLocalPublisher` owns the DNSService connection exclusively on the
    // native mDNS thread and never shares the raw refs across threads.
    unsafe impl Send for LinkLocalPublisher {}

    impl LinkLocalPublisher {
        pub(super) fn advertise(
            hostname_label: &str,
            tcp_instance_name: &str,
            udp_instance_name: &str,
            tcp_port: u16,
            udp_port: u16,
        ) -> Result<Self, MdnsError> {
            let link_local_hosts = local_unicast_link_local_hosts();
            if link_local_hosts.is_empty() {
                crate::diagnostic_log::debug!(
                    "mdns: no eligible unicast link-local IPv6 hosts for DNS-SD publication"
                );
                return Err(MdnsError::PublishFailed);
            }

            let hostname = CString::new(format!("{hostname_label}.local."))
                .map_err(|_| MdnsError::InvalidPublicationName)?;
            let tcp_name =
                CString::new(tcp_instance_name).map_err(|_| MdnsError::InvalidPublicationName)?;
            let udp_name =
                CString::new(udp_instance_name).map_err(|_| MdnsError::InvalidPublicationName)?;
            let tcp_type = CString::new(format!(
                "{}.",
                DiscoveryTransport::Tcp.dns_sd_base_service_type()
            ))
            .map_err(|_| MdnsError::InvalidPublicationName)?;
            let udp_type = CString::new(format!(
                "{}.",
                DiscoveryTransport::Udp.dns_sd_base_service_type()
            ))
            .map_err(|_| MdnsError::InvalidPublicationName)?;
            let txt_record = txt_version_record();

            let mut connection: DnsServiceRef = ptr::null_mut();
            // SAFETY: DNSServiceCreateConnection writes a valid opaque ref on success.
            let create_error = unsafe { DNSServiceCreateConnection(&mut connection) };
            if create_error != DNS_SERVICE_NO_ERROR || connection.is_null() {
                crate::diagnostic_log::debug!(
                    "mdns: DNSServiceCreateConnection failed ({create_error})"
                );
                return Err(publication_error(create_error));
            }

            let advertised_addresses = link_local_hosts
                .iter()
                .map(|host| host.address)
                .collect::<Vec<_>>();
            let mut publisher = Self {
                connection,
                _aaaa_records: Vec::new(),
                _service_refs: Vec::new(),
                _advertised_addresses: advertised_addresses.clone(),
            };

            for (index, host) in link_local_hosts.iter().enumerate() {
                let mut record_ref: DnsRecordRef = ptr::null_mut();
                let octets = host.address.octets();
                // First AAAA claims uniqueness; additional same-name records are shared.
                let record_flags = record_registration_flags(index);
                // SAFETY: connection is live; callback is non-null (required by DNS-SD);
                // rdata points at a 16-byte AAAA payload for the call.
                let register_error = unsafe {
                    DNSServiceRegisterRecord(
                        publisher.connection,
                        &mut record_ref,
                        record_flags,
                        host.interface_index,
                        hostname.as_ptr(),
                        DNS_SERVICE_TYPE_AAAA,
                        DNS_SERVICE_CLASS_IN,
                        u16::try_from(octets.len()).unwrap_or(0),
                        octets.as_ptr().cast::<c_void>(),
                        0,
                        Some(register_record_reply),
                        ptr::null_mut(),
                    )
                };
                if register_error != DNS_SERVICE_NO_ERROR {
                    crate::diagnostic_log::debug!(
                        "mdns: DNSServiceRegisterRecord({}) on ifindex {} failed ({register_error})",
                        host.address,
                        host.interface_index
                    );
                    return Err(publication_error(register_error));
                }
                publisher._aaaa_records.push(record_ref);
            }

            publisher.register_service(&tcp_name, &tcp_type, &hostname, tcp_port, &txt_record)?;
            publisher.register_service(&udp_name, &udp_type, &hostname, udp_port, &txt_record)?;

            crate::diagnostic_log::debug!(
                "mdns: publishing LL-only AAAA {:?} for {hostname_label}.local. ports {tcp_port}/{udp_port}",
                advertised_addresses
            );
            Ok(publisher)
        }

        fn register_service(
            &mut self,
            name: &CString,
            regtype: &CString,
            host: &CString,
            port: u16,
            txt_record: &[u8],
        ) -> Result<(), MdnsError> {
            let mut service_ref = self.connection;
            // SAFETY: ShareConnection reuses `self.connection`; port is network-order as required.
            let register_error = unsafe {
                DNSServiceRegister(
                    &mut service_ref,
                    DNS_SERVICE_FLAGS_SHARE_CONNECTION,
                    DNS_SERVICE_INTERFACE_INDEX_ANY,
                    name.as_ptr(),
                    regtype.as_ptr(),
                    ptr::null(),
                    host.as_ptr(),
                    port.to_be(),
                    u16::try_from(txt_record.len()).unwrap_or(0),
                    txt_record.as_ptr().cast::<c_void>(),
                    None,
                    ptr::null_mut(),
                )
            };
            if register_error != DNS_SERVICE_NO_ERROR {
                crate::diagnostic_log::debug!("mdns: DNSServiceRegister failed ({register_error})");
                return Err(publication_error(register_error));
            }
            self._service_refs.push(service_ref);
            Ok(())
        }

        pub(super) fn process_pending(&self) {
            let sock_fd = // SAFETY: connection remains owned by this publisher.
                unsafe { DNSServiceRefSockFD(self.connection) };
            if sock_fd < 0 {
                return;
            }
            let mut poll_fd = libc::pollfd {
                fd: sock_fd,
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: poll inspects one stack-local descriptor for readability.
            let ready = unsafe { libc::poll(&mut poll_fd, 1, 0) };
            if ready > 0 && poll_fd.revents & libc::POLLIN != 0 {
                // SAFETY: connection is valid and has readable daemon traffic.
                let _ = unsafe { DNSServiceProcessResult(self.connection) };
            }
        }
    }

    impl Drop for LinkLocalPublisher {
        fn drop(&mut self) {
            // SAFETY: deallocate the shared connection; subordinate refs are invalidated with it.
            unsafe { DNSServiceRefDeallocate(self.connection) };
            self.connection = ptr::null_mut();
            self._aaaa_records.clear();
            self._service_refs.clear();
        }
    }

    fn txt_version_record() -> Vec<u8> {
        let entry = format!("{TXT_VERSION_KEY}={TXT_VERSION_VALUE}");
        let mut record = Vec::with_capacity(entry.len() + 1);
        record.push(u8::try_from(entry.len()).unwrap_or(0));
        record.extend(entry.bytes());
        record
    }

    pub(super) fn interface_name_is_infrastructure(name: &str) -> bool {
        // Skip Apple peer-to-peer / tunnel ifaces; AutoWifi peers over LAN infra (en*).
        !(name.starts_with("awdl")
            || name.starts_with("llw")
            || name.starts_with("utun")
            || name.starts_with("bridge")
            || name.starts_with("ap"))
    }

    pub(super) fn local_unicast_link_local_hosts() -> Vec<LinkLocalHostAddress> {
        let mut hosts = Vec::new();
        let mut ifaddrs_head: *mut libc::ifaddrs = ptr::null_mut();
        // SAFETY: getifaddrs allocates a linked list freed with freeifaddrs below.
        if unsafe { libc::getifaddrs(&mut ifaddrs_head) } != 0 {
            return hosts;
        }
        let mut cursor = ifaddrs_head;
        while !cursor.is_null() {
            // SAFETY: cursor walks the getifaddrs list until null.
            let interface = unsafe { &*cursor };
            let name = if interface.ifa_name.is_null() {
                None
            } else {
                // SAFETY: ifa_name is a C string owned by getifaddrs.
                unsafe { std::ffi::CStr::from_ptr(interface.ifa_name) }
                    .to_str()
                    .ok()
            };
            let Some(name) = name else {
                cursor = interface.ifa_next;
                continue;
            };
            let flags = interface.ifa_flags;
            let is_up = (flags & libc::IFF_UP as libc::c_uint) != 0;
            let is_running = (flags & libc::IFF_RUNNING as libc::c_uint) != 0;
            let is_loopback = (flags & libc::IFF_LOOPBACK as libc::c_uint) != 0;
            let is_point_to_point = (flags & libc::IFF_POINTOPOINT as libc::c_uint) != 0;
            let is_multicast = (flags & libc::IFF_MULTICAST as libc::c_uint) != 0;
            if !is_up
                || !is_running
                || is_loopback
                || is_point_to_point
                || !is_multicast
                || !interface_name_is_infrastructure(name)
            {
                cursor = interface.ifa_next;
                continue;
            }

            let addr = interface.ifa_addr;
            if !addr.is_null() {
                // SAFETY: ifa_addr is a sockaddr from getifaddrs when non-null.
                let family = unsafe { (*addr).sa_family };
                if family == libc::AF_INET6 as libc::sa_family_t {
                    // SAFETY: AF_INET6 sockaddr is sockaddr_in6-sized in this list.
                    let sockaddr_in6 = unsafe { &*(addr as *const libc::sockaddr_in6) };
                    let octets = sockaddr_in6.sin6_addr.s6_addr;
                    let ipv6 = Ipv6Addr::from(octets);
                    if ipv6.is_unicast_link_local() {
                        // SAFETY: if_nametoindex accepts the same C string getifaddrs provided.
                        let interface_index = unsafe { libc::if_nametoindex(interface.ifa_name) };
                        if interface_index != 0 {
                            hosts.push(LinkLocalHostAddress {
                                address: ipv6,
                                interface_index,
                            });
                        }
                    }
                }
            }
            cursor = interface.ifa_next;
        }
        // SAFETY: pairs with the successful getifaddrs above.
        unsafe { libc::freeifaddrs(ifaddrs_head) };
        hosts.sort_by(|left, right| {
            left.interface_index
                .cmp(&right.interface_index)
                .then_with(|| left.address.cmp(&right.address))
        });
        hosts.dedup();
        hosts
    }
}

fn browse_for_services(
    discovery_transport: DiscoveryTransport,
    browser_delegate: &BrowserDelegate,
) -> Retained<NSNetServiceBrowser> {
    let browser = NSNetServiceBrowser::new();
    let browser_protocol = ProtocolObject::from_ref(browser_delegate);
    // SAFETY: the browser and retained delegate remain on the native run-loop thread, and the
    // protocol object has NSNetServiceBrowserDelegate's runtime type.
    unsafe { browser.setDelegate(Some(browser_protocol)) };
    browser.setIncludesPeerToPeer(true);
    browser.searchForServicesOfType_inDomain(
        &NSString::from_str(&apple_service_type(discovery_transport)),
        &NSString::from_str(DNS_SD_LOCAL_DOMAIN),
    );
    browser
}

async fn wait_for_publication(
    publication_outcome: oneshot::Receiver<PublicationOutcome>,
) -> Result<(), MdnsError> {
    match publication_outcome.await {
        Ok(PublicationOutcome::Published) => Ok(()),
        Ok(PublicationOutcome::Rejected(error)) => Err(error),
        Err(_native_thread_closed) => Err(MdnsError::Closed),
    }
}

async fn wait_for_publications(
    tcp_publication_outcome: oneshot::Receiver<PublicationOutcome>,
    udp_publication_outcome: oneshot::Receiver<PublicationOutcome>,
) -> Result<(), MdnsError> {
    wait_for_publication(tcp_publication_outcome).await?;
    wait_for_publication(udp_publication_outcome).await
}

impl AppleServiceDiscoveryBackend {
    pub async fn new(
        tcp_instance_name: EphemeralDiscoveryInstanceName,
        udp_instance_name: EphemeralDiscoveryInstanceName,
    ) -> Result<Self, MdnsError> {
        let tcp_publication = ApplePublication::new(DiscoveryTransport::Tcp, tcp_instance_name)?;
        let udp_publication = ApplePublication::new(DiscoveryTransport::Udp, udp_instance_name)?;
        let local_service_names = Arc::new(BTreeSet::from([
            tcp_publication.service_name.clone(),
            udp_publication.service_name.clone(),
        ]));
        let (tcp_ready_sender, tcp_ready_receiver) = oneshot::channel::<PublicationOutcome>();
        let (udp_ready_sender, udp_ready_receiver) = oneshot::channel::<PublicationOutcome>();
        let (snapshot_state, snapshot_receiver) = SnapshotState::channel(DISCOVERY_CAPACITY);
        let (shutdown_tx, shutdown_rx) = sync_mpsc::channel::<()>();
        let backend_failed = Arc::new(AtomicBool::new(false));

        let join = thread::Builder::new()
            .name("hopspot-mdns".into())
            .spawn(move || {
                log_main_bundle_privacy_keys();
                trigger_local_network_privacy_alert();
                let publisher = match dns_sd_ll::LinkLocalPublisher::advertise(
                    tcp_publication.instance_name.as_str(),
                    tcp_publication.instance_name.as_str(),
                    udp_publication.instance_name.as_str(),
                    tcp_publication.transport.port(),
                    udp_publication.transport.port(),
                ) {
                    Ok(publisher) => {
                        let _ = tcp_ready_sender.send(PublicationOutcome::Published);
                        let _ = udp_ready_sender.send(PublicationOutcome::Published);
                        Some(publisher)
                    }
                    Err(publication_error) => {
                        backend_failed.store(true, Ordering::Release);
                        let _ =
                            tcp_ready_sender.send(PublicationOutcome::Rejected(publication_error));
                        let _ =
                            udp_ready_sender.send(PublicationOutcome::Rejected(publication_error));
                        None
                    }
                };

                let resolver = ResolveDelegate::new(Arc::clone(&snapshot_state));
                let browser_delegate = BrowserDelegate::new(
                    resolver,
                    snapshot_state,
                    local_service_names,
                    Arc::clone(&backend_failed),
                );
                let tcp_browser = browse_for_services(DiscoveryTransport::Tcp, &browser_delegate);
                let udp_browser = browse_for_services(DiscoveryTransport::Udp, &browser_delegate);

                while !backend_failed.load(Ordering::Acquire)
                    && matches!(shutdown_rx.try_recv(), Err(sync_mpsc::TryRecvError::Empty))
                {
                    if let Some(publisher) = publisher.as_ref() {
                        publisher.process_pending();
                    }
                    // SAFETY: the process-global default run-loop mode has static lifetime.
                    let mode = unsafe { kCFRunLoopDefaultMode };
                    let _ = CFRunLoop::run_in_mode(mode, 0.1, false);
                }
                tcp_browser.stop();
                udp_browser.stop();
                drop((publisher, tcp_browser, udp_browser, browser_delegate));
            })
            .map_err(|_| MdnsError::Closed)?;
        let native_thread = NativeMdnsThread {
            shutdown: Some(shutdown_tx),
            join: Some(join),
        };

        let publication_result = tokio::time::timeout(
            PUBLISH_TIMEOUT,
            wait_for_publications(tcp_ready_receiver, udp_ready_receiver),
        )
        .await;
        match publication_result {
            Ok(Ok(())) => {
                crate::diagnostic_log::debug!(
                    "mdns: advertising LL-only + browsing {} on port {} and {} on port {}",
                    TCP_DNS_SD_BASE_SERVICE_TYPE,
                    TCP_RENDEZVOUS_PORT,
                    UDP_DNS_SD_BASE_SERVICE_TYPE,
                    UNICAST_DISCOVERY_PORT,
                );
                Ok(Self {
                    snapshot_receiver,
                    _native_thread: native_thread,
                })
            }
            Ok(Err(publication_error)) => Err(publication_error),
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
    CandidateTransport(CandidateInsertionError),
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
    let discovery_transport = service_name.transport();
    if u16::try_from(service.port()) != Ok(discovery_transport.port()) {
        return Err(ServiceRecordRejection::WrongPort);
    }
    discovery_version(service).map_err(ServiceRecordRejection::InvalidVersion)?;

    let mut discovery_endpoints = BTreeSet::new();
    for address in addresses.iter() {
        let Some(socket_address) = parse_sockaddr(&address.to_vec()) else {
            continue;
        };
        let SocketAddr::V6(ipv6_socket_address) = socket_address else {
            continue;
        };
        if !ipv6_socket_address.ip().is_unicast_link_local() {
            continue;
        }
        if let Ok(discovery_endpoint) =
            DiscoveryEndpoint::try_from((discovery_transport, socket_address))
        {
            discovery_endpoints.insert(discovery_endpoint);
        }
    }

    let mut service_advertisement = ServiceAdvertisement::new(service_name);
    for discovery_endpoint in discovery_endpoints {
        match service_advertisement.insert(discovery_endpoint) {
            Ok(CandidateInsertion::RejectedLowerPriority) => break,
            Ok(
                CandidateInsertion::Inserted
                | CandidateInsertion::AlreadyPresent
                | CandidateInsertion::ReplacedLowerPriority,
            ) => {}
            Err(candidate_error) => {
                return Err(ServiceRecordRejection::CandidateTransport(candidate_error));
            }
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
    let discovery_transport = match classify_apple_service_type(&service.r#type().to_string()) {
        AppleServiceTypeClassification::Supported(discovery_transport) => discovery_transport,
        AppleServiceTypeClassification::Unsupported => {
            return Err(ServiceRecordRejection::WrongServiceType);
        }
    };
    if !service
        .domain()
        .to_string()
        .eq_ignore_ascii_case(DNS_SD_LOCAL_DOMAIN)
    {
        return Err(ServiceRecordRejection::WrongDomain);
    }
    DiscoveryServiceName::from_instance(&service.name().to_string(), discovery_transport)
        .map_err(ServiceRecordRejection::InvalidServiceName)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AppleServiceTypeClassification {
    Supported(DiscoveryTransport),
    Unsupported,
}

fn classify_apple_service_type(service_type: &str) -> AppleServiceTypeClassification {
    for discovery_transport in [DiscoveryTransport::Tcp, DiscoveryTransport::Udp] {
        if service_type.eq_ignore_ascii_case(&apple_service_type(discovery_transport)) {
            return AppleServiceTypeClassification::Supported(discovery_transport);
        }
    }
    AppleServiceTypeClassification::Unsupported
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
    const SOCKADDR_V6_LENGTH: u8 = 28;

    fn apple_service(
        discovery_transport: DiscoveryTransport,
        instance_name: &str,
        port: u16,
    ) -> Retained<NSNetService> {
        NSNetService::initWithDomain_type_name_port(
            NSNetService::alloc(),
            &NSString::from_str(DNS_SD_LOCAL_DOMAIN),
            &NSString::from_str(&apple_service_type(discovery_transport)),
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

    fn ipv6_sockaddr_data(ip_address: Ipv6Addr, port: u16, scope_id: u32) -> Retained<NSData> {
        let mut bytes = vec![0u8; usize::from(SOCKADDR_V6_LENGTH)];
        bytes[0] = SOCKADDR_V6_LENGTH;
        bytes[1] = AF_INET6;
        bytes[2..4].copy_from_slice(&port.to_be_bytes());
        bytes[8..24].copy_from_slice(&ip_address.octets());
        bytes[24..28].copy_from_slice(&scope_id.to_ne_bytes());
        NSData::with_bytes(&bytes)
    }

    fn service_advertisement(
        discovery_transport: DiscoveryTransport,
        service_name: &str,
        socket_address: &str,
    ) -> Result<ServiceAdvertisement, Box<dyn std::error::Error>> {
        let discovery_service_name =
            DiscoveryServiceName::from_instance(service_name, discovery_transport)?;
        let discovery_endpoint =
            DiscoveryEndpoint::try_from((discovery_transport, socket_address.parse()?))?;
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
    fn apple_publications_use_independent_shared_names_and_transport_contracts(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let tcp_instance_name = EphemeralDiscoveryInstanceName::from_random_bytes(
            [0x11; EPHEMERAL_DISCOVERY_INSTANCE_RANDOM_BYTES],
        );
        let udp_instance_name = EphemeralDiscoveryInstanceName::from_random_bytes(
            [0x22; EPHEMERAL_DISCOVERY_INSTANCE_RANDOM_BYTES],
        );
        let tcp_publication = ApplePublication::new(DiscoveryTransport::Tcp, tcp_instance_name)?;
        let udp_publication = ApplePublication::new(DiscoveryTransport::Udp, udp_instance_name)?;

        assert_ne!(
            tcp_publication.instance_name.as_str(),
            udp_publication.instance_name.as_str()
        );
        assert_eq!(
            (
                apple_service_type(tcp_publication.transport),
                tcp_publication.transport.port(),
                tcp_publication.service_name.transport(),
            ),
            (
                format!("{TCP_DNS_SD_BASE_SERVICE_TYPE}."),
                TCP_RENDEZVOUS_PORT,
                DiscoveryTransport::Tcp,
            )
        );
        assert_eq!(
            (
                apple_service_type(udp_publication.transport),
                udp_publication.transport.port(),
                udp_publication.service_name.transport(),
            ),
            (
                format!("{UDP_DNS_SD_BASE_SERVICE_TYPE}."),
                UNICAST_DISCOVERY_PORT,
                DiscoveryTransport::Udp,
            )
        );
        Ok(())
    }

    #[tokio::test]
    async fn backend_readiness_requires_both_publications() {
        let (tcp_ready_sender, tcp_ready_receiver) = oneshot::channel();
        let (udp_ready_sender, udp_ready_receiver) = oneshot::channel();
        assert_eq!(tcp_ready_sender.send(PublicationOutcome::Published), Ok(()));
        assert_eq!(udp_ready_sender.send(PublicationOutcome::Published), Ok(()));
        assert_eq!(
            wait_for_publications(tcp_ready_receiver, udp_ready_receiver).await,
            Ok(())
        );

        let (tcp_ready_sender, tcp_ready_receiver) = oneshot::channel();
        let (udp_ready_sender, udp_ready_receiver) = oneshot::channel();
        assert_eq!(tcp_ready_sender.send(PublicationOutcome::Published), Ok(()));
        assert_eq!(
            udp_ready_sender.send(PublicationOutcome::Rejected(MdnsError::PublishFailed)),
            Ok(())
        );
        assert_eq!(
            wait_for_publications(tcp_ready_receiver, udp_ready_receiver).await,
            Err(MdnsError::PublishFailed)
        );

        let (tcp_ready_sender, tcp_ready_receiver) = oneshot::channel();
        let (udp_ready_sender, udp_ready_receiver) = oneshot::channel();
        assert_eq!(
            tcp_ready_sender.send(PublicationOutcome::Rejected(MdnsError::ServiceUnavailable)),
            Ok(())
        );
        assert_eq!(udp_ready_sender.send(PublicationOutcome::Published), Ok(()));
        assert_eq!(
            wait_for_publications(tcp_ready_receiver, udp_ready_receiver).await,
            Err(MdnsError::ServiceUnavailable)
        );
    }

    #[test]
    fn tcp_and_udp_resolutions_share_one_snapshot_and_remove_independently(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let discovery_capacity = NonZeroU8::new(2).ok_or("invalid test capacity")?;
        let (snapshot_state, snapshot_receiver) = SnapshotState::channel(discovery_capacity);
        let tcp_service = apple_service(DiscoveryTransport::Tcp, "tcp-peer", TCP_RENDEZVOUS_PORT);
        let udp_service =
            apple_service(DiscoveryTransport::Udp, "udp-peer", UNICAST_DISCOVERY_PORT);
        let tcp_addresses = NSArray::from_retained_slice(&[
            ipv4_sockaddr_data("192.168.1.8".parse()?, TCP_RENDEZVOUS_PORT),
            ipv6_sockaddr_data("fe80::8".parse()?, TCP_RENDEZVOUS_PORT, 7),
        ]);
        let udp_addresses = NSArray::from_retained_slice(&[ipv6_sockaddr_data(
            "fe80::8".parse()?,
            UNICAST_DISCOVERY_PORT,
            7,
        )]);

        assert_eq!(
            apply_resolved_service(&snapshot_state, &tcp_service, &tcp_addresses),
            ServiceResolutionOutcome::SnapshotChanged
        );
        assert_eq!(
            apply_resolved_service(&snapshot_state, &udp_service, &udp_addresses),
            ServiceResolutionOutcome::SnapshotChanged
        );

        let tcp_service_name =
            DiscoveryServiceName::from_instance("tcp-peer", DiscoveryTransport::Tcp)?;
        let udp_service_name =
            DiscoveryServiceName::from_instance("udp-peer", DiscoveryTransport::Udp)?;
        let snapshot = snapshot_receiver.borrow().clone();
        assert_eq!(snapshot.len(), 2);
        assert_eq!(
            snapshot
                .get(&tcp_service_name)
                .map(ServiceAdvertisement::endpoints),
            Some(&[DiscoveryEndpoint::tcp("[fe80::8%7]:42699".parse()?)?][..])
        );
        assert_eq!(
            snapshot
                .get(&udp_service_name)
                .map(ServiceAdvertisement::endpoints),
            Some(&[DiscoveryEndpoint::udp("[fe80::8%7]:29717".parse()?)?][..])
        );

        assert_eq!(
            snapshot_state.remove(&udp_service_name),
            SnapshotRemoval::Changed
        );
        assert_eq!(snapshot_receiver.borrow().len(), 1);
        assert!(snapshot_receiver.borrow().get(&tcp_service_name).is_some());
        Ok(())
    }

    #[test]
    fn snapshot_state_is_bounded_and_known_service_updates_win(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let (snapshot_state, snapshot_receiver) = SnapshotState::channel(NonZeroU8::MIN);
        let initial = service_advertisement(DiscoveryTransport::Tcp, "first", "192.168.1.2:42699")?;
        assert_eq!(
            snapshot_state.resolve(initial.clone()),
            SnapshotResolution::Changed
        );
        assert_eq!(
            snapshot_state.resolve(initial),
            SnapshotResolution::Unchanged
        );
        assert_eq!(
            snapshot_state.resolve(service_advertisement(
                DiscoveryTransport::Udp,
                "overflow",
                "[fe80::3%7]:29717",
            )?),
            SnapshotResolution::RejectedAtCapacity
        );

        let replacement =
            service_advertisement(DiscoveryTransport::Tcp, "first", "192.168.1.4:42699")?;
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
        let service = apple_service(DiscoveryTransport::Tcp, "peer", TCP_RENDEZVOUS_PORT);
        let addresses = NSArray::from_retained_slice(&[
            ipv4_sockaddr_data("192.168.1.8".parse()?, TCP_RENDEZVOUS_PORT),
            ipv6_sockaddr_data("fe80::8".parse()?, TCP_RENDEZVOUS_PORT, 7),
            ipv6_sockaddr_data("fd00::8".parse()?, TCP_RENDEZVOUS_PORT, 0),
        ]);

        assert_eq!(
            apply_resolved_service(&snapshot_state, &service, &addresses),
            ServiceResolutionOutcome::SnapshotChanged
        );
        let mut expected_advertisement = ServiceAdvertisement::new(
            DiscoveryServiceName::from_instance("peer", DiscoveryTransport::Tcp)?,
        );
        assert_eq!(
            expected_advertisement.insert(DiscoveryEndpoint::tcp("[fe80::8%7]:42699".parse()?,)?),
            Ok(CandidateInsertion::Inserted)
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

    #[test]
    fn link_local_publisher_skips_peer_to_peer_and_tunnel_interfaces() {
        assert!(super::dns_sd_ll::interface_name_is_infrastructure("en0"));
        assert!(super::dns_sd_ll::interface_name_is_infrastructure("en1"));
        assert!(!super::dns_sd_ll::interface_name_is_infrastructure("awdl0"));
        assert!(!super::dns_sd_ll::interface_name_is_infrastructure("llw0"));
        assert!(!super::dns_sd_ll::interface_name_is_infrastructure("utun5"));
        assert!(!super::dns_sd_ll::interface_name_is_infrastructure(
            "bridge0"
        ));
        assert!(!super::dns_sd_ll::interface_name_is_infrastructure("ap1"));
    }

    #[test]
    fn link_local_record_flags_are_valid_for_a_connected_dns_sd_ref() {
        assert_eq!(super::dns_sd_ll::record_registration_flags(0), 0x20);
        assert_eq!(super::dns_sd_ll::record_registration_flags(1), 0x10);
        assert_eq!(super::dns_sd_ll::record_registration_flags(0) & 0x4000, 0);
        assert_eq!(super::dns_sd_ll::record_registration_flags(1) & 0x4000, 0);
    }

    #[test]
    fn link_local_publisher_names_daemon_unavailability_as_environmental() {
        if super::dns_sd_ll::local_unicast_link_local_hosts().is_empty() {
            return;
        }
        let publisher = super::dns_sd_ll::LinkLocalPublisher::advertise(
            "prns-llpubtest",
            "prns-llpubtest-tcp",
            "prns-llpubtest-udp",
            TCP_RENDEZVOUS_PORT,
            UNICAST_DISCOVERY_PORT,
        );
        match publisher {
            Ok(_) | Err(MdnsError::ServiceUnavailable) => {}
            Err(error) => panic!(
                "expected DNS-SD LL publish to succeed with a non-null RegisterRecord callback: {error:?}"
            ),
        }
    }
}
