import { FlashBridgeError } from "./core.js";

const CORE_PATH = "/assets/flasher/nrf-dfu/prns_nrf_dfu_core.js";
const APPLICATION_TOUCH_HOLD_MS = 100;
const BOOTLOADER_INITIALIZATION_MS = 1_500;
const BOOTLOADER_RECONNECT_TIMEOUT_MS = 10_000;
const BOOTLOADER_SELECTION_TIMEOUT_MS = 30_000;
const ENUMERATION_INTERVAL_MS = 100;
const BOOTLOADER_OPEN_WAIT_MS = 100;
const ACKNOWLEDGEMENT_TIMEOUT_MS = 1_000;

let pendingBootloaderSelection = null;

export async function createNrfDfuSession(contract, files, dependencies = {}) {
  const application = files.find((file) => file.kind === "dfu-application");
  const initPacket = files.find((file) => file.kind === "dfu-init-packet");
  if (!application || !initPacket || files.length !== 2) {
    throw new FlashBridgeError(
      "invalid_request",
      "The verified Nordic DFU artifacts are incomplete.",
    );
  }
  const core = dependencies.nrfDfuCore ?? await import(CORE_PATH);
  if (dependencies.nrfDfuCore === undefined) {
    await core.default();
  }
  const bankLayout = contract.compatibility.bankLayout === "single"
    ? core.NrfDfuBankLayout.Single
    : core.NrfDfuBankLayout.Dual;
  let compatibility = null;
  try {
    compatibility = core.NrfDfuCompatibility.notEnforcedApplication(
      contract.compatibility.deviceType,
      contract.compatibility.deviceRevision,
      Uint16Array.from(contract.compatibility.softdeviceFwids),
      bankLayout,
    );
    const session = new core.NrfDfuSession(
      application.bytes,
      initPacket.bytes,
      compatibility,
    );
    return { core, session, contract };
  } catch (error) {
    throw new FlashBridgeError(
      "verification_failure",
      "The Rust Nordic DFU core rejected the signed application and init packet.",
      { cause: error },
    );
  } finally {
    compatibility?.free?.();
  }
}

export async function runNrfSerialDfu({
  prepared,
  events,
  dependencies = {},
  isCancelled,
}) {
  const environment = dependencies.environment ?? globalThis;
  const serial = dependencies.serial ?? environment.navigator?.serial;
  if (!serial?.requestPort || !serial?.getPorts) {
    throw new FlashBridgeError(
      "unsupported_browser",
      "This browser does not provide the Web Serial APIs required for Nordic DFU.",
    );
  }
  const { contract, core, session } = prepared.nrfDfu;
  let port = null;
  let deviceLost = false;
  let transferComplete = false;
  const onDisconnect = (event) => {
    if (port !== null && event.target === port && !transferComplete) {
      deviceLost = true;
    }
  };
  serial.addEventListener?.("disconnect", onDisconnect);
  try {
    events.emit({ phase: "requesting_port" });
    if (contract.entry === "managed-application") {
      const usb = dependencies.usb ?? environment.navigator?.usb;
      if (!usb?.requestDevice) {
        throw new FlashBridgeError(
          "unsupported_browser",
          "Personal Hopspot bootloader entry requires WebUSB in this browser.",
        );
      }
      await requestManagedBootloader(usb, contract.managedApplication, isCancelled);
      requireActive(isCancelled);
      events.emit({ phase: "connecting" });
      await cancellableWait(BOOTLOADER_INITIALIZATION_MS, isCancelled, dependencies);
      port = await authorizedBootloader(serial, contract, isCancelled, dependencies);
      if (port === null) {
        events.emit({ phase: "awaiting_bootloader_port" });
        port = await awaitBootloaderSelection(serial, prepared.serialFilters, dependencies);
      }
    } else {
      port = await requestExactSerialPort(serial, prepared.serialFilters, "permission_denied");
      requireExactSerialIdentity(port, contract.touchApplicationAndBootloaderUsb);
      requireActive(isCancelled);
      events.emit({ phase: "connecting" });
      port = await touchAndReopen(port, serial, contract, isCancelled, dependencies);
    }

    requireExactSerialIdentity(port, contract.touchApplicationAndBootloaderUsb);
    requireActive(isCancelled);
    events.emit({
      phase: "verifying_target",
      detectedChip: `nRF52840 (${usbIdentityLabel(contract.touchApplicationAndBootloaderUsb)})`,
    });
    await ensurePortOpen(port, contract.transferBaudRate, dependencies);
    await cancellableWait(BOOTLOADER_OPEN_WAIT_MS, isCancelled, dependencies);

    const total = prepared.files.find((file) => file.kind === "dfu-application")?.bytes.length;
    if (!Number.isSafeInteger(total) || total <= 0) {
      throw new FlashBridgeError("invalid_request", "The Nordic DFU application length is invalid.");
    }
    events.emit({ phase: "writing", current: 0, total });
    await transfer(session, core, port, contract.transferBaudRate, total, events, {
      ...dependencies,
      isCancelled,
      isDeviceLost: () => deviceLost,
    });
    transferComplete = true;
    events.emit({ phase: "verifying_flash", current: total, total });
    events.emit({ phase: "resetting" });
    await safeClosePort(port);
    port = null;
    requireActive(isCancelled);
    events.emit({ phase: "success", current: total, total });
    return { success: true };
  } catch (error) {
    if (deviceLost && !transferComplete && error?.code !== "cancelled") {
      throw new FlashBridgeError(
        "device_lost",
        "The Nordic DFU serial device disconnected before the transfer completed.",
        { cause: error },
      );
    }
    throw error;
  } finally {
    cancelNrfBootloaderSelection();
    serial.removeEventListener?.("disconnect", onDisconnect);
    await safeClosePort(port);
  }
}

