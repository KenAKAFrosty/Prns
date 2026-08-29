import { createServer } from "node:http";
import { readFile, stat } from "node:fs/promises";
import { extname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { startNativeWebSocketFixture } from "../../scripts/native-websocket-fixture.mjs";

const LOOPBACK_HOST = "127.0.0.1";
const DEFAULT_HTTP_PORT = 4173;
const galleryRoot = resolve(import.meta.dirname, "dist");
const contentTypes = new Map([
  [".css", "text/css; charset=utf-8"],
  [".html", "text/html; charset=utf-8"],
  [".js", "text/javascript; charset=utf-8"],
  [".json", "application/json; charset=utf-8"],
  [".mjs", "text/javascript; charset=utf-8"],
  [".wasm", "application/wasm"],
]);

export async function startFrameworkGalleryServer(options = {}) {
  const host = options.host ?? LOOPBACK_HOST;
  const port = options.port ?? DEFAULT_HTTP_PORT;
  const fixture = await startNativeWebSocketFixture({ host });
  const server = createServer(async (request, response) => {
    try {
      const url = new URL(request.url ?? "/", `http://${host}`);
      if (request.method === "GET" && url.pathname === "/api/gallery-session") {
        response.writeHead(200, {
          "cache-control": "no-store",
          "content-type": "application/json; charset=utf-8",
        });
        response.end(JSON.stringify({
          destinationHex: fixture.destinationHex,
          webSocketUrl: fixture.webSocketUrl,
        }));
        return;
      }
      if (request.method !== "GET" && request.method !== "HEAD") {
        response.writeHead(405, { allow: "GET, HEAD" });
        response.end();
        return;
      }
      const relative = url.pathname === "/"
        ? "index.html"
        : decodeURIComponent(url.pathname).slice(1);
      const path = resolve(galleryRoot, relative);
      const metadata = await stat(path);
      if (!path.startsWith(`${galleryRoot}/`) || !metadata.isFile()) {
        response.writeHead(404);
        response.end();
        return;
      }
      response.writeHead(200, {
        "cache-control": "no-cache",
        "content-type": contentTypes.get(extname(path)) ?? "application/octet-stream",
      });
      if (request.method === "HEAD") {
        response.end();
        return;
      }
      response.end(await readFile(path));
    } catch {
      response.writeHead(404);
      response.end();
    }
  });
  try {
    await new Promise((resolveListen, rejectListen) => {
      server.once("error", rejectListen);
      server.listen(port, host, resolveListen);
    });
  } catch (error) {
    await fixture.stop();
    throw error;
  }
  const address = server.address();
  if (address === null || typeof address === "string") {
    server.close();
    await fixture.stop();
    throw new Error("framework gallery server did not expose a TCP address");
  }
  let closed = false;
  return {
    fixture,
    url: `http://${host}:${address.port}/`,
    async close() {
      if (closed) {
        return;
      }
      closed = true;
      server.closeAllConnections();
      await new Promise((resolveClose, rejectClose) => {
        server.close((error) => error ? rejectClose(error) : resolveClose());
      });
      await fixture.stop();
    },
  };
}

function cliOptions(arguments_) {
  let host = LOOPBACK_HOST;
  let port = DEFAULT_HTTP_PORT;
  for (let index = 0; index < arguments_.length; index += 1) {
    const argument = arguments_[index];
    if (argument === "--host") {
      host = requiredArgument(arguments_, ++index, "--host");
    } else if (argument === "--port") {
      port = Number.parseInt(requiredArgument(arguments_, ++index, "--port"), 10);
      if (!Number.isSafeInteger(port) || port < 0 || port > 65_535) {
        throw new Error("--port must be an integer from 0 through 65535");
      }
    } else {
      throw new Error(`unknown framework gallery option ${argument}`);
    }
  }
  return { host, port };
}

function requiredArgument(arguments_, index, option) {
  const value = arguments_[index];
  if (value === undefined || value.length === 0) {
    throw new Error(`${option} requires a value`);
  }
  return value;
}

if (fileURLToPath(import.meta.url) === process.argv[1]) {
  const running = await startFrameworkGalleryServer(cliOptions(process.argv.slice(2)));
  console.log(`Prns framework gallery: ${running.url}`);
  let stopping = false;
  const stop = async () => {
    if (stopping) {
      return;
    }
    stopping = true;
    await running.close();
  };
  process.once("SIGINT", () => void stop());
  process.once("SIGTERM", () => void stop());
}
