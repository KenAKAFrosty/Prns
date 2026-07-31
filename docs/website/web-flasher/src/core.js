import SparkMD5 from "spark-md5";

import { BRIDGE_SCHEMA, RESPONSE_LIMITS } from "./contract.js";

export const CONFIG_OFFSET = 0xd000;
export const CONFIG_SIZE = 0x1000;
export const CONFIG_SSID_MAX_BYTES = 32;
export const CONFIG_PASSWORD_MAX_BYTES = 64;
export const CONFIG_TCP_HOSTNAME_MAX_BYTES = 253;

const CONFIG_TCP_HOST_LENGTH_OFFSET = 112;
const CONFIG_TCP_PORT_OFFSET = 113;
const CONFIG_TCP_TARGET_OFFSET = 115;

const SHA256_PATTERN = /^[0-9a-f]{64}$/;
const VERSION_PATTERN = /^[A-Za-z0-9.+-]+$/;
const PATH_COMPONENT_PATTERN = /^[A-Za-z0-9._+-]+$/;
const MOUNT_LABEL_PATTERN = /^[A-Za-z0-9][A-Za-z0-9._-]{0,31}$/;
const ESP_PARTS = ["bootloader", "partition-table", "application"];
const INSTALL_MODES = new Set(["preserve-data", "erase-all"]);
const FLASH_SIZE_PROFILES = new Map([
  [4 * 1024 * 1024, Object.freeze({ label: "4 MiB", esptool: "4MB" })],
  [8 * 1024 * 1024, Object.freeze({ label: "8 MiB", esptool: "8MB" })],
  [16 * 1024 * 1024, Object.freeze({ label: "16 MiB", esptool: "16MB" })],
]);
const JEDEC_FLASH_CAPACITIES = new Map([
  [0x16, 4 * 1024 * 1024],
  [0x17, 8 * 1024 * 1024],
  [0x18, 16 * 1024 * 1024],
  [0x36, 4 * 1024 * 1024],
  [0x37, 8 * 1024 * 1024],
  [0x38, 16 * 1024 * 1024],
]);

const RECOVERY_GUIDANCE = Object.freeze({
  invalid_request: "Reload this page to rebuild the signed plan; if it repeats, use the CLI and report the release version.",
  invalid_config: "Correct the local configuration values, then prepare and verify the release again.",
  unsupported_browser: "Open this page in current Chrome or Edge over HTTPS, or use the standalone CLI.",
  insecure_context: "Reopen the flasher over HTTPS or localhost before trying again.",
  permission_denied: "Review the selected board, retry, and choose its serial port in the browser prompt.",
  connection_failure: "Disconnect the board, follow its BOOT/RESET preparation steps, reconnect it, and restart the complete operation.",
  wrong_chip: "Re-check the printed board label, select the matching board and serial port, then prepare the release again.",
  wrong_flash_size: "Re-check the printed board label and serial port; do not write this plan to a device with a different flash capacity.",
  erase_failure: "The device may be blank. Reconnect it, re-enter BOOT mode, select Fresh install, confirm the destructive action again, and retry the complete fresh-install plan from the beginning.",
  artifact_fetch: "Do not connect the device. Check the network, reload this page, and prepare the signed release again.",
  artifact_size_mismatch: "Do not connect the device. Reload this page and prepare again; if it repeats, use the CLI and report the release version.",
  artifact_hash_mismatch: "Do not connect the device. Reload this page and prepare again; if it repeats, use the CLI and report the release version.",
  device_lost: "Reconnect the board, follow its BOOT/RESET preparation steps, and restart the complete sparse plan from the beginning.",
  write_failure: "Re-enter BOOT mode, press RESET as instructed for this board, and restart the complete sparse plan.",
  verification_failure: "Do not boot the partial image. Re-enter BOOT mode and restart the complete sparse plan from the beginning.",
  reset_failure: "The firmware bytes are verified, but automatic reboot was not confirmed. Press RESET and check the next boot; if firmware does not start, re-enter BOOT mode and repeat the complete plan.",
  cancelled: "Review the current board state, re-enter bootloader mode if writing began, and restart the complete plan when ready.",
  not_prepared: "Prepare and verify the signed release again before requesting device access.",
  busy: "Finish or safely cancel the active operation before starting another one.",
  flash_failed: "Re-enter bootloader mode and restart the complete flash; use the CLI if the browser path fails again.",
});

