import { svelte } from "@sveltejs/vite-plugin-svelte";
import { defineConfig } from "vite";
import { resolve } from "node:path";

const root = resolve(import.meta.dirname);

export default defineConfig({
  plugins: [svelte()],
  worker: {
    format: "es",
  },
  build: {
    target: "es2022",
    outDir: resolve(root, "dist"),
    emptyOutDir: false,
    lib: {
      entry: resolve(root, "svelte.ts"),
      formats: ["es"],
      fileName: () => "svelte.mjs",
    },
  },
});
