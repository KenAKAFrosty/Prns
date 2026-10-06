//! Operates an already-authorized remote controller over a TCP Reticulum bridge.
//! Keys are separate raw 64-byte private-identity files, never command-line key bytes.
use personal_rns::engine::{EstablishLinkFailure, IdentifyFailure};
use personal_rns::identity::vault::IdentitySecretKey;
use personal_rns::interfaces::lora::{LoRaProfile, GHZ24_BALANCED_PROFILE};
use personal_rns::prelude::*;
use personal_rns::remote_control::{
    RemoteControlInterfaceContinuation, RemoteControlInterfacePage, RemoteControlLoRaProfile,
    RemoteControlNodeIdentitySecretsError, RemoteControlRadioConfiguration,
    RemoteControlRadioOutcome,
};
use personal_rns::runtime::{NodeRunError, RemoteControlError, SendError};
use std::{io::Read, time::Duration};

#[derive(Debug)]
enum Error {
    Usage,
    InvalidDestination,
    InvalidProfile,
    InvalidKeyLength,
    Io(std::io::Error),
    Identity(RemoteControlNodeIdentitySecretsError),
    Link(SendError<EstablishLinkFailure>),
    Identify(SendError<IdentifyFailure>),
    Remote(RemoteControlError),
    Node(NodeRunError),
    NodeStopped,
    DiscoveryClosed,
    Timeout,
    UnsupportedTarget,
    NoRadio,
    Configuration(RemoteControlRadioOutcome),
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "{e}"),
            Self::Identity(e) => write!(f, "{e:?}"),
            Self::Link(e) => write!(f, "{e:?}"),
            Self::Identify(e) => write!(f, "{e:?}"),
            Self::Remote(e) => write!(f, "{e:?}"),
            Self::Node(e) => write!(f, "{e:?}"),
            Self::Configuration(outcome) => write!(f, "Configuration outcome: {outcome:?}"),
            other => write!(f, "{other:?}"),
        }
    }
}
impl std::error::Error for Error {}

fn destination(text: &str) -> Result<DestinationHash, Error> {
    if text.len() != 32 {
        return Err(Error::InvalidDestination);
    }
    let mut bytes = [0; 16];
    for (out, pair) in bytes.iter_mut().zip(text.as_bytes().as_chunks::<2>().0) {
        let pair = core::str::from_utf8(pair).map_err(|_| Error::InvalidDestination)?;
        *out = u8::from_str_radix(pair, 16).map_err(|_| Error::InvalidDestination)?;
    }
    Ok(DestinationHash::new(bytes))
}

fn read_key(mut file: impl Read) -> Result<IdentitySecretKey, Error> {
    let mut secret = IdentitySecretKey::new([0; IDENTITY_SECRET_KEY_LEN]);
    file.read_exact(secret.as_mut()).map_err(|error| {
        if error.kind() == std::io::ErrorKind::UnexpectedEof {
            Error::InvalidKeyLength
        } else {
            Error::Io(error)
        }
    })?;
    let mut trailing = [0];
    if file.read(&mut trailing).map_err(Error::Io)? != 0 {
        return Err(Error::InvalidKeyLength);
    }
    Ok(secret)
}

