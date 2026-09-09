import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

type JsonRecord = Readonly<Record<string, unknown>>;

const appRoot = fileURLToPath(new URL("..", import.meta.url));
const workspaceRoot = fileURLToPath(new URL("../../..", import.meta.url));
const localPersonalRnsSelection = "file:../../../prns-js";

const expectedDependencies = {
  "@expo/metro-runtime": "~57.0.15",
  "@prns-internal/expo": "*",
  "@prns-internal/native-bindings": "*",
  "@ubjs/core": "file:../../vendor/ubrn/packages/ubjs-core-0.31.0-5.tgz",
  "@ubjs/react-native": "file:../../vendor/ubrn/packages/ubjs-react-native-0.31.0-5.tgz",
  "@react-native-async-storage/async-storage": "2.2.0",
  expo: "~57.0.21",
  "expo-constants": "~57.0.17",
  "expo-document-picker": "~57.0.1",
  "expo-file-system": "~57.0.6",
  "expo-linking": "~57.0.9",
  "expo-router": "~57.0.20",
  "expo-status-bar": "~57.0.1",
  effect: "4.0.0-rc.112",
  "personal-rns": localPersonalRnsSelection,
  react: "19.2.3",
  "react-dom": "19.2.3",
  "react-native": "0.86.3",
  "react-native-safe-area-context": "~5.7.0",
  "react-native-screens": "~4.26.0",
  "react-native-web": "~0.21.0",
} as const;

const expectedDevelopmentDependencies = {
  "@biomejs/biome": "2.5.11",
  "@react-native/jest-preset": "0.86.3",
  "@testing-library/react-native": "13.3.3",
  "@types/jest": "29.5.14",
  "@types/node": "24.13.3",
  "@types/react": "~19.2.2",
  "@types/react-test-renderer": "19.1.0",
  "expo-doctor": "1.20.4",
  jest: "29.7.0",
  "jest-expo": "57.0.5",
  "react-test-renderer": "19.2.3",
  typescript: "7.0.2",
} as const;

function fail(message: string): never {
  throw new Error(`versions:check: ${message}`);
}

