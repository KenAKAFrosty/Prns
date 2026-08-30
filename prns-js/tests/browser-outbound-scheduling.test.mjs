import assert from "node:assert/strict";
import { test } from "node:test";

import { Tag } from "personal-rns/browser";
import { RuntimeHost } from "../dist/browser/runtime.js";
import { BrowserUsbAutoSession } from "../dist/browser/usb_auto/session.js";
import { BrowserWebSocketSession } from "../dist/browser/websocket/session.js";

test("Automatic USB starts its outbound consumer after confirmation and then sleeps", async () => {
  const host = new UsbSessionHost();
  const transport = new FakeUsbTransport();
  const session = new BrowserUsbAutoSession(
    host,
    transport,
    new Uint8Array([1, 0, 0, 0, 0, 0, 0, 0]),
  );
  session.start();

  await waitUntil(() => host.outboundTakes === 1);
  await new Promise((resolve) => setTimeout(resolve, 75));
  assert.equal(host.outboundTakes, 1);

  host.queueOutbound({ bytes: new Uint8Array([0x31, 0x32]) });
  await waitUntil(() => transport.writes.some((bytes) => bytes[0] === 0xd0));
  assert.deepEqual(
    transport.writes.find((bytes) => bytes[0] === 0xd0),
    [0xd0, 0x31, 0x32],
  );
  assert.equal(host.outboundTakes, 2);
  assert.equal((await session.close()).tag, "Closed");
});

test("WebSocket outbound sleeps while idle and wakes for a runtime batch", async () => {
  const host = new SessionOutboundHost();
  const socket = new FakeSocket();
  const session = new BrowserWebSocketSession(
    host,
    socket,
    new Uint8Array([1, 0, 0, 0, 0, 0, 0, 0]),
    "ws://localhost:42721/prns",
    16_384,
    "RawPacket",
    new FixedCodec(),
    () => undefined,
  );
  session.start();

  await Promise.resolve();
  assert.equal(host.outboundTakes, 1);
  await new Promise((resolve) => setTimeout(resolve, 75));
  assert.equal(host.outboundTakes, 1);

  host.queueOutbound({ bytes: new Uint8Array([0x41, 0x42]) });
  await waitUntil(() => socket.outbound.length === 1);
  assert.deepEqual(socket.outbound, [[0x41, 0x42]]);
  assert.equal(host.outboundTakes, 2);
  assert.equal((await session.close()).tag, "Closed");
});

test("WebSocket session status pushes a remote close exactly once", async () => {
  const host = new SessionOutboundHost();
  const socket = new FakeSocket();
  const session = new BrowserWebSocketSession(
    host,
    socket,
    new Uint8Array([1, 0, 0, 0, 0, 0, 0, 0]),
    "ws://localhost:42721/prns",
    16_384,
    "RawPacket",
    new FixedCodec(),
    () => undefined,
  );
  const statuses = [];
  const release = session.subscribeStatus((status) => {
    statuses.push(status.tag);
  });
  session.start();

  socket.disconnect();

  await waitUntil(() => session.status.tag === "Closed");
  assert.deepEqual(statuses, ["Closed"]);
  release();
  assert.equal((await session.close()).tag, "Closed");
  assert.deepEqual(statuses, ["Closed"]);
});

test("WebSocket framing evidence wakes outbound before the fallback deadline", async () => {
  const host = new SessionOutboundHost([
    { bytes: new Uint8Array([0x51, 0x52]) },
  ]);
  const socket = new FakeSocket();
  const codec = new DetectingCodec();
  const session = new BrowserWebSocketSession(
    host,
    socket,
    new Uint8Array([1, 0, 0, 0, 0, 0, 0, 0]),
    "ws://localhost:42721/prns",
    16_384,
    "Auto",
    codec,
    () => undefined,
  );
  session.start();
  await waitUntil(() => codec.rawFallbackIsArmed());

  socket.receive(new Uint8Array([0x61]));

  await waitUntil(() => host.outboundTakes === 2);
  assert.deepEqual(socket.outbound, [[0x51, 0x52]]);
  assert.equal(codec.fallbackReleases, 0);
  assert.equal((await session.close()).tag, "Closed");
});

