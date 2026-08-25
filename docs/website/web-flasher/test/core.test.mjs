import assert from "node:assert/strict";
import { webcrypto } from "node:crypto";
import test from "node:test";

import {
  esptoolFlashSizeValue,
  FlashBridgeError,
  flashSizeLabel,
  jedecFlashSizeBytes,
  md5Hex,
  normalizeChipName,
  provisioningImage,
  recoveryGuidance,
  sha256Hex,
  validateUf2Artifact,
  validateRequest,
} from "../src/core.js";
import { testingContract } from "../src/contract.js";

function request() {
  return {
    schema: 1,
    boardSlug: "heltec-v4",
    displayName: "Heltec LoRa 32 V4",
    transport: "esp-serial",
    installMode: "preserve-data",
    eraseConfirmed: false,
    expectedChip: "esp32s3",
    flashSize: 8 * 1024 * 1024,
    flashMode: "dio",
    flashFrequency: "40m",
    beforeReset: "usb-reset",
    afterReset: "watchdog-reset",
    mountLabel: null,
    uf2Compatibility: null,
    nrfSerialDfu: null,
    serialFilters: [{ usbVendorId: 0x303a }],
    provisioning: { action: "preserve", offset: 0xd000, size: 0x1000 },
    parts: [
      { kind: "bootloader", path: "firmware/hopspot/heltec-v4/0.2.6/bootloader.bin", url: "/releases/0.2.6/firmware/hopspot/heltec-v4/0.2.6/bootloader.bin", offset: 0, size: 32, sha256: "a".repeat(64) },
      { kind: "partition-table", path: "firmware/hopspot/heltec-v4/0.2.6/partition.bin", url: "/releases/0.2.6/firmware/hopspot/heltec-v4/0.2.6/partition.bin", offset: 0x8000, size: 32, sha256: "b".repeat(64) },
      { kind: "application", path: "firmware/hopspot/heltec-v4/0.2.6/app.bin", url: "/releases/0.2.6/firmware/hopspot/heltec-v4/0.2.6/app.bin", offset: 0x10000, size: 32, sha256: "c".repeat(64) },
    ],
  };
}

function nrfRequest() {
  const value = request();
  Object.assign(value, {
    boardSlug: "t1000-e",
    displayName: "Seeed Studio SenseCAP Card Tracker T1000-E",
    transport: "nrf-serial-dfu",
    expectedChip: null,
    flashSize: null,
    flashMode: null,
    flashFrequency: null,
    beforeReset: null,
    afterReset: null,
    provisioning: null,
    serialFilters: [{ usbVendorId: 0x2886, usbProductId: 0x0057 }],
    nrfSerialDfu: {
      entry: "managed-application",
      touchApplicationAndBootloaderUsb: { vendorId: 0x2886, productId: 0x0057 },
      touchBaudRate: 1_200,
      transferBaudRate: 115_200,
      managedApplication: {
        usb: { vendorId: 0x1209, productId: 0x0001 },
        manufacturer: "Stay Personal",
        product: "Personal Hopspot (T1000-E)",
        serialNumber: "PERSONAL-RNS-T1000E-HOP",
        interfaceNumber: 0,
        request: 0x50,
        value: 0x5052,
        index: 0x4e53,
      },
      compatibility: {
        softdeviceFamily: "s140",
        softdeviceVersion: "7.3.0",
        softdeviceFwids: [0x0123],
        deviceType: 0x0052,
        deviceRevision: 52840,
        applicationVersion: "not-enforced",
        applicationBase: 0x27000,
        applicationEndExclusive: 0xea000,
        bankLayout: "single",
      },
    },
    parts: [
      { kind: "dfu-application", path: "firmware/hopspot/t1000-e/0.3.7/app.bin", url: "/releases/0.3.7/firmware/hopspot/t1000-e/0.3.7/app.bin", offset: null, size: 32, sha256: "d".repeat(64) },
      { kind: "dfu-init-packet", path: "firmware/hopspot/t1000-e/0.3.7/app.dat", url: "/releases/0.3.7/firmware/hopspot/t1000-e/0.3.7/app.dat", offset: null, size: 14, sha256: "e".repeat(64) },
    ],
  });
  delete value.installMode;
  delete value.eraseConfirmed;
  return value;
}

test("valid sparse request is accepted", () => {
  assert.equal(validateRequest(request()).boardSlug, "heltec-v4");
});

