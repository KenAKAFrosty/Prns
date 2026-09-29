import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { test } from "node:test";

const packageRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const packageDocument = JSON.parse(
  readFileSync(resolve(packageRoot, "package.json"), "utf8"),
);
const require = createRequire(import.meta.url);
const esmContract = await import("personal-rns/contract");
const commonJsContract = require("personal-rns/contract");

test("contract subpath exposes one provider-neutral ESM and CommonJS API", () => {
  assert.deepEqual(packageDocument.exports["./contract"], {
    types: "./dist/contract.d.ts",
    import: "./dist/contract.js",
    require: "./dist-cjs/contract.js",
  });
  assert.deepEqual(
    Object.keys(commonJsContract).sort(),
    Object.keys(esmContract).sort(),
  );
  assert.equal(esmContract.HOST_CONTRACT_ABI, 1);
  assert.equal(commonJsContract.HOST_CONTRACT_ABI, 1);
  assert.equal("Prns" in esmContract, false);
  assert.equal("Prns" in commonJsContract, false);

  const source = new Uint8Array(esmContract.DESTINATION_HASH_LENGTH);
  const destination = esmContract.destinationHash(source);
  assert.deepEqual(destination, source);
  assert.notEqual(destination, source);
});

test("contract runtime modules have no provider dependency edge", () => {
  const runtimeGraph = new Map([
    ["dist/contract.js", ["./contract.generated.js"]],
    ["dist/contract.generated.js", []],
    ["dist-cjs/contract.js", ["./contract.generated.js"]],
    ["dist-cjs/contract.generated.js", []],
  ]);

  for (const [path, expected] of runtimeGraph) {
    const source = readFileSync(resolve(packageRoot, path), "utf8");
    assert.deepEqual(runtimeModuleReferences(source), expected, path);
  }
  assert.equal(existsSync(resolve(packageRoot, "native")), false);
  assert.equal(existsSync(resolve(packageRoot, "wasm")), false);
});

test("packed contract imports in a clean provider-free consumer", () => {
  const temporaryRoot = mkdtempSync(resolve(tmpdir(), "personal-rns-contract-"));
  try {
    const packed = JSON.parse(runNpm([
      "pack",
      "--json",
      "--ignore-scripts",
      "--pack-destination",
      temporaryRoot,
    ]));
    assert.equal(packed.length, 1);
    const packedFiles = new Set(packed[0].files.map(({ path }) => path));
    for (const path of [
      "dist/contract.d.ts",
      "dist/contract.js",
      "dist/contract.generated.d.ts",
      "dist/contract.generated.js",
      "dist-cjs/contract.generated.js",
      "dist-cjs/contract.js",
      "dist-cjs/package.json",
    ]) {
      assert.equal(packedFiles.has(path), true, path);
    }

    const consumerRoot = resolve(temporaryRoot, "consumer");
    mkdirSync(consumerRoot, { recursive: true });
    writeFileSync(
      resolve(consumerRoot, "package.json"),
      '{"private":true,"type":"module"}\n',
    );
    runNpm(
      [
        "install",
        "--audit=false",
        "--fund=false",
        "--ignore-scripts",
        "--no-save",
        "--offline",
        "--omit=optional",
        "--omit=peer",
        resolve(temporaryRoot, packed[0].filename),
        resolve(packageRoot, "node_modules", "casework"),
      ],
      consumerRoot,
    );
    const installedPackage = resolve(
      consumerRoot,
      "node_modules",
      "personal-rns",
    );
    assert.equal(existsSync(resolve(installedPackage, "native")), false);
    assert.equal(existsSync(resolve(installedPackage, "wasm")), false);

    const providerTrap = `
      process.dlopen = () => { throw new Error("native provider loaded"); };
      Object.defineProperty(globalThis, "Worker", {
        configurable: true,
        get() { throw new Error("browser provider loaded"); },
      });
    `;
    execFileSync(
      process.execPath,
      [
        "--input-type=module",
        "--eval",
        `${providerTrap}
          const contract = await import("personal-rns/contract");
          if (contract.HOST_CONTRACT_ABI !== 1 || "Prns" in contract) {
            throw new Error("unexpected contract export");
          }
        `,
      ],
      { cwd: consumerRoot, stdio: "pipe" },
    );
    execFileSync(
      process.execPath,
      [
        "--eval",
        `${providerTrap}
          const contract = require("personal-rns/contract");
          if (contract.HOST_CONTRACT_ABI !== 1 || "Prns" in contract) {
            throw new Error("unexpected contract export");
          }
        `,
      ],
      { cwd: consumerRoot, stdio: "pipe" },
    );

    writeFileSync(
      resolve(consumerRoot, "contract-consumer.ts"),
      `import { HOST_CONTRACT_ABI, destinationHash } from "personal-rns/contract";
import type { DestinationHash, HostCommand } from "personal-rns/contract";

const destination: DestinationHash = destinationHash(new Uint8Array(16));
declare const command: HostCommand;
void [HOST_CONTRACT_ABI, destination, command];
`,
    );
    writeFileSync(
      resolve(consumerRoot, "tsconfig.json"),
      `${JSON.stringify(
        {
          compilerOptions: {
            module: "NodeNext",
            moduleResolution: "NodeNext",
            noEmit: true,
            strict: true,
            target: "ES2022",
          },
          include: ["contract-consumer.ts"],
        },
        null,
        2,
      )}\n`,
    );
    execFileSync(
      process.execPath,
      [
        resolve(packageRoot, "node_modules", "typescript", "bin", "tsc"),
        "--project",
        resolve(consumerRoot, "tsconfig.json"),
      ],
      { cwd: consumerRoot, stdio: "pipe" },
    );
  } finally {
    rmSync(temporaryRoot, { force: true, recursive: true });
  }
});

function runtimeModuleReferences(source) {
  const references = [
    ...source.matchAll(
      /(?:\bfrom\s*|\bimport\s*)["']([^"']+)["']|\brequire\(["']([^"']+)["']\)|\bimport\(\s*["']([^"']+)["']\s*\)/g,
    ),
  ].map((match) => match[1] ?? match[2] ?? match[3]);
  return [...new Set(references)];
}

function runNpm(arguments_, cwd = packageRoot) {
  if (process.env.npm_execpath) {
    return execFileSync(
      process.execPath,
      [process.env.npm_execpath, ...arguments_],
      { cwd, encoding: "utf8" },
    );
  }
  return execFileSync(
    process.platform === "win32" ? "npm.cmd" : "npm",
    arguments_,
    { cwd, encoding: "utf8" },
  );
}
