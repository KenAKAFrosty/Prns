import { requireNativeModule } from "expo-modules-core";
import type { AppPlatform, InstallationMode, SettingsDestination } from "./types";
export type { BluetoothAuthorization, InstallationMode, PlatformStatus, BluetoothDiscoveryGroups, SettingsDestination } from "./types";

const native = requireNativeModule<AppPlatform>("PrnsAppPlatform");

/** Presence is a path fact, not proof that the installation is healthy. */
export const installationPresent = (): Promise<boolean> =>
  native.installationPresent();

/** Bluetooth selection is explicit; ordinary startup works without it. */
export const start = (
  mode: InstallationMode,
  bluetooth = false,
): Promise<void> => native.start(mode, bluetooth);

export const startImported = (reviewId: string, bluetooth = false): Promise<void> =>
  native.startImported(reviewId, bluetooth);

/** Observes actual retained cleanup. Clients and diagnostics remain canonical. */
export const stop = (): Promise<void> => native.stop();
export const prepareReset = (): Promise<void> => native.prepareReset();
export const resetPreferences = (): Promise<void> => native.resetPreferences();
export const bluetoothAuthorization = () => native.bluetoothAuthorization();

/** Saved explicit transport choice, independent from current permission. */
export const bluetoothSelected = (): Promise<boolean> => native.bluetoothSelected();

export const status = () => native.status();
export const requestBluetoothPermission = () => native.requestBluetoothPermission();
export const requestNotificationPermission = () => native.requestNotificationPermission();
export const openSettings = (destination: SettingsDestination) => native.openSettings(destination);
export const setBluetoothEnabled = (enabled: boolean) => native.setBluetoothEnabled(enabled);
export const bluetoothDiscoveryGroups = () => native.bluetoothDiscoveryGroups();
export const setBluetoothDiscoveryGroups = (groups: string[]) => native.setBluetoothDiscoveryGroups(groups);
