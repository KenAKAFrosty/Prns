import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { requestEspControllerEnrollment, frameUsbAuto, espEnrollmentCleanup } from "../src/esp-controller-enrollment.js";
import { enrollController, continueControllerEnrollment, stopControllerEnrollmentCheck } from "../src/prns-flash.js";

const vectors = JSON.parse(readFileSync(new URL("../../../../prns-core/src/interfaces/usb_auto/enrollment-vectors.json", import.meta.url)));
const bytes = (hex) => Uint8Array.from(Buffer.from(hex, "hex"));
const vector = (name) => bytes(vectors.vectors.find((entry) => entry.name === name).payload_hex);
const request = { transport: "esp-serial", boardSlug: "e290", controllerPublicKey: vectors.controller_public_key_hex, action: "authorize" };
const check = { ...request, action: "check-status" };
const word = (data, at) => new DataView(data.buffer, data.byteOffset, data.byteLength).getUint32(at, true);
function unframe(framed) {
  const output = [];
  let escaped = false;
  for (const byte of framed.subarray(1, -1)) {
    if (byte === 0x7d && !escaped) { escaped = true; continue; }
    output.push(escaped ? byte ^ 0x20 : byte);
    escaped = false;
  }
  return Uint8Array.from(output);
}
function receipt(nonce, transaction, status = 2, target = vectors.target_public_key_hex) {
  const output = new Uint8Array(74);
  output[0] = 6;
  new DataView(output.buffer).setUint32(1, nonce, true);
  output[5] = status;
  new DataView(output.buffer).setUint32(6, transaction, true);
  output.set(bytes(target), 10);
  return output;
}
function fixture(options = {}) {
  const calls = [];
  const writes = [];
  const enrollmentSession = { operation: null };
  let input;
  let transaction = 0;
  let state = 0;
  let random = 0;
  const environment = { isSecureContext: true, crypto: { getRandomValues(data) { new DataView(data.buffer).setUint32(0, 123 + random++, true); } } };
  const port = {
    readable: null, writable: null,
    async open(settings) {
      assert.deepEqual(settings, { baudRate: 115200, dataBits: 8, stopBits: 1, parity: "none", flowControl: "none", bufferSize: 2048 });
      calls.push("open");
      await options.open?.();
      this.readable = new ReadableStream({ start(controller) { input = controller; }, cancel() { calls.push("cancel"); } });
      this.writable = new WritableStream({ async write(framed) {
        const payload = unframe(framed);
        writes.push(payload);
        if (await options.write?.(payload, f)) return;
        if (payload[0] === 1) f.push(vector("capability_ack"));
        if (payload[0] === 4) {
          assert.equal(payload.length, 133);
          assert.equal(Buffer.from(payload.subarray(5, 69)).toString("hex"), request.controllerPublicKey);
          assert.equal(Buffer.from(payload.subarray(69)).toString("hex"), vectors.target_public_key_hex);
          transaction = word(payload, 1); state = 1;
          f.push(receipt(0, transaction, state));
        }
        if (payload[0] === 5) {
          assert.equal(payload.length, 5); assert.notEqual(word(payload, 1), 0);
          if (state === 1) state = 2;
          f.push(receipt(word(payload, 1), transaction, state));
        }
      } });
    },
    async close() {
      assert.equal(this.readable?.locked, false); assert.equal(this.writable?.locked, false);
      calls.push("close"); await options.close?.(); this.readable = this.writable = null;
    },
    setSignals() { assert.fail("enrollment must not reset or enter a bootloader"); },
  };
  const serial = { async requestPort(filters) { assert.deepEqual(filters, { filters: [] }); calls.push("picker"); return port; } };
  const dependencies = { environment, serial, enrollmentSession, responseTimeoutMs: 30, handshakeTimeoutMs: 50, setupTimeoutMs: 60, sleep: async () => {} };
  const f = {
    calls, writes, port, dependencies, enrollmentSession,
    push(payload) {
      const framed = frameUsbAuto(payload);
      if (options.fragment) for (const byte of framed) input.enqueue(Uint8Array.of(byte));
      else input.enqueue(framed);
    },
    raw(chunk) { input.enqueue(chunk); },
    run: (value = request) => requestEspControllerEnrollment(serial, value, environment, dependencies),
    grants: () => writes.filter((payload) => payload[0] === 4),
  };
  return f;
}

