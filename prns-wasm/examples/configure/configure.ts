// Headless config webUI entry module for `/configure`. Loaded by the Dioxus
// page via `document::eval` (see `docs/website/src/pages/configure/bridge.rs`),
// which injects a `dioxus` object exposing `recv()` / `send()`. This module owns
// the WebUSB session + the wasm codec; the Rust side owns rendering and sends
// typed requests, this side sends typed events back.
//
// It mirrors the browser-playground entry (`prns-wasm/examples/browser-playground`)
// rather than the web-flasher esbuild bundle, because — unlike the flasher,
// which is pure JS over Web Serial — the config lane needs the prns-wasm codec
// (frame encode/decode, snapshot decode). wasm-bindgen `--target web` output
// cannot be esbuild-bundled (the `.wasm` is a sibling asset resolved via
// `import.meta.url`), so the staged layout is: this entry + the tsc-emitted
// prns-js browser modules under `sdk/` + the wasm-bindgen pkg under `pkg/`.
// See `tools/build/stage-configure-asset.sh`.

import init, * as wasm from "./pkg/prns_wasm.js";
import { UsbAutoConfigInterface } from "./sdk/index.js";
import type {
  InterfaceSessionFailure,
  PrnsWasmModule,
  Tag,
  UsbAutoConfigActionOutcome,
  UsbAutoConfigConnectOutcome,
  UsbAutoConfigResult,
  UsbAutoConfigSnapshotOutcome,
  UsbAutoSnapshot,
} from "./sdk/index.js";

// Resolved relative to this module's URL (`/assets/configure/configure.js`), not
// the page URL (`/configure`), so the wasm + glue load from the hosted asset
// tree regardless of the Dioxus route depth.
const WASM_BINARY_PATH = new URL("./pkg/prns_wasm_bg.wasm", import.meta.url);

// The request envelope Rust sends. Mirror the `ConfigureRequest` serde type in
// `bridge.rs` (camelCase, deny_unknown_fields).
type ConfigureRequest =
  | { kind: "ready" }
  | { kind: "connect" }
  | { kind: "snapshot" }
  | {
      kind: "applySetLoRaProfile";
      frequencyHz: number;
      spreadingFactor: number;
      bandwidth: number;
      codingRate: number;
      txPowerDbm: number;
      preamble: number;
      regionCode: number;
    }
  | { kind: "applyResetLoRaProfile" }
  | { kind: "applyToggleInterface"; interfaceCode: number }
  | { kind: "applySleep" }
  | { kind: "applyWake" }
  | { kind: "applyAnnounce" }
  | { kind: "close" };

// The event envelope this side sends back via `dioxus.send(...)`. Mirror the
// `ConfigureEvent` serde type in `bridge.rs`.
type ConfigureEvent =
  | { kind: "ready"; supported: boolean; reason?: string }
  | { kind: "connected" }
  | { kind: "connectFailed"; code: string; detail: string }
  | { kind: "snapshot"; snapshot: UsbAutoSnapshot }
  | { kind: "snapshotFailed"; code: string; detail: string }
  | { kind: "applied"; result: UsbAutoConfigResult }
  | { kind: "applyFailed"; code: string; detail: string }
  | { kind: "closed" }
  | { kind: "sessionFailed"; code: string; detail: string };

// The connected session handle. Derived from the connect outcome so this
// module does not depend on `BrowserUsbAutoConfigSession` being re-exported.
type ConfigSession = Extract<
  UsbAutoConfigConnectOutcome,
  Tag<"Connected", unknown>
>["data"];

let session: ConfigSession | undefined;
let wasmReady = false;

