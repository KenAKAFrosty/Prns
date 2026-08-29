import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { readFile, stat } from "node:fs/promises";
import { createServer } from "node:http";
import { extname, resolve } from "node:path";

import { startNativeWebSocketFixture } from "../scripts/native-websocket-fixture.mjs";

const galleryRoot = resolve("examples/framework-gallery/dist");
const fixture = await startNativeWebSocketFixture();
const chromium = [
  process.env.CHROMIUM_PATH,
  "/snap/bin/chromium",
  "/usr/bin/chromium",
  "/usr/bin/chromium-browser",
  "/usr/bin/google-chrome",
].find((candidate) => candidate && existsSync(candidate));
assert.ok(chromium, "Chromium is required for the framework gallery smoke");

const contentTypes = new Map([
  [".css", "text/css; charset=utf-8"],
  [".html", "text/html; charset=utf-8"],
  [".js", "text/javascript; charset=utf-8"],
  [".json", "application/json; charset=utf-8"],
  [".mjs", "text/javascript; charset=utf-8"],
  [".wasm", "application/wasm"],
]);
let settleResult;
const resultPromise = new Promise((resolveResult) => {
  settleResult = resolveResult;
});
const requests = [];
const instrumentation = `<script>
  let reported = false;
  let journeyStarted = false;
  let linkedAt;
  const report = (value) => {
    if (reported) return;
    reported = true;
    fetch("/framework-gallery-result", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(value),
    });
  };
  addEventListener("error", (event) => report({
    error: event.error instanceof Error ? event.error.stack : event.message,
    file: event.filename,
    line: event.lineno,
    column: event.colno,
  }));
  addEventListener("unhandledrejection", (event) => report({
    error: event.reason instanceof Error ? event.reason.stack : String(event.reason),
  }));
  setInterval(() => {
    if (
      document.documentElement.dataset.gallery === "Ready" &&
      document.documentElement.dataset.journey === "Ready" &&
      !journeyStarted
    ) {
      journeyStarted = true;
      document.getElementById("run-journey").click();
    }
    const panels = [...document.querySelectorAll("[data-framework]")];
    if (
      document.documentElement.dataset.journey === "Linked" &&
      linkedAt === undefined
    ) {
      linkedAt = performance.now();
    }
    if (
      document.documentElement.dataset.journey === "Linked" &&
      panels.length === 6 &&
      (
        panels.every((panel) =>
          Number(panel.dataset.interfaces) === 1 &&
          Number(panel.dataset.routes) >= 1 &&
          Number(panel.dataset.links) === 1 &&
          Number(panel.dataset.diagnostics) >= 1
        ) ||
        performance.now() - linkedAt >= 2_000
      )
    ) {
      report({
        ready: true,
        journey: document.documentElement.dataset.journey,
        html: document.documentElement.outerHTML,
        panels: panels.map((panel) => ({
          framework: panel.dataset.framework,
          interfaces: Number(panel.dataset.interfaces),
          routes: Number(panel.dataset.routes),
          links: Number(panel.dataset.links),
          diagnostics: Number(panel.dataset.diagnostics),
        })),
      });
    }
  }, 10);
</script>`;
const server = createServer(async (request, response) => {
  try {
    const url = new URL(request.url ?? "/", "http://127.0.0.1");
    requests.push(`${request.method ?? "UNKNOWN"} ${url.pathname}`);
    if (request.method === "GET" && url.pathname === "/api/gallery-session") {
      response.writeHead(200, { "content-type": "application/json" });
      response.end(JSON.stringify({
        destinationHex: fixture.destinationHex,
        webSocketUrl: fixture.webSocketUrl,
      }));
      return;
    }
    if (
      request.method === "POST" &&
      url.pathname === "/framework-gallery-result"
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
      ? "index.html"
      : decodeURIComponent(url.pathname).slice(1);
    const path = resolve(galleryRoot, relative);
    const metadata = await stat(path);
    assert.ok(path.startsWith(`${galleryRoot}/`) && metadata.isFile());
    response.writeHead(200, {
      "content-type": contentTypes.get(extname(path)) ?? "application/octet-stream",
    });
    const body = await readFile(path);
    response.end(
      relative === "index.html"
        ? body.toString("utf8").replace("</body>", `${instrumentation}</body>`)
        : body,
    );
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
        `Chromium exited before the gallery reported: code=${code} signal=${signal}`,
      ));
    });
  });
  const timedOut = new Promise((_, rejectTimeout) => {
    browserTimeout = setTimeout(
      () => rejectTimeout(new Error(
        `framework gallery smoke timed out; requests=${requests.join(",")}; chromium=${Buffer.concat(browserErrors)}`,
      )),
      30_000,
    );
  });
  const result = await Promise.race([resultPromise, browserExited, timedOut]);
  assert.equal(result.error, undefined);
  assert.equal(result.ready, true);
  assert.equal(result.journey, "Linked");
  for (const framework of [
    "react",
    "solid",
    "vue",
    "svelte",
    "qwik",
    "web-component",
  ]) {
    assert.match(result.html, new RegExp(`data-framework="${framework}"`));
  }
  assert.match(result.html, /Ready · DedicatedWorker · Cooperative/);
  assert.deepEqual(
    result.panels.map((panel) => panel.framework).sort(),
    ["qwik", "react", "solid", "svelte", "vue", "web-component"],
  );
  for (const panel of result.panels) {
    assert.equal(panel.interfaces, 1, `${panel.framework} interface projection`);
    assert.ok(panel.routes >= 1, `${panel.framework} route projection`);
    assert.equal(panel.links, 1, `${panel.framework} link projection`);
    assert.ok(panel.diagnostics >= 1, `${panel.framework} diagnostic projection`);
  }
} finally {
  clearTimeout(browserTimeout);
  browser?.kill("SIGTERM");
  server.closeAllConnections();
  await new Promise((resolveClosed, rejectClosed) => {
    server.close((error) => error ? rejectClosed(error) : resolveClosed());
  });
  await fixture.stop();
}
