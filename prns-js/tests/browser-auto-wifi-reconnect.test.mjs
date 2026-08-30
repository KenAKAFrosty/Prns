import assert from "node:assert/strict";
import { test } from "node:test";

import {
  AutoWifiController,
  Tag,
} from "personal-rns/browser";
import { RecoverySchedule } from "../dist/browser/auto_wifi/recovery.js";

const GATEWAY_ID = new Uint8Array([
  0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88,
  0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, 0x10,
]);
const HELLO_MAGIC = new Uint8Array([
  0x50, 0x52, 0x4e, 0x53, 0x57, 0x53, 0x00, 0x00,
]);

test("recovery waits for cooldown and advances bounded backoff", () => {
  const schedule = new RecoverySchedule();
  schedule.begin("gateway", 1_000);

  assert.equal(schedule.nextDueAt(), 6_000);
  assert.equal(schedule.ready("gateway", 5_999), false);
  assert.equal(schedule.ready("gateway", 6_000), true);

  const [first] = schedule.due(6_000);
  assert.ok(first);
  schedule.retry(first, 6_000);
  assert.equal(schedule.nextDueAt(), 11_000);

  const [second] = schedule.due(11_000);
  assert.ok(second);
  schedule.retry(second, 11_000);
  assert.equal(schedule.nextDueAt(), 16_000);

  const [third] = schedule.due(16_000);
  assert.ok(third);
  schedule.retry(third, 16_000);
  assert.equal(schedule.nextDueAt(), 24_000);
});

test("a stale recovery attempt cannot advance a new connection lifecycle", () => {
  const schedule = new RecoverySchedule();
  schedule.begin("gateway", 1_000);
  const [stale] = schedule.due(6_000);
  assert.ok(stale);
  schedule.begin("gateway", 6_000);
  schedule.retry(stale, 6_000);

  assert.equal(schedule.nextDueAt(), 11_000);
});

test(
  "auto wifi re-registers a dropped gateway and resumes inbound traffic",
  { timeout: 10_000 },
  async () => {
    const restoreWebSocket = replaceGlobal("WebSocket", FakeWebSocket);
    const restoreStorage = replaceGlobal("localStorage", new MemoryStorage());
    const restoreFetch = replaceGlobal("fetch", async () => {
      throw new TypeError("catalog unavailable");
    });
    const host = new AutoWifiHost();
    const controller = new AutoWifiController(host);
    const statuses = [];
    const releaseStatus = controller.subscribeStatus((status) => {
      statuses.push(status.tag);
    });

    try {
      await waitUntil(() => host.registrations.length === 1, 2_000);
      await waitUntil(() => controller.status.tag === "Active", 1_000);
      assert.ok(statuses.includes("Active"));
      assert.deepEqual(host.registrationBitrates, [1_000_000_000]);
      const first = FakeWebSocket.connected()[0];
      assert.ok(first);
      await waitUntil(() => host.outboundTakes === 1, 1_000);
      await new Promise((resolve) => setTimeout(resolve, 75));
      assert.equal(host.outboundTakes, 1);

      host.queueOutbound(host.interfaceIds[0], {
        bytes: new Uint8Array([0x31, 0x32, 0x33]),
      });
      await waitUntil(() => first.outbound.length === 1, 1_000);
      assert.deepEqual(first.outbound, [[0x31, 0x32, 0x33]]);

      const disconnectedAt = Date.now();
      const reconnectTransitions = statuses.length;
      first.disconnect();
      await waitUntil(() => host.deactivations.length === 1, 1_000);
      await waitUntil(
        () => statuses.slice(reconnectTransitions).includes("Discovering"),
        1_000,
      );
      await new Promise((resolve) => setTimeout(resolve, 100));
      assert.equal(FakeWebSocket.localAttempts(), 1);
      await waitUntil(() => host.registrations.length === 2, 7_000);

      assert.ok(Date.now() - disconnectedAt >= 4_500);
      assert.equal(host.deactivations.length, 1);
      assert.equal(
        bytesHex(host.registrations[0]),
        bytesHex(host.registrations[1]),
      );

      const second = FakeWebSocket.connected()[0];
      assert.ok(second);
      await waitUntil(
        () => statuses.slice(reconnectTransitions).includes("Active"),
        1_000,
      );
      second.receive(new Uint8Array([0x7e, 0x01, 0x7d]));
      await waitUntil(() => host.inbound.length === 1, 1_000);
      assert.deepEqual(host.inbound, [[0x7e, 0x01, 0x7d]]);
      const firstClose = controller.close();
      const secondClose = controller.close();
      assert.equal(firstClose, secondClose);
      assert.equal((await firstClose).tag, "Closed");
    } finally {
      releaseStatus();
      await controller.close();
      restoreFetch();
      restoreStorage();
      restoreWebSocket();
      FakeWebSocket.reset();
    }
  },
);

