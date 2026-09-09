import { requireNativeModule } from "expo-modules-core";
import type { EventSubscription } from "expo-modules-core";

export type PrnsAppNativeModule = {
  readonly contractFingerprint: () => Promise<string>;
  readonly hostContractFingerprint: () => Promise<string>;
  readonly inspectIdentity: () => Promise<string>;
  readonly previewIdentityImport: (identity: readonly number[]) => Promise<string>;
  readonly createGeneratedIdentity: () => Promise<string>;
  readonly createImportedIdentity: (identity: readonly number[]) => Promise<string>;
  readonly start: (inputJson: string) => Promise<string>;
  readonly snapshot: () => Promise<string>;
  readonly initiatePairing: (inputJson: string) => Promise<string>;
  readonly approvePairing: (inputJson: string) => Promise<string>;
  readonly rejectPairing: (inputJson: string) => Promise<string>;
  readonly describeTarget: (inputJson: string) => Promise<string>;
  readonly announceTarget: (inputJson: string) => Promise<string>;
  readonly saveObservedDestination: (inputJson: string) => Promise<string>;
  readonly createManualContact: (inputJson: string) => Promise<string>;
  readonly setContactAlias: (inputJson: string) => Promise<string>;
  readonly setContactPinned: (inputJson: string) => Promise<string>;
  readonly deleteContact: (inputJson: string) => Promise<string>;
  readonly getContact: (inputJson: string) => Promise<string>;
  readonly listContacts: () => Promise<string>;
  readonly listLxmfPeers: () => Promise<string>;
  readonly listLxmfMessages: (inputJson: string) => Promise<string>;
  readonly retryLxmfMessage: (inputJson: string) => Promise<string>;
  readonly cancelLxmfMessage: (inputJson: string) => Promise<string>;
  readonly announceLxmf: () => Promise<string>;
  readonly measureLxmfText: (inputJson: string) => Promise<string>;
  readonly sendDirectText: (inputJson: string) => Promise<string>;
  readonly stop: () => Promise<string>;
  readonly reset: () => Promise<string>;
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

// These wrappers describe separate platform capabilities. Only the selected
// platform provider calls its capability; the shared runtime requires neither.
export const nativeAccessorySetup = requireNativeModule<AppleAccessorySetupNativeModule>("PrnsApp");
export const nativeAndroidRuntime = requireNativeModule<AndroidRuntimeNativeModule>("PrnsApp");
const nativePrnsApp = requireNativeModule<PrnsAppNativeModule>("PrnsApp");

export default nativePrnsApp;
