use dioxus::prelude::*;
use prns_flash_manifest::{
    provisioning_image, sha256_hex, verify_minisign, ProvisioningAction, ReleaseChannel,
    ReleasePartRef, ReleaseTarget, TcpClientEndpoint, TcpClientHost, ValidatedChannelDescriptor,
    WifiCredentials,
};
use serde::{Deserialize, Serialize};
use std::fmt;

use crate::platforms::BoardFlashTarget;

use super::bridge::{self, BridgeProvisioning, BridgeRequest};
use super::contract::BridgePhase;
use super::model::{
    part_kind, DestructiveConfirmation, FlasherState, InstallMode, PartDetails,
    ReleaseCompatibility, ReleaseDetails, WifiAction,
};
use super::trust;

const RELEASE_CHANNEL: &str = env!("PRNS_BUILD_CHANNEL");

const FETCH_SIGNED_DOCUMENTS_SCRIPT: &str = r#"
const request = await dioxus.recv();
try {
  window.__prnsFlashReleaseBoundary = window.__prnsFlashReleaseBoundary || await import('/assets/flasher/prns-flash.js');
  dioxus.send(await window.__prnsFlashReleaseBoundary.fetchSignedDocuments(request));
} catch (_) {
  dioxus.send({ status: 'error', error: 'unavailable' });
}
"#;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SignedDocumentRequest {
    document_url: String,
    document_max_bytes: u64,
    signature_max_bytes: u64,
}

#[derive(Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
enum SignedDocumentResponse {
    Ready { document: String, signature: String },
    Error { error: SignedDocumentFetchError },
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SignedDocumentFetchError {
    Unavailable,
    TooLarge,
    InvalidUtf8,
}

struct SignedDocuments {
    document: String,
    signature: String,
}

#[derive(Clone, Copy)]
enum ReleaseRecovery {
    BrowserRetry,
    CustodyUnavailable,
    ReviewSelection,
}

struct ReleaseAcquisitionError {
    diagnosis: String,
    recovery: ReleaseRecovery,
}

impl ReleaseAcquisitionError {
    fn browser_retry(diagnosis: impl Into<String>) -> Self {
        Self {
            diagnosis: diagnosis.into(),
            recovery: ReleaseRecovery::BrowserRetry,
        }
    }

    fn custody_unavailable(diagnosis: impl Into<String>) -> Self {
        Self {
            diagnosis: diagnosis.into(),
            recovery: ReleaseRecovery::CustodyUnavailable,
        }
    }

    fn review_selection(diagnosis: impl Into<String>) -> Self {
        Self {
            diagnosis: diagnosis.into(),
            recovery: ReleaseRecovery::ReviewSelection,
        }
    }
}

impl From<String> for ReleaseAcquisitionError {
    fn from(diagnosis: String) -> Self {
        Self::browser_retry(diagnosis)
    }
}

impl fmt::Display for ReleaseAcquisitionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.diagnosis.trim())?;
        formatter.write_str(" ")?;
        formatter.write_str(match self.recovery {
            ReleaseRecovery::BrowserRetry => {
                "Do not connect a device. Reload this page and prepare the signed release again; if it repeats, use the CLI and report the selected board and release channel."
            }
            ReleaseRecovery::CustodyUnavailable => {
                "Do not connect a device. Use the signed CLI release path until website signing custody is restored."
            }
            ReleaseRecovery::ReviewSelection => {
                "Review the selected board, Wi-Fi values, and optional TCP target, then prepare and verify the release again. No device access has started."
            }
        })
    }
}

struct AcquiredRelease {
    request: BridgeRequest,
    details: ReleaseDetails,
}

pub(super) struct ReleaseSelection {
    pub(super) board_slug: String,
    pub(super) install_mode: InstallMode,
    pub(super) destructive_confirmation: DestructiveConfirmation,
    pub(super) wifi_action: WifiAction,
    pub(super) ssid: String,
    pub(super) password: String,
    pub(super) tcp_target: Option<String>,
    pub(super) compatibility: ReleaseCompatibility,
}