// Minimal `PrnsWasmModule` shaped object — `UsbAutoConfigInterface` only touches
// the usb-auto config exports, so the rest of the interface is left unset via
// the same `as unknown as PrnsWasmModule` cast the playground uses. Runtime only
// reads the fields below.
function wasmModule(): PrnsWasmModule {
  return {
    UsbAutoDecoder: wasm.UsbAutoDecoder,
    usbAutoWebUsbVendorId: wasm.usbAutoWebUsbVendorId,
    usbAutoWebUsbProductId: wasm.usbAutoWebUsbProductId,
    usbAutoHostHelloFrame: wasm.usbAutoHostHelloFrame,
    usbAutoHostHelloAckFrame: wasm.usbAutoHostHelloAckFrame,
    usbAutoConfigRequestFrame: wasm.usbAutoConfigRequestFrame,
    usbAutoSnapshotDecode: wasm.usbAutoSnapshotDecode,
    usbAutoConfigActionRequestSnapshot: wasm.usbAutoConfigActionRequestSnapshot,
    usbAutoConfigActionSetLoRaProfile: wasm.usbAutoConfigActionSetLoRaProfile,
    usbAutoConfigActionResetLoRaProfile: wasm.usbAutoConfigActionResetLoRaProfile,
    usbAutoConfigActionToggleInterface: wasm.usbAutoConfigActionToggleInterface,
    usbAutoConfigActionSleep: wasm.usbAutoConfigActionSleep,
    usbAutoConfigActionWake: wasm.usbAutoConfigActionWake,
    usbAutoConfigActionAnnounce: wasm.usbAutoConfigActionAnnounce,
  } as unknown as PrnsWasmModule;
}

async function ensureWasm(): Promise<void> {
  if (wasmReady) {
    return;
  }
  await init({ module_or_path: WASM_BINARY_PATH });
  wasmReady = true;
}

function webUsbSupported(): { supported: true } | { supported: false; reason: string } {
  if (!globalThis.isSecureContext) {
    return { supported: false, reason: "HTTPS required for WebUSB" };
  }
  if (typeof navigator === "undefined" || !(("usb" in navigator))) {
    return { supported: false, reason: "WebUSB not available in this browser" };
  }
  return { supported: true };
}

// Map any `InterfaceSessionFailure` onto the wire `{code, detail}` the Rust side
// renders. A plain `switch` (not casework `match`) so the mapping stays robust
// to new failure variants — the default arm covers anything the session emits
// that this UI does not render specially.
function sessionFailureCode(
  failure: InterfaceSessionFailure,
): { code: string; detail: string } {
  switch (failure.tag) {
    case "Disconnected":
      return { code: "disconnected", detail: failure.data.detail };
    case "TransferFailed":
      return { code: "transferFailed", detail: failure.data.detail };
    case "ProtocolViolation":
      return { code: "protocolViolation", detail: failure.data.detail };
    case "UnsupportedFrame":
      return { code: "unsupportedFrame", detail: `format ${failure.data.format}` };
    case "FrameTooLarge":
      return {
        code: "frameTooLarge",
        detail: `${failure.data.length}/${failure.data.maximum} bytes`,
      };
    case "OutboundQueueFull":
      return {
        code: "outboundQueueFull",
        detail: `capacity ${failure.data.capacity}`,
      };
    case "CloseFailed":
      return { code: "closeFailed", detail: "session close failed" };
    case "UnexpectedSessionFailure":
      return { code: "unexpectedSessionFailure", detail: failure.data.detail };
    default:
      return { code: "sessionFailure", detail: String(failure) };
  }
}

function connectOutcomeToEvent(
  outcome: UsbAutoConfigConnectOutcome,
): ConfigureEvent {
  switch (outcome.tag) {
    case "Connected":
      session = outcome.data;
      return { kind: "connected" };
    case "HostApiUnavailable":
      return {
        kind: "connectFailed",
        code: "hostApiUnavailable",
        detail: `${outcome.data.api} unavailable`,
      };
    case "PermissionDenied":
      return { kind: "connectFailed", code: "permissionDenied", detail: outcome.data.detail };
    case "Cancelled":
      return {
        kind: "connectFailed",
        code: "cancelled",
        detail: "device selection cancelled",
      };
    case "UnsupportedDevice":
      return {
        kind: "connectFailed",
        code: "unsupportedDevice",
        detail: outcome.data.capability,
      };
    case "ConnectionFailed":
      return { kind: "connectFailed", code: "connectionFailed", detail: outcome.data.detail };
    default:
      return { kind: "connectFailed", code: "connectFailed", detail: String(outcome) };
  }
}

function snapshotOutcomeToEvent(
  outcome: UsbAutoConfigSnapshotOutcome,
): ConfigureEvent {
  if (outcome.tag === "Snapshot") {
    return { kind: "snapshot", snapshot: outcome.data };
  }
  const { code, detail } = sessionFailureCode(outcome);
  return { kind: "snapshotFailed", code, detail };
}

