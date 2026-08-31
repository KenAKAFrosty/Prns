import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync } from "node:fs";
import { readFile, stat } from "node:fs/promises";
import { createServer } from "node:http";
import { dirname, extname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const packageRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const repositoryRoot = resolve(packageRoot, "..");
const browserTimeoutMs = process.env.CI ? 60_000 : 20_000;
const chromium = [
  process.env.CHROMIUM_PATH,
  "/snap/bin/chromium",
  "/usr/bin/chromium",
  "/usr/bin/chromium-browser",
  "/usr/bin/google-chrome",
].find((candidate) => candidate && existsSync(candidate));
assert.ok(chromium, "Chromium is required for the browser package smoke");

const contentTypes = new Map([
  [".html", "text/html; charset=utf-8"],
  [".js", "text/javascript; charset=utf-8"],
  [".wasm", "application/wasm"],
]);
let settleBrowserResult;
const browserResult = new Promise((resolveResult) => {
  settleBrowserResult = resolveResult;
});
const workerStatusSockets = new Set();
const server = createServer(async (request, response) => {
  try {
    const url = new URL(request.url ?? "/", "http://127.0.0.1");
    if (
      request.method === "POST" &&
      url.pathname === "/browser-smoke-result"
    ) {
      const chunks = [];
      for await (const chunk of request) {
        chunks.push(chunk);
      }
      const result = JSON.parse(Buffer.concat(chunks).toString("utf8"));
      response.writeHead(204);
      response.end();
      settleBrowserResult(result);
      return;
    }
    if (
      request.method === "POST" &&
      url.pathname === "/close-worker-status-sockets"
    ) {
      response.writeHead(204);
      response.end();
      for (const socket of workerStatusSockets) {
        socket.end(Buffer.from([0x88, 0x00]));
      }
      return;
    }
    const path = resolve(repositoryRoot, `.${decodeURIComponent(url.pathname)}`);
    const metadata = await stat(path);
    assert.ok(path.startsWith(`${repositoryRoot}/`) && metadata.isFile());
    response.writeHead(200, {
      "content-type":
        contentTypes.get(extname(path)) ?? "application/octet-stream",
    });
    response.end(await readFile(path));
  } catch {
    response.writeHead(404);
    response.end();
  }
});
server.on("upgrade", (request, socket) => {
  const url = new URL(request.url ?? "/", "http://127.0.0.1");
  const key = request.headers["sec-websocket-key"];
  if (url.pathname !== "/worker-status-socket" || typeof key !== "string") {
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
  workerStatusSockets.add(socket);
  socket.once("close", () => workerStatusSockets.delete(socket));
});

await new Promise((resolveListening) => {
  server.listen(0, "127.0.0.1", resolveListening);
});

let browser;
try {
  const address = server.address();
  assert.ok(address && typeof address === "object");
  const url =
    `http://127.0.0.1:${address.port}` +
    "/prns-js/tests/browser-auto-consumer.html";
  browser = spawn(
    chromium,
    [
      "--headless=new",
      "--no-sandbox",
      "--disable-dev-shm-usage",
      "--disable-gpu",
      url,
    ],
    { stdio: "ignore" },
  );
  const browserExited = new Promise((_, rejectExit) => {
    browser.once("error", rejectExit);
    browser.once("exit", (code, signal) => {
      rejectExit(
        new Error(
          `Chromium exited before reporting a result: code=${code} signal=${signal}`,
        ),
      );
    });
  });
  let browserTimeout;
  const result = await Promise.race([
    browserResult,
    browserExited,
    new Promise((_, rejectTimeout) => {
      browserTimeout = setTimeout(
        () => rejectTimeout(
          new Error(`browser smoke timed out after ${browserTimeoutMs}ms`),
        ),
        browserTimeoutMs,
      );
    }),
  ]);
  clearTimeout(browserTimeout);
  assert.deepEqual(result, {
    title: "PASS",
    execution: "DedicatedWorker",
    outcome: "Ready",
    command: "Failed:UnknownLink",
    resource: "Failed:UnknownLink",
    blob: "Failed:UnknownLink",
    snapshot: "Consistent",
    persistence: "Restored",
    persistenceFailures: "Typed",
    routePersistence: "Restored",
    webSocketFraming: "Resolved",
    webSocketWorkerStatus: "PushedOnce",
    bluetoothContract: "Shared",
    bluetoothSession: "Bridged",
    bluetoothWorker: "Bridged",
    projection: "Reactive",
    admission: "RuntimeRejected:worker-admission",
    autoWifiAdmission: "Retained",
    stop: "Idempotent",
    compression: "Compressed",
    compressionDetail: "message:message",
  });
} finally {
  browser?.kill("SIGTERM");
  for (const socket of workerStatusSockets) {
    socket.destroy();
  }
  await new Promise((resolveClosed, rejectClosed) => {
    server.close((error) => {
      if (error) {
        rejectClosed(error);
      } else {
        resolveClosed();
      }
    });
  });
}
