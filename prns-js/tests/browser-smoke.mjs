import assert from "node:assert/strict";
import { spawn } from "node:child_process";
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
    outcome: "Ready",
    command: "Failed:UnknownLink",
    resource: "Failed:UnknownLink",
    blob: "Failed:UnknownLink",
    snapshot: "Consistent",
    persistence: "Restored",
    persistenceFailures: "Typed",
    routePersistence: "Restored",
    webSocketFraming: "Resolved",
    bluetoothContract: "Shared",
    bluetoothSession: "Bridged",
    compression: "Compressed",
    compressionDetail: "message:message",
  });
} finally {
  browser?.kill("SIGTERM");
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