test("ESP serial filters are explicit, bounded, and unique", () => {
  for (const serialFilters of [
    undefined,
    [],
    [{ usbVendorId: 0 }],
    [{ usbVendorId: 0x10000 }],
    [{ usbVendorId: 0x303a, usbProductId: 0x1001 }],
    [{ usbVendorId: 0x303a }, { usbVendorId: 0x303a }],
  ]) {
    const value = request();
    value.serialFilters = serialFilters;
    assert.throws(() => validateRequest(value), /serial device filter/);
  }
});

test("T1000-E Nordic serial DFU identity is exact and closed", () => {
  const value = nrfRequest();
  assert.equal(validateRequest(value).nrfSerialDfu.entry, "managed-application");
  value.nrfSerialDfu.entry = "touch-application-or-bootloader";
  assert.equal(validateRequest(value).nrfSerialDfu.entry, "touch-application-or-bootloader");

  for (const mutate of [
    (candidate) => { candidate.serialFilters[0].usbProductId = 0x0058; },
    (candidate) => { candidate.nrfSerialDfu.touchBaudRate = 2_400; },
    (candidate) => { candidate.nrfSerialDfu.managedApplication.product = "T1000-E"; },
    (candidate) => { candidate.nrfSerialDfu.managedApplication.request = 0x51; },
    (candidate) => { candidate.nrfSerialDfu.compatibility.softdeviceFwids = [0x00b6]; },
    (candidate) => { candidate.nrfSerialDfu.compatibility.applicationBase = 0x26000; },
    (candidate) => { candidate.parts.reverse(); },
  ]) {
    const candidate = nrfRequest();
    mutate(candidate);
    assert.throws(
      () => validateRequest(candidate),
      /Nordic serial DFU target|ordered application and init packet/,
    );
  }
});

test("transport-specific request identity is complete and bounded", () => {
  const esp = request();
  esp.flashSize = 32 * 1024 * 1024;
  assert.throws(() => validateRequest(esp), /target identity is incomplete/);

  const uf2 = request();
  Object.assign(uf2, {
    transport: "uf2-mass-storage",
    expectedChip: null,
    flashSize: null,
    flashMode: null,
    flashFrequency: null,
    beforeReset: null,
    afterReset: null,
    mountLabel: "TECHOBOOT",
    serialFilters: [],
    uf2Compatibility: {
      softdeviceFamily: "s140",
      softdeviceVersion: "7.3.0",
      fwid: 0x0123,
      applicationBase: 0x27000,
      familyId: 0xada52840,
    },
    provisioning: null,
    parts: [{
      kind: "uf2",
      path: "firmware/hopspot/t-echo/0.2.6/t-echo.uf2",
      url: "/releases/0.2.6/firmware/hopspot/t-echo/0.2.6/t-echo.uf2",
      offset: null,
      size: 32,
      sha256: "d".repeat(64),
    }],
  });
  delete uf2.installMode;
  delete uf2.eraseConfirmed;
  assert.equal(validateRequest(uf2).mountLabel, "TECHOBOOT");
  for (const mountLabel of ["", ".UF2", "BAD LABEL", "../UF2", "UF2/BOOT", "A".repeat(33)]) {
    uf2.mountLabel = mountLabel;
    assert.throws(() => validateRequest(uf2), /UF2 target identity is incomplete/);
  }
  for (const mountLabel of ["T114_BOOT", "UF2.1"]) {
    uf2.mountLabel = mountLabel;
    assert.equal(validateRequest(uf2).mountLabel, mountLabel);
  }
});

function uf2Block(applicationBase, familyId, blockNumber = 0, blockCount = 1) {
  const bytes = new Uint8Array(512);
  const view = new DataView(bytes.buffer);
  for (const [offset, value] of [
    [0, 0x0a324655],
    [4, 0x9e5d5157],
    [8, 0x00002000],
    [12, applicationBase + blockNumber * 256],
    [16, 256],
    [20, blockNumber],
    [24, blockCount],
    [28, familyId],
    [508, 0x0ab16f30],
  ]) {
    view.setUint32(offset, value, true);
  }
  return bytes;
}

