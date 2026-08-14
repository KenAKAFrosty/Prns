import path from "node:path";
import { pathToFileURL } from "node:url";


const repository = process.argv[2];
const packetHex = "0000000102030405060708090a0b0c0d0e0f00c0db7e7d42";
const kissHex = "c0000000000102030405060708090a0b0c0d0e0f00dbdcdbdd7e7d42c0";


class FakeWebSocket {
  constructor() {
    this.binaryType = "arraybuffer";
    this.readyState = 1;
    this.sent = [];
    this.listeners = new Map();
  }

  addEventListener(name, listener) {
    const listeners = this.listeners.get(name) ?? [];
    listeners.push(listener);
    this.listeners.set(name, listeners);
  }

  emit(name, event = {}) {
    for (const listener of this.listeners.get(name) ?? []) {
      listener(event);
    }
  }

  send(bytes) {
    this.sent.push(new Uint8Array(bytes));
  }

  close() {
    this.readyState = 3;
    this.emit("close");
  }
}


globalThis.WebSocket = { OPEN: 1 };

const websocketUrl = pathToFileURL(
  path.join(repository, "packages/core/src/interfaces/websocket.js"),
);
const packetUrl = pathToFileURL(
  path.join(repository, "packages/core/src/core/packet.js"),
);
const { WebSocketClientInterface } = await import(websocketUrl);
const { Packet } = await import(packetUrl);
const packetBytes = Uint8Array.from(Buffer.from(packetHex, "hex"));
const kissBytes = Uint8Array.from(Buffer.from(kissHex, "hex"));


async function settle() {
  await new Promise((resolve) => setImmediate(resolve));
  await new Promise((resolve) => setImmediate(resolve));
}


async function characterizeRaw() {
  const socket = new FakeWebSocket();
  const interface_ = new WebSocketClientInterface({
    websocket: socket,
    framing: "raw",
  });
  interface_._setupStreams(socket);
  const inbound = [];
  interface_.addEventListener("packet", (event) => {
    inbound.push(Buffer.from(event.detail.packet.serialize()).toString("hex"));
  });
  const silentUntilOutbound = socket.sent.length === 0;
  await interface_.send(Packet.deserialize(packetBytes));
  socket.emit("message", { data: packetBytes.slice().buffer });
  await settle();
  socket.emit("close");
  return {
    inbound,
    outbound: Buffer.from(socket.sent[0]).toString("hex"),
    silent_until_outbound: silentUntilOutbound,
  };
}


async function characterizeKiss() {
  const socket = new FakeWebSocket();
  const interface_ = new WebSocketClientInterface({
    websocket: socket,
    framing: "kiss",
  });
  interface_._setupStreams(socket);
  const inbound = [];
  interface_.addEventListener("packet", (event) => {
    inbound.push(Buffer.from(event.detail.packet.serialize()).toString("hex"));
  });
  const silentUntilOutbound = socket.sent.length === 0;
  await interface_.send(Packet.deserialize(packetBytes));
  socket.emit("message", { data: kissBytes.slice(0, 11).buffer });
  const coalesced = new Uint8Array(kissBytes.length - 11 + kissBytes.length);
  coalesced.set(kissBytes.slice(11));
  coalesced.set(kissBytes, kissBytes.length - 11);
  socket.emit("message", { data: coalesced.buffer });
  await settle();
  socket.emit("close");
  return {
    inbound,
    outbound: Buffer.from(socket.sent[0]).toString("hex"),
    silent_until_outbound: silentUntilOutbound,
  };
}


console.log(
  JSON.stringify({
    kind: "runtime",
    raw: await characterizeRaw(),
    kiss: await characterizeKiss(),
  }),
);
