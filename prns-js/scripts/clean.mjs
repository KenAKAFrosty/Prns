import { rm } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const packageRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");

await Promise.all(
  ["dist", "dist-cjs", "native", "wasm", "LICENSE-MIT", "LICENSE-APACHE"].map(
    (path) => rm(resolve(packageRoot, path), { force: true, recursive: true }),
  ),
);
