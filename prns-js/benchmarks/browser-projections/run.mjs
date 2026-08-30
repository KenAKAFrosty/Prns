import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { readFile, stat } from "node:fs/promises";
import { createServer } from "node:http";
import { dirname, extname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const packageRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const chromium = [
  process.env.CHROMIUM_PATH,
  "/snap/bin/chromium",
  "/usr/bin/chromium",
  "/usr/bin/chromium-browser",
  "/usr/bin/google-chrome",
].find((candidate) => candidate && existsSync(candidate));
assert.ok(chromium, "Chromium is required for the browser projection smoke");

const contentTypes = new Map([
  [".html", "text/html; charset=utf-8"],
  [".js", "text/javascript; charset=utf-8"],
]);
let settleResult;
const resultPromise = new Promise((resolveResult) => {
  settleResult = resolveResult;
});
const server = createServer(async (request, response) => {
  try {
    const url = new URL(request.url ?? "/", "http://127.0.0.1");
    if (
      request.method === "POST" &&
      url.pathname === "/browser-projections-result"
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
    const path = resolve(packageRoot, `.${decodeURIComponent(url.pathname)}`);
    const metadata = await stat(path);
    assert.ok(path.startsWith(`${packageRoot}/`) && metadata.isFile());
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
try {
  const address = server.address();
  assert.ok(address && typeof address === "object");
  const url = `http://127.0.0.1:${address.port}/benchmarks/browser-projections/index.html`;
  browser = spawn(chromium, [
    "--headless=new",
    "--no-sandbox",
    "--disable-dev-shm-usage",
    "--disable-gpu",
    url,
  ], { stdio: "ignore" });
  const browserExited = new Promise((_, rejectExit) => {
    browser.once("error", rejectExit);
    browser.once("exit", (code, signal) => {
      rejectExit(new Error(
        `Chromium exited before reporting projection results: code=${code} signal=${signal}`,
      ));
    });
  });
  const timedOut = new Promise((_, rejectTimeout) => {
    browserTimeout = setTimeout(
      () => rejectTimeout(new Error("browser projection smoke timed out")),
      30_000,
    );
  });
  const result = await Promise.race([resultPromise, browserExited, timedOut]);
  assert.deepEqual({
    publications: result.publications,
    notifications: result.local.notifications,
    localLifecycle: result.local.finalLifecycle,
    frames: result.wire.frames,
    frameKind: result.wire.frameKind,
    updates: result.wire.updates,
    wireLifecycle: result.wire.finalLifecycle,
  }, {
    publications: 10_000,
    notifications: 1,
    localLifecycle: "Running",
    frames: 1,
    frameKind: "ClonedBatch",
    updates: 1,
    wireLifecycle: "Running",
  });
  process.stdout.write(`${JSON.stringify(result, null, 2)}\n`);
} finally {
  clearTimeout(browserTimeout);
  browser?.kill("SIGTERM");
  server.closeAllConnections();
  await new Promise((resolveClosed, rejectClosed) => {
    server.close((error) => error ? rejectClosed(error) : resolveClosed());
  });
}
