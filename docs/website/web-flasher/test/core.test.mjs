import assert from "node:assert/strict";
import { webcrypto } from "node:crypto";
import test from "node:test";

import {
  FlashBridgeError,
  md5Hex,
  normalizeChipName,
  provisioningImage,
  sha256Hex,
  validateRequest,
} from "../src/core.js";

function request() {
  return {
    schema: 1,
    boardSlug: "heltec-v4",
    displayName: "Heltec LoRa 32 V4",
    transport: "esp-serial",
    expectedChip: "esp32s3",
    flashSize: 8 * 1024 * 1024,
    flashMode: "dio",
    flashFrequency: "40m",
    beforeReset: "usb-reset",
    afterReset: "watchdog-reset",
    provisioning: { action: "preserve", offset: 0xd000, size: 0x1000 },
    parts: [
      { kind: "bootloader", path: "bootloader.bin", url: "/bootloader.bin", offset: 0, size: 32, sha256: "a".repeat(64) },
      { kind: "partition-table", path: "partition.bin", url: "/partition.bin", offset: 0x8000, size: 32, sha256: "b".repeat(64) },
      { kind: "application", path: "app.bin", url: "/app.bin", offset: 0x10000, size: 32, sha256: "c".repeat(64) },
    ],
  };
}

test("valid sparse request is accepted", () => {
  assert.equal(validateRequest(request()).boardSlug, "heltec-v4");
});

test("provisioning overlap is rejected", () => {
  const value = request();
  value.parts[1].offset = 0xd000;
  assert.throws(() => validateRequest(value), FlashBridgeError);
});

test("reserved configuration overlap is rejected without provisioning", () => {
  const value = request();
  value.provisioning = null;
  value.parts[1].offset = 0xd000;
  assert.throws(() => validateRequest(value), /reserved configuration slot/);
});

test("sparse part order is canonical", () => {
  const value = request();
  [value.parts[0], value.parts[1]] = [value.parts[1], value.parts[0]];
  assert.throws(() => validateRequest(value), /invalid kind or offset/);
});

test("configuration uses UTF-8 byte limits without truncation", () => {
  assert.throws(
    () => provisioningImage({ action: "configure", ssid: "é".repeat(17), password: "" }),
    /34 bytes/,
  );
  const image = provisioningImage({ action: "clear" });
  assert.equal(image.length, 4096);
  assert.equal(image[10], 0);
  assert.equal(image[11], 0);
});

test("standard digest vectors match", async () => {
  const bytes = new TextEncoder().encode("test");
  assert.equal(await sha256Hex(bytes, webcrypto), "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08");
  assert.equal(md5Hex(bytes), "098f6bcd4621d373cade4e832627b4f6");
});

test("chip comparison is punctuation independent", () => {
  assert.equal(normalizeChipName("ESP32-S3"), normalizeChipName("esp32s3"));
});
