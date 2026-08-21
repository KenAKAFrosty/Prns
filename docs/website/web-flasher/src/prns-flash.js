import {
  BoundedResponseError,
  esptoolFlashSizeValue,
  FlashBridgeError,
  flashSizeLabel,
  jedecFlashSizeBytes,
  md5Hex,
  normalizeChipName,
  provisioningImage,
  readBoundedBytes,
  safeFailure,
  sha256Hex,
  validateUf2Artifact,
  validateRequest,
} from "./core.js";
import { BRIDGE_SCHEMA, BridgeEventSequence, RESPONSE_LIMITS } from "./contract.js";
import {
  cancelNrfBootloaderSelection,
  continueNrfBootloaderSelection as continuePendingNrfBootloaderSelection,
  createNrfDfuSession,
  runNrfSerialDfu,
} from "./nrf-serial-dfu.js";

let prepared = null;
let active = false;
let cancelRequested = false;
let preparationGeneration = 0;
let preparingRequest = null;
let DefaultLoader = null;
let DefaultTransport = null;
let activeNavigationEnvironment = null;
let activeHistoryGuard = null;
let historyGuardSequence = 0;
let cancellationLocked = false;
const RESET_ENUMERATION_TIMEOUT_MS = 10_000;
const ESP32S3_WDT_WPROTECT = 0x600080b0;
const ESP32S3_WDT_CONFIG0 = 0x60008098;
const ESP32S3_WDT_CONFIG1 = 0x6000809c;
const ESP32S3_WDT_WRITE_KEY = 0x50d83aa1;
const ESP32S3_WDT_RESET_FLAGS = 0xd0000104;
const ESP32C6_CHIP_NAME = "ESP32-C6";
const ESP32C6_SPI_REGISTER_BASE = 0x60003000;
const ESP32C6_RESET_SIGNAL_DELAY_MS = 100;
const ESP32C6_RESET_COMPLETION_MESSAGE = "Finished — Verified serial flash complete. The C6 reset signal was sent. Because this USB path does not provide reliable browser re-enumeration evidence, press RESET once if Personal Hopspot does not start automatically. You can close this page.";

function operationEvents(emit, operation) {
  const sequence = new BridgeEventSequence(operation);
  return {
    emit(event) {
      emit(sequence.accept({ schema: BRIDGE_SCHEMA, ...event }));
    },
    get terminal() {
      return sequence.terminal;
    },
  };
}

function assertHostedEnvironment(environment = globalThis) {
  if (!environment.isSecureContext) {
    throw new FlashBridgeError(
      "insecure_context",
      "Open the flasher over HTTPS or localhost before connecting a device.",
    );
  }
}

export async function fetchSignedDocuments(request, dependencies = {}) {
  try {
    const documentMaximum = request?.documentMaxBytes;
    if (
      ![RESPONSE_LIMITS.channel_bytes, RESPONSE_LIMITS.manifest_bytes].includes(documentMaximum)
      || request?.signatureMaxBytes !== RESPONSE_LIMITS.signature_bytes
    ) {
      throw new BoundedResponseError("invalid_limit", "The signed-document limits are invalid.");
    }
    const environment = dependencies.environment ?? globalThis;
    const fetchImpl = dependencies.fetchImpl ?? globalThis.fetch;
    const TextDecoderImpl = dependencies.TextDecoderImpl ?? globalThis.TextDecoder;
    if (typeof fetchImpl !== "function" || !TextDecoderImpl) {
      throw new BoundedResponseError(
        "stream_failure",
        "This browser cannot stream signed release documents.",
      );
    }
    const documentUrl = resolveSignedDocumentUrl(request.documentUrl, environment);
    const signatureUrl = resolveSignedDocumentUrl(`${request.documentUrl}.minisig`, environment);
    const documentBytes = await fetchBoundedDocument(fetchImpl, documentUrl, documentMaximum);
    const signatureBytes = await fetchBoundedDocument(
      fetchImpl,
      signatureUrl,
      request.signatureMaxBytes,
    );
    let document;
    let signature;
    try {
      const decoder = new TextDecoderImpl("utf-8", { fatal: true });
      document = decoder.decode(documentBytes);
      signature = decoder.decode(signatureBytes);
    } catch (error) {
      return { status: "error", error: "invalid_utf8" };
    }
    return { status: "ready", document, signature };
  } catch (error) {
    return {
      status: "error",
      error: error instanceof BoundedResponseError && error.code === "response_too_large"
        ? "too_large"
        : "unavailable",
    };
  }
}