test("USB-Auto framing matches shared Rust golden vectors including escaped full public keys", () => {
  for (const entry of vectors.vectors) assert.equal(Buffer.from(frameUsbAuto(bytes(entry.payload_hex))).toString("hex"), entry.framed_hex, entry.name);
});

test("fragmented USB-Auto capability, target pin and matching durable receipt succeed without reset", async () => {
  const f = fixture({ fragment: true });
  assert.deepEqual(await f.run(), { status: "saved", targetPublicKey: vectors.target_public_key_hex });
  assert.equal(f.grants().length, 1);
  assert.deepEqual(f.calls, ["picker", "open", "cancel", "close"]);
  const queries = f.writes.filter((payload) => payload[0] === 5).map((payload) => word(payload, 1));
  assert.equal(new Set(queries).size, queries.length);
});

test("invalid requests and a check with no retained operation never open serial", async () => {
  const f = fixture();
  await assert.rejects(f.run({ ...request, action: undefined }), /valid controller/);
  await assert.rejects(f.run({ ...request, controllerPublicKey: "bad" }), /valid controller/);
  assert.equal((await f.run(check)).status, "unknown");
  assert.deepEqual(f.calls, []);
});

test("incompatible protocol or missing enrollment capability cannot authorize", async () => {
  for (const broken of ["version", "capability"]) {
    const f = fixture({ write(payload, f) {
      if (payload[0] !== 1) return;
      const ack = vector("capability_ack"); ack[broken === "version" ? 5 : 6] = 0;
      f.push(ack); return true;
    } });
    assert.equal((await f.run()).status, "error");
    assert.equal(f.grants().length, 0);
    assert.equal(f.calls.at(-1), "close");
  }
});

test("startup noise and large unrelated frames are skipped before the firmware handshake", async () => {
  let booted = false;
  const f = fixture({ write(payload, f) {
    if (payload[0] !== 1 || booted) return;
    booted = true;
    f.raw(new TextEncoder().encode("booting firmware\n"));
    f.push(new Uint8Array(1000).fill(9));
  } });
  assert.equal((await f.run()).status, "saved");
});

test("only a fresh query nonce can supply the target or confirm a saved receipt", async () => {
  const f = fixture({ write(payload, f) {
    if (payload[0] !== 5) return;
    f.push(receipt(0, 999, 2, "ee".repeat(64)));
    f.push(receipt(word(payload, 1) - 1, 999, 2, "ee".repeat(64)));
  } });
  assert.equal((await f.run()).status, "saved");
  assert.equal(f.grants().length, 1);
});

test("stale authorization acknowledgement is ignored until the matching transaction", async () => {
  const f = fixture({ write(payload, f) { if (payload[0] === 4) f.push(receipt(0, word(payload, 1) - 1, 2)); } });
  assert.equal((await f.run()).status, "saved");
});

test("busy or malformed preflight cannot receive authorization", async () => {
  for (const malformed of [false, true]) {
    const f = fixture({ write(payload, f) {
      if (payload[0] !== 5) return;
      f.push(malformed ? Uint8Array.of(6, 0) : receipt(word(payload, 1), 7, 1)); return true;
    } });
    assert.equal((await f.run()).status, "error");
    assert.equal(f.grants().length, 0);
  }
});

test("rejection and target identity change cannot report a saved grant", async () => {
  for (const targetChange of [false, true]) {
    const f = fixture({ write(payload, f) {
      if (payload[0] !== 4) return;
      f.push(receipt(0, word(payload, 1), targetChange ? 2 : 3, targetChange ? "ee".repeat(64) : vectors.target_public_key_hex)); return true;
    } });
    assert.equal((await f.run()).status, targetChange ? "unknown" : "error");
    assert.equal(f.grants().length, 1);
  }
});

