import { requireNativeModule } from "expo-modules-core";
import type { EventSubscription } from "expo-modules-core";

// Only platform-owned storage, identity and lifecycle admission cross Expo.
// Payloads use the generated UniFFI codecs over owned byte arrays.
export type PrnsAppNativeModule = {
  readonly prepareStorage: () => Promise<number[]>;
  readonly inspectIdentity: () => Promise<number[]>;
  readonly createGeneratedIdentity: () => Promise<number[]>;
  readonly createImportedIdentity: (identity: number[]) => Promise<number[]>;
  readonly start: (input: number[]) => Promise<number[]>;
  readonly stop: () => Promise<number[]>;
  readonly reset: () => Promise<number[]>;
  readonly prepareOutbound: () => Promise<void>;
};

export type AppleAccessorySetupNativeModule = {
  readonly accessorySetupStatus: () => Promise<string>;
  readonly showAccessorySetupPicker: () => Promise<string>;
  readonly addListener: (
    eventName: "onAccessorySetupStatus",
    listener: (event: { readonly status: string }) => void,
  ) => EventSubscription;
};

export type AndroidRuntimeNativeModule = {
  readonly androidRuntimeStatus: () => Promise<string>;
  readonly requestBluetoothPermissions: () => Promise<string>;
  readonly requestBackgroundBluetoothPermission: () => Promise<string>;
  readonly requestConnectionNotificationPermission: () => Promise<string>;
  readonly addListener: (
    eventName: "onAndroidRuntimeStatus",
    listener: (event: { readonly status: string }) => void,
  ) => EventSubscription;
};

// Resolving the SDK's values on web or in Expo Go must not load the player.
export function getNativePrnsApp(): PrnsAppNativeModule {
  return requireNativeModule<PrnsAppNativeModule>("PrnsApp");
}

export const nativeAccessorySetup: AppleAccessorySetupNativeModule = {
  accessorySetupStatus: () =>
    requireNativeModule<AppleAccessorySetupNativeModule>("PrnsApp").accessorySetupStatus(),
  showAccessorySetupPicker: () =>
    requireNativeModule<AppleAccessorySetupNativeModule>("PrnsApp").showAccessorySetupPicker(),
  addListener: (event, listener) =>
    requireNativeModule<AppleAccessorySetupNativeModule>("PrnsApp").addListener(event, listener),
};
export const nativeAndroidRuntime: AndroidRuntimeNativeModule = {
  androidRuntimeStatus: () =>
    requireNativeModule<AndroidRuntimeNativeModule>("PrnsApp").androidRuntimeStatus(),
  requestBluetoothPermissions: () =>
    requireNativeModule<AndroidRuntimeNativeModule>("PrnsApp").requestBluetoothPermissions(),
  requestBackgroundBluetoothPermission: () =>
    requireNativeModule<AndroidRuntimeNativeModule>(
      "PrnsApp",
    ).requestBackgroundBluetoothPermission(),
  requestConnectionNotificationPermission: () =>
    requireNativeModule<AndroidRuntimeNativeModule>(
      "PrnsApp",
    ).requestConnectionNotificationPermission(),
  addListener: (event, listener) =>
    requireNativeModule<AndroidRuntimeNativeModule>("PrnsApp").addListener(event, listener),
};