export class FlashBridgeError extends Error {
  constructor(code, message, options) {
    super(message, options);
    this.name = "FlashBridgeError";
    this.code = code;
  }
}

export class BoundedResponseError extends Error {
  constructor(code, message, options) {
    super(message, options);
    this.name = "BoundedResponseError";
    this.code = code;
  }
}

export async function readBoundedBytes(response, maximumBytes, afterChunk = () => {}) {
  if (
    !Number.isSafeInteger(maximumBytes)
    || maximumBytes <= 0
    || maximumBytes > RESPONSE_LIMITS.artifact_bytes
  ) {
    throw new BoundedResponseError("invalid_limit", "The response safety limit is invalid.");
  }

  let declaredLength;
  try {
    declaredLength = response?.headers?.get?.("content-length") ?? null;
  } catch (error) {
    throw new BoundedResponseError("stream_failure", "The response headers could not be read.", {
      cause: error,
    });
  }
  if (declaredLength !== null) {
    const normalized = String(declaredLength).trim();
    if (!/^(0|[1-9][0-9]*)$/.test(normalized)) {
      throw new BoundedResponseError("stream_failure", "The response length header is invalid.");
    }
    if (BigInt(normalized) > BigInt(maximumBytes)) {
      throw new BoundedResponseError("response_too_large", "The response exceeds its safety limit.");
    }
  }

  let reader;
  try {
    reader = response?.body?.getReader?.();
  } catch (error) {
    throw new BoundedResponseError("stream_failure", "The response stream could not be opened.", {
      cause: error,
    });
  }
  if (!reader?.read) {
    throw new BoundedResponseError(
      "stream_failure",
      "The browser did not expose a bounded response stream.",
    );
  }

  const output = new Uint8Array(maximumBytes);
  let total = 0;
  try {
    while (true) {
      let result;
      try {
        result = await reader.read();
      } catch (error) {
        throw new BoundedResponseError("stream_failure", "The response stream stopped early.", {
          cause: error,
        });
      }
      if (result?.done) break;
      const chunk = asByteView(result?.value);
      if (chunk.byteLength > maximumBytes - total) {
        try {
          await reader.cancel?.("response safety limit exceeded");
        } catch {
          // The size violation remains authoritative even if cancellation races a closed stream.
        }
        throw new BoundedResponseError(
          "response_too_large",
          "The response exceeds its safety limit.",
        );
      }
      output.set(chunk, total);
      total += chunk.byteLength;
      await afterChunk(total);
    }
  } finally {
    try {
      reader.releaseLock?.();
    } catch {
      // A failed or cancelled stream may already have released its reader.
    }
  }
  return total === maximumBytes ? output : output.slice(0, total);
}

function asByteView(value) {
  if (value instanceof Uint8Array) return value;
  if (ArrayBuffer.isView(value)) {
    return new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
  }
  throw new BoundedResponseError("stream_failure", "The response stream returned invalid bytes.");
}

