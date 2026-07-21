import {
  FlashBridgeError,
  flashSizeValue,
  md5Hex,
  normalizeChipName,
  provisioningImage,
  safeFailure,
  sha256Hex,
  validateRequest,
} from "./core.js";
import { BRIDGE_SCHEMA, validateBridgeEvent } from "./contract.js";

let prepared = null;
let active = false;
let cancelRequested = false;
let DefaultLoader = null;
let DefaultTransport = null;

function emitEvent(emit, event) {
  emit(validateBridgeEvent({ schema: BRIDGE_SCHEMA, ...event }));
}

function assertHostedEnvironment(environment = globalThis) {
  if (!environment.isSecureContext) {
    throw new FlashBridgeError(
      "insecure_context",
      "Open the flasher over HTTPS or localhost before connecting a device.",
    );
  }
}

export async function prepare(request, emit = () => {}, dependencies = {}) {
  if (active) {
    throwEarlyFailure(
      emit,
      new FlashBridgeError("busy", "A device operation is already active."),
      false,
    );
  }
  discardPrepared();
  cancelRequested = false;
  const fetchImpl = dependencies.fetchImpl ?? globalThis.fetch;
  const cryptoImpl = dependencies.cryptoImpl ?? globalThis.crypto;
  try {
    validateRequest(request);
    if (request.transport === "esp-serial" && dependencies.loadEsptool !== false) {
      const module = await import("esptool-js");
      DefaultLoader = module.ESPLoader;
      DefaultTransport = module.Transport;
    }
    emitEvent(emit, { phase: "validating_manifest" });
    const files = [];
    let completed = 0;
    const total = request.parts.reduce((sum, part) => sum + part.size, 0);
    for (const part of request.parts) {
      if (cancelRequested) {
        throw new FlashBridgeError("cancelled", "Preparation was cancelled before device access.");
      }
      emitEvent(emit, {
        phase: "downloading",
        part: part.kind,
        partIndex: files.length,
        partCount: request.parts.length,
        current: completed,
        total,
      });
      const response = await fetchImpl(part.url, {
        cache: "no-store",
        credentials: "omit",
        redirect: "error",
      });
      if (!response.ok) {
        throw new FlashBridgeError("artifact_fetch", "A signed firmware part could not be downloaded.");
      }
      const bytes = new Uint8Array(await response.arrayBuffer());
      if (bytes.length !== part.size) {
        throw new FlashBridgeError("artifact_size_mismatch", "A firmware part has the wrong byte length.");
      }
      const actual = await sha256Hex(bytes, cryptoImpl);
      if (actual !== part.sha256) {
        throw new FlashBridgeError("artifact_hash_mismatch", "A firmware part failed SHA-256 verification.");
      }
      files.push({ ...part, bytes });
      completed += bytes.length;
      emitEvent(emit, {
        phase: "verifying_artifacts",
        part: part.kind,
        partIndex: files.length - 1,
        partCount: request.parts.length,
        current: completed,
        total,
      });
    }

    const config = provisioningImage(request.provisioning);
    if (config) {
      files.push({
        kind: "provisioning",
        path: "local-only",
        url: null,
        offset: request.provisioning.offset,
        size: config.length,
        sha256: await sha256Hex(config, cryptoImpl),
        bytes: config,
      });
    }
    prepared = {
      boardSlug: request.boardSlug,
      displayName: request.displayName,
      transport: request.transport,
      expectedChip: request.expectedChip,
      flashSize: request.flashSize,
      flashMode: request.flashMode,
      flashFrequency: request.flashFrequency,
      beforeReset: request.beforeReset,
      afterReset: request.afterReset,
      files,
    };
    emitEvent(emit, {
      phase: "ready",
      current: completed,
      total,
      bytes: files.reduce((sum, file) => sum + file.bytes.length, 0),
    });
    return { ready: true };
  } catch (error) {
    const failure = safeFailure(error);
    discardPrepared();
    emitEvent(emit, { phase: failure.code === "cancelled" ? "cancelled" : "failed", ...failure });
    throw error;
  } finally {
    if (request?.provisioning) {
      request.provisioning.password = "";
      request.provisioning.ssid = "";
    }
  }
}

