import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync } from "node:fs";
import { readFile, stat } from "node:fs/promises";
import { createServer } from "node:http";
import { extname, resolve } from "node:path";

const repositoryRoot = resolve("..");
const chromium = [
  process.env.CHROMIUM_PATH,
  "/snap/bin/chromium",
  "/usr/bin/chromium",
  "/usr/bin/chromium-browser",
  "/usr/bin/google-chrome",
].find((candidate) => candidate && existsSync(candidate));
assert.ok(chromium, "Chromium is required for the browser full-engine benchmark");
const wasmArtifactPath = resolve("../prns-wasm/smoke/pkg/prns_wasm_bg.wasm");
const wasmArtifact = await readFile(wasmArtifactPath);
const wasmArtifactProvenance = {
  byteLength: wasmArtifact.byteLength,
  sha256: createHash("sha256").update(wasmArtifact).digest("hex"),
};

let settleResult;
let nextRelayMeasurementId = 1;
const activeRelayMeasurements = new Map();
const relayMeasurements = new Map();
const resultPromise = new Promise((resolveResult) => {
  settleResult = resolveResult;
});
const contentTypes = new Map([
  [".html", "text/html; charset=utf-8"],
  [".js", "text/javascript; charset=utf-8"],
  [".wasm", "application/wasm"],
]);
const server = createServer(async (request, response) => {
  try {
    const url = new URL(request.url ?? "/", "http://127.0.0.1");
    if (
      request.method === "GET" &&
      url.pathname === "/browser-full-engine-session"
    ) {
      response.writeHead(200, { "content-type": "application/json" });
      const address = server.address();
      assert.ok(address && typeof address === "object");
      response.end(JSON.stringify({
        webSocketUrl: `ws://127.0.0.1:${address.port}/browser-full-engine-relay`,
        wasmArtifact: wasmArtifactProvenance,
      }));
      return;
    }
    if (
      request.method === "GET" &&
      url.pathname === "/browser-full-engine-progress"
    ) {
      process.stderr.write(`[browser] ${url.searchParams.get("stage") ?? "unknown"}\n`);
      response.writeHead(204);
      response.end();
      return;
    }
    if (
      request.method === "GET" &&
      url.pathname === "/browser-full-engine-relay-measure-start"
    ) {
      const peer = url.searchParams.get("peer");
      const expected = Number(url.searchParams.get("expected"));
      assert.ok(peer);
      assert.ok(Number.isSafeInteger(expected) && expected > 0);
      const measurement = beginRelayMeasurement(peer, expected);
      response.writeHead(200, { "content-type": "application/json" });
      response.end(JSON.stringify({ id: measurement.id }));
      return;
    }
    if (
      request.method === "GET" &&
      url.pathname === "/browser-full-engine-relay-span-start"
    ) {
      const peer = url.searchParams.get("peer");
      assert.ok(peer);
      const measurement = beginRelayMeasurement(peer, undefined);
      response.writeHead(200, { "content-type": "application/json" });
      response.end(JSON.stringify({ id: measurement.id }));
      return;
    }
    if (
      request.method === "GET" &&
      url.pathname === "/browser-full-engine-relay-span-stop"
    ) {
      const id = Number(url.searchParams.get("id"));
      const measurement = relayMeasurements.get(id);
      assert.ok(measurement);
      measurement.completedAt = performance.now();
      activeRelayMeasurements.delete(measurement.peer);
      response.writeHead(200, { "content-type": "application/json" });
      response.end(JSON.stringify(relayMeasurementResult(measurement)));
      return;
    }
    if (
      request.method === "GET" &&
      url.pathname === "/browser-full-engine-relay-measure"
    ) {
      const id = Number(url.searchParams.get("id"));
      const measurement = relayMeasurements.get(id);
      assert.ok(measurement);
      response.writeHead(200, { "content-type": "application/json" });
      response.end(JSON.stringify(relayMeasurementResult(measurement)));
      return;
    }
    if (
      request.method === "POST" &&
      url.pathname === "/browser-full-engine-result"
    ) {
      const chunks = [];
      for await (const chunk of request) {
        chunks.push(chunk);
      }
      const result = JSON.parse(Buffer.concat(chunks).toString("utf8"));
      response.writeHead(204);
      response.end();
      settleResult(result);
      return;
    }
    const relative = url.pathname === "/"
      ? "prns-js/benchmarks/browser-full-engine/index.html"
      : decodeURIComponent(url.pathname).slice(1);
    const path = resolve(repositoryRoot, relative);
    const metadata = await stat(path);
    assert.ok(path.startsWith(`${repositoryRoot}/`) && metadata.isFile());
    response.writeHead(200, {
      "content-type": contentTypes.get(extname(path)) ?? "application/octet-stream",
    });
    response.end(await readFile(path));
  } catch {
    response.writeHead(404);
    response.end();
  }
});
const relaySockets = new Set();
server.on("upgrade", (request, socket) => {
  const url = new URL(request.url ?? "/", "http://127.0.0.1");
  const key = request.headers["sec-websocket-key"];
  const peer = url.searchParams.get("peer");
  const lane = url.searchParams.get("lane");
  if (
    url.pathname !== "/browser-full-engine-relay" ||
    typeof key !== "string" ||
    peer === null ||
    lane === null
  ) {
    socket.destroy();
    return;
  }
  const accept = createHash("sha1")
    .update(`${key}258EAFA5-E914-47DA-95CA-C5AB0DC85B11`)
    .digest("base64");
  socket.write([
    "HTTP/1.1 101 Switching Protocols",
    "Upgrade: websocket",
    "Connection: Upgrade",
    `Sec-WebSocket-Accept: ${accept}`,
    "",
    "",
  ].join("\r\n"));
  relaySockets.add(socket);
  socket.prnsPeer = peer;
  socket.prnsLane = lane;
  let buffered = Buffer.alloc(0);
  let fragments = [];
  socket.on("data", (chunk) => {
    buffered = Buffer.concat([buffered, chunk]);
    while (true) {
      const decoded = decodeClientFrame(buffered);
      if (decoded === undefined) {
        return;
      }
      buffered = buffered.subarray(decoded.consumed);
      if (decoded.opcode === 8) {
        socket.end(serverFrame(decoded.payload, 8));
        return;
      }
      if (decoded.opcode === 9) {
        socket.write(serverFrame(decoded.payload, 10));
        continue;
      }
      if (decoded.opcode === 2) {
        if (decoded.final) {
          observeRelayFrame(socket, decoded.payload.length);
          broadcastBinary(socket, decoded.payload);
        } else {
          fragments = [decoded.payload];
        }
        continue;
      }
      if (decoded.opcode === 0 && fragments.length > 0) {
        fragments.push(decoded.payload);
        if (decoded.final) {
          const payload = Buffer.concat(fragments);
          observeRelayFrame(socket, payload.length);
          broadcastBinary(socket, payload);
          fragments = [];
        }
      }
    }
  });
  socket.once("close", () => relaySockets.delete(socket));
  socket.once("error", () => relaySockets.delete(socket));
});
await new Promise((resolveListening) => {
  server.listen(0, "127.0.0.1", resolveListening);
});

