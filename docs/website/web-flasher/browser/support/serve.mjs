import { createReadStream } from "node:fs";
import { stat } from "node:fs/promises";
import http from "node:http";
import path from "node:path";
import { fileURLToPath } from "node:url";

const supportDirectory = path.dirname(fileURLToPath(import.meta.url));
const websiteRoot = path.resolve(supportDirectory, "../../..");
const siteRoot = path.resolve(
  process.env.PRNS_BROWSER_TEST_SITE ?? path.join(websiteRoot, "target/browser-tests/site"),
);
const allowedRoot = path.join(websiteRoot, "target/browser-tests");
const host = "127.0.0.1";
const port = Number(process.env.PRNS_BROWSER_TEST_PORT ?? "4173");

if (!isWithin(allowedRoot, siteRoot)) {
  throw new Error(`browser test server root must remain under ${allowedRoot}`);
}
if (!Number.isInteger(port) || port < 1024 || port > 65535) {
  throw new Error("PRNS_BROWSER_TEST_PORT must be a non-privileged TCP port");
}

const server = http.createServer(async (request, response) => {
  try {
    const url = new URL(request.url ?? "/", `http://${host}:${port}`);
    const decodedPath = decodeURIComponent(url.pathname);
    if (decodedPath.includes("\0") || decodedPath.includes("\\")) {
      return send(response, 400, "invalid path\n", "text/plain; charset=utf-8");
    }
    const candidate = path.resolve(siteRoot, `.${decodedPath}`);
    if (!isWithin(siteRoot, candidate)) {
      return send(response, 403, "forbidden\n", "text/plain; charset=utf-8");
    }

    let file = candidate;
    let metadata = await fileStat(file);
    if (metadata?.isDirectory()) {
      file = path.join(file, "index.html");
      metadata = await fileStat(file);
    }
    if (!metadata?.isFile()) {
      if (path.extname(decodedPath)) {
        return send(response, 404, "not found\n", "text/plain; charset=utf-8");
      }
      file = path.join(siteRoot, "index.html");
      metadata = await fileStat(file);
    }
    if (!metadata?.isFile()) {
      return send(response, 404, "not found\n", "text/plain; charset=utf-8");
    }

    response.writeHead(200, {
      "cache-control": "no-store",
      "content-length": metadata.size,
      "content-type": contentType(file),
      "x-content-type-options": "nosniff",
    });
    if (request.method === "HEAD") {
      response.end();
      return;
    }
    createReadStream(file).pipe(response);
  } catch (error) {
    send(response, 500, "test server failure\n", "text/plain; charset=utf-8");
    console.error(error);
  }
});

server.listen(port, host, () => {
  console.log(`PRNS browser fixture available at http://${host}:${port}`);
});

for (const signal of ["SIGINT", "SIGTERM"]) {
  process.on(signal, () => server.close(() => process.exit(0)));
}

async function fileStat(file) {
  try {
    return await stat(file);
  } catch (error) {
    if (error?.code === "ENOENT" || error?.code === "ENOTDIR") return null;
    throw error;
  }
}

function send(response, status, body, type) {
  response.writeHead(status, {
    "cache-control": "no-store",
    "content-type": type,
    "x-content-type-options": "nosniff",
  });
  response.end(body);
}

function isWithin(root, candidate) {
  const relative = path.relative(root, candidate);
  return relative === "" || (!relative.startsWith("..") && !path.isAbsolute(relative));
}

function contentType(file) {
  return (
    {
      ".bin": "application/octet-stream",
      ".css": "text/css; charset=utf-8",
      ".html": "text/html; charset=utf-8",
      ".js": "text/javascript; charset=utf-8",
      ".json": "application/json; charset=utf-8",
      ".map": "application/json; charset=utf-8",
      ".minisig": "text/plain; charset=utf-8",
      ".png": "image/png",
      ".svg": "image/svg+xml",
      ".uf2": "application/octet-stream",
      ".wasm": "application/wasm",
      ".zip": "application/zip",
    }[path.extname(file).toLowerCase()] ?? "application/octet-stream"
  );
}
