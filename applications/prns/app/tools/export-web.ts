import { spawnSync } from "node:child_process";
import { rmSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const appRoot = resolve(fileURLToPath(new URL("..", import.meta.url)));
const output = resolve(appRoot, "dist");

if (dirname(output) !== appRoot) {
  throw new Error(`export:web: refusing to remove unexpected output path ${output}`);
}
rmSync(output, { recursive: true, force: true });

const result = spawnSync("expo", ["export", "--platform", "web", "--output-dir", "dist"], {
  cwd: appRoot,
  encoding: "utf8",
  stdio: "inherit",
});
if (result.error !== undefined) {
  throw result.error;
}
if (result.status !== 0) {
  throw new Error(`export:web: Expo exited with status ${String(result.status)}`);
}
