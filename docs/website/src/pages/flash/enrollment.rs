use dioxus::prelude::*;
use prns_flash_manifest::{board_catalog, BoardBuild, WebUsbControlRequest};
use serde::{Deserialize, Serialize};

const SCRIPT: &str = r#"
const request = await dioxus.recv();
window.__prnsFlash = window.__prnsFlash || await import('/assets/flasher/prns-flash.js');
return await window.__prnsFlash.enrollController(request);
"#;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EnrollmentRequest {
    managed_application: ManagedApplication,
    controller_public_key: String,
    status_request: u8,
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
    Saved {
        #[serde(rename = "targetPublicKey")]
        target_public_key: String,
    },
    Error {
        message: String,
    },
}

fn request(board_slug: &str, public_key: String) -> Result<EnrollmentRequest, String> {
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
            return Err("This board does not support USB controller setup.".to_string())
        }
    };
    let control = WebUsbControlRequest::CONTROLLER_ENROLLMENT;
    Ok(EnrollmentRequest {
        managed_application: ManagedApplication {
            usb,
            manufacturer,
            product,
            serial_number,
            interface_number,
            request: control.request(),
            value: control.value(),
            index: control.index(),
        },
        controller_public_key: public_key,
        status_request: WebUsbControlRequest::CONTROLLER_ENROLLMENT_STATUS.request(),
    })
}

#[component]
pub(super) fn ControllerEnrollment(
    board_slug: &'static str,
    busy: bool,
    mut active: Signal<bool>,
    on_start: EventHandler<()>,
) -> Element {
    let mut key = use_signal(String::new);
    let mut status = use_signal(String::new);
    let mut target_key = use_signal(String::new);
    let valid = key().len() == 128 && key().bytes().all(|byte| byte.is_ascii_hexdigit());
    rsx! {
        section {
            class: "flash-recovery-panel mt-5 rounded-card border border-line/60 bg-layer/40 p-5 text-sm text-soft",
            "aria-labelledby": "flash-controller-title",
            h2 { id: "flash-controller-title", class: "text-lg font-semibold text-paper", "Connect your controller" }
            p { class: "mt-2", "After installing Hopspot, connect this board by USB and authorize your controller to manage it, including its radio settings." }
            label { r#for: "controller-public-key", class: "mt-4 block font-semibold", "Controller public key" }
            input {
                id: "controller-public-key", r#type: "text", autocomplete: "off", spellcheck: "false",
                class: "mt-2 w-full rounded-lg border border-line bg-layer p-3 font-mono text-xs text-paper",
                placeholder: "128 hexadecimal characters", value: "{key}", disabled: busy || active(),
                oninput: move |event| key.set(event.value().trim().to_string()),
            }
            p { class: "mt-2 text-xs", "This grants administrator access to that controller. Use its public key; keep its private identity on your controller." }
            button {
                r#type: "button",
                class: "mt-4 rounded-lg border border-line px-4 py-3 font-semibold text-paper disabled:opacity-50",
                disabled: busy || active() || !valid,
                onclick: move |_| {
                    let request = match request(board_slug, key()) {
                        Ok(request) => request,
                        Err(message) => { status.set(message); return; }
                    };
                    target_key.set(String::new());
                    on_start.call(());
                    active.set(true);
                    status.set("Select your Hopspot board in the USB picker…".to_string());
                    spawn(async move {
                        let bridge = document::eval(SCRIPT);
                        let result = if bridge.send(request).is_err() {
                            Err("Could not start controller setup.".to_string())
                        } else {
                            match bridge.join::<Outcome>().await {
                                Ok(Outcome::Saved { target_public_key }) => Ok(target_public_key),
                                Ok(Outcome::Error { message }) => Err(message),
                                Err(_) => Err("Controller setup could not be confirmed. Reconnect and retry with the same public key.".to_string()),
                            }
                        };
                        status.set(match result {
                            Ok(public_key) => {
                                target_key.set(public_key);
                                "Controller authorized and saved. You can now connect from that controller.".to_string()
                            },
                            Err(message) => message,
                        });
                        active.set(false);
                    });
                },
                "Authorize controller over USB"
            }
            p { class: "mt-2 text-xs", "Use desktop Chrome or Edge with Hopspot 0.3.8 or later running on the board. Close other apps using its USB connection first." }
            if !target_key().is_empty() {
                p { class: "mt-4 text-xs", "Board public key — save this in your controller to identify this board:" }
                code { class: "mt-2 block break-all text-xs text-paper", "{target_key}" }
            }
            if !status().is_empty() {
                p { class: "mt-3 text-xs", role: "status", "aria-live": "polite", "{status}" }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enrollment_selects_each_boards_application_identity_and_canonical_usb_contract() {
        let catalog = board_catalog().unwrap();
        for board in catalog.shipping_boards() {
            let request = request(&board.slug, "ab".repeat(64));
            if matches!(board.build, BoardBuild::Esp(_)) {
                assert!(request.is_err());
                continue;
            }
            let request = request.unwrap();
            assert_eq!(request.managed_application.request, 0x56);
            assert_eq!(request.status_request, 0x57);
            assert_eq!(request.managed_application.value, 0x5052);
            assert_eq!(request.managed_application.index, 0x4e53);
            assert_eq!(request.managed_application.usb.vendor_id, 0x1209);
            assert!(request.managed_application.product.contains("Hopspot"));
        }
    }
}
