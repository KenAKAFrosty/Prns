#![expect(
    clippy::panic,
    reason = "qualification failures persist artifacts before failing"
)]
#![expect(
    clippy::expect_used,
    reason = "bounded deterministic qualification fixtures"
)]
pub(crate) mod adapter;
mod campaign;
pub(crate) mod node;
pub(crate) mod pairing;
pub(crate) mod persistence;
mod tests;

use personal_rns::identity::vault::IdentitySecretKey;
use personal_rns::remote_control::*;
use personal_rns::runtime::{RemoteControlAppMessages, RemoteControlHostCommandError};
use std::cell::RefCell;
use std::rc::Rc;

const CONTROLLER: usize = 0;
const TARGET: usize = 1;
const REQUEST_TIMEOUT_MS: u64 = 50;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Invocation {
    pub identity: personal_rns::identity::IdentityHash,
    pub payload: Vec<u8>,
}
#[derive(Clone)]
pub(crate) struct Messages(
    pub Rc<RefCell<Vec<Invocation>>>,
    pub Rc<RefCell<Vec<pairing::Observation>>>,
);
impl RemoteControlAppMessages<()> for Messages {
    async fn handle_app_message(
        &self,
        _: &(),
        identity: personal_rns::identity::IdentityHash,
        payload: &[u8],
    ) -> Result<RemoteControlAppMessage, RemoteControlHostCommandError> {
        let mut calls = self.0.borrow_mut();
        assert!(calls.len() < 512, "bounded app ledger");
        calls.push(Invocation {
            identity,
            payload: payload.to_vec(),
        });
        if payload.first() == Some(&0xff) {
            return Err(RemoteControlHostCommandError::ApplyFailed);
        }
        RemoteControlAppMessage::from_slice(payload)
            .map_err(|_| RemoteControlHostCommandError::ApplyFailed)
    }
}
pub(crate) fn secrets(index: usize) -> RemoteControlNodeIdentitySecrets {
    let seed = 0x61 + index as u8 * 2;
    RemoteControlNodeIdentitySecrets::new(
        RemoteControlControllerIdentitySecret::from(IdentitySecretKey::new([seed; 64])),
        RemoteControlTargetIdentitySecret::from(IdentitySecretKey::new([seed + 1; 64])),
    )
    .expect("distinct identities")
}
pub(crate) fn requests() -> RemoteControlRequestSet {
    let mut requests = RemoteControlRequestSet::empty();
    for request in [
        RemoteControlRequestKind::Describe,
        RemoteControlRequestKind::DescribeBuild,
        RemoteControlRequestKind::InventoryInterfaces,
        RemoteControlRequestKind::InventoryInterfaceConfig,
        RemoteControlRequestKind::InventoryInterfacePeers,
        RemoteControlRequestKind::InventoryControllers,
        RemoteControlRequestKind::AuthorizeController,
        RemoteControlRequestKind::RevokeController,
        RemoteControlRequestKind::AppMessage,
        RemoteControlRequestKind::WatchInterfaces,
    ] {
        requests.insert(request);
    }
    requests
}
fn service(
    index: usize,
    initial: RemoteControlInitialControllerGrants<'_>,
) -> RemoteControlService<'_> {
    RemoteControlService::with_capabilities(
        secrets(index),
        initial,
        RemoteControlSelfAnnouncement::Unavailable,
        RemoteControlCapabilities::from_requests(requests()).expect("Describe supported"),
    )
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct WireValue {
    connection: usize,
    from: [u8; 6],
    to: [u8; 6],
    channel: String,
    bytes: Vec<u8>,
}