async function fetchBoundedDocument(fetchImpl, url, maximumBytes) {
  let response;
  try {
    response = await fetchImpl(url, {
      cache: "no-store",
      credentials: "omit",
      redirect: "error",
    });
  } catch (error) {
    throw new BoundedResponseError("stream_failure", "A signed document is unavailable.", {
      cause: error,
    });
  }
  if (!response?.ok) {
    throw new BoundedResponseError("stream_failure", "A signed document is unavailable.");
  }
  return readBoundedBytes(response, maximumBytes);
}

function resolveSignedDocumentUrl(value, environment) {
  if (typeof value !== "string" || value.includes("%") || value.includes("\\")) {
    throw new BoundedResponseError("stream_failure", "The signed document URL is invalid.");
  }
  const localOrigin = environment.location?.origin;
  if (!localOrigin) {
    throw new BoundedResponseError("stream_failure", "The page origin is unavailable.");
  }
  const resolved = new URL(value, localOrigin);
  if (
    !resolved.pathname.startsWith("/releases/")
    || resolved.search
    || resolved.hash
    || (resolved.origin !== localOrigin && resolved.origin !== "https://reticulum.rs")
  ) {
    throw new BoundedResponseError("stream_failure", "The signed document URL is invalid.");
  }
  const localQualification = ["localhost", "127.0.0.1", "::1"].includes(
    environment.location?.hostname,
  );
  if (localQualification && resolved.origin === "https://reticulum.rs") {
    return resolved.pathname;
  }
  return resolved.origin === localOrigin ? resolved.pathname : resolved.href;
}

