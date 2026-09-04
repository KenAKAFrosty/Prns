import { execFileSync, spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

type JsonRecord = Readonly<Record<string, unknown>>;

const appRoot = fileURLToPath(new URL("..", import.meta.url));
const bluetoothUsageDescription = "prns uses Bluetooth to connect to nearby Reticulum nodes.";
const localNetworkUsageDescription =
  "prns uses the local network for an explicitly configured development LXMF peer.";
const prnsBluetoothServiceUuid = "37145B00-442D-4A94-917F-8F42C5DA28E3";

function fail(message: string): never {
  throw new Error(`config:check: ${message}`);
}

function isRecord(value: unknown): value is JsonRecord {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function readRecord(value: unknown, owner: string): JsonRecord {
  if (!isRecord(value)) {
    return fail(`${owner} must be an object`);
  }
  return value;
}

function render(variant: "development" | "production"): JsonRecord {
  const output = execFileSync("expo", ["config", "--type", "public", "--json"], {
    cwd: appRoot,
    encoding: "utf8",
    env: { ...process.env, PRNS_APP_VARIANT: variant },
  });
  const parsed: unknown = JSON.parse(output);
  return readRecord(parsed, `${variant} Expo config`);
}

function assertVariant(
  variant: "development" | "production",
  expected: { readonly name: string; readonly slug: string; readonly identifier: string },
): void {
  const config = render(variant);
  const ios = readRecord(config.ios, `${variant}.ios`);
  const infoPlist = readRecord(ios.infoPlist, `${variant}.ios.infoPlist`);
  const android = readRecord(config.android, `${variant}.android`);
  const web = readRecord(config.web, `${variant}.web`);

  const checks: ReadonlyArray<readonly [string, unknown, string]> = [
    ["name", config.name, expected.name],
    ["slug", config.slug, expected.slug],
    ["ios.bundleIdentifier", ios.bundleIdentifier, expected.identifier],
    ["android.package", android.package, expected.identifier],
    ["web.bundler", web.bundler, "metro"],
  ];
  for (const [field, actual, wanted] of checks) {
    if (actual !== wanted) {
      fail(`${variant}.${field} must be ${wanted}, received ${String(actual)}`);
    }
  }
  if (infoPlist.NSBluetoothAlwaysUsageDescription !== bluetoothUsageDescription) {
    fail(
      `${variant}.ios.infoPlist.NSBluetoothAlwaysUsageDescription must be the tracked product copy`,
    );
  }
  if (infoPlist.NSLocalNetworkUsageDescription !== localNetworkUsageDescription) {
    fail(
      `${variant}.ios.infoPlist.NSLocalNetworkUsageDescription must be the tracked development fixture copy`,
    );
  }
  const expectedCentral = `${expected.identifier}.bluetooth-auto.central.v1`;
  if (infoPlist.PRNSCoreBluetoothCentralRestorationIdentifier !== expectedCentral) {
    fail(`${variant}.ios.infoPlist must contain its exact central restoration identifier`);
  }
  if ("PRNSCoreBluetoothPeripheralRestorationIdentifier" in infoPlist) {
    fail(`${variant}.ios.infoPlist must not declare a peripheral restoration identifier`);
  }
  if ("UIApplicationSceneManifest" in infoPlist) {
    fail(`${variant}.ios.infoPlist must not declare a scene manifest`);
  }
  if (
    !Array.isArray(infoPlist.NSAccessorySetupBluetoothServices) ||
    infoPlist.NSAccessorySetupBluetoothServices.length !== 1 ||
    infoPlist.NSAccessorySetupBluetoothServices[0] !== prnsBluetoothServiceUuid
  ) {
    fail(`${variant}.ios.infoPlist must declare the exact ASK Bluetooth service UUID`);
  }
  if (
    !Array.isArray(infoPlist.NSAccessorySetupKitSupports) ||
    infoPlist.NSAccessorySetupKitSupports.length !== 1 ||
    infoPlist.NSAccessorySetupKitSupports[0] !== "Bluetooth"
  ) {
    fail(`${variant}.ios.infoPlist must declare ASK Bluetooth support`);
  }
  const backgroundModes = infoPlist.UIBackgroundModes;
  if (
    !Array.isArray(backgroundModes) ||
    backgroundModes.length !== 1 ||
    backgroundModes[0] !== "bluetooth-central"
  ) {
    fail(`${variant}.ios.infoPlist must declare only the central Bluetooth background mode`);
  }
}

assertVariant("development", {
  name: "prns dev",
  slug: "prns-dev",
  identifier: "rs.reticulum.prns.dev",
});
assertVariant("production", {
  name: "prns",
  slug: "prns",
  identifier: "rs.reticulum.prns",
});

const invalid = spawnSync("expo", ["config", "--type", "public", "--json"], {
  cwd: appRoot,
  encoding: "utf8",
  env: { ...process.env, PRNS_APP_VARIANT: "preview" },
});
if (invalid.status === 0) {
  fail("PRNS_APP_VARIANT=preview must be rejected");
}

console.log("config:check: coordinates and variant-isolated ASK central declarations are exact");
