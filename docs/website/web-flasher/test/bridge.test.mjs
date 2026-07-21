import assert from "node:assert/strict";
import { createHash, webcrypto } from "node:crypto";
import test from "node:test";

import { cancel, clearPrepared, flash, prepare, testing } from "../src/prns-flash.js";

function bytes(value) {
  return new Uint8Array([value, value + 1, value + 2, value + 3]);
}

function sha(data) {
  return createHash("sha256").update(data).digest("hex");
}

function request() {
  const payloads = [bytes(1), bytes(5), bytes(9)];
  return {
    payloads,
    value: {
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
      provisioning: { action: "configure", offset: 0xd000, size: 0x1000, ssid: "local", password: "private" },
      parts: [
        { kind: "bootloader", path: "a", url: "https://example.test/a", offset: 0, size: 4, sha256: sha(payloads[0]) },
        { kind: "partition-table", path: "b", url: "https://example.test/b", offset: 0x8000, size: 4, sha256: sha(payloads[1]) },
        { kind: "application", path: "c", url: "https://example.test/c", offset: 0x10000, size: 4, sha256: sha(payloads[2]) },
      ],
    },
  };
}

test.beforeEach(() => testing.reset());

test("prepare verifies every artifact and never sends credentials", async () => {
  const { value, payloads } = request();
  const calls = [];
  const events = [];
  await prepare(value, (event) => events.push(event), {
    loadEsptool: false,
    cryptoImpl: webcrypto,
    fetchImpl: async (url, options) => {
      calls.push({ url, options });
      const data = payloads[calls.length - 1];
      return { ok: true, arrayBuffer: async () => data.buffer.slice(0) };
    },
  });
  assert.deepEqual(calls.map((call) => call.options.credentials), ["omit", "omit", "omit"]);
  assert.equal(JSON.stringify(calls).includes("private"), false);
  assert.equal(JSON.stringify(events).includes("private"), false);
  assert.equal(JSON.stringify(events).includes("local"), false);
  assert.equal(events.at(-1).phase, "ready");
  assert.equal(testing.prepared().files.length, 4);
  const configurationBytes = testing.prepared().files.at(-1).bytes;
  assert.equal(configurationBytes.some((byte) => byte !== 0), true);
  assert.equal(value.provisioning.ssid, "");
  assert.equal(value.provisioning.password, "");
  clearPrepared();
  assert.equal(testing.prepared(), null);
  assert.equal(configurationBytes.every((byte) => byte === 0), true);
});

test("throwing preparation event consumers cannot retain provisioning bytes", async () => {
  const { value, payloads } = request();
  let fetchIndex = 0;
  let configurationBytes;
  await assert.rejects(
    prepare(value, (event) => {
      if (event.phase === "ready") {
        configurationBytes = testing.prepared().files.at(-1).bytes;
        throw new Error("consumer stopped");
      }
      if (event.phase === "failed") {
        throw new Error("consumer stopped");
      }
    }, {
      loadEsptool: false,
      cryptoImpl: webcrypto,
      fetchImpl: async () => {
        const data = payloads[fetchIndex++];
        return { ok: true, arrayBuffer: async () => data.buffer.slice(0) };
      },
    }),
    /consumer stopped/,
  );
  assert.ok(configurationBytes);
  assert.equal(testing.prepared(), null);
  assert.equal(configurationBytes.every((byte) => byte === 0), true);
});

test("hash mismatch fails before serial access", async () => {
  const { value, payloads } = request();
  payloads[0][0] = 99;
  const events = [];
  await assert.rejects(
    prepare(value, (event) => events.push(event), {
      loadEsptool: false,
      cryptoImpl: webcrypto,
      fetchImpl: async () => ({ ok: true, arrayBuffer: async () => payloads[0].buffer.slice(0) }),
    }),
  );
  assert.equal(events.at(-1).code, "artifact_hash_mismatch");
});

test("wrong chip is rejected and the port is released", async () => {
  const { value, payloads } = request();
  let fetchIndex = 0;
  await prepare(value, () => {}, {
    loadEsptool: false,
    cryptoImpl: webcrypto,
    fetchImpl: async () => {
      const data = payloads[fetchIndex++];
      return { ok: true, arrayBuffer: async () => data.buffer.slice(0) };
    },
  });
  const configurationBytes = testing.prepared().files.at(-1).bytes;
  let disconnected = false;
  class FakeTransport {
    setDeviceLostCallback() {}
    async disconnect() { disconnected = true; }
  }
  class FakeLoader {
    async main() { return "ESP32-C6"; }
  }
  const events = [];
  await assert.rejects(
    flash((event) => events.push(event), {
      environment: { isSecureContext: true, addEventListener() {}, removeEventListener() {} },
      serial: { requestPort: async () => ({}) },
      TransportImpl: FakeTransport,
      LoaderImpl: FakeLoader,
    }),
  );
  assert.equal(disconnected, true);
  assert.equal(events.at(-1).code, "wrong_chip");
  assert.equal(testing.prepared(), null);
  assert.equal(configurationBytes.every((byte) => byte === 0), true);
});