function isRecord(value: unknown): value is JsonRecord {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function parseJsonFile(path: string): JsonRecord {
  const parsed: unknown = JSON.parse(readFileSync(path, "utf8"));
  if (!isRecord(parsed)) {
    return fail(`${path} must contain a JSON object`);
  }
  return parsed;
}

function stringRecord(value: unknown, owner: string): Readonly<Record<string, string>> {
  if (!isRecord(value) || Object.values(value).some((entry) => typeof entry !== "string")) {
    return fail(`${owner} must be an object whose values are strings`);
  }
  const result: Record<string, string> = {};
  for (const [name, entry] of Object.entries(value)) {
    if (typeof entry !== "string") {
      return fail(`${owner}.${name} must be a string`);
    }
    result[name] = entry;
  }
  return result;
}

function personalRnsSelection(appPackage: JsonRecord): string {
  const dependencies = stringRecord(appPackage.dependencies, "dependencies");
  const selection = dependencies["personal-rns"];
  if (selection === localPersonalRnsSelection) {
    return selection;
  }

  const compatibility = parseJsonFile(`${workspaceRoot}/release/compatibility.json`);
  const prns = compatibility.prns;
  if (!isRecord(prns) || !isRecord(prns.javascriptContract)) {
    return fail("release/compatibility.json must declare prns.javascriptContract");
  }
  const packageName = prns.javascriptContract.package;
  const packageVersion = prns.javascriptContract.version;
  const expectedSha256 = prns.javascriptContract.artifactSha256;
  if (
    typeof packageName !== "string" ||
    typeof packageVersion !== "string" ||
    typeof expectedSha256 !== "string" ||
    !/^[0-9a-f]{64}$/u.test(expectedSha256)
  ) {
    return fail("release/compatibility.json has an invalid JavaScript artifact record");
  }
  const detachedSelection = `file:../../vendor/${packageName}-${packageVersion}.tgz`;
  if (selection !== detachedSelection) {
    return fail(
      `dependencies.personal-rns must be ${localPersonalRnsSelection} or the recorded ` +
        `detached artifact ${detachedSelection}, received ${String(selection)}`,
    );
  }
  const artifact = resolve(appRoot, selection.slice("file:".length));
  const actualSha256 = createHash("sha256").update(readFileSync(artifact)).digest("hex");
  if (actualSha256 !== expectedSha256) {
    return fail(
      `detached personal-rns artifact has SHA-256 ${actualSha256}, expected ${expectedSha256}`,
    );
  }
  return selection;
}

function assertSelections(
  owner: string,
  actual: Readonly<Record<string, string>>,
  expected: Readonly<Record<string, string>>,
): void {
  for (const [name, selection] of Object.entries(expected)) {
    if (actual[name] !== selection) {
      fail(`${owner}.${name} must be ${selection}, received ${String(actual[name])}`);
    }
  }
  const unexpected = Object.keys(actual).filter((name) => !(name in expected));
  if (unexpected.length > 0) {
    fail(`${owner} contains unreviewed direct packages: ${unexpected.join(", ")}`);
  }
}

function parseVersion(value: string, owner: string): readonly [number, number, number] {
  const match = /^(\d+)\.(\d+)\.(\d+)/u.exec(value);
  if (match === null) {
    return fail(`${owner} emitted an invalid version: ${value}`);
  }
  return [Number(match[1]), Number(match[2]), Number(match[3])];
}

function nodeVersionIsSupported(value: string): boolean {
  const [major, minor] = parseVersion(value, "node");
  return (major === 22 && minor >= 13) || (major === 24 && minor >= 3) || major >= 25;
}

function collectVersions(node: JsonRecord, packageName: string, versions: Set<string>): void {
  const dependencies = node.dependencies;
  if (!isRecord(dependencies)) {
    return;
  }
  for (const [name, child] of Object.entries(dependencies)) {
    if (!isRecord(child)) {
      continue;
    }
    if (name === packageName && typeof child.version === "string") {
      versions.add(child.version);
    }
    collectVersions(child, packageName, versions);
  }
}

const appPackage = parseJsonFile(`${appRoot}/package.json`);
const workspacePackage = parseJsonFile(`${workspaceRoot}/package.json`);
const selectedPersonalRns = personalRnsSelection(appPackage);

assertSelections("dependencies", stringRecord(appPackage.dependencies, "dependencies"), {
  ...expectedDependencies,
  "personal-rns": selectedPersonalRns,
});
assertSelections(
  "devDependencies",
  stringRecord(appPackage.devDependencies, "devDependencies"),
  expectedDevelopmentDependencies,
);

if (workspacePackage.packageManager !== "npm@11.16.0") {
  fail(`packageManager must be npm@11.16.0, received ${String(workspacePackage.packageManager)}`);
}
const engines = stringRecord(workspacePackage.engines, "applications.engines");
if (engines.node !== "^22.13.0 || ^24.3.0 || >=25.0.0") {
  fail(`applications.engines.node has an unexpected range: ${String(engines.node)}`);
}
assertSelections(
  "applications.overrides",
  stringRecord(workspacePackage.overrides, "applications.overrides"),
  { react: "19.2.3", "react-dom": "19.2.3" },
);
if (!nodeVersionIsSupported(process.versions.node)) {
  fail(
    `Node ${process.versions.node} is outside the supported Expo/RN range ` +
      "(^22.13.0 || ^24.3.0 || >=25.0.0)",
  );
}

const npmVersion = execFileSync("npm", ["--version"], { encoding: "utf8" }).trim();
if (npmVersion !== "11.16.0") {
  fail(`npm must be 11.16.0, received ${npmVersion}`);
}

const typeScriptVersion = execFileSync("tsc", ["--version"], { encoding: "utf8" }).trim();
if (typeScriptVersion !== "Version 7.0.2") {
  fail(`tsc must be Version 7.0.2, received ${typeScriptVersion}`);
}

const npmTreeText = execFileSync(
  "npm",
  ["ls", "react", "react-dom", "--all", "--json", "--package-lock-only"],
  {
    cwd: workspaceRoot,
    encoding: "utf8",
  },
);
const npmTree: unknown = JSON.parse(npmTreeText);
if (!isRecord(npmTree)) {
  fail("npm ls react react-dom returned a non-object graph");
}
const reactVersions = new Set<string>();
collectVersions(npmTree, "react", reactVersions);
if (reactVersions.size !== 1 || !reactVersions.has("19.2.3")) {
  fail(`the install must resolve only React 19.2.3, received ${[...reactVersions].join(", ")}`);
}
const reactDomVersions = new Set<string>();
collectVersions(npmTree, "react-dom", reactDomVersions);
if (reactDomVersions.size !== 1 || !reactDomVersions.has("19.2.3")) {
  fail(
    `the install must resolve only React DOM 19.2.3, received ${[...reactDomVersions].join(", ")}`,
  );
}

console.log(
  `versions:check: Node ${process.versions.node}, npm ${npmVersion}, TypeScript 7.0.2, React 19.2.3`,
);
