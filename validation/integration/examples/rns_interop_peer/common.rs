use core::time::Duration;

use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, DeliveryEvidence, PacketReceiptDelivered,
    PrnsCommand, RatchetPolicy,
};
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::BitrateBps;
use personal_rns::routing::links::request::{
    write_packed_binary_header, PackBinaryError, MAX_PACKED_BINARY_HEADER_LEN,
};
use personal_rns::routing::links::resources::ResourceStrategy;
use personal_rns::routing::{LinkRequestPolicy, ProofStrategy};
use personal_rns::runtime::{PreConfiguredDestination, ServeMyRequestEndpoints};
use personal_rns::units::ByteLimit;
use personal_rns::wire::DestinationHash;
use personal_rns::PrnsNodeHandle;

pub const BITRATE: BitrateBps = BitrateBps::guess(65_000_000);
pub const COMPLETION_GRACE: Duration = Duration::from_secs(1);
pub const COMPLETION_TIMEOUT: Duration = Duration::from_secs(35);
pub const ANNOUNCE_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProofFailure {
    ResponseInsteadOfProof,
}

pub fn secret(byte: u8) -> Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]> {
    Zeroizing::new([byte; IDENTITY_SECRET_KEY_LEN])
}

pub struct SingleDestination<'a> {
    pub app_name: &'a str,
    pub aspects: &'a [&'a str],
    pub test_identity_byte: u8,
    pub announce_app_data: &'a [u8],
    pub proof: ProofStrategy,
    pub link_requests: LinkRequestPolicy,
    pub ratchet: RatchetPolicy,
    pub resource_strategy: ResourceStrategy,
    pub maximum_request_bytes: ByteLimit,
    pub request_endpoints: ServeMyRequestEndpoints,
}

impl<'a> SingleDestination<'a> {
    pub fn into_preconfigured(self) -> PreConfiguredDestination<'a> {
        PreConfiguredDestination::Single {
            app_name: self.app_name,
            aspects: self.aspects,
            identity: secret(self.test_identity_byte),
            announce_app_data: self.announce_app_data,
            proof: self.proof,
            link_requests: self.link_requests,
            ratchet: self.ratchet,
            resource_strategy: self.resource_strategy,
            maximum_request_bytes: self.maximum_request_bytes,
            request_endpoints: self.request_endpoints,
        }
    }
}

pub fn spawn_announces(handle: PrnsNodeHandle, destination: DestinationHash) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(ANNOUNCE_INTERVAL);
        loop {
            interval.tick().await;
            if handle
                .issue(PrnsCommand::AnnounceNow(AnnounceNow {
                    destination,
                    target: AnnounceTarget::AllInterfaces,
                    app_data: AnnounceAppData::Registered,
                }))
                .is_none()
            {
                return;
            }
        }
    });
}

pub fn require_proof(receipt: PacketReceiptDelivered) -> Result<(), ProofFailure> {
    match receipt.evidence {
        DeliveryEvidence::Proof(_) => Ok(()),
        DeliveryEvidence::Response => Err(ProofFailure::ResponseInsteadOfProof),
    }
}

pub fn messagepack_binary(bytes: &[u8]) -> Result<Vec<u8>, PackBinaryError> {
    let mut header = [0u8; MAX_PACKED_BINARY_HEADER_LEN];
    let header_len = write_packed_binary_header(bytes.len(), &mut header)?;
    let mut packed = Vec::with_capacity(header_len + bytes.len());
    packed.extend_from_slice(&header[..header_len]);
    packed.extend_from_slice(bytes);
    Ok(packed)
}

pub fn required_environment(name: &'static str) -> Result<String, RequiredEnvironment> {
    std::env::var(name).map_err(|_| RequiredEnvironment(name))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequiredEnvironment(pub &'static str);