export async function continueNrfBootloaderSelection() {
  const pending = pendingBootloaderSelection;
  if (pending === null) {
    throw new FlashBridgeError(
      "not_prepared",
      "No managed Nordic DFU operation is waiting for a bootloader port.",
    );
  }
  if (pending.selecting) {
    throw new FlashBridgeError("busy", "The bootloader port picker is already open.");
  }
  pending.selecting = true;
  try {
    const port = await requestExactSerialPort(
      pending.serial,
      pending.filters,
      "bootloader_permission_denied",
    );
    pending.resolve(port);
    return { selected: true };
  } catch (error) {
    pending.reject(error);
    throw error;
  } finally {
    if (pendingBootloaderSelection === pending) {
      pendingBootloaderSelection = null;
    }
  }
}

export function cancelNrfBootloaderSelection() {
  const pending = pendingBootloaderSelection;
  if (pending === null) return;
  pendingBootloaderSelection = null;
  pending.reject(new FlashBridgeError("cancelled", "Nordic DFU was cancelled."));
}

async function requestManagedBootloader(usb, expected, isCancelled) {
  requireActive(isCancelled);
  let device;
  try {
    device = await usb.requestDevice({
      filters: [{
        vendorId: expected.usb.vendorId,
        productId: expected.usb.productId,
        serialNumber: expected.serialNumber,
      }],
    });
  } catch (error) {
    if (error?.name === "NotFoundError") {
      throw new FlashBridgeError(
        "permission_denied",
        "No exact Personal Hopspot WebUSB device was selected.",
        { cause: error },
      );
    }
    if (error?.name === "SecurityError") {
      throw new FlashBridgeError(
        "insecure_context",
        "WebUSB bootloader entry requires HTTPS or localhost and an explicit user gesture.",
        { cause: error },
      );
    }
    throw new FlashBridgeError(
      "connection_failure",
      "The browser could not open the exact Personal Hopspot WebUSB picker.",
      { cause: error },
    );
  }
  requireManagedIdentity(device, expected);
  requireActive(isCancelled);
  try {
    await device.open();
    if (device.configuration === null) {
      const configurations = Array.from(device.configurations ?? []);
      if (configurations.length !== 1) {
        throw new FlashBridgeError(
          "ambiguous_device",
          "Personal Hopspot exposes an ambiguous WebUSB configuration set.",
        );
      }
      await device.selectConfiguration(configurations[0].configurationValue);
    }
    const interfaces = Array.from(device.configuration?.interfaces ?? []);
    if (
      interfaces.length !== 1
      || interfaces[0].interfaceNumber !== expected.interfaceNumber
    ) {
      throw new FlashBridgeError(
        "ambiguous_device",
        "Personal Hopspot exposes an unexpected WebUSB interface set.",
      );
    }
    await device.claimInterface(expected.interfaceNumber);
    const result = await device.controlTransferOut({
      requestType: "vendor",
      recipient: "device",
      request: expected.request,
      value: expected.value,
      index: expected.index,
    });
    if (result?.status !== "ok" || result.bytesWritten !== 0) {
      throw new FlashBridgeError(
        "connection_failure",
        "Personal Hopspot rejected the exact bootloader-entry request.",
      );
    }
  } catch (error) {
    if (error instanceof FlashBridgeError) throw error;
    throw new FlashBridgeError(
      "connection_failure",
      "Could not request Nordic bootloader entry from Personal Hopspot.",
      { cause: error },
    );
  } finally {
    try {
      if (device?.opened) await device.close();
    } catch {
      // A successful bootloader request intentionally disconnects the application device.
    }
  }
}