test("successful flash requires MD5 callback and cleans up", async () => {
  const { value, payloads } = request();
  let fetchIndex = 0;
  await prepare(value, () => {}, {
    loadEsptool: false,
    cryptoImpl: webcrypto,
    fetchImpl: async () => {
      const data = payloads[fetchIndex++];
      return { ok: true, arrayBuffer: async () => data.buffer.slice(0) };
    },
  });
  const configurationBytes = testing.prepared().files.at(-1).bytes;
  let disconnected = false;
  class FakeTransport {
    setDeviceLostCallback() {}
    async disconnect() { disconnected = true; }
  }
  class FakeLoader {
    async main() { return "ESP32-S3"; }
    async detectFlashSize() { return "8MB"; }
    async writeFlash(options) {
      assert.equal(options.eraseAll, false);
      assert.equal(options.compress, true);
      assert.match(options.calculateMD5Hash(options.fileArray[0].data), /^[0-9a-f]{32}$/);
      options.reportProgress(0, options.fileArray[0].data.length, options.fileArray[0].data.length);
    }
    async after(mode) { assert.equal(mode, "hard_reset"); }
  }
  const events = [];
  await flash((event) => events.push(event), {
    environment: { isSecureContext: true, addEventListener() {}, removeEventListener() {} },
    serial: { requestPort: async () => ({}) },
    TransportImpl: FakeTransport,
    LoaderImpl: FakeLoader,
  });
  assert.equal(disconnected, true);
  assert.equal(events.at(-1).phase, "success");
  assert.equal(testing.prepared(), null);
  assert.equal(configurationBytes.every((byte) => byte === 0), true);
});

async function prepareDefault() {
  const { value, payloads } = request();
  let fetchIndex = 0;
  await prepare(value, () => {}, {
    loadEsptool: false,
    cryptoImpl: webcrypto,
    fetchImpl: async () => {
      const data = payloads[fetchIndex++];
      return { ok: true, arrayBuffer: async () => data.buffer.slice(0) };
    },
  });
}

function environment() {
  return { isSecureContext: true, addEventListener() {}, removeEventListener() {} };
}

test("permission cancellation is distinct and happens before transport creation", async () => {
  await prepareDefault();
  const events = [];
  const denied = Object.assign(new Error("cancelled"), { name: "NotFoundError" });
  await assert.rejects(
    flash((event) => events.push(event), {
      environment: environment(),
      serial: { requestPort: async () => { throw denied; } },
      TransportImpl: class { constructor() { assert.fail("transport must not be created"); } },
      LoaderImpl: class {},
    }),
  );
  assert.equal(events.at(-1).code, "permission_denied");
});

test("unsupported and insecure browser failures emit terminal bridge events", async () => {
  await prepareDefault();
  const insecureConfiguration = testing.prepared().files.at(-1).bytes;
  const insecureEvents = [];
  await assert.rejects(
    flash((event) => insecureEvents.push(event), {
      environment: { isSecureContext: false },
    }),
  );
  assert.deepEqual(
    { phase: insecureEvents.at(-1).phase, code: insecureEvents.at(-1).code },
    { phase: "failed", code: "insecure_context" },
  );
  assert.equal(testing.prepared(), null);
  assert.equal(insecureConfiguration.every((byte) => byte === 0), true);

  await prepareDefault();
  const unsupportedConfiguration = testing.prepared().files.at(-1).bytes;
  const unsupportedEvents = [];
  await assert.rejects(
    flash((event) => unsupportedEvents.push(event), {
      environment: environment(),
    }),
  );
  assert.deepEqual(
    { phase: unsupportedEvents.at(-1).phase, code: unsupportedEvents.at(-1).code },
    { phase: "failed", code: "unsupported_browser" },
  );
  assert.equal(testing.prepared(), null);
  assert.equal(unsupportedConfiguration.every((byte) => byte === 0), true);
});

test("throwing early-failure consumers cannot retain provisioning bytes", async () => {
  await prepareDefault();
  const configurationBytes = testing.prepared().files.at(-1).bytes;

  await assert.rejects(
    flash(() => {
      throw new Error("consumer stopped");
    }, {
      environment: { isSecureContext: false },
    }),
    /consumer stopped/,
  );
  assert.equal(testing.prepared(), null);
  assert.equal(configurationBytes.every((byte) => byte === 0), true);
});

