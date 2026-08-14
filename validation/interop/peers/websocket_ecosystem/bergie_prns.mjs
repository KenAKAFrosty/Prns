import path from "node:path";
import { pathToFileURL } from "node:url";


const repository = process.argv[2];
const target = process.argv[3];
const framing = process.argv[4];
const websocketUrl = pathToFileURL(
  path.join(repository, "packages/core/src/interfaces/websocket.js"),
);
const packetUrl = pathToFileURL(
  path.join(repository, "packages/core/src/core/packet.js"),
);
const { WebSocketClientInterface } = await import(websocketUrl);
const { Packet } = await import(packetUrl);


function packet(address, payload) {
  return new Uint8Array([
    0,
    0,
    ...new Uint8Array(16).fill(address),
    0,
    ...new TextEncoder().encode(payload),
  ]);
}


function equalBytes(left, right) {
  return left.length === right.length &&
    left.every((byte, index) => byte === right[index]);
}


function waitFor(predicate, description) {
  return new Promise((resolve, reject) => {
    const deadline = Date.now() + 5000;
    const poll = () => {
      if (predicate()) {
        resolve();
        return;
      }
      if (Date.now() >= deadline) {
        reject(new Error(`timed out waiting for ${description}`));
        return;
      }
      setTimeout(poll, 10);
    };
    poll();
  });
}


function firstWireMessage(socket) {
  return new Promise((resolve, reject) => {
    const timeout = setTimeout(
      () => reject(new Error("timed out waiting for provisional raw")),
      5000,
    );
    socket.addEventListener("message", (event) => {
      clearTimeout(timeout);
      resolve(new Uint8Array(event.data));
    }, { once: true });
  });
}


const firstPacket = packet(0x31, "provisional-raw");
const evidencePacket = packet(0x32, "bergie-evidence");
const returnedPacket = packet(0x33, "resolved-egress");
const received = [];
let client;

try {
  client = new WebSocketClientInterface({
    url: target,
    framing,
    autoReconnect: false,
  });
  client.addEventListener("packet", (event) => {
    received.push(event.detail.packet.serialize());
  });
  await client.connect();
  const provisional = await firstWireMessage(client.socket);
  if (!equalBytes(provisional, firstPacket)) {
    throw new Error("Prns provisional output was not the expected raw packet");
  }
  if (framing === "raw") {
    await waitFor(() => received.length === 1, "Bergie raw ingress");
    if (!equalBytes(received[0], firstPacket)) {
      throw new Error("Bergie did not decode provisional raw ingress");
    }
  } else {
    await new Promise((resolve) => setImmediate(resolve));
    await new Promise((resolve) => setImmediate(resolve));
    if (received.length !== 0) {
      throw new Error("Bergie KISS accepted provisional raw ingress");
    }
  }
  await client.send(Packet.deserialize(evidencePacket));
  const expectedReceived = framing === "raw" ? 2 : 1;
  await waitFor(
    () => received.length === expectedReceived,
    "Prns resolved return traffic",
  );
  if (!equalBytes(received[expectedReceived - 1], returnedPacket)) {
    throw new Error("Bergie did not receive resolved Prns egress");
  }
  console.log(`PASS: bergie ${framing} interoperated with Prns auto`);
} catch (error) {
  console.error(error instanceof Error ? error.message : error);
  process.exitCode = 1;
} finally {
  await client?.disconnect();
}
