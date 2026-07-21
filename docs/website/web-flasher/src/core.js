import SparkMD5 from "spark-md5";

export const CONFIG_OFFSET = 0xd000;
export const CONFIG_SIZE = 0x1000;
export const CONFIG_SSID_MAX_BYTES = 32;
export const CONFIG_PASSWORD_MAX_BYTES = 64;

const SHA256_PATTERN = /^[0-9a-f]{64}$/;
const ESP_PARTS = ["bootloader", "partition-table", "application"];

export class FlashBridgeError extends Error {
  constructor(code, message, options) {
    super(message, options);
    this.name = "FlashBridgeError";
    this.code = code;
  }
}

export function validateRequest(request) {
  if (!request || request.schema !== 1) {
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
    if (!Number.isSafeInteger(part.size) || part.size <= 0 || !SHA256_PATTERN.test(part.sha256 ?? "")) {
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
      || request.flashMode !== "dio"
      || request.flashFrequency !== "40m"
      || !["default-reset", "usb-reset"].includes(request.beforeReset)
      || !["hard-reset", "watchdog-reset"].includes(request.afterReset)
    ) {
      throw new FlashBridgeError("invalid_request", "The ESP target identity is incomplete.");
    }
    for (const [start, end] of ranges) {
      if (start < CONFIG_OFFSET + CONFIG_SIZE && CONFIG_OFFSET < end) {
        throw new FlashBridgeError("invalid_request", "Firmware overlaps the reserved configuration slot.");
      }
    }
    const config = request.provisioning;
    if (config) {
      if (config.offset !== CONFIG_OFFSET || config.size !== CONFIG_SIZE) {
        throw new FlashBridgeError("invalid_request", "The provisioning slot disagrees with the firmware contract.");
      }
    }
  }
  return request;
}

export function provisioningImage(provisioning) {
  if (!provisioning || provisioning.action === "preserve") {
    return null;
  }
  const image = new Uint8Array(CONFIG_SIZE);
  image.fill(0xff);
  image.set(new TextEncoder().encode("HSPCFG1\0"), 0);
  image[8] = 1;
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
  return image;
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

export function flashSizeValue(bytes) {
  const sizes = new Map([
    [4 * 1024 * 1024, "4MB"],
    [8 * 1024 * 1024, "8MB"],
    [16 * 1024 * 1024, "16MB"],
  ]);
  const value = sizes.get(bytes);
  if (!value) {
    throw new FlashBridgeError("invalid_request", "The target flash capacity is unsupported.");
  }
  return value;
}

export function safeFailure(error, deviceLost = false) {
  if (deviceLost) {
    return {
      code: "device_lost",
      message: "The USB device disconnected. Re-enter bootloader mode and restart the complete flash.",
    };
  }
  if (error instanceof FlashBridgeError) {
    return { code: error.code, message: error.message };
  }
  if (error?.name === "NotFoundError") {
    return { code: "permission_denied", message: "No serial port was selected." };
  }
  if (error?.name === "SecurityError") {
    return {
      code: "insecure_context",
      message: "Web Serial requires HTTPS or localhost and an explicit user gesture.",
    };
  }
  return {
    code: "flash_failed",
    message: "The device operation failed. Re-enter bootloader mode and restart the complete flash.",
  };
}