test("UF2 structure is bound to the exact detected foundation", () => {
  const v6 = {
    softdeviceFamily: "s140",
    softdeviceVersion: "6.1.1",
    fwid: 0x00b6,
    applicationBase: 0x26000,
    familyId: 0xada52840,
  };
  const v7 = {
    softdeviceFamily: "s140",
    softdeviceVersion: "7.3.0",
    fwid: 0x0123,
    applicationBase: 0x27000,
    familyId: 0xada52840,
  };
  assert.equal(validateUf2Artifact(uf2Block(v6.applicationBase, v6.familyId), v6, "t-echo").length, 512);
  assert.equal(validateUf2Artifact(uf2Block(v7.applicationBase, v7.familyId), v7, "t-echo").length, 512);

  const corrupt = uf2Block(v7.applicationBase, v7.familyId);
  corrupt[0] = 0;
  assert.throws(() => validateUf2Artifact(corrupt, v7, "t-echo"), /block magic/);

  const reordered = uf2Block(v7.applicationBase, v7.familyId, 1, 2);
  assert.throws(() => validateUf2Artifact(reordered, v7, "t-echo"), /block sequence/);
  assert.throws(
    () => validateUf2Artifact(uf2Block(v7.applicationBase + 256, v7.familyId), v7, "t-echo"),
    /application address/,
  );
  assert.throws(
    () => validateUf2Artifact(uf2Block(v7.applicationBase, 0x12345678), v7, "t-echo"),
    /family ID/,
  );
  assert.throws(
    () => validateUf2Artifact(uf2Block(v6.applicationBase, v6.familyId), v7, "t-echo"),
    /application address/,
  );
});

function uf2Image(applicationBase, familyId, blockCount) {
  const bytes = new Uint8Array(blockCount * 512);
  for (let index = 0; index < blockCount; index += 1) {
    bytes.set(uf2Block(applicationBase, familyId, index, blockCount), index * 512);
  }
  return bytes;
}

test("the UF2 application region bound is pinned per board", () => {
  const v6 = {
    softdeviceFamily: "s140",
    softdeviceVersion: "6.1.1",
    fwid: 0x00b6,
    applicationBase: 0x26000,
    familyId: 0xada52840,
  };
  const pastTechoEnd = (0xc0000 - v6.applicationBase) / 256 + 1;
  const image = uf2Image(v6.applicationBase, v6.familyId, pastTechoEnd);
  assert.throws(() => validateUf2Artifact(image, v6, "t-echo"), /payload bounds/);
  assert.equal(validateUf2Artifact(image, v6, "t096").length, image.length);
  assert.equal(validateUf2Artifact(image, v6, "t114").length, image.length);
  const pastT096End = (0xe8000 - v6.applicationBase) / 256 + 1;
  assert.throws(
    () => validateUf2Artifact(uf2Image(v6.applicationBase, v6.familyId, pastT096End), v6, "t096"),
    /payload bounds/,
  );
  const v7 = {
    softdeviceFamily: "s140",
    softdeviceVersion: "7.3.0",
    fwid: 0x0123,
    applicationBase: 0x27000,
    familyId: 0xada52840,
  };
  assert.equal(validateUf2Artifact(uf2Block(v7.applicationBase, v7.familyId), v7, "t1000-e").length, 512);
  const pastT1000End = (0xea000 - v7.applicationBase) / 256 + 1;
  assert.throws(
    () => validateUf2Artifact(uf2Image(v7.applicationBase, v7.familyId, pastT1000End), v7, "t1000-e"),
    /payload bounds/,
  );
  assert.throws(
    () => validateUf2Artifact(uf2Block(v6.applicationBase, v6.familyId), v6, "nrf52840-second-board"),
    /pinned application region/,
  );
});

test("ESP install mode requires an exact destructive confirmation", () => {
  const fresh = request();
  fresh.installMode = "erase-all";
  assert.throws(() => validateRequest(fresh), /target identity is incomplete/);
  fresh.eraseConfirmed = true;
  fresh.provisioning = null;
  assert.equal(validateRequest(fresh).installMode, "erase-all");

  const standard = request();
  standard.eraseConfirmed = true;
  assert.throws(() => validateRequest(standard), /target identity is incomplete/);

  const unknown = request();
  unknown.installMode = "erase";
  assert.throws(() => validateRequest(unknown), /target identity is incomplete/);
});

