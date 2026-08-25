import assert from "node:assert/strict";
import {
  existsSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { createRequire } from "node:module";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { test } from "node:test";

const packageRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const napiRoot = resolve(packageRoot, "../prns-napi");
const productVersion = readFileSync(resolve(packageRoot, "../VERSION"), "utf8").trim();
if (existsSync(napiRoot) && !process.env.NAPI_RS_NATIVE_LIBRARY_PATH) {
  const bindings = readdirSync(napiRoot)
    .filter((file) => file.endsWith(".node"))
    .sort();
  assert.equal(bindings.length, 1, "exactly one local N-API binding is required");
  process.env.NAPI_RS_NATIVE_LIBRARY_PATH = resolve(napiRoot, bindings[0]);
}

const esm = await import("personal-rns");
const require = createRequire(import.meta.url);
const commonjs = require("personal-rns");

test("root export selects one native API for ESM and CommonJS", () => {
  assert.equal(esm.HOST_CONTRACT_ABI, 1);
  assert.equal(esm.PRODUCT_VERSION, productVersion);
  assert.equal(commonjs.HOST_CONTRACT_ABI, esm.HOST_CONTRACT_ABI);
  assert.equal(commonjs.Prns, esm.Prns);
});

test("packaged native API starts, exposes lifecycle, and stops", async () => {
  const created = await esm.Prns.create({
    identity: esm.Tag("GenerateEphemeral"),
    role: "Endpoint",
  });
  assert.equal(created.tag, "Ready");
  assert.equal(created.data.lifecycle.tag, "Running");
  try {
    const events = created.data.claimEvents();
    assert.equal(events.tag, "Claimed");
    assert.deepEqual(created.data.claimEvents(), {
      tag: "AlreadyClaimed",
      data: { lane: "ApplicationEvents" },
    });
    const attached = await created.data.attachTcpServer({ bind: "127.0.0.1:0" });
    assert.equal(attached.tag, "Succeeded");
    assert.equal(attached.data.tag, "InterfaceAttached");
    assert.equal(created.data.backendInfo.backend, "Native");
    assert.ok(created.data.backendInfo.interfaceKinds.includes("TcpServer"));
    const attachedSnapshot = await created.data.snapshot();
    assert.equal(attachedSnapshot.backend.backend, "Native");
    assert.ok(attachedSnapshot.backend.interfaceKinds.includes("TcpServer"));
    assert.equal(attachedSnapshot.interfaces.length, 1);
    assert.equal(attachedSnapshot.interfaces[0].kind, "TcpServer");
    const webSocketAttached = await created.data.attachInterface(
      esm.Tag("WebSocketClient", {
        target: "ws://127.0.0.1:9",
        framing: "Auto",
      }),
    );
    assert.equal(webSocketAttached.tag, "Succeeded");
    assert.equal(webSocketAttached.data.tag, "InterfaceAttached");
    const webSocketDetached = await created.data.execute(
      esm.Tag("DetachInterface", {
        interface: webSocketAttached.data.data.interface,
      }),
    );
    assert.equal(webSocketDetached.tag, "Succeeded");
    assert.equal(webSocketDetached.data.tag, "InterfaceDetached");
    const unsupported = await created.data.attachInterface(
      esm.Tag("BrowserRendezvous", { url: "wss://fixture.invalid/rendezvous" }),
      {
        mode: "Boundary",
        gravity: -73,
        recursivePathRequests: true,
        announcesFromInternal: false,
        announcesToInternal: true,
      },
    );
    assert.deepEqual(unsupported, {
      tag: "Failed",
      data: { tag: "UnsupportedByBackend", data: undefined },
    });
    const detached = await created.data.execute(
      esm.Tag("DetachInterface", {
        interface: attached.data.data.interface,
      }),
    );
    assert.equal(detached.tag, "Succeeded");
    assert.equal(detached.data.tag, "InterfaceDetached");
    assert.equal((await created.data.snapshot()).interfaces.length, 0);
    assert.equal((await created.data.stop()).tag, "Stopped");
    assert.equal(created.data.lifecycle.tag, "Stopped");
  } finally {
    await created.data.stop().catch(() => {});
  }
});

test("packaged native API completes the persistent two-node journey", async () => {
  const fixture = JSON.parse(
    readFileSync(resolve(packageRoot, "../prns-host/conformance/persistent-two-node-v1.json"), "utf8"),
  );
  assert.equal(fixture.schemaVersion, esm.HOST_SCHEMA_VERSION);
  const port = await reserveLoopbackPort();
  const root = mkdtempSync(resolve(tmpdir(), "prns-js-journey-"));
  const resourcePath = resolve(root, "resource.bin");
  const destination = esm.Tag("Single", {
    name: {
      appName: fixture.destination.appName,
      aspects: fixture.destination.aspects,
    },
    identity: esm.Tag("HostIdentity"),
    announceAppData: Buffer.from(fixture.destination.announceAppDataHex, "hex"),
    requestHandlers: [
      { path: fixture.request.path, policy: "AllowAll" },
    ],
  });
  let server;
  let client;
  let restoredServer;
  let restoredClient;
  try {
    const serverCreated = await esm.Prns.create(
      esm.persistentEndpoint(resolve(root, "server"), [destination]),
    );
    const clientCreated = await esm.Prns.create(
      esm.persistentEndpoint(resolve(root, "client")),
    );
    assert.equal(serverCreated.tag, "Ready");
    assert.equal(clientCreated.tag, "Ready");
    server = serverCreated.data;
    client = clientCreated.data;
    const serverIdentity = Buffer.from(server.identityHash);
    const clientIdentity = Buffer.from(client.identityHash);
    const destinationHash = Buffer.from(server.destinationHashes[0]);
    const eventClaim = server.claimEvents();
    assert.equal(eventClaim.tag, "Claimed");
    const events = eventClaim.data;
    const diagnosticClaim = client.claimDiagnostics();
    assert.equal(diagnosticClaim.tag, "Claimed");
    const diagnostics = diagnosticClaim.data;

    assertSucceeded(
      await server.attachInterface(
        esm.Tag("TcpServer", {
          bind: `127.0.0.1:${port}`,
          bitrate: esm.Tag("Auto"),
        }),
      ),
      "InterfaceAttached",
    );
    assertSucceeded(
      await client.attachInterface(
        esm.Tag("TcpClient", {
          target: `127.0.0.1:${port}`,
          bitrate: esm.Tag("Auto"),
        }),
      ),
      "InterfaceAttached",
    );

    let routed = false;
    for (let attempt = 0; attempt < 50 && !routed; attempt += 1) {
      routed = (await client.snapshot()).routes.some((route) =>
        Buffer.from(route.destination).equals(destinationHash),
      );
      if (!routed) {
        assertSucceeded(await server.announce(destinationHash), "Announced");
        await new Promise((resolveDelay) => setTimeout(resolveDelay, 50));
      }
    }
    assert.equal(routed, true, "announced destination did not become routable");
    const announce = await nextTagged(diagnostics, "AnnounceHeard");
    assert.deepEqual(
      Buffer.from(announce.data.appData),
      Buffer.from(fixture.destination.announceAppDataHex, "hex"),
    );

    const established = assertSucceeded(
      await client.establishLink(destinationHash),
      "LinkEstablished",
    );
    const linkPayload = Buffer.from("native direct Link payload");
    const linkDeliveryResult = client.sendLinkPacket(
      established.data.linkId,
      linkPayload,
    );
    const linkDelivery = await nextTagged(events, "LinkDelivery");
    assert.deepEqual(Buffer.from(linkDelivery.data.linkId), established.data.linkId);
    assert.deepEqual(Buffer.from(linkDelivery.data.plaintext), linkPayload);
    assert.equal(linkDelivery.data.sourceInterface.length, 8);
    assertSucceeded(await linkDeliveryResult, "PacketDelivered");

    const requestPayload = Buffer.from(fixture.request.payloadHex, "hex");
    const responsePayload = Buffer.from(fixture.request.responseHex, "hex");
    const requestResult = client.request(
      established.data.linkId,
      esm.requestPathHash(Buffer.from(fixture.request.pathHashHex, "hex")),
      requestPayload,
      esm.Tag("Exact", { millis: fixture.request.timeoutMillis }),
    );
    const request = await nextTagged(events, "Request");
    assert.deepEqual(Buffer.from(request.data.data), requestPayload);
    assertSucceeded(
      await server.respond(
        request.data.linkId,
        request.data.requestId,
        request.data.rttMillis,
        responsePayload,
      ),
      "ResponseSent",
    );
    const response = assertSucceeded(await requestResult, "ResponseReceived");
    assert.deepEqual(Buffer.from(response.data.data), responsePayload);

    assertSucceeded(
      await server.setLinkResourceStrategy(
        request.data.linkId,
        esm.Tag("Accept", {
          maximumUncompressedBytes: fixture.resource.maximumUncompressedBytes,
          acceptCompressed: fixture.resource.acceptCompressed,
        }),
      ),
      "ResourceStrategySet",
    );
    const resourcePayload = Buffer.concat(
      fixture.resource.chunksHex.map((chunk) => Buffer.from(chunk, "hex")),
    );
    const metadata = Buffer.from(fixture.resource.metadataHex, "hex");
    writeFileSync(resourcePath, resourcePayload);
    assertSucceeded(
      await client.sendResourceFile(established.data.linkId, resourcePath, {
        packedMetadata: metadata,
        compression: esm.Tag("Never"),
      }),
      "ResourceSent",
    );
    const resource = await nextTagged(events, "ResourceAvailable");
    assert.deepEqual(Buffer.from(resource.data.metadata), metadata);
    const resourceClaim = resource.data.resource.claim();
    assert.equal(resourceClaim.tag, "Claimed");
    const received = [];
    for await (const chunk of resourceClaim.data) {
      received.push(Buffer.from(chunk));
    }
    assert.deepEqual(Buffer.concat(received), resourcePayload);

    assert.equal((await client.stop()).tag, "Stopped");
    assert.equal((await server.stop()).tag, "Stopped");
    client = undefined;
    server = undefined;

    const restoredServerCreated = await esm.Prns.create(
      esm.persistentEndpoint(resolve(root, "server"), [destination]),
    );
    const restoredClientCreated = await esm.Prns.create(
      esm.persistentEndpoint(resolve(root, "client")),
    );
    assert.equal(restoredServerCreated.tag, "Ready");
    assert.equal(restoredClientCreated.tag, "Ready");
    restoredServer = restoredServerCreated.data;
    restoredClient = restoredClientCreated.data;
    assert.deepEqual(Buffer.from(restoredServer.identityHash), serverIdentity);
    assert.deepEqual(Buffer.from(restoredClient.identityHash), clientIdentity);
    assert.deepEqual(Buffer.from(restoredServer.destinationHashes[0]), destinationHash);
    const restoredServerSnapshot = await restoredServer.snapshot();
    const restoredClientSnapshot = await restoredClient.snapshot();
    assert.equal(restoredServerSnapshot.persistence.restored, true);
    assert.equal(restoredClientSnapshot.persistence.restored, true);
    assert.ok(restoredClientSnapshot.routes.some((route) =>
      Buffer.from(route.destination).equals(destinationHash),
    ));
  } finally {
    await restoredClient?.stop().catch(() => {});
    await restoredServer?.stop().catch(() => {});
    await client?.stop().catch(() => {});
    await server?.stop().catch(() => {});
    rmSync(root, { recursive: true, force: true });
  }
});

function assertSucceeded(settlement, outcome) {
  assert.equal(settlement.tag, "Succeeded");
  assert.equal(settlement.data.tag, outcome);
  return settlement.data;
}

async function nextTagged(events, tag) {
  while (true) {
    const result = await withTimeout(events.next(), 5_000, `event ${tag}`);
    assert.equal(result.done, false, `event stream ended before ${tag}`);
    if (result.value.tag === tag) {
      return result.value;
    }
  }
}

async function withTimeout(promise, milliseconds, label) {
  let timer;
  try {
    return await Promise.race([
      promise,
      new Promise((_, reject) => {
        timer = setTimeout(() => reject(new Error(`timeout waiting for ${label}`)), milliseconds);
      }),
    ]);
  } finally {
    clearTimeout(timer);
  }
}

function reserveLoopbackPort() {
  return new Promise((resolvePort, reject) => {
    const server = createServer();
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      assert.ok(address && typeof address !== "string");
      server.close((error) => {
        if (error) {
          reject(error);
        } else {
          resolvePort(address.port);
        }
      });
    });
  });
}