export function validateRequest(request) {
  if (!request || request.schema !== BRIDGE_SCHEMA) {
    throw new FlashBridgeError("invalid_request", "The flasher request schema is unsupported.");
  }
  if (!/^[a-z0-9-]+$/.test(request.boardSlug ?? "")) {
    throw new FlashBridgeError("invalid_request", "The selected board identifier is invalid.");
  }
  if (!Array.isArray(request.parts) || request.parts.length === 0) {
    throw new FlashBridgeError("invalid_request", "The signed release has no firmware parts.");
  }
  const transport = request.transport;
  if (transport !== "esp-serial" && transport !== "uf2-mass-storage") {
    throw new FlashBridgeError("invalid_request", "The signed release transport is unsupported.");
  }

  const ranges = [];
  const kinds = new Set();
  for (const [partIndex, part] of request.parts.entries()) {
    if (!part || typeof part.url !== "string" || typeof part.path !== "string") {
      throw new FlashBridgeError("invalid_request", "A firmware part has no immutable path.");
    }
    validateArtifactLocation(part);
    if (
      !Number.isSafeInteger(part.size)
      || part.size <= 0
      || part.size > RESPONSE_LIMITS.artifact_bytes
      || !SHA256_PATTERN.test(part.sha256 ?? "")
    ) {
      throw new FlashBridgeError("invalid_request", "A firmware part has invalid size or SHA-256 metadata.");
    }
    if (kinds.has(part.kind)) {
      throw new FlashBridgeError("invalid_request", "The signed release repeats a firmware part kind.");
    }
    kinds.add(part.kind);
    if (transport === "esp-serial") {
      if (part.kind !== ESP_PARTS[partIndex] || !Number.isSafeInteger(part.offset) || part.offset < 0) {
        throw new FlashBridgeError("invalid_request", "An ESP firmware part has an invalid kind or offset.");
      }
      const end = part.offset + part.size;
      if (!Number.isSafeInteger(end) || end > request.flashSize) {
        throw new FlashBridgeError("invalid_request", "An ESP firmware part exceeds physical flash.");
      }
      ranges.push([part.offset, end]);
    } else if (part.kind !== "uf2" || part.offset !== null || request.parts.length !== 1) {
      throw new FlashBridgeError("invalid_request", "A UF2 target must contain one offset-free UF2 file.");
    }
  }
  if (transport === "esp-serial") {
    if (request.parts.length !== ESP_PARTS.length) {
      throw new FlashBridgeError("invalid_request", "The ESP release must contain the three ordered sparse parts.");
    }
    ranges.sort((left, right) => left[0] - right[0]);
    for (let index = 1; index < ranges.length; index += 1) {
      if (ranges[index - 1][1] > ranges[index][0]) {
        throw new FlashBridgeError("invalid_request", "Sparse firmware parts overlap.");
      }
    }
    if (
      !request.expectedChip
      || !Number.isSafeInteger(request.flashSize)
      || !FLASH_SIZE_PROFILES.has(request.flashSize)
      || !INSTALL_MODES.has(request.installMode)
      || typeof request.eraseConfirmed !== "boolean"
      || request.eraseConfirmed !== (request.installMode === "erase-all")
      || request.flashMode !== "dio"
      || request.flashFrequency !== "40m"
      || !["default-reset", "usb-reset"].includes(request.beforeReset)
      || !["hard-reset", "watchdog-reset"].includes(request.afterReset)
      || request.mountLabel !== null
    ) {
      throw new FlashBridgeError("invalid_request", "The ESP target identity is incomplete.");
    }
    for (const [start, end] of ranges) {
      if (start < CONFIG_OFFSET + CONFIG_SIZE && CONFIG_OFFSET < end) {
        throw new FlashBridgeError("invalid_request", "Firmware overlaps the reserved configuration slot.");
      }
    }
    const config = request.provisioning;
    if (
      request.installMode === "erase-all"
      && config !== null
      && config !== undefined
      && config.action !== "configure"
    ) {
      throw new FlashBridgeError(
        "invalid_request",
        "A fresh install may only add explicitly configured new provisioning.",
      );
    }
    if (config) {
      if (config.offset !== CONFIG_OFFSET || config.size !== CONFIG_SIZE) {
        throw new FlashBridgeError("invalid_request", "The provisioning slot disagrees with the firmware contract.");
      }
    }
  } else {
    if (
      typeof request.mountLabel !== "string"
      || !MOUNT_LABEL_PATTERN.test(request.mountLabel)
      || request.expectedChip !== null
      || request.flashSize !== null
      || request.flashMode !== null
      || request.flashFrequency !== null
      || request.beforeReset !== null
      || request.afterReset !== null
      || request.provisioning !== null
      || request.installMode !== undefined
      || request.eraseConfirmed !== undefined
    ) {
      throw new FlashBridgeError("invalid_request", "The UF2 target identity is incomplete.");
    }
  }
  return request;
}

