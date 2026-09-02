import { spawnSync } from "node:child_process";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const packageRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const applicationsRoot = resolve(packageRoot, "../..");
const outputPath = resolve(packageRoot, "src/contract.generated.ts");
const result = spawnSync(
  "cargo",
  [
    "run",
    "--quiet",
    "--locked",
    "--manifest-path",
    resolve(applicationsRoot, "Cargo.toml"),
    "-p",
    "prns-app-native",
    "--bin",
    "export_contract",
  ],
  { encoding: "utf8" },
);

if (result.status !== 0) {
  process.stderr.write(result.stderr);
  process.exit(result.status ?? 1);
}

const generated = result.stdout;
if (process.argv.includes("--check")) {
  let current;
  try {
    current = readFileSync(outputPath, "utf8");
  } catch {
    process.stderr.write(`${outputPath} is missing; run npm run api:generate.\n`);
    process.exit(1);
  }
  if (current !== generated) {
    process.stderr.write(
      `${outputPath} differs from the Rust contract; run npm run api:generate.\n`,
    );
    process.exit(1);
  }
} else {
  mkdirSync(dirname(outputPath), { recursive: true });
  writeFileSync(outputPath, generated);
}