test("runtime outbound waits without polling and wakes with a non-empty batch", async () => {
  const runtime = new FakeRuntime();
  const host = runtimeHost(runtime);
  const id = register(host, 1);
  let settled = false;

  const outbound = host.nextOutboundFor(id).then((outcome) => {
    settled = true;
    return outcome;
  });
  await Promise.resolve();

  assert.equal(runtime.outboundDrains, 1);
  assert.equal(settled, false);

  runtime.queue(frameFor(id, [0x11, 0x12]));
  host.notifyRuntimeActivity();

  const outcome = await outbound;
  assert.equal(outcome.tag, "Outbound");
  assert.deepEqual([...outcome.data[0].bytes], [0x11, 0x12]);
  assert.equal(runtime.outboundDrains, 2);
});

test("runtime outbound detachment settles an idle consumer", async () => {
  const runtime = new FakeRuntime();
  const host = runtimeHost(runtime);
  const id = register(host, 1);
  const outbound = host.nextOutboundFor(id);

  assert.equal(host.deactivateInterface(id).tag, "Detached");
  assert.deepEqual(await outbound, Tag("InterfaceDetached"));
});

test("one interface draining another interface's frame wakes its consumer", async () => {
  const runtime = new FakeRuntime();
  const host = runtimeHost(runtime);
  const first = register(host, 1);
  const second = register(host, 2);
  const secondOutbound = host.nextOutboundFor(second);

  runtime.queue(frameFor(second, [0x21]));
  const firstOutbound = host.nextOutboundFor(first);

  const outcome = await secondOutbound;
  assert.equal(outcome.tag, "Outbound");
  assert.deepEqual([...outcome.data[0].bytes], [0x21]);

  assert.equal(host.deactivateInterface(first).tag, "Detached");
  assert.deepEqual(await firstOutbound, Tag("InterfaceDetached"));
});

test("bounded outbound reads preserve queued frames without another wake", async () => {
  const runtime = new FakeRuntime();
  const host = runtimeHost(runtime);
  const id = register(host, 1);
  runtime.queue(frameFor(id, [0x31]), frameFor(id, [0x32]));

  const first = await host.nextOutboundFor(id, 1);
  const second = await host.nextOutboundFor(id, 1);

  assert.equal(first.tag, "Outbound");
  assert.equal(second.tag, "Outbound");
  assert.deepEqual([...first.data[0].bytes], [0x31]);
  assert.deepEqual([...second.data[0].bytes], [0x32]);
});

test("an invalid outbound batch limit is rejected instead of waiting forever", async () => {
  const runtime = new FakeRuntime();
  const host = runtimeHost(runtime);
  const id = register(host, 1);

  const outcome = await host.nextOutboundFor(id, 0);

  assert.equal(outcome.tag, "RuntimeRejected");
  assert.equal(outcome.data.operation, "drain-outbound");
});

class FakeRuntime {
  outboundDrains = 0;
  #active = new Set();
  #nextInterface = 1;
  #outbound = [];

  snapshot() {
    return {};
  }

