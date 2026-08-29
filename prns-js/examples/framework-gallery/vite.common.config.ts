import { defineConfig } from "vite";
import { resolve } from "node:path";

const root = resolve(import.meta.dirname);

export default defineConfig({
  define: {
    "process.env.NODE_ENV": JSON.stringify("production"),
  },
  worker: {
    format: "es",
  },
  build: {
    target: "es2022",
    outDir: resolve(root, "dist"),
    emptyOutDir: false,
    lib: {
      entry: resolve(root, "common.ts"),
      formats: ["es"],
      fileName: () => "common.mjs",
    },
  },
});
