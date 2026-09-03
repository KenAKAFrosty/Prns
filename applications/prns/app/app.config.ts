import type { ConfigContext, ExpoConfig } from "expo/config";

const variants = {
  development: {
    name: "prns dev",
    slug: "prns-dev",
    identifier: "rs.reticulum.prns.dev",
  },
  production: {
    name: "prns",
    slug: "prns",
    identifier: "rs.reticulum.prns",
  },
} as const;

type AppVariant = keyof typeof variants;

function selectedVariant(value: string | undefined): AppVariant {
  if (value === undefined || value === "development") {
    return "development";
  }
  if (value === "production") {
    return value;
  }
  throw new Error(
    `PRNS_APP_VARIANT must be development or production, received ${JSON.stringify(value)}`,
  );
}

export default ({ config }: ConfigContext): ExpoConfig => {
  const selection = variants[selectedVariant(process.env.PRNS_APP_VARIANT)];
  const bluetoothRestorationPrefix = `${selection.identifier}.bluetooth-auto`;

  return {
    ...config,
    name: selection.name,
    slug: selection.slug,
    version: "0.0.0",
    orientation: "default",
    userInterfaceStyle: "automatic",
    plugins: ["expo-router"],
    ios: {
      ...config.ios,
      bundleIdentifier: selection.identifier,
      infoPlist: {
        ...config.ios?.infoPlist,
        NSBluetoothAlwaysUsageDescription:
          "prns uses Bluetooth to connect to nearby Reticulum nodes.",
        NSLocalNetworkUsageDescription:
          "prns uses the local network for an explicitly configured development LXMF peer.",
        PRNSCoreBluetoothCentralRestorationIdentifier: `${bluetoothRestorationPrefix}.central.v1`,
        PRNSCoreBluetoothPeripheralRestorationIdentifier: `${bluetoothRestorationPrefix}.peripheral.v1`,
        UIBackgroundModes: ["bluetooth-central", "bluetooth-peripheral"],
      },
      supportsTablet: true,
    },
    android: {
      ...config.android,
      package: selection.identifier,
    },
    web: {
      ...config.web,
      bundler: "metro",
    },
  };
};