test("lost authorization acknowledgement stays unknown and reconnect checks without another grant", async () => {
  let authorized = false;
  const f = fixture({ write(payload, f) {
    if (payload[0] === 4) { authorized = true; return true; }
    if (payload[0] === 5 && authorized) { f.push(receipt(word(payload, 1), f.enrollmentSession.operation.transaction)); return true; }
  } });
  assert.equal((await f.run()).status, "unknown");
  assert.equal((await f.run(check)).status, "saved");
  assert.equal(f.grants().length, 1);
});

test("a failed or ambiguous authorization write is never retried by check status", async () => {
  let authorized = false;
  const f = fixture({ write(payload, f) {
    if (payload[0] === 4) { authorized = true; throw new Error("connection lost after write"); }
    if (payload[0] === 5 && authorized) { f.push(receipt(word(payload, 1), f.enrollmentSession.operation.transaction)); return true; }
  } });
  assert.equal((await f.run()).status, "unknown");
  assert.equal((await f.run(check)).status, "saved");
  assert.equal(f.grants().length, 1);
});

test("reconnect pins the full original target and cleared receipts do not reenroll", async () => {
  for (const targetChange of [false, true]) {
    let reconnect = false;
    const f = fixture({ write(payload, f) {
      if (reconnect && payload[0] === 5) {
        f.push(receipt(word(payload, 1), targetChange ? f.enrollmentSession.operation.transaction : 0, targetChange ? 2 : 0, targetChange ? "ee".repeat(64) : vectors.target_public_key_hex)); return true;
      }
    } });
    assert.equal((await f.run()).status, "saved"); reconnect = true;
    assert.equal((await f.run(check)).status, "unknown");
    assert.equal(f.grants().length, 1);
  }
});

test("explicit second authorization gets a fresh transaction even with a retained saved receipt", async () => {
  const f = fixture();
  assert.equal((await f.run()).status, "saved");
  assert.equal((await f.run()).status, "saved");
  assert.equal(f.grants().length, 2);
  assert.notEqual(word(f.grants()[0], 1), word(f.grants()[1], 1));
});

test("oversized serial chunks are bounded and never authorize", async () => {
  const f = fixture({ write(payload, f) { if (payload[0] === 1) { f.raw(new Uint8Array(4097)); return true; } } });
  assert.equal((await f.run()).status, "error");
  assert.equal(f.grants().length, 0);
});

test("timed-out open retains ownership until the eventual port is closed", async () => {
  let release;
  const f = fixture({ open: () => new Promise((resolve) => { release = resolve; }) });
  assert.equal((await enrollController(request, f.dependencies)).status, "unknown");
  assert.ok(espEnrollmentCleanup());
  assert.match((await enrollController(request, f.dependencies)).message, /already active/);
  const cleanup = espEnrollmentCleanup(); release(); await cleanup; await Promise.resolve();
  assert.equal(espEnrollmentCleanup(), null);
  assert.equal(f.calls.at(-1), "close");
  assert.equal(f.grants().length, 0);
});

test("bridge holds the device lock across delayed cleanup and releases it only after close", async () => {
  let release;
  let delayed = true;
  const f = fixture({ close: () => delayed ? new Promise((resolve) => { release = resolve; }) : undefined });
  assert.equal((await enrollController(request, f.dependencies)).status, "unknown");
  assert.match((await enrollController(request, f.dependencies)).message, /already active/);
  const cleanup = espEnrollmentCleanup(); delayed = false; release(); await cleanup; await Promise.resolve();
  assert.equal((await enrollController(check, f.dependencies)).status, "saved");
  assert.equal(f.grants().length, 1);
});

test("a timed-out write keeps ownership and returns unknown until it settles, without replay", async () => {
  let release;
  let written = false;
  const f = fixture({ write(payload, f) {
    if (payload[0] === 4) {
      written = true;
      return new Promise((resolve) => { release = () => resolve(true); });
    }
    if (payload[0] === 5 && written) {
      f.push(receipt(word(payload, 1), f.enrollmentSession.operation.transaction)); return true;
    }
  } });
  assert.equal((await enrollController(request, f.dependencies)).status, "unknown");
  assert.match((await enrollController(check, f.dependencies)).message, /already active/);
  const cleanup = espEnrollmentCleanup(); release(); await cleanup; await Promise.resolve();
  assert.equal((await enrollController(check, f.dependencies)).status, "saved");
  assert.equal(f.grants().length, 1);
});