  registerInterface() {
    const id = new Uint8Array([this.#nextInterface, 0, 0, 0, 0, 0, 0, 0]);
    this.#nextInterface += 1;
    this.#active.add(bytesHex(id));
    return id;
  }

  removeInterface({ interfaceId }) {
    return this.#active.delete(bytesHex(interfaceId));
  }

  drainOutbound() {
    this.outboundDrains += 1;
    const outbound = this.#outbound;
    this.#outbound = [];
    return outbound;
  }

  queue(...frames) {
    this.#outbound.push(...frames);
  }
}

class SessionOutboundHost {
  outboundTakes = 0;
  #outbound;
  #waiter;

  constructor(outbound = []) {
    this.#outbound = outbound;
  }

  nextOutboundFor() {
    this.outboundTakes += 1;
    if (this.#outbound.length > 0) {
      const outbound = this.#outbound;
      this.#outbound = [];
      return Promise.resolve(Tag("Outbound", outbound));
    }
    return new Promise((resolve) => {
      this.#waiter = resolve;
    });
  }

  deactivateInterface() {
    this.#waiter?.(Tag("InterfaceDetached"));
    this.#waiter = undefined;
    return Tag("Detached");
  }

  webSocketIngest() {
    return Tag("Accepted");
  }

  queueOutbound(frame) {
    if (this.#waiter !== undefined) {
      const waiter = this.#waiter;
      this.#waiter = undefined;
      waiter(Tag("Outbound", [frame]));
      return;
    }
    this.#outbound.push(frame);
  }
}

class UsbSessionHost extends SessionOutboundHost {
  createUsbAutoDecoder() {
    return {
      feed: () => [{ type: "helloAck", tag: new Uint8Array([0x91]) }],
    };
  }

  usbAutoNodeTagFor() {
    return new Uint8Array([0x91]);
  }

  usbAutoHostHelloFrame() {
    return new Uint8Array([0xc0]);
  }

  usbAutoHostHelloAckFrame() {
    return new Uint8Array([0xc1]);
  }

  usbAutoDataFrame(packet) {
    return new Uint8Array([0xd0, ...packet]);
  }
}

class FakeUsbTransport {
  writes = [];
  #reads = [Tag("Read", new Uint8Array([0xa1]))];
  #readWaiter;

  read() {
    const next = this.#reads.shift();
    if (next !== undefined) {
      return Promise.resolve(next);
    }
    return new Promise((resolve) => {
      this.#readWaiter = resolve;
    });
  }

  write(bytes) {
    this.writes.push([...bytes]);
    return Promise.resolve(Tag("Written"));
  }

  close() {
    this.#readWaiter?.(Tag("Read", undefined));
    this.#readWaiter = undefined;
    return Promise.resolve([]);
  }
}

class FixedCodec {
  messageCap() {
    return 16_384;
  }

  canReadOutbound() {
    return true;
  }

  canStageMultipleOutbound() {
    return true;
  }

  rawFallbackIsArmed() {
    return false;
  }

  rawFallbackDelayMillis() {
    return 1_000;
  }

  decode() {
    return { packets: [] };
  }

  stageOutbound(packet) {
    return packet;
  }
}

class DetectingCodec extends FixedCodec {
  fallbackReleases = 0;
  #pending;
  #ready = true;

  canReadOutbound() {
    return this.#ready;
  }

  canStageMultipleOutbound() {
    return false;
  }

  rawFallbackIsArmed() {
    return this.#pending !== undefined;
  }

  decode() {
    const resolvedOutbound = this.#pending;
    this.#pending = undefined;
    this.#ready = true;
    return { packets: [], resolvedOutbound };
  }

  stageOutbound(packet) {
    this.#pending = packet;
    this.#ready = false;
    return undefined;
  }

  releaseRawFallback() {
    this.fallbackReleases += 1;
    const pending = this.#pending;
    this.#pending = undefined;
    this.#ready = true;
    return pending;
  }
}

class FakeSocket {
  bufferedAmount = 0;
  outbound = [];
  readyState = 1;
  #listeners = new Map();

  addEventListener(type, listener) {
    const listeners = this.#listeners.get(type) ?? new Set();
    listeners.add(listener);
    this.#listeners.set(type, listeners);
  }

  send(bytes) {
    this.outbound.push([...bytes]);
  }

  close() {
    this.readyState = 3;
    this.#emit("close", {});
  }

  disconnect() {
    this.close();
  }

  receive(bytes) {
    this.#emit("message", {
      data: bytes.buffer.slice(
        bytes.byteOffset,
        bytes.byteOffset + bytes.byteLength,
      ),
    });
  }

  #emit(type, event) {
    for (const listener of this.#listeners.get(type) ?? []) {
      listener(event);
    }
  }
}

function runtimeHost(runtime) {
  return new RuntimeHost(
    {},
    runtime,
    (length) => Tag("Filled", new Uint8Array(length)),
    () => 1_000,
    Tag("Available", new Uint8Array(16)),
    () => undefined,
  );
}

function register(host, channel) {
  const outcome = host.registerInterface({
    interfaceName: "websocket",
    kind: "websocket-client",
    channelTag: new Uint8Array([channel]),
    bitrateBps: 1_000_000,
    hardwareMtu: 16_384,
  });
  assert.equal(outcome.tag, "Registered");
  return outcome.data;
}

function frameFor(interfaceId, bytes) {
  return {
    type: "frame",
    target: { type: "interface", interfaceId },
    bytes: new Uint8Array(bytes),
  };
}

function bytesHex(bytes) {
  return [...bytes].map((byte) => byte.toString(16).padStart(2, "0")).join("");
}

async function waitUntil(predicate, timeoutMs = 1_000) {
  const deadline = Date.now() + timeoutMs;
  while (!predicate()) {
    if (Date.now() >= deadline) {
      throw new Error("timed out waiting for condition");
    }
    await new Promise((resolve) => setTimeout(resolve, 5));
  }
}
