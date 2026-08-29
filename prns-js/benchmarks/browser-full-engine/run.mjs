import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { readFile, stat } from "node:fs/promises";
import { createServer } from "node:http";
import { extname, resolve } from "node:path";

import { startNativeWebSocketFixture } from "../../scripts/native-websocket-fixture.mjs";

const repositoryRoot = resolve("..");
const chromium = [
  process.env.CHROMIUM_PATH,
  "/snap/bin/chromium",
  "/usr/bin/chromium",
  "/usr/bin/chromium-browser",
  "/usr/bin/google-chrome",
].find((candidate) => candidate && existsSync(candidate));
assert.ok(chromium, "Chromium is required for the browser full-engine benchmark");

const fixture = await startNativeWebSocketFixture();
let settleResult;
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
      response.end(JSON.stringify({
        destinationHex: fixture.destinationHex,
        webSocketUrl: fixture.webSocketUrl,
      }));
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
await new Promise((resolveListening) => {
  server.listen(0, "127.0.0.1", resolveListening);
});

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
      180_000,
    );
  });
  const result = await Promise.race([resultPromise, browserExited, timedOut]);
  assert.equal(result.error, undefined);
  console.log(JSON.stringify(result, null, 2));
} finally {
  clearTimeout(browserTimeout);
  browser?.kill("SIGTERM");
  server.closeAllConnections();
  await new Promise((resolveClosed, rejectClosed) => {
    server.close((error) => error ? rejectClosed(error) : resolveClosed());
  });
  await fixture.stop();
}
