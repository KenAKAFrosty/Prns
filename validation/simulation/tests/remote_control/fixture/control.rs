use super::*;
use personal_rns::engine::RequestResponseTimeout;
use personal_rns::routing::request_handlers::RequestPathHash;
use personal_rns::runtime::{RemoteControlTargetAccessControl, RequestOptions};
use personal_rns::units::{ByteLimit, DurationMillis};

const BOOTSTRAP_BUDGET_MS: usize = 1_000;

impl Lab<'_> {
    pub fn link(&mut self, controller: usize) -> LinkId {
        self.link_with_identity(controller, LinkIdentity::Controller)
    }
    pub fn link_with_identity(&mut self, controller: usize, identity: LinkIdentity) -> LinkId {
        self.connect(controller, identity, TargetProvisioning::Explicit)
    }
    pub fn link_pinned(&mut self, controller: usize) -> LinkId {
        self.connect(
            controller,
            LinkIdentity::Controller,
            TargetProvisioning::Persisted,
        )
    }
    fn connect(
        &mut self,
        controller: usize,
        identity: LinkIdentity,
        provisioning: TargetProvisioning,
    ) -> LinkId {
        let handle = self.nodes[controller].handle.clone();
        let target = RemoteControlTargetIdentity::new(*self.nodes[TARGET].target.public_keys());
        let task = self.insert(async move {
            let endpoint = target.endpoint();
            let target_hash = target.identity_hash();
            if matches!(provisioning, TargetProvisioning::Explicit) {
                let access = RemoteControlTargetAccess::new(
                    target,
                    RemoteControlControllerAuthority::Operator,
                    requests(),
                )
                .expect("pinned target access");
                handle
                    .set_remote_control_target_access(access)
                    .await
                    .expect("local target provisioning");
            }
            if handle
                .destination_identity_hash(endpoint.destination_hash())
                .await
                .is_none()
            {
                handle
                    .request_path(endpoint.destination_hash())
                    .await
                    .expect("cold control path discovery");
            }
            let link = match identity {
                LinkIdentity::Controller => handle
                    .connect_remote_control_target(target_hash)
                    .await
                    .expect("identified control connection")
                    .connection()
                    .link_id(),
                LinkIdentity::Unidentified => handle
                    .establish_link(endpoint.destination_hash())
                    .await
                    .expect("unidentified connection"),
            };
            Event::Linked(link)
        });
        let mut completed = self.settle();
        for _ in 0..BOOTSTRAP_BUDGET_MS {
            if !completed.is_empty() {
                break;
            }
            completed = self.advance(1);
        }
        assert_eq!(
            completed.len(),
            1,
            "one control link completion within the bootstrap budget"
        );
        let (found, Event::Linked(link)) = completed.remove(0) else {
            unreachable!("linked control actor");
        };
        assert_eq!(found, task);
        link
    }
    pub fn expect_timeout(&mut self, task: ManualTaskId) {
        assert!(
            self.settle().is_empty(),
            "request must remain pending before its deadline"
        );
        assert!(
            self.advance(REQUEST_TIMEOUT_MS - 1).is_empty(),
            "no premature timeout"
        );
        let mut completed = self.advance(1);
        assert_eq!(completed.len(), 1, "one deadline completion");
        let (found, Event::Response(response)) = completed.remove(0) else {
            unreachable!("timed out request");
        };
        assert_eq!(found, task);
        assert_eq!(
            response,
            Err(SendError::Failed(SendRequestFailure::Timeout))
        );
    }
    pub fn target_response_count(&self) -> usize {
        let target = self.nodes[TARGET].endpoint;
        self.medium.inspect_trace(|trace| {
            trace
                .events()
                .filter(|event| match event {
                    MediumEvent::TransmissionAccepted { from, frame, .. } if *from == target => {
                        personal_rns::wire::WirePacketHeader::parse(frame).is_ok_and(
                            |(header, _)| {
                                header.context == personal_rns::wire::WireContext::Response
                            },
                        )
                    }
                    _ => false,
                })
                .count()
        })
    }
    pub fn request(&mut self, controller: usize, link: LinkId, bytes: Vec<u8>) -> ManualTaskId {
        let handle = self.nodes[controller].handle.clone();
        self.insert(async move {
            Event::Response(
                handle
                    .request_with_options(
                        link,
                        RequestPathHash::of(REMOTE_CONTROL_REQUEST_ENDPOINT_ID),
                        &bytes,
                        RequestOptions {
                            response_timeout: RequestResponseTimeout::Exact(DurationMillis(
                                REQUEST_TIMEOUT_MS,
                            )),
                            maximum_response_bytes: ByteLimit::Maximum(
                                RemoteControlResponse::MAX_ENCODED_LEN as u64,
                            ),
                        },
                    )
                    .await
                    .map(|(bytes, _)| bytes),
            )
        })
    }
    pub fn exchange(
        &mut self,
        controller: usize,
        link: LinkId,
        request: RemoteControlRequest,
    ) -> RemoteControlResponse {
        let task = self.request(controller, link, encoded(request));
        let mut completed = self.settle();
        assert_eq!(completed.len(), 1, "one control response");
        let (found, Event::Response(result)) = completed.remove(0) else {
            unreachable!("response actor");
        };
        assert_eq!(found, task);
        RemoteControlResponse::parse(&result.expect("authorized response"))
            .expect("canonical response")
    }
}

pub fn encoded(request: RemoteControlRequest) -> Vec<u8> {
    let mut bytes = vec![0; RemoteControlRequest::MAX_ENCODED_LEN];
    let len = request
        .write_into(&mut bytes)
        .expect("bounded control request");
    bytes.truncate(len);
    bytes
}

enum TargetProvisioning {
    Explicit,
    Persisted,
}
