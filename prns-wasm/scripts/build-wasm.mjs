import { execFileSync } from "node:child_process";
import { isAbsolute, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { parseArgs } from "node:util";

const packageRoot = fileURLToPath(new URL("../", import.meta.url));
const target = "wasm32-unknown-unknown";

export function buildWasm(args = [], { run = execFileSync } = {}) {
  const { values } = parseArgs({
    args,
    options: {
      release: { type: "boolean", default: false },
      "out-dir": { type: "string", default: "smoke/pkg" },
      features: { type: "string" },
    },
  });
  if (!values["out-dir"] || values.features === "") {
    throw new Error("--out-dir and --features must not be empty");
  }

  // Ask Cargo rather than interpreting CARGO_TARGET_DIR or Cargo configuration
  // ourselves. Keep all commands in this package, even when invoked elsewhere.
  const metadata = JSON.parse(
    run("cargo", ["metadata", "--locked", "--format-version", "1", "--no-deps"], {
      cwd: packageRoot,
      encoding: "utf8",
      stdio: ["inherit", "pipe", "inherit"],
    }),
  );
  if (typeof metadata?.target_directory !== "string" || !isAbsolute(metadata.target_directory)) {
    throw new Error("cargo metadata did not return an absolute target_directory");
  }

  const buildArgs = ["build", "--locked", "--target", target];
  if (values.release) buildArgs.push("--release");
  if (values.features) buildArgs.push("--features", values.features);
  run("cargo", buildArgs, { cwd: packageRoot, stdio: "inherit" });
  run("wasm-bindgen", [
    join(metadata.target_directory, target, values.release ? "release" : "debug", "prns_wasm.wasm"),
    "--target", "web", "--out-dir", values["out-dir"],
  ], { cwd: packageRoot, stdio: "inherit" });
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  try {
    buildWasm(process.argv.slice(2));
  } catch (error) {
    console.error(error.message);
    process.exitCode = Number.isInteger(error.status) && error.status > 0 ? error.status : 1;
  }
}
