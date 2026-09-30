import { execFileSync, spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

type JsonRecord = Readonly<Record<string, unknown>>;

const appRoot = fileURLToPath(new URL("..", import.meta.url));
const bluetoothUsageDescription = "prns uses Bluetooth to connect to nearby Reticulum nodes.";
const localNetworkUsageDescription =
  "prns uses the local network for an explicitly configured development LXMF peer.";

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

function render(
  variant: "development" | "production",
  environment: Readonly<Record<string, string>> = {},
): JsonRecord {
  const output = execFileSync("expo", ["config", "--type", "public", "--json"], {
    cwd: appRoot,
    encoding: "utf8",
    env: { ...process.env, ...environment, PRNS_APP_VARIANT: variant },
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
    ["scheme", config.scheme, expected.identifier],
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
  if (
    infoPlist.PRNSCoreBluetoothPeripheralRestorationIdentifier !==
    `${expected.identifier}.bluetooth-auto.peripheral.v1`
  ) {
    fail(`${variant}.ios.infoPlist must contain its exact peripheral restoration identifier`);
  }
  const scenePlugins = Array.isArray(config.plugins)
    ? config.plugins.filter(
        (plugin) => Array.isArray(plugin) && plugin[0] === "expo-build-properties",
      )
    : [];
  const scenePlugin = scenePlugins[0];
  if (
    scenePlugins.length !== 1 ||
    !Array.isArray(scenePlugin) ||
    !isRecord(scenePlugin[1]) ||
    !isRecord(scenePlugin[1].ios) ||
    scenePlugin[1].ios.enableSceneSupport !== true
  ) {
    fail(`${variant} must enable Expo's scene lifecycle support`);
  }
  if (Object.keys(infoPlist).some((key) => key.startsWith("NSAccessorySetup"))) {
    fail(
      `${variant}.ios.infoPlist must use ordinary Bluetooth authorization, not accessory-only access`,
    );
  }
  const backgroundModes = infoPlist.UIBackgroundModes;
  if (
    !Array.isArray(backgroundModes) ||
    backgroundModes.length !== 2 ||
    !backgroundModes.includes("bluetooth-central") ||
    !backgroundModes.includes("bluetooth-peripheral")
  ) {
    fail(`${variant}.ios.infoPlist must declare central and peripheral Bluetooth background modes`);
  }
  if (android.allowBackup !== false) {
    fail(`${variant}.android must disable backups of disposable development state`);
  }
  if (!Array.isArray(config.plugins) || !config.plugins.includes("./tools/with-android-runtime")) {
    fail(`${variant} must apply the Android API 29 runtime plugin`);
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

for (const variant of ["development", "production"] as const) {
  const fixture = "127.0.0.1:4242";
  const fixtureConfig = render(variant, { EXPO_PUBLIC_PRNS_LXMF_TCP_TARGET: fixture });
  const ios = readRecord(fixtureConfig.ios, `${variant}.ios`);
  const info = readRecord(ios.infoPlist, `${variant}.ios.infoPlist`);
  if (info.PRNSDevelopmentTcpTarget !== (variant === "development" ? fixture : undefined)) {
    fail(`${variant} must include the native TCP fixture only in the development variant`);
  }
}

const noFixture = render("development", { EXPO_PUBLIC_PRNS_LXMF_TCP_TARGET: "" });
if (
  readRecord(readRecord(noFixture.ios, "development.ios").infoPlist, "development.ios.infoPlist")
    .PRNSDevelopmentTcpTarget !== undefined
) {
  fail("an unset native TCP fixture must remain absent");
}

const invalid = spawnSync("expo", ["config", "--type", "public", "--json"], {
  cwd: appRoot,
  encoding: "utf8",
  env: { ...process.env, PRNS_APP_VARIANT: "preview" },
});
if (invalid.status === 0) {
  fail("PRNS_APP_VARIANT=preview must be rejected");
}

console.log(
  "config:check: variant coordinates, dual-role Bluetooth restoration, and Android runtime policy are exact",
);
