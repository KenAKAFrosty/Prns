import { copyFile, mkdir } from "node:fs/promises";
import "../../prns-js/scripts/stage-code.mjs";

const generatedCasework = new URL(
  "../../prns-js/dist/casework.js",
  import.meta.url,
);
const smokeCasework = new URL(
  "../smoke/dist/prns-js/src/casework.js",
  import.meta.url,
);
const sdkDirectory = new URL(
  "../smoke/dist/prns-wasm/examples/browser-playground/sdk/",
  import.meta.url,
);
await mkdir(sdkDirectory, { recursive: true });
await Promise.all([
  copyFile(generatedCasework, smokeCasework),
  copyFile(generatedCasework, new URL("index.js", sdkDirectory)),
]);
