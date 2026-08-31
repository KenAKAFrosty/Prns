import { qwikVite } from "@builder.io/qwik/optimizer";
import { defineConfig } from "vite";

export default defineConfig({
  plugins: [qwikVite({
    srcDir: "src/browser/adapters",
    client: { input: "src/browser/adapters/qwik.tsx" },
  })],
  build: {
    target: "es2022",
    emptyOutDir: false,
    outDir: "dist/qwik",
    lib: {
      entry: "src/browser/adapters/qwik.tsx",
      formats: ["es"],
      fileName: () => "index.qwik.mjs",
    },
    rollupOptions: {
      external: ["@builder.io/qwik", "personal-rns/browser"],
    },
  },
});