fn load_key(path: &str) -> Result<IdentitySecretKey, Error> {
    read_key(std::fs::File::open(path).map_err(Error::Io)?)
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    let [bridge, target, controller_key, local_target_key, operation] = arguments.as_slice() else {
        eprintln!("Usage: radio_control HOST:PORT TARGET_DESTINATION_HEX CONTROLLER_KEY_FILE LOCAL_TARGET_KEY_FILE inspect|balanced|clear|PROFILE_TEXT");
        return Err(Error::Usage);
    };
    let target = destination(target)?;
    let requested = match operation.as_str() {
        "inspect" => None,
        "clear" => Some(RemoteControlRadioConfiguration::Unconfigured),
        "balanced" => Some(RemoteControlRadioConfiguration::Profile(
            RemoteControlLoRaProfile::from_band_profile(LoRaProfile::Ghz24(GHZ24_BALANCED_PROFILE))
                .ok_or(Error::InvalidProfile)?,
        )),
        text => Some(RemoteControlRadioConfiguration::Profile(
            RemoteControlLoRaProfile::parse(text).ok_or(Error::InvalidProfile)?,
        )),
    };
    let secrets = RemoteControlNodeIdentitySecrets::new(
        RemoteControlControllerIdentitySecret::from(load_key(controller_key)?),
        RemoteControlTargetIdentitySecret::from(load_key(local_target_key)?),
    )
    .map_err(Error::Identity)?;
    let controller_identity = secrets.identities().controller().identity_hash();
    let service = RemoteControlService::new(
        secrets,
        RemoteControlInitialControllerGrants::Nobody,
        RemoteControlSelfAnnouncement::Unavailable,
    );
    let (heard, mut announces) = tokio::sync::mpsc::channel(1);
    let bridge = bridge.clone();
    let node = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        remote_control: service.into(),
        pre_configured_destinations: [] as [PreConfiguredDestination<'static>; 0],
        app_state: personal_rns::runtime::NoRemoteControlHostControls,
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        on_event: move |event, _state| {
            if let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { destination, .. }) = event {
                if destination == target {
                    let _ = heard.try_send(());
                }
            }
        },
        interfaces: move |node: &PrnsNodeHandle| {
            node.attach(TcpClientInterface::new(bridge));
        },
        persistence: NoPersistence,
    });
    let handle = node.handle();
    println!("Waiting for the target announcement. Use the Base Duo announce button if needed.");
    let operation = async {
        announces.recv().await.ok_or(Error::DiscoveryClosed)?;
        let link = handle.establish_link(target).await.map_err(Error::Link)?;
        handle
            .identify(link, controller_identity)
            .await
            .map_err(Error::Identify)?;
        let remote = handle.remote_control(link);
        let (description, _) = remote.describe().await.map_err(Error::Remote)?;
        let capabilities = description.available_requests();
        if !capabilities.supports(RemoteControlRequestKind::InspectRadio)
            || (requested.is_some()
                && !capabilities.supports(RemoteControlRequestKind::ConfigureRadio))
        {
            return Err(Error::UnsupportedTarget);
        }
        let mut page = RemoteControlInterfacePage::First;
        let id = loop {
            let (inventory, _) = remote
                .inventory_interfaces_page(page)
                .await
                .map_err(Error::Remote)?;
            if let Some(entry) = inventory
                .entries()
                .iter()
                .find(|entry| entry.kind == InterfaceKind::LoRa)
            {
                break entry.id;
            }
            page = match inventory.continuation() {
                RemoteControlInterfaceContinuation::Complete => return Err(Error::NoRadio),
                RemoteControlInterfaceContinuation::More(cursor) => {
                    RemoteControlInterfacePage::After(cursor)
                }
            };
        };
        let (status, _) = remote.inspect_radio(id).await.map_err(Error::Remote)?;
        println!("{status:?}");
        if let Some(configuration) = requested {
            let (outcome, _) = remote
                .configure_radio(id, configuration)
                .await
                .map_err(Error::Remote)?;
            println!("{outcome:?}");
            if outcome != RemoteControlRadioOutcome::Saved {
                return Err(Error::Configuration(outcome));
            }
        }
        Ok(())
    };
    tokio::select! {
        result = tokio::time::timeout(Duration::from_secs(60), operation) => result.map_err(|_| Error::Timeout)?,
        result = node.run() => { result.map_err(Error::Node)?; Err(Error::NodeStopped) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn destination_accepts_exactly_sixteen_hex_bytes() {
        let expected = DestinationHash::new([0xab; 16]);
        assert_eq!(
            destination("abababababababababababababababab").unwrap(),
            expected
        );
        assert_eq!(
            destination("ABABABABABABABABABABABABABABABAB").unwrap(),
            expected
        );
        for invalid in [
            "",
            "ab",
            "abababababababababababababababa",
            "ababababababababababababababababab",
            "zbababababababababababababababab",
            "éababababababababababababababab",
        ] {
            assert!(matches!(
                destination(invalid),
                Err(Error::InvalidDestination)
            ));
        }
    }

    #[test]
    fn private_key_requires_exactly_the_identity_secret_length() {
        let bytes = [0x42; IDENTITY_SECRET_KEY_LEN];
        assert_eq!(read_key(bytes.as_slice()).unwrap().as_ref(), bytes);
        for length in [
            0,
            1,
            IDENTITY_SECRET_KEY_LEN - 1,
            IDENTITY_SECRET_KEY_LEN + 1,
        ] {
            assert!(matches!(
                read_key(vec![0; length].as_slice()),
                Err(Error::InvalidKeyLength)
            ));
        }
        struct FailedRead;
        impl Read for FailedRead {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("read failed"))
            }
        }
        assert!(matches!(read_key(FailedRead), Err(Error::Io(_))));
    }
}
