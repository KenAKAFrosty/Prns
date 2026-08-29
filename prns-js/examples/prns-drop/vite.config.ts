import { resolve } from "node:path";
import { defineConfig } from "vite";
import solid from "vite-plugin-solid";

const root = resolve(import.meta.dirname);

export default defineConfig({
  root,
  publicDir: resolve(root, "../../wasm"),
  plugins: [solid()],
  worker: {
    format: "es",
  },
  build: {
    outDir: resolve(root, "dist"),
    emptyOutDir: true,
    target: "es2022",
  },
});