export async function flash(emit = () => {}, dependencies = {}) {
  if (!prepared) {
    throwEarlyFailure(
      emit,
      new FlashBridgeError("not_prepared", "Prepare and verify the release before connecting."),
    );
  }
  if (active) {
    throwEarlyFailure(
      emit,
      new FlashBridgeError("busy", "A device operation is already active."),
      false,
    );
  }
  if (prepared.transport === "uf2-mass-storage") {
    try {
      return downloadUf2(emit, dependencies);
    } catch (error) {
      discardPrepared();
      throwEarlyFailure(emit, error);
    }
  }
  const environment = dependencies.environment ?? globalThis;
  try {
    assertHostedEnvironment(environment);
  } catch (error) {
    throwEarlyFailure(emit, error);
  }
  const serial = dependencies.serial ?? environment.navigator?.serial;
  if (!serial?.requestPort) {
    throwEarlyFailure(
      emit,
      new FlashBridgeError(
        "unsupported_browser",
        "This browser does not provide Web Serial. Use current Chrome/Edge or the CLI.",
      ),
    );
  }
  const TransportImpl = dependencies.TransportImpl ?? DefaultTransport;
  const LoaderImpl = dependencies.LoaderImpl ?? DefaultLoader;
  if (!TransportImpl || !LoaderImpl) {
    throwEarlyFailure(
      emit,
      new FlashBridgeError("not_prepared", "The Espressif engine was not loaded during preparation."),
    );
  }
  let transport = null;
  let deviceLost = false;
  active = true;
  cancelRequested = false;
  setNavigationGuard(true, environment);
  try {
    emitEvent(emit, { phase: "requesting_port" });
    const port = await serial.requestPort();
    if (cancelRequested) {
      throw new FlashBridgeError("cancelled", "Flashing was cancelled before connecting.");
    }
    let loader;
    try {
      transport = new TransportImpl(port, false);
      transport.setDeviceLostCallback?.(() => {
        deviceLost = true;
      });
      const terminal = { clean() {}, writeLine() {}, write() {} };
      loader = new LoaderImpl({
        transport,
        baudrate: 921600,
        terminal,
        debugLogging: false,
      });
    } catch (error) {
      throw new FlashBridgeError("connection_failure", "Could not initialize the serial transport.", { cause: error });
    }
    emitEvent(emit, { phase: "connecting" });
    let chipName;
    try {
      chipName = await loader.main(mapBeforeReset(prepared.beforeReset));
    } catch (error) {
      throw new FlashBridgeError("connection_failure", "Could not connect to the Espressif bootloader.", { cause: error });
    }
    emitEvent(emit, { phase: "verifying_target", detectedChip: chipName });
    if (normalizeChipName(chipName) !== normalizeChipName(prepared.expectedChip)) {
      throw new FlashBridgeError(
        "wrong_chip",
        `Wrong chip family: selected ${prepared.expectedChip}, detected ${chipName}.`,
      );
    }
    let detectedFlashSize;
    try {
      detectedFlashSize = await loader.detectFlashSize();
    } catch (error) {
      throw new FlashBridgeError("connection_failure", "Could not identify the device flash capacity.", { cause: error });
    }
    if (detectedFlashSize !== flashSizeValue(prepared.flashSize)) {
      throw new FlashBridgeError(
        "wrong_flash_size",
        `Wrong flash capacity: selected ${flashSizeValue(prepared.flashSize)}, detected ${detectedFlashSize}.`,
      );
    }
    if (cancelRequested) {
      throw new FlashBridgeError("cancelled", "Flashing was cancelled before writing.");
    }

    const total = prepared.files.reduce((sum, file) => sum + file.bytes.length, 0);
    emitEvent(emit, { phase: "writing", current: 0, total });
    let completed = 0;
    for (let index = 0; index < prepared.files.length; index += 1) {
      if (cancelRequested) {
        throw new FlashBridgeError("cancelled", "Flashing stopped at a verified part boundary.");
      }
      const file = prepared.files[index];
      try {
        await loader.writeFlash({
          fileArray: [{ data: file.bytes, address: file.offset }],
          flashMode: prepared.flashMode,
          flashFreq: prepared.flashFrequency,
          flashSize: flashSizeValue(prepared.flashSize),
          eraseAll: false,
          compress: true,
          reportProgress(_fileIndex, written) {
            emitEvent(emit, {
              phase: "writing",
              part: file.kind,
              partIndex: index,
              partCount: prepared.files.length,
              current: Math.min(total, completed + written),
              total,
            });
          },
          calculateMD5Hash: md5Hex,
        });
      } catch (error) {
        if (/md5|checksum|verify/i.test(String(error?.message ?? error))) {
          throw new FlashBridgeError("verification_failure", `Device-side verification failed for ${file.kind}.`, { cause: error });
        }
        throw new FlashBridgeError("write_failure", `Writing ${file.kind} failed.`, { cause: error });
      }
      completed += file.bytes.length;
      if (cancelRequested) {
        throw new FlashBridgeError("cancelled", "Flashing stopped at a verified part boundary.");
      }
    }
    emitEvent(emit, { phase: "verifying_flash", current: total, total });
    emitEvent(emit, { phase: "resetting" });
    try {
      await loader.after(mapAfterReset(prepared.afterReset));
    } catch (error) {
      throw new FlashBridgeError("reset_failure", "All parts verified, but the final device reset failed.", { cause: error });
    }
    if (cancelRequested) {
      throw new FlashBridgeError(
        "cancelled",
        "Cancellation was requested during writing; verification and reset finished safely, but success was not reported.",
      );
    }
    emitEvent(emit, { phase: "success", current: total, total });
    return { success: true };
  } catch (error) {
    const failure = safeFailure(error, deviceLost);
    emitEvent(emit, { phase: failure.code === "cancelled" ? "cancelled" : "failed", ...failure });
    throw error;
  } finally {
    active = false;
    setNavigationGuard(false, environment);
    try {
      await transport?.disconnect();
    } catch {
      // The device may already be gone after a successful reset. Cleanup remains best effort.
    }
    discardPrepared();
  }
}

