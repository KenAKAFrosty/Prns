import { copyFile, mkdir } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const packageRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");

await mkdir(resolve(packageRoot, "dist-cjs"), { recursive: true });
await copyFile(
  resolve(packageRoot, "scripts/commonjs-package.json"),
  resolve(packageRoot, "dist-cjs/package.json"),
);