function validateArtifactLocation(part) {
  if (
    part.path.length === 0 ||
    part.path.includes("%") ||
    part.path.includes("\\") ||
    part.path.includes("?") ||
    part.path.includes("#") ||
    part.path.split("/").some((component) =>
      !component || component === "." || component === ".." || !PATH_COMPONENT_PATTERN.test(component))
  ) {
    throw new FlashBridgeError("invalid_request", "A firmware artifact path is not normalized.");
  }
  const match = /^\/releases\/([^/]+)\/(.+)$/.exec(part.url);
  if (!match || !VERSION_PATTERN.test(match[1]) || match[1].toLowerCase() === "next" || match[2] !== part.path) {
    throw new FlashBridgeError("invalid_request", "A firmware artifact URL is not immutable.");
  }
  const resolved = new URL(part.url, "https://reticulum.rs");
  if (
    resolved.origin !== "https://reticulum.rs" ||
    resolved.pathname !== part.url ||
    resolved.search ||
    resolved.hash ||
    resolved.href !== `https://reticulum.rs${part.url}`
  ) {
    throw new FlashBridgeError("invalid_request", "A firmware artifact URL is not normalized.");
  }
}

export function provisioningImage(provisioning) {
  if (!provisioning || provisioning.action === "preserve") {
    return null;
  }
  const image = new Uint8Array(CONFIG_SIZE);
  image.fill(0xff);
  image.set(new TextEncoder().encode("HSPCFG1\0"), 0);
  image[8] = 1;
  image[9] = 0;
  if (provisioning.action === "clear") {
    image[10] = 0;
    image[11] = 0;
    return image;
  }
  if (provisioning.action !== "configure") {
    throw new FlashBridgeError("invalid_request", "Unknown provisioning action.");
  }
  const ssid = new TextEncoder().encode(provisioning.ssid ?? "");
  const password = new TextEncoder().encode(provisioning.password ?? "");
  if (ssid.length === 0) {
    throw new FlashBridgeError("invalid_config", "Wi-Fi SSID cannot be empty.");
  }
  if (ssid.length > CONFIG_SSID_MAX_BYTES) {
    throw new FlashBridgeError(
      "invalid_config",
      `Wi-Fi SSID is ${ssid.length} bytes; maximum is ${CONFIG_SSID_MAX_BYTES}.`,
    );
  }
  if (password.length > CONFIG_PASSWORD_MAX_BYTES) {
    throw new FlashBridgeError(
      "invalid_config",
      `Wi-Fi password is ${password.length} bytes; maximum is ${CONFIG_PASSWORD_MAX_BYTES}.`,
    );
  }
  image[10] = ssid.length;
  image[11] = password.length;
  image.set(ssid, 16);
  image.set(password, 16 + CONFIG_SSID_MAX_BYTES);
  const tcpClient = provisioning.tcpClient;
  if (tcpClient !== null && tcpClient !== undefined) {
    if (
      typeof tcpClient !== "object"
      || !Number.isInteger(tcpClient.port)
      || tcpClient.port < 1
      || tcpClient.port > 65535
    ) {
      throw new FlashBridgeError("invalid_config", "TCP client port must be between 1 and 65535.");
    }
    let target;
    if (tcpClient.hostKind === "ipv4") {
      const octets = parseIpv4(tcpClient.host);
      if (
        octets === null
        || octets.every(byte => byte === 0)
        || octets.every(byte => byte === 255)
        || (octets[0] >= 224 && octets[0] <= 239)
      ) {
        throw new FlashBridgeError("invalid_config", "TCP client IPv4 address is not a usable unicast target.");
      }
      image[9] = 1;
      target = Uint8Array.from(octets);
    } else if (tcpClient.hostKind === "hostname") {
      if (!validHostname(tcpClient.host)) {
        throw new FlashBridgeError("invalid_config", "TCP client hostname is not canonical.");
      }
      target = new TextEncoder().encode(tcpClient.host);
      if (target.length > CONFIG_TCP_HOSTNAME_MAX_BYTES) {
        throw new FlashBridgeError(
          "invalid_config",
          `TCP client hostname is ${target.length} bytes; maximum is ${CONFIG_TCP_HOSTNAME_MAX_BYTES}.`,
        );
      }
      image[9] = 2;
    } else {
      throw new FlashBridgeError("invalid_config", "TCP client target kind is unsupported.");
    }
    image[CONFIG_TCP_HOST_LENGTH_OFFSET] = target.length;
    image[CONFIG_TCP_PORT_OFFSET] = tcpClient.port >>> 8;
    image[CONFIG_TCP_PORT_OFFSET + 1] = tcpClient.port & 0xff;
    image.set(target, CONFIG_TCP_TARGET_OFFSET);
  }
  return image;
}

