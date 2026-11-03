import type { BluetoothAuthorization, InstallationMode, PlatformStatus, BluetoothDiscoveryGroups, SettingsDestination } from "./types";
export type { BluetoothAuthorization, InstallationMode, PlatformStatus, BluetoothDiscoveryGroups, SettingsDestination } from "./types";

const unavailable = (): never => {
  throw new Error("The PRNS installation requires the native application.");
};
export const installationPresent = async (): Promise<boolean> => unavailable();
export const start = async (
  _mode: InstallationMode,
  _bluetooth = false,
): Promise<void> => unavailable();
export const startImported = async (_reviewId: string, _bluetooth = false): Promise<void> => unavailable();
export const stop = async (): Promise<void> => unavailable();
export const prepareReset = async (): Promise<void> => unavailable();
export const resetPreferences = async (): Promise<void> => unavailable();
export const bluetoothAuthorization = async (): Promise<BluetoothAuthorization> =>
  unavailable();

export const bluetoothSelected = async (): Promise<boolean> => unavailable();

export const status = async (): Promise<PlatformStatus> => unavailable();
export const requestBluetoothPermission = async (): Promise<PlatformStatus> => unavailable();
export const requestNotificationPermission = async (): Promise<PlatformStatus> => unavailable();
export const openSettings = async (_destination: SettingsDestination): Promise<void> => unavailable();
export const setBluetoothEnabled = async (_enabled: boolean): Promise<void> => unavailable();
export const bluetoothDiscoveryGroups = async (): Promise<BluetoothDiscoveryGroups> => unavailable();
export const setBluetoothDiscoveryGroups = async (_groups: string[]): Promise<BluetoothDiscoveryGroups> => unavailable();
