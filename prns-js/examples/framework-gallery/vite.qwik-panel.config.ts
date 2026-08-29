import { qwikVite } from "@builder.io/qwik/optimizer";
import { defineConfig } from "vite";
import { resolve } from "node:path";

const root = resolve(import.meta.dirname);

export default defineConfig({
  plugins: [qwikVite({ srcDir: root })],
  build: {
    target: "es2022",
    outDir: resolve(root, "dist-qwik"),
    emptyOutDir: true,
    lib: {
      entry: resolve(root, "QwikPanel.tsx"),
      formats: ["es"],
      fileName: () => "QwikPanel.qwik.mjs",
    },
    rollupOptions: {
      external: [
        "@builder.io/qwik",
        "@builder.io/qwik/jsx-runtime",
        "personal-rns/browser",
      ],
    },
  },
});
