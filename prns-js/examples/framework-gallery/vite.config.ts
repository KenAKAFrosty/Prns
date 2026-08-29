import { defineConfig } from "vite";
import { resolve } from "node:path";

const root = resolve(import.meta.dirname);

export default defineConfig({
  root,
  publicDir: resolve(root, "../../wasm"),
  worker: {
    format: "es",
  },
  build: {
    outDir: resolve(root, "dist"),
    emptyOutDir: true,
  },
});
