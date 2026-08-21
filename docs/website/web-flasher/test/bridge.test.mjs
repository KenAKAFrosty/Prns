import assert from "node:assert/strict";
import { createHash, webcrypto } from "node:crypto";
import test from "node:test";

import {
  cancel,
  clearPrepared,
  fetchSignedDocuments,
  flash,
  prepare,
  testing,
} from "../src/prns-flash.js";
import { testingContract } from "../src/contract.js";

const TERMINAL_PHASES = new Set(
  testingContract.phases.filter(({ terminal }) => terminal).map(({ wire }) => wire),
);
const FLASH_ID_4_MB = 0x1640ef;
const FLASH_ID_8_MB = 0x1740ef;

function bytes(value) {
  return new Uint8Array([value, value + 1, value + 2, value + 3]);
}

function streamedResponse(chunks, options = {}) {
  const values = (Array.isArray(chunks) ? chunks : [chunks]).map((chunk) =>
    chunk instanceof Uint8Array ? chunk : new Uint8Array(chunk));
  let index = 0;
  const state = { cancelled: false, released: false };
  return {
    ok: options.ok ?? true,
    state,
    headers: {
      get(name) {
        return name.toLowerCase() === "content-length"
          ? (options.contentLength ?? null)
          : null;
      },
    },
    body: options.withoutBody ? null : {
      getReader() {
        return {
          async read() {
            if (options.readErrorAt === index) throw new Error("body stream stopped");
            if (index >= values.length) return { done: true, value: undefined };
            return { done: false, value: values[index++] };
          },
          async cancel() { state.cancelled = true; },
          releaseLock() { state.released = true; },
        };
      },
    },
    async arrayBuffer() {
      assert.fail("bounded acquisition must not call arrayBuffer()");
    },
  };
}

function sha(data) {
  return createHash("sha256").update(data).digest("hex");
}

function uf2Bytes(applicationBase, familyId) {
  const payload = new Uint8Array(512);
  const view = new DataView(payload.buffer);
  for (const [offset, value] of [
    [0, 0x0a324655],
    [4, 0x9e5d5157],
    [8, 0x00002000],
    [12, applicationBase],
    [16, 256],
    [20, 0],
    [24, 1],
    [28, familyId],
    [508, 0x0ab16f30],
  ]) {
    view.setUint32(offset, value, true);
  }
  return payload;
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
      provisioning: { action: "configure", offset: 0xd000, size: 0x1000, ssid: "local", password: "private" },
      parts: [
        { kind: "bootloader", path: "firmware/hopspot/heltec-v4/0.2.6/a.bin", url: "/releases/0.2.6/firmware/hopspot/heltec-v4/0.2.6/a.bin", offset: 0, size: 4, sha256: sha(payloads[0]) },
        { kind: "partition-table", path: "firmware/hopspot/heltec-v4/0.2.6/b.bin", url: "/releases/0.2.6/firmware/hopspot/heltec-v4/0.2.6/b.bin", offset: 0x8000, size: 4, sha256: sha(payloads[1]) },
        { kind: "application", path: "firmware/hopspot/heltec-v4/0.2.6/c.bin", url: "/releases/0.2.6/firmware/hopspot/heltec-v4/0.2.6/c.bin", offset: 0x10000, size: 4, sha256: sha(payloads[2]) },
      ],
    },
  };
}

function terminalEvents(events) {
  return events.filter(({ phase }) => TERMINAL_PHASES.has(phase));
}

test.beforeEach(() => testing.reset());

test("signed release documents are streamed sequentially within shared limits", async () => {
  const descriptor = new TextEncoder().encode('{"version":"0.2.6"}');
  const signature = new TextEncoder().encode("untrusted comment\nsignature\n");
  const calls = [];
  const result = await fetchSignedDocuments({
    documentUrl: "/releases/channels/stable.json",
    documentMaxBytes: testingContract.response_limits.channel_bytes,
    signatureMaxBytes: testingContract.response_limits.signature_bytes,
  }, {
    environment: {
      location: {
        origin: "https://reticulum.rs",
        hostname: "reticulum.rs",
      },
    },
    fetchImpl: async (url, options) => {
      calls.push({ url, options });
      const payload = calls.length === 1 ? descriptor : signature;
      return streamedResponse([payload.subarray(0, 2), payload.subarray(2)]);
    },
  });

  assert.deepEqual(result, {
    status: "ready",
    document: new TextDecoder().decode(descriptor),
    signature: new TextDecoder().decode(signature),
  });
  assert.deepEqual(calls.map(({ url }) => url), [
    "/releases/channels/stable.json",
    "/releases/channels/stable.json.minisig",
  ]);
  assert.deepEqual(calls.map(({ options }) => options.credentials), ["omit", "omit"]);
});

test("oversized signed documents stop before the signature response is fetched", async () => {
  let response;
  let fetches = 0;
  const result = await fetchSignedDocuments({
    documentUrl: "https://reticulum.rs/releases/0.2.6/flash-manifest.json",
    documentMaxBytes: testingContract.response_limits.manifest_bytes,
    signatureMaxBytes: testingContract.response_limits.signature_bytes,
  }, {
    environment: {
      location: {
        origin: "http://127.0.0.1:4173",
        hostname: "127.0.0.1",
      },
    },
    fetchImpl: async (url) => {
      fetches += 1;
      assert.equal(url, "/releases/0.2.6/flash-manifest.json");
      response = streamedResponse(new Uint8Array([1]), {
        contentLength: String(testingContract.response_limits.manifest_bytes + 1),
      });
      return response;
    },
  });

  assert.deepEqual(result, { status: "error", error: "too_large" });
  assert.equal(fetches, 1);
  assert.equal(response.state.released, false);
});