test("device-side MD5 mismatch is a verification failure and releases the port", async () => {
  await prepareDefault();
  let disconnected = false;
  class FakeTransport {
    setDeviceLostCallback() {}
    async disconnect() { disconnected = true; }
  }
  class FakeLoader {
    async main() { return "ESP32-S3"; }
    async detectFlashSize() { return "8MB"; }
    async writeFlash() { throw new Error("MD5 of file does not match data in flash"); }
  }
  const events = [];
  await assert.rejects(
    flash((event) => events.push(event), {
      environment: environment(),
      serial: { requestPort: async () => ({}) },
      TransportImpl: FakeTransport,
      LoaderImpl: FakeLoader,
    }),
  );
  assert.equal(events.at(-1).code, "verification_failure");
  assert.equal(disconnected, true);
});

test("device loss takes precedence over a generic write failure", async () => {
  await prepareDefault();
  let lost;
  class FakeTransport {
    setDeviceLostCallback(callback) { lost = callback; }
    async disconnect() {}
  }
  class FakeLoader {
    async main() { return "ESP32-S3"; }
    async detectFlashSize() { return "8MB"; }
    async writeFlash() { lost(); throw new Error("serial closed"); }
  }
  const events = [];
  await assert.rejects(
    flash((event) => events.push(event), {
      environment: environment(),
      serial: { requestPort: async () => ({}) },
      TransportImpl: FakeTransport,
      LoaderImpl: FakeLoader,
    }),
  );
  assert.equal(events.at(-1).code, "device_lost");
});

test("reset failure is reported only after writes verify", async () => {
  await prepareDefault();
  class FakeTransport {
    setDeviceLostCallback() {}
    async disconnect() {}
  }
  class FakeLoader {
    async main() { return "ESP32-S3"; }
    async detectFlashSize() { return "8MB"; }
    async writeFlash() {}
    async after() { throw new Error("reset unavailable"); }
  }
  const events = [];
  await assert.rejects(
    flash((event) => events.push(event), {
      environment: environment(),
      serial: { requestPort: async () => ({}) },
      TransportImpl: FakeTransport,
      LoaderImpl: FakeLoader,
    }),
  );
  assert.equal(events.at(-1).code, "reset_failure");
});

test("cancellation stops at the next verified part boundary", async () => {
  await prepareDefault();
  const configurationBytes = testing.prepared().files.at(-1).bytes;
  let writes = 0;
  class FakeTransport {
    setDeviceLostCallback() {}
    async disconnect() {}
  }
  class FakeLoader {
    async main() { return "ESP32-S3"; }
    async detectFlashSize() { return "8MB"; }
    async writeFlash() { writes += 1; cancel(); }
  }
  const events = [];
  await assert.rejects(
    flash((event) => events.push(event), {
      environment: environment(),
      serial: { requestPort: async () => ({}) },
      TransportImpl: FakeTransport,
      LoaderImpl: FakeLoader,
    }),
  );
  assert.equal(writes, 1);
  assert.equal(events.at(-1).phase, "cancelled");
  assert.equal(testing.prepared(), null);
  assert.equal(configurationBytes.every((byte) => byte === 0), true);
});

test("UF2 completion reports delivery guidance without claiming device verification", async () => {
  const payload = bytes(21);
  const value = {
    schema: 1,
    boardSlug: "t-echo",
    displayName: "LilyGO T-Echo",
    transport: "uf2-mass-storage",
    expectedChip: null,
    flashSize: null,
    flashMode: null,
    flashFrequency: null,
    beforeReset: null,
    afterReset: null,
    provisioning: null,
    parts: [{
      kind: "uf2",
      path: "t-echo.uf2",
      url: "https://example.test/t-echo.uf2",
      offset: null,
      size: payload.length,
      sha256: sha(payload),
    }],
  };
  await prepare(value, () => {}, {
    cryptoImpl: webcrypto,
    fetchImpl: async () => ({ ok: true, arrayBuffer: async () => payload.buffer.slice(0) }),
  });
  let clicked = false;
  const events = [];
  await flash((event) => events.push(event), {
    BlobImpl: class {},
    urlApi: { createObjectURL: () => "blob:test", revokeObjectURL() {} },
    documentImpl: {
      createElement: () => ({
        click() { clicked = true; },
      }),
    },
  });
  assert.equal(clicked, true);
  assert.match(events.at(-1).message, /Copy it to TECHOBOOT/);
  assert.doesNotMatch(events.at(-1).message, /device-side verification/i);
});
