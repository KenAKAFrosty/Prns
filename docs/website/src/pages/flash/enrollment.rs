use dioxus::prelude::*;
use prns_core::remote_control::{
    parse_controller_public_keys, RemoteControlControllerAuthority, RemoteControlControllerCard,
    RemoteControlControllerIdentity, RemoteControlRequestSet, RemoteControlTargetAccess,
    RemoteControlTargetAccessCard, RemoteControlTargetIdentity,
};
use prns_flash_manifest::{board_catalog, BoardBuild, WebUsbControlRequest};
use serde::{Deserialize, Serialize};

const SCRIPT: &str = r#"
try {
  const request = await dioxus.recv();
  window.__prnsFlash = window.__prnsFlash || await import('/assets/flasher/prns-flash.js');
  const outcome = await window.__prnsFlash.enrollController(request, {
    onProgress: event => dioxus.send(event),
  });
  await dioxus.send(outcome);
} catch (_) {
  // recv() does not observe script failures. Always send a terminal event.
  // The operation owns its cleanup; an old script must not stop a newer one.
  await dioxus.send({ status: 'unknown', message: 'Setup could not be confirmed. Check setup status before authorizing again.' });
}
"#;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EnrollmentRequest {
    transport: &'static str,
    board_slug: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    managed_application: Option<ManagedApplication>,
    controller_public_key: String,
    status_request: u8,
    action: EnrollmentAction,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "kebab-case")]
enum EnrollmentAction {
    Authorize,
    CheckStatus,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ManagedApplication {
    usb: UsbIdentity,
    manufacturer: String,
    product: String,
    serial_number: String,
    interface_number: u8,
    request: u8,
    value: u16,
    index: u16,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UsbIdentity {
    vendor_id: u16,
    product_id: u16,
}

#[derive(Deserialize)]
#[serde(tag = "status", rename_all = "kebab-case")]
enum Outcome {
    Waiting {
        message: String,
        #[serde(rename = "canContinue")]
        can_continue: bool,
        paused: bool,
    },
    Saved {
        #[serde(rename = "targetPublicKey")]
        target_public_key: String,
    },
    Error {
        message: String,
    },
    Unknown {
        message: String,
    },
    Pending {
        message: String,
    },
}

fn public_key_bytes(text: &str) -> Result<[u8; 64], String> {
    if text.len() != 128 || !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(
            "Enter a controller public card or its complete 128-character public key.".into(),
        );
    }
    let mut bytes = [0; 64];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16)
            .map_err(|_| "Invalid public key.".to_string())?;
    }
    Ok(bytes)
}

fn controller_from_input(text: &str) -> Result<RemoteControlControllerIdentity, String> {
    let text = text.trim();
    let card = if text.starts_with("prns-rc:") {
        RemoteControlControllerCard::decode_text(text)
            .map_err(|_| "This is not a valid controller public card.".to_string())?
    } else {
        let bytes = public_key_bytes(text)?;
        let identity = parse_controller_public_keys(&bytes)
            .ok_or_else(|| "Invalid controller public identity.".to_string())?;
        RemoteControlControllerCard::new(identity)
            .map_err(|_| "Invalid controller public identity.".to_string())?
    };
    Ok(*card.controller())
}

