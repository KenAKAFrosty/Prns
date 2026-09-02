import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdtemp, readFile, rm, stat } from "node:fs/promises";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { dirname, extname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { qwikVite } from "@builder.io/qwik/optimizer";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { build } from "vite";
import { findChromium } from "./chromium.mjs";

const packageRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const testRoot = resolve(packageRoot, "tests/framework-adapters");
const browserTimeoutMs = process.env.CI ? 60_000 : 30_000;
const qwikRuntime = fileURLToPath(import.meta.resolve("@builder.io/qwik"));
const prnsBrowser = fileURLToPath(import.meta.resolve("personal-rns/browser"));
const prnsQwik = fileURLToPath(import.meta.resolve("personal-rns/qwik"));
const chromium = findChromium();
assert.ok(chromium, "Chromium is required for the framework adapter test");

const temporaryRoot = await mkdtemp(join(tmpdir(), "prns-framework-adapters-"));
const qwikOutput = resolve(temporaryRoot, "qwik");
const browserOutput = resolve(temporaryRoot, "browser");

let server;
let browser;
try {
  await build({
    configFile: false,
    logLevel: "error",
    mode: "lib",
    plugins: [qwikVite({ srcDir: testRoot })],
    build: {
      target: "es2022",
      outDir: qwikOutput,
      emptyOutDir: true,
      lib: {
        entry: resolve(testRoot, "QwikConsumer.tsx"),
        formats: ["es"],
        fileName: () => "QwikConsumer.qwik.mjs",
      },
      rollupOptions: {
        external: [
          "@builder.io/qwik",
          "@builder.io/qwik/jsx-runtime",
          "personal-rns/browser",
          "personal-rns/qwik",
        ],
      },
    },
  });

  await build({
    configFile: false,
    logLevel: "error",
    root: testRoot,
    publicDir: false,
    plugins: [svelte()],
    define: {
      "process.env.NODE_ENV": JSON.stringify("production"),
    },
    resolve: {
      alias: [
        {
          find: "adapter-qwik-consumer",
          replacement: resolve(
            qwikOutput,
            "QwikConsumer.qwik.mjs",
          ),
        },
        {
          find: /^@builder\.io\/qwik\/jsx-runtime$/,
          replacement: qwikRuntime,
        },
        { find: /^@builder\.io\/qwik$/, replacement: qwikRuntime },
        { find: /^personal-rns\/browser$/, replacement: prnsBrowser },
        { find: /^personal-rns\/qwik$/, replacement: prnsQwik },
      ],
    },
    worker: { format: "es" },
    build: {
      target: "es2022",
      outDir: browserOutput,
      emptyOutDir: true,
    },
  });

  let settleBrowserResult;
  const browserResult = new Promise((resolveResult) => {
    settleBrowserResult = resolveResult;
  });
  server = createServer(async (request, response) => {
    try {
      const url = new URL(request.url ?? "/", "http://127.0.0.1");
      if (
        request.method === "POST" &&
        url.pathname === "/framework-adapter-result"
      ) {
        const chunks = [];
        for await (const chunk of request) {
          chunks.push(chunk);
        }
        response.writeHead(204);
        response.end();
        settleBrowserResult(JSON.parse(Buffer.concat(chunks).toString("utf8")));
        return;
      }
      const requestedPath = url.pathname === "/" ? "/index.html" : url.pathname;
      const path = resolve(browserOutput, `.${decodeURIComponent(requestedPath)}`);
      const metadata = await stat(path);
      assert.ok(path.startsWith(`${browserOutput}/`) && metadata.isFile());
      response.writeHead(200, {
        "content-type": contentType(extname(path)),
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

  const address = server.address();
  assert.ok(address && typeof address === "object");
  browser = spawn(
    chromium,
    [
      "--headless=new",
      "--no-sandbox",
      "--disable-dev-shm-usage",
      "--disable-gpu",
      `http://127.0.0.1:${address.port}/`,
    ],
    { stdio: "ignore" },
  );
  const browserExited = new Promise((_, rejectExit) => {
    browser.once("error", rejectExit);
    browser.once("exit", (code, signal) => {
      rejectExit(new Error(
        `Chromium exited before reporting a result: code=${code} signal=${signal}`,
      ));
    });
  });
  let browserTimeout;
  const result = await Promise.race([
    browserResult,
    browserExited,
    new Promise((_, rejectTimeout) => {
      browserTimeout = setTimeout(
        () => rejectTimeout(new Error(
          `framework adapter test timed out after ${browserTimeoutMs}ms`,
        )),
        browserTimeoutMs,
      );
    }),
  ]);
  clearTimeout(browserTimeout);

  assert.deepEqual(result, {
    ready: true,
    frameworks: [
      "qwik",
      "react",
      "solid",
      "svelte",
      "vue",
      "web-component",
    ],
    states: ["Running", "Running", "Running", "Running", "Running", "Running"],
    peakSubscriptions: 10,
    remainingSubscriptions: 0,
    deliveriesBeforeCleanup: 6,
    deliveriesAfterCleanup: 6,
  });
} finally {
  browser?.kill("SIGTERM");
  if (server !== undefined) {
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
  await rm(temporaryRoot, { recursive: true, force: true });
}

function contentType(extension) {
  if (extension === ".html") {
    return "text/html; charset=utf-8";
  }
  if (extension === ".js" || extension === ".mjs") {
    return "text/javascript; charset=utf-8";
  }
  return "application/octet-stream";
}