export async function prepare(request, emit = () => {}, dependencies = {}) {
  const events = operationEvents(emit, "preparation");
  if (active) {
    clearProvisioning(request);
    throwEarlyFailure(
      events,
      new FlashBridgeError("busy", "A device operation is already active."),
      false,
    );
  }
  const generation = ++preparationGeneration;
  discardPreparingRequest();
  preparingRequest = request;
  discardPrepared();
  cancelRequested = false;
  const fetchImpl = dependencies.fetchImpl ?? globalThis.fetch;
  const cryptoImpl = dependencies.cryptoImpl ?? globalThis.crypto;
  let nrfDfu = null;
  try {
    validateRequest(request);
    if (request.transport === "esp-serial" && dependencies.loadEsptool !== false) {
      const module = await import("esptool-js");
      requireCurrentPreparation(generation);
      DefaultLoader = module.ESPLoader;
      DefaultTransport = module.Transport;
    }
    requireCurrentPreparation(generation);
    events.emit({ phase: "validating_manifest" });
    const files = [];
    let completed = 0;
    const total = request.parts.reduce((sum, part) => sum + part.size, 0);
    for (const part of request.parts) {
      requireCurrentPreparation(generation);
      events.emit({
        phase: "downloading",
        part: part.kind,
        partIndex: files.length,
        partCount: request.parts.length,
        current: completed,
        total,
      });
      let response;
      try {
        response = await fetchImpl(part.url, {
          cache: "no-store",
          credentials: "omit",
          redirect: "error",
        });
      } catch (error) {
        throw new FlashBridgeError(
          "artifact_fetch",
          "A signed firmware part could not be downloaded.",
          { cause: error },
        );
      }
      requireCurrentPreparation(generation);
      if (!response?.ok) {
        throw new FlashBridgeError("artifact_fetch", "A signed firmware part could not be downloaded.");
      }
      let bytes;
      try {
        bytes = await readBoundedBytes(response, part.size, () => {
          requireCurrentPreparation(generation);
        });
      } catch (error) {
        if (error instanceof FlashBridgeError) throw error;
        if (error instanceof BoundedResponseError && error.code === "response_too_large") {
          throw new FlashBridgeError(
            "artifact_size_mismatch",
            "A firmware part exceeds its signed byte length.",
            { cause: error },
          );
        }
        throw new FlashBridgeError(
          "artifact_fetch",
          "A signed firmware part could not be streamed safely.",
          { cause: error },
        );
      }
      requireCurrentPreparation(generation);
      if (bytes.length !== part.size) {
        throw new FlashBridgeError("artifact_size_mismatch", "A firmware part has the wrong byte length.");
      }
      const actual = await sha256Hex(bytes, cryptoImpl);
      requireCurrentPreparation(generation);
      if (actual !== part.sha256) {
        throw new FlashBridgeError("artifact_hash_mismatch", "A firmware part failed SHA-256 verification.");
      }
      if (request.transport === "uf2-mass-storage") {
        validateUf2Artifact(bytes, request.uf2Compatibility, request.boardSlug);
      }
      files.push({ ...part, bytes });
      completed += bytes.length;
      events.emit({
        phase: "verifying_artifacts",
        part: part.kind,
        partIndex: files.length - 1,
        partCount: request.parts.length,
        current: completed,
        total,
      });
    }

    requireCurrentPreparation(generation);
    if (request.transport === "nrf-serial-dfu") {
      nrfDfu = await createNrfDfuSession(request.nrfSerialDfu, files, dependencies);
      requireCurrentPreparation(generation);
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
    requireCurrentPreparation(generation);
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
      mountLabel: request.mountLabel,
      serialFilters: request.serialFilters.map((filter) => ({ ...filter })),
      installMode: request.installMode,
      files,
      nrfDfu,
    };
    nrfDfu = null;
    events.emit({
      phase: "ready",
      current: completed,
      total,
      bytes: files.reduce((sum, file) => sum + file.bytes.length, 0),
    });
    return { ready: true };
  } catch (error) {
    nrfDfu?.session?.free?.();
    const failure = safeFailure(error);
    if (generation === preparationGeneration) {
      discardPrepared();
    }
    if (!events.terminal) {
      events.emit({ phase: failure.code === "cancelled" ? "cancelled" : "failed", ...failure });
    }
    throw error;
  } finally {
    clearProvisioning(request);
    if (preparingRequest === request) {
      preparingRequest = null;
    }
  }
}

