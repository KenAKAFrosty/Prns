import { requireNativeModule } from "expo-modules-core";

export type PrnsAppNativeModule = {
  readonly contractFingerprint: () => Promise<string>;
  readonly start: () => Promise<string>;
  readonly snapshot: () => Promise<string>;
  readonly initiatePairing: (inputJson: string) => Promise<string>;
  readonly approvePairing: (inputJson: string) => Promise<string>;
  readonly rejectPairing: (inputJson: string) => Promise<string>;
  readonly describeTarget: (inputJson: string) => Promise<string>;
  readonly stop: () => Promise<string>;
  readonly reset: () => Promise<string>;
};

const nativePrnsApp = requireNativeModule<PrnsAppNativeModule>("PrnsApp");

export default nativePrnsApp;
