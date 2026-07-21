use dioxus::prelude::*;
use prns_flash_manifest::{
    board_catalog, pinned_key_id, pinned_key_is_configured, provisioning_image, sha256_hex,
    verify_minisign, ChannelDescriptor, FlashManifest, ProvisioningAction, ReleaseChannel,
    TargetManifest, WifiCredentials, PINNED_MINISIGN_PUBLIC_KEY,
};
use serde::Deserialize;

use super::bridge::{self, BridgeProvisioning, BridgeRequest};
use super::model::{part_kind, FlasherState, PartDetails, ReleaseDetails, WifiAction};

const RELEASE_CHANNEL: &str = env!("PRNS_BUILD_CHANNEL");

const FETCH_CHANNEL_SCRIPT: &str = r#"
const channel = '__PRNS_RELEASE_CHANNEL__';
const descriptor = await fetch(`/releases/channels/${channel}.json`, { cache: 'no-store', credentials: 'omit', redirect: 'error' });
const signature = await fetch(`/releases/channels/${channel}.json.minisig`, { cache: 'no-store', credentials: 'omit', redirect: 'error' });
if (!descriptor.ok || !signature.ok) throw new Error('release channel documents unavailable');
dioxus.send({ descriptor: await descriptor.text(), signature: await signature.text() });
"#;

const FETCH_MANIFEST_SCRIPT: &str = r#"
const manifestUrl = await dioxus.recv();
const immutable = new URL(manifestUrl);
const localQualification = ['localhost', '127.0.0.1', '::1'].includes(location.hostname);
const resolvedUrl = localQualification ? immutable.pathname : immutable.href;
const manifest = await fetch(resolvedUrl, { cache: 'no-store', credentials: 'omit', redirect: 'error' });
const signature = await fetch(`${resolvedUrl}.minisig`, { cache: 'no-store', credentials: 'omit', redirect: 'error' });
if (!manifest.ok || !signature.ok) throw new Error('immutable release documents unavailable');
dioxus.send({ manifest: await manifest.text(), signature: await signature.text() });
"#;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChannelDocuments {
    descriptor: String,
    signature: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestDocuments {
    manifest: String,
    signature: String,
}

struct AcquiredRelease {
    request: BridgeRequest,
    details: ReleaseDetails,
}

pub(super) async fn prepare_release(
    board_slug: String,
    selected_action: WifiAction,
    ssid: String,
    password: String,
    mut state: FlasherState,
) {
    bridge::clear_prepared();
    state.phase.set("validating_manifest".to_string());
    state
        .status
        .set("Downloading and verifying the signed release manifest…".to_string());
    state.progress_current.set(0);
    state.progress_total.set(0);

    let result = async {
        let acquired = acquire_release(board_slug, selected_action, ssid, password).await?;
        bridge::prepare(acquired.request, state).await?;
        Ok::<ReleaseDetails, String>(acquired.details)
    }
    .await;

    match result {
        Ok(details) => {
            state.release.set(Some(details));
            state.prepared.set(true);
            state.ssid.set(String::new());
            state.password.set(String::new());
            bridge::focus_status();
        }
        Err(message) => {
            state.phase.set("failed".to_string());
            state.status.set(message);
            state.prepared.set(false);
            state.ssid.set(String::new());
            state.password.set(String::new());
            bridge::focus_status();
        }
    }
}