function actionOutcomeToEvent(
  outcome: UsbAutoConfigActionOutcome,
): ConfigureEvent {
  if (outcome.tag === "Result") {
    return { kind: "applied", result: outcome.data };
  }
  const { code, detail } = sessionFailureCode(outcome);
  return { kind: "applyFailed", code, detail };
}

async function dispatch(request: ConfigureRequest): Promise<ConfigureEvent> {
  switch (request.kind) {
    case "ready": {
      const probe = webUsbSupported();
      return probe.supported
        ? { kind: "ready", supported: true }
        : { kind: "ready", supported: false, reason: probe.reason };
    }
    case "connect": {
      await ensureWasm();
      const iface = new UsbAutoConfigInterface(wasmModule());
      const outcome = await iface.connect();
      return connectOutcomeToEvent(outcome);
    }
    case "snapshot": {
      if (!session) {
        return {
          kind: "snapshotFailed",
          code: "notConnected",
          detail: "no config session",
        };
      }
      const outcome = await session.requestSnapshot();
      return snapshotOutcomeToEvent(outcome);
    }
    case "applySetLoRaProfile": {
      if (!session) {
        return {
          kind: "applyFailed",
          code: "notConnected",
          detail: "no config session",
        };
      }
      const action = wasmModule().usbAutoConfigActionSetLoRaProfile(
        request.frequencyHz,
        request.spreadingFactor,
        request.bandwidth,
        request.codingRate,
        request.txPowerDbm,
        request.preamble,
        request.regionCode,
      );
      const outcome = await session.sendAction(action);
      return actionOutcomeToEvent(outcome);
    }
    case "applyResetLoRaProfile": {
      if (!session) {
        return {
          kind: "applyFailed",
          code: "notConnected",
          detail: "no config session",
        };
      }
      const outcome = await session.sendAction(
        wasmModule().usbAutoConfigActionResetLoRaProfile(),
      );
      return actionOutcomeToEvent(outcome);
    }
    case "applyToggleInterface": {
      if (!session) {
        return {
          kind: "applyFailed",
          code: "notConnected",
          detail: "no config session",
        };
      }
      const outcome = await session.sendAction(
        wasmModule().usbAutoConfigActionToggleInterface(request.interfaceCode),
      );
      return actionOutcomeToEvent(outcome);
    }
    case "applySleep": {
      if (!session) {
        return {
          kind: "applyFailed",
          code: "notConnected",
          detail: "no config session",
        };
      }
      const outcome = await session.sendAction(wasmModule().usbAutoConfigActionSleep());
      return actionOutcomeToEvent(outcome);
    }
    case "applyWake": {
      if (!session) {
        return {
          kind: "applyFailed",
          code: "notConnected",
          detail: "no config session",
        };
      }
      const outcome = await session.sendAction(wasmModule().usbAutoConfigActionWake());
      return actionOutcomeToEvent(outcome);
    }
    case "applyAnnounce": {
      if (!session) {
        return {
          kind: "applyFailed",
          code: "notConnected",
          detail: "no config session",
        };
      }
      const outcome = await session.sendAction(
        wasmModule().usbAutoConfigActionAnnounce(),
      );
      return actionOutcomeToEvent(outcome);
    }
    case "close": {
      if (session) {
        await session.close();
        session = undefined;
      }
      return { kind: "closed" };
    }
  }
}

// The Dioxus page drives the lane one action at a time: per user action it
// evals a one-shot script that imports this module (cached on `window` after
// the first load) and `await`s `dispatch(request)`, returning the event as the
// script's value:
//   const mod = window.__prnsConfigure || await import('/assets/configure/configure.js');
//   return await mod.dispatch({ kind: "snapshot" });
// This keeps the Rust side stateless across actions (no persistent eval
// handle); the WebUSB session handle lives in this module's `session` slot.
async function dispatchRequest(request: ConfigureRequest): Promise<ConfigureEvent> {
  try {
    return await dispatch(request);
  } catch (error: unknown) {
    return {
      kind: "sessionFailed",
      code: "unexpected",
      detail: error instanceof Error ? `${error.name}: ${error.message}` : String(error),
    };
  }
}

export { dispatchRequest as dispatch };

// Expose on `window` so the eval'd bootstrap script can reach the dispatcher
// without relying on the ESM module's return value shape across `document::eval`.
declare global {
  interface Window {
    __prnsConfigure?: { dispatch: typeof dispatchRequest };
  }
}
window.__prnsConfigure = { dispatch: dispatchRequest };