pub(super) async fn prepare_release(
    selection: ReleaseSelection,
    mut state: FlasherState,
    generation: u64,
) {
    if !state.preparation_is_current(generation) {
        return;
    }
    bridge::clear_prepared();
    state.phase.set(BridgePhase::ValidatingManifest);
    state
        .status
        .set("Downloading and verifying the signed release manifest…".to_string());
    state.progress_current.set(0);
    state.progress_total.set(0);

    let install_mode = selection.install_mode;
    let acquired = acquire_release(selection, state.flash_target).await;
    if !state.preparation_is_current(generation) {
        return;
    }
    let result = match acquired {
        Ok(acquired) => match bridge::prepare(acquired.request, state.clone(), generation).await {
            Ok(()) => Ok(acquired.details),
            Err(bridge::PreparationError::Stale) => return,
            Err(bridge::PreparationError::Failed(message)) => Err(message),
        },
        Err(error) => Err(error.to_string()),
    };
    if !state.preparation_is_current(generation) {
        return;
    }
    state.preparation_active.set(false);

    match result {
        Ok(details) => {
            state.release.set(Some(details));
            state.prepared.set(true);
            bridge::focus_status();
        }
        Err(message) => {
            state.phase.set(BridgePhase::Failed);
            state.status.set(message);
            state.prepared.set(false);
            if install_mode == InstallMode::EraseAll {
                state
                    .destructive_confirmation
                    .set(DestructiveConfirmation::Unconfirmed);
            }
            bridge::focus_status();
        }
    }
}

