// Manual hardware qualification. Requires the documentation site's Playwright dependency.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { createServer } from "node:http";
import { createRequire } from "node:module";
import { readFile } from "node:fs/promises";
import { extname, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

const [assetsArgument, expectedPage, url, ...destinations] = process.argv.slice(2);
if (!assetsArgument || !expectedPage || !url || destinations.length === 0 ||
    destinations.some(value => !/^[a-f0-9]{32}$/i.test(value))) {
  throw new Error("Usage: node websocket-browser-smoke.mjs ASSET_DIR EXPECTED_PAGE_FILE WS_URL DESTINATION...");
}
const assets = resolve(assetsArgument);
const root = fileURLToPath(new URL("../../../", import.meta.url));
const require = createRequire(resolve(root, "docs/website/package.json"));
const { chromium } = require("@playwright/test");
const pageBytes = await readFile(expectedPage);
assert(pageBytes.length > 255 && pageBytes.length <= 65535, "fixture uses MessagePack bin16");
const header = Buffer.alloc(3);
header[0] = 0xc5;
header.writeUInt16BE(pageBytes.length, 1);
const expected = Buffer.concat([header, pageBytes]);
const requestPath = [...createHash("sha256").update("/page/index.mu").digest().subarray(0, 16)];
const remoteOrigin = process.env.PRNS_BROWSER_ORIGIN;
const holdMillis = Number(process.env.PRNS_BROWSER_HOLD_MS ?? 0);
assert(Number.isSafeInteger(holdMillis) && holdMillis >= 0 && holdMillis <= 30000);
const server = createServer(async (request, response) => {
  try {
    if (request.url === "/favicon.ico") {
      response.writeHead(204).end();
      return;
    }
    if (request.url === "/") {
      response.setHeader("Content-Type", "text/html");
      response.end("<!doctype html><title>Hopspot browser qualification</title>");
      return;
    }
    const path = resolve(assets, `.${new URL(request.url, "http://localhost").pathname}`);
    if (!path.startsWith(assets + sep)) {
      response.writeHead(403).end();
      return;
    }
    const data = await readFile(path);
    response.setHeader("Content-Type", extname(path) === ".wasm" ? "application/wasm" :
      extname(path) === ".js" ? "text/javascript" : "application/octet-stream");
    response.end(data);
  } catch {
    response.writeHead(404).end();
  }
});
await new Promise(done => server.listen(0, "127.0.0.1", done));
let browser;
try {
  browser = await chromium.launch({
    headless: true,
    ...(process.env.PRNS_BROWSER_CHANNEL ? { channel: process.env.PRNS_BROWSER_CHANNEL } : {}),
  });
  const results = await Promise.all(destinations.map(async destination => {
    const context = await browser.newContext();
    const blockedRequests = [];
    if (remoteOrigin) {
      const allowedOrigin = new URL(remoteOrigin).origin;
      await context.route("**/*", route => {
        if (new URL(route.request().url()).origin !== allowedOrigin) {
          blockedRequests.push(route.request().url());
          return route.abort();
        }
        return route.continue();
      });
    }
    const page = await context.newPage();
    await page.goto(remoteOrigin ?? `http://127.0.0.1:${server.address().port}/`);
    const result = await page.evaluate(async ({ url, destination, holdMillis, requestPath }) => {
      const sdk = await import("/sdk/browser/index.js");
      const created = await sdk.Prns.create({
        wasmModuleUrl: new URL("/pkg/prns_wasm.js", location.href),
        resourceCompressionModuleUrl: new URL("/pkg/prns_wasm.js", location.href),
      });
      if (created.tag !== "Ready") throw new Error(`create: ${JSON.stringify(created)}`);
      const node = created.data;
      const diagnostics = [];
      const consumers = [node.claimEvents(), node.claimDiagnostics()].map(async claim => {
        if (claim.tag === "AlreadyClaimed") throw new Error("event stream already claimed");
        for await (const event of claim.data) {
          if (diagnostics.length < 64) diagnostics.push(event.tag);
        }
      });
      let timer;
      try {
        return await Promise.race([
          new Promise((_, reject) => {
            timer = setTimeout(() => reject(new Error(`probe timeout; ${diagnostics}`)), 60000);
          }),
          (async () => {
            const connected = await node.interfaces.webSocket.connect(url, { framing: "RawPacket" });
            if (connected.tag !== "Connected") throw new Error(`connect: ${JSON.stringify(connected)}`);
            const target = sdk.destinationHash(Uint8Array.from(destination.match(/../g), value => parseInt(value, 16)));
            const path = await node.requestPath(target);
            if (path.tag !== "Succeeded") throw new Error(`path: ${JSON.stringify(path)}`);
            const link = await node.establishLink(target);
            if (link.tag !== "Succeeded") throw new Error(`link: ${JSON.stringify(link)}`);
            const pathHash = new Uint8Array(requestPath);
            const response = await node.request(link.data.data.linkId, sdk.requestPathHash(pathHash), new Uint8Array());
            if (response.tag !== "Succeeded") throw new Error(`request: ${JSON.stringify(response)}`);
            const bytes = response.data.data.data;

            await new Promise(done => setTimeout(done, holdMillis));
            return { destination, payload: Array.from(bytes), rttMillis: response.data.data.rttMillis, secureContext: isSecureContext, subtleCrypto: typeof crypto.subtle !== "undefined" };
          })(),
        ]);
      } finally {
        clearTimeout(timer);
        await node.stop();
        await Promise.all(consumers);
      }
    }, { url, destination, holdMillis, requestPath });
    assert.deepEqual(blockedRequests, [], "remote browser assets must be self-contained");
    const { payload, ...measurement } = result;
    const received = Buffer.from(payload);
    assert.deepEqual(received, expected, "browser response must match the compiled Hopspot page");
    return { ...measurement, bytes: received.length, sha256: createHash("sha256").update(received).digest("hex") };
  }));
  console.log(JSON.stringify({ browser: await browser.version(), origin: remoteOrigin ?? "loopback", url, results }, null, 2));
} finally {
  await browser?.close();
  server.close();
}