async fn acquire_release(
    board_slug: String,
    selected_action: WifiAction,
    ssid: String,
    password: String,
) -> Result<AcquiredRelease, String> {
    if !pinned_key_is_configured() {
        return Err("Release signing custody is not configured.".to_string());
    }
    let channel_script = FETCH_CHANNEL_SCRIPT.replace("__PRNS_RELEASE_CHANNEL__", RELEASE_CHANNEL);
    let mut channel_eval = document::eval(&channel_script);
    let channel_documents = channel_eval
        .recv::<ChannelDocuments>()
        .await
        .map_err(|_| format!("The signed {RELEASE_CHANNEL} channel is unavailable."))?;
    verify_minisign(
        channel_documents.descriptor.as_bytes(),
        &channel_documents.signature,
        PINNED_MINISIGN_PUBLIC_KEY,
    )
    .map_err(|error| error.to_string())?;
    let descriptor = ChannelDescriptor::from_json(
        channel_documents.descriptor.as_bytes(),
        configured_release_channel(),
    )
    .map_err(|error| error.to_string())?;

    let mut manifest_eval = document::eval(FETCH_MANIFEST_SCRIPT);
    manifest_eval
        .send(descriptor.manifest_url.clone())
        .map_err(|_| "Could not request the immutable release manifest.".to_string())?;
    let documents = manifest_eval
        .recv::<ManifestDocuments>()
        .await
        .map_err(|_| "The immutable signed release is unavailable.".to_string())?;
    if sha256_hex(documents.manifest.as_bytes()) != descriptor.manifest_sha256 {
        return Err("The manifest does not match the signed release channel.".to_string());
    }
    verify_minisign(
        documents.manifest.as_bytes(),
        &documents.signature,
        PINNED_MINISIGN_PUBLIC_KEY,
    )
    .map_err(|error| error.to_string())?;
    let catalog = board_catalog().map_err(|error| error.to_string())?;
    let manifest = FlashManifest::from_json(documents.manifest.as_bytes(), &catalog)
        .map_err(|error| error.to_string())?;
    let expected_key_id = pinned_key_id()
        .ok_or_else(|| "The pinned release key has no canonical key ID.".to_string())?;
    if !manifest
        .signing
        .key_id
        .eq_ignore_ascii_case(&expected_key_id)
    {
        return Err("The signed manifest names a different release key.".to_string());
    }
    if manifest.release.version != descriptor.version
        || manifest.release.channel != descriptor.channel
    {
        return Err("The signed channel and manifest release identity disagree.".to_string());
    }
    let target = manifest
        .targets
        .iter()
        .find(|target| target.board_slug == board_slug)
        .ok_or_else(|| "The signed release does not contain this board.".to_string())?;
    let provisioning = bridge_provisioning(target, selected_action, ssid, password)?;
    let request = BridgeRequest::from_target(target, &descriptor.manifest_url, provisioning)?;
    let details = ReleaseDetails {
        version: manifest.release.version.to_string(),
        channel: match manifest.release.channel {
            ReleaseChannel::Stable => "stable".to_string(),
            ReleaseChannel::Preview => "preview".to_string(),
        },
        total: target.parts.iter().map(|part| part.size).sum(),
        parts: target
            .parts
            .iter()
            .map(|part| PartDetails {
                kind: part_kind(part.kind),
                size: part.size,
                sha256: part.sha256.to_string(),
            })
            .collect(),
    };
    Ok(AcquiredRelease { request, details })
}

fn bridge_provisioning(
    target: &TargetManifest,
    action: WifiAction,
    ssid: String,
    password: String,
) -> Result<Option<BridgeProvisioning>, String> {
    let Some(slot) = &target.provisioning else {
        return Ok(None);
    };
    let provisioning_action = match action {
        WifiAction::Preserve => ProvisioningAction::Preserve,
        WifiAction::Clear => ProvisioningAction::Clear,
        WifiAction::Configure => ProvisioningAction::Configure(WifiCredentials {
            ssid: ssid.clone(),
            password: password.clone(),
        }),
    };
    provisioning_image(&provisioning_action).map_err(|error| error.to_string())?;
    Ok(Some(BridgeProvisioning {
        action: action.wire().to_string(),
        offset: slot.offset,
        size: slot.size,
        ssid: if action == WifiAction::Configure {
            ssid
        } else {
            String::new()
        },
        password: if action == WifiAction::Configure {
            password
        } else {
            String::new()
        },
    }))
}

fn configured_release_channel() -> ReleaseChannel {
    match RELEASE_CHANNEL {
        "stable" => ReleaseChannel::Stable,
        "preview" => ReleaseChannel::Preview,
        _ => panic!("unsupported compiled release channel"),
    }
}
