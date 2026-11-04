use std::cell::Cell;
use std::io::Read;
use std::net::SocketAddr;
use std::path::Path;

use personal_rns::identity::IdentityHash;
use personal_rns::interfaces::ConnectionState;
use personal_rns::persistence::read_remote_control_target_accesses_snapshot;
use personal_rns::prelude::*;
use personal_rns::remote_control::{
    RemoteControlAuthorizeControllerOutcome, RemoteControlControllerAuthority,
    RemoteControlControllerCard, RemoteControlTargetAccessCard,
};
use personal_rns::runtime::RemoteControlTargetAccessControl;
use serde_json::json;

use super::Error;

#[derive(Clone, Copy, clap::ValueEnum)]
pub(super) enum OperatorProfile {
    /// Read-only inspection and an explicit announcement, where supported.
    Inspect,
    /// Supported non-administrative operations, excluding application messages and watch streams.
    Operate,
}

impl OperatorProfile {
    fn requests(self) -> RemoteControlRequestSet {
        match self {
            Self::Inspect => personal_hopspot_headless::control::operator_requests(),
            Self::Operate => RemoteControlRequestSet::all_operator(),
        }
    }
}

pub(super) fn identity_hash(value: &str) -> Result<IdentityHash, String> {
    let mut bytes = [0; 16];
    hex::decode_to_slice(value, &mut bytes).map_err(|error| error.to_string())?;
    Ok(IdentityHash::new(bytes))
}