function throwEarlyFailure(emit, error, discard = true) {
  const failure = safeFailure(error);
  if (discard) {
    discardPrepared();
  }
  emitEvent(emit, { phase: failure.code === "cancelled" ? "cancelled" : "failed", ...failure });
  throw error;
}

export function cancel() {
  cancelRequested = true;
}

export function clearPrepared() {
  cancelRequested = active;
  if (!active) {
    discardPrepared();
  }
}

function downloadUf2(emit, dependencies) {
  const environment = dependencies.environment ?? globalThis;
  const [file] = prepared.files;
  if (!file || file.kind !== "uf2") {
    throw new FlashBridgeError("invalid_request", "The prepared target has no UF2 payload.");
  }
  const BlobImpl = dependencies.BlobImpl ?? environment.Blob;
  const urlApi = dependencies.urlApi ?? environment.URL;
  const documentImpl = dependencies.documentImpl ?? environment.document;
  if (!BlobImpl || !urlApi?.createObjectURL || !documentImpl?.createElement) {
    throw new FlashBridgeError("unsupported_browser", "This browser cannot create a verified UF2 download.");
  }
  const blobUrl = urlApi.createObjectURL(new BlobImpl([file.bytes], { type: "application/octet-stream" }));
  try {
    const link = documentImpl.createElement("a");
    link.href = blobUrl;
    link.download = "prns-hopspot-t-echo.uf2";
    link.click();
    emitEvent(emit, {
      phase: "success",
      current: file.bytes.length,
      total: file.bytes.length,
      message: "Verified UF2 downloaded. Copy it to TECHOBOOT; the drive disappears when the device reboots.",
    });
    discardPrepared();
    return { success: true };
  } finally {
    urlApi.revokeObjectURL(blobUrl);
  }
}

function mapBeforeReset(value) {
  return value === "usb-reset" ? "usb_reset" : "default_reset";
}

function mapAfterReset(value) {
  if (value === "hard-reset" || value === "watchdog-reset") {
    // esptool-js exposes a transport hard reset; this is the safe browser equivalent for
    // espflash's watchdog reset strategy on native-USB ESP32-S3 boards.
    return "hard_reset";
  }
  throw new FlashBridgeError("invalid_request", "The signed release requested an unsupported reset mode.");
}

function setNavigationGuard(enabled, environment) {
  if (!environment.addEventListener || !environment.removeEventListener) {
    return;
  }
  if (enabled) {
    environment.addEventListener("beforeunload", navigationGuard);
  } else {
    environment.removeEventListener("beforeunload", navigationGuard);
  }
}

function navigationGuard(event) {
  if (!active) return;
  event.preventDefault();
  event.returnValue = "";
}

function discardPrepared() {
  if (!prepared) {
    return;
  }
  for (const file of prepared.files) {
    if (file.kind === "provisioning") {
      file.bytes.fill(0);
    }
  }
  prepared = null;
}

export const testing = {
  prepared: () => prepared,
  reset() {
    discardPrepared();
    active = false;
    cancelRequested = false;
  },
};
