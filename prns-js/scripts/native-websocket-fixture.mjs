import { createServer } from "node:net";

import {
  Prns,
  Tag,
} from "personal-rns/native";

const LOOPBACK_HOST = "127.0.0.1";

export async function startNativeWebSocketFixture(options = {}) {
  const host = options.host ?? LOOPBACK_HOST;
  const port = options.port ?? await reservePort(host);
  const destination = Tag("Single", {
    name: {
      appName: "prns-browser-lab",
      aspects: ["native-companion"],
    },
    identity: Tag("HostIdentity"),
    announceAppData: new Uint8Array([0x50, 0x72, 0x6e, 0x73]),
    requestHandlers: [],
  });
  const created = await Prns.create({
    identity: Tag("GenerateEphemeral"),
    role: "Endpoint",
    destinations: [destination],
  });
  if (created.tag !== "Ready") {
    throw new Error(`native Prns fixture failed to start: ${created.tag}`);
  }
  const node = created.data;
  const drains = [
    drainClaim(node.claimEvents(), "application events"),
    drainClaim(node.claimDiagnostics(), "diagnostics"),
  ];
  try {
    const attached = await node.attachInterface(Tag("WebSocketServer", {
      bind: `${host}:${port}`,
      framing: "Auto",
    }));
    if (
      attached.tag !== "Succeeded" ||
      attached.data.tag !== "InterfaceAttached"
    ) {
      throw new Error(
        `native Prns WebSocket fixture failed to attach: ${attached.tag}:${attached.data.tag}`,
      );
    }
  } catch (error) {
    await node.stop();
    await Promise.all(drains);
    throw error;
  }
  const destinationHash = node.destinationHashes[0];
  if (destinationHash === undefined) {
    await node.stop();
    await Promise.all(drains);
    throw new Error("native Prns fixture did not register its destination");
  }
  let stopped = false;
  return {
    destinationHash,
    destinationHex: Buffer.from(destinationHash).toString("hex"),
    node,
    webSocketUrl: `ws://${host}:${port}`,
    async stop() {
      if (stopped) {
        return;
      }
      stopped = true;
      await node.stop();
      await Promise.all(drains);
    },
  };
}

async function reservePort(host) {
  const server = createServer();
  await new Promise((resolveListen, rejectListen) => {
    server.once("error", rejectListen);
    server.listen(0, host, resolveListen);
  });
  const address = server.address();
  if (address === null || typeof address === "string") {
    server.close();
    throw new Error("native Prns fixture could not reserve a loopback port");
  }
  await new Promise((resolveClose, rejectClose) => {
    server.close((error) => error ? rejectClose(error) : resolveClose());
  });
  return address.port;
}

function drainClaim(claim, name) {
  if (claim.tag !== "Claimed") {
    throw new Error(`native Prns fixture could not claim ${name}: ${claim.tag}`);
  }
  return drain(claim.data);
}

async function drain(values) {
  for await (const _value of values) {
  }
}