class AutoWifiHost {
  registrations = [];
  registrationBitrates = [];
  deactivations = [];
  inbound = [];
  interfaceIds = [];
  outboundTakes = 0;
  #outbound = new Map();
  #outboundWaiters = new Map();

  autoWifiReady() {
    return Tag("Ready");
  }

  autoWifiRegister(id, bitrate) {
    this.registrations.push(new Uint8Array(id));
    this.registrationBitrates.push(bitrate);
    const interfaceId = new Uint8Array([
      this.registrations.length,
      0,
      0,
      0,
      0,
      0,
      0,
      0,
    ]);
    this.interfaceIds.push(interfaceId);
    return Tag("Registered", interfaceId);
  }

  autoWifiDeactivate(id) {
    this.deactivations.push(new Uint8Array(id));
    const key = bytesHex(id);
    this.#outboundWaiters.get(key)?.(Tag("InterfaceDetached"));
    this.#outboundWaiters.delete(key);
    return Tag("Detached");
  }

  autoWifiIngest(_id, bytes) {
    this.inbound.push([...bytes]);
    return Tag("Accepted");
  }

  nextOutboundFor(id) {
    this.outboundTakes += 1;
    const key = bytesHex(id);
    const outbound = this.#outbound.get(key) ?? [];
    if (outbound.length > 0) {
      this.#outbound.delete(key);
      return Promise.resolve(Tag("Outbound", outbound));
    }
    return new Promise((resolve) => {
      this.#outboundWaiters.set(key, resolve);
    });
  }

  queueOutbound(id, frame) {
    const key = bytesHex(id);
    const waiter = this.#outboundWaiters.get(key);
    if (waiter !== undefined) {
      this.#outboundWaiters.delete(key);
      waiter(Tag("Outbound", [frame]));
      return;
    }
    const outbound = this.#outbound.get(key) ?? [];
    outbound.push(frame);
    this.#outbound.set(key, outbound);
  }

  autoWifiBitrateBps() {
    return 500_000_000;
  }

  autoWifiHardwareMtu() {
    return 16_384;
  }

  autoWifiFrameCap() {
    return 16_384;
  }
}

class FakeWebSocket {
  static #instances = [];

  static connected() {
    return this.#instances.filter(
      (socket) =>
        socket.url.startsWith("ws://localhost:") && socket.readyState === 1,
    );
  }

  static localAttempts() {
    return this.#instances.filter((socket) =>
      socket.url.startsWith("ws://localhost:"),
    ).length;
  }

  static reset() {
    this.#instances = [];
  }

  #listeners = new Map();
  bufferedAmount = 0;
  binaryType = "blob";
  protocol = "";
  readyState = 0;
  outbound = [];

  constructor(url, protocol) {
    this.url = url;
    this.requestedProtocol = protocol;
    FakeWebSocket.#instances.push(this);
    queueMicrotask(() => {
      if (url.startsWith("ws://localhost:")) {
        this.readyState = 1;
        this.protocol = protocol;
        this.#emit("open", {});
      } else {
        this.readyState = 3;
        this.#emit("error", {});
        this.#emit("close", {});
      }
    });
  }

  addEventListener(type, listener) {
    const listeners = this.#listeners.get(type) ?? new Set();
    listeners.add(listener);
    this.#listeners.set(type, listeners);
  }

  removeEventListener(type, listener) {
    this.#listeners.get(type)?.delete(listener);
  }

  send(bytes) {
    if (bytes.byteLength !== 10) {
      this.outbound.push([...bytes]);
      return;
    }
    const hello = new Uint8Array(26);
    hello.set(HELLO_MAGIC);
    hello[9] = 1;
    hello.set(GATEWAY_ID, 10);
    queueMicrotask(() => this.#emit("message", { data: hello.buffer }));
  }

  close() {
    if (this.readyState === 3) {
      return;
    }
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

class MemoryStorage {
  #values = new Map();

  getItem(key) {
    return this.#values.get(key) ?? null;
  }

  setItem(key, value) {
    this.#values.set(key, value);
  }
}

function replaceGlobal(name, value) {
  const previous = Object.getOwnPropertyDescriptor(globalThis, name);
  Object.defineProperty(globalThis, name, {
    configurable: true,
    writable: true,
    value,
  });
  return () => {
    if (previous) {
      Object.defineProperty(globalThis, name, previous);
    } else {
      delete globalThis[name];
    }
  };
}

async function waitUntil(ready, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  while (!ready()) {
    assert.ok(Date.now() < deadline, "timed out waiting for browser recovery");
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
}

function bytesHex(bytes) {
  return [...bytes].map((byte) => byte.toString(16).padStart(2, "0")).join("");
}