if (process.env.PRNS_BENCH_SERVE_ONLY === "1") {
  const address = server.address();
  assert.ok(address && typeof address === "object");
  console.log(`http://127.0.0.1:${address.port}/`);
  await new Promise(() => undefined);
}

let browser;
let browserTimeout;
const browserErrors = [];
try {
  const address = server.address();
  assert.ok(address && typeof address === "object");
  browser = spawn(chromium, [
    "--headless=new",
    "--no-sandbox",
    "--disable-dev-shm-usage",
    "--disable-gpu",
    `http://127.0.0.1:${address.port}/`,
  ], { stdio: ["ignore", "ignore", "pipe"] });
  browser.stderr.on("data", (chunk) => browserErrors.push(chunk));
  const browserExited = new Promise((_, rejectExit) => {
    browser.once("error", rejectExit);
    browser.once("exit", (code, signal) => {
      rejectExit(new Error(
        `Chromium exited before the full-engine benchmark reported: code=${code} signal=${signal}`,
      ));
    });
  });
  const timedOut = new Promise((_, rejectTimeout) => {
    browserTimeout = setTimeout(
      () => rejectTimeout(new Error(
        `browser full-engine benchmark timed out; chromium=${Buffer.concat(browserErrors)}`,
      )),
      600_000,
    );
  });
  const result = await Promise.race([resultPromise, browserExited, timedOut]);
  assert.equal(result.error, undefined);
  console.log(JSON.stringify(result, null, 2));
} finally {
  clearTimeout(browserTimeout);
  browser?.kill("SIGTERM");
  server.closeAllConnections();
  for (const socket of relaySockets) {
    socket.destroy();
  }
  await new Promise((resolveClosed, rejectClosed) => {
    server.close((error) => error ? rejectClosed(error) : resolveClosed());
  });
}