export async function flash(emit = () => {}, dependencies = {}) {
  const events = operationEvents(emit, "device");
  if (!prepared) {
    throwEarlyFailure(
      events,
      new FlashBridgeError("not_prepared", "Prepare and verify the release before connecting."),
    );
  }
  if (active) {
    throwEarlyFailure(
      events,
      new FlashBridgeError("busy", "A device operation is already active."),
      false,
    );
  }
  if (prepared.transport === "uf2-mass-storage") {
    try {
      return downloadUf2(events, dependencies);
    } catch (error) {
      discardPrepared();
      throwEarlyFailure(events, error);
    }
  }
  const environment = dependencies.environment ?? globalThis;
  try {
    assertHostedEnvironment(environment);
  } catch (error) {
    throwEarlyFailure(events, error);
  }
  const serial = dependencies.serial ?? environment.navigator?.serial;
  if (!serial?.requestPort) {
    throwEarlyFailure(
      events,
      new FlashBridgeError(
        "unsupported_browser",
        "This browser does not provide Web Serial. Use current desktop Chrome, Edge, or Firefox 151 or later, or the CLI.",
      ),
    );
  }
  if (prepared.transport === "nrf-serial-dfu") {
    return flashNrfSerialDfu(events, dependencies, environment);
  }
  const TransportImpl = dependencies.TransportImpl ?? DefaultTransport;
  const LoaderImpl = dependencies.LoaderImpl ?? DefaultLoader;
  if (!TransportImpl || !LoaderImpl) {
    throwEarlyFailure(
      events,
      new FlashBridgeError("not_prepared", "The Espressif engine was not loaded during preparation."),
    );
  }
  let transport = null;
  let deviceLost = false;
  let retainPreparedPlan = false;
  active = true;
  cancelRequested = false;
  cancellationLocked = false;
  setNavigationGuard(true, environment);
  try {
    events.emit({ phase: "requesting_port" });
    let port;
    try {
      port = await serial.requestPort({ filters: prepared.serialFilters });
    } catch (error) {
      if (error?.name === "NotFoundError" || error?.name === "SecurityError") {
        throw error;
      }
      throw new FlashBridgeError(
        "connection_failure",
        "The browser could not open the serial device picker.",
        { cause: error },
      );
    }
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
    events.emit({ phase: "connecting" });
    try {
      await loader.main(mapBeforeReset(prepared.beforeReset));
    } catch (error) {
      throw new FlashBridgeError("connection_failure", "Could not connect to the Espressif bootloader.", { cause: error });
    }
    const chipName = loader.chip?.CHIP_NAME;
    if (typeof chipName !== "string" || chipName.trim().length === 0) {
      throw new FlashBridgeError(
        "connection_failure",
        "The Espressif loader did not expose a canonical chip family.",
      );
    }
    events.emit({ phase: "verifying_target", detectedChip: chipName });
    if (normalizeChipName(chipName) !== normalizeChipName(prepared.expectedChip)) {
      throw new FlashBridgeError(
        "wrong_chip",
        `Wrong chip family: selected ${prepared.expectedChip}, detected ${chipName}.`,
      );
    }
    if (normalizeChipName(chipName) === normalizeChipName(ESP32C6_CHIP_NAME)) {
      loader.chip.SPI_REG_BASE = ESP32C6_SPI_REGISTER_BASE;
    }
    let flashId;
    try {
      flashId = await loader.readFlashId();
    } catch (error) {
      throw new FlashBridgeError("connection_failure", "Could not identify the device flash capacity.", { cause: error });
    }
    const detectedFlashSize = jedecFlashSizeBytes(flashId);
    if (detectedFlashSize === null) {
      const flashIdLabel = Number.isSafeInteger(flashId)
        ? `0x${(flashId >>> 0).toString(16).padStart(8, "0")}`
        : String(flashId);
      throw new FlashBridgeError(
        "connection_failure",
        `The device returned an unknown JEDEC flash-capacity identifier: ${flashIdLabel}.`,
      );
    }
    if (detectedFlashSize !== prepared.flashSize) {
      throw new FlashBridgeError(
        "wrong_flash_size",
        `Wrong flash capacity: selected ${flashSizeLabel(prepared.flashSize)}, detected ${flashSizeLabel(detectedFlashSize)}.`,
      );
    }
    if (cancelRequested) {
      throw new FlashBridgeError("cancelled", "Flashing was cancelled before writing.");
    }

    if (prepared.installMode === "erase-all") {
      cancellationLocked = true;
      events.emit({ phase: "erasing" });
      try {
        await loader.eraseFlash();
      } catch (error) {
        throw new FlashBridgeError(
          "erase_failure",
          "Full-chip erasure failed.",
          { cause: error },
        );
      }
    }

    const total = prepared.files.reduce((sum, file) => sum + file.bytes.length, 0);
    events.emit({ phase: "writing", current: 0, total });
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
          flashSize: esptoolFlashSizeValue(prepared.flashSize),
          eraseAll: false,
          compress: true,
          reportProgress(_fileIndex, written, compressedTotal) {
            const ratio = Number.isFinite(written) && Number.isFinite(compressedTotal) && compressedTotal > 0
              ? Math.min(1, Math.max(0, written / compressedTotal))
              : 0;
            const logicalPartBytes = Math.floor(file.bytes.length * ratio);
            events.emit({
              phase: "writing",
              part: file.kind,
              partIndex: index,
              partCount: prepared.files.length,
              current: Math.min(total, completed + logicalPartBytes),
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
      events.emit({
        phase: "writing",
        part: file.kind,
        partIndex: index,
        partCount: prepared.files.length,
        current: completed,
        total,
      });
      if (cancelRequested) {
        throw new FlashBridgeError("cancelled", "Flashing stopped at a verified part boundary.");
      }
    }
    cancellationLocked = false;
    events.emit({ phase: "verifying_flash", current: total, total });
    events.emit({ phase: "resetting" });
    const c6HardReset = prepared.afterReset === "hard-reset"
      && normalizeChipName(chipName) === normalizeChipName(ESP32C6_CHIP_NAME);
    try {
      if (c6HardReset) {
        await resetEspDevice(loader, prepared.afterReset, dependencies.resetSleep);
      } else {
        const proveReset = dependencies.proveReset ?? proveUsbReset;
        await proveReset(
          serial,
          port,
          () => resetEspDevice(loader, prepared.afterReset, dependencies.resetSleep),
          {
            timeoutMs: dependencies.resetEnumerationTimeoutMs,
            setTimeoutImpl: dependencies.setTimeoutImpl,
            clearTimeoutImpl: dependencies.clearTimeoutImpl,
          },
        );
      }
    } catch (error) {
      throw new FlashBridgeError(
        "reset_failure",
        "All parts verified, but USB disconnect and re-enumeration after reset were not observed.",
        { cause: error },
      );
    }
    if (cancelRequested) {
      throw new FlashBridgeError(
        "cancelled",
        "Cancellation was requested during writing; verification and reset finished safely, but success was not reported.",
      );
    }
    events.emit({
      phase: "success",
      current: total,
      total,
      ...(c6HardReset ? { message: ESP32C6_RESET_COMPLETION_MESSAGE } : {}),
    });
    return { success: true };
  } catch (error) {
    const resetFailure = error instanceof FlashBridgeError && error.code === "reset_failure";
    const failure = safeFailure(
      error,
      deviceLost && !resetFailure,
      cancellationLocked,
    );
    if (!events.terminal) {
      events.emit({ phase: failure.code === "cancelled" ? "cancelled" : "failed", ...failure });
    }
    retainPreparedPlan = failure.code === "permission_denied"
      && prepared.installMode === "preserve-data";
    throw error;
  } finally {
    active = false;
    setNavigationGuard(false, environment);
    try {
      await transport?.disconnect();
    } catch {
      // The device may already be gone after a successful reset. Cleanup remains best effort.
    }
    if (!retainPreparedPlan) {
      discardPrepared();
    }
    cancellationLocked = false;
  }
}

function throwEarlyFailure(events, error, discard = true) {
  const failure = safeFailure(error);
  if (discard) {
    discardPrepared();
  }
  if (!events.terminal) {
    events.emit({ phase: failure.code === "cancelled" ? "cancelled" : "failed", ...failure });
  }
  throw error;
}

export function cancel() {
  preparationGeneration += 1;
  discardPreparingRequest();
  if (!cancellationLocked) {
    cancelRequested = true;
  }
  cancelNrfBootloaderSelection();
}

export function clearPrepared() {
  preparationGeneration += 1;
  discardPreparingRequest();
  cancelRequested = active && !cancellationLocked;
  if (!active) {
    discardPrepared();
  }
  cancelNrfBootloaderSelection();
}

export function continueNrfBootloaderSelection() {
  return continuePendingNrfBootloaderSelection();
}

async function flashNrfSerialDfu(events, dependencies, environment) {
  active = true;
  cancelRequested = false;
  cancellationLocked = false;
  setNavigationGuard(true, environment);
  try {
    return await runNrfSerialDfu({
      prepared,
      events,
      dependencies,
      isCancelled: () => cancelRequested,
    });
  } catch (error) {
    const failure = safeFailure(error);
    if (!events.terminal) {
      events.emit({ phase: failure.code === "cancelled" ? "cancelled" : "failed", ...failure });
    }
    throw error;
  } finally {
    active = false;
    setNavigationGuard(false, environment);
    discardPrepared();
    cancellationLocked = false;
  }
}

function requireCurrentPreparation(generation) {
  if (generation !== preparationGeneration) {
    throw new FlashBridgeError("cancelled", "Preparation was cancelled before device access.");
  }
}

function downloadUf2(events, dependencies) {
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
    link.download = `prns-hopspot-${prepared.boardSlug}.uf2`;
    link.click();
    events.emit({
      phase: "download_requested",
      current: file.bytes.length,
      total: file.bytes.length,
      message: `Verified UF2 download requested. Check the browser's downloads, then copy it to ${prepared.mountLabel}; the bootloader drive disappears when the device reboots.`,
    });
    discardPrepared();
    return { downloadRequested: true };
  } finally {
    urlApi.revokeObjectURL(blobUrl);
  }
}

function mapBeforeReset(value) {
  return value === "usb-reset" ? "usb_reset" : "default_reset";
}

async function resetEspDevice(loader, afterReset, sleepImpl = sleep) {
  if (afterReset === "hard-reset") {
    if (normalizeChipName(loader.chip?.CHIP_NAME) === normalizeChipName(ESP32C6_CHIP_NAME)) {
      await resetEsp32C6UsbJtag(loader.transport, sleepImpl);
      return;
    }
    await loader.after("hard_reset");
    return;
  }
  if (
    afterReset !== "watchdog-reset"
    || normalizeChipName(loader.chip?.CHIP_NAME) !== "esp32s3"
  ) {
    throw new FlashBridgeError(
      "invalid_request",
      "The signed release requested an unsupported reset mode for the detected chip.",
    );
  }
  await loader.writeReg(ESP32S3_WDT_WPROTECT, ESP32S3_WDT_WRITE_KEY);
  await loader.writeReg(ESP32S3_WDT_CONFIG1, 2000);
  await loader.writeReg(ESP32S3_WDT_CONFIG0, ESP32S3_WDT_RESET_FLAGS);
  await loader.writeReg(ESP32S3_WDT_WPROTECT, 0);
}

async function resetEsp32C6UsbJtag(transport, sleepImpl) {
  await transport.setDTR(false);
  await sleepImpl(ESP32C6_RESET_SIGNAL_DELAY_MS);
  await transport.setRTS(true);
  await transport.setDTR(false);
  await transport.setRTS(true);
  await sleepImpl(ESP32C6_RESET_SIGNAL_DELAY_MS);
  await transport.setRTS(false);
}

function sleep(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

async function proveUsbReset(serial, selectedPort, reset, options = {}) {
  if (
    typeof serial?.addEventListener !== "function"
    || typeof serial?.removeEventListener !== "function"
  ) {
    throw new Error("Web Serial USB lifecycle events are unavailable.");
  }
  const selectedIdentity = usbPortIdentity(selectedPort);
  if (!selectedIdentity) {
    throw new Error("The selected serial port has no stable USB identity.");
  }
  const timeoutMs = options.timeoutMs ?? RESET_ENUMERATION_TIMEOUT_MS;
  const setTimeoutImpl = options.setTimeoutImpl ?? globalThis.setTimeout;
  const clearTimeoutImpl = options.clearTimeoutImpl ?? globalThis.clearTimeout;
  let disconnected = false;
  let timeout;
  let resolveEvidence;
  let rejectEvidence;
  const evidence = new Promise((resolve, reject) => {
    resolveEvidence = resolve;
    rejectEvidence = reject;
  });
  const onDisconnect = (event) => {
    if (event.target === selectedPort) {
      disconnected = true;
    }
  };
  const onConnect = (event) => {
    if (disconnected && sameUsbIdentity(selectedIdentity, usbPortIdentity(event.target))) {
      resolveEvidence();
    }
  };
  serial.addEventListener("disconnect", onDisconnect);
  serial.addEventListener("connect", onConnect);
  timeout = setTimeoutImpl(() => {
    rejectEvidence(new Error("Timed out waiting for USB disconnect and re-enumeration."));
  }, timeoutMs);
  try {
    await reset();
    await evidence;
  } finally {
    clearTimeoutImpl(timeout);
    serial.removeEventListener("disconnect", onDisconnect);
    serial.removeEventListener("connect", onConnect);
  }
}

function usbPortIdentity(port) {
  const info = port?.getInfo?.();
  if (!Number.isInteger(info?.usbVendorId) || !Number.isInteger(info?.usbProductId)) {
    return null;
  }
  return {
    vendorId: info.usbVendorId,
    productId: info.usbProductId,
  };
}

function sameUsbIdentity(expected, actual) {
  return actual !== null
    && actual.vendorId === expected.vendorId
    && actual.productId === expected.productId;
}

function setNavigationGuard(enabled, environment) {
  if (!environment.addEventListener || !environment.removeEventListener) {
    return;
  }
  if (enabled) {
    activeNavigationEnvironment = environment;
    environment.addEventListener("beforeunload", navigationGuard);
    environment.document?.addEventListener("click", internalNavigationGuard, true);
    installHistoryGuard(environment);
  } else {
    environment.removeEventListener("beforeunload", navigationGuard);
    environment.document?.removeEventListener("click", internalNavigationGuard, true);
    removeHistoryGuard(environment);
    activeNavigationEnvironment = null;
  }
}

function installHistoryGuard(environment) {
  const history = environment.history;
  const href = environment.location?.href;
  if (!history?.pushState || !href) return;
  const token = `prns-flash-${++historyGuardSequence}`;
  const priorState = history.state;
  const state = priorState && typeof priorState === "object" && !Array.isArray(priorState)
    ? { ...priorState, __prnsFlashGuard: token }
    : { __prnsFlashGuard: token };
  try {
    history.pushState(state, "", href);
    activeHistoryGuard = { environment, token };
    environment.addEventListener("popstate", historyNavigationGuard);
  } catch {
    activeHistoryGuard = null;
  }
}

function historyNavigationGuard(event) {
  if (!active || !activeHistoryGuard) return;
  event?.stopImmediatePropagation?.();
  try {
    activeHistoryGuard.environment.history?.forward?.();
  } catch {
    // The same-URL sentinel still prevents this traversal from changing the active route.
  }
}

function removeHistoryGuard(environment) {
  const guard = activeHistoryGuard;
  activeHistoryGuard = null;
  if (!guard || guard.environment !== environment) return;
  environment.removeEventListener("popstate", historyNavigationGuard);
  try {
    if (environment.history?.state?.__prnsFlashGuard === guard.token) {
      environment.history.back?.();
    }
  } catch {
    // Leaving a same-URL history entry is safer than disturbing a completed operation's route.
  }
}

function navigationGuard(event) {
  if (!active) return;
  event.preventDefault();
  event.returnValue = "";
}

function internalNavigationGuard(event) {
  if (!active || event.defaultPrevented || event.button > 0) return;
  const link = event.target?.closest?.("a[href]");
  if (!link || link.download || (link.target && link.target !== "_self")) return;
  const currentHref = activeNavigationEnvironment?.location?.href;
  if (!currentHref) return;
  const current = new URL(currentHref);
  const destination = new URL(link.href, current);
  if (destination.origin !== current.origin || destination.href === current.href) return;
  event.preventDefault();
  event.stopImmediatePropagation();
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
  prepared.nrfDfu?.session?.free?.();
  prepared = null;
}

function clearProvisioning(request) {
  if (request?.provisioning) {
    request.provisioning.password = "";
    request.provisioning.ssid = "";
    if (request.provisioning.tcpClient) {
      request.provisioning.tcpClient.host = "";
    }
  }
}

function discardPreparingRequest() {
  clearProvisioning(preparingRequest);
  preparingRequest = null;
}

export const testing = {
  prepared: () => prepared,
  proveUsbReset,
  reset() {
    preparationGeneration += 1;
    discardPreparingRequest();
    discardPrepared();
    active = false;
    cancelRequested = false;
    cancellationLocked = false;
    activeNavigationEnvironment = null;
    activeHistoryGuard = null;
  },
};