async fn acquire_release(
    selection: ReleaseSelection,
    flash_target: BoardFlashTarget,
) -> Result<AcquiredRelease, ReleaseAcquisitionError> {
    let ReleaseSelection {
        board_slug,
        install_mode,
        destructive_confirmation,
        wifi_action,
        ssid,
        password,
        tcp_target,
        compatibility,
    } = selection;
    if !trust::key_is_configured() {
        return Err(ReleaseAcquisitionError::custody_unavailable(
            "Release signing custody is not configured.",
        ));
    }
    let limits = super::contract::response_limits();
    let channel_documents = fetch_signed_documents(
        format!("/releases/channels/{RELEASE_CHANNEL}.json"),
        limits.channel_bytes(),
        "release channel",
    )
    .await?;
    verify_minisign(
        channel_documents.document.as_bytes(),
        &channel_documents.signature,
        trust::PUBLIC_KEY,
    )
    .map_err(|error| error.to_string())?;
    let descriptor = ValidatedChannelDescriptor::from_json(
        channel_documents.document.as_bytes(),
        configured_release_channel(),
    )
    .map_err(|error| error.to_string())?;
    require_exact_manifest_url(descriptor.version().as_str(), descriptor.manifest_url())?;

    let documents = fetch_signed_documents(
        descriptor.manifest_url().to_string(),
        limits.manifest_bytes(),
        "immutable release manifest",
    )
    .await?;
    if sha256_hex(documents.document.as_bytes()) != descriptor.manifest_sha256().as_str() {
        return Err(ReleaseAcquisitionError::browser_retry(
            "The manifest does not match the signed release channel.",
        ));
    }
    verify_minisign(
        documents.document.as_bytes(),
        &documents.signature,
        trust::PUBLIC_KEY,
    )
    .map_err(|error| error.to_string())?;
    #[cfg(feature = "browser-test-fixture")]
    let manifest = super::validate_release_0_2_6_fixture(documents.document.as_bytes())
        .map_err(|error| error.to_string())?;
    #[cfg(feature = "local-dev-flasher")]
    let manifest = {
        let catalog = prns_flash_manifest::board_catalog().map_err(|error| error.to_string())?;
        let policy = crate::local_development::manifest_target_set_policy(&catalog)
            .map_err(|error| error.to_string())?;
        prns_flash_manifest::ValidatedFlashManifest::from_json_with_target_set(
            documents.document.as_bytes(),
            &catalog,
            &policy,
        )
        .map_err(|error| error.to_string())?
    };
    #[cfg(not(any(feature = "browser-test-fixture", feature = "local-dev-flasher")))]
    let manifest = {
        let catalog = prns_flash_manifest::board_catalog().map_err(|error| error.to_string())?;
        prns_flash_manifest::ValidatedFlashManifest::from_json(
            documents.document.as_bytes(),
            &catalog,
        )
        .map_err(|error| error.to_string())?
    };
    let expected_key_id = trust::key_id()
        .ok_or_else(|| "The pinned release key has no canonical key ID.".to_string())?;
    if !manifest
        .signing()
        .key_id()
        .as_str()
        .eq_ignore_ascii_case(&expected_key_id)
    {
        return Err(ReleaseAcquisitionError::browser_retry(
            "The signed manifest names a different release key.",
        ));
    }
    if manifest.release().version() != descriptor.version()
        || manifest.release().channel() != descriptor.channel()
    {
        return Err(ReleaseAcquisitionError::browser_retry(
            "The signed channel and manifest release identity disagree.",
        ));
    }
    let target = manifest
        .targets()
        .iter()
        .find(|target| target.board_id().as_str() == board_slug)
        .ok_or_else(|| {
            ReleaseAcquisitionError::review_selection(
                "The signed release does not contain this board.",
            )
        })?;
    let provisioning = bridge_provisioning(
        target,
        install_mode,
        wifi_action,
        ssid,
        password,
        tcp_target,
    )
    .map_err(ReleaseAcquisitionError::review_selection)?;
    let request = BridgeRequest::from_target(
        target,
        descriptor.manifest_url(),
        install_mode,
        destructive_confirmation,
        provisioning,
        flash_target,
        &compatibility,
    )?;
    let parts = match (target, &compatibility) {
        (ReleaseTarget::EspSerial(_), ReleaseCompatibility::Esp) => target.parts(),
        (ReleaseTarget::Uf2(target), ReleaseCompatibility::Uf2(softdevice)) => vec![
            ReleasePartRef::Uf2(
                target
                    .variant_for(softdevice)
                    .ok_or_else(|| {
                        ReleaseAcquisitionError::review_selection(format!(
                            "The signed release does not support the detected SoftDevice {softdevice}."
                        ))
                    })?
                    .part(),
            ),
        ],
        (ReleaseTarget::NrfSerialDfu(target), ReleaseCompatibility::Uf2(softdevice))
            if target.compatibility().softdevice() == softdevice =>
        {
            vec![ReleasePartRef::NrfSerialDfu(target.recovery().artifact())]
        }
        (ReleaseTarget::NrfSerialDfu(target), ReleaseCompatibility::NrfSerialDfu(_)) => vec![
            ReleasePartRef::NrfSerialDfu(target.application()),
            ReleasePartRef::NrfSerialDfu(target.init_packet()),
        ],
        _ => {
            return Err(ReleaseAcquisitionError::review_selection(
                "The detected compatibility foundation does not match the selected transport.",
            ))
        }
    };
    let details = ReleaseDetails {
        version: manifest.release().version().as_str().to_string(),
        channel: match manifest.release().channel() {
            ReleaseChannel::Stable => "stable".to_string(),
            ReleaseChannel::Preview => "preview".to_string(),
        },
        total: parts.iter().map(|part| part.size()).sum(),
        parts: parts
            .iter()
            .map(|part| PartDetails {
                kind: part_kind(part.kind()),
                size: part.size(),
                sha256: part.sha256().as_str().to_string(),
            })
            .collect(),
    };
    Ok(AcquiredRelease { request, details })
}