fn read_card(path: &Path, maximum: usize) -> Result<String, Error> {
    let mut bytes = Vec::new();
    // Permit one trailing CRLF from a text file without accepting unbounded input.
    std::fs::File::open(path)?
        .take((maximum + 3) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > maximum + 2 {
        return Err(Error::Setup(
            "card file exceeds the shared codec's size limit".into(),
        ));
    }
    String::from_utf8(bytes).map_err(|_| Error::Setup("card file is not UTF-8 text".into()))
}

pub(super) fn controller_card_text(card: &RemoteControlControllerCard) -> Result<String, Error> {
    let mut bytes = [0; RemoteControlControllerCard::MAX_TEXT_LEN];
    let len = card.encode_text(&mut bytes)?;
    String::from_utf8(bytes[..len].to_vec())
        .map_err(|_| Error::Setup("card codec returned invalid text".into()))
}

fn target_card_text(card: &RemoteControlTargetAccessCard) -> Result<String, Error> {
    let mut bytes = [0; RemoteControlTargetAccessCard::MAX_TEXT_LEN];
    let len = card.encode_text(&mut bytes)?;
    String::from_utf8(bytes[..len].to_vec())
        .map_err(|_| Error::Setup("card codec returned invalid text".into()))
}

pub(super) async fn import_target(
    handle: &PrnsNodeHandle,
    path: &Path,
    trusted_source: bool,
) -> Result<(), Error> {
    if !trusted_source {
        return Err(Error::Setup("verify the card's source and target, then pass --trusted-source; decoding does not establish trust".into()));
    }
    let text = read_card(path, RemoteControlTargetAccessCard::MAX_TEXT_LEN)?;
    let card = RemoteControlTargetAccessCard::decode_text(text.trim())?;
    let target = card.access().target().identity_hash();
    let outcome = handle.save_remote_control_target_access_card(card).await?;
    println!(
        "{}",
        json!({
            "event": "target_access_saved",
            "target": hex::encode(target.as_bytes()),
            "outcome": format!("{outcome:?}"),
            "control_verified": false,
        })
    );
    Ok(())
}

pub(super) async fn list_targets(handle: &PrnsNodeHandle) -> Result<(), Error> {
    let inventory = handle
        .remote_control_target_inventory()
        .await
        .map_err(|error| Error::Setup(format!("target inventory: {error:?}")))?;
    for target in inventory.targets() {
        let resolved = handle
            .resolve_remote_control_target(target.identity_hash())
            .await
            .map_err(Error::Resolve)?;
        println!(
            "{}",
            json!({
                "event": "saved_target",
                "target": hex::encode(target.identity_hash().as_bytes()),
                "effective_requests": resolved.permitted_requests().iter().map(|kind| format!("{kind:?}")).collect::<Vec<_>>(),
            })
        );
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn delegate(
    handle: &PrnsNodeHandle,
    tcp: SocketAddr,
    target_id: IdentityHash,
    recipient_path: &Path,
    trusted_recipient: bool,
    profile: OperatorProfile,
    stage: &Cell<&'static str>,
) -> Result<(), Error> {
    let card = delegate_card(
        handle,
        tcp,
        target_id,
        recipient_path,
        trusted_recipient,
        profile,
        stage,
    )
    .await?;
    println!(
        "{}",
        json!({
            "event": "controller_authorized",
            "target": hex::encode(target_id.as_bytes()),
            "recipient": hex::encode(card.recipient().identity_hash().as_bytes()),
            "authority": "operator",
            "requests": card.access().permitted_requests().iter().map(|kind| format!("{kind:?}")).collect::<Vec<_>>(),
            "target_access_card": target_card_text(&card)?,
            "recipient_connection_verified": false,
        })
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn delegate_card(
    handle: &PrnsNodeHandle,
    tcp: SocketAddr,
    target_id: IdentityHash,
    recipient_path: &Path,
    trusted_recipient: bool,
    profile: OperatorProfile,
    stage: &Cell<&'static str>,
) -> Result<RemoteControlTargetAccessCard, Error> {
    if !trusted_recipient {
        return Err(Error::Setup(
            "verify the intended recipient card, then pass --trusted-recipient".into(),
        ));
    }
    let text = read_card(recipient_path, RemoteControlControllerCard::MAX_TEXT_LEN)?;
    let recipient = *RemoteControlControllerCard::decode_text(text.trim())?.controller();
    // Read the typed local snapshot to retain the exact pinned full target key.
    // This snapshot never serves as a cross-client exchange format.
    let snapshot = handle
        .snapshot_remote_control_target_accesses()
        .await
        .map_err(|error| Error::Setup(format!("saved target snapshot: {error:?}")))?
        .ok_or_else(|| Error::Setup("RemoteControl service is unavailable".into()))?;
    let access = read_remote_control_target_accesses_snapshot(&snapshot)
        .map_err(|error| Error::Setup(format!("saved target snapshot: {error:?}")))?
        .find(|access| access.target().identity_hash() == target_id)
        .ok_or_else(|| {
            Error::Setup("target is not saved; import its trusted access card first".into())
        })?;
    if !access
        .effective_requests()
        .supports(RemoteControlRequestKind::AuthorizeController)
    {
        return Err(Error::Setup(
            "saved access does not permit controller delegation".into(),
        ));
    }
    let target = RemoteControlTargetIdentity::new(*access.target().public_keys());
    let endpoint = target.endpoint().destination_hash();
    let interface = TcpClientInterface::new(tcp.to_string());
    let status = interface.status();
    handle.add_interface(interface);
    stage.set("interface connection");
    while status.connection() != ConnectionState::Connected {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    stage.set("path discovery");
    handle.request_path(endpoint).await.map_err(Error::Path)?;
    stage.set("authenticated target connection");
    let connection = handle
        .connect_remote_control_target(target_id)
        .await
        .map_err(Error::Connect)?;
    let result = async {
        stage.set("supported permissions");
        let (description, _) = connection.describe().await.map_err(Error::Operation)?;
        let mut requests = RemoteControlRequestSet::empty();
        for kind in profile.requests().iter() {
            if description.available_requests().supports(kind) {
                requests.insert(kind);
            }
        }
        let delegated_access = RemoteControlTargetAccess::new(
            target,
            RemoteControlControllerAuthority::Operator,
            requests,
        )
        .map_err(Error::Grant)?;
        // Validate the output before changing the target's grant table.
        let card = RemoteControlTargetAccessCard::new(recipient, delegated_access)?;
        let _encoded = target_card_text(&card)?;
        stage.set("controller authorization");
        let (outcome, _) = connection
            .authorize_controller(recipient, requests)
            .await
            .map_err(Error::Operation)?;
        if outcome != RemoteControlAuthorizeControllerOutcome::Applied {
            return Err(Error::Setup(format!(
                "target did not apply the grant: {outcome:?}"
            )));
        }
        Ok(card)
    }
    .await;
    connection.close();
    result
}

#[cfg(test)]
mod tests;