function broadcastBinary(sender, payload) {
  const frame = serverFrame(payload, 2);
  for (const socket of relaySockets) {
    if (
      socket !== sender &&
      !socket.destroyed &&
      socket.prnsLane === sender.prnsLane
    ) {
      socket.write(frame);
    }
  }
}

function beginRelayMeasurement(peer, expected) {
  assert.equal(activeRelayMeasurements.has(peer), false);
  const id = nextRelayMeasurementId;
  nextRelayMeasurementId += 1;
  const measurement = {
    id,
    peer,
    expected,
    count: 0,
    bytes: 0,
    startedAt: performance.now(),
    firstAt: undefined,
    lastAt: undefined,
    completedAt: undefined,
  };
  activeRelayMeasurements.set(peer, measurement);
  relayMeasurements.set(id, measurement);
  return measurement;
}

function relayMeasurementResult(measurement) {
  return {
    count: measurement.count,
    ...(measurement.expected === undefined ? {} : { expected: measurement.expected }),
    bytes: measurement.bytes,
    ...(measurement.firstAt === undefined
      ? {}
      : { firstMillis: measurement.firstAt - measurement.startedAt }),
    ...(measurement.lastAt === undefined
      ? {}
      : { lastMillis: measurement.lastAt - measurement.startedAt }),
    ...(measurement.completedAt === undefined
      ? {}
      : { completeMillis: measurement.completedAt - measurement.startedAt }),
  };
}

function observeRelayFrame(socket, payloadBytes) {
  const peer = socket.prnsPeer;
  if (peer === null) {
    return;
  }
  const measurement = activeRelayMeasurements.get(peer);
  if (measurement === undefined) {
    return;
  }
  const now = performance.now();
  measurement.firstAt ??= now;
  measurement.lastAt = now;
  measurement.count += 1;
  measurement.bytes += payloadBytes;
  if (
    measurement.expected !== undefined &&
    measurement.count === measurement.expected
  ) {
    measurement.completedAt = now;
    activeRelayMeasurements.delete(peer);
  }
}

function decodeClientFrame(buffer) {
  if (buffer.length < 2) {
    return undefined;
  }
  const final = (buffer[0] & 0x80) !== 0;
  const opcode = buffer[0] & 0x0f;
  const masked = (buffer[1] & 0x80) !== 0;
  let length = buffer[1] & 0x7f;
  let offset = 2;
  if (length === 126) {
    if (buffer.length < 4) {
      return undefined;
    }
    length = buffer.readUInt16BE(2);
    offset = 4;
  } else if (length === 127) {
    if (buffer.length < 10) {
      return undefined;
    }
    const wide = buffer.readBigUInt64BE(2);
    if (wide > BigInt(Number.MAX_SAFE_INTEGER)) {
      throw new Error("WebSocket frame exceeds safe length");
    }
    length = Number(wide);
    offset = 10;
  }
  if (!masked || buffer.length < offset + 4 + length) {
    return undefined;
  }
  const mask = buffer.subarray(offset, offset + 4);
  offset += 4;
  const payload = Buffer.allocUnsafe(length);
  for (let index = 0; index < length; index += 1) {
    payload[index] = buffer[offset + index] ^ mask[index & 3];
  }
  return { final, opcode, payload, consumed: offset + length };
}

function serverFrame(payload, opcode) {
  let header;
  if (payload.length < 126) {
    header = Buffer.from([0x80 | opcode, payload.length]);
  } else if (payload.length <= 0xffff) {
    header = Buffer.allocUnsafe(4);
    header[0] = 0x80 | opcode;
    header[1] = 126;
    header.writeUInt16BE(payload.length, 2);
  } else {
    header = Buffer.allocUnsafe(10);
    header[0] = 0x80 | opcode;
    header[1] = 127;
    header.writeBigUInt64BE(BigInt(payload.length), 2);
  }
  return Buffer.concat([header, payload]);
}