async fn fetch_signed_documents(
    document_url: String,
    document_max_bytes: u64,
    subject: &str,
) -> Result<SignedDocuments, ReleaseAcquisitionError> {
    let mut eval = document::eval(FETCH_SIGNED_DOCUMENTS_SCRIPT);
    eval.send(SignedDocumentRequest {
        document_url,
        document_max_bytes,
        signature_max_bytes: super::contract::response_limits().signature_bytes(),
    })
    .map_err(|_| {
        ReleaseAcquisitionError::browser_retry(format!("Could not request the signed {subject}."))
    })?;
    let response = eval.recv::<SignedDocumentResponse>().await.map_err(|_| {
        ReleaseAcquisitionError::browser_retry(format!("The signed {subject} is unavailable."))
    })?;
    match response {
        SignedDocumentResponse::Ready {
            document,
            signature,
        } => Ok(SignedDocuments {
            document,
            signature,
        }),
        SignedDocumentResponse::Error { error } => {
            let diagnosis = match error {
                SignedDocumentFetchError::Unavailable => {
                    format!("The signed {subject} is unavailable or could not be streamed safely.")
                }
                SignedDocumentFetchError::TooLarge => {
                    format!("The signed {subject} exceeds the browser safety limit.")
                }
                SignedDocumentFetchError::InvalidUtf8 => {
                    format!("The signed {subject} is not valid UTF-8 text.")
                }
            };
            Err(ReleaseAcquisitionError::browser_retry(diagnosis))
        }
    }
}

fn require_exact_manifest_url(version: &str, manifest_url: &str) -> Result<(), String> {
    let expected = format!("https://reticulum.rs/releases/{version}/flash-manifest.json");
    if manifest_url != expected || manifest_url.contains('%') {
        return Err(
            "The signed channel does not name an exact immutable manifest URL.".to_string(),
        );
    }
    Ok(())
}

fn bridge_provisioning(
    target: &ReleaseTarget,
    install_mode: InstallMode,
    action: WifiAction,
    ssid: String,
    password: String,
    tcp_target: Option<String>,
) -> Result<Option<BridgeProvisioning>, String> {
    if install_mode == InstallMode::EraseAll && action == WifiAction::Clear {
        if tcp_target.is_some() {
            return Err("Blank fresh installation cannot configure a TCP client.".to_string());
        }
        return Ok(None);
    }
    let Some(slot) = target.provisioning() else {
        return match action {
            WifiAction::Preserve => Ok(None),
            WifiAction::Configure | WifiAction::Clear => {
                Err("This target does not support Wi-Fi provisioning.".to_string())
            }
        };
    };
    let tcp_client = tcp_target
        .as_deref()
        .map(TcpClientEndpoint::parse)
        .transpose()
        .map_err(|error| error.to_string())?;
    if tcp_client.is_some() && action != WifiAction::Configure {
        return Err("TCP client configuration requires configured Wi-Fi.".to_string());
    }
    if tcp_client.is_some() && slot.tcp_client().is_none() {
        return Err("This signed target does not support TCP client provisioning.".to_string());
    }
    let provisioning_action = match (action, tcp_client.as_ref()) {
        (WifiAction::Preserve, None) => ProvisioningAction::Preserve,
        (WifiAction::Clear, None) => ProvisioningAction::Clear,
        (WifiAction::Preserve | WifiAction::Clear, Some(_)) => {
            return Err("TCP client configuration requires configured Wi-Fi.".to_string())
        }
        (WifiAction::Configure, None) => ProvisioningAction::Configure(WifiCredentials {
            ssid: ssid.clone(),
            password: password.clone(),
        }),
        (WifiAction::Configure, Some(tcp_client)) => ProvisioningAction::ConfigureWithTcp {
            wifi: WifiCredentials {
                ssid: ssid.clone(),
                password: password.clone(),
            },
            tcp_client: tcp_client.clone(),
        },
    };
    provisioning_image(&provisioning_action).map_err(|error| error.to_string())?;
    Ok(Some(BridgeProvisioning {
        action: action.wire().to_string(),
        offset: slot.flash_offset(),
        size: slot.reserved_size_bytes(),
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
        tcp_client: tcp_client.map(|endpoint| match endpoint.host {
            TcpClientHost::Ipv4(address) => bridge::BridgeTcpClient {
                host_kind: "ipv4",
                host: address.to_string(),
                port: endpoint.port,
            },
            TcpClientHost::Hostname(hostname) => bridge::BridgeTcpClient {
                host_kind: "hostname",
                host: hostname,
                port: endpoint.port,
            },
        }),
    }))
}

