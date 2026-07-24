use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;

use crate::identity::IdentityHash;
use crate::routing::links::resources::{ResourceCompression, ResourceOffer};
use crate::routing::links::LinkId;

use super::PrnsNodeHandle;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceAdmissionPeer {
    Any,
    Authenticated(IdentityHash),
    AuthenticatedOneOf(Arc<[IdentityHash]>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceOfferAdmission {
    pub peer: ResourceAdmissionPeer,
    pub max_uncompressed_bytes: u64,
    pub accept_compressed: bool,
}

pub struct ResourceOfferMonitor {
    offers: mpsc::UnboundedReceiver<ResourceOffer>,
}

impl ResourceOfferMonitor {
    pub async fn recv(&mut self) -> Option<ResourceOffer> {
        self.offers.recv().await
    }
}

struct AdmissionEntry {
    rule: ResourceOfferAdmission,
    offers: mpsc::UnboundedSender<ResourceOffer>,
}

#[derive(Clone, Default)]
pub(crate) struct ResourceAdmissionRegistry {
    entries: Arc<Mutex<HashMap<LinkId, AdmissionEntry>>>,
}

impl ResourceAdmissionRegistry {
    pub(crate) fn install(
        &self,
        link_id: LinkId,
        rule: ResourceOfferAdmission,
    ) -> ResourceOfferMonitor {
        let (offers, receiver) = mpsc::unbounded_channel();
        if let Ok(mut entries) = self.entries.lock() {
            entries.insert(link_id, AdmissionEntry { rule, offers });
        }
        ResourceOfferMonitor { offers: receiver }
    }

    pub(crate) fn remove(&self, link_id: LinkId) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.remove(&link_id);
        }
    }

    pub(crate) fn permits(&self, offer: &ResourceOffer) -> bool {
        let Ok(entries) = self.entries.lock() else {
            return false;
        };
        let Some(entry) = entries.get(&offer.link_id) else {
            return false;
        };
        if offer.uncompressed_data_bytes > entry.rule.max_uncompressed_bytes {
            return false;
        }
        if !entry.rule.accept_compressed && offer.compression == ResourceCompression::Bz2 {
            return false;
        }
        let peer_allowed = match &entry.rule.peer {
            ResourceAdmissionPeer::Any => true,
            ResourceAdmissionPeer::Authenticated(expected) => {
                offer.remote_identity == Some(*expected)
            }
            ResourceAdmissionPeer::AuthenticatedOneOf(expected) => offer
                .remote_identity
                .is_some_and(|identity| expected.contains(&identity)),
        };
        if peer_allowed {
            let _ = entry.offers.send(*offer);
        }
        peer_allowed
    }
}

impl PrnsNodeHandle {
    pub fn admit_resource_offers(
        &self,
        link_id: LinkId,
        admission: ResourceOfferAdmission,
    ) -> ResourceOfferMonitor {
        self.resource_admission.install(link_id, admission)
    }

    pub fn deny_resource_offers(&self, link_id: LinkId) {
        self.resource_admission.remove(link_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::links::resources::{ResourceHash, ResourceOffer};

    const LINK: LinkId = LinkId::new([0x11; 16]);
    const PEER: IdentityHash = IdentityHash::new([0x22; 16]);

    fn offer(identity: Option<IdentityHash>) -> ResourceOffer {
        ResourceOffer {
            link_id: LINK,
            remote_identity: identity,
            hash: ResourceHash::new([0x33; 32]),
            uncompressed_data_bytes: 1024,
            sealed_transfer_bytes: 900,
            part_count: 2,
            segment_index: 1,
            total_segment_count: 1,
            compression: ResourceCompression::Bz2,
            has_metadata: true,
        }
    }

    #[tokio::test]
    async fn authenticated_admission_is_per_link_identity_size_and_compression() {
        let registry = ResourceAdmissionRegistry::default();
        let mut monitor = registry.install(
            LINK,
            ResourceOfferAdmission {
                peer: ResourceAdmissionPeer::Authenticated(PEER),
                max_uncompressed_bytes: 2048,
                accept_compressed: true,
            },
        );
        assert!(!registry.permits(&offer(None)));
        assert!(!registry.permits(&offer(Some(IdentityHash::new([0x44; 16])))));
        let accepted = offer(Some(PEER));
        assert!(registry.permits(&accepted));
        assert_eq!(monitor.recv().await, Some(accepted));
        registry.remove(LINK);
        assert!(!registry.permits(&accepted));
    }
}
