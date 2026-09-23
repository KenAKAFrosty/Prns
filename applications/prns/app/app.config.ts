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
  const variant = selectedVariant(process.env.PRNS_APP_VARIANT);
  const selection = variants[variant];
  const developmentTcpTarget =
    variant === "development" ? process.env.EXPO_PUBLIC_PRNS_LXMF_TCP_TARGET?.trim() : undefined;
  const bluetoothRestorationPrefix = `${selection.identifier}.bluetooth-auto`;

  return {
    ...config,
    name: selection.name,
    slug: selection.slug,
    scheme: selection.identifier,
    version: "0.0.0",
    runtimeVersion: { policy: "fingerprint" },
    orientation: "default",
    userInterfaceStyle: "automatic",
    plugins: [
      "expo-router",
      ["expo-build-properties", { ios: { enableSceneSupport: true } }],
      "./tools/with-ios-18",
      "./tools/with-android-runtime",
    ],
    ios: {
      ...config.ios,
      bundleIdentifier: selection.identifier,
      infoPlist: {
        ...config.ios?.infoPlist,
        ...(developmentTcpTarget ? { PRNSDevelopmentTcpTarget: developmentTcpTarget } : {}),
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
      allowBackup: false,
    },
    web: {
      ...config.web,
      bundler: "metro",
    },
  };
};
