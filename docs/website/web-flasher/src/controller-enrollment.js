import { FlashBridgeError } from "./core.js";
import { requireManagedIdentity } from "./nrf-serial-dfu.js";

const REQUEST_BYTES = 68;
const STATUS_BYTES = 69;
const ENROLL_REQUEST = 0x56;
const STATUS_REQUEST = 0x57;
const CONTROL_VALUE = 0x5052;
const CONTROL_INDEX = 0x4e53;
const POLL_MILLIS = 100;
const MAX_POLLS = 100;

export async function requestControllerEnrollment(usb, request, environment, dependencies = {}) {
  const { managedApplication: expected, controllerPublicKey } = request;
  if (!/^[0-9a-f]{128}$/i.test(controllerPublicKey ?? "") || !expected
      || expected.request !== ENROLL_REQUEST || request.statusRequest !== STATUS_REQUEST
      || expected.value !== CONTROL_VALUE || expected.index !== CONTROL_INDEX) {
    throw new FlashBridgeError("invalid_request", "Enter the controller’s complete 128-character public key.");
  }
  const bytes = new Uint8Array(REQUEST_BYTES);
  environment.crypto.getRandomValues(bytes.subarray(0, 4));
  const transaction = new DataView(bytes.buffer).getUint32(0, true);
  for (let index = 0; index < 64; index += 1) {
    bytes[index + 4] = Number.parseInt(controllerPublicKey.slice(index * 2, index * 2 + 2), 16);
  }
  const device = await usb.requestDevice({ filters: [{
    vendorId: expected.usb.vendorId,
    productId: expected.usb.productId,
    serialNumber: expected.serialNumber,
  }] });
  requireManagedIdentity(device, expected);
  const sleep = dependencies.sleep ?? ((ms) => new Promise((resolve) => setTimeout(resolve, ms)));
  try {
    await device.open();
    if (device.configuration === null) {
      if (device.configurations?.length !== 1) {
        throw new FlashBridgeError("ambiguous_device", "Unexpected Hopspot USB configurations.");
      }
      await device.selectConfiguration(device.configurations[0].configurationValue);
    }
    const interfaces = device.configuration?.interfaces ?? [];
    if (interfaces.length !== 1 || interfaces[0].interfaceNumber !== expected.interfaceNumber) {
      throw new FlashBridgeError("ambiguous_device", "Unexpected Hopspot USB interface.");
    }
    await device.claimInterface(expected.interfaceNumber);
    const control = { requestType: "vendor", recipient: "device", value: expected.value, index: expected.index };
    const written = await device.controlTransferOut({ ...control, request: ENROLL_REQUEST }, bytes);
    if (written?.status !== "ok" || written.bytesWritten !== REQUEST_BYTES) {
      throw new FlashBridgeError("connection_failure", "Controller setup was rejected. Install a Hopspot release with USB controller setup support and retry.");
    }
    for (let attempt = 0; attempt < MAX_POLLS; attempt += 1) {
      await sleep(POLL_MILLIS);
      const response = await device.controlTransferIn({ ...control, request: STATUS_REQUEST }, STATUS_BYTES);
      const data = response?.data;
      if (response?.status !== "ok" || data?.byteLength !== STATUS_BYTES) {
        throw new FlashBridgeError("connection_failure", "Could not confirm controller setup. Reconnect and retry with the same public key.");
      }
      if (data.getUint32(1, true) !== transaction) continue;
      const status = data.getUint8(0);
      if (status === 2) {
        return Array.from(new Uint8Array(data.buffer, data.byteOffset + 5, 64),
          (byte) => byte.toString(16).padStart(2, "0")).join("");
      }
      if (status !== 1) {
        throw new FlashBridgeError("connection_failure", "The controller grant could not be saved. Reconnect and retry.");
      }
    }
    throw new FlashBridgeError("connection_failure", "Controller setup has not been confirmed. It may still finish; retry with the same public key after reconnecting.");
  } finally {
    if (device.opened) await device.close().catch(() => {});
  }
}
