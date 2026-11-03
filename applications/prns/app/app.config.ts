import type { ConfigContext, ExpoConfig } from "expo/config";

const variants = {
  development: { name: "PRNS Chat Dev", slug: "prns-dev", android: "rs.reticulum.prns.dev", ios: "rs.reticulum.prns.dev" },
  production: { name: "PRNS Chat", slug: "prns", android: "rs.reticulum.prns", ios: "rs.reticulum.prns.chat" },
} as const;

export default ({ config }: ConfigContext): ExpoConfig => {
  const variant = process.env.PRNS_APP_VARIANT ?? "development";
  if (variant !== "development" && variant !== "production") {
    throw new Error("PRNS_APP_VARIANT must be development or production");
  }
  const selection = variants[variant];
  const buildNumber = process.env.PRNS_IOS_BUILD_NUMBER?.trim() ?? "1";
  if (!/^[1-9][0-9]*$/.test(buildNumber) || !Number.isSafeInteger(Number(buildNumber))) {
    throw new Error("PRNS_IOS_BUILD_NUMBER must be a positive decimal safe integer");
  }
  const buildLabel = process.env.PRNS_BUILD_ID?.trim();
  if (buildLabel && !/^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$/.test(buildLabel)) {
    throw new Error("PRNS_BUILD_ID must be a public build label of 1–64 letters, numbers, dots, underscores or hyphens");
  }
  const multicast = variant === "production";
  const entitlements = { ...config.ios?.entitlements };
  if (multicast) entitlements["com.apple.developer.networking.multicast"] = true;
  else delete entitlements["com.apple.developer.networking.multicast"];
  const restoration = `${selection.ios}.bluetooth-auto`;
  return {
    ...config,
    name: selection.name,
    slug: selection.slug,
    version: "0.1.0",
    icon: "./assets/icon.png",
    extra: { ...config.extra, prnsBuild: { variant, ...(buildLabel ? { label: buildLabel } : {}) } },
    runtimeVersion: { policy: "fingerprint" },
    scheme: [selection.android, "rns-contact"],
    orientation: "default",
    userInterfaceStyle: "automatic",
    plugins: ["expo-router", "@prns/app-platform/app.plugin", ["expo-camera", {
      cameraPermission: "prns uses your camera to scan signed contact cards. You can also paste a contact link.",
      microphonePermission: false,
      recordAudioAndroid: false,
      barcodeScannerEnabled: true,
    }]],
    ios: {
      ...config.ios,
      bundleIdentifier: selection.ios,
      buildNumber,
      deploymentTarget: "18.0",
      supportsTablet: true,
      entitlements,
      infoPlist: {
        ...config.ios?.infoPlist,
        NSBluetoothAlwaysUsageDescription: "prns uses Bluetooth to connect to nearby Reticulum nodes.",
        NSLocalNetworkUsageDescription: "prns uses your local network to find nearby Reticulum nodes and connect to saved peers.",
        NSBonjourServices: ["_reticulum._tcp", "_reticulum._udp"],
        PRNSMulticastEnabled: multicast,
        PRNSCoreBluetoothCentralRestorationIdentifier: `${restoration}.central.v1`,
        PRNSCoreBluetoothPeripheralRestorationIdentifier: `${restoration}.peripheral.v1`,
        UIBackgroundModes: ["bluetooth-central", "bluetooth-peripheral"],
      },
    },
    android: { ...config.android, package: selection.android, allowBackup: false },
    web: { ...config.web, bundler: "metro" },
  };
};