function requireManagedIdentity(device, expected) {
  if (
    device?.vendorId !== expected.usb.vendorId
    || device?.productId !== expected.usb.productId
    || device?.manufacturerName !== expected.manufacturer
    || device?.productName !== expected.product
    || device?.serialNumber !== expected.serialNumber
  ) {
    throw new FlashBridgeError(
      "ambiguous_device",
      "The selected WebUSB device does not have the exact Personal Hopspot identity.",
    );
  }
}

async function requestExactSerialPort(serial, filters, deniedCode) {
  try {
    return await serial.requestPort({ filters });
  } catch (error) {
    if (error?.name === "NotFoundError") {
      throw new FlashBridgeError(deniedCode, "No exact Nordic serial port was selected.", {
        cause: error,
      });
    }
    if (error?.name === "SecurityError") {
      throw new FlashBridgeError(
        "insecure_context",
        "Web Serial requires HTTPS or localhost and an explicit user gesture.",
        { cause: error },
      );
    }
    throw new FlashBridgeError(
      "connection_failure",
      "The browser could not open the exact Nordic serial device picker.",
      { cause: error },
    );
  }
}

async function touchAndReopen(port, serial, contract, isCancelled, dependencies) {
  await ensurePortOpen(port, contract.touchBaudRate, dependencies);
  try {
    await port.setSignals?.({ dataTerminalReady: true });
    await cancellableWait(APPLICATION_TOUCH_HOLD_MS, isCancelled, dependencies);
  } finally {
    await safeClosePort(port);
  }
  const deadline = now(dependencies) + BOOTLOADER_RECONNECT_TIMEOUT_MS;
  while (now(dependencies) < deadline) {
    requireActive(isCancelled);
    const candidates = await openableExactPorts(
      [port, ...(await serial.getPorts())],
      contract.touchApplicationAndBootloaderUsb,
      contract.transferBaudRate,
    );
    if (candidates.length > 1) {
      throw new FlashBridgeError(
        "ambiguous_device",
        "Multiple exact Nordic bootloader ports appeared after entry.",
      );
    }
    if (candidates.length === 1) {
      return candidates[0];
    }
    await cancellableWait(ENUMERATION_INTERVAL_MS, isCancelled, dependencies);
  }
  throw new FlashBridgeError(
    "reconnect_timeout",
    "The exact Nordic bootloader did not reappear before the bounded timeout.",
  );
}

async function authorizedBootloader(serial, contract, isCancelled, dependencies) {
  const deadline = now(dependencies) + BOOTLOADER_RECONNECT_TIMEOUT_MS;
  while (now(dependencies) < deadline) {
    requireActive(isCancelled);
    const candidates = await openableExactPorts(
      await serial.getPorts(),
      contract.touchApplicationAndBootloaderUsb,
      contract.transferBaudRate,
    );
    if (candidates.length > 1) {
      throw new FlashBridgeError(
        "ambiguous_device",
        "Multiple authorized Nordic bootloader ports are present.",
      );
    }
    if (candidates.length === 1) {
      return candidates[0];
    }
    await cancellableWait(ENUMERATION_INTERVAL_MS, isCancelled, dependencies);
  }
  return null;
}