function parseIpv4(value) {
  if (typeof value !== "string") {
    return null;
  }
  const parts = value.split(".");
  if (parts.length !== 4) {
    return null;
  }
  const octets = parts.map(part => {
    if (!/^(0|[1-9][0-9]{0,2})$/.test(part)) {
      return NaN;
    }
    return Number(part);
  });
  return octets.every(octet => Number.isInteger(octet) && octet <= 255) ? octets : null;
}

function validHostname(value) {
  if (typeof value !== "string" || value.length === 0 || value.length > CONFIG_TCP_HOSTNAME_MAX_BYTES) {
    return false;
  }
  return value.split(".").every(label => (
    label.length > 0
    && label.length <= 63
    && /^[a-z0-9](?:[a-z0-9-]*[a-z0-9])?$/.test(label)
  ));
}

export async function sha256Hex(bytes, cryptoImpl = globalThis.crypto) {
  if (!cryptoImpl?.subtle) {
    throw new FlashBridgeError("unsupported_browser", "This browser cannot verify SHA-256 artifacts.");
  }
  const view = bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength);
  const digest = new Uint8Array(await cryptoImpl.subtle.digest("SHA-256", view));
  return Array.from(digest, (byte) => byte.toString(16).padStart(2, "0")).join("");
}

export function md5Hex(bytes) {
  const view = bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength);
  return SparkMD5.ArrayBuffer.hash(view);
}

export function normalizeChipName(name) {
  return String(name ?? "")
    .toLowerCase()
    .replace(/[^a-z0-9]/g, "");
}

function flashSizeProfile(bytes) {
  const profile = FLASH_SIZE_PROFILES.get(bytes);
  if (!profile) {
    throw new FlashBridgeError("invalid_request", "The target flash capacity is unsupported.");
  }
  return profile;
}

export function flashSizeLabel(bytes) {
  return flashSizeProfile(bytes).label;
}

export function esptoolFlashSizeValue(bytes) {
  return flashSizeProfile(bytes).esptool;
}

export function jedecFlashSizeBytes(flashId) {
  if (!Number.isSafeInteger(flashId) || flashId < 0 || flashId > 0xffffffff) {
    return null;
  }
  const capacityId = (flashId >>> 16) & 0xff;
  return JEDEC_FLASH_CAPACITIES.get(capacityId) ?? null;
}

export function recoveryGuidance(code) {
  return RECOVERY_GUIDANCE[code] ?? RECOVERY_GUIDANCE.flash_failed;
}

export function safeFailure(error, deviceLost = false, completeFreshInstallRequired = false) {
  if (deviceLost) {
    return recoverableFailure(
      "device_lost",
      "The USB device disconnected.",
      completeFreshInstallRequired,
    );
  }
  if (error instanceof FlashBridgeError) {
    return recoverableFailure(error.code, error.message, completeFreshInstallRequired);
  }
  if (error?.name === "NotFoundError") {
    return recoverableFailure("permission_denied", "No serial port was selected.");
  }
  if (error?.name === "SecurityError") {
    return recoverableFailure(
      "insecure_context",
      "Web Serial requires HTTPS or localhost and an explicit user gesture.",
    );
  }
  return recoverableFailure("flash_failed", "The device operation failed.");
}

function recoverableFailure(code, message, completeFreshInstallRequired = false) {
  const guidance = completeFreshInstallRequired
    ? RECOVERY_GUIDANCE.erase_failure
    : recoveryGuidance(code);
  return { code, message: `${message} ${guidance}` };
}
