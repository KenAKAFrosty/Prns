const { execFileSync } = require("node:child_process");
const { readFileSync } = require("node:fs");
const { resolve } = require("node:path");

const contracts = [
  { subpath: "personal-rns/contract", source: "contract.generated.ts", name: "HOST_SEMANTIC_FINGERPRINT" },
  { subpath: "personal-rns/remote-control", source: "remote-control.generated.ts", name: "REMOTE_CONTROL_SEMANTIC_FINGERPRINT" },
];
const repair = "Run npm --prefix prns-js run build:code before bundling.";

function checkBuiltContracts(packageRoot = resolve(__dirname, "..")) {
  const expected = contracts.map(({ source, name }) => {
    const text = readFileSync(resolve(packageRoot, "src", source), "utf8");
    const matches = [...text.matchAll(new RegExp(`\\b${name}\\s*=\\s*["']([0-9a-f]{64})["']`, "g"))];
    if (matches.length !== 1) throw new Error(`Invalid generated ${name} in ${source}`);
    return matches[0][1];
  });
  let actual;
  try {
    // Resolve public ESM exports in a fresh process, independent of Metro's module cache.
    // These provider-neutral modules must not load the application or native runtime.
    actual = JSON.parse(execFileSync(process.execPath, ["--input-type=module", "--eval", `
      const contracts = ${JSON.stringify(contracts)};
      const values = await Promise.all(contracts.map(async ({ subpath, name }) => (await import(subpath))[name]));
      process.stdout.write(JSON.stringify(values));
    `], { cwd: packageRoot, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] }));
  } catch (error) {
    throw new Error(`Cannot load personal-rns contract exports. ${repair}`, { cause: error });
  }
  for (const [index, { subpath, name }] of contracts.entries()) {
    if (actual[index] !== expected[index]) {
      throw new Error(`Stale ${subpath} export ${name}: expected ${expected[index]}, found ${actual[index]}. ${repair}`);
    }
  }
  return Object.fromEntries(contracts.map(({ name }, index) => [name, actual[index]]));
}

module.exports = { checkBuiltContracts };
if (require.main === module) console.log(JSON.stringify(checkBuiltContracts(), null, 2));
