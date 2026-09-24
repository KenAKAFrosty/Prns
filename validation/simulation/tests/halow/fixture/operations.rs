use super::*;
use personal_rns::engine::{AnnounceAppData, AnnounceNow, AnnounceTarget, RequestResponseTimeout};
use personal_rns::routing::{links::LinkId, request_handlers::RequestPathHash};
use personal_rns::units::{ByteLimit, DurationMillis};

pub fn encoded(request: RemoteControlRequest) -> Vec<u8> {
    let mut bytes = vec![0; RemoteControlRequest::MAX_ENCODED_LEN];
    let length = request.write_into(&mut bytes).expect("request encoding");
    bytes.truncate(length);
    bytes
}
impl Lab<'_> {
    pub fn announce(&mut self, index: usize) {
        let handle = self.nodes[index].handle.clone();
        self.complete(async move {
            handle
                .announce_now(AnnounceNow {
                    destination: target(index).endpoint().destination_hash(),
                    target: AnnounceTarget::AllInterfaces,
                    app_data: AnnounceAppData::Registered,
                })
                .await
                .expect("explicit announce");
        });
    }
    pub fn rediscover(&mut self, index: usize) {
        self.rediscover_via(index, TARGET);
    }
    pub fn rediscover_via(&mut self, index: usize, neighbor: usize) {
        let id = scope(index).peer_id(mac(neighbor));
        let received = |lab: &Lab<'_>| {
            lab.nodes[index]
                .handle
                .interfaces()
                .iter()
                .find(|row| row.id == id)
                .map_or(0, |row| row.rx_bytes)
        };
        let before = received(self);
        self.announce(TARGET);
        for _ in 0..1000 {
            if received(self) > before {
                return;
            }
            assert!(self.advance(1).is_empty());
        }
        panic!("target frame must reach the actual peer lane before reconnect");
    }
    pub fn link(&mut self, index: usize) -> LinkId {
        self.link_to(index, TARGET)
    }
    pub fn link_to(&mut self, index: usize, destination_node: usize) -> LinkId {
        let handle = self.nodes[index].handle.clone();
        let task = self.insert(async move {
            let target = target(destination_node);
            let destination = target.endpoint().destination_hash();
            let identity = target.identity_hash();
            let access = RemoteControlTargetAccess::new(
                target,
                RemoteControlControllerAuthority::Operator,
                permissions(),
            )
            .expect("pinned test target");
            handle
                .set_remote_control_target_access(access)
                .await
                .expect("local pin");
            if handle
                .destination_identity_hash(destination)
                .await
                .is_none()
            {
                handle
                    .request_path(destination)
                    .await
                    .expect("path discovery");
            }
            let link = handle
                .connect_remote_control_target(identity)
                .await
                .expect("verified link")
                .connection()
                .link_id();
            Event::Linked(link)
        });
        let Event::Linked(link) = self.wait(task, 10_000) else {
            panic!("link completion");
        };
        link
    }
    pub fn request(
        &mut self,
        index: usize,
        link: LinkId,
        request: RemoteControlRequest,
    ) -> ManualTaskId {
        self.request_bytes(
            index,
            link,
            REMOTE_CONTROL_REQUEST_ENDPOINT_ID,
            encoded(request),
            REQUEST_TIMEOUT_MS,
        )
    }
    pub fn request_bytes(
        &mut self,
        index: usize,
        link: LinkId,
        path: &'static str,
        bytes: Vec<u8>,
        timeout: u64,
    ) -> ManualTaskId {
        let handle = self.nodes[index].handle.clone();
        self.insert(async move {
            Event::Response(
                handle
                    .request_with_options(
                        link,
                        RequestPathHash::of(path),
                        &bytes,
                        RequestOptions {
                            response_timeout: RequestResponseTimeout::Exact(DurationMillis(
                                timeout,
                            )),
                            maximum_response_bytes: ByteLimit::Maximum(16_384),
                        },
                    )
                    .await
                    .map(|(bytes, _)| bytes),
            )
        })
    }
    pub fn exchange(
        &mut self,
        index: usize,
        link: LinkId,
        request: RemoteControlRequest,
    ) -> RemoteControlResponse {
        let task = self.request(index, link, request);
        let Event::Response(reply) = self.wait(task, 10_000) else {
            panic!("response actor");
        };
        RemoteControlResponse::parse(&reply.expect("admitted response")).expect("canonical reply")
    }
    pub fn app(&mut self, index: usize, link: LinkId, payload: &[u8]) {
        let message = RemoteControlAppMessage::from_slice(payload).expect("bounded message");
        assert_eq!(
            self.exchange(
                index,
                link,
                RemoteControlRequest::AppMessage(message.clone())
            ),
            RemoteControlResponse::AppMessage(message)
        );
        assert!(self.calls.borrow().contains(&AppInvocation {
            controller: controller(index),
            payload: payload.to_vec()
        }));
    }
    pub fn arm(&self, from: usize, destination: FaultDestination, action: TransmissionAction) {
        self.medium
            .arm_next(NextDatagramFault {
                from: self.nodes[from].radio,
                destination,
                action,
            })
            .expect("bounded fault arm");
    }
    pub fn response_count(&self, index: usize) -> usize {
        self.medium
            .snapshot()
            .events
            .iter()
            .filter(|event| match event {
                HaLowEvent::Transmitted { from, bytes, .. } if *from == self.nodes[index].radio => {
                    decode(bytes).is_ok_and(|frame| {
                        personal_rns::wire::WirePacketHeader::parse(frame).is_ok_and(
                            |(header, _)| {
                                header.context == personal_rns::wire::WireContext::Response
                            },
                        )
                    })
                }
                _ => false,
            })
            .count()
    }
}
