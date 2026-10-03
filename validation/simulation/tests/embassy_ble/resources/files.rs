use super::*;
use personal_rns::engine::{PrnsCommand, Respond, RespondPayload};

pub(super) const FILE_PATH: &str = "/simulation/file-reply";
// A recognized response-envelope header for an unrelated request is still
// literal file content. The receiver must not interpret or remove its prefix.
pub(super) const FILE_BYTES: [u8; RESPONSE_BYTES + 1] = {
    let mut bytes = [0; RESPONSE_BYTES + 1];
    let mut index = 0;
    while index < PAYLOAD.len() {
        bytes[index] = PAYLOAD[index];
        index += 1;
    }
    bytes[RESPONSE_BYTES] = 0xA7;
    bytes[0] = 0x92;
    bytes[1] = 0xC4;
    bytes[2] = 16;
    let mut index = 3;
    while index < RESPONSE_WIRE_OVERHEAD {
        bytes[index] = 0xF3;
        index += 1;
    }
    bytes
};

pub(super) struct FileReply;

impl RequestEndpoint<NoRemoteControlHostControls> for FileReply {
    const ENDPOINT_ID: &'static str = FILE_PATH;
    const POLICY: RequestEndpointPolicy = RequestEndpointPolicy::AllowAll;

    async fn handle(
        context: RequestContext<'_, NoRemoteControlHostControls>,
        node: &impl PrnsNodeApi,
    ) -> Result<(), Decline> {
        let encoded: [u8; 2] = context.data.try_into().map_err(|_| Decline::Ignore)?;
        let requested = usize::from(u16::from_be_bytes(encoded));
        let bytes = FILE_BYTES.get(..requested).ok_or(Decline::Ignore)?;
        // Exercise the shared static-file command through both real command
        // lanes. Tokio's streaming-file convenience API uses blocking workers
        // outside this controlled executor; it is not the subject of this test.
        let token = context.respond_token();
        assert!(node
            .issue(PrnsCommand::Respond(Respond {
                link_id: token.link_id,
                request_id: token.request_id,
                payload: RespondPayload::StaticFile {
                    name: "fixture.bin",
                    bytes
                },
            }))
            .is_some());
        Err(Decline::Ignore)
    }
}

pub(super) fn exchange(
    tasks: &mut EmbassyTasks<'_>,
    node: &ResourceNode,
    desktop: &PrnsNodeHandle,
    links: [LinkId; 2],
) {
    for maximum in [TRANSFER_BYTES - 1, TRANSFER_BYTES] {
        let desktop = desktop.clone();
        let (result, elapsed) = complete(tasks, async move {
            measured(desktop.request_with_options(
                links[1],
                RequestPathHash::of(FILE_PATH),
                &(TRANSFER_BYTES as u16).to_be_bytes(),
                RequestOptions {
                    response_timeout: RequestResponseTimeout::LinkDefault,
                    maximum_response_bytes: ByteLimit::Maximum(maximum as u64),
                },
            ))
            .await
        });
        assert_eq!(
            result,
            if maximum == TRANSFER_BYTES {
                Ok((FILE_BYTES[..TRANSFER_BYTES].to_vec(), elapsed))
            } else {
                Err(SendError::Failed(SendRequestFailure::ResponseTooLarge))
            }
        );
        // The proof acknowledges successful transport, not the requester's
        // application budget. Metadata length becomes known only at conclusion.
        assert_eq!(response_settlements(node), [Settlement::Respond(Ok(()))]);
    }
    let embedded = node.handle;
    let (result, elapsed) = complete(tasks, async move {
        measured(embedded.request(
            links[0],
            RequestPathHash::of(FILE_PATH),
            &(RESPONSE_BYTES as u16).to_be_bytes(),
        ))
        .await
    });
    assert_eq!(
        result.map(|(bytes, rtt)| (bytes.as_slice().to_vec(), rtt)),
        Ok((FILE_BYTES[..RESPONSE_BYTES].to_vec(), elapsed)),
    );
    assert!(response_settlements(node).is_empty());
}

#[test]
fn esp32_and_apple_nodes_preserve_file_bytes_and_exclude_metadata_from_the_budget() {
    scenario(
        Endpoint::Esp32(Esp32Host::Esp32),
        Endpoint::CoreBluetooth(AppleHost::MacOs),
        ResponseScenario::Files,
    );
}

#[test]
fn nrf52_and_bluez_nodes_preserve_file_bytes_and_exclude_metadata_from_the_budget() {
    scenario(
        Endpoint::Nrf52(Nrf52Host::Nrf52),
        Endpoint::BlueZ(BlueZHost::Linux),
        ResponseScenario::Files,
    );
}