test("prepare verifies every artifact and never sends credentials", async () => {
  const { value, payloads } = request();
  value.provisioning.tcpClient = {
    hostKind: "hostname",
    host: "node.example",
    port: 4242,
  };
  const calls = [];
  const events = [];
  await prepare(value, (event) => events.push(event), {
    loadEsptool: false,
    cryptoImpl: webcrypto,
    fetchImpl: async (url, options) => {
      calls.push({ url, options });
      const data = payloads[calls.length - 1];
      return streamedResponse(data);
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
  assert.equal(value.provisioning.tcpClient.host, "");
  clearPrepared();
  assert.equal(testing.prepared(), null);
  assert.equal(configurationBytes.every((byte) => byte === 0), true);
});

test("a stale delayed preparation cannot replace or clear the newer verified plan", async () => {
  const first = request();
  const second = request();
  second.value.boardSlug = "newer-board";
  let releaseFirstFetch;
  let markFirstFetchStarted;
  const firstFetchStarted = new Promise((resolve) => { markFirstFetchStarted = resolve; });
  const firstFetchGate = new Promise((resolve) => { releaseFirstFetch = resolve; });
  let firstIndex = 0;
  const stale = prepare(first.value, () => {}, {
    loadEsptool: false,
    cryptoImpl: webcrypto,
    fetchImpl: async () => {
      markFirstFetchStarted();
      await firstFetchGate;
      const data = first.payloads[firstIndex++];
      return streamedResponse(data);
    },
  });
  await firstFetchStarted;
  clearPrepared();
  assert.equal(first.value.provisioning.ssid, "");
  assert.equal(first.value.provisioning.password, "");

  let secondIndex = 0;
  await prepare(second.value, () => {}, {
    loadEsptool: false,
    cryptoImpl: webcrypto,
    fetchImpl: async () => {
      const data = second.payloads[secondIndex++];
      return streamedResponse(data);
    },
  });
  assert.equal(testing.prepared().boardSlug, "newer-board");

  releaseFirstFetch();
  await assert.rejects(stale, /cancelled/i);
  assert.equal(testing.prepared().boardSlug, "newer-board");
  assert.equal(first.value.provisioning.ssid, "");
  assert.equal(first.value.provisioning.password, "");
});

test("a preparation rejected as busy clears the rejected credentials", async () => {
  await prepareDefault();
  let releaseWrite;
  let markWriteStarted;
  const writeStarted = new Promise((resolve) => { markWriteStarted = resolve; });
  class FakeTransport {
    setDeviceLostCallback() {}
    async disconnect() {}
  }
  class FakeLoader {
    chip = { CHIP_NAME: "ESP32-S3" };

    async main() { return "ESP32-S3 (QFN56) (revision v0.2)"; }
    async readFlashId() { return FLASH_ID_8_MB; }
    async writeFlash() {
      if (!releaseWrite) {
        markWriteStarted();
        await new Promise((resolve) => { releaseWrite = resolve; });
      }
    }
    async after() {}
  }
  const flashing = flash(() => {}, {
    environment: environment(),
    serial: { requestPort: async () => ({}) },
    TransportImpl: FakeTransport,
    LoaderImpl: FakeLoader,
    proveReset: async () => {},
  });
  await writeStarted;

  const rejected = request();
  const events = [];
  let fetches = 0;
  await assert.rejects(
    prepare(rejected.value, (event) => events.push(event), {
      loadEsptool: false,
      cryptoImpl: webcrypto,
      fetchImpl: async () => { fetches += 1; return { ok: false }; },
    }),
    /already active/,
  );
  assert.equal(rejected.value.provisioning.ssid, "");
  assert.equal(rejected.value.provisioning.password, "");
  assert.equal(fetches, 0);
  assert.deepEqual(
    terminalEvents(events).map(({ phase, code }) => ({ phase, code })),
    [{ phase: "failed", code: "busy" }],
  );

  releaseWrite();
  await flashing;
});

test("throwing preparation event consumers cannot retain provisioning bytes", async () => {
  const { value, payloads } = request();
  let fetchIndex = 0;
  let configurationBytes;
  const terminalPhases = [];
  await assert.rejects(
    prepare(value, (event) => {
      if (event.phase === "ready") {
        terminalPhases.push(event.phase);
        configurationBytes = testing.prepared().files.at(-1).bytes;
        throw new Error("consumer stopped");
      }
      if (event.phase === "failed") {
        terminalPhases.push(event.phase);
        throw new Error("consumer stopped");
      }
    }, {
      loadEsptool: false,
      cryptoImpl: webcrypto,
      fetchImpl: async () => {
        const data = payloads[fetchIndex++];
        return streamedResponse(data);
      },
    }),
    /consumer stopped/,
  );
  assert.ok(configurationBytes);
  assert.deepEqual(terminalPhases, ["ready"]);
  assert.equal(testing.prepared(), null);
  assert.equal(configurationBytes.every((byte) => byte === 0), true);
});

test("hash mismatch fails before serial access", async () => {
  const { value, payloads } = request();
  value.installMode = "erase-all";
  value.eraseConfirmed = true;
  value.provisioning = null;
  payloads[0][0] = 99;
  const events = [];
  await assert.rejects(
    prepare(value, (event) => events.push(event), {
      loadEsptool: false,
      cryptoImpl: webcrypto,
      fetchImpl: async () => streamedResponse(payloads[0]),
    }),
  );
  assert.equal(events.at(-1).code, "artifact_hash_mismatch");
  let serialRequests = 0;
  await assert.rejects(
    flash(() => {}, {
      serial: { requestPort: async () => { serialRequests += 1; } },
    }),
    /Prepare and verify/,
  );
  assert.equal(serialRequests, 0);
});

test("artifact acquisition accepts exact multi-chunk streams without buffering fallbacks", async () => {
  const { value, payloads } = request();
  let fetchIndex = 0;
  await prepare(value, () => {}, {
    loadEsptool: false,
    cryptoImpl: webcrypto,
    fetchImpl: async () => {
      const payload = payloads[fetchIndex++];
      return streamedResponse([payload.subarray(0, 1), payload.subarray(1, 3), payload.subarray(3)]);
    },
  });
  assert.equal(testing.prepared().files.length, 4);
});

test("artifact acquisition failures retain their typed boundary before device access", async () => {
  let oversized;
  const cases = [
    {
      name: "fetch rejection",
      code: "artifact_fetch",
      fetchImpl: async () => { throw new Error("network unavailable"); },
    },
    {
      name: "HTTP rejection",
      code: "artifact_fetch",
      fetchImpl: async () => ({ ok: false }),
    },
    {
      name: "response body rejection",
      code: "artifact_fetch",
      fetchImpl: async () => streamedResponse(bytes(1), { readErrorAt: 0 }),
    },
    {
      name: "partial response body",
      code: "artifact_size_mismatch",
      fetchImpl: async () => streamedResponse(new Uint8Array([1, 2, 3])),
    },
    {
      name: "oversized chunked response body",
      code: "artifact_size_mismatch",
      fetchImpl: async () => {
        oversized = streamedResponse([bytes(1), new Uint8Array([99])]);
        return oversized;
      },
      check: () => assert.equal(oversized.state.cancelled, true),
    },
    {
      name: "oversized declared response body",
      code: "artifact_size_mismatch",
      fetchImpl: async () => streamedResponse(bytes(1), { contentLength: "5" }),
    },
    {
      name: "response without a readable stream",
      code: "artifact_fetch",
      fetchImpl: async () => streamedResponse(bytes(1), { withoutBody: true }),
    },
  ];

  for (const scenario of cases) {
    testing.reset();
    const { value } = request();
    const events = [];
    await assert.rejects(
      prepare(value, (event) => events.push(event), {
        loadEsptool: false,
        cryptoImpl: webcrypto,
        fetchImpl: scenario.fetchImpl,
      }),
      scenario.name,
    );

    assert.deepEqual(
      terminalEvents(events).map(({ phase, code }) => ({ phase, code })),
      [{ phase: "failed", code: scenario.code }],
      scenario.name,
    );
    assert.equal(testing.prepared(), null, scenario.name);
    assert.equal(value.provisioning.ssid, "", scenario.name);
    assert.equal(value.provisioning.password, "", scenario.name);
    assert.equal(JSON.stringify(events).includes("local"), false, scenario.name);
    assert.equal(JSON.stringify(events).includes("private"), false, scenario.name);
    scenario.check?.();
  }
});

test("an artifact above the shared safety ceiling is rejected before fetch", async () => {
  const { value } = request();
  value.parts[0].size = testingContract.response_limits.artifact_bytes + 1;
  let fetches = 0;
  const events = [];
  await assert.rejects(
    prepare(value, (event) => events.push(event), {
      loadEsptool: false,
      cryptoImpl: webcrypto,
      fetchImpl: async () => {
        fetches += 1;
        return streamedResponse(bytes(1));
      },
    }),
  );
  assert.equal(fetches, 0);
  assert.equal(events.at(-1).code, "invalid_request");
});

test("wrong chip is rejected and the port is released", async () => {
  const { value, payloads } = request();
  let fetchIndex = 0;
  await prepare(value, () => {}, {
    loadEsptool: false,
    cryptoImpl: webcrypto,
    fetchImpl: async () => {
      const data = payloads[fetchIndex++];
      return streamedResponse(data);
    },
  });
  const configurationBytes = testing.prepared().files.at(-1).bytes;
  let disconnected = false;
  class FakeTransport {
    setDeviceLostCallback() {}
    async disconnect() { disconnected = true; }
  }
  class FakeLoader {
    chip = { CHIP_NAME: "ESP32-C6" };

    async main() { return "ESP32-S3 (QFN56) (revision v0.2)"; }
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
  assert.deepEqual(
    terminalEvents(events).map(({ phase, code }) => ({ phase, code })),
    [{ phase: "failed", code: "wrong_chip" }],
  );
  assert.equal(
    events.find(({ phase }) => phase === "verifying_target").detectedChip,
    "ESP32-C6",
    "the canonical loader.chip.CHIP_NAME must override main()'s descriptive string",
  );
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
      return streamedResponse(data);
    },
  });
  const configurationBytes = testing.prepared().files.at(-1).bytes;
  let disconnected = false;
  const timeline = [];
  const watchdogWrites = [];
  class FakeTransport {
    setDeviceLostCallback() {}
    async disconnect() { disconnected = true; }
  }
  class FakeLoader {
    chip = { CHIP_NAME: "ESP32-S3" };

    async main() { timeline.push("chip-detected"); return "ESP32-S3 (QFN56) (revision v0.2)"; }
    async readFlashId() { timeline.push("flash-size-checked"); return FLASH_ID_8_MB; }
    async eraseFlash() { assert.fail("standard install must never erase the full flash"); }
    async writeFlash(options) {
      assert.equal(options.eraseAll, false);
      assert.equal(options.compress, true);
      assert.equal(options.flashSize, "8MB");
      assert.match(options.calculateMD5Hash(options.fileArray[0].data), /^[0-9a-f]{32}$/);
      options.reportProgress(0, options.fileArray[0].data.length, options.fileArray[0].data.length);
      timeline.push("part-write-and-md5-complete");
    }
    async writeReg(address, value) {
      watchdogWrites.push([address, value]);
      if (watchdogWrites.length === 4) timeline.push("reset-complete");
    }
  }
  const events = [];
  let serialPickerOptions;
  await flash((event) => {
    events.push(event);
    timeline.push(`event:${event.phase}`);
  }, {
    environment: { isSecureContext: true, addEventListener() {}, removeEventListener() {} },
    serial: { requestPort: async (options) => { serialPickerOptions = options; return {}; } },
    TransportImpl: FakeTransport,
    LoaderImpl: FakeLoader,
    proveReset: async (_serial, _port, reset) => reset(),
  });
  assert.equal(disconnected, true);
  assert.deepEqual(serialPickerOptions, { filters: [{ usbVendorId: 0x303a }] });
  assert.equal(events.at(-1).phase, "success");
  assert.ok(
    timeline.indexOf("event:verifying_target") < timeline.indexOf("flash-size-checked"),
    "target verification must be presented as ongoing until flash capacity is checked",
  );
  assert.equal(
    timeline.filter((entry) => entry === "part-write-and-md5-complete").length,
    4,
  );
  assert.ok(
    timeline.lastIndexOf("part-write-and-md5-complete") < timeline.indexOf("event:verifying_flash"),
    "the verification-complete phase must follow every writeFlash MD5 result",
  );
  assert.ok(
    timeline.indexOf("event:verifying_flash") < timeline.indexOf("event:resetting"),
  );
  assert.ok(timeline.indexOf("event:resetting") < timeline.indexOf("reset-complete"));
  assert.deepEqual(watchdogWrites, [
    [0x600080b0, 0x50d83aa1],
    [0x6000809c, 2000],
    [0x60008098, 0xd0000104],
    [0x600080b0, 0],
  ]);
  assert.equal(testing.prepared(), null);
  assert.equal(configurationBytes.every((byte) => byte === 0), true);
});

test("compressed write progress is scaled to logical manifest bytes", async () => {
  const { value, payloads } = request();
  value.provisioning.action = "preserve";
  let fetchIndex = 0;
  await prepare(value, () => {}, {
    loadEsptool: false,
    cryptoImpl: webcrypto,
    fetchImpl: async () => {
      const data = payloads[fetchIndex++];
      return streamedResponse(data);
    },
  });
  class FakeTransport {
    setDeviceLostCallback() {}
    async disconnect() {}
  }
  class FakeLoader {
    chip = { CHIP_NAME: "ESP32-S3" };

    async main() { return "ESP32-S3 (QFN56) (revision v0.2)"; }
    async readFlashId() { return FLASH_ID_8_MB; }
    async writeFlash(options) {
      options.reportProgress(0, 5, 10);
    }
    async after() {}
  }
  const events = [];
  await flash((event) => events.push(event), {
    environment: { isSecureContext: true, addEventListener() {}, removeEventListener() {} },
    serial: { requestPort: async () => ({}) },
    TransportImpl: FakeTransport,
    LoaderImpl: FakeLoader,
    proveReset: async () => {},
  });

  const halfBootloader = events.find(
    (event) => event.phase === "writing" && event.part === "bootloader" && event.current === 2,
  );
  assert.ok(halfBootloader, "5/10 compressed bytes must map to 2/4 logical bootloader bytes");
  assert.equal(halfBootloader.total, 12);
  assert.equal(events.at(-1).phase, "success");
  assert.equal(events.at(-1).current, 12);
});

test("active writes install and remove the navigation guard", async () => {
  await prepareDefault();
  let navigationGuard;
  let internalNavigationGuard;
  let popstateGuard;
  let releaseWrite;
  let writeStarted;
  let writes = 0;
  const writing = new Promise((resolve) => { writeStarted = resolve; });
  const historyState = { route: "/flash/heltec-v4" };
  let historyBacks = 0;
  let historyForwards = 0;
  let pushedHref;
  class FakeTransport {
    setDeviceLostCallback() {}
    async disconnect() {}
  }
  class FakeLoader {
    chip = { CHIP_NAME: "ESP32-S3" };

    async main() { return "ESP32-S3 (QFN56) (revision v0.2)"; }
    async readFlashId() { return FLASH_ID_8_MB; }
    async writeFlash() {
      writes += 1;
      if (writes > 1) return;
      writeStarted();
      await new Promise((resolve) => { releaseWrite = resolve; });
    }
    async after() {}
  }
  const testEnvironment = {
    isSecureContext: true,
    location: { href: "https://reticulum.rs/flash/heltec-v4" },
    history: {
      state: historyState,
      pushState(state, _title, href) {
        this.state = state;
        pushedHref = href;
      },
      forward() { historyForwards += 1; },
      back() {
        historyBacks += 1;
        this.state = historyState;
      },
    },
    document: {
      addEventListener(name, listener, capture) {
        assert.deepEqual([name, capture], ["click", true]);
        internalNavigationGuard = listener;
      },
      removeEventListener(name, listener, capture) {
        assert.deepEqual([name, listener, capture], ["click", internalNavigationGuard, true]);
        internalNavigationGuard = undefined;
      },
    },
    addEventListener(name, listener) {
      if (name === "beforeunload") navigationGuard = listener;
      else if (name === "popstate") popstateGuard = listener;
      else assert.fail(`unexpected navigation listener ${name}`);
    },
    removeEventListener(name, listener) {
      if (name === "beforeunload") {
        assert.equal(listener, navigationGuard);
        navigationGuard = undefined;
      } else if (name === "popstate") {
        assert.equal(listener, popstateGuard);
        popstateGuard = undefined;
      } else {
        assert.fail(`unexpected navigation listener ${name}`);
      }
    },
  };

  const operation = flash(() => {}, {
    environment: testEnvironment,
    serial: { requestPort: async () => ({}) },
    TransportImpl: FakeTransport,
    LoaderImpl: FakeLoader,
    proveReset: async () => {},
  });
  await writing;
  let prevented = false;
  const event = {
    preventDefault() { prevented = true; },
    returnValue: undefined,
  };
  navigationGuard(event);
  assert.equal(prevented, true);
  assert.equal(event.returnValue, "");
  assert.equal(pushedHref, testEnvironment.location.href);
  assert.equal(typeof popstateGuard, "function");
  let historyPropagationStopped = false;
  popstateGuard({ stopImmediatePropagation() { historyPropagationStopped = true; } });
  assert.equal(historyPropagationStopped, true);
  assert.equal(historyForwards, 1);

  let internalPrevented = false;
  let propagationStopped = false;
  internalNavigationGuard({
    button: 0,
    defaultPrevented: false,
    preventDefault() { internalPrevented = true; },
    stopImmediatePropagation() { propagationStopped = true; },
    target: {
      closest() {
        return {
          download: "",
          href: "https://reticulum.rs/flash/xiao-esp32-c6",
          target: "",
        };
      },
    },
  });
  assert.deepEqual({ internalPrevented, propagationStopped }, {
    internalPrevented: true,
    propagationStopped: true,
  });

  releaseWrite();
  await operation;
  assert.equal(navigationGuard, undefined);
  assert.equal(internalNavigationGuard, undefined);
  assert.equal(popstateGuard, undefined);
  assert.equal(historyBacks, 1);
});

async function prepareDefault() {
  const { value, payloads } = request();
  let fetchIndex = 0;
  await prepare(value, () => {}, {
    loadEsptool: false,
    cryptoImpl: webcrypto,
    fetchImpl: async () => {
      const data = payloads[fetchIndex++];
      return streamedResponse(data);
    },
  });
}

async function prepareFresh(configure = false) {
  const { value, payloads } = request();
  value.installMode = "erase-all";
  value.eraseConfirmed = true;
  value.provisioning = configure
    ? {
        action: "configure",
        offset: 0xd000,
        size: 0x1000,
        ssid: "fresh-network",
        password: "fresh-password",
      }
    : null;
  let fetchIndex = 0;
  await prepare(value, () => {}, {
    loadEsptool: false,
    cryptoImpl: webcrypto,
    fetchImpl: async () => {
      const data = payloads[fetchIndex++];
      return streamedResponse(data);
    },
  });
}

function environment() {
  return { isSecureContext: true, addEventListener() {}, removeEventListener() {} };
}

test("fresh blank install erases exactly once after target validation and before every write", async () => {
  await prepareFresh();
  assert.equal(testing.prepared().files.length, 3);
  const timeline = [];
  class FakeTransport {
    setDeviceLostCallback() {}
    async disconnect() {}
  }
  class FakeLoader {
    chip = { CHIP_NAME: "ESP32-S3" };

    async main() {
      timeline.push("chip");
      return "ESP32-S3 (QFN56) (revision v0.2)";
    }
    async readFlashId() {
      timeline.push("capacity");
      return FLASH_ID_8_MB;
    }
    async eraseFlash() { timeline.push("erase"); }
    async writeFlash({ fileArray }) {
      timeline.push(`write:${fileArray[0].address}`);
    }
    async writeReg() {}
  }
  const events = [];
  await flash((event) => {
    events.push(event);
    timeline.push(`event:${event.phase}`);
  }, {
    environment: environment(),
    serial: { requestPort: async () => ({}) },
    TransportImpl: FakeTransport,
    LoaderImpl: FakeLoader,
    proveReset: async (_serial, _port, reset) => reset(),
  });

  assert.equal(timeline.filter((entry) => entry === "erase").length, 1);
  assert.ok(timeline.indexOf("capacity") < timeline.indexOf("event:erasing"));
  assert.ok(timeline.indexOf("event:erasing") < timeline.indexOf("erase"));
  assert.ok(timeline.indexOf("erase") < timeline.indexOf("write:0"));
  assert.deepEqual(
    timeline.filter((entry) => entry.startsWith("write:")),
    ["write:0", "write:32768", "write:65536"],
  );
  assert.equal(events.at(-1).phase, "success");
});

test("fresh explicit configuration adds only the new provisioning image", async () => {
  await prepareFresh(true);
  assert.deepEqual(
    testing.prepared().files.map(({ kind, offset }) => [kind, offset]),
    [
      ["bootloader", 0],
      ["partition-table", 0x8000],
      ["application", 0x10000],
      ["provisioning", 0xd000],
    ],
  );
});

test("fresh erase failure performs no writes and requires a complete fresh-install retry", async () => {
  await prepareFresh();
  let erases = 0;
  let writes = 0;
  class FakeTransport {
    setDeviceLostCallback() {}
    async disconnect() {}
  }
  class FakeLoader {
    chip = { CHIP_NAME: "ESP32-S3" };

    async main() { return "ESP32-S3 (QFN56) (revision v0.2)"; }
    async readFlashId() { return FLASH_ID_8_MB; }
    async eraseFlash() {
      erases += 1;
      throw new Error("erase command stopped");
    }
    async writeFlash() { writes += 1; }
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
  assert.deepEqual({ erases, writes }, { erases: 1, writes: 0 });
  assert.deepEqual(
    terminalEvents(events).map(({ phase, code }) => ({ phase, code })),
    [{ phase: "failed", code: "erase_failure" }],
  );
  assert.match(events.at(-1).message, /device may be blank/i);
  assert.match(events.at(-1).message, /complete fresh-install plan/i);
});

test("fresh target failures and missing confirmation cannot reach full-chip erase", async () => {
  for (const scenario of [
    { chip: "ESP32-C6", flashId: FLASH_ID_8_MB, code: "wrong_chip" },
    { chip: "ESP32-S3", flashId: FLASH_ID_4_MB, code: "wrong_flash_size" },
  ]) {
    testing.reset();
    await prepareFresh();
    let erases = 0;
    class FakeTransport {
      setDeviceLostCallback() {}
      async disconnect() {}
    }
    class FakeLoader {
      chip = { CHIP_NAME: scenario.chip };

      async main() { return scenario.chip; }
      async readFlashId() { return scenario.flashId; }
      async eraseFlash() { erases += 1; }
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
    assert.equal(erases, 0);
    assert.equal(events.at(-1).code, scenario.code);
  }

  const unconfirmed = request();
  unconfirmed.value.installMode = "erase-all";
  unconfirmed.value.provisioning = null;
  let fetches = 0;
  await assert.rejects(
    prepare(unconfirmed.value, () => {}, {
      loadEsptool: false,
      cryptoImpl: webcrypto,
      fetchImpl: async () => {
        fetches += 1;
        return streamedResponse(unconfirmed.payloads[0]);
      },
    }),
    /target identity is incomplete/,
  );
  assert.equal(fetches, 0);
  assert.equal(testing.prepared(), null);
});

test("C6 completion verifies identity and flash before sending its USB-JTAG reset signal", async () => {
  const { value, payloads } = request();
  value.boardSlug = "xiao-esp32-c6";
  value.displayName = "Seeed XIAO ESP32-C6";
  value.expectedChip = "esp32c6";
  value.flashSize = 4 * 1024 * 1024;
  value.afterReset = "hard-reset";
  let fetchIndex = 0;
  await prepare(value, () => {}, {
    loadEsptool: false,
    cryptoImpl: webcrypto,
    fetchImpl: async () => streamedResponse(payloads[fetchIndex++]),
  });
  let registerBaseAtIdentityRead;
  const resetSignals = [];
  class FakeTransport {
    setDeviceLostCallback() {}
    async disconnect() {}
    async setDTR(state) { resetSignals.push(["dtr", state]); }
    async setRTS(state) { resetSignals.push(["rts", state]); }
  }
  class FakeLoader {
    chip = { CHIP_NAME: "ESP32-C6", SPI_REG_BASE: 0x60002000 };

    constructor({ transport }) { this.transport = transport; }
    async main() { return "ESP32-C6 (revision 2)"; }
    async readFlashId() {
      registerBaseAtIdentityRead = this.chip.SPI_REG_BASE;
      return FLASH_ID_4_MB;
    }
    async writeFlash() {}
    async after() {}
  }
  const events = [];
  await flash((event) => events.push(event), {
    environment: environment(),
    serial: { requestPort: async () => ({}) },
    TransportImpl: FakeTransport,
    LoaderImpl: FakeLoader,
    proveReset: async () => assert.fail("C6 completion must not require browser USB lifecycle evidence"),
    resetSleep: async (milliseconds) => resetSignals.push(["sleep", milliseconds]),
  });
  assert.equal(registerBaseAtIdentityRead, 0x60003000);
  assert.deepEqual(resetSignals, [
    ["dtr", false],
    ["sleep", 100],
    ["rts", true],
    ["dtr", false],
    ["rts", true],
    ["sleep", 100],
    ["rts", false],
  ]);
  assert.equal(events.at(-1).phase, "success");
  assert.match(events.at(-1).message, /reset signal was sent/i);
  assert.doesNotMatch(events.at(-1).message, /disconnected and re-enumerated/i);
});

test("fresh erasure locks cancellation and device loss requires complete reinstall", async () => {
  await prepareFresh();
  let writes = 0;
  class FakeTransport {
    setDeviceLostCallback(callback) { this.lost = callback; }
    async disconnect() {}
  }
  class FakeLoader {
    chip = { CHIP_NAME: "ESP32-S3" };

    constructor({ transport }) {
      this.transport = transport;
    }
    async main() { return "ESP32-S3 (QFN56) (revision v0.2)"; }
    async readFlashId() { return FLASH_ID_8_MB; }
    async eraseFlash() {
      cancel();
      clearPrepared();
    }
    async writeFlash() {
      writes += 1;
      if (writes === 1) {
        this.transport.lost();
        throw new Error("device disconnected");
      }
    }
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
  assert.equal(events.at(-1).code, "device_lost");
  assert.match(events.at(-1).message, /device may be blank/i);
  assert.match(events.at(-1).message, /complete fresh-install plan/i);
  assert.equal(events.some(({ phase }) => phase === "cancelled"), false);
});

test("typed ESP failures emit once, clean up, and never reset after an incomplete plan", async () => {
  const cases = [
    {
      name: "generic device-picker rejection",
      code: "connection_failure",
      portError: new Error("device picker unavailable"),
      expectedDisconnects: 0,
      expectedWrites: 0,
    },
    {
      name: "bootloader connection rejection",
      code: "connection_failure",
      mainError: new Error("bootloader did not answer"),
      expectedDisconnects: 1,
      expectedWrites: 0,
    },
    {
      name: "wrong flash capacity",
      code: "wrong_flash_size",
      flashId: FLASH_ID_4_MB,
      expectedDisconnects: 1,
      expectedWrites: 0,
    },
    {
      name: "unknown JEDEC flash capacity",
      code: "connection_failure",
      flashId: 0x9940ef,
      expectedDisconnects: 1,
      expectedWrites: 0,
    },
    {
      name: "generic part write rejection",
      code: "write_failure",
      writeError: new Error("serial write stopped"),
      expectedDisconnects: 1,
      expectedWrites: 1,
    },
  ];

  for (const scenario of cases) {
    testing.reset();
    await prepareDefault();
    const configurationBytes = testing.prepared().files.at(-1).bytes;
    const lifecycle = {
      disconnects: 0,
      guardAdds: 0,
      guardRemoves: 0,
      resets: 0,
      writes: 0,
    };
    class FakeTransport {
      setDeviceLostCallback() {}
      async disconnect() { lifecycle.disconnects += 1; }
    }
    class FakeLoader {
      chip = { CHIP_NAME: "ESP32-S3" };

      async main() {
        if (scenario.mainError) throw scenario.mainError;
        return "ESP32-S3 (QFN56) (revision v0.2)";
      }
      async readFlashId() { return scenario.flashId ?? FLASH_ID_8_MB; }
      async writeFlash() {
        lifecycle.writes += 1;
        if (scenario.writeError) throw scenario.writeError;
      }
      async after() { lifecycle.resets += 1; }
    }
    const events = [];
    await assert.rejects(
      flash((event) => events.push(event), {
        environment: {
          isSecureContext: true,
          addEventListener(name) {
            assert.equal(name, "beforeunload");
            lifecycle.guardAdds += 1;
          },
          removeEventListener(name) {
            assert.equal(name, "beforeunload");
            lifecycle.guardRemoves += 1;
          },
        },
        serial: {
          requestPort: async () => {
            if (scenario.portError) throw scenario.portError;
            return {};
          },
        },
        TransportImpl: FakeTransport,
        LoaderImpl: FakeLoader,
      }),
      scenario.name,
    );

    assert.deepEqual(
      terminalEvents(events).map(({ phase, code }) => ({ phase, code })),
      [{ phase: "failed", code: scenario.code }],
      scenario.name,
    );
    assert.deepEqual(
      lifecycle,
      {
        disconnects: scenario.expectedDisconnects,
        guardAdds: 1,
        guardRemoves: 1,
        resets: 0,
        writes: scenario.expectedWrites,
      },
      scenario.name,
    );
    assert.equal(testing.prepared(), null, scenario.name);
    assert.equal(configurationBytes.every((byte) => byte === 0), true, scenario.name);
    if (scenario.name === "unknown JEDEC flash capacity") {
      assert.match(events.at(-1).message, /unknown JEDEC flash-capacity identifier/i);
      assert.match(events.at(-1).message, /0x009940ef/i);
      assert.match(events.at(-1).message, /BOOT\/RESET preparation steps/i);
    }
  }
});

test("permission cancellation is distinct and happens before transport creation", async () => {
  await prepareDefault();
  const preparedPlan = testing.prepared();
  const configurationBytes = testing.prepared().files.at(-1).bytes;
  const events = [];
  let guardAdds = 0;
  let guardRemoves = 0;
  const denied = Object.assign(new Error("cancelled"), { name: "NotFoundError" });
  await assert.rejects(
    flash((event) => events.push(event), {
      environment: {
        isSecureContext: true,
        addEventListener() { guardAdds += 1; },
        removeEventListener() { guardRemoves += 1; },
      },
      serial: { requestPort: async () => { throw denied; } },
      TransportImpl: class { constructor() { assert.fail("transport must not be created"); } },
      LoaderImpl: class {},
    }),
  );
  assert.deepEqual(
    terminalEvents(events).map(({ phase, code }) => ({ phase, code })),
    [{ phase: "failed", code: "permission_denied" }],
  );
  assert.deepEqual({ guardAdds, guardRemoves }, { guardAdds: 1, guardRemoves: 1 });
  assert.equal(testing.prepared(), preparedPlan);
  assert.equal(configurationBytes.every((byte) => byte === 0), false);

  class FakeTransport {
    setDeviceLostCallback() {}
    async disconnect() {}
  }
  class FakeLoader {
    chip = { CHIP_NAME: "ESP32-S3" };

    async main() { return "ESP32-S3 (QFN56) (revision v0.2)"; }
    async readFlashId() { return FLASH_ID_8_MB; }
    async writeFlash() {}
    async after() {}
  }
  const retryEvents = [];
  await flash((event) => retryEvents.push(event), {
    environment: environment(),
    serial: { requestPort: async () => ({}) },
    TransportImpl: FakeTransport,
    LoaderImpl: FakeLoader,
    proveReset: async () => {},
  });
  assert.equal(retryEvents.at(-1).phase, "success");
  assert.equal(testing.prepared(), null);
  assert.equal(configurationBytes.every((byte) => byte === 0), true);
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
  assert.match(unsupportedEvents.at(-1).message, /Firefox/);
  assert.doesNotMatch(unsupportedEvents.at(-1).message, /Chrome\/Edge/);
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
  const configurationBytes = testing.prepared().files.at(-1).bytes;
  let disconnected = false;
  class FakeTransport {
    setDeviceLostCallback() {}
    async disconnect() { disconnected = true; }
  }
  class FakeLoader {
    chip = { CHIP_NAME: "ESP32-S3" };

    async main() { return "ESP32-S3 (QFN56) (revision v0.2)"; }
    async readFlashId() { return FLASH_ID_8_MB; }
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
  assert.deepEqual(
    terminalEvents(events).map(({ phase, code }) => ({ phase, code })),
    [{ phase: "failed", code: "verification_failure" }],
  );
  assert.equal(disconnected, true);
  assert.equal(testing.prepared(), null);
  assert.equal(configurationBytes.every((byte) => byte === 0), true);
});

test("device loss takes precedence over a generic write failure", async () => {
  await prepareDefault();
  const configurationBytes = testing.prepared().files.at(-1).bytes;
  let disconnects = 0;
  let lost;
  class FakeTransport {
    setDeviceLostCallback(callback) { lost = callback; }
    async disconnect() { disconnects += 1; }
  }
  class FakeLoader {
    chip = { CHIP_NAME: "ESP32-S3" };

    async main() { return "ESP32-S3 (QFN56) (revision v0.2)"; }
    async readFlashId() { return FLASH_ID_8_MB; }
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
  assert.deepEqual(
    terminalEvents(events).map(({ phase, code }) => ({ phase, code })),
    [{ phase: "failed", code: "device_lost" }],
  );
  assert.equal(disconnects, 1);
  assert.equal(testing.prepared(), null);
  assert.equal(configurationBytes.every((byte) => byte === 0), true);
});

test("reset failure is reported only after writes verify", async () => {
  await prepareDefault();
  const configurationBytes = testing.prepared().files.at(-1).bytes;
  let disconnects = 0;
  let writes = 0;
  let resets = 0;
  class FakeTransport {
    setDeviceLostCallback() {}
    async disconnect() { disconnects += 1; }
  }
  class FakeLoader {
    chip = { CHIP_NAME: "ESP32-S3" };

    async main() { return "ESP32-S3 (QFN56) (revision v0.2)"; }
    async readFlashId() { return FLASH_ID_8_MB; }
    async writeFlash() { writes += 1; }
    async writeReg() { resets += 1; throw new Error("reset unavailable"); }
  }
  const events = [];
  await assert.rejects(
    flash((event) => events.push(event), {
      environment: environment(),
      serial: { requestPort: async () => ({}) },
      TransportImpl: FakeTransport,
      LoaderImpl: FakeLoader,
      proveReset: async (_serial, _port, reset) => reset(),
    }),
  );
  assert.deepEqual(
    terminalEvents(events).map(({ phase, code }) => ({ phase, code })),
    [{ phase: "failed", code: "reset_failure" }],
  );
  assert.deepEqual({ disconnects, resets, writes }, { disconnects: 1, resets: 1, writes: 4 });
  assert.equal(testing.prepared(), null);
  assert.equal(configurationBytes.every((byte) => byte === 0), true);
});

test("reset enumeration timeout after a verified fresh install does not claim the device is blank", async () => {
  await prepareFresh();
  let erases = 0;
  let writes = 0;
  class FakeTransport {
    setDeviceLostCallback() {}
    async disconnect() {}
  }
  class FakeLoader {
    chip = { CHIP_NAME: "ESP32-S3" };

    async main() { return "ESP32-S3 (QFN56) (revision v0.2)"; }
    async readFlashId() { return FLASH_ID_8_MB; }
    async eraseFlash() { erases += 1; }
    async writeFlash() { writes += 1; }
    async writeReg() {}
  }
  const events = [];
  await assert.rejects(
    flash((event) => events.push(event), {
      environment: environment(),
      serial: { requestPort: async () => ({}) },
      TransportImpl: FakeTransport,
      LoaderImpl: FakeLoader,
      proveReset: async (_serial, _port, reset) => {
        await reset();
        throw new Error("USB enumeration timeout");
      },
    }),
  );
  assert.deepEqual({ erases, writes }, { erases: 1, writes: 3 });
  assert.deepEqual(
    terminalEvents(events).map(({ phase, code }) => ({ phase, code })),
    [{ phase: "failed", code: "reset_failure" }],
  );
  assert.match(events.at(-1).message, /firmware bytes are verified/i);
  assert.match(events.at(-1).message, /press reset/i);
  assert.doesNotMatch(events.at(-1).message, /device may be blank/i);
  assert.doesNotMatch(events.at(-1).message, /complete fresh-install plan/i);
  assert.equal(events.some(({ phase }) => phase === "success"), false);
});

test("USB reset proof requires selected-port disconnect before matching re-enumeration", async () => {
  const listeners = new Map();
  const selectedPort = {
    getInfo: () => ({ usbVendorId: 0x303a, usbProductId: 0x1001 }),
  };
  const reenumeratedPort = {
    getInfo: () => ({ usbVendorId: 0x303a, usbProductId: 0x1001 }),
  };
  const serial = {
    addEventListener(name, listener) { listeners.set(name, listener); },
    removeEventListener(name, listener) {
      assert.equal(listeners.get(name), listener);
      listeners.delete(name);
    },
  };
  await testing.proveUsbReset(serial, selectedPort, async () => {
    listeners.get("connect")({ target: reenumeratedPort });
    listeners.get("disconnect")({ target: selectedPort });
    listeners.get("connect")({ target: reenumeratedPort });
  });
  assert.equal(listeners.size, 0);
});

test("cancellation stops at the next verified part boundary", async () => {
  await prepareDefault();
  const configurationBytes = testing.prepared().files.at(-1).bytes;
  let disconnects = 0;
  let resets = 0;
  let writes = 0;
  class FakeTransport {
    setDeviceLostCallback() {}
    async disconnect() { disconnects += 1; }
  }
  class FakeLoader {
    chip = { CHIP_NAME: "ESP32-S3" };

    async main() { return "ESP32-S3 (QFN56) (revision v0.2)"; }
    async readFlashId() { return FLASH_ID_8_MB; }
    async writeFlash() { writes += 1; cancel(); }
    async after() { resets += 1; }
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
  assert.deepEqual(
    terminalEvents(events).map(({ phase, code }) => ({ phase, code })),
    [{ phase: "cancelled", code: "cancelled" }],
  );
  assert.deepEqual({ disconnects, resets }, { disconnects: 1, resets: 0 });
  assert.equal(testing.prepared(), null);
  assert.equal(configurationBytes.every((byte) => byte === 0), true);
});

test("clearing a prepared plan during an active write cancels before the next part", async () => {
  await prepareDefault();
  const configurationBytes = testing.prepared().files.at(-1).bytes;
  let disconnects = 0;
  let resets = 0;
  let writes = 0;
  class FakeTransport {
    setDeviceLostCallback() {}
    async disconnect() { disconnects += 1; }
  }
  class FakeLoader {
    chip = { CHIP_NAME: "ESP32-S3" };

    async main() { return "ESP32-S3 (QFN56) (revision v0.2)"; }
    async readFlashId() { return FLASH_ID_8_MB; }
    async writeFlash() { writes += 1; clearPrepared(); }
    async after() { resets += 1; }
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
  assert.deepEqual(
    terminalEvents(events).map(({ phase, code }) => ({ phase, code })),
    [{ phase: "cancelled", code: "cancelled" }],
  );
  assert.deepEqual({ disconnects, resets }, { disconnects: 1, resets: 0 });
  assert.equal(testing.prepared(), null);
  assert.equal(configurationBytes.every((byte) => byte === 0), true);
});

test("retry after a partial write requires re-preparation and restarts the complete plan", async () => {
  await prepareDefault();
  const firstAttemptAddresses = [];
  let firstAttemptResets = 0;
  class FirstTransport {
    setDeviceLostCallback() {}
    async disconnect() {}
  }
  class FailingLoader {
    chip = { CHIP_NAME: "ESP32-S3" };

    async main() { return "ESP32-S3 (QFN56) (revision v0.2)"; }
    async readFlashId() { return FLASH_ID_8_MB; }
    async writeFlash({ fileArray }) {
      firstAttemptAddresses.push(fileArray[0].address);
      if (firstAttemptAddresses.length === 2) {
        throw new Error("injected serial write failure");
      }
    }
    async after() { firstAttemptResets += 1; }
  }
  const firstEvents = [];
  await assert.rejects(
    flash((event) => firstEvents.push(event), {
      environment: environment(),
      serial: { requestPort: async () => ({}) },
      TransportImpl: FirstTransport,
      LoaderImpl: FailingLoader,
    }),
  );
  assert.deepEqual(firstAttemptAddresses, [0, 0x8000]);
  assert.equal(firstAttemptResets, 0);
  assert.deepEqual(
    terminalEvents(firstEvents).map(({ phase, code }) => ({ phase, code })),
    [{ phase: "failed", code: "write_failure" }],
  );

  let unpreparedPortRequests = 0;
  const unpreparedEvents = [];
  await assert.rejects(
    flash((event) => unpreparedEvents.push(event), {
      environment: environment(),
      serial: { requestPort: async () => { unpreparedPortRequests += 1; return {}; } },
      TransportImpl: FirstTransport,
      LoaderImpl: FailingLoader,
    }),
  );
  assert.equal(unpreparedPortRequests, 0);
  assert.deepEqual(
    terminalEvents(unpreparedEvents).map(({ phase, code }) => ({ phase, code })),
    [{ phase: "failed", code: "not_prepared" }],
  );

  await prepareDefault();
  const retryAddresses = [];
  let retryDisconnects = 0;
  let retryResets = 0;
  class RetryTransport {
    setDeviceLostCallback() {}
    async disconnect() { retryDisconnects += 1; }
  }
  class RetryLoader {
    chip = { CHIP_NAME: "ESP32-S3" };

    async main() { return "ESP32-S3 (QFN56) (revision v0.2)"; }
    async readFlashId() { return FLASH_ID_8_MB; }
    async writeFlash({ fileArray }) { retryAddresses.push(fileArray[0].address); }
    async writeReg(address) {
      if (address === 0x60008098) retryResets += 1;
    }
  }
  const retryEvents = [];
  await flash((event) => retryEvents.push(event), {
    environment: environment(),
    serial: { requestPort: async () => ({}) },
    TransportImpl: RetryTransport,
    LoaderImpl: RetryLoader,
    proveReset: async (_serial, _port, reset) => reset(),
  });
  assert.deepEqual(retryAddresses, [0, 0x8000, 0x10000, 0xd000]);
  assert.deepEqual({ retryDisconnects, retryResets }, { retryDisconnects: 1, retryResets: 1 });
  assert.deepEqual(
    terminalEvents(retryEvents).map(({ phase, code }) => ({ phase, code })),
    [{ phase: "success", code: undefined }],
  );
});

test("UF2 completion reports delivery guidance without claiming device verification", async () => {
  const payload = uf2Bytes(0x27000, 0xada52840);
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
    mountLabel: "TECHOBOOT",
    serialFilters: [],
    uf2Compatibility: {
      softdeviceFamily: "s140",
      softdeviceVersion: "7.3.0",
      fwid: 0x0123,
      applicationBase: 0x27000,
      familyId: 0xada52840,
    },
    nrfSerialDfu: null,
    provisioning: null,
    parts: [{
      kind: "uf2",
      path: "firmware/hopspot/t-echo/0.2.6/t-echo.uf2",
      url: "/releases/0.2.6/firmware/hopspot/t-echo/0.2.6/t-echo.uf2",
      offset: null,
      size: payload.length,
      sha256: sha(payload),
    }],
  };
  await prepare(value, () => {}, {
    cryptoImpl: webcrypto,
    fetchImpl: async () => streamedResponse(payload),
  });
  let clicked = false;
  const events = [];
  const result = await flash((event) => events.push(event), {
    BlobImpl: class {},
    urlApi: { createObjectURL: () => "blob:test", revokeObjectURL() {} },
    documentImpl: {
      createElement: () => ({
        click() { clicked = true; },
      }),
    },
  });
  assert.equal(clicked, true);
  assert.deepEqual(result, { downloadRequested: true });
  assert.equal(events.at(-1).phase, "download_requested");
  assert.notEqual(events.at(-1).phase, "success");
  assert.match(events.at(-1).message, /download requested/i);
  assert.doesNotMatch(events.at(-1).message, /UF2 downloaded/i);
  assert.match(events.at(-1).message, /copy it to TECHOBOOT/i);
  assert.doesNotMatch(events.at(-1).message, /device-side verification/i);
});