test("a silent port exits after the bounded firmware handshake without an authorization", async () => {
  const f = fixture({ write: () => true });
  const started = performance.now();
  assert.equal((await f.run()).status, "error");
  assert.ok(performance.now() - started < 1000);
  assert.equal(f.grants().length, 0);
  assert.equal(f.calls.at(-1), "close");
});

test("a rejected authorization cannot be revived by checking an older saved transaction", async () => {
  let rejected = false;
  const f = fixture({ write(payload, f) {
    if (payload[0] === 4) { rejected = true; f.push(receipt(0, word(payload, 1), 3)); return true; }
    if (payload[0] === 5 && rejected) { assert.fail("definitively rejected attempts must not be queried again"); }
  } });
  assert.equal((await f.run()).status, "error");
  const calls = f.calls.length;
  assert.equal((await f.run(check)).status, "unknown");
  assert.equal(f.calls.length, calls);
  assert.equal(f.grants().length, 1);
});

test("Idle receipts must have the canonical zero transaction", async () => {
  const f = fixture({ write(payload, f) {
    if (payload[0] === 5) { f.push(receipt(word(payload, 1), 7, 0)); return true; }
  } });
  assert.equal((await f.run()).status, "error");
  assert.equal(f.grants().length, 0);
});

test("a 30-second startup remains visibly pending on one connection until durable Saved", async () => {
  let elapsed = 0;
  const progress = [];
  const f = fixture({ write(payload, f) {
    if (payload[0] === 5 && f.enrollmentSession.operation) {
      f.push(receipt(word(payload, 1), f.enrollmentSession.operation.transaction, elapsed < 30_000 ? 1 : 2));
      return true;
    }
  } });
  Object.assign(f.dependencies, {
    now: () => elapsed, sleep: async ms => { elapsed += ms; },
    setupTimeoutMs: 120_000, onProgress: event => progress.push(event),
  });
  assert.equal((await enrollController(request, f.dependencies)).status, "saved");
  assert.equal(elapsed, 30_000);
  assert.ok(progress.length > 1);
  assert.ok(progress.every(event => event.status === "waiting" && !event.paused));
  assert.deepEqual(f.calls, ["picker", "open", "cancel", "close"]);
  assert.equal(f.grants().length, 1);
});

test("expired windows hold the port without polling; Continue retains ownership and the transaction", async () => {
  let elapsed = 0, ready = false;
  let firstPause, secondPause;
  const paused = [new Promise(resolve => { firstPause = resolve; }), new Promise(resolve => { secondPause = resolve; })];
  const releases = [firstPause, secondPause];
  let pauseCount = 0;
  const f = fixture({ write(payload, f) {
    if (payload[0] === 5 && f.enrollmentSession.operation) {
      f.push(receipt(word(payload, 1), f.enrollmentSession.operation.transaction, ready ? 2 : 1));
      return true;
    }
  } });
  Object.assign(f.dependencies, {
    now: () => elapsed, sleep: async ms => { elapsed += ms; }, setupTimeoutMs: 120_000,
    onProgress(event) { if (event.paused) releases[pauseCount++](); },
  });
  const running = enrollController(request, f.dependencies);
  await paused[0];
  const queryCount = f.writes.length;
  await new Promise(resolve => setTimeout(resolve, 10));
  assert.equal(f.writes.length, queryCount);
  assert.deepEqual(f.calls, ["picker", "open"]);
  assert.match((await enrollController(check, f.dependencies)).message, /already active/);
  assert.equal(continueControllerEnrollment(), true);
  await paused[1];
  assert.equal(f.grants().length, 1);
  ready = true;
  assert.equal(continueControllerEnrollment(), true);
  assert.equal((await running).status, "saved");
  // Over 64KiB of legitimate status replies must not exhaust the per-exchange
  // noise budget during explicitly continued checks.
  assert.ok(f.writes.filter(payload => payload[0] === 5).length > 900);
  assert.deepEqual(f.calls, ["picker", "open", "cancel", "close"]);
  assert.equal(continueControllerEnrollment(), false);
});

