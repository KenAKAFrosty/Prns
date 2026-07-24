import { copyFile, mkdir } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const packageRoot = resolve(root, "prns-js");

await mkdir(resolve(packageRoot, "native"), { recursive: true });
await mkdir(resolve(packageRoot, "wasm"), { recursive: true });
await mkdir(resolve(packageRoot, "dist-cjs"), { recursive: true });
await copyFile(resolve(root, "prns-napi/index.js"), resolve(packageRoot, "native/addon.cjs"));
await copyFile(resolve(root, "LICENSE-MIT"), resolve(packageRoot, "LICENSE-MIT"));
await copyFile(resolve(root, "LICENSE-APACHE"), resolve(packageRoot, "LICENSE-APACHE"));
await Promise.all(
  [
    "prns_wasm.js",
    "prns_wasm.d.ts",
    "prns_wasm_bg.wasm",
    "prns_wasm_bg.wasm.d.ts",
  ].map((file) =>
    copyFile(
      resolve(root, "prns-wasm/smoke/pkg", file),
      resolve(packageRoot, "wasm", file),
    ),
  ),
);
await copyFile(
  resolve(packageRoot, "scripts/commonjs-package.json"),
  resolve(packageRoot, "dist-cjs/package.json"),
);
