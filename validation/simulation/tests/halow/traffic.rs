use personal_rns::runtime::{request_endpoints::*, PrnsNodeApi};
pub const RESOURCE_PATH: &str = "/simulation/halow/resource";
pub const RESOURCE_BYTES: usize = 4096;
const fn payload() -> [u8; RESOURCE_BYTES] {
    let mut bytes = [0; RESOURCE_BYTES];
    let mut value = 0x53a91c2du32;
    let mut index = 0;
    while index < RESOURCE_BYTES {
        value ^= value << 13;
        value ^= value >> 17;
        value ^= value << 5;
        bytes[index] = value as u8;
        index += 1;
    }
    bytes
}
pub const PAYLOAD: [u8; RESOURCE_BYTES] = payload();
pub struct ResourceReply;
impl RequestEndpoint<()> for ResourceReply {
    const ENDPOINT_ID: &'static str = RESOURCE_PATH;
    const POLICY: RequestEndpointPolicy = RequestEndpointPolicy::AllowAll;
    async fn handle(
        context: RequestContext<'_, ()>,
        node: &impl PrnsNodeApi,
    ) -> Result<(), Decline> {
        let token = context.respond_token();
        assert!(node
            .issue(personal_rns::engine::PrnsCommand::Respond(
                personal_rns::engine::Respond {
                    link_id: token.link_id,
                    request_id: token.request_id,
                    payload: personal_rns::engine::RespondPayload::StaticFile {
                        name: "halow.bin",
                        bytes: &PAYLOAD
                    }
                }
            ))
            .is_some());
        Err(Decline::Ignore)
    }
}
