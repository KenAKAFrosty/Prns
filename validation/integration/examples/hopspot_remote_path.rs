#![allow(clippy::expect_used)]

use core::fmt::Write as _;

use personal_hopspot_core::{node_pages::NodePageRoutes, HopspotDestinationSet};
use personal_rns::identity::{
    in_memory::InMemoryNodeIdentity, IdentitySigner, PublicIdentityMaterial,
    IDENTITY_PUBLIC_KEY_LEN,
};
use personal_rns::prelude::*;
use personal_rns::remote_control::RemoteControlControllerAuthority;
use personal_rns::storage::GrowableHeap;
use personal_rns::tcp::TcpServer;

fn hex_array<const N: usize>(name: &str) -> [u8; N] {
    let value = std::env::var(name).expect("required hexadecimal environment variable");
    assert_eq!(value.len(), N * 2, "{name} has the wrong length");
    let mut bytes = [0; N];
    for (index, byte) in bytes.iter_mut().enumerate() {
        let offset = index * 2;
        *byte = u8::from_str_radix(&value[offset..offset + 2], 16)
            .expect("environment variable is hexadecimal");
    }
    bytes
}

fn hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(output, "{byte:02x}").expect("String writes cannot fail");
    }
    output
}

#[tokio::main]
async fn main() {
    let port: u16 = std::env::var("PRNS_HOPSPOT_PORT")
        .expect("PRNS_HOPSPOT_PORT is set")
        .parse()
        .expect("PRNS_HOPSPOT_PORT is a port");
    let controller = RemoteControlControllerIdentity::new(
        PublicIdentityMaterial::from_bytes(hex_array::<IDENTITY_PUBLIC_KEY_LEN>(
            "PRNS_CONTROLLER_PUBLIC_KEY",
        ))
        .public_keys(),
    );
    let grant = RemoteControlControllerGrant::new(
        controller,
        RemoteControlControllerAuthority::Operator,
        RemoteControlRequestSet::only(RemoteControlRequestKind::Describe),
    )
    .expect("Describe is a valid operator grant");
    let grants = [grant];
    let remote_control = RemoteControlService::new(
        RemoteControlNodeIdentitySecrets::new(
            RemoteControlControllerIdentitySecret::from(Zeroizing::new(
                [0xc1; IDENTITY_SECRET_KEY_LEN],
            )),
            RemoteControlTargetIdentitySecret::from(Zeroizing::new(
                [0xc2; IDENTITY_SECRET_KEY_LEN],
            )),
        )
        .expect("fixture Remote Control identities are distinct"),
        RemoteControlInitialControllerGrants::Grants(
            RemoteControlControllerGrants::try_from(grants.as_slice())
                .expect("one controller grant is configured"),
        ),
        RemoteControlSelfAnnouncement::Unavailable,
    );

    let identity = Zeroizing::new([0x5a; IDENTITY_SECRET_KEY_LEN]);
    let transport_hash = InMemoryNodeIdentity::from_secret_key_bytes(&identity).identity_hash();
    let destinations = HopspotDestinationSet::new(
        identity.clone(),
        b"Hopspot RNS path interop",
        b"Hopspot RNS path interop",
    );
    let server = TcpServer::bind(std::format!("127.0.0.1:{port}"))
        .await
        .expect("Hopspot interop TCP server binds");
    let node = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: Some(identity),
        remote_control,
        pre_configured_destinations: destinations.into_preconfigured_destinations(),
        app_state: NoRemoteControlHostControls,
        storage: GrowableHeap,
        request_endpoints: NodePageRoutes,
        interfaces: ManuallyAttached,
        persistence: NoPersistence,
        on_event: |_, _state: &NoRemoteControlHostControls| {},
    });
    let handle = node.handle();
    let _server = handle.supervise(server);
    println!(
        "HOPSPOT_RNS_PATH_READY transport={}",
        hex(transport_hash.as_bytes())
    );
    node.run().await.expect("Hopspot interop node remains live");
}
