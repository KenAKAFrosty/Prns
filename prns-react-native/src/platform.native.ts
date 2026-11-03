import { requireNativeModule } from "expo-modules-core";
/** Returns a private writable directory; the caller chooses identity and state policy. */
export async function defaultStoragePath(): Promise<string> {
  return requireNativeModule<{ defaultStoragePath(): Promise<string> }>("PrnsHostPlatform").defaultStoragePath();
}