function awaitBootloaderSelection(serial, filters, dependencies) {
  if (pendingBootloaderSelection !== null) {
    throw new FlashBridgeError("busy", "A bootloader port selection is already pending.");
  }
  return new Promise((resolve, reject) => {
    const setTimeoutImpl = dependencies.setTimeoutImpl ?? globalThis.setTimeout;
    const clearTimeoutImpl = dependencies.clearTimeoutImpl ?? globalThis.clearTimeout;
    const timeout = setTimeoutImpl(() => {
      if (pendingBootloaderSelection === pending) {
        pendingBootloaderSelection = null;
      }
      reject(new FlashBridgeError(
        "reconnect_timeout",
        "The bootloader port was not selected before the bounded continuation timeout.",
      ));
    }, BOOTLOADER_SELECTION_TIMEOUT_MS);
    const pending = {
      serial,
      filters,
      selecting: false,
      resolve(value) {
        clearTimeoutImpl(timeout);
        resolve(value);
      },
      reject(error) {
        clearTimeoutImpl(timeout);
        reject(error);
      },
    };
    pendingBootloaderSelection = pending;
  });
}

async function transfer(session, core, port, baudRate, total, events, dependencies) {
  let frame = session.nextFrame();
  let lastRetryReason = null;
  try {
    while (true) {
      requireTransferActive(dependencies);
      await writeFrame(port, frame.bytes);
      let transition;
      try {
        transition = await readAcknowledgement(session, core, port, dependencies);
      } catch (error) {
        if (error?.code !== "acknowledgement_timeout") throw error;
        await reopenAfterTimeout(port, baudRate, dependencies);
        frame.free();
        frame = null;
        frame = retryFrame(session, lastRetryReason);
        continue;
      }
      try {
        if (transition.kind === core.NrfDfuAcknowledgementTransitionKind.RetryRequired) {
          lastRetryReason = transition.retryReason();
          frame.free();
          frame = null;
          frame = retryFrame(session, lastRetryReason);
          continue;
        }
        if (
          transition.kind !== core.NrfDfuAcknowledgementTransitionKind.FrameAccepted
          && transition.kind !== core.NrfDfuAcknowledgementTransitionKind.TransferComplete
        ) {
          throw new FlashBridgeError(
            "malformed_acknowledgement",
            "The Rust Nordic DFU core returned an invalid acknowledgement transition.",
          );
        }
        const current = transition.writtenBytes;
        const rustTotal = transition.totalBytes;
        if (rustTotal !== total) {
          throw new FlashBridgeError(
            "verification_failure",
            "The Rust Nordic DFU progress total changed across the browser boundary.",
          );
        }
        await cancellableWait(transition.waitMilliseconds, dependencies.isCancelled, dependencies);
        events.emit({ phase: "writing", current, total });
        if (transition.kind === core.NrfDfuAcknowledgementTransitionKind.TransferComplete) {
          return;
        }
        frame.free();
        frame = null;
        frame = session.nextFrame();
        lastRetryReason = null;
      } finally {
        transition.free();
      }
    }
  } finally {
    frame?.free?.();
  }
}

function retryFrame(session, lastRetryReason) {
  try {
    return session.retryFrame();
  } catch (error) {
    const reason = lastRetryReason === null ? "timeout" : "malformed acknowledgement";
    throw new FlashBridgeError(
      "retries_exhausted",
      `The Nordic bootloader exhausted its bounded frame retries after ${reason}.`,
      { cause: error },
    );
  }
}

async function writeFrame(port, bytes) {
  const writer = port.writable?.getWriter?.();
  if (!writer) {
    throw new FlashBridgeError("device_lost", "The Nordic DFU serial writer is unavailable.");
  }
  try {
    await writer.write(bytes);
  } catch (error) {
    throw new FlashBridgeError("write_failure", "Writing a Nordic DFU frame failed.", {
      cause: error,
    });
  } finally {
    writer.releaseLock();
  }
}

async function readAcknowledgement(session, core, port, dependencies) {
  const reader = port.readable?.getReader?.();
  if (!reader) {
    throw new FlashBridgeError("device_lost", "The Nordic DFU serial reader is unavailable.");
  }
  let timedOut = false;
  try {
    while (true) {
      requireTransferActive(dependencies);
      let result;
      try {
        result = await readWithTimeout(reader, dependencies);
      } catch (error) {
        if (error?.code === "acknowledgement_timeout") timedOut = true;
        throw error;
      }
      if (result.done) {
        throw new FlashBridgeError(
          "device_lost",
          "The Nordic DFU serial stream ended before acknowledgement.",
        );
      }
      let transition;
      try {
        transition = session.pushAcknowledgement(result.value);
      } catch (error) {
        throw new FlashBridgeError(
          "malformed_acknowledgement",
          "The Rust Nordic DFU core rejected acknowledgement bytes.",
          { cause: error },
        );
      }
      if (transition.kind === core.NrfDfuAcknowledgementTransitionKind.AwaitingMore) {
        transition.free();
        continue;
      }
      return transition;
    }
  } finally {
    if (timedOut) {
      try {
        await reader.cancel("Nordic DFU acknowledgement timeout");
      } catch {
        // Closing and reopening the exact port below owns timeout recovery.
      }
    }
    reader.releaseLock();
  }
}

