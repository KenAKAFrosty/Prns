#![allow(clippy::undocumented_unsafe_blocks)]
#![allow(deprecated)]

use core::cell::RefCell;
use core::time::Duration;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV6};
use std::string::String;
use std::thread;
use std::vec::Vec;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{define_class, msg_send, AllocAnyThread, DefinedClass, Message};
use objc2_core_foundation::CFRunLoop;
use objc2_foundation::{
    NSArray, NSData, NSDictionary, NSNetService, NSNetServiceBrowser, NSNetServiceBrowserDelegate,
    NSNetServiceDelegate, NSNumber, NSObject, NSObjectProtocol, NSString,
};
use tokio::sync::{mpsc as tokio_mpsc, oneshot};

const SERVICE_TYPE: &str = "_reticulum._tcp.";
const DOMAIN: &str = "local.";
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

struct AdvertiserDelegateIvars {
    ready: RefCell<Option<oneshot::Sender<bool>>>,
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
                let _ = ready.send(true);
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
                let _ = ready.send(false);
            }
        }
    }
);

impl AdvertiserDelegate {
    fn new(ready: oneshot::Sender<bool>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(AdvertiserDelegateIvars {
            ready: RefCell::new(Some(ready)),
        });
        unsafe { msg_send![super(this), init] }
    }
}

struct ResolveDelegateIvars {
    sightings: tokio_mpsc::UnboundedSender<SocketAddr>,
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
            if let Some(addr) = select_sighting(&addresses) {
                let _ = self.ivars().sightings.send(addr);
            }
        }

        #[unsafe(method(netService:didNotResolve:))]
        fn did_not_resolve(
            &self,
            _sender: &NSNetService,
            error: &NSDictionary<NSString, NSNumber>,
        ) {
            crate::diagnostic_log::debug!("mdns: resolve failed: {error:?}");
        }
    }
);

impl ResolveDelegate {
    fn new(sightings: tokio_mpsc::UnboundedSender<SocketAddr>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(ResolveDelegateIvars { sightings });
        unsafe { msg_send![super(this), init] }
    }
}

struct BrowserDelegateIvars {
    resolver: Retained<ResolveDelegate>,
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
            let resolver: &ResolveDelegate = &self.ivars().resolver;
            let proto = ProtocolObject::from_ref(resolver);
            unsafe { service.setDelegate(Some(proto)) };
            service.resolveWithTimeout(RESOLVE_TIMEOUT);
            self.ivars().resolving.borrow_mut().push(service.retain());
        }

        #[unsafe(method(netServiceBrowser:didRemoveService:moreComing:))]
        fn did_remove(
            &self,
            _browser: &NSNetServiceBrowser,
            _service: &NSNetService,
            _more_coming: bool,
        ) {
        }
    }
);

impl BrowserDelegate {
    fn new(resolver: Retained<ResolveDelegate>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(BrowserDelegateIvars {
            resolver,
            resolving: RefCell::new(Vec::new()),
        });
        unsafe { msg_send![super(this), init] }
    }
}

/// Advertises our TCP rendezvous as a `_reticulum._tcp` Bonjour service AND browses for peers,
/// surfacing each resolved peer's rendezvous endpoint through [`next_sighting`](Self::next_sighting).
/// Standard Bonjour rides the system mDNSResponder, so this is free-team (Local Network permission +
/// the `NSBonjourServices` declaration), never the multicast entitlement. Lives on its own thread
/// running a `CFRunLoop`, since `NSNetService` is run-loop affine.
pub struct MacosMdnsBackend {
    sightings: tokio_mpsc::UnboundedReceiver<SocketAddr>,
}

impl MacosMdnsBackend {
    pub async fn new(port: u16, txt: &[(&str, &[u8])]) -> Result<Self, MdnsError> {
        let (ready_tx, ready_rx) = oneshot::channel::<bool>();
        let (sightings_tx, sightings_rx) = tokio_mpsc::unbounded_channel::<SocketAddr>();
        let port = core::ffi::c_int::from(port);
        let txt: Vec<(String, Vec<u8>)> = txt
            .iter()
            .map(|(key, value)| (String::from(*key), value.to_vec()))
            .collect();

        thread::Builder::new()
            .name("hopspot-mdns".into())
            .spawn(move || {
                let advertiser = AdvertiserDelegate::new(ready_tx);
                let advertiser_proto = ProtocolObject::from_ref(&*advertiser);
                let service = NSNetService::initWithDomain_type_name_port(
                    NSNetService::alloc(),
                    &NSString::from_str(DOMAIN),
                    &NSString::from_str(SERVICE_TYPE),
                    &NSString::from_str(""),
                    port,
                );
                unsafe { service.setDelegate(Some(advertiser_proto)) };
                service.setIncludesPeerToPeer(true);
                if let Some(data) = build_txt(&txt) {
                    service.setTXTRecordData(Some(&data));
                }
                service.publish();

                let resolver = ResolveDelegate::new(sightings_tx);
                let browser_delegate = BrowserDelegate::new(resolver);
                let browser_proto = ProtocolObject::from_ref(&*browser_delegate);
                let browser = NSNetServiceBrowser::new();
                unsafe { browser.setDelegate(Some(browser_proto)) };
                browser.setIncludesPeerToPeer(true);
                browser.searchForServicesOfType_inDomain(
                    &NSString::from_str(SERVICE_TYPE),
                    &NSString::from_str(DOMAIN),
                );

                CFRunLoop::run();
                drop((service, advertiser, browser, browser_delegate));
            })
            .map_err(|_| MdnsError::Closed)?;

        match tokio::time::timeout(PUBLISH_TIMEOUT, ready_rx).await {
            Ok(Ok(true)) => {
                crate::diagnostic_log::debug!(
                    "mdns: advertising + browsing {SERVICE_TYPE} on port {port}"
                );
                Ok(Self {
                    sightings: sightings_rx,
                })
            }
            Ok(Ok(false)) => Err(MdnsError::PublishFailed),
            Ok(Err(_)) => Err(MdnsError::Closed),
            Err(_) => Err(MdnsError::PublishTimeout),
        }
    }

    /// The next discovered peer's rendezvous endpoint, resolved to a concrete address. `None` once the
    /// browse thread is gone.
    pub async fn next_sighting(&mut self) -> Option<SocketAddr> {
        self.sightings.recv().await
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

/// Pick the best of a resolved service's addresses to dial: a routable address (IPv4, or a global
/// IPv6) is preferred over a link-local one, since a link-local must be dialed with the right
/// interface scope id and only reaches an AWDL/same-link peer. The link-local — now carrying its
/// `sin6_scope_id` from [`parse_sockaddr`] — is kept as the fallback for exactly the router-free
/// hotspot case where it is all Bonjour has.
fn select_sighting(addresses: &NSArray<NSData>) -> Option<SocketAddr> {
    let mut link_local: Option<SocketAddr> = None;
    for address in addresses.iter() {
        let Some(addr) = parse_sockaddr(&address.to_vec()) else {
            continue;
        };
        if is_link_local(addr.ip()) {
            link_local.get_or_insert(addr);
        } else {
            return Some(addr);
        }
    }
    link_local
}

fn is_link_local(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_link_local(),
        IpAddr::V6(v6) => (v6.segments()[0] & 0xffc0) == 0xfe80,
    }
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
