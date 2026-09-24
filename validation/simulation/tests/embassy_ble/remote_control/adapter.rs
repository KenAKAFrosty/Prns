use super::*;
use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, RequestResponseTimeout, SendRequestFailure,
};
use personal_rns::routing::links::LinkId;
use personal_rns::routing::request_handlers::RequestPathHash;
use personal_rns::runtime::*;
use personal_rns::units::DurationMillis;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Runtime {
    Tokio,
    Embassy,
}
pub const PAIRS: [[Runtime; 2]; 4] = [
    [Runtime::Tokio, Runtime::Tokio],
    [Runtime::Tokio, Runtime::Embassy],
    [Runtime::Embassy, Runtime::Tokio],
    [Runtime::Embassy, Runtime::Embassy],
];
#[derive(Clone)]
pub enum Handle {
    Tokio(PrnsNodeHandle),
    Embassy(super::node::EmbeddedHandle),
}
impl Handle {
    pub async fn grant(
        &self,
        grant: RemoteControlControllerGrant,
    ) -> Result<SetRemoteControlControllerGrantOutcome, SetRemoteControlControllerGrantControlError>
    {
        match self {
            Self::Tokio(h) => h.set_remote_control_controller_grant(grant).await,
            Self::Embassy(h) => h.set_remote_control_controller_grant(grant).await,
        }
    }
    pub async fn revoke(
        &self,
        controller: RemoteControlControllerIdentity,
    ) -> Result<RevokeRemoteControlControllerOutcome, RevokeRemoteControlControllerControlError>
    {
        match self {
            Self::Tokio(h) => h.revoke_remote_control_controller(controller).await,
            Self::Embassy(h) => h.revoke_remote_control_controller(controller).await,
        }
    }
    pub async fn announce(&self) {
        let command = AnnounceNow {
            destination: secrets(TARGET)
                .identities()
                .target()
                .endpoint()
                .destination_hash(),
            target: AnnounceTarget::AllInterfaces,
            app_data: AnnounceAppData::Registered,
        };
        match self {
            Self::Tokio(h) => h
                .announce_now(command)
                .await
                .expect("explicit target discovery"),
            Self::Embassy(h) => h
                .announce_now(command)
                .await
                .expect("explicit target discovery"),
        }
    }
    pub async fn connect(&self) -> LinkId {
        let identity = secrets(TARGET).identities().target().identity_hash();
        match self {
            Self::Tokio(h) => h
                .connect_remote_control_target(identity)
                .await
                .expect("pinned connect")
                .connection()
                .link_id(),
            Self::Embassy(h) => h
                .connect_remote_control_target(identity)
                .await
                .expect("pinned connect")
                .connection()
                .link_id(),
        }
    }
    pub fn close(&self, link: LinkId) {
        let queued = match self {
            Self::Tokio(h) => h.close_link(link),
            Self::Embassy(h) => h.close_link(link),
        };
        assert!(queued, "close live control link");
    }
    pub async fn raw(
        &self,
        link: LinkId,
        bytes: Vec<u8>,
    ) -> Result<Vec<u8>, SendError<SendRequestFailure>> {
        let path = RequestPathHash::of(REMOTE_CONTROL_REQUEST_ENDPOINT_ID);
        let timeout = RequestResponseTimeout::Exact(DurationMillis(REQUEST_TIMEOUT_MS));
        match self {
            Self::Tokio(h) => h
                .request_with_response_timeout(link, path, &bytes, timeout)
                .await
                .map(|(bytes, _)| bytes),
            Self::Embassy(h) => h
                .request_with_response_timeout(link, path, &bytes, timeout)
                .await
                .map(|(bytes, _)| bytes.as_slice().to_vec()),
        }
    }
}