test("fresh install accepts blank or explicitly configured provisioning only", () => {
  const blank = request();
  blank.installMode = "erase-all";
  blank.eraseConfirmed = true;
  blank.provisioning = null;
  assert.equal(validateRequest(blank).provisioning, null);

  const configured = request();
  configured.installMode = "erase-all";
  configured.eraseConfirmed = true;
  configured.provisioning.action = "configure";
  assert.equal(validateRequest(configured).provisioning.action, "configure");

  const misleadingPreserve = request();
  misleadingPreserve.installMode = "erase-all";
  misleadingPreserve.eraseConfirmed = true;
  assert.throws(() => validateRequest(misleadingPreserve), /explicitly configured new provisioning/);
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

test("artifact paths and URLs must be exact normalized immutable locations", () => {
  for (const path of [
    "firmware/%2e%2e/application.bin",
    "firmware/%252e%252e/application.bin",
    "firmware/../application.bin",
    "firmware//application.bin",
  ]) {
    const value = request();
    value.parts[0].path = path;
    value.parts[0].url = `/releases/0.2.6/${path}`;
    assert.throws(() => validateRequest(value), /not normalized/);
  }

  for (const url of [
    "/releases/0.2.6/firmware/hopspot/heltec-v4/0.2.6/../bootloader.bin",
    "/releases/%30.2.6/firmware/hopspot/heltec-v4/0.2.6/bootloader.bin",
    "https://reticulum.rs/releases/0.2.6/firmware/hopspot/heltec-v4/0.2.6/bootloader.bin",
  ]) {
    const value = request();
    value.parts[0].url = url;
    assert.throws(() => validateRequest(value), /not immutable|not normalized/);
  }
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

test("configuration encodes one numeric or DNS TCP target", () => {
  const numeric = provisioningImage({
    action: "configure",
    ssid: "mesh",
    password: "",
    tcpClient: { hostKind: "ipv4", host: "192.0.2.10", port: 4242 },
  });
  assert.equal(numeric[9], 1);
  assert.equal(numeric[112], 4);
  assert.deepEqual(Array.from(numeric.slice(113, 119)), [0x10, 0x92, 192, 0, 2, 10]);

  const hostname = provisioningImage({
    action: "configure",
    ssid: "mesh",
    password: "",
    tcpClient: { hostKind: "hostname", host: "node.example", port: 5252 },
  });
  assert.equal(hostname[9], 2);
  assert.equal(hostname[112], 12);
  assert.deepEqual(Array.from(hostname.slice(113, 115)), [0x14, 0x84]);
  assert.equal(new TextDecoder().decode(hostname.slice(115, 127)), "node.example");
});

test("configuration rejects malformed TCP targets", () => {
  for (const tcpClient of [
    { hostKind: "ipv4", host: "0.0.0.0", port: 4242 },
    { hostKind: "ipv4", host: "192.0.2.01", port: 4242 },
    { hostKind: "hostname", host: "Node.Example", port: 4242 },
    { hostKind: "hostname", host: "node..example", port: 4242 },
    { hostKind: "hostname", host: "node.example", port: 0 },
  ]) {
    assert.throws(
      () => provisioningImage({
        action: "configure",
        ssid: "mesh",
        password: "",
        tcpClient,
      }),
      FlashBridgeError,
    );
  }
});

test("standard digest vectors match", async () => {
  const bytes = new TextEncoder().encode("test");
  assert.equal(await sha256Hex(bytes, webcrypto), "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08");
  assert.equal(md5Hex(bytes), "098f6bcd4621d373cade4e832627b4f6");
});

test("chip comparison is punctuation independent", () => {
  assert.equal(normalizeChipName("ESP32-S3"), normalizeChipName("esp32s3"));
});

test("JEDEC capacity decoding accepts known IDs and fails closed on unknown IDs", () => {
  assert.equal(jedecFlashSizeBytes(0x1640ef), 4 * 1024 * 1024);
  assert.equal(jedecFlashSizeBytes(0x1740ef), 8 * 1024 * 1024);
  assert.equal(jedecFlashSizeBytes(0x3640ef), 4 * 1024 * 1024);
  assert.equal(jedecFlashSizeBytes(0x9940ef), null);
  assert.equal(jedecFlashSizeBytes(0xffffff), null);
  assert.equal(jedecFlashSizeBytes(Number.NaN), null);
});

test("binary flash capacities use IEC units", () => {
  assert.equal(flashSizeLabel(4 * 1024 * 1024), "4 MiB");
  assert.equal(flashSizeLabel(8 * 1024 * 1024), "8 MiB");
  assert.equal(flashSizeLabel(16 * 1024 * 1024), "16 MiB");
});

test("esptool flash capacities use exact API tokens", () => {
  assert.equal(esptoolFlashSizeValue(4 * 1024 * 1024), "4MB");
  assert.equal(esptoolFlashSizeValue(8 * 1024 * 1024), "8MB");
  assert.equal(esptoolFlashSizeValue(16 * 1024 * 1024), "16MB");
});

test("every production bridge error has actionable recovery guidance", () => {
  for (const code of testingContract.errors) {
    const guidance = recoveryGuidance(code);
    assert.match(
      guidance,
      /reload|correct|open|review|disconnect|re-check|do not|reconnect|re-enter|press|prepare|finish/i,
      code,
    );
  }
});
