import path from "node:path";
import { pathToFileURL } from "node:url";


const repository = process.argv[2];
const packetHex = "0000000102030405060708090a0b0c0d0e0f00c0db7e7d42";
const hdlcHex = "7e0000000102030405060708090a0b0c0d0e0f00c0db7d5e7d5d427e";


class FakeWebSocket {
  constructor() {
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
}


const websocketUrl = pathToFileURL(
  path.join(repository, "src/interfaces/WebSocketInterface.js"),
);
const { WebSocketClientInterface } = await import(websocketUrl);
const packetBytes = Uint8Array.from(Buffer.from(packetHex, "hex"));
const hdlcBytes = Uint8Array.from(Buffer.from(hdlcHex, "hex"));
const interface_ = new WebSocketClientInterface("oracle", "ws://127.0.0.1");
const socket = new FakeWebSocket();
interface_._ws = socket;
interface_.online = true;
interface_._setupHandlers();
const inbound = [];
interface_.on("packet", (packet) => {
  inbound.push(Buffer.from(packet).toString("hex"));
});
const silentUntilOutbound = socket.sent.length === 0;
interface_.send(packetBytes);
socket.emit("message", { data: hdlcBytes.slice(0, 9).buffer });
const coalesced = new Uint8Array(hdlcBytes.length - 9 + hdlcBytes.length);
coalesced.set(hdlcBytes.slice(9));
coalesced.set(hdlcBytes, hdlcBytes.length - 9);
socket.emit("message", { data: coalesced.buffer });

console.log(
  JSON.stringify({
    kind: "runtime",
    hdlc: {
      inbound,
      outbound: Buffer.from(socket.sent[0]).toString("hex"),
      silent_until_outbound: silentUntilOutbound,
    },
  }),
);
