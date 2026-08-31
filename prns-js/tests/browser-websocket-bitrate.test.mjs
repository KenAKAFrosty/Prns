import assert from "node:assert/strict";
import { test } from "node:test";

import {
  Tag,
  WebSocketInterface,
  bitrateBps,
} from "personal-rns/browser";

test("browser WebSockets raise only inferred loopback bitrate", async () => {
  const previous = globalThis.WebSocket;
  globalThis.WebSocket = FakeWebSocket;
  const host = new WebSocketHost();
  const webSocket = new WebSocketInterface(host);
  try {
    await connectAndClose(webSocket, "ws://192.168.1.22:4284/prns");
    await connectAndClose(webSocket, "ws://localhost:4284/prns");
    await connectAndClose(webSocket, "ws://drop.localhost:4284/prns");
    await connectAndClose(webSocket, "ws://127.0.0.2:4284/prns");
    await connectAndClose(webSocket, "ws://[::1]:4284/prns");
    await connectAndClose(webSocket, "ws://localhost:4284/explicit", {
      bitrateBps: bitrateBps(750_000_000),
    });

    assert.deepEqual(
      host.registrations.map(({ bitrateBps: bitrate }) => bitrate),
      [
        500_000_000,
        1_000_000_000,
        1_000_000_000,
        1_000_000_000,
        1_000_000_000,
        750_000_000,
      ],
    );
  } finally {
    globalThis.WebSocket = previous;
  }
});

async function connectAndClose(webSocket, url, options) {
  const connected = await webSocket.connect(url, options);
  assert.equal(connected.tag, "Connected");
  assert.equal((await connected.data.close()).tag, "Closed");
}

class WebSocketHost {
  registrations = [];
  #nextId = 0;
  #waiters = new Map();

  runtimeReadiness() {
    return Tag("Ready");
  }

  webSocketRegister(options) {
    this.registrations.push(options);
    this.#nextId += 1;
    return Tag("Registered", new Uint8Array([this.#nextId]));
  }

  deactivateInterface(id) {
    const waiter = this.#waiters.get(id[0]);
    this.#waiters.delete(id[0]);
    waiter?.(Tag("InterfaceDetached"));
    return Tag("Detached");
  }

  webSocketIngest() {
    return Tag("Accepted");
  }

  nextOutboundFor(id) {
    return new Promise((resolve) => {
      this.#waiters.set(id[0], resolve);
    });
  }

  createWebSocketFramingCodec() {
    return {
      messageCap: () => 572,
      canReadOutbound: () => true,
      canStageMultipleOutbound: () => true,
      rawFallbackIsArmed: () => false,
      isDetecting: () => false,
      rawFallbackDelayMillis: () => 0,
      decode: () => ({ packets: [] }),
      stageOutbound: () => undefined,
      releaseRawFallback: () => undefined,
    };
  }

  websocketBitrateBps() {
    return 500_000_000;
  }

  websocketHardwareMtu() {
    return 508;
  }

  websocketFrameCap() {
    return 572;
  }
}

class FakeWebSocket {
  static OPEN = 1;
  #listeners = new Map();
  binaryType = "blob";
  bufferedAmount = 0;
  readyState = 0;

  constructor(url) {
    this.url = url;
    queueMicrotask(() => {
      this.readyState = FakeWebSocket.OPEN;
      this.#emit("open", {});
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

  close() {
    this.readyState = 3;
  }

  send() {}

  #emit(type, event) {
    for (const listener of this.#listeners.get(type) ?? []) {
      listener(event);
    }
  }
}
