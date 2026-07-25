import { copyFile, mkdir } from "node:fs/promises";

const sdkDirectory = new URL(
  "../smoke/dist/prns-wasm/examples/browser-playground/sdk/",
  import.meta.url,
);
await mkdir(sdkDirectory, { recursive: true });
await copyFile(
  new URL("../smoke/dist/prns-js/src/casework.js", import.meta.url),
  new URL("index.js", sdkDirectory),
);