fn controller_key(controller: &RemoteControlControllerIdentity) -> String {
    controller
        .public_keys()
        .public_key_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn saved_target_card(
    controller: RemoteControlControllerIdentity,
    target_key: &str,
) -> Result<String, String> {
    let target_bytes = public_key_bytes(target_key)
        .map_err(|_| "The board returned an invalid public identity.".to_string())?;
    let public = parse_controller_public_keys(&target_bytes)
        .ok_or_else(|| "The board returned an invalid public identity.".to_string())?;
    let access = RemoteControlTargetAccess::new(
        RemoteControlTargetIdentity::new(*public.public_keys()),
        RemoteControlControllerAuthority::Administrator,
        RemoteControlRequestSet::all(),
    )
    .map_err(|_| "The board access could not be represented.".to_string())?;
    let card = RemoteControlTargetAccessCard::new(controller, access)
        .map_err(|_| "The board returned an invalid public identity.".to_string())?;
    let mut buffer = [0; RemoteControlTargetAccessCard::MAX_TEXT_LEN];
    let len = card
        .encode_text(&mut buffer)
        .map_err(|_| "The board access card could not be encoded.".to_string())?;
    String::from_utf8(buffer[..len].to_vec())
        .map_err(|_| "The board access card could not be encoded.".to_string())
}

fn request(
    board_slug: &str,
    controller: &RemoteControlControllerIdentity,
    action: EnrollmentAction,
) -> Result<EnrollmentRequest, String> {
    let catalog = board_catalog().map_err(|error| error.to_string())?;
    let board = catalog.board(board_slug).ok_or("Unknown board")?;
    let (usb, manufacturer, product, serial_number, interface_number) = match &board.build {
        BoardBuild::Uf2(build) => {
            let app = &build.application_usb;
            let parse = |value: &str| {
                u16::from_str_radix(value.trim_start_matches("0x"), 16)
                    .map_err(|error| error.to_string())
            };
            (
                UsbIdentity {
                    vendor_id: parse(&app.usb.vendor_id)?,
                    product_id: parse(&app.usb.product_id)?,
                },
                app.manufacturer.clone(),
                app.product.clone(),
                app.serial_number.clone(),
                0,
            )
        }
        BoardBuild::NrfSerialDfu(build) => {
            let serial = build
                .serial
                .clone()
                .into_validated()
                .map_err(|error| error.to_string())?;
            let usb = serial.managed_application_usb();
            (
                UsbIdentity {
                    vendor_id: usb.vendor_id(),
                    product_id: usb.product_id(),
                },
                serial.managed_application_manufacturer().to_string(),
                serial.managed_application_product().to_string(),
                serial.managed_application_serial_number().to_string(),
                serial.managed_application_interface_number(),
            )
        }
        BoardBuild::Esp(_) => {
            return Ok(EnrollmentRequest {
                transport: "esp-serial",
                board_slug: board_slug.to_string(),
                managed_application: None,
                controller_public_key: controller_key(controller),
                status_request: WebUsbControlRequest::CONTROLLER_ENROLLMENT_STATUS.request(),
                action,
            });
        }
    };
    let control = WebUsbControlRequest::CONTROLLER_ENROLLMENT;
    Ok(EnrollmentRequest {
        transport: "webusb",
        board_slug: board_slug.to_string(),
        managed_application: Some(ManagedApplication {
            usb,
            manufacturer,
            product,
            serial_number,
            interface_number,
            request: control.request(),
            value: control.value(),
            index: control.index(),
        }),
        controller_public_key: controller_key(controller),
        status_request: WebUsbControlRequest::CONTROLLER_ENROLLMENT_STATUS.request(),
        action,
    })
}

#[derive(Clone, Default)]
struct EnrollmentState {
    operation_key: String,
    message: String,
    target_card: String,
    waiting: bool,
    can_continue: bool,
    pending: bool,
}

impl EnrollmentState {
    fn waiting(&mut self, message: String, can_continue: bool, paused: bool) {
        self.message = message;
        self.waiting = true;
        self.can_continue = can_continue || paused;
        self.pending = true;
    }
}

fn continue_checking() {
    document::eval("window.__prnsFlash?.continueControllerEnrollment();");
}

fn stop_checking() {
    document::eval("window.__prnsFlash?.stopControllerEnrollmentCheck();");
}

fn run_enrollment(
    board_slug: &'static str,
    input: String,
    action: EnrollmentAction,
    mut state: Signal<EnrollmentState>,
    mut active: Signal<bool>,
    on_start: EventHandler<()>,
) {
    let controller = match controller_from_input(&input) {
        Ok(controller) => controller,
        Err(message) => {
            state.write().message = message;
            return;
        }
    };
    let request = match request(board_slug, &controller, action) {
        Ok(request) => request,
        Err(message) => {
            state.write().message = message;
            return;
        }
    };
    state.set(EnrollmentState {
        operation_key: format!("{board_slug}:{}", controller_key(&controller)),
        message: "Select your Hopspot board in the USB picker…".to_string(),
        target_card: String::new(),
        ..Default::default()
    });
    on_start.call(());
    active.set(true);
    spawn(async move {
        let mut bridge = document::eval(SCRIPT);
        let outcome = if bridge.send(request).is_err() {
            Outcome::Unknown {
                message:
                    "Setup could not be confirmed. Check setup status before authorizing again."
                        .to_string(),
            }
        } else {
            loop {
                match bridge.recv::<Outcome>().await {
                    Ok(Outcome::Waiting {
                        message,
                        can_continue,
                        paused,
                    }) => {
                        state.write().waiting(message, can_continue, paused);
                    }
                    Ok(outcome) => break outcome,
                    Err(_) => {
                        stop_checking();
                        break Outcome::Unknown {
                            message: "Setup could not be confirmed. Check setup status before authorizing again."
                                .to_string(),
                        };
                    }
                }
            }
        };
        let mut result = state.write();
        result.waiting = false;
        result.can_continue = false;
        result.pending = matches!(outcome, Outcome::Pending { .. });
        match outcome {
            Outcome::Saved { target_public_key } => {
                match saved_target_card(controller, &target_public_key) {
                    Ok(card) => {
                        result.target_card = card;
                        result.message = "This setup was saved on the board. Import its access card into your controller, then verify current access.".to_string();
                    }
                    Err(message) => result.message = message,
                }
            }
            Outcome::Error { message }
            | Outcome::Unknown { message }
            | Outcome::Pending { message } => result.message = message,
            Outcome::Waiting { .. } => unreachable!("progress is consumed before completion"),
        }
        drop(result);
        active.set(false);
    });
}

#[component]
pub(super) fn ControllerEnrollment(
    board_slug: &'static str,
    busy: bool,
    active: Signal<bool>,
    on_start: EventHandler<()>,
) -> Element {
    use_drop(stop_checking);
    let mut key = use_signal(String::new);
    let state = use_signal(EnrollmentState::default);
    let controller = controller_from_input(&key());
    let valid = controller.is_ok();
    let recipient_fingerprint = controller.as_ref().ok().map(|controller| {
        controller
            .identity_hash()
            .as_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    });
    let current_attempt = controller.is_ok_and(|controller| {
        state.read().operation_key == format!("{board_slug}:{}", controller_key(&controller))
    });
    rsx! {
        section {
            class: "flash-recovery-panel mt-5 rounded-card border border-line/60 bg-layer/40 p-5 text-sm text-soft",
            "aria-labelledby": "flash-controller-title",
            h2 { id: "flash-controller-title", class: "text-lg font-semibold text-paper", "Connect your controller" }
            p { class: "mt-2", "After installing Hopspot, connect this board by USB and authorize your controller to manage it, including its radio settings." }
            label { r#for: "controller-public-key", class: "mt-4 block font-semibold", "Controller public card or key" }
            input {
                id: "controller-public-key", r#type: "text", autocomplete: "off", spellcheck: "false",
                class: "mt-2 w-full rounded-lg border border-line bg-layer p-3 font-mono text-xs text-paper",
                placeholder: "prns-rc:… or 128-character public key", value: "{key}", disabled: busy || active(),
                oninput: move |event| key.set(event.value().trim().to_string()),
            }
            p { class: "mt-2 text-xs", "Paste the controller’s public card or key; keep its private identity on your controller. Radio settings are not changed during authorization." }
            if let Some(fingerprint) = recipient_fingerprint {
                p { class: "mt-4 text-xs", "Recipient controller fingerprint — compare this with the controller you intend to authorize:" }
                code { class: "mt-2 block break-all text-xs text-paper", "{fingerprint}" }
                p { class: "mt-2 text-xs", "This USB setup grants Administrator authority with the full request set. This includes every remote-control operation supported by this firmware, such as changing settings and adding or revoking controller grants." }
            }
            button {
                r#type: "button",
                class: "mt-4 rounded-lg border border-line px-4 py-3 font-semibold text-paper disabled:opacity-50",
                disabled: busy || active() || !valid,
                onclick: move |_| run_enrollment(board_slug, key(), EnrollmentAction::Authorize, state, active, on_start),
                if current_attempt { "Start a new authorization" } else { "Authorize controller over USB" }
            }
            if current_attempt {
                if active() && state.read().waiting {
                    if state.read().can_continue {
                        button {
                            r#type: "button",
                            class: "ml-3 mt-4 rounded-lg border border-line px-4 py-3 font-semibold text-paper",
                            onclick: move |_| continue_checking(),
                            "Continue checking"
                        }
                    }
                    button {
                        r#type: "button",
                        class: "ml-3 mt-4 rounded-lg border border-line px-4 py-3 font-semibold text-paper",
                        onclick: move |_| stop_checking(),
                        "Stop checking"
                    }
                    p { class: "mt-2 text-xs", "This page is keeping the USB connection open. Continuing only reads status on the same connection. Stopping releases the connection; it does not cancel the board’s setup request." }
                } else {
                  button {
                    r#type: "button",
                    class: "ml-3 mt-4 rounded-lg border border-line px-4 py-3 font-semibold text-paper disabled:opacity-50",
                    disabled: busy || active(),
                    onclick: move |_| run_enrollment(board_slug, key(), EnrollmentAction::CheckStatus, state, active, on_start),
                    if state.read().pending { "Continue checking" } else { "Check setup status" }
                  }
                }
                p { class: "mt-2 text-xs", "Checking only reads the original attempt’s result. A new authorization can restore access that was revoked. A saved result records setup completion; verify current access from your controller." }
            }
            p { class: "mt-2 text-xs", "Use desktop Chrome or Edge with firmware that supports USB controller setup running on the board. Choose its running USB connection, and close other apps using that connection first. Setup does not enter the bootloader or flash firmware." }
            if current_attempt && !state.read().target_card.is_empty() {
                p { class: "mt-4 text-xs", "Board access card — import this into the controller whose public card or key you used above:" }
                code { class: "mt-2 block break-all text-xs text-paper", "{state.read().target_card}" }
            }
            if !state.read().message.is_empty() {
                p { class: "mt-3 text-xs", role: "status", "aria-live": "polite", "{state.read().message}" }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prns_core::identity::{
        in_memory::InMemoryNodeIdentity, IdentityPublicKeys, IdentitySigner,
    };

    fn identity(seed: u8) -> RemoteControlControllerIdentity {
        let identity = InMemoryNodeIdentity::from_secret_key_bytes(&[seed; 64]);
        RemoteControlControllerIdentity::new(IdentityPublicKeys {
            encryption: identity.encryption_public_key(),
            signing: identity.signing_public_key(),
        })
    }

    #[test]
    fn waiting_progress_keeps_pending_separate_from_saved_access() {
        let mut state = EnrollmentState::default();
        let event: Outcome = serde_json::from_str(
            r#"{"status":"waiting","message":"Still starting","canContinue":false,"paused":true}"#,
        )
        .unwrap();
        let Outcome::Waiting {
            message,
            can_continue,
            paused,
        } = event
        else {
            panic!("expected waiting progress");
        };
        state.waiting(message, can_continue, paused);
        assert!(state.waiting);
        assert!(state.pending);
        assert!(state.can_continue);
        assert!(state.target_card.is_empty());
        assert!(matches!(
            serde_json::from_str::<Outcome>(r#"{"status":"pending","message":"Checking stopped"}"#)
                .unwrap(),
            Outcome::Pending { .. }
        ));
        assert!(serde_json::from_str::<Outcome>(r#"{"status":"saved"}"#).is_err());
    }

    #[test]
    fn controller_cards_and_legacy_keys_resolve_to_the_same_validated_identity() {
        let controller = identity(11);
        let card = RemoteControlControllerCard::new(controller).unwrap();
        let mut bytes = [0; RemoteControlControllerCard::MAX_TEXT_LEN];
        let len = card.encode_text(&mut bytes).unwrap();
        let text = std::str::from_utf8(&bytes[..len]).unwrap();
        assert_eq!(controller_from_input(text).unwrap(), controller);
        assert_eq!(
            controller_from_input(&controller_key(&controller).to_uppercase()).unwrap(),
            controller
        );
        assert!(controller_from_input(&"00".repeat(64)).is_err());
        assert!(controller_from_input(&"ab".repeat(32)).is_err());
        let target_card = saved_target_card(controller, &controller_key(&identity(12))).unwrap();
        assert!(controller_from_input(&target_card).is_err());
    }

    #[test]
    fn saved_target_card_is_bound_to_the_controller_and_records_usb_authority() {
        let controller = identity(11);
        let target = identity(12);
        let text = saved_target_card(controller, &controller_key(&target)).unwrap();
        let card = RemoteControlTargetAccessCard::decode_text(&text).unwrap();
        let access = card.into_access_for(&controller).unwrap();
        assert_eq!(access.target().public_keys(), target.public_keys());
        assert_eq!(
            access.authority(),
            RemoteControlControllerAuthority::Administrator
        );
        assert_eq!(access.permitted_requests(), &RemoteControlRequestSet::all());
        assert!(RemoteControlTargetAccessCard::decode_text(&text)
            .unwrap()
            .into_access_for(&identity(13))
            .is_err());
        assert!(saved_target_card(controller, &"00".repeat(64)).is_err());
    }

    #[test]
    fn enrollment_selects_each_boards_application_identity_and_canonical_usb_contract() {
        let catalog = board_catalog().unwrap();
        for board in catalog.shipping_boards() {
            let request = request(&board.slug, &identity(11), EnrollmentAction::Authorize);
            if matches!(board.build, BoardBuild::Esp(_)) {
                let request = request.unwrap();
                assert_eq!(request.transport, "esp-serial");
                assert!(request.managed_application.is_none());
                assert_eq!(request.board_slug, board.slug);
                continue;
            }
            let request = request.unwrap();
            assert_eq!(request.transport, "webusb");
            let application = request.managed_application.unwrap();
            assert_eq!(application.request, 0x56);
            assert_eq!(request.status_request, 0x57);
            assert_eq!(application.value, 0x5052);
            assert_eq!(application.index, 0x4e53);
            assert_eq!(application.usb.vendor_id, 0x1209);
            assert!(application.product.contains("Hopspot"));
        }
    }
}
