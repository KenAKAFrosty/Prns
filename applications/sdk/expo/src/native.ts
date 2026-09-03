import { requireNativeModule } from "expo-modules-core";

export type PrnsAppNativeModule = {
  readonly contractFingerprint: () => Promise<string>;
  readonly hostContractFingerprint: () => Promise<string>;
  readonly inspectIdentity: () => Promise<string>;
  readonly previewIdentityImport: (identity: readonly number[]) => Promise<string>;
  readonly createGeneratedIdentity: () => Promise<string>;
  readonly createImportedIdentity: (identity: readonly number[]) => Promise<string>;
  readonly start: () => Promise<string>;
  readonly snapshot: () => Promise<string>;
  readonly initiatePairing: (inputJson: string) => Promise<string>;
  readonly approvePairing: (inputJson: string) => Promise<string>;
  readonly rejectPairing: (inputJson: string) => Promise<string>;
  readonly describeTarget: (inputJson: string) => Promise<string>;
  readonly saveObservedDestination: (inputJson: string) => Promise<string>;
  readonly createManualContact: (inputJson: string) => Promise<string>;
  readonly setContactAlias: (inputJson: string) => Promise<string>;
  readonly setContactPinned: (inputJson: string) => Promise<string>;
  readonly deleteContact: (inputJson: string) => Promise<string>;
  readonly getContact: (inputJson: string) => Promise<string>;
  readonly listContacts: () => Promise<string>;
  readonly stop: () => Promise<string>;
  readonly reset: () => Promise<string>;
};

const nativePrnsApp = requireNativeModule<PrnsAppNativeModule>("PrnsApp");

export default nativePrnsApp;
