import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { readFile, stat } from "node:fs/promises";
import { createServer } from "node:http";
import { dirname, extname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const packageRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const repositoryRoot = resolve(packageRoot, "..");
const chromium = [
  process.env.CHROMIUM_PATH,
  "/snap/bin/chromium",
  "/usr/bin/chromium",
  "/usr/bin/chromium-browser",
  "/usr/bin/google-chrome",
].find((candidate) => candidate && existsSync(candidate));
assert.ok(chromium, "Chromium is required for the browser crypto worker benchmark");

const contentTypes = new Map([
  [".html", "text/html; charset=utf-8"],
  [".js", "text/javascript; charset=utf-8"],
  [".wasm", "application/wasm"],
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
      url.pathname === "/browser-crypto-workers-result"
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
    const root = url.pathname.startsWith("/prns-wasm/")
      ? repositoryRoot
      : packageRoot;
    const path = resolve(root, `.${decodeURIComponent(url.pathname)}`);
    const metadata = await stat(path);
    assert.ok(path.startsWith(`${root}/`) && metadata.isFile());
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
let benchmarkTimeout;
try {
  const address = server.address();
  assert.ok(address && typeof address === "object");
  const url = `http://127.0.0.1:${address.port}/benchmarks/browser-crypto-workers/index.html`;
  browser = spawn(chromium, [
    "--headless=new",
    "--no-sandbox",
    "--disable-dev-shm-usage",
    "--disable-gpu",
    url,
  ], { stdio: "ignore" });
  const exited = new Promise((_, reject) => {
    browser.once("error", reject);
    browser.once("exit", (code, signal) => {
      reject(new Error(`Chromium exited before reporting: code=${code} signal=${signal}`));
    });
  });
  const timeout = new Promise((_, reject) => {
    benchmarkTimeout = setTimeout(
      () => reject(new Error("browser crypto worker benchmark timed out")),
      120_000,
    );
  });
  const result = await Promise.race([resultPromise, exited, timeout]);
  if (result.error !== undefined) {
    throw new Error(result.error);
  }
  const output = process.argv.includes("--summary")
    ? summarize(result)
    : result;
  process.stdout.write(`${JSON.stringify(output, null, 2)}\n`);
} finally {
  clearTimeout(benchmarkTimeout);
  browser?.kill("SIGTERM");
  server.closeAllConnections();
  await new Promise((resolveClosed, rejectClosed) => {
    server.close((error) => error ? rejectClosed(error) : resolveClosed());
  });
}

function summarize(result) {
  return {
    userAgent: result.userAgent,
    hardwareConcurrency: result.hardwareConcurrency,
    gatewayReadiness: result.gatewayReadiness,
    protocol: result.protocolPerformance
      .filter(({ lanes }) => lanes === 4)
      .map(({ operation, configuration, elapsedMillis, operationsPerSecond }) => ({
        operation,
        configuration,
        elapsedMillis,
        operationsPerSecond,
      })),
    portableWasm: result.portableWasmWorkers.results
      .filter(({ configuration }) => configuration !== "OneWorker")
      .map(({
        operation,
        mode,
        configuration,
        elapsedMillis,
        operationsPerSecond,
        speedupOverInline,
        medianCoordinatorP95Millis,
      }) => ({
        operation,
        mode,
        configuration,
        elapsedMillis,
        operationsPerSecond,
        speedupOverInline,
        medianCoordinatorP95Millis,
      })),
    mixedRoles: result.portableWasmWorkers.mixedRoleScaling,
    resources: result.results
      .filter(({ lanes }) => lanes === 4)
      .map(({
        resourceBytes,
        jobs,
        configuration,
        elapsedMillis,
        mebibytesPerSecond,
        speedupOverInline,
        medianCoordinatorP95Millis,
      }) => ({
        resourceBytes,
        jobs,
        configuration,
        elapsedMillis,
        mebibytesPerSecond,
        speedupOverInline,
        medianCoordinatorP95Millis,
      })),
  };
}
