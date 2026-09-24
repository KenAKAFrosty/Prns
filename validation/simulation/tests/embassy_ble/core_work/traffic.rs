use personal_rns::runtime::request_endpoints::{
    Decline, RequestContext, RequestEndpoint, RequestEndpointPolicy,
};
use personal_rns::runtime::PrnsNodeApi;

pub const ECHO_PATH: &str = "/simulation/core-work/echo";
pub const RESOURCE_PATH: &str = "/simulation/core-work/resource";
pub const COMMON_RESPONSE_BYTES: usize = 512;
pub const LARGE_RESPONSE_BYTES: usize = 16384;

pub struct Echo;
impl RequestEndpoint<()> for Echo {
    const ENDPOINT_ID: &'static str = ECHO_PATH;
    const POLICY: RequestEndpointPolicy = RequestEndpointPolicy::AllowAll;
    async fn handle(
        mut context: RequestContext<'_, ()>,
        _: &impl PrnsNodeApi,
    ) -> Result<(), Decline> {
        context.respond(context.data)
    }
}
pub struct ResourceReply;
impl RequestEndpoint<()> for ResourceReply {
    const ENDPOINT_ID: &'static str = RESOURCE_PATH;
    const POLICY: RequestEndpointPolicy = RequestEndpointPolicy::AllowAll;
    async fn handle(
        context: RequestContext<'_, ()>,
        node: &impl PrnsNodeApi,
    ) -> Result<(), Decline> {
        let encoded: [u8; 2] = context.data.try_into().map_err(|_| Decline::Ignore)?;
        let requested = usize::from(u16::from_be_bytes(encoded));
        let bytes = PAYLOAD.get(..requested).ok_or(Decline::Ignore)?;
        let token = context.respond_token();
        assert!(node
            .issue(personal_rns::engine::PrnsCommand::Respond(
                personal_rns::engine::Respond {
                    link_id: token.link_id,
                    request_id: token.request_id,
                    payload: personal_rns::engine::RespondPayload::StaticFile {
                        name: "core-work.bin",
                        bytes
                    },
                }
            ))
            .is_some());
        Err(Decline::Ignore)
    }
}
const fn dense_payload() -> [u8; LARGE_RESPONSE_BYTES] {
    let mut bytes = [0; LARGE_RESPONSE_BYTES];
    let mut state = 0x914c_70a3u32;
    let mut index = 0;
    while index < bytes.len() {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        bytes[index] = state as u8;
        index += 1;
    }
    bytes
}
pub const PAYLOAD: [u8; LARGE_RESPONSE_BYTES] = dense_payload();