test("Stop checking releases a paused connection and later checks never authorize again", async () => {
  let elapsed = 0, ready = false;
  const f = fixture({ write(payload, f) {
    if (payload[0] === 5 && f.enrollmentSession.operation) {
      f.push(receipt(word(payload, 1), f.enrollmentSession.operation.transaction, ready ? 2 : 1));
      return true;
    }
  } });
  Object.assign(f.dependencies, {
    now: () => elapsed, sleep: async ms => { elapsed += ms; }, setupTimeoutMs: 500,
    onProgress(event) { if (event.paused) assert.equal(stopControllerEnrollmentCheck(), true); },
  });
  assert.equal((await enrollController(request, f.dependencies)).status, "pending");
  assert.equal(f.calls.at(-1), "close");
  assert.equal(stopControllerEnrollmentCheck(), false);
  ready = true;
  assert.equal((await enrollController(check, f.dependencies)).status, "saved");
  assert.equal(f.grants().length, 1);
});

test("a target change after a paused wait remains Unknown with no second authorization", async () => {
  let elapsed = 0, changed = false;
  const f = fixture({ write(payload, f) {
    if (payload[0] === 5 && f.enrollmentSession.operation) {
      f.push(receipt(word(payload, 1), f.enrollmentSession.operation.transaction, changed ? 2 : 1,
        changed ? "ee".repeat(64) : vectors.target_public_key_hex));
      return true;
    }
  } });
  Object.assign(f.dependencies, {
    now: () => elapsed, sleep: async ms => { elapsed += ms; }, setupTimeoutMs: 500,
    onProgress(event) { if (event.paused) { changed = true; continueControllerEnrollment(); } },
  });
  assert.equal((await enrollController(request, f.dependencies)).status, "unknown");
  assert.equal(f.grants().length, 1);
});

test("leaving during a delayed port open latches Stop and prevents authorization", async () => {
  let opened, release;
  const opening = new Promise(resolve => { opened = resolve; });
  const f = fixture({ open() { opened(); return new Promise(resolve => { release = resolve; }); } });
  const running = enrollController(request, f.dependencies);
  await opening;
  assert.equal(stopControllerEnrollmentCheck(), true);
  release();
  assert.equal((await running).status, "error");
  assert.equal(f.grants().length, 0);
  assert.equal(f.calls.at(-1), "close");
});

test("leaving during the firmware handshake prevents the later authorization", async () => {
  let probing, release;
  const probe = new Promise(resolve => { probing = resolve; });
  let first = true;
  const f = fixture({ async write(payload) {
    if (payload[0] === 1 && first) {
      first = false;
      probing();
      await new Promise(resolve => { release = resolve; });
    }
  } });
  const running = enrollController(request, f.dependencies);
  await probe;
  assert.equal(stopControllerEnrollmentCheck(), true);
  release();
  assert.equal((await running).status, "error");
  assert.equal(f.grants().length, 0);
  assert.equal(f.calls.at(-1), "close");
});

test("lost transport while paused becomes Unknown when resumed without authorizing", async () => {
  let elapsed = 0, lost = false;
  const f = fixture({ write(payload, f) {
    if (payload[0] === 5 && f.enrollmentSession.operation) {
      if (lost) throw Error("disconnected during pause");
      f.push(receipt(word(payload, 1), f.enrollmentSession.operation.transaction, 1));
      return true;
    }
  } });
  Object.assign(f.dependencies, {
    now: () => elapsed, sleep: async ms => { elapsed += ms; }, setupTimeoutMs: 500,
    onProgress(event) { if (event.paused) { lost = true; continueControllerEnrollment(); } },
  });
  assert.equal((await enrollController(request, f.dependencies)).status, "unknown");
  assert.equal(f.grants().length, 1);
});