async function readWithTimeout(reader, dependencies) {
  const setTimeoutImpl = dependencies.setTimeoutImpl ?? globalThis.setTimeout;
  const clearTimeoutImpl = dependencies.clearTimeoutImpl ?? globalThis.clearTimeout;
  let timeout;
  const timed = new Promise((_, reject) => {
    timeout = setTimeoutImpl(() => reject(Object.assign(
      new Error("Nordic DFU acknowledgement timeout"),
      { code: "acknowledgement_timeout" },
    )), ACKNOWLEDGEMENT_TIMEOUT_MS);
  });
  try {
    return await Promise.race([reader.read(), timed]);
  } finally {
    clearTimeoutImpl(timeout);
  }
}

async function reopenAfterTimeout(port, baudRate, dependencies) {
  await safeClosePort(port);
  await ensurePortOpen(port, baudRate, dependencies);
}

async function ensurePortOpen(port, baudRate, dependencies) {
  if (port.readable && port.writable) return;
  try {
    await port.open(serialOptions(baudRate));
    await port.setSignals?.({ dataTerminalReady: true });
  } catch (error) {
    throw new FlashBridgeError(
      "connection_failure",
      "Could not open the exact Nordic serial port.",
      { cause: error },
    );
  }
  await wait(0, dependencies);
}

async function tryOpen(port, baudRate) {
  try {
    await port.open(serialOptions(baudRate));
    return true;
  } catch {
    return false;
  }
}

function serialOptions(baudRate) {
  return {
    baudRate,
    dataBits: 8,
    stopBits: 1,
    parity: "none",
    flowControl: "none",
    bufferSize: 4_096,
  };
}

async function safeClosePort(port) {
  if (!port) return;
  try {
    if (port.readable || port.writable) await port.close();
  } catch {
    // Physical reset and disconnect commonly race the final close.
  }
}

function requireExactSerialIdentity(port, expected) {
  const info = port?.getInfo?.();
  if (
    info?.usbVendorId !== expected.vendorId
    || info?.usbProductId !== expected.productId
  ) {
    throw new FlashBridgeError(
      "ambiguous_device",
      "The selected serial port does not have the exact Nordic bootloader identity.",
    );
  }
}

function uniqueExactPorts(ports, expected) {
  return Array.from(new Set(ports)).filter((port) => {
    const info = port?.getInfo?.();
    return info?.usbVendorId === expected.vendorId
      && info?.usbProductId === expected.productId;
  });
}

async function openableExactPorts(ports, expected, baudRate) {
  const openable = [];
  for (const port of uniqueExactPorts(ports, expected)) {
    if (await tryOpen(port, baudRate)) {
      await safeClosePort(port);
      openable.push(port);
    }
  }
  return openable;
}

function usbIdentityLabel(identity) {
  return `${identity.vendorId.toString(16).padStart(4, "0")}:${identity.productId
    .toString(16).padStart(4, "0")}`;
}

function requireTransferActive(dependencies) {
  requireActive(dependencies.isCancelled);
  if (dependencies.isDeviceLost()) {
    throw new FlashBridgeError(
      "device_lost",
      "The Nordic DFU serial device disconnected before acknowledgement.",
    );
  }
}

function requireActive(isCancelled) {
  if (isCancelled()) {
    throw new FlashBridgeError("cancelled", "Nordic DFU was cancelled; no success was reported.");
  }
}

async function cancellableWait(milliseconds, isCancelled, dependencies) {
  const deadline = now(dependencies) + milliseconds;
  while (now(dependencies) < deadline) {
    requireActive(isCancelled);
    await wait(Math.min(25, deadline - now(dependencies)), dependencies);
  }
  requireActive(isCancelled);
}

function wait(milliseconds, dependencies) {
  const sleepImpl = dependencies.sleepImpl
    ?? ((duration) => new Promise((resolve) => globalThis.setTimeout(resolve, duration)));
  return sleepImpl(milliseconds);
}

function now(dependencies) {
  return dependencies.nowImpl?.() ?? Date.now();
}
