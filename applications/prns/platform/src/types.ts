/** A present installation is always opened strictly; failure never selects create. */
export type InstallationMode = "create" | "open";
export type BluetoothAuthorization =
  | "notDetermined"
  | "restricted"
  | "denied"
  | "allowed";

export type BluetoothAvailability = "unknown" | "unsupported" | "off" | "locationOff" | "ready";
export type NotificationPermission = "notApplicable" | "notRequested" | "denied" | "blocked" | "enabled";
export type SettingsDestination = "application" | "bluetooth" | "location" | "notifications";
export interface PlatformStatus {
  bluetooth: {
    authorization: BluetoothAuthorization;
    availability: BluetoothAvailability;
    selected: boolean;
    attached: boolean;
  };
  notifications: NotificationPermission;
}
/** Desired settings are durable; applied is null until this host confirms settlement. */
export interface BluetoothDiscoveryGroups {
  saved: string[];
  applied: string[] | null;
}

/** Platform actions only. Domain clients and status come from generated app_owner. */
export interface AppPlatform {
  status(): Promise<PlatformStatus>;
  requestBluetoothPermission(): Promise<PlatformStatus>;
  requestNotificationPermission(): Promise<PlatformStatus>;
  openSettings(destination: SettingsDestination): Promise<void>;
  setBluetoothEnabled(enabled: boolean): Promise<void>;
  bluetoothDiscoveryGroups(): Promise<BluetoothDiscoveryGroups>;
  /** A rejected change can already be saved. Reread before offering another change. */
  setBluetoothDiscoveryGroups(groups: string[]): Promise<BluetoothDiscoveryGroups>;
  installationPresent(): Promise<boolean>;
  start(mode: InstallationMode, bluetooth: boolean): Promise<void>;
  /** Uses a process-retained reviewed import; no private key crosses this bridge. */
  startImported(reviewId: string, bluetooth: boolean): Promise<void>;
  stop(): Promise<void>;
  /** Allowed only after native reset and physical platform stop have completed. */
  resetPreferences(): Promise<void>;
  /** Capture the native private reset root without opening local data or networking. */
  prepareReset(): Promise<void>;
  bluetoothSelected(): Promise<boolean>;
  bluetoothAuthorization(): Promise<BluetoothAuthorization>;
}