fn configured_release_channel() -> ReleaseChannel {
    match RELEASE_CHANNEL {
        "stable" => ReleaseChannel::Stable,
        "preview" => ReleaseChannel::Preview,
        _ => panic!("unsupported compiled release channel"),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        bridge_provisioning, require_exact_manifest_url, ReleaseAcquisitionError,
        FETCH_SIGNED_DOCUMENTS_SCRIPT,
    };
    use crate::pages::flash::model::{InstallMode, WifiAction};

    #[test]
    fn manifest_url_must_be_exact_and_normalized() {
        assert!(require_exact_manifest_url(
            "0.2.6",
            "https://reticulum.rs/releases/0.2.6/flash-manifest.json"
        )
        .is_ok());
        for malformed in [
            "https://reticulum.rs/releases/0.2.5/../0.2.6/flash-manifest.json",
            "https://reticulum.rs/releases/%30.2.6/flash-manifest.json",
            "https://reticulum.rs/releases/0.2.6//flash-manifest.json",
            "https://example.test/releases/0.2.6/flash-manifest.json",
        ] {
            assert!(require_exact_manifest_url("0.2.6", malformed).is_err());
        }
    }

    #[test]
    fn unsupported_provisioning_is_rejected_instead_of_discarded(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let manifest = super::super::release_0_2_6_fixture()?;
        let target = manifest
            .targets()
            .iter()
            .find(|target| target.board_id().as_str() == "xiao-esp32-c6")
            .ok_or("missing non-provisionable target")?;

        assert!(bridge_provisioning(
            target,
            InstallMode::PreserveData,
            WifiAction::Preserve,
            String::new(),
            String::new(),
            None,
        )?
        .is_none());
        assert!(bridge_provisioning(
            target,
            InstallMode::PreserveData,
            WifiAction::Configure,
            "network".to_string(),
            "password".to_string(),
            None,
        )
        .is_err());
        assert!(bridge_provisioning(
            target,
            InstallMode::PreserveData,
            WifiAction::Clear,
            String::new(),
            String::new(),
            None,
        )
        .is_err());
        assert!(bridge_provisioning(
            target,
            InstallMode::EraseAll,
            WifiAction::Clear,
            String::new(),
            String::new(),
            None,
        )?
        .is_none());
        Ok(())
    }

    #[test]
    fn release_failures_keep_the_diagnosis_and_an_actionable_safe_path() {
        let trust = ReleaseAcquisitionError::browser_retry("Minisign verification failed");
        let trust_message = trust.to_string();
        assert!(trust_message.contains("Minisign verification failed"));
        assert!(trust_message.contains("Do not connect a device"));
        assert!(trust_message.contains("Reload this page"));
        assert!(trust_message.contains("use the CLI"));

        let selection = ReleaseAcquisitionError::review_selection("Wi-Fi value is too long");
        let selection_message = selection.to_string();
        assert!(selection_message.contains("Wi-Fi value is too long"));
        assert!(selection_message.contains("optional TCP target"));
        assert!(selection_message.contains("No device access has started"));
    }

    #[test]
    fn signed_document_eval_delegates_to_the_bounded_production_reader() {
        assert!(FETCH_SIGNED_DOCUMENTS_SCRIPT.contains("fetchSignedDocuments"));
        assert!(!FETCH_SIGNED_DOCUMENTS_SCRIPT.contains(".text("));
        assert!(!FETCH_SIGNED_DOCUMENTS_SCRIPT.contains(".arrayBuffer("));
    }
}
