import { FlashBridgeError } from "./core.js";
import { requireManagedIdentity } from "./nrf-serial-dfu.js";
import { createEnrollmentWait, ENROLLMENT_POLL_MILLIS } from "./enrollment-wait.js";

const REQUEST_BYTES = 68;
const STATUS_BYTES = 69;
const ENROLL_REQUEST = 0x56;
const STATUS_REQUEST = 0x57;
const CONTROL_VALUE = 0x5052;
const CONTROL_INDEX = 0x4e53;
// Public setup metadata only, retained across USB reconnects while this page is open.
// Checking an uncertain operation never writes its grant again.
// The existing nRF firmware does not deduplicate authorization writes; recovery
// is read-only here, rather than relying on firmware replay protection.
const enrollmentSession = { operation: null };

function requestIdentity(request) {
  const expected = request.managedApplication;
  return JSON.stringify([
    expected.usb.vendorId, expected.usb.productId, expected.manufacturer,
    expected.product, expected.serialNumber, expected.interfaceNumber,
    request.controllerPublicKey.toLowerCase(),
  ]);
}

function unknown(message) {
  return { status: "unknown", message };
}

async function readStatus(device, control) {
  const response = await device.controlTransferIn({ ...control, request: STATUS_REQUEST }, STATUS_BYTES);
  const data = response?.data;
  if (response?.status !== "ok" || data?.byteLength !== STATUS_BYTES || data.getUint8(0) > 3) {
    throw new FlashBridgeError("connection_failure", "Could not read the board’s setup status.");
  }
  return {
    status: data.getUint8(0),
    transaction: data.getUint32(1, true),
    targetPublicKey: Array.from(new Uint8Array(data.buffer, data.byteOffset + 5, 64),
      (byte) => byte.toString(16).padStart(2, "0")).join(""),
  };
}

export async function requestControllerEnrollment(usb, request, environment, dependencies = {}) {
  const { managedApplication: expected, controllerPublicKey, action } = request;
  if (!/^[0-9a-f]{128}$/i.test(controllerPublicKey ?? "") || !expected
      || expected.request !== ENROLL_REQUEST || request.statusRequest !== STATUS_REQUEST
      || expected.value !== CONTROL_VALUE || expected.index !== CONTROL_INDEX
      || !["authorize", "check-status"].includes(action)) {
    throw new FlashBridgeError("invalid_request", "Enter the controller’s complete 128-character public key.");
  }
  const session = dependencies.enrollmentSession ?? enrollmentSession;
  const identity = requestIdentity(request);
  let operation = action === "check-status" ? session.operation : null;
  if (action === "check-status" && (!operation || operation.identity !== identity)) {
    return unknown("No setup attempt for this controller and board is retained on this page. Check access from your controller before starting a new authorization.");
  }
  const sleep = dependencies.sleep ?? ((ms) => new Promise((resolve) => setTimeout(resolve, ms)));
  const wait = dependencies.enrollmentWait ?? createEnrollmentWait(dependencies);
  let device;
  try {
    device = await usb.requestDevice({ filters: [{
      vendorId: expected.usb.vendorId,
      productId: expected.usb.productId,
      serialNumber: expected.serialNumber,
    }] });
    wait.throwIfStopped();
    requireManagedIdentity(device, expected);
    await device.open();
    wait.throwIfStopped();
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
    const before = await readStatus(device, control);
    wait.throwIfStopped();
    if (action === "authorize") {
      if (before.status === 1) {
        throw new FlashBridgeError("busy", "The board is still saving a setup request. Check its status before starting another authorization.");
      }
      const bytes = new Uint8Array(REQUEST_BYTES);
      environment.crypto.getRandomValues(bytes.subarray(0, 4));
      const transactionBytes = new DataView(bytes.buffer);
      let transaction = transactionBytes.getUint32(0, true);
      while (transaction === 0 || transaction === before.transaction
          || transaction === session.operation?.transaction) {
        transaction = (transaction + 1) >>> 0;
      }
      transactionBytes.setUint32(0, transaction, true);
      for (let index = 0; index < 64; index += 1) {
        bytes[index + 4] = Number.parseInt(controllerPublicKey.slice(index * 2, index * 2 + 2), 16);
      }
      operation = {
        identity,
        transaction,
        targetPublicKey: before.targetPublicKey,
        submitted: true,
      };
      session.operation = operation;
      const written = await device.controlTransferOut({ ...control, request: ENROLL_REQUEST }, bytes);
      if (written?.status === "stall") {
        operation.submitted = false;
        session.operation = null;
        throw new FlashBridgeError("connection_failure", "Controller setup was rejected. No authorization was confirmed.");
      }
      if (written?.status !== "ok" || written.bytesWritten !== REQUEST_BYTES) {
        return unknown("The setup write could not be confirmed. Reconnect and check setup status without authorizing again.");
      }
    }
    for (let attempt = 0; ; attempt += 1) {
      let receipt = before;
      if (action === "authorize" || attempt > 0) {
        await sleep(ENROLLMENT_POLL_MILLIS);
        receipt = await readStatus(device, control);
      }
      if (receipt.targetPublicKey !== operation.targetPublicKey) {
        return unknown("This board has a different identity. Reconnect the board used for this setup and check its status before starting a new authorization.");
      }
      if (receipt.transaction !== operation.transaction || receipt.status === 0) {
        return unknown("The board has no matching setup receipt. It may have restarted or handled another request. Check access from your controller before explicitly authorizing again.");
      }
      if (receipt.status === 2) {
        return { status: "saved", targetPublicKey: receipt.targetPublicKey };
      }
      if (receipt.status === 3) {
        operation.submitted = false;
        session.operation = null;
        throw new FlashBridgeError("connection_failure", "The controller grant could not be saved.");
      }
      const stopped = await wait.afterPending();
      if (stopped) return stopped;
    }
  } catch (error) {
    if (operation?.submitted) {
      return unknown("Controller setup could not be confirmed. Reconnect and check setup status; this does not authorize the controller again.");
    }
    throw error;
  } finally {
    wait.close();
    if (device?.opened) await device.close().catch(() => {});
  }
}